use std::{env, net::SocketAddr, time::Duration as StdDuration};

use axum::{
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use tower_http::trace::TraceLayer;

use crate::{
    auth::{session_token_from_headers, AuthenticatedUser, CookieSecurity},
    runner_ws,
    state::{
        AppState, RunnerCommandWaitError, UniversalCommandConflict,
        UniversalCommandPersistenceError, UniversalCommandResponse, UniversalCommandStart,
    },
};

pub mod approval_rules;
pub mod approvals;
pub mod auth;
pub mod repositories;
pub mod runners;
pub mod services;
pub mod sessions;
pub mod slash;
pub mod ws;

pub const DEFAULT_BIND_ADDR: &str = "127.0.0.1:7777";
pub const DEFAULT_DEV_RUNNER_TOKEN: &str = "dev-runner-token";
const RUNNER_COMMAND_RESPONSE_TIMEOUT: StdDuration = StdDuration::from_secs(30);

pub fn app(state: AppState) -> Router {
    let sessions_router = sessions::router().merge(slash::router());
    let app_router: Router = Router::new()
        .route("/healthz", get(healthz))
        .nest("/api/auth", auth::router())
        .nest("/api/runners", runners::router())
        .nest("/api/sessions", sessions_router)
        .nest("/api/approvals", approvals::router())
        .nest("/api/approval-rules", approval_rules::router())
        .route(
            "/api/workspaces/{workspace_id}/providers/{provider_id}/sessions/refresh",
            post(sessions::refresh_workspace_provider_sessions),
        )
        .route(
            "/api/workspaces/{workspace_id}/providers/{provider_id}/sessions/refresh/{refresh_id}",
            get(sessions::workspace_provider_session_refresh_status),
        )
        .route(
            "/api/questions/{question_id}/answer",
            post(approvals::answer_question),
        )
        .route("/api/runner/ws", get(runner_ws::handler))
        .route("/api/browser/ws", get(ws::browser_ws_authenticated))
        .layer(TraceLayer::new_for_http())
        .with_state(state);
    app_router
}

pub async fn serve() -> anyhow::Result<()> {
    let bind_addr = env::var("AGENTER_BIND_ADDR").unwrap_or_else(|_| DEFAULT_BIND_ADDR.to_owned());
    let bind_addr: SocketAddr = bind_addr.parse()?;
    let runner_token = env::var("AGENTER_DEV_RUNNER_TOKEN")
        .unwrap_or_else(|_| DEFAULT_DEV_RUNNER_TOKEN.to_owned());
    let cookie_security = cookie_security_from_env();
    let bootstrap_admin = match (
        env::var("AGENTER_BOOTSTRAP_ADMIN_EMAIL"),
        env::var("AGENTER_BOOTSTRAP_ADMIN_PASSWORD"),
    ) {
        (Ok(email), Ok(password)) => Some((email, password)),
        _ => None,
    };

    tracing::info!(
        %bind_addr,
        cookie_secure = !matches!(cookie_security, CookieSecurity::DevelopmentInsecure),
        has_bootstrap_admin = bootstrap_admin.is_some(),
        has_database_url = env::var("DATABASE_URL").is_ok(),
        "starting agenter control plane"
    );

    let state = if let Ok(database_url) = env::var("DATABASE_URL") {
        tracing::info!("connecting to postgres");
        let pool = sqlx::PgPool::connect(&database_url).await?;
        tracing::info!("running database migrations");
        sqlx::migrate!("../../migrations").run(&pool).await?;
        tracing::info!("database migrations completed");
        AppState::new_with_database(runner_token, cookie_security, pool, bootstrap_admin).await?
    } else {
        tracing::warn!("DATABASE_URL not set; using in-memory development state");
        match bootstrap_admin {
            Some((email, password)) => {
                AppState::new_with_bootstrap_admin(runner_token, email, password, cookie_security)?
            }
            None => AppState::new(runner_token, cookie_security),
        }
    };

    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    tracing::info!(%bind_addr, "agenter control plane listening");
    axum::serve(listener, app(state)).await?;

    Ok(())
}

fn cookie_security_from_env() -> CookieSecurity {
    match env::var("AGENTER_COOKIE_SECURE") {
        Ok(value) if matches!(value.as_str(), "0" | "false" | "FALSE" | "False") => {
            CookieSecurity::DevelopmentInsecure
        }
        _ => CookieSecurity::Secure,
    }
}

async fn healthz() -> &'static str {
    "ok"
}

fn runner_wait_error_status(error: RunnerCommandWaitError) -> StatusCode {
    match error {
        RunnerCommandWaitError::NotConnected
        | RunnerCommandWaitError::Closed
        | RunnerCommandWaitError::StaleApproval => StatusCode::SERVICE_UNAVAILABLE,
        RunnerCommandWaitError::TimedOut => StatusCode::GATEWAY_TIMEOUT,
    }
}

fn runner_wait_error_code(error: RunnerCommandWaitError) -> &'static str {
    match error {
        RunnerCommandWaitError::NotConnected => "runner_not_connected",
        RunnerCommandWaitError::Closed => "runner_command_channel_closed",
        RunnerCommandWaitError::StaleApproval => "stale_approval",
        RunnerCommandWaitError::TimedOut => "runner_command_timeout",
    }
}

fn runner_wait_error_message(error: RunnerCommandWaitError) -> &'static str {
    match error {
        RunnerCommandWaitError::NotConnected => "Runner is not connected.",
        RunnerCommandWaitError::Closed => "Runner command channel closed.",
        RunnerCommandWaitError::StaleApproval => "Approval is no longer pending.",
        RunnerCommandWaitError::TimedOut => "Runner command timed out.",
    }
}

fn command_replay_response(
    start: Result<UniversalCommandStart, UniversalCommandPersistenceError>,
) -> Option<Response> {
    match start {
        Err(error) => Some(command_persistence_error_response(error)),
        Ok(start) => command_start_response(start),
    }
}

fn command_start_response(start: UniversalCommandStart) -> Option<Response> {
    match start {
        UniversalCommandStart::Started => None,
        UniversalCommandStart::Duplicate {
            response: Some(response),
            ..
        } => Some(stored_command_response(response)),
        UniversalCommandStart::Duplicate { .. } => Some(
            (
                StatusCode::ACCEPTED,
                Json(agenter_protocol::runner::RunnerError {
                    code: "command_pending".to_owned(),
                    message: "Command with this idempotency key is already in progress.".to_owned(),
                }),
            )
                .into_response(),
        ),
        UniversalCommandStart::Conflict(conflict) => Some(command_conflict_response(conflict)),
    }
}

fn command_persistence_error_response(error: UniversalCommandPersistenceError) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(agenter_protocol::runner::RunnerError {
            code: error.code,
            message: error.message,
        }),
    )
        .into_response()
}

fn command_finish_error_response(error: UniversalCommandPersistenceError) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(agenter_protocol::runner::RunnerError {
            code: error.code,
            message: format!(
                "{} The command side effect may already have completed; refresh before retrying.",
                error.message
            ),
        }),
    )
        .into_response()
}

async fn finish_command_or_error_response(
    state: &AppState,
    command: &agenter_core::UniversalCommandEnvelope,
    status: crate::state::UniversalCommandIdempotencyStatus,
    response: UniversalCommandResponse,
) -> Option<Response> {
    match state
        .finish_universal_command(command, status, response)
        .await
    {
        Ok(()) => None,
        Err(error) => Some(command_finish_error_response(error)),
    }
}

fn command_conflict_response(conflict: UniversalCommandConflict) -> Response {
    (
        StatusCode::CONFLICT,
        Json(agenter_protocol::runner::RunnerError {
            code: conflict.code,
            message: conflict.message,
        }),
    )
        .into_response()
}

fn stored_command_response(response: UniversalCommandResponse) -> Response {
    let status = StatusCode::from_u16(response.status).unwrap_or(StatusCode::OK);
    match response.body {
        Some(body) => (status, Json(body)).into_response(),
        None => status.into_response(),
    }
}

fn command_response(
    status: StatusCode,
    body: Option<serde_json::Value>,
) -> UniversalCommandResponse {
    UniversalCommandResponse {
        status: status.as_u16(),
        body,
    }
}

async fn publish_runner_error_event(
    state: &AppState,
    session_id: agenter_core::SessionId,
    operation: &str,
    request_id: Option<&agenter_protocol::RequestId>,
    code: &str,
    message: &str,
) {
    state
        .publish_universal_event(
            session_id,
            None,
            None,
            agenter_core::UniversalEventKind::ErrorReported {
                code: Some(code.to_owned()),
                message: message.to_owned(),
            },
        )
        .await
        .map(|_| ())
        .unwrap_or_else(|error| {
            tracing::warn!(
                %session_id,
                operation,
                request_id = request_id.map(ToString::to_string).as_deref(),
                %error,
                "failed to publish universal runner error event"
            );
        });
}

async fn publish_session_status_event(
    state: &AppState,
    session_id: agenter_core::SessionId,
    status: agenter_core::SessionStatus,
    reason: Option<String>,
) {
    state
        .publish_universal_event(
            session_id,
            None,
            None,
            agenter_core::UniversalEventKind::SessionStatusChanged { status, reason },
        )
        .await
        .map(|_| ())
        .unwrap_or_else(|error| {
            tracing::warn!(
                %session_id,
                %error,
                "failed to publish universal session status event"
            );
        });
}

async fn authenticated_user_from_headers(
    state: &AppState,
    headers: &HeaderMap,
) -> Option<AuthenticatedUser> {
    let token = session_token_from_headers(headers)?;
    state.authenticated_user(token).await
}

#[cfg(test)]
mod tests {
    use crate::auth::SESSION_COOKIE_NAME;
    use agenter_core::{
        AgentCapabilities, AgentProviderId, ApprovalDecision, ApprovalId, ApprovalKind,
        ApprovalRequestEvent, FileChangeKind, NormalizedEvent, RunnerId, SessionId, SessionInfo,
        UserMessageEvent, WorkspaceId,
    };
    use agenter_protocol::{
        browser::{BrowserClientMessage, BrowserServerMessage, SubscribeSession},
        chunk_message,
        runner::{
            AgentProviderAdvertisement, AgentUniversalEvent, RunnerCapabilities,
            RunnerClientMessage, RunnerEvent, RunnerEventEnvelope, RunnerHello,
            RunnerServerMessage, PROTOCOL_VERSION,
        },
        RequestId, RunnerTransportOutboundFrame,
    };
    type BrowserEventEnvelope = crate::state::CachedEventEnvelope;
    use axum::{
        body::{to_bytes, Body},
        http::{header, Request, StatusCode},
    };
    use chrono::Utc;
    use futures_util::{SinkExt, StreamExt};
    use tokio::net::TcpListener;
    use tokio::time::{timeout, Duration};
    use tokio_tungstenite::{
        connect_async,
        tungstenite::{client::IntoClientRequest, Message},
    };
    use tower::ServiceExt;

    use super::slash;
    use super::*;
    use crate::runner_ws::smoke_session_id;

    async fn next_runner_command(
        receiver: &mut futures_util::stream::SplitStream<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
        >,
    ) -> agenter_protocol::runner::RunnerCommandEnvelope {
        let frame = timeout(Duration::from_secs(2), receiver.next())
            .await
            .expect("runner command timeout")
            .expect("runner command frame")
            .expect("runner command websocket result");
        let Message::Text(text) = frame else {
            panic!("expected text runner command");
        };
        let RunnerServerMessage::Command(command) =
            serde_json::from_str::<RunnerServerMessage>(&text).expect("decode runner command")
        else {
            panic!("expected runner command");
        };
        *command
    }

    async fn expect_no_runner_command(
        receiver: &mut futures_util::stream::SplitStream<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
        >,
    ) {
        assert!(
            timeout(Duration::from_millis(100), receiver.next())
                .await
                .is_err(),
            "runner should not receive an unsolicited command"
        );
    }

    async fn user_message_count(
        state: &AppState,
        user_id: agenter_core::UserId,
        session_id: SessionId,
        content: &str,
    ) -> usize {
        state
            .session_history(user_id, session_id)
            .await
            .expect("session history")
            .into_iter()
            .filter(|entry| {
                matches!(
                    &entry.event,
                    NormalizedEvent::UserMessage(message) if message.content == content
                )
            })
            .count()
    }

    fn approval_request_event(
        session_id: SessionId,
        approval_id: ApprovalId,
    ) -> ApprovalRequestEvent {
        ApprovalRequestEvent {
            session_id,
            approval_id,
            kind: ApprovalKind::Command,
            title: "Run tests".to_owned(),
            details: Some("cargo test".to_owned()),
            expires_at: None,
            presentation: None,
            resolution_state: None,
            resolving_decision: None,
            status: None,
            turn_id: None,
            item_id: None,
            options: Vec::new(),
            risk: None,
            subject: None,
            native_request_id: None,
            native_blocking: true,
            policy: None,
            provider_payload: None,
        }
    }

    fn approval_test_command(
        user_id: agenter_core::UserId,
        session_id: SessionId,
        approval_id: ApprovalId,
        option_id: &str,
        feedback: Option<String>,
    ) -> agenter_core::UniversalCommandEnvelope {
        agenter_core::UniversalCommandEnvelope {
            command_id: agenter_core::CommandId::new(),
            idempotency_key: format!("uap:approval:{user_id}:{approval_id}:{option_id}"),
            session_id: Some(session_id),
            turn_id: None,
            command: agenter_core::UniversalCommand::ResolveApproval {
                approval_id,
                option_id: option_id.to_owned(),
                feedback,
            },
        }
    }

    fn approval_feedback_for_test(decision: &ApprovalDecision) -> Option<String> {
        match decision {
            ApprovalDecision::ProviderSpecific { payload } => {
                use sha2::{Digest, Sha256};
                let hash = Sha256::digest(serde_json::to_vec(payload).expect("serialize payload"));
                Some(format!("provider_specific_sha256:{hash:x}"))
            }
            _ => None,
        }
    }

    async fn send_runner_response(
        sender: &mut futures_util::stream::SplitSink<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
            Message,
        >,
        request_id: RequestId,
    ) {
        send_runner_response_result(
            sender,
            request_id,
            agenter_protocol::runner::RunnerCommandResult::Accepted,
        )
        .await;
    }

    async fn send_runner_response_result(
        sender: &mut futures_util::stream::SplitSink<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
            Message,
        >,
        request_id: RequestId,
        result: agenter_protocol::runner::RunnerCommandResult,
    ) {
        sender
            .send(Message::Text(
                serde_json::to_string(&RunnerClientMessage::Response(
                    agenter_protocol::runner::RunnerResponseEnvelope {
                        request_id,
                        outcome: agenter_protocol::runner::RunnerResponseOutcome::Ok { result },
                    },
                ))
                .expect("serialize runner response")
                .into(),
            ))
            .await
            .expect("send runner response");
    }

    async fn create_session_through_runner(
        app_service: axum::Router,
        cookie: &str,
        workspace_id: WorkspaceId,
        runner_sender: &mut futures_util::stream::SplitSink<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
            Message,
        >,
        runner_receiver: &mut futures_util::stream::SplitStream<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
        >,
    ) -> SessionInfo {
        let create_response = tokio::spawn({
            let cookie = cookie.to_owned();
            async move {
                app_service
                    .oneshot(
                        Request::builder()
                            .method("POST")
                            .uri("/api/sessions")
                            .header(header::COOKIE, &cookie)
                            .header(header::CONTENT_TYPE, "application/json")
                            .body(Body::from(
                                serde_json::json!({
                                    "workspace_id": workspace_id,
                                    "provider_id": AgentProviderId::from(AgentProviderId::CODEX),
                                    "title": "test session"
                                })
                                .to_string(),
                            ))
                            .expect("build create session request"),
                    )
                    .await
                    .expect("route create session request")
            }
        });

        let create_command = next_runner_command(runner_receiver).await;
        let agenter_protocol::runner::RunnerCommand::CreateSession(command) =
            &create_command.command
        else {
            panic!("expected create session command");
        };
        send_runner_response_result(
            runner_sender,
            create_command.request_id,
            agenter_protocol::runner::RunnerCommandResult::SessionCreated {
                session_id: command.session_id,
                external_session_id: "codex-thread-1".to_owned(),
            },
        )
        .await;

        let response = create_response.await.expect("create response task");
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read create session body");
        serde_json::from_slice(&body).expect("session info json")
    }

    #[test]
    fn slash_command_registry_is_provider_heavy_but_marks_dangerous_commands() {
        let commands = slash::local_slash_commands(&AgentProviderId::from(AgentProviderId::CODEX));

        assert!(commands.iter().any(|command| command.id == "local.help"));
        assert!(commands
            .iter()
            .any(|command| command.id == "runner.interrupt"));
        assert!(commands.iter().any(|command| command.id == "codex.compact"));
        assert!(commands.iter().any(|command| command.id == "codex.review"));
        let shell = commands
            .iter()
            .find(|command| command.id == "codex.shell")
            .expect("shell command");
        assert_eq!(
            shell.danger_level,
            agenter_core::SlashCommandDangerLevel::Dangerous
        );
        assert_eq!(shell.target, agenter_core::SlashCommandTarget::Provider);

        let qwen_commands =
            slash::local_slash_commands(&AgentProviderId::from(AgentProviderId::QWEN));
        assert!(qwen_commands
            .iter()
            .any(|command| command.id == "local.help"));
        assert!(!qwen_commands
            .iter()
            .any(|command| command.id.starts_with("codex.")));
    }

    #[tokio::test]
    async fn runner_list_reports_online_and_offline_status() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let browser_session_token = state
            .login_password("admin@example.test", "correct horse battery staple")
            .await
            .expect("login bootstrap admin");
        let mut online_hello = fake_hello();
        let online_runner_id = RunnerId::new();
        online_hello.runner_id = online_runner_id;
        online_hello.workspaces[0].runner_id = online_runner_id;
        online_hello.workspaces[0].display_name = Some("online-workspace".to_owned());
        let mut offline_hello = fake_hello();
        let offline_runner_id = RunnerId::new();
        offline_hello.runner_id = offline_runner_id;
        offline_hello.workspaces[0].runner_id = offline_runner_id;
        offline_hello.workspaces[0].display_name = Some("offline-workspace".to_owned());

        let online_runner = state
            .register_runner(
                online_hello.runner_id,
                online_hello.capabilities,
                online_hello.workspaces,
            )
            .await;
        let offline_runner = state
            .register_runner(
                offline_hello.runner_id,
                offline_hello.capabilities,
                offline_hello.workspaces,
            )
            .await;
        let (runner_sender, _runner_receiver) = tokio::sync::mpsc::unbounded_channel();
        state
            .connect_runner(online_runner.runner_id, runner_sender)
            .await;

        let response = app(state)
            .oneshot(
                Request::builder()
                    .uri("/api/runners")
                    .header(
                        header::COOKIE,
                        format!("{}={browser_session_token}", SESSION_COOKIE_NAME),
                    )
                    .body(Body::empty())
                    .expect("build runners request"),
            )
            .await
            .expect("route runners request");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read runners body");
        let runners: serde_json::Value = serde_json::from_slice(&body).expect("runner json");
        let statuses: std::collections::HashMap<_, _> = runners
            .as_array()
            .expect("runner list")
            .iter()
            .map(|runner| {
                (
                    runner["runner_id"].as_str().expect("runner id").to_owned(),
                    runner["status"].as_str().expect("runner status").to_owned(),
                )
            })
            .collect();

        assert_eq!(statuses[&online_runner.runner_id.to_string()], "online");
        assert_eq!(statuses[&offline_runner.runner_id.to_string()], "offline");
    }

    #[tokio::test]
    async fn runner_handshake_does_not_send_unsolicited_smoke_command() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let addr = listener.local_addr().expect("read listener address");
        tokio::spawn(async move {
            axum::serve(listener, app(state)).await.expect("serve app");
        });

        let (runner_socket, _) = connect_async(format!("ws://{addr}/api/runner/ws"))
            .await
            .expect("connect runner");
        let (mut runner_sender, mut runner_receiver) = runner_socket.split();
        runner_sender
            .send(Message::Text(
                serde_json::to_string(&RunnerClientMessage::Hello(fake_hello()))
                    .expect("serialize hello")
                    .into(),
            ))
            .await
            .expect("send runner hello");

        expect_no_runner_command(&mut runner_receiver).await;
    }

    #[tokio::test]
    async fn create_session_waits_for_runner_thread_and_stores_external_id() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let browser_session_token = state
            .login_password("admin@example.test", "correct horse battery staple")
            .await
            .expect("login bootstrap admin");
        let app_service = app(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let addr = listener.local_addr().expect("read listener address");
        tokio::spawn(async move {
            axum::serve(listener, app(state)).await.expect("serve app");
        });

        let (runner_socket, _) = connect_async(format!("ws://{addr}/api/runner/ws"))
            .await
            .expect("connect runner");
        let (mut runner_sender, mut runner_receiver) = runner_socket.split();
        let hello = fake_hello();
        let workspace_id = hello.workspaces[0].workspace_id;
        runner_sender
            .send(Message::Text(
                serde_json::to_string(&RunnerClientMessage::Hello(hello))
                    .expect("serialize hello")
                    .into(),
            ))
            .await
            .expect("send runner hello");
        expect_no_runner_command(&mut runner_receiver).await;

        let cookie = format!("{}={browser_session_token}", SESSION_COOKIE_NAME);
        let create_response = tokio::spawn(async move {
            app_service
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/api/sessions")
                        .header(header::COOKIE, &cookie)
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(
                            serde_json::json!({
                                "workspace_id": workspace_id,
                                "provider_id": AgentProviderId::from(AgentProviderId::CODEX),
                                "title": "codex real session"
                            })
                            .to_string(),
                        ))
                        .expect("build create session request"),
                )
                .await
                .expect("route create session request")
        });

        let create_command = next_runner_command(&mut runner_receiver).await;
        let agenter_protocol::runner::RunnerCommand::CreateSession(command) =
            &create_command.command
        else {
            panic!("expected create session command");
        };
        assert_eq!(command.workspace.workspace_id, workspace_id);
        let session_id = command.session_id;
        send_runner_response_result(
            &mut runner_sender,
            create_command.request_id,
            agenter_protocol::runner::RunnerCommandResult::SessionCreated {
                session_id,
                external_session_id: "codex-thread-1".to_owned(),
            },
        )
        .await;

        let response = create_response.await.expect("create response task");
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read create session body");
        let info: agenter_core::SessionInfo =
            serde_json::from_slice(&body).expect("session info json");
        assert_eq!(info.external_session_id.as_deref(), Some("codex-thread-1"));
    }

    #[tokio::test]
    async fn create_acp_session_waits_for_runner_and_stores_external_id() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let browser_session_token = state
            .login_password("admin@example.test", "correct horse battery staple")
            .await
            .expect("login bootstrap admin");
        let app_service = app(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let addr = listener.local_addr().expect("read listener address");
        tokio::spawn(async move {
            axum::serve(listener, app(state)).await.expect("serve app");
        });

        let (runner_socket, _) = connect_async(format!("ws://{addr}/api/runner/ws"))
            .await
            .expect("connect runner");
        let (mut runner_sender, mut runner_receiver) = runner_socket.split();
        let mut hello = fake_hello();
        hello.capabilities.agent_providers[0].provider_id =
            AgentProviderId::from(AgentProviderId::QWEN);
        hello.capabilities.agent_providers[0]
            .capabilities
            .session_resume = true;
        let workspace_id = hello.workspaces[0].workspace_id;
        runner_sender
            .send(Message::Text(
                serde_json::to_string(&RunnerClientMessage::Hello(hello))
                    .expect("serialize hello")
                    .into(),
            ))
            .await
            .expect("send runner hello");
        expect_no_runner_command(&mut runner_receiver).await;

        let cookie = format!("{}={browser_session_token}", SESSION_COOKIE_NAME);
        let create_response = tokio::spawn(async move {
            app_service
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/api/sessions")
                        .header(header::COOKIE, &cookie)
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(
                            serde_json::json!({
                                "workspace_id": workspace_id,
                                "provider_id": AgentProviderId::from(AgentProviderId::QWEN),
                                "title": "qwen real session"
                            })
                            .to_string(),
                        ))
                        .expect("build create session request"),
                )
                .await
                .expect("route create session request")
        });

        let create_command = next_runner_command(&mut runner_receiver).await;
        let agenter_protocol::runner::RunnerCommand::CreateSession(command) =
            &create_command.command
        else {
            panic!("expected create session command");
        };
        assert_eq!(command.provider_id.as_str(), AgentProviderId::QWEN);
        let session_id = command.session_id;
        send_runner_response_result(
            &mut runner_sender,
            create_command.request_id,
            agenter_protocol::runner::RunnerCommandResult::SessionCreated {
                session_id,
                external_session_id: "qwen-session-1".to_owned(),
            },
        )
        .await;

        let response = create_response.await.expect("create response task");
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read create session body");
        let info: agenter_core::SessionInfo =
            serde_json::from_slice(&body).expect("session info json");
        assert_eq!(info.provider_id.as_str(), AgentProviderId::QWEN);
        assert_eq!(info.external_session_id.as_deref(), Some("qwen-session-1"));
    }

    #[tokio::test]
    async fn refresh_workspace_sessions_sends_runner_command_and_rewrites_history() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let browser_session_token = state
            .login_password("admin@example.test", "correct horse battery staple")
            .await
            .expect("login bootstrap admin");
        let app_service = app(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let addr = listener.local_addr().expect("read listener address");
        tokio::spawn(async move {
            axum::serve(listener, app(state)).await.expect("serve app");
        });

        let (runner_socket, _) = connect_async(format!("ws://{addr}/api/runner/ws"))
            .await
            .expect("connect runner");
        let (mut runner_sender, mut runner_receiver) = runner_socket.split();
        let hello = fake_hello();
        let workspace = hello.workspaces[0].clone();
        runner_sender
            .send(Message::Text(
                serde_json::to_string(&RunnerClientMessage::Hello(hello))
                    .expect("serialize hello")
                    .into(),
            ))
            .await
            .expect("send runner hello");
        expect_no_runner_command(&mut runner_receiver).await;

        let cookie = format!("{}={browser_session_token}", SESSION_COOKIE_NAME);
        let session = create_session_through_runner(
            app_service.clone(),
            &cookie,
            workspace.workspace_id,
            &mut runner_sender,
            &mut runner_receiver,
        )
        .await;

        let refresh_response = tokio::spawn({
            let app_service = app_service.clone();
            let cookie = cookie.clone();
            let uri = format!(
                "/api/workspaces/{}/providers/{}/sessions/refresh",
                workspace.workspace_id,
                AgentProviderId::CODEX
            );
            async move {
                app_service
                    .oneshot(
                        Request::builder()
                            .method("POST")
                            .uri(uri)
                            .header(header::COOKIE, &cookie)
                            .body(Body::empty())
                            .expect("build refresh request"),
                    )
                    .await
                    .expect("route refresh request")
            }
        });

        let refresh_command = next_runner_command(&mut runner_receiver).await;
        let agenter_protocol::runner::RunnerCommand::RefreshSessions(command) =
            &refresh_command.command
        else {
            panic!("expected refresh sessions command");
        };
        assert_eq!(command.workspace.workspace_id, workspace.workspace_id);
        assert_eq!(command.provider_id.as_str(), AgentProviderId::CODEX);
        runner_sender
            .send(Message::Text(
                serde_json::to_string(&RunnerClientMessage::Event(Box::new(RunnerEventEnvelope {
                    request_id: Some(refresh_command.request_id.clone()),
                    runner_event_seq: None,
                    acked_runner_event_seq: None,
                    event: RunnerEvent::SessionsDiscovered(agenter_protocol::runner::DiscoveredSessions {
                        workspace: workspace.clone(),
                        provider_id: AgentProviderId::from(AgentProviderId::CODEX),
                        sessions: vec![agenter_protocol::runner::DiscoveredSession {
                            external_session_id: "codex-thread-1".to_owned(),
                            title: Some("Refreshed".to_owned()),
                            updated_at: None,
                            history_status: agenter_protocol::runner::DiscoveredSessionHistoryStatus::Loaded,
                            history: vec![agenter_protocol::runner::DiscoveredSessionHistoryItem::AgentMessage {
                                message_id: "agent-new".to_owned(),
                                content: "new history".to_owned(),
                            }],
                        }],
                    }),
                })))
                .expect("serialize discovered event")
                .into(),
            ))
            .await
            .expect("send discovered event");
        tokio::time::sleep(Duration::from_millis(20)).await;
        let refresh_id = refresh_command.request_id.clone();
        send_runner_response(&mut runner_sender, refresh_command.request_id).await;

        let response = refresh_response.await.expect("refresh response task");
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read refresh body");
        let accepted: serde_json::Value = serde_json::from_slice(&body).expect("refresh json");
        assert_eq!(accepted["refresh_id"], refresh_id.to_string());
        assert_eq!(accepted["status"], "queued");

        let mut status = None;
        let mut last_status = None;
        for _ in 0..100 {
            let status_response = app_service
                .clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(format!(
                            "/api/workspaces/{}/providers/{}/sessions/refresh/{}",
                            workspace.workspace_id,
                            AgentProviderId::CODEX,
                            refresh_id
                        ))
                        .header(header::COOKIE, &cookie)
                        .body(Body::empty())
                        .expect("build refresh status request"),
                )
                .await
                .expect("route refresh status request");
            assert_eq!(status_response.status(), StatusCode::OK);
            let body = to_bytes(status_response.into_body(), usize::MAX)
                .await
                .expect("read refresh status body");
            let value: serde_json::Value =
                serde_json::from_slice(&body).expect("refresh status json");
            if value["status"] == "succeeded" {
                status = Some(value);
                break;
            }
            last_status = Some(value);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let status = status.unwrap_or_else(|| {
            panic!(
                "refresh status eventually succeeds, last status: {:?}",
                last_status
            )
        });
        assert_eq!(status["status"], "succeeded");
        assert_eq!(status["summary"]["discovered_count"], 1);
        assert_eq!(status["summary"]["refreshed_cache_count"], 1);
        assert_eq!(status["summary"]["skipped_failed_count"], 0);

        let history_response = app_service
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/sessions/{}/history", session.session_id))
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .expect("build history request"),
            )
            .await
            .expect("route history request");
        assert_eq!(history_response.status(), StatusCode::OK);
        let history_body = to_bytes(history_response.into_body(), usize::MAX)
            .await
            .expect("read history body");
        let history: Vec<BrowserEventEnvelope> =
            serde_json::from_slice(&history_body).expect("history json");
        assert_eq!(history.len(), 1);
        assert!(matches!(
            history[0].event,
            NormalizedEvent::AgentMessageCompleted(_)
        ));
    }

    async fn next_browser_message(
        receiver: &mut futures_util::stream::SplitStream<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
        >,
    ) -> BrowserServerMessage {
        loop {
            let frame = timeout(Duration::from_secs(2), receiver.next())
                .await
                .expect("browser message timeout")
                .expect("browser message frame")
                .expect("browser message websocket result");
            let Message::Text(text) = frame else {
                continue;
            };
            return serde_json::from_str::<BrowserServerMessage>(&text)
                .expect("decode browser message");
        }
    }

    #[tokio::test]
    async fn smoke_routes_runner_events_to_subscribed_browser() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let browser_session_token = state
            .login_password("admin@example.test", "correct horse battery staple")
            .await
            .expect("login bootstrap admin");
        let app_service = app(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let addr = listener.local_addr().expect("read listener address");
        tokio::spawn(async move {
            axum::serve(listener, app(state)).await.expect("serve app");
        });

        let (runner_socket, _) = connect_async(format!("ws://{addr}/api/runner/ws"))
            .await
            .expect("connect runner");
        let (mut runner_sender, mut runner_receiver) = runner_socket.split();
        let hello = fake_hello();
        let workspace_id = hello.workspaces[0].workspace_id;
        runner_sender
            .send(Message::Text(
                serde_json::to_string(&RunnerClientMessage::Hello(hello))
                    .expect("serialize hello")
                    .into(),
            ))
            .await
            .expect("send runner hello");
        expect_no_runner_command(&mut runner_receiver).await;

        let cookie = format!("{}={browser_session_token}", SESSION_COOKIE_NAME);
        let session = create_session_through_runner(
            app_service,
            &cookie,
            workspace_id,
            &mut runner_sender,
            &mut runner_receiver,
        )
        .await;

        let mut browser_request = format!("ws://{addr}/api/browser/ws")
            .into_client_request()
            .expect("build browser websocket request");
        browser_request
            .headers_mut()
            .insert(header::COOKIE, cookie.parse().expect("cookie header"));
        let (browser_socket, _) = connect_async(browser_request)
            .await
            .expect("connect browser");
        let (mut browser_sender, mut browser_receiver) = browser_socket.split();
        browser_sender
            .send(Message::Text(
                serde_json::to_string(&BrowserClientMessage::SubscribeSession(SubscribeSession {
                    request_id: Some(RequestId::from("sub-1")),
                    session_id: session.session_id,
                    after_seq: None,
                    include_snapshot: false,
                }))
                .expect("serialize subscribe")
                .into(),
            ))
            .await
            .expect("send browser subscription");

        let browser_ack = browser_receiver
            .next()
            .await
            .expect("browser ack frame")
            .expect("browser ack websocket result");
        assert!(matches!(
            serde_json::from_str::<BrowserServerMessage>(browser_ack.to_text().unwrap()),
            Ok(BrowserServerMessage::Ack(_))
        ));

        runner_sender
            .send(Message::Text(
                serde_json::to_string(&RunnerClientMessage::Event(Box::new(RunnerEventEnvelope {
                    request_id: Some(RequestId::from("event-1")),
                    runner_event_seq: None,
                    acked_runner_event_seq: None,
                    event: RunnerEvent::AgentEvent(Box::new(AgentUniversalEvent {
                        protocol_version: agenter_core::UNIVERSAL_PROTOCOL_VERSION.to_owned(),
                        session_id: session.session_id,
                        event_id: None,
                        turn_id: None,
                        item_id: None,
                        ts: None,
                        source: agenter_core::UniversalEventSource::Native,
                        native: None,
                        event: agenter_core::UniversalEventKind::NativeUnknown {
                            summary: Some("hello browser".to_owned()),
                        },
                    })),
                })))
                .expect("serialize runner event")
                .into(),
            ))
            .await
            .expect("send runner event");

        loop {
            let browser_event = browser_receiver
                .next()
                .await
                .expect("browser event frame")
                .expect("browser event websocket result");
            if matches!(
                serde_json::from_str::<BrowserServerMessage>(browser_event.to_text().unwrap()),
                Ok(BrowserServerMessage::UniversalEvent(_))
            ) {
                break;
            }
        }
    }

    #[tokio::test]
    async fn subscribe_truncated_universal_snapshot_continues_with_live_events() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let user_id = state.bootstrap_user_id().expect("bootstrap user");
        let runner = fake_hello();
        let workspace = runner.workspaces[0].clone();
        state
            .register_runner(
                runner.runner_id,
                runner.capabilities.clone(),
                runner.workspaces.clone(),
            )
            .await;
        let session = state
            .create_session(
                SessionId::new(),
                user_id,
                runner.runner_id,
                workspace,
                AgentProviderId::from(AgentProviderId::CODEX),
            )
            .await;
        for i in 0..1026 {
            state
                .publish_event(
                    session.session_id,
                    NormalizedEvent::UserMessage(UserMessageEvent {
                        session_id: session.session_id,
                        message_id: Some(format!("replay-{i}")),
                        author_user_id: None,
                        content: "replay".to_owned(),
                    }),
                )
                .await;
        }

        let browser_session_token = state
            .login_password("admin@example.test", "correct horse battery staple")
            .await
            .expect("login bootstrap admin");
        let server_state = state.clone();
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let addr = listener.local_addr().expect("read listener address");
        tokio::spawn(async move {
            axum::serve(listener, app(server_state))
                .await
                .expect("serve app");
        });

        let mut browser_request = format!("ws://{addr}/api/browser/ws")
            .into_client_request()
            .expect("build browser websocket request");
        browser_request.headers_mut().insert(
            header::COOKIE,
            format!("{}={browser_session_token}", SESSION_COOKIE_NAME)
                .parse()
                .expect("cookie header"),
        );
        let (browser_socket, _) = connect_async(browser_request)
            .await
            .expect("connect browser");
        let (mut browser_sender, mut browser_receiver) = browser_socket.split();
        browser_sender
            .send(Message::Text(
                serde_json::to_string(&BrowserClientMessage::SubscribeSession(SubscribeSession {
                    request_id: Some(RequestId::from("sub-truncated")),
                    session_id: session.session_id,
                    after_seq: Some(agenter_core::UniversalSeq::zero()),
                    include_snapshot: true,
                }))
                .expect("serialize subscribe")
                .into(),
            ))
            .await
            .expect("send browser subscription");

        assert!(matches!(
            next_browser_message(&mut browser_receiver).await,
            BrowserServerMessage::Ack(_)
        ));
        let BrowserServerMessage::SessionSnapshot(snapshot) =
            next_browser_message(&mut browser_receiver).await
        else {
            panic!("expected session snapshot");
        };
        assert!(snapshot.has_more);
        state
            .publish_event(
                session.session_id,
                NormalizedEvent::UserMessage(UserMessageEvent {
                    session_id: session.session_id,
                    message_id: Some("live-after-truncated-snapshot".to_owned()),
                    author_user_id: None,
                    content: "live".to_owned(),
                }),
            )
            .await;
        let BrowserServerMessage::UniversalEvent(event) =
            next_browser_message(&mut browser_receiver).await
        else {
            panic!("expected live universal event after truncated snapshot");
        };
        assert!(
            event.seq > snapshot.latest_seq.expect("truncated replay cursor"),
            "live event should advance beyond the truncated replay cursor"
        );
    }

    #[tokio::test]
    async fn subscribe_truncated_replay_without_snapshot_stops_before_live_events() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let user_id = state.bootstrap_user_id().expect("bootstrap user");
        let runner = fake_hello();
        let workspace = runner.workspaces[0].clone();
        state
            .register_runner(
                runner.runner_id,
                runner.capabilities.clone(),
                runner.workspaces.clone(),
            )
            .await;
        let session = state
            .create_session(
                SessionId::new(),
                user_id,
                runner.runner_id,
                workspace,
                AgentProviderId::from(AgentProviderId::CODEX),
            )
            .await;
        for i in 0..1026 {
            state
                .publish_event(
                    session.session_id,
                    NormalizedEvent::UserMessage(UserMessageEvent {
                        session_id: session.session_id,
                        message_id: Some(format!("replay-only-{i}")),
                        author_user_id: None,
                        content: "replay".to_owned(),
                    }),
                )
                .await;
        }

        let browser_session_token = state
            .login_password("admin@example.test", "correct horse battery staple")
            .await
            .expect("login bootstrap admin");
        let server_state = state.clone();
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let addr = listener.local_addr().expect("read listener address");
        tokio::spawn(async move {
            axum::serve(listener, app(server_state))
                .await
                .expect("serve app");
        });

        let mut browser_request = format!("ws://{addr}/api/browser/ws")
            .into_client_request()
            .expect("build browser websocket request");
        browser_request.headers_mut().insert(
            header::COOKIE,
            format!("{}={browser_session_token}", SESSION_COOKIE_NAME)
                .parse()
                .expect("cookie header"),
        );
        let (browser_socket, _) = connect_async(browser_request)
            .await
            .expect("connect browser");
        let (mut browser_sender, mut browser_receiver) = browser_socket.split();
        browser_sender
            .send(Message::Text(
                serde_json::to_string(&BrowserClientMessage::SubscribeSession(SubscribeSession {
                    request_id: Some(RequestId::from("sub-truncated-replay-only")),
                    session_id: session.session_id,
                    after_seq: Some(agenter_core::UniversalSeq::zero()),
                    include_snapshot: false,
                }))
                .expect("serialize subscribe")
                .into(),
            ))
            .await
            .expect("send browser subscription");

        assert!(matches!(
            next_browser_message(&mut browser_receiver).await,
            BrowserServerMessage::Ack(_)
        ));
        let BrowserServerMessage::SessionSnapshot(snapshot) =
            next_browser_message(&mut browser_receiver).await
        else {
            panic!("expected replay frame");
        };
        assert!(snapshot.has_more);
        let safe_latest_seq = snapshot.latest_seq;
        state
            .publish_event(
                session.session_id,
                NormalizedEvent::UserMessage(UserMessageEvent {
                    session_id: session.session_id,
                    message_id: Some("live-after-truncated-replay-only".to_owned()),
                    author_user_id: None,
                    content: "live".to_owned(),
                }),
            )
            .await;
        let BrowserServerMessage::Error(error) = next_browser_message(&mut browser_receiver).await
        else {
            panic!("expected replay incomplete error");
        };
        assert_eq!(error.code, "snapshot_replay_incomplete");
        assert!(
            error.message.contains("snapshot.latest_seq"),
            "error should instruct clients to resume from the replay cursor: {}",
            error.message
        );
        assert!(
            error.message.contains(&format!("{safe_latest_seq:?}")),
            "error should not point at a later live cursor: {}",
            error.message
        );

        let next = timeout(Duration::from_millis(250), browser_receiver.next())
            .await
            .expect("subscription should close instead of waiting for live universal events");
        assert!(
            matches!(next, None | Some(Ok(Message::Close(_))) | Some(Err(_))),
            "truncated replay-only subscription must not deliver live events: {next:?}"
        );
    }

    #[tokio::test]
    async fn session_rest_apis_list_history_and_send_message_to_connected_runner() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let browser_session_token = state
            .login_password("admin@example.test", "correct horse battery staple")
            .await
            .expect("login bootstrap admin");
        let app_service = app(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let addr = listener.local_addr().expect("read listener address");
        tokio::spawn(async move {
            axum::serve(listener, app(state)).await.expect("serve app");
        });

        let (runner_socket, _) = connect_async(format!("ws://{addr}/api/runner/ws"))
            .await
            .expect("connect runner");
        let (mut runner_sender, mut runner_receiver) = runner_socket.split();
        let hello = fake_hello();
        let workspace_id = hello.workspaces[0].workspace_id;
        runner_sender
            .send(Message::Text(
                serde_json::to_string(&RunnerClientMessage::Hello(hello))
                    .expect("serialize hello")
                    .into(),
            ))
            .await
            .expect("send runner hello");
        expect_no_runner_command(&mut runner_receiver).await;

        let cookie = format!("{}={browser_session_token}", SESSION_COOKIE_NAME);
        let session = create_session_through_runner(
            app_service.clone(),
            &cookie,
            workspace_id,
            &mut runner_sender,
            &mut runner_receiver,
        )
        .await;
        let sessions_response = app_service
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/sessions")
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .expect("build sessions request"),
            )
            .await
            .expect("route sessions request");
        assert_eq!(sessions_response.status(), StatusCode::OK);
        let sessions_body = to_bytes(sessions_response.into_body(), usize::MAX)
            .await
            .expect("read sessions body");
        let sessions: Vec<SessionInfo> =
            serde_json::from_slice(&sessions_body).expect("sessions json");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, session.session_id);

        let send_response = tokio::spawn({
            let app_service = app_service.clone();
            let cookie = cookie.clone();
            let session_id = session.session_id;
            async move {
                app_service
                    .oneshot(
                        Request::builder()
                            .method("POST")
                            .uri(format!("/api/sessions/{session_id}/messages"))
                            .header(header::COOKIE, &cookie)
                            .header(header::CONTENT_TYPE, "application/json")
                            .body(Body::from(
                                serde_json::json!({ "content": "from browser" }).to_string(),
                            ))
                            .expect("build message request"),
                    )
                    .await
                    .expect("route message request")
            }
        });

        let command = next_runner_command(&mut runner_receiver).await;
        let agenter_protocol::runner::RunnerCommand::AgentSendInput(input_command) =
            &command.command
        else {
            panic!(
                "expected agent send input command, got {:?}",
                command.command
            );
        };
        assert_eq!(input_command.session_id, session.session_id);
        assert_eq!(
            input_command.external_session_id.as_deref(),
            Some("codex-thread-1")
        );
        send_runner_response(&mut runner_sender, command.request_id).await;
        let send_response = send_response.await.expect("send response task");
        assert_eq!(send_response.status(), StatusCode::ACCEPTED);

        let history_response = app_service
            .oneshot(
                Request::builder()
                    .uri(format!("/api/sessions/{}/history", session.session_id))
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .expect("build history request"),
            )
            .await
            .expect("route history request");
        assert_eq!(history_response.status(), StatusCode::OK);
        let history_body = to_bytes(history_response.into_body(), usize::MAX)
            .await
            .expect("read history body");
        let history: Vec<BrowserEventEnvelope> =
            serde_json::from_slice(&history_body).expect("history json");
        assert!(
            history.is_empty(),
            "universal-only create/message flow should not backfill cached normalized history"
        );
    }

    #[tokio::test]
    async fn send_session_message_with_settings_override_persists_and_forwards_to_runner() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let browser_session_token = state
            .login_password("admin@example.test", "correct horse battery staple")
            .await
            .expect("login bootstrap admin");
        let user = state
            .authenticated_user(&browser_session_token)
            .await
            .expect("authenticate bootstrap admin");
        let app_service = app(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let addr = listener.local_addr().expect("read listener address");
        tokio::spawn(async move {
            axum::serve(listener, app(state.clone()))
                .await
                .expect("serve app");
        });

        let (runner_socket, _) = connect_async(format!("ws://{addr}/api/runner/ws"))
            .await
            .expect("connect runner");
        let (mut runner_sender, mut runner_receiver) = runner_socket.split();
        let hello = fake_hello();
        let workspace_id = hello.workspaces[0].workspace_id;
        runner_sender
            .send(Message::Text(
                serde_json::to_string(&RunnerClientMessage::Hello(hello))
                    .expect("serialize hello")
                    .into(),
            ))
            .await
            .expect("send runner hello");
        expect_no_runner_command(&mut runner_receiver).await;

        let cookie = format!("{}={browser_session_token}", SESSION_COOKIE_NAME);
        let session = create_session_through_runner(
            app_service.clone(),
            &cookie,
            workspace_id,
            &mut runner_sender,
            &mut runner_receiver,
        )
        .await;

        let app_state_handle = app_service.clone();
        let send_response = tokio::spawn({
            let app_service = app_state_handle.clone();
            let cookie = cookie.clone();
            let session_id = session.session_id;
            async move {
                app_service
                    .oneshot(
                        Request::builder()
                            .method("POST")
                            .uri(format!("/api/sessions/{session_id}/messages"))
                            .header(header::COOKIE, &cookie)
                            .header(header::CONTENT_TYPE, "application/json")
                            .body(Body::from(
                                serde_json::json!({
                                    "content": "Implement the plan.",
                                    "settings_override": {
                                        "collaboration_mode": "default"
                                    }
                                })
                                .to_string(),
                            ))
                            .expect("build message request"),
                    )
                    .await
                    .expect("route message request")
            }
        });

        let command = next_runner_command(&mut runner_receiver).await;
        let agenter_protocol::runner::RunnerCommand::AgentSendInput(input_command) =
            &command.command
        else {
            panic!("expected agent send input command");
        };
        assert_eq!(input_command.session_id, session.session_id);
        let forwarded = input_command
            .settings
            .as_ref()
            .expect("settings forwarded with override");
        assert_eq!(forwarded.collaboration_mode.as_deref(), Some("default"));
        send_runner_response(&mut runner_sender, command.request_id).await;
        let send_response = send_response.await.expect("send response task");
        assert_eq!(send_response.status(), StatusCode::ACCEPTED);

        let settings_response = app_service
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/sessions/{}/settings", session.session_id))
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .expect("build settings request"),
            )
            .await
            .expect("route settings request");
        assert_eq!(settings_response.status(), StatusCode::OK);
        let body = to_bytes(settings_response.into_body(), usize::MAX)
            .await
            .expect("read settings body");
        let persisted: agenter_core::AgentTurnSettings =
            serde_json::from_slice(&body).expect("settings json");
        assert_eq!(persisted.collaboration_mode.as_deref(), Some("default"));
        let _ = user;
    }

    #[tokio::test]
    async fn idempotent_send_session_message_replays_rest_response() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let user_id = state.bootstrap_user_id().expect("bootstrap user");
        let runner = fake_hello();
        let workspace = runner.workspaces[0].clone();
        state
            .register_runner(
                runner.runner_id,
                runner.capabilities.clone(),
                runner.workspaces.clone(),
            )
            .await;
        let (runner_sender, mut runner_receiver) = tokio::sync::mpsc::unbounded_channel();
        state.connect_runner(runner.runner_id, runner_sender).await;
        let session = state
            .create_session(
                SessionId::new(),
                user_id,
                runner.runner_id,
                workspace,
                AgentProviderId::from(AgentProviderId::CODEX),
            )
            .await;
        let browser_session_token = state
            .login_password("admin@example.test", "correct horse battery staple")
            .await
            .expect("login bootstrap admin");
        let app = app(state.clone());
        let request_body = serde_json::json!({
            "content": "from browser",
            "idempotency_key": "send-message-idempotency-1"
        })
        .to_string();

        let first = tokio::spawn({
            let app = app.clone();
            let request_body = request_body.clone();
            let browser_session_token = browser_session_token.clone();
            async move {
                app.oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(format!("/api/sessions/{}/messages", session.session_id))
                        .header(
                            header::COOKIE,
                            format!("{}={browser_session_token}", SESSION_COOKIE_NAME),
                        )
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(request_body))
                        .expect("build send request"),
                )
                .await
                .expect("route send request")
            }
        });
        let outbound = runner_receiver.recv().await.expect("runner command");
        let RunnerServerMessage::Command(command) = outbound.message else {
            panic!("expected runner command");
        };
        outbound
            .delivered
            .send(Ok(()))
            .expect("report runner delivery");
        state
            .finish_runner_response(
                runner.runner_id,
                command.request_id,
                agenter_protocol::runner::RunnerResponseOutcome::Ok {
                    result: agenter_protocol::runner::RunnerCommandResult::Accepted,
                },
            )
            .await;
        let first = first.await.expect("join first send");
        assert_eq!(first.status(), StatusCode::ACCEPTED);

        let duplicate = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/sessions/{}/messages", session.session_id))
                    .header(
                        header::COOKIE,
                        format!("{}={browser_session_token}", SESSION_COOKIE_NAME),
                    )
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(request_body))
                    .expect("build duplicate send request"),
            )
            .await
            .expect("route duplicate send request");
        assert_eq!(duplicate.status(), StatusCode::ACCEPTED);
        assert!(
            runner_receiver.try_recv().is_err(),
            "duplicate same-key send must not dispatch another runner command"
        );
    }

    #[tokio::test]
    async fn idempotent_send_message_conflicts_when_settings_override_changes() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let user_id = state.bootstrap_user_id().expect("bootstrap user");
        let runner = fake_hello();
        let workspace = runner.workspaces[0].clone();
        state
            .register_runner(
                runner.runner_id,
                runner.capabilities.clone(),
                runner.workspaces.clone(),
            )
            .await;
        let (runner_sender, mut runner_receiver) = tokio::sync::mpsc::unbounded_channel();
        state.connect_runner(runner.runner_id, runner_sender).await;
        let session = state
            .create_session(
                SessionId::new(),
                user_id,
                runner.runner_id,
                workspace,
                AgentProviderId::from(AgentProviderId::CODEX),
            )
            .await;
        let browser_session_token = state
            .login_password("admin@example.test", "correct horse battery staple")
            .await
            .expect("login bootstrap admin");
        let app = app(state.clone());
        let first_body = serde_json::json!({
            "content": "same text",
            "idempotency_key": "send-message-settings-key",
            "settings_override": { "collaboration_mode": "plan" }
        })
        .to_string();

        let first = tokio::spawn({
            let app = app.clone();
            let token = browser_session_token.clone();
            async move {
                app.oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(format!("/api/sessions/{}/messages", session.session_id))
                        .header(header::COOKIE, format!("{}={token}", SESSION_COOKIE_NAME))
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(first_body))
                        .expect("build first send"),
                )
                .await
                .expect("route first send")
            }
        });
        let outbound = runner_receiver.recv().await.expect("runner command");
        let RunnerServerMessage::Command(command) = outbound.message else {
            panic!("expected runner command");
        };
        outbound.delivered.send(Ok(())).expect("runner delivered");
        state
            .finish_runner_response(
                runner.runner_id,
                command.request_id,
                agenter_protocol::runner::RunnerResponseOutcome::Ok {
                    result: agenter_protocol::runner::RunnerCommandResult::Accepted,
                },
            )
            .await;
        assert_eq!(
            first.await.expect("join first").status(),
            StatusCode::ACCEPTED
        );

        let conflict = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/sessions/{}/messages", session.session_id))
                    .header(
                        header::COOKIE,
                        format!("{}={browser_session_token}", SESSION_COOKIE_NAME),
                    )
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "content": "same text",
                            "idempotency_key": "send-message-settings-key",
                            "settings_override": { "collaboration_mode": "default" }
                        })
                        .to_string(),
                    ))
                    .expect("build conflicting send"),
            )
            .await
            .expect("route conflicting send");
        assert_eq!(conflict.status(), StatusCode::CONFLICT);
        assert!(runner_receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn idempotent_question_answer_replays_after_question_is_resolved_and_conflicts() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let user_id = state.bootstrap_user_id().expect("bootstrap user");
        let runner = fake_hello();
        let workspace = runner.workspaces[0].clone();
        state
            .register_runner(
                runner.runner_id,
                runner.capabilities.clone(),
                runner.workspaces.clone(),
            )
            .await;
        let (runner_sender, mut runner_receiver) = tokio::sync::mpsc::unbounded_channel();
        state.connect_runner(runner.runner_id, runner_sender).await;
        let session = state
            .create_session(
                SessionId::new(),
                user_id,
                runner.runner_id,
                workspace,
                AgentProviderId::from(AgentProviderId::CODEX),
            )
            .await;
        let question_id = agenter_core::QuestionId::new();
        state
            .publish_event(
                session.session_id,
                NormalizedEvent::QuestionRequested(agenter_core::QuestionRequestedEvent {
                    session_id: session.session_id,
                    question_id,
                    title: "Pick target".to_owned(),
                    description: None,
                    fields: Vec::new(),
                    provider_payload: None,
                }),
            )
            .await;
        let token = state
            .login_password("admin@example.test", "correct horse battery staple")
            .await
            .expect("login bootstrap admin");
        let app = app(state.clone());
        let answer = serde_json::json!({
            "question_id": question_id,
            "answers": { "target": ["browser"] }
        })
        .to_string();

        let first = tokio::spawn({
            let app = app.clone();
            let token = token.clone();
            async move {
                app.oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(format!("/api/questions/{question_id}/answer"))
                        .header(header::COOKIE, format!("{}={token}", SESSION_COOKIE_NAME))
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(answer))
                        .expect("build answer"),
                )
                .await
                .expect("route answer")
            }
        });
        let outbound = runner_receiver.recv().await.expect("runner command");
        let RunnerServerMessage::Command(command) = outbound.message else {
            panic!("expected runner command");
        };
        outbound.delivered.send(Ok(())).expect("runner delivered");
        state
            .finish_runner_response(
                runner.runner_id,
                command.request_id,
                agenter_protocol::runner::RunnerResponseOutcome::Ok {
                    result: agenter_protocol::runner::RunnerCommandResult::Accepted,
                },
            )
            .await;
        let first = first.await.expect("join answer");
        assert_eq!(first.status(), StatusCode::OK);
        let first_body = to_bytes(first.into_body(), usize::MAX)
            .await
            .expect("read first answer body");
        let first_envelope: BrowserEventEnvelope =
            serde_json::from_slice(&first_body).expect("answer envelope");

        let duplicate = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/questions/{question_id}/answer"))
                    .header(header::COOKIE, format!("{}={token}", SESSION_COOKIE_NAME))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "question_id": question_id,
                            "answers": { "target": ["browser"] }
                        })
                        .to_string(),
                    ))
                    .expect("build duplicate answer"),
            )
            .await
            .expect("route duplicate answer");
        assert_eq!(duplicate.status(), StatusCode::OK);
        let duplicate_body = to_bytes(duplicate.into_body(), usize::MAX)
            .await
            .expect("read duplicate answer body");
        let duplicate_envelope: BrowserEventEnvelope =
            serde_json::from_slice(&duplicate_body).expect("duplicate envelope");
        assert_eq!(duplicate_envelope.event_id, first_envelope.event_id);
        assert!(runner_receiver.try_recv().is_err());

        let conflict = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/questions/{question_id}/answer"))
                    .header(header::COOKIE, format!("{}={token}", SESSION_COOKIE_NAME))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "question_id": question_id,
                            "answers": { "target": ["runner"] }
                        })
                        .to_string(),
                    ))
                    .expect("build conflicting answer"),
            )
            .await
            .expect("route conflicting answer");
        assert_eq!(conflict.status(), StatusCode::CONFLICT);
        assert!(runner_receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn idempotent_slash_interrupt_replays_and_conflicts() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let user_id = state.bootstrap_user_id().expect("bootstrap user");
        let runner = fake_hello();
        let workspace = runner.workspaces[0].clone();
        state
            .register_runner(
                runner.runner_id,
                runner.capabilities.clone(),
                runner.workspaces.clone(),
            )
            .await;
        let (runner_sender, mut runner_receiver) = tokio::sync::mpsc::unbounded_channel();
        state.connect_runner(runner.runner_id, runner_sender).await;
        let session = state
            .create_session(
                SessionId::new(),
                user_id,
                runner.runner_id,
                workspace,
                AgentProviderId::from(AgentProviderId::CODEX),
            )
            .await;
        let token = state
            .login_password("admin@example.test", "correct horse battery staple")
            .await
            .expect("login bootstrap admin");
        let app = app(state.clone());
        let body = serde_json::json!({
            "command_id": "runner.interrupt",
            "raw_input": "/interrupt",
            "arguments": {},
            "confirmed": true,
            "idempotency_key": "slash-interrupt-key"
        })
        .to_string();

        let first = tokio::spawn({
            let app = app.clone();
            let token = token.clone();
            let body = body.clone();
            async move {
                app.oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(format!(
                            "/api/sessions/{}/slash-commands",
                            session.session_id
                        ))
                        .header(header::COOKIE, format!("{}={token}", SESSION_COOKIE_NAME))
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(body))
                        .expect("build interrupt"),
                )
                .await
                .expect("route interrupt")
            }
        });
        let outbound = runner_receiver.recv().await.expect("runner command");
        let RunnerServerMessage::Command(command) = outbound.message else {
            panic!("expected runner command");
        };
        assert!(matches!(
            command.command,
            agenter_protocol::runner::RunnerCommand::InterruptSession { .. }
        ));
        outbound.delivered.send(Ok(())).expect("runner delivered");
        state
            .finish_runner_response(
                runner.runner_id,
                command.request_id,
                agenter_protocol::runner::RunnerResponseOutcome::Ok {
                    result: agenter_protocol::runner::RunnerCommandResult::Accepted,
                },
            )
            .await;
        assert_eq!(
            first.await.expect("join interrupt").status(),
            StatusCode::OK
        );
        assert_eq!(
            user_message_count(&state, user_id, session.session_id, "/interrupt").await,
            1
        );

        let duplicate = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/sessions/{}/slash-commands",
                        session.session_id
                    ))
                    .header(header::COOKIE, format!("{}={token}", SESSION_COOKIE_NAME))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .expect("build duplicate interrupt"),
            )
            .await
            .expect("route duplicate interrupt");
        assert_eq!(duplicate.status(), StatusCode::OK);
        assert!(runner_receiver.try_recv().is_err());
        assert_eq!(
            user_message_count(&state, user_id, session.session_id, "/interrupt").await,
            1
        );

        let conflict = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/sessions/{}/slash-commands",
                        session.session_id
                    ))
                    .header(header::COOKIE, format!("{}={token}", SESSION_COOKIE_NAME))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "command_id": "runner.interrupt",
                            "raw_input": "/interrupt please",
                            "arguments": {},
                            "confirmed": true,
                            "idempotency_key": "slash-interrupt-key"
                        })
                        .to_string(),
                    ))
                    .expect("build conflicting interrupt"),
            )
            .await
            .expect("route conflicting interrupt");
        assert_eq!(conflict.status(), StatusCode::CONFLICT);
        assert!(runner_receiver.try_recv().is_err());
        assert_eq!(
            user_message_count(&state, user_id, session.session_id, "/interrupt").await,
            1
        );
    }

    #[tokio::test]
    async fn idempotent_provider_slash_command_replays_and_conflicts() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let user_id = state.bootstrap_user_id().expect("bootstrap user");
        let runner = fake_hello();
        let workspace = runner.workspaces[0].clone();
        state
            .register_runner(
                runner.runner_id,
                runner.capabilities.clone(),
                runner.workspaces.clone(),
            )
            .await;
        let (runner_sender, mut runner_receiver) = tokio::sync::mpsc::unbounded_channel();
        state.connect_runner(runner.runner_id, runner_sender).await;
        let session = state
            .create_session(
                SessionId::new(),
                user_id,
                runner.runner_id,
                workspace,
                AgentProviderId::from(AgentProviderId::CODEX),
            )
            .await;
        let token = state
            .login_password("admin@example.test", "correct horse battery staple")
            .await
            .expect("login bootstrap admin");
        let app = app(state.clone());
        let body = serde_json::json!({
            "command_id": "codex.compact",
            "raw_input": "/compact",
            "arguments": {},
            "confirmed": false,
            "idempotency_key": "provider-slash-key"
        })
        .to_string();

        let first = tokio::spawn({
            let app = app.clone();
            let token = token.clone();
            let body = body.clone();
            async move {
                app.oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(format!(
                            "/api/sessions/{}/slash-commands",
                            session.session_id
                        ))
                        .header(header::COOKIE, format!("{}={token}", SESSION_COOKIE_NAME))
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(body))
                        .expect("build provider command"),
                )
                .await
                .expect("route provider command")
            }
        });
        let outbound = runner_receiver.recv().await.expect("runner command");
        let RunnerServerMessage::Command(command) = outbound.message else {
            panic!("expected runner command");
        };
        assert!(matches!(
            command.command,
            agenter_protocol::runner::RunnerCommand::ExecuteProviderCommand(_)
        ));
        outbound.delivered.send(Ok(())).expect("runner delivered");
        state
            .finish_runner_response(
                runner.runner_id,
                command.request_id,
                agenter_protocol::runner::RunnerResponseOutcome::Ok {
                    result:
                        agenter_protocol::runner::RunnerCommandResult::ProviderCommandExecuted {
                            result: agenter_core::SlashCommandResult {
                                accepted: true,
                                message: "Compacted.".to_owned(),
                                session: None,
                                provider_payload: None,
                            },
                        },
                },
            )
            .await;
        assert_eq!(
            first.await.expect("join provider command").status(),
            StatusCode::OK
        );
        assert_eq!(
            user_message_count(&state, user_id, session.session_id, "/compact").await,
            1
        );

        let duplicate = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/sessions/{}/slash-commands",
                        session.session_id
                    ))
                    .header(header::COOKIE, format!("{}={token}", SESSION_COOKIE_NAME))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .expect("build duplicate provider command"),
            )
            .await
            .expect("route duplicate provider command");
        assert_eq!(duplicate.status(), StatusCode::OK);
        assert!(runner_receiver.try_recv().is_err());
        assert_eq!(
            user_message_count(&state, user_id, session.session_id, "/compact").await,
            1
        );

        let conflict = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/sessions/{}/slash-commands",
                        session.session_id
                    ))
                    .header(header::COOKIE, format!("{}={token}", SESSION_COOKIE_NAME))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "command_id": "codex.compact",
                            "raw_input": "/compact now",
                            "arguments": {},
                            "confirmed": false,
                            "idempotency_key": "provider-slash-key"
                        })
                        .to_string(),
                    ))
                    .expect("build conflicting provider command"),
            )
            .await
            .expect("route conflicting provider command");
        assert_eq!(conflict.status(), StatusCode::CONFLICT);
        assert!(runner_receiver.try_recv().is_err());
        assert_eq!(
            user_message_count(&state, user_id, session.session_id, "/compact").await,
            1
        );
    }

    #[tokio::test]
    async fn idempotent_slash_without_key_executes_repeated_invocations() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let user_id = state.bootstrap_user_id().expect("bootstrap user");
        let runner = fake_hello();
        let workspace = runner.workspaces[0].clone();
        state
            .register_runner(
                runner.runner_id,
                runner.capabilities.clone(),
                runner.workspaces.clone(),
            )
            .await;
        let (runner_sender, mut runner_receiver) = tokio::sync::mpsc::unbounded_channel();
        state.connect_runner(runner.runner_id, runner_sender).await;
        let session = state
            .create_session(
                SessionId::new(),
                user_id,
                runner.runner_id,
                workspace,
                AgentProviderId::from(AgentProviderId::CODEX),
            )
            .await;
        let token = state
            .login_password("admin@example.test", "correct horse battery staple")
            .await
            .expect("login bootstrap admin");
        let app = app(state.clone());
        let body = serde_json::json!({
            "command_id": "codex.compact",
            "raw_input": "/compact",
            "arguments": {},
            "confirmed": false
        })
        .to_string();

        for expected_message in ["Compacted once.", "Compacted twice."] {
            let response_task = tokio::spawn({
                let app = app.clone();
                let token = token.clone();
                let body = body.clone();
                let session_id = session.session_id;
                async move {
                    app.oneshot(
                        Request::builder()
                            .method("POST")
                            .uri(format!("/api/sessions/{session_id}/slash-commands"))
                            .header(header::COOKIE, format!("{}={token}", SESSION_COOKIE_NAME))
                            .header(header::CONTENT_TYPE, "application/json")
                            .body(Body::from(body))
                            .expect("build provider command"),
                    )
                    .await
                    .expect("route provider command")
                }
            });
            let outbound = runner_receiver.recv().await.expect("runner command");
            let RunnerServerMessage::Command(command) = outbound.message else {
                panic!("expected runner command");
            };
            assert!(matches!(
                command.command,
                agenter_protocol::runner::RunnerCommand::ExecuteProviderCommand(_)
            ));
            outbound.delivered.send(Ok(())).expect("runner delivered");
            state
                .finish_runner_response(
                    runner.runner_id,
                    command.request_id,
                    agenter_protocol::runner::RunnerResponseOutcome::Ok {
                        result:
                            agenter_protocol::runner::RunnerCommandResult::ProviderCommandExecuted {
                                result: agenter_core::SlashCommandResult {
                                    accepted: true,
                                    message: expected_message.to_owned(),
                                    session: None,
                                    provider_payload: None,
                                },
                            },
                    },
                )
                .await;
            assert_eq!(
                response_task.await.expect("join provider command").status(),
                StatusCode::OK
            );
        }

        assert!(runner_receiver.try_recv().is_err());
        assert_eq!(
            user_message_count(&state, user_id, session.session_id, "/compact").await,
            2
        );
    }

    #[tokio::test]
    async fn idempotent_local_new_slash_replays_created_session() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let user_id = state.bootstrap_user_id().expect("bootstrap user");
        let runner = fake_hello();
        let workspace = runner.workspaces[0].clone();
        state
            .register_runner(
                runner.runner_id,
                runner.capabilities.clone(),
                runner.workspaces.clone(),
            )
            .await;
        let (runner_sender, mut runner_receiver) = tokio::sync::mpsc::unbounded_channel();
        state.connect_runner(runner.runner_id, runner_sender).await;
        let source_session = state
            .create_session(
                SessionId::new(),
                user_id,
                runner.runner_id,
                workspace,
                AgentProviderId::from(AgentProviderId::CODEX),
            )
            .await;
        let token = state
            .login_password("admin@example.test", "correct horse battery staple")
            .await
            .expect("login bootstrap admin");
        let app = app(state.clone());
        let body = serde_json::json!({
            "command_id": "local.new",
            "raw_input": "/new follow-up",
            "arguments": { "title": "follow-up" },
            "confirmed": false,
            "idempotency_key": "local-new-key"
        })
        .to_string();

        let first = tokio::spawn({
            let app = app.clone();
            let token = token.clone();
            let body = body.clone();
            let source_session_id = source_session.session_id;
            async move {
                app.oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(format!("/api/sessions/{source_session_id}/slash-commands"))
                        .header(header::COOKIE, format!("{}={token}", SESSION_COOKIE_NAME))
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(body))
                        .expect("build local new"),
                )
                .await
                .expect("route local new")
            }
        });
        let outbound = runner_receiver.recv().await.expect("runner command");
        let RunnerServerMessage::Command(command) = outbound.message else {
            panic!("expected runner command");
        };
        let agenter_protocol::runner::RunnerCommand::CreateSession(create_command) =
            &command.command
        else {
            panic!("expected create-session command");
        };
        let created_session_id = create_command.session_id;
        outbound.delivered.send(Ok(())).expect("runner delivered");
        state
            .finish_runner_response(
                runner.runner_id,
                command.request_id,
                agenter_protocol::runner::RunnerResponseOutcome::Ok {
                    result: agenter_protocol::runner::RunnerCommandResult::SessionCreated {
                        session_id: created_session_id,
                        external_session_id: "codex-local-new".to_owned(),
                    },
                },
            )
            .await;
        let first = first.await.expect("join local new");
        assert_eq!(first.status(), StatusCode::OK);
        let first_body = to_bytes(first.into_body(), usize::MAX)
            .await
            .expect("read local new body");
        let first_result: agenter_core::SlashCommandResult =
            serde_json::from_slice(&first_body).expect("local new result");
        assert_eq!(
            first_result.session.expect("created session").session_id,
            created_session_id
        );

        let duplicate = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/sessions/{}/slash-commands",
                        source_session.session_id
                    ))
                    .header(header::COOKIE, format!("{}={token}", SESSION_COOKIE_NAME))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .expect("build duplicate local new"),
            )
            .await
            .expect("route duplicate local new");
        assert_eq!(duplicate.status(), StatusCode::OK);
        let duplicate_body = to_bytes(duplicate.into_body(), usize::MAX)
            .await
            .expect("read duplicate local new body");
        let duplicate_result: agenter_core::SlashCommandResult =
            serde_json::from_slice(&duplicate_body).expect("duplicate local new result");
        assert_eq!(
            duplicate_result
                .session
                .expect("created session")
                .session_id,
            created_session_id
        );
        assert!(runner_receiver.try_recv().is_err());

        let conflict = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/sessions/{}/slash-commands",
                        source_session.session_id
                    ))
                    .header(header::COOKIE, format!("{}={token}", SESSION_COOKIE_NAME))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "command_id": "local.new",
                            "raw_input": "/new other",
                            "arguments": { "title": "other" },
                            "confirmed": false,
                            "idempotency_key": "local-new-key"
                        })
                        .to_string(),
                    ))
                    .expect("build conflicting local new"),
            )
            .await
            .expect("route conflicting local new");
        assert_eq!(conflict.status(), StatusCode::CONFLICT);
        assert!(runner_receiver.try_recv().is_err());
        assert_eq!(
            user_message_count(&state, user_id, source_session.session_id, "/new follow-up").await,
            1
        );
        assert_eq!(
            user_message_count(&state, user_id, source_session.session_id, "/new other").await,
            0
        );
    }

    #[tokio::test]
    async fn idempotent_local_refresh_replays_unavailable_workspace_failure() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let user_id = state.bootstrap_user_id().expect("bootstrap user");
        let runner = fake_hello();
        let workspace = runner.workspaces[0].clone();
        let session = state
            .create_session(
                SessionId::new(),
                user_id,
                runner.runner_id,
                workspace,
                AgentProviderId::from(AgentProviderId::CODEX),
            )
            .await;
        let token = state
            .login_password("admin@example.test", "correct horse battery staple")
            .await
            .expect("login bootstrap admin");
        let app = app(state.clone());
        let body = serde_json::json!({
            "command_id": "local.refresh",
            "raw_input": "/refresh",
            "arguments": {},
            "confirmed": false,
            "idempotency_key": "local-refresh-missing-workspace"
        })
        .to_string();

        let first = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/sessions/{}/slash-commands",
                        session.session_id
                    ))
                    .header(header::COOKIE, format!("{}={token}", SESSION_COOKIE_NAME))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.clone()))
                    .expect("build local refresh"),
            )
            .await
            .expect("route local refresh");
        assert_eq!(first.status(), StatusCode::NOT_FOUND);
        let first_body = to_bytes(first.into_body(), usize::MAX)
            .await
            .expect("read local refresh body");
        let first_result: agenter_core::SlashCommandResult =
            serde_json::from_slice(&first_body).expect("local refresh result");
        assert!(!first_result.accepted);

        let duplicate = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/sessions/{}/slash-commands",
                        session.session_id
                    ))
                    .header(header::COOKIE, format!("{}={token}", SESSION_COOKIE_NAME))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .expect("build duplicate local refresh"),
            )
            .await
            .expect("route duplicate local refresh");
        assert_eq!(duplicate.status(), StatusCode::NOT_FOUND);
        let duplicate_body = to_bytes(duplicate.into_body(), usize::MAX)
            .await
            .expect("read duplicate local refresh body");
        let duplicate_result: agenter_core::SlashCommandResult =
            serde_json::from_slice(&duplicate_body).expect("duplicate local refresh result");
        assert_eq!(duplicate_result.message, first_result.message);
        assert_eq!(
            user_message_count(&state, user_id, session.session_id, "/refresh").await,
            1
        );
    }

    #[tokio::test]
    async fn idempotent_local_model_slash_replays_and_conflicts() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let user_id = state.bootstrap_user_id().expect("bootstrap user");
        let runner = fake_hello();
        let workspace = runner.workspaces[0].clone();
        state
            .register_runner(
                runner.runner_id,
                runner.capabilities.clone(),
                runner.workspaces.clone(),
            )
            .await;
        let session = state
            .create_session(
                SessionId::new(),
                user_id,
                runner.runner_id,
                workspace,
                AgentProviderId::from(AgentProviderId::CODEX),
            )
            .await;
        let token = state
            .login_password("admin@example.test", "correct horse battery staple")
            .await
            .expect("login bootstrap admin");
        let app = app(state.clone());
        let body = serde_json::json!({
            "command_id": "local.model",
            "raw_input": "/model gpt-5",
            "arguments": { "model": "gpt-5" },
            "confirmed": false,
            "idempotency_key": "local-model-key"
        })
        .to_string();

        let first = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/sessions/{}/slash-commands",
                        session.session_id
                    ))
                    .header(header::COOKIE, format!("{}={token}", SESSION_COOKIE_NAME))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.clone()))
                    .expect("build local model"),
            )
            .await
            .expect("route local model");
        assert_eq!(first.status(), StatusCode::OK);

        let duplicate = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/sessions/{}/slash-commands",
                        session.session_id
                    ))
                    .header(header::COOKIE, format!("{}={token}", SESSION_COOKIE_NAME))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .expect("build duplicate local model"),
            )
            .await
            .expect("route duplicate local model");
        assert_eq!(duplicate.status(), StatusCode::OK);

        let conflict = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/sessions/{}/slash-commands",
                        session.session_id
                    ))
                    .header(header::COOKIE, format!("{}={token}", SESSION_COOKIE_NAME))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "command_id": "local.model",
                            "raw_input": "/model gpt-5.1",
                            "arguments": { "model": "gpt-5.1" },
                            "confirmed": false,
                            "idempotency_key": "local-model-key"
                        })
                        .to_string(),
                    ))
                    .expect("build conflicting local model"),
            )
            .await
            .expect("route conflicting local model");
        assert_eq!(conflict.status(), StatusCode::CONFLICT);

        let settings = state
            .session_turn_settings(user_id, session.session_id)
            .await
            .expect("settings");
        assert_eq!(settings.model.as_deref(), Some("gpt-5"));
    }

    #[tokio::test]
    async fn approval_decision_resolves_only_after_runner_ack() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let user_id = state.bootstrap_user_id().expect("bootstrap user");
        let session_id = smoke_session_id();
        let runner = fake_hello();
        let workspace = runner.workspaces[0].clone();
        state
            .register_runner(
                runner.runner_id,
                runner.capabilities.clone(),
                runner.workspaces.clone(),
            )
            .await;
        let (runner_sender, mut runner_receiver) = tokio::sync::mpsc::unbounded_channel();
        state.connect_runner(runner.runner_id, runner_sender).await;
        let (observed_sender, mut observed_receiver) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(outbound) = runner_receiver.recv().await {
                observed_sender.send(outbound.message).ok();
                outbound
                    .delivered
                    .send(Ok(()))
                    .expect("report websocket delivery");
            }
        });
        state
            .create_session(
                session_id,
                user_id,
                runner.runner_id,
                workspace,
                AgentProviderId::from(AgentProviderId::CODEX),
            )
            .await;
        let approval_id = ApprovalId::new();
        state
            .publish_event(
                session_id,
                NormalizedEvent::ApprovalRequested(ApprovalRequestEvent {
                    session_id,
                    approval_id,
                    kind: ApprovalKind::Command,
                    title: "Run tests".to_owned(),
                    details: Some("cargo test".to_owned()),
                    expires_at: None,
                    presentation: None,
                    resolution_state: None,
                    resolving_decision: None,
                    status: None,
                    turn_id: None,
                    item_id: None,
                    options: Vec::new(),
                    risk: None,
                    subject: None,
                    native_request_id: None,
                    native_blocking: true,
                    policy: None,
                    provider_payload: None,
                }),
            )
            .await;
        let browser_session_token = state
            .login_password("admin@example.test", "correct horse battery staple")
            .await
            .expect("login bootstrap admin");
        let app = app(state.clone());

        let mut response_task = tokio::spawn(
            app.clone().oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/approvals/{approval_id}/decision"))
                    .header(
                        header::COOKIE,
                        format!("{}={browser_session_token}", SESSION_COOKIE_NAME),
                    )
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_string(&ApprovalDecision::Accept)
                            .expect("serialize decision"),
                    ))
                    .expect("build approval decision request"),
            ),
        );

        let RunnerServerMessage::Command(command) =
            timeout(Duration::from_millis(250), observed_receiver.recv())
                .await
                .expect("runner command should be sent")
                .expect("runner receives approval answer")
        else {
            panic!("expected runner command");
        };
        let agenter_protocol::runner::RunnerCommand::AnswerApproval(answer) =
            command.command.clone()
        else {
            panic!("expected approval answer command");
        };
        assert_eq!(answer.approval_id, approval_id);

        tokio::select! {
            result = &mut response_task => {
                panic!("approval HTTP response finished before runner acknowledgement: {result:?}");
            }
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
        let history = state
            .session_history(user_id, session_id)
            .await
            .expect("history while resolving");
        assert!(
            !history
                .iter()
                .any(|entry| matches!(entry.event, NormalizedEvent::ApprovalResolved(_))),
            "approval must not resolve before the runner acknowledges provider delivery"
        );
        let pending = state.pending_approval_request_envelopes(session_id).await;
        let pending_json = serde_json::to_value(&pending[0].event).expect("pending json");
        assert_eq!(pending_json["payload"]["resolution_state"], "resolving");

        state
            .finish_runner_response(
                runner.runner_id,
                command.request_id,
                agenter_protocol::runner::RunnerResponseOutcome::Ok {
                    result: agenter_protocol::runner::RunnerCommandResult::Accepted,
                },
            )
            .await;

        let response = response_task
            .await
            .expect("join approval response")
            .expect("route duplicate approval decision");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read approval response");
        let envelope: BrowserEventEnvelope =
            serde_json::from_slice(&body).expect("approval response json");
        assert!(matches!(
            envelope.event,
            NormalizedEvent::ApprovalResolved(_)
        ));

        let duplicate_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/approvals/{approval_id}/decision"))
                    .header(
                        header::COOKIE,
                        format!("{}={browser_session_token}", SESSION_COOKIE_NAME),
                    )
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_string(&ApprovalDecision::Accept)
                            .expect("serialize decision"),
                    ))
                    .expect("build duplicate approval decision request"),
            )
            .await
            .expect("route duplicate approval decision");
        assert_eq!(duplicate_response.status(), StatusCode::OK);
        let duplicate_body = to_bytes(duplicate_response.into_body(), usize::MAX)
            .await
            .expect("read duplicate approval response");
        let duplicate_envelope: BrowserEventEnvelope =
            serde_json::from_slice(&duplicate_body).expect("duplicate approval response json");
        assert_eq!(duplicate_envelope.event_id, envelope.event_id);
        assert!(
            observed_receiver.try_recv().is_err(),
            "duplicate decisions must not send another runner command"
        );

        let conflicting_response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/approvals/{approval_id}/decision"))
                    .header(
                        header::COOKIE,
                        format!("{}={browser_session_token}", SESSION_COOKIE_NAME),
                    )
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_string(&ApprovalDecision::Decline)
                            .expect("serialize conflicting decision"),
                    ))
                    .expect("build conflicting approval decision request"),
            )
            .await
            .expect("route conflicting approval decision");
        assert_eq!(conflicting_response.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn approval_decision_retry_after_transient_runner_failure_is_not_poisoned() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let user_id = state.bootstrap_user_id().expect("bootstrap user");
        let session_id = smoke_session_id();
        let runner = fake_hello();
        let workspace = runner.workspaces[0].clone();
        state
            .register_runner(
                runner.runner_id,
                runner.capabilities.clone(),
                runner.workspaces.clone(),
            )
            .await;
        state
            .create_session(
                session_id,
                user_id,
                runner.runner_id,
                workspace,
                AgentProviderId::from(AgentProviderId::CODEX),
            )
            .await;
        let approval_id = ApprovalId::new();
        state
            .publish_event(
                session_id,
                NormalizedEvent::ApprovalRequested(ApprovalRequestEvent {
                    session_id,
                    approval_id,
                    kind: ApprovalKind::Command,
                    title: "Run tests".to_owned(),
                    details: Some("cargo test".to_owned()),
                    expires_at: None,
                    presentation: None,
                    resolution_state: None,
                    resolving_decision: None,
                    status: None,
                    turn_id: None,
                    item_id: None,
                    options: vec![agenter_core::ApprovalOption::approve_once()],
                    risk: None,
                    subject: None,
                    native_request_id: None,
                    native_blocking: true,
                    policy: None,
                    provider_payload: None,
                }),
            )
            .await;
        let browser_session_token = state
            .login_password("admin@example.test", "correct horse battery staple")
            .await
            .expect("login bootstrap admin");
        let app = app(state.clone());

        let first_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/approvals/{approval_id}/decision"))
                    .header(
                        header::COOKIE,
                        format!("{}={browser_session_token}", SESSION_COOKIE_NAME),
                    )
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "decision": "accept",
                            "option_id": "approve_once"
                        })
                        .to_string(),
                    ))
                    .expect("build transient approval decision request"),
            )
            .await
            .expect("route transient approval decision");
        assert_eq!(first_response.status(), StatusCode::SERVICE_UNAVAILABLE);

        let (runner_sender, mut runner_receiver) = tokio::sync::mpsc::unbounded_channel();
        state.connect_runner(runner.runner_id, runner_sender).await;
        let retry_task = tokio::spawn(
            app.clone().oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/approvals/{approval_id}/decision"))
                    .header(
                        header::COOKIE,
                        format!("{}={browser_session_token}", SESSION_COOKIE_NAME),
                    )
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "decision": "accept",
                            "option_id": "approve_once"
                        })
                        .to_string(),
                    ))
                    .expect("build retry approval decision request"),
            ),
        );

        let outbound = runner_receiver
            .recv()
            .await
            .expect("retry sends runner command");
        outbound
            .delivered
            .send(Ok(()))
            .expect("report runner delivery");
        let RunnerServerMessage::Command(command) = outbound.message else {
            panic!("expected runner command");
        };
        state
            .finish_runner_response(
                runner.runner_id,
                command.request_id,
                agenter_protocol::runner::RunnerResponseOutcome::Ok {
                    result: agenter_protocol::runner::RunnerCommandResult::Accepted,
                },
            )
            .await;

        let retry_response = retry_task
            .await
            .expect("join retry approval decision")
            .expect("route retry approval decision");
        assert_eq!(retry_response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn unauthorized_approval_decision_does_not_mark_resolving() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let owner = state.bootstrap_user_id().expect("bootstrap user");
        let runner = fake_hello();
        let session_id = smoke_session_id();
        state
            .create_session(
                session_id,
                owner,
                runner.runner_id,
                runner.workspaces[0].clone(),
                AgentProviderId::from(AgentProviderId::CODEX),
            )
            .await;
        let approval_id = ApprovalId::new();
        state
            .publish_event(
                session_id,
                NormalizedEvent::ApprovalRequested(approval_request_event(session_id, approval_id)),
            )
            .await;
        let other_token = state
            .create_authenticated_session(AuthenticatedUser {
                user_id: agenter_core::UserId::new(),
                email: "other@example.test".to_owned(),
                display_name: None,
            })
            .await;

        let response = app(state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/approvals/{approval_id}/decision"))
                    .header(
                        header::COOKIE,
                        format!("{}={other_token}", SESSION_COOKIE_NAME),
                    )
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_string(&ApprovalDecision::Accept)
                            .expect("serialize decision"),
                    ))
                    .expect("build approval request"),
            )
            .await
            .expect("route approval request");

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(!state.approval_is_resolving(approval_id).await);
        let history = state
            .session_history(owner, session_id)
            .await
            .expect("history");
        assert!(
            history.iter().all(|entry| {
                !matches!(
                    &entry.event,
                    NormalizedEvent::ApprovalRequested(request)
                        if request.approval_id == approval_id
                            && request.resolution_state
                                == Some(agenter_core::ApprovalResolutionState::Resolving)
                )
            }),
            "unauthorized decision must not emit resolving transition"
        );
    }

    #[tokio::test]
    async fn idempotency_conflict_does_not_mark_approval_resolving() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let owner = state.bootstrap_user_id().expect("bootstrap user");
        let runner = fake_hello();
        let session_id = smoke_session_id();
        state
            .create_session(
                session_id,
                owner,
                runner.runner_id,
                runner.workspaces[0].clone(),
                AgentProviderId::from(AgentProviderId::CODEX),
            )
            .await;
        let approval_id = ApprovalId::new();
        state
            .publish_event(
                session_id,
                NormalizedEvent::ApprovalRequested(approval_request_event(session_id, approval_id)),
            )
            .await;
        let token = state
            .login_password("admin@example.test", "correct horse battery staple")
            .await
            .expect("login bootstrap admin");
        let mut conflicting_command =
            approval_test_command(owner, session_id, approval_id, "approve_once", None);
        if let agenter_core::UniversalCommand::ResolveApproval { option_id, .. } =
            &mut conflicting_command.command
        {
            *option_id = "deny".to_owned();
        }
        assert!(matches!(
            state.begin_universal_command(&conflicting_command).await,
            Ok(crate::state::UniversalCommandStart::Started)
        ));

        let response = app(state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/approvals/{approval_id}/decision"))
                    .header(header::COOKIE, format!("{}={token}", SESSION_COOKIE_NAME))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_string(&ApprovalDecision::Accept)
                            .expect("serialize decision"),
                    ))
                    .expect("build approval request"),
            )
            .await
            .expect("route approval request");

        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert!(!state.approval_is_resolving(approval_id).await);
    }

    #[tokio::test]
    async fn resolved_approval_with_pending_idempotency_replays_and_finishes() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let owner = state.bootstrap_user_id().expect("bootstrap user");
        let runner = fake_hello();
        let session_id = smoke_session_id();
        state
            .create_session(
                session_id,
                owner,
                runner.runner_id,
                runner.workspaces[0].clone(),
                AgentProviderId::from(AgentProviderId::CODEX),
            )
            .await;
        let approval_id = ApprovalId::new();
        state
            .publish_event(
                session_id,
                NormalizedEvent::ApprovalRequested(approval_request_event(session_id, approval_id)),
            )
            .await;
        state
            .publish_event(
                session_id,
                NormalizedEvent::ApprovalResolved(agenter_core::ApprovalResolvedEvent {
                    session_id,
                    approval_id,
                    decision: ApprovalDecision::Accept,
                    resolved_by_user_id: Some(owner),
                    resolved_at: Utc::now(),
                    provider_payload: None,
                }),
            )
            .await;
        let pending_command =
            approval_test_command(owner, session_id, approval_id, "approve_once", None);
        assert!(matches!(
            state.begin_universal_command(&pending_command).await,
            Ok(crate::state::UniversalCommandStart::Started)
        ));
        let token = state
            .login_password("admin@example.test", "correct horse battery staple")
            .await
            .expect("login bootstrap admin");

        for _ in 0..2 {
            let response = app(state.clone())
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(format!("/api/approvals/{approval_id}/decision"))
                        .header(header::COOKIE, format!("{}={token}", SESSION_COOKIE_NAME))
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(
                            serde_json::to_string(&ApprovalDecision::Accept)
                                .expect("serialize decision"),
                        ))
                        .expect("build approval request"),
                )
                .await
                .expect("route approval request");
            assert_eq!(response.status(), StatusCode::OK);
            let body = to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("read approval body");
            let envelope: BrowserEventEnvelope =
                serde_json::from_slice(&body).expect("approval envelope");
            assert!(matches!(
                envelope.event,
                NormalizedEvent::ApprovalResolved(_)
            ));
        }
    }

    #[tokio::test]
    async fn provider_specific_approval_payload_changes_conflict_by_hash() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let owner = state.bootstrap_user_id().expect("bootstrap user");
        let runner = fake_hello();
        let session_id = smoke_session_id();
        state
            .create_session(
                session_id,
                owner,
                runner.runner_id,
                runner.workspaces[0].clone(),
                AgentProviderId::from(AgentProviderId::CODEX),
            )
            .await;
        let approval_id = ApprovalId::new();
        state
            .publish_event(
                session_id,
                NormalizedEvent::ApprovalRequested(approval_request_event(session_id, approval_id)),
            )
            .await;
        let first_decision = ApprovalDecision::ProviderSpecific {
            payload: serde_json::json!({"decision": "allow", "nonce": 1}),
        };
        let first_command = approval_test_command(
            owner,
            session_id,
            approval_id,
            "provider_specific",
            approval_feedback_for_test(&first_decision),
        );
        assert!(matches!(
            state.begin_universal_command(&first_command).await,
            Ok(crate::state::UniversalCommandStart::Started)
        ));
        let token = state
            .login_password("admin@example.test", "correct horse battery staple")
            .await
            .expect("login bootstrap admin");

        let response = app(state.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/approvals/{approval_id}/decision"))
                    .header(header::COOKIE, format!("{}={token}", SESSION_COOKIE_NAME))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_string(&ApprovalDecision::ProviderSpecific {
                            payload: serde_json::json!({"decision": "allow", "nonce": 2}),
                        })
                        .expect("serialize decision"),
                    ))
                    .expect("build approval request"),
            )
            .await
            .expect("route approval request");

        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert!(!state.approval_is_resolving(approval_id).await);
    }

    #[tokio::test]
    async fn create_session_rejects_provider_not_advertised_by_workspace_runner() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let mut runner = fake_hello();
        runner.capabilities.agent_providers[0].provider_id =
            AgentProviderId::from(AgentProviderId::QWEN);
        let workspace_id = runner.workspaces[0].workspace_id;
        state
            .register_runner(
                runner.runner_id,
                runner.capabilities.clone(),
                runner.workspaces.clone(),
            )
            .await;
        let browser_session_token = state
            .login_password("admin@example.test", "correct horse battery staple")
            .await
            .expect("login bootstrap admin");
        let response = app(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/sessions")
                    .header(
                        header::COOKIE,
                        format!("{}={browser_session_token}", SESSION_COOKIE_NAME),
                    )
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "workspace_id": workspace_id,
                            "provider_id": AgentProviderId::from(AgentProviderId::CODEX),
                            "title": "wrong provider"
                        })
                        .to_string(),
                    ))
                    .expect("build create session request"),
            )
            .await
            .expect("route create session request");

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn runner_resolved_approval_is_idempotent_for_browser_decision() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let user_id = state.bootstrap_user_id().expect("bootstrap user");
        let session_id = smoke_session_id();
        let runner = fake_hello();
        let workspace = runner.workspaces[0].clone();
        state
            .register_runner(
                runner.runner_id,
                runner.capabilities.clone(),
                runner.workspaces.clone(),
            )
            .await;
        let (runner_sender, mut runner_receiver) = tokio::sync::mpsc::unbounded_channel();
        state.connect_runner(runner.runner_id, runner_sender).await;
        state
            .create_session(
                session_id,
                user_id,
                runner.runner_id,
                workspace,
                AgentProviderId::from(AgentProviderId::CODEX),
            )
            .await;
        let approval_id = ApprovalId::new();
        state
            .publish_event(
                session_id,
                NormalizedEvent::ApprovalRequested(ApprovalRequestEvent {
                    session_id,
                    approval_id,
                    kind: ApprovalKind::Command,
                    title: "Run tests".to_owned(),
                    details: Some("cargo test".to_owned()),
                    expires_at: None,
                    presentation: None,
                    resolution_state: None,
                    resolving_decision: None,
                    status: None,
                    turn_id: None,
                    item_id: None,
                    options: Vec::new(),
                    risk: None,
                    subject: None,
                    native_request_id: None,
                    native_blocking: true,
                    policy: None,
                    provider_payload: None,
                }),
            )
            .await;
        state
            .publish_event(
                session_id,
                NormalizedEvent::ApprovalResolved(agenter_core::ApprovalResolvedEvent {
                    session_id,
                    approval_id,
                    decision: ApprovalDecision::Decline,
                    resolved_by_user_id: None,
                    resolved_at: Utc::now(),
                    provider_payload: None,
                }),
            )
            .await;
        let browser_session_token = state
            .login_password("admin@example.test", "correct horse battery staple")
            .await
            .expect("login bootstrap admin");
        let response = app(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/approvals/{approval_id}/decision"))
                    .header(
                        header::COOKIE,
                        format!("{}={browser_session_token}", SESSION_COOKIE_NAME),
                    )
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_string(&ApprovalDecision::Accept)
                            .expect("serialize decision"),
                    ))
                    .expect("build stale approval decision request"),
            )
            .await
            .expect("route stale approval decision");

        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert!(
            runner_receiver.try_recv().is_err(),
            "stale browser decisions must not send runner commands"
        );
    }

    #[tokio::test]
    async fn auth_me_rejects_missing_session_cookie() {
        let response = app(AppState::new(
            "dev-token".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        ))
        .oneshot(
            Request::builder()
                .uri("/api/auth/me")
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("route request");

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn browser_websocket_rejects_missing_session_cookie() {
        let state = AppState::new("dev-token".to_owned(), CookieSecurity::DevelopmentInsecure);
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let addr = listener.local_addr().expect("read listener address");
        tokio::spawn(async move {
            axum::serve(listener, app(state)).await.expect("serve app");
        });

        let connection = connect_async(format!("ws://{addr}/api/browser/ws")).await;

        assert!(connection.is_err(), "browser websocket should require auth");
    }

    #[tokio::test]
    async fn password_login_sets_session_cookie_and_me_returns_user() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let app = app(state);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/password/login")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "email": "admin@example.test",
                            "password": "correct horse battery staple"
                        })
                        .to_string(),
                    ))
                    .expect("build login request"),
            )
            .await
            .expect("route login request");

        assert_eq!(response.status(), StatusCode::OK);
        let cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .expect("login sets cookie")
            .to_str()
            .expect("cookie is ascii")
            .to_owned();
        assert!(cookie.contains("agenter_session="));
        assert!(cookie.contains("HttpOnly"));
        assert!(!cookie.contains("Secure"));
        assert!(cookie.contains("Max-Age=2592000"));

        let me_response = app
            .oneshot(
                Request::builder()
                    .uri("/api/auth/me")
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())
                    .expect("build me request"),
            )
            .await
            .expect("route me request");

        assert_eq!(me_response.status(), StatusCode::OK);
        let body = to_bytes(me_response.into_body(), usize::MAX)
            .await
            .expect("read me body");
        let body: serde_json::Value = serde_json::from_slice(&body).expect("json response");
        assert_eq!(body["email"], "admin@example.test");
    }

    #[tokio::test]
    async fn password_login_rejects_wrong_password() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let response = app(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/password/login")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "email": "admin@example.test",
                            "password": "wrong password"
                        })
                        .to_string(),
                    ))
                    .expect("build login request"),
            )
            .await
            .expect("route login request");

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn logout_invalidates_session_cookie() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let app = app(state);

        let login_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/password/login")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "email": "admin@example.test",
                            "password": "correct horse battery staple"
                        })
                        .to_string(),
                    ))
                    .expect("build login request"),
            )
            .await
            .expect("route login request");
        let cookie = login_response
            .headers()
            .get(header::SET_COOKIE)
            .expect("login sets cookie")
            .to_str()
            .expect("cookie is ascii")
            .to_owned();

        let logout_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/password/logout")
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .expect("build logout request"),
            )
            .await
            .expect("route logout request");
        assert_eq!(logout_response.status(), StatusCode::NO_CONTENT);

        let me_response = app
            .oneshot(
                Request::builder()
                    .uri("/api/auth/me")
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())
                    .expect("build me request"),
            )
            .await
            .expect("route me request");
        assert_eq!(me_response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn browser_websocket_rejects_session_not_owned_by_user() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let browser_session_token = state
            .login_password("admin@example.test", "correct horse battery staple")
            .await
            .expect("login bootstrap admin");
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let addr = listener.local_addr().expect("read listener address");
        tokio::spawn(async move {
            axum::serve(listener, app(state)).await.expect("serve app");
        });

        let mut browser_request = format!("ws://{addr}/api/browser/ws")
            .into_client_request()
            .expect("build browser websocket request");
        browser_request.headers_mut().insert(
            header::COOKIE,
            format!("{}={browser_session_token}", SESSION_COOKIE_NAME)
                .parse()
                .expect("cookie header"),
        );
        let (browser_socket, _) = connect_async(browser_request)
            .await
            .expect("connect browser");
        let (mut browser_sender, mut browser_receiver) = browser_socket.split();
        browser_sender
            .send(Message::Text(
                serde_json::to_string(&BrowserClientMessage::SubscribeSession(SubscribeSession {
                    request_id: Some(RequestId::from("sub-forbidden")),
                    session_id: SessionId::new(),
                    after_seq: None,
                    include_snapshot: false,
                }))
                .expect("serialize subscribe")
                .into(),
            ))
            .await
            .expect("send browser subscription");

        let browser_error = browser_receiver
            .next()
            .await
            .expect("browser error frame")
            .expect("browser error websocket result");
        let BrowserServerMessage::Error(error) =
            serde_json::from_str::<BrowserServerMessage>(browser_error.to_text().unwrap())
                .expect("decode browser error")
        else {
            panic!("expected browser error");
        };
        assert_eq!(error.code, "forbidden");
    }

    #[tokio::test]
    async fn runner_websocket_accepts_chunked_large_discovered_history_without_truncation() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.com".to_owned(),
            "agenter-dev-password".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let user_id = state.bootstrap_user_id().expect("bootstrap user");
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let addr = listener.local_addr().expect("read listener address");
        tokio::spawn({
            let state = state.clone();
            async move {
                axum::serve(listener, app(state)).await.expect("serve app");
            }
        });

        let (runner_socket, _) = connect_async(format!("ws://{addr}/api/runner/ws"))
            .await
            .expect("connect runner");
        let (mut runner_sender, mut runner_receiver) = runner_socket.split();
        let hello = fake_hello();
        let workspace = hello.workspaces[0].clone();
        runner_sender
            .send(Message::Text(
                serde_json::to_string(&RunnerClientMessage::Hello(hello))
                    .expect("serialize hello")
                    .into(),
            ))
            .await
            .expect("send runner hello");
        expect_no_runner_command(&mut runner_receiver).await;

        let huge_diff = "x".repeat(17 * 1024 * 1024);
        let huge_payload = "y".repeat(17 * 1024 * 1024);
        let message = RunnerClientMessage::Event(Box::new(RunnerEventEnvelope {
            request_id: None,
            runner_event_seq: None,
            acked_runner_event_seq: None,
            event: RunnerEvent::SessionsDiscovered(agenter_protocol::runner::DiscoveredSessions {
                workspace,
                provider_id: AgentProviderId::from(AgentProviderId::CODEX),
                sessions: vec![agenter_protocol::runner::DiscoveredSession {
                    external_session_id: "codex-large-thread".to_owned(),
                    title: Some("Large Thread".to_owned()),
                    updated_at: None,
                    history_status:
                        agenter_protocol::runner::DiscoveredSessionHistoryStatus::Loaded,
                    history: vec![
                        agenter_protocol::runner::DiscoveredSessionHistoryItem::FileChange {
                            change_id: "change-large".to_owned(),
                            path: "large.patch".to_owned(),
                            change_kind: FileChangeKind::Modify,
                            status: agenter_protocol::runner::DiscoveredFileChangeStatus::Proposed,
                            diff: Some(huge_diff.clone()),
                            provider_payload: Some(serde_json::json!({
                                "raw": huge_payload.clone()
                            })),
                        },
                    ],
                }],
            }),
        }));

        let frames = chunk_message(&message, 1024 * 1024).expect("chunk large runner message");
        assert!(frames.len() > 3);
        for frame in frames {
            let RunnerTransportOutboundFrame::Text(text) = frame;
            assert!(text.len() < 2 * 1024 * 1024);
            runner_sender
                .send(Message::Text(text.into()))
                .await
                .expect("send chunk frame");
        }

        let mut imported_session = None;
        for _ in 0..50 {
            let sessions = state.list_sessions(user_id).await;
            imported_session = sessions.into_iter().find(|session| {
                session.external_session_id.as_deref() == Some("codex-large-thread")
            });
            if imported_session.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let imported_session = imported_session.expect("large discovered session imported");
        let history = state
            .session_history(user_id, imported_session.session_id)
            .await
            .expect("read imported history");
        let event_json = serde_json::to_string(&history).expect("serialize imported history");

        assert!(event_json.contains(&huge_diff));
        assert!(event_json.contains(&huge_payload));
        assert!(!event_json.contains("agenter_truncated"));
    }

    fn fake_hello() -> RunnerHello {
        let runner_id = RunnerId::nil();
        RunnerHello {
            runner_id,
            protocol_version: PROTOCOL_VERSION.to_owned(),
            token: "dev-token".to_owned(),
            capabilities: RunnerCapabilities {
                agent_providers: vec![AgentProviderAdvertisement {
                    provider_id: AgentProviderId::from(AgentProviderId::CODEX),
                    capabilities: AgentCapabilities {
                        streaming: true,
                        ..AgentCapabilities::default()
                    },
                }],
                transports: vec!["fake".to_owned()],
                workspace_discovery: false,
            },
            acked_runner_event_seq: None,
            replay_from_runner_event_seq: None,
            workspaces: vec![agenter_core::WorkspaceRef {
                workspace_id: WorkspaceId::nil(),
                runner_id,
                path: "/tmp/agenter-fake".to_owned(),
                display_name: Some("fake".to_owned()),
            }],
        }
    }
}

use std::{env, net::SocketAddr, time::Duration as StdDuration};

use axum::{
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Router,
};
use tower_http::trace::TraceLayer;

use crate::{
    auth::{session_token_from_headers, AuthenticatedUser, CookieSecurity},
    runner_ws,
    state::{AppState, RunnerCommandWaitError},
};

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
        .route(
            "/api/workspaces/{workspace_id}/providers/{provider_id}/sessions/refresh",
            post(sessions::refresh_workspace_provider_sessions),
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

async fn publish_runner_error_event(
    state: &AppState,
    session_id: agenter_core::SessionId,
    operation: &str,
    request_id: Option<&agenter_protocol::RequestId>,
    code: &str,
    message: &str,
) {
    state
        .publish_event(
            session_id,
            agenter_core::AppEvent::Error(agenter_core::AgentErrorEvent {
                session_id: Some(session_id),
                code: Some(code.to_owned()),
                message: message.to_owned(),
                provider_payload: Some(serde_json::json!({
                    "operation": operation,
                    "request_id": request_id.map(|id| id.to_string()),
                    "detail": message,
                })),
            }),
        )
        .await;
}

async fn publish_session_status_event(
    state: &AppState,
    session_id: agenter_core::SessionId,
    status: agenter_core::SessionStatus,
    reason: Option<String>,
) {
    state
        .publish_event(
            session_id,
            agenter_core::AppEvent::SessionStatusChanged(agenter_core::SessionStatusChangedEvent {
                session_id,
                status,
                reason,
            }),
        )
        .await;
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
        AgentCapabilities, AgentMessageDeltaEvent, AgentProviderId, AppEvent, ApprovalDecision,
        ApprovalId, ApprovalKind, ApprovalRequestEvent, FileChangeKind, RunnerId, SessionId,
        SessionInfo, WorkspaceId,
    };
    use agenter_protocol::{
        browser::{
            BrowserClientMessage, BrowserEventEnvelope, BrowserServerMessage, SubscribeSession,
        },
        chunk_message,
        runner::{
            AgentEvent, AgentProviderAdvertisement, RunnerCapabilities, RunnerClientMessage,
            RunnerEvent, RunnerEventEnvelope, RunnerHello, RunnerServerMessage, PROTOCOL_VERSION,
        },
        RequestId, RunnerTransportOutboundFrame,
    };
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
                serde_json::to_string(&RunnerClientMessage::Event(RunnerEventEnvelope {
                    request_id: Some(refresh_command.request_id.clone()),
                    event: RunnerEvent::SessionsDiscovered(agenter_protocol::runner::DiscoveredSessions {
                        workspace: workspace.clone(),
                        provider_id: AgentProviderId::from(AgentProviderId::CODEX),
                        sessions: vec![agenter_protocol::runner::DiscoveredSession {
                            external_session_id: "codex-thread-1".to_owned(),
                            title: Some("Refreshed".to_owned()),
                            history_status: agenter_protocol::runner::DiscoveredSessionHistoryStatus::Loaded,
                            history: vec![agenter_protocol::runner::DiscoveredSessionHistoryItem::AgentMessage {
                                message_id: "agent-new".to_owned(),
                                content: "new history".to_owned(),
                            }],
                        }],
                    }),
                }))
                .expect("serialize discovered event")
                .into(),
            ))
            .await
            .expect("send discovered event");
        send_runner_response(&mut runner_sender, refresh_command.request_id).await;

        let response = refresh_response.await.expect("refresh response task");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read refresh body");
        let summary: serde_json::Value = serde_json::from_slice(&body).expect("refresh json");
        assert_eq!(summary["discovered_count"], 1);
        assert_eq!(summary["refreshed_cache_count"], 1);
        assert_eq!(summary["skipped_failed_count"], 0);

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
            AppEvent::AgentMessageCompleted(_)
        ));
    }

    async fn send_runner_app_event(
        sender: &mut futures_util::stream::SplitSink<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
            Message,
        >,
        request_id: Option<RequestId>,
        session_id: SessionId,
        event: AppEvent,
    ) {
        sender
            .send(Message::Text(
                serde_json::to_string(&RunnerClientMessage::Event(RunnerEventEnvelope {
                    request_id,
                    event: RunnerEvent::AgentEvent(AgentEvent { session_id, event }),
                }))
                .expect("serialize runner event")
                .into(),
            ))
            .await
            .expect("send runner event");
    }

    async fn next_browser_event(
        receiver: &mut futures_util::stream::SplitStream<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
        >,
    ) -> BrowserEventEnvelope {
        loop {
            let frame = timeout(Duration::from_secs(2), receiver.next())
                .await
                .expect("browser event timeout")
                .expect("browser event frame")
                .expect("browser event websocket result");
            let Message::Text(text) = frame else {
                continue;
            };
            match serde_json::from_str::<BrowserServerMessage>(&text)
                .expect("decode browser message")
            {
                BrowserServerMessage::Event(event) => return event,
                BrowserServerMessage::Ack(_) => continue,
                BrowserServerMessage::Error(error) => panic!("unexpected browser error: {error:?}"),
            }
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
                serde_json::to_string(&RunnerClientMessage::Event(RunnerEventEnvelope {
                    request_id: Some(RequestId::from("event-1")),
                    event: RunnerEvent::AgentEvent(AgentEvent {
                        session_id: session.session_id,
                        event: AppEvent::AgentMessageDelta(AgentMessageDeltaEvent {
                            session_id: session.session_id,
                            message_id: "agent-1".to_owned(),
                            delta: "hello browser".to_owned(),
                            provider_payload: None,
                        }),
                    }),
                }))
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
            let BrowserServerMessage::Event(event) =
                serde_json::from_str::<BrowserServerMessage>(browser_event.to_text().unwrap())
                    .expect("decode browser event")
            else {
                panic!("expected browser app event");
            };

            if matches!(event.event, AppEvent::AgentMessageDelta(_)) {
                break;
            }
        }
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
            panic!("expected agent send input command");
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
        assert!(history
            .iter()
            .any(|entry| matches!(entry.event, AppEvent::SessionStarted(_))));
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
    async fn full_browser_fake_runner_pipeline_routes_messages_events_history_and_approvals() {
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
        let runners_response = app_service
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/runners")
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .expect("build runners request"),
            )
            .await
            .expect("route runners request");
        assert_eq!(runners_response.status(), StatusCode::OK);
        let runners_body = to_bytes(runners_response.into_body(), usize::MAX)
            .await
            .expect("read runners body");
        let runners: serde_json::Value =
            serde_json::from_slice(&runners_body).expect("runners json");
        assert_eq!(runners.as_array().expect("runner list").len(), 1);

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
                    request_id: Some(RequestId::from("sub-full")),
                    session_id: session.session_id,
                }))
                .expect("serialize subscribe")
                .into(),
            ))
            .await
            .expect("send browser subscription");
        let _session_started = next_browser_event(&mut browser_receiver).await;

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
                                serde_json::json!({ "content": "full pipeline message" })
                                    .to_string(),
                            ))
                            .expect("build message request"),
                    )
                    .await
                    .expect("route message request")
            }
        });

        let message_command = next_runner_command(&mut runner_receiver).await;
        let agenter_protocol::runner::RunnerCommand::AgentSendInput(input_command) =
            &message_command.command
        else {
            panic!("expected agent input command");
        };
        assert_eq!(input_command.session_id, session.session_id);
        assert_eq!(
            input_command.external_session_id.as_deref(),
            Some("codex-thread-1")
        );
        send_runner_response(&mut runner_sender, message_command.request_id.clone()).await;
        let send_response = send_response.await.expect("send response task");
        assert_eq!(send_response.status(), StatusCode::ACCEPTED);

        let browser_user_echo = next_browser_event(&mut browser_receiver).await;
        let AppEvent::UserMessage(user_echo) = browser_user_echo.event else {
            panic!("expected browser-submitted user echo");
        };
        assert_eq!(user_echo.session_id, session.session_id);
        assert_eq!(user_echo.content, "full pipeline message");
        assert!(user_echo.message_id.is_some());

        let approval_id = ApprovalId::new();
        send_runner_app_event(
            &mut runner_sender,
            Some(message_command.request_id.clone()),
            session.session_id,
            AppEvent::ApprovalRequested(ApprovalRequestEvent {
                session_id: session.session_id,
                approval_id,
                kind: ApprovalKind::Command,
                title: "Approve full pipeline command".to_owned(),
                details: Some("printf ok".to_owned()),
                expires_at: None,
                provider_payload: None,
            }),
        )
        .await;
        send_runner_app_event(
            &mut runner_sender,
            Some(message_command.request_id.clone()),
            session.session_id,
            AppEvent::AgentMessageDelta(AgentMessageDeltaEvent {
                session_id: session.session_id,
                message_id: "agent-full-1".to_owned(),
                delta: "full pipeline response".to_owned(),
                provider_payload: None,
            }),
        )
        .await;

        let mut saw_user = true;
        let mut saw_approval = false;
        let mut saw_delta = false;
        for _ in 0..6 {
            let event = next_browser_event(&mut browser_receiver).await;
            match event.event {
                AppEvent::UserMessage(_) => saw_user = true,
                AppEvent::ApprovalRequested(_) => saw_approval = true,
                AppEvent::AgentMessageDelta(_) => saw_delta = true,
                _ => {}
            }
            if saw_user && saw_approval && saw_delta {
                break;
            }
        }
        assert!(saw_user, "browser receives user event");
        assert!(saw_approval, "browser receives approval event");
        assert!(saw_delta, "browser receives agent delta event");

        let decision_response = app_service
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/approvals/{approval_id}/decision"))
                    .header(header::COOKIE, &cookie)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::to_string(&ApprovalDecision::Accept)
                            .expect("serialize approval decision"),
                    ))
                    .expect("build approval decision request"),
            )
            .await
            .expect("route approval decision");
        assert_eq!(decision_response.status(), StatusCode::OK);

        let approval_command = next_runner_command(&mut runner_receiver).await;
        let agenter_protocol::runner::RunnerCommand::AnswerApproval(answer) =
            approval_command.command
        else {
            panic!("expected approval answer command");
        };
        assert_eq!(answer.approval_id, approval_id);
        send_runner_response(&mut runner_sender, approval_command.request_id).await;

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
        assert!(history
            .iter()
            .any(|entry| matches!(entry.event, AppEvent::UserMessage(_))));
        assert!(history
            .iter()
            .any(|entry| matches!(entry.event, AppEvent::ApprovalRequested(_))));
        assert!(history
            .iter()
            .any(|entry| matches!(entry.event, AppEvent::AgentMessageDelta(_))));
        assert!(history
            .iter()
            .any(|entry| matches!(entry.event, AppEvent::ApprovalResolved(_))));
    }

    #[tokio::test]
    async fn approval_decision_publishes_resolved_event_for_owned_session() {
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
                AppEvent::ApprovalRequested(ApprovalRequestEvent {
                    session_id,
                    approval_id,
                    kind: ApprovalKind::Command,
                    title: "Run tests".to_owned(),
                    details: Some("cargo test".to_owned()),
                    expires_at: None,
                    provider_payload: None,
                }),
            )
            .await;
        let browser_session_token = state
            .login_password("admin@example.test", "correct horse battery staple")
            .await
            .expect("login bootstrap admin");
        let app = app(state);

        let response = app
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
                    .expect("build approval decision request"),
            )
            .await
            .expect("route approval decision");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read approval response");
        let envelope: BrowserEventEnvelope =
            serde_json::from_slice(&body).expect("approval response json");
        assert!(matches!(envelope.event, AppEvent::ApprovalResolved(_)));

        let RunnerServerMessage::Command(command) = observed_receiver
            .try_recv()
            .expect("runner receives approval answer")
        else {
            panic!("expected runner command");
        };
        let agenter_protocol::runner::RunnerCommand::AnswerApproval(answer) = command.command
        else {
            panic!("expected approval answer command");
        };
        assert_eq!(answer.approval_id, approval_id);

        let duplicate_response = app
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
                AppEvent::ApprovalRequested(ApprovalRequestEvent {
                    session_id,
                    approval_id,
                    kind: ApprovalKind::Command,
                    title: "Run tests".to_owned(),
                    details: Some("cargo test".to_owned()),
                    expires_at: None,
                    provider_payload: None,
                }),
            )
            .await;
        let resolved = state
            .publish_event(
                session_id,
                AppEvent::ApprovalResolved(agenter_core::ApprovalResolvedEvent {
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

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read stale approval response");
        let envelope: BrowserEventEnvelope =
            serde_json::from_slice(&body).expect("stale approval response json");
        assert_eq!(envelope.event_id, resolved.event_id);
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
        let message = RunnerClientMessage::Event(RunnerEventEnvelope {
            request_id: None,
            event: RunnerEvent::SessionsDiscovered(agenter_protocol::runner::DiscoveredSessions {
                workspace,
                provider_id: AgentProviderId::from(AgentProviderId::CODEX),
                sessions: vec![agenter_protocol::runner::DiscoveredSession {
                    external_session_id: "codex-large-thread".to_owned(),
                    title: Some("Large Thread".to_owned()),
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
        });

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
            workspaces: vec![agenter_core::WorkspaceRef {
                workspace_id: WorkspaceId::nil(),
                runner_id,
                path: "/tmp/agenter-fake".to_owned(),
                display_name: Some("fake".to_owned()),
            }],
        }
    }
}

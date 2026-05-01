use std::{env, net::SocketAddr, time::Duration as StdDuration};

use axum::{
    extract::{ws::WebSocketUpgrade, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use tower_http::trace::TraceLayer;
use uuid::Uuid;

use crate::{
    auth::{self, CookieSecurity},
    browser_ws, runner_ws,
    state::{
        AppState, ApprovalResolutionStart, RunnerCommandWaitError, RunnerSendError,
        SessionRegistration,
    },
};

pub const DEFAULT_BIND_ADDR: &str = "127.0.0.1:7777";
pub const DEFAULT_DEV_RUNNER_TOKEN: &str = "dev-runner-token";
const RUNNER_COMMAND_RESPONSE_TIMEOUT: StdDuration = StdDuration::from_secs(30);

pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/api/auth/password/login", post(auth_login))
        .route("/api/auth/password/logout", post(auth_logout))
        .route("/api/auth/me", get(auth_me))
        .route("/api/auth/oidc/{provider_id}/login", get(oidc_login))
        .route("/api/auth/oidc/{provider_id}/callback", post(oidc_callback))
        .route("/api/link-codes", post(create_link_code))
        .route("/api/link/{code}", post(consume_link_code))
        .route("/api/runners", get(list_runners))
        .route(
            "/api/runners/{runner_id}/workspaces",
            get(list_runner_workspaces),
        )
        .route("/api/sessions", get(list_sessions).post(create_session))
        .route(
            "/api/sessions/{session_id}",
            get(get_session).patch(update_session),
        )
        .route(
            "/api/sessions/{session_id}/messages",
            post(send_session_message),
        )
        .route(
            "/api/sessions/{session_id}/agent-options",
            get(session_agent_options),
        )
        .route(
            "/api/sessions/{session_id}/settings",
            get(get_session_settings).patch(update_session_settings),
        )
        .route("/api/sessions/{session_id}/history", get(session_history))
        .route("/api/approvals", get(list_approvals))
        .route(
            "/api/approvals/{approval_id}/decision",
            post(decide_approval),
        )
        .route("/api/questions/{question_id}/answer", post(answer_question))
        .route("/api/runner/ws", get(runner_ws::handler))
        .route("/api/browser/ws", get(browser_ws_authenticated))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
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

#[derive(Debug, Deserialize)]
struct PasswordLoginRequest {
    email: String,
    password: String,
}

#[derive(Debug, Serialize)]
struct RunnerInfoResponse {
    runner_id: agenter_core::RunnerId,
    name: String,
    status: &'static str,
    last_seen_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateSessionRequest {
    workspace_id: agenter_core::WorkspaceId,
    provider_id: agenter_core::AgentProviderId,
    title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SendMessageRequest {
    content: String,
}

#[derive(Debug, Deserialize)]
struct UpdateSessionRequest {
    title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UpdateSessionSettingsRequest {
    #[serde(flatten)]
    settings: agenter_core::AgentTurnSettings,
}

#[derive(Debug, Deserialize)]
struct OidcCallbackRequest {
    state: String,
    subject: String,
    email: String,
    display_name: Option<String>,
}

#[derive(Debug, Serialize)]
struct OidcLoginResponse {
    provider_id: String,
    state: String,
    nonce: String,
    authorization_url: String,
}

#[derive(Debug, Deserialize)]
struct CreateLinkCodeRequest {
    connector_id: String,
    external_account_id: String,
    display_name: Option<String>,
}

#[derive(Debug, Serialize)]
struct CreateLinkCodeResponse {
    code: String,
    expires_at: chrono::DateTime<Utc>,
}

async fn auth_login(
    State(state): State<AppState>,
    Json(request): Json<PasswordLoginRequest>,
) -> Response {
    tracing::debug!(email = %request.email, "password login requested");
    let Some(token) = state
        .login_password(&request.email, &request.password)
        .await
    else {
        tracing::warn!(email = %request.email, "password login rejected");
        return StatusCode::UNAUTHORIZED.into_response();
    };
    tracing::info!(email = %request.email, "password login accepted");

    (
        StatusCode::OK,
        [(
            axum::http::header::SET_COOKIE,
            auth::session_cookie_with_policy(&token, state.cookie_security()),
        )],
        Json(serde_json::json!({ "ok": true })),
    )
        .into_response()
}

async fn auth_logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    tracing::debug!("logout requested");
    if let Some(token) = auth::session_token_from_headers(&headers) {
        state.logout(token).await;
        tracing::info!("session logged out");
    }

    (
        StatusCode::NO_CONTENT,
        [(
            axum::http::header::SET_COOKIE,
            auth::expired_session_cookie_with_policy(state.cookie_security()),
        )],
    )
        .into_response()
}

async fn auth_me(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(user) = authenticated_user_from_headers(&state, &headers).await else {
        tracing::debug!("auth me rejected missing or invalid session");
        return StatusCode::UNAUTHORIZED.into_response();
    };
    tracing::debug!(user_id = %user.user_id, "auth me accepted");

    Json(user).into_response()
}

async fn oidc_login(State(state): State<AppState>, Path(provider_id): Path<String>) -> Response {
    tracing::debug!(%provider_id, "oidc login requested");
    let Some(pool) = state.db_pool() else {
        tracing::warn!(%provider_id, "oidc login requested without database");
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let Ok(Some(provider)) = agenter_db::find_oidc_provider(pool, &provider_id).await else {
        tracing::warn!(%provider_id, "oidc provider not found");
        return StatusCode::NOT_FOUND.into_response();
    };
    let state_token = Uuid::new_v4().to_string();
    let nonce = Uuid::new_v4().to_string();
    let expires_at = Utc::now() + Duration::minutes(10);
    if agenter_db::create_oidc_login_state(
        pool,
        &state_token,
        &provider.provider_id,
        &nonce,
        None,
        Some("/"),
        expires_at,
    )
    .await
    .is_err()
    {
        tracing::error!(%provider_id, "failed to create oidc login state");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let authorization_url = oidc_authorization_url(&provider, &state_token, &nonce);
    Json(OidcLoginResponse {
        provider_id: provider.provider_id,
        state: state_token,
        nonce,
        authorization_url,
    })
    .into_response()
}

async fn oidc_callback(
    State(state): State<AppState>,
    Path(provider_id): Path<String>,
    Json(request): Json<OidcCallbackRequest>,
) -> Response {
    if !unsafe_dev_oidc_callback_enabled() {
        tracing::warn!(%provider_id, "unsafe development oidc callback rejected because flag is disabled");
        return StatusCode::NOT_IMPLEMENTED.into_response();
    }
    let Some(pool) = state.db_pool() else {
        tracing::warn!(%provider_id, "oidc callback requested without database");
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let Ok(Some(login_state)) =
        agenter_db::consume_oidc_login_state(pool, &provider_id, &request.state, Utc::now()).await
    else {
        tracing::warn!(%provider_id, "oidc callback state rejected");
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if login_state.provider_id != provider_id {
        tracing::warn!(%provider_id, login_state_provider_id = %login_state.provider_id, "oidc callback provider mismatch");
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Ok(user) = agenter_db::upsert_oidc_identity(
        pool,
        &provider_id,
        &request.subject,
        &request.email,
        request.display_name.as_deref(),
    )
    .await
    else {
        tracing::error!(%provider_id, "failed to bind oidc identity");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let token = state
        .create_authenticated_session(auth::AuthenticatedUser {
            user_id: user.user_id,
            email: user.email,
            display_name: user.display_name,
        })
        .await;

    (
        StatusCode::OK,
        [(
            axum::http::header::SET_COOKIE,
            auth::session_cookie_with_policy(&token, state.cookie_security()),
        )],
        Json(serde_json::json!({ "ok": true })),
    )
        .into_response()
}

async fn create_link_code(
    State(state): State<AppState>,
    Json(request): Json<CreateLinkCodeRequest>,
) -> Response {
    if !unsafe_dev_link_code_creation_enabled() {
        tracing::warn!("unsafe development link-code creation rejected because flag is disabled");
        return StatusCode::NOT_IMPLEMENTED.into_response();
    }
    let Some(pool) = state.db_pool() else {
        tracing::warn!("link-code creation requested without database");
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let code = Uuid::new_v4().to_string();
    let expires_at = Utc::now() + Duration::minutes(15);
    let Ok(link_code) = agenter_db::create_connector_link_code(
        pool,
        &code,
        &request.connector_id,
        &request.external_account_id,
        request.display_name.as_deref(),
        expires_at,
    )
    .await
    else {
        tracing::error!(connector_id = %request.connector_id, "failed to create connector link code");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };

    (
        StatusCode::CREATED,
        Json(CreateLinkCodeResponse {
            code: link_code.code,
            expires_at: link_code.expires_at,
        }),
    )
        .into_response()
}

async fn consume_link_code(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(code): Path<String>,
) -> Response {
    let Some(user) = authenticated_user_from_headers(&state, &headers).await else {
        tracing::debug!("link-code consume rejected missing or invalid session");
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let Some(pool) = state.db_pool() else {
        tracing::warn!(user_id = %user.user_id, "link-code consume requested without database");
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    match agenter_db::consume_connector_link_code(pool, &code, user.user_id, Utc::now()).await {
        Ok(Some(account)) => {
            tracing::info!(
                user_id = %user.user_id,
                connector_id = %account.connector_id,
                "connector account linked"
            );
            Json(serde_json::json!({
                "connector_id": account.connector_id,
                "external_account_id": account.external_account_id,
                "display_name": account.display_name
            }))
            .into_response()
        }
        Ok(None) => {
            tracing::warn!(user_id = %user.user_id, "link-code consume rejected");
            StatusCode::NOT_FOUND.into_response()
        }
        Err(error) => {
            tracing::error!(user_id = %user.user_id, %error, "failed to consume connector link code");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

fn oidc_authorization_url(
    provider: &agenter_db::models::OidcProvider,
    state: &str,
    nonce: &str,
) -> String {
    format!(
        "{}/authorize?client_id={}&response_type=code&scope={}&state={}&nonce={}",
        provider.issuer_url.trim_end_matches('/'),
        provider.client_id,
        provider.scopes.join("%20"),
        state,
        nonce
    )
}

fn unsafe_dev_oidc_callback_enabled() -> bool {
    env_flag_enabled("AGENTER_UNSAFE_DEV_OIDC_CALLBACK")
}

fn unsafe_dev_link_code_creation_enabled() -> bool {
    env_flag_enabled("AGENTER_UNSAFE_DEV_LINK_CODE_CREATION")
}

fn env_flag_enabled(name: &str) -> bool {
    env::var(name).is_ok_and(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "True"))
}

async fn list_runners(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if authenticated_user_from_headers(&state, &headers)
        .await
        .is_none()
    {
        tracing::debug!("list runners rejected missing or invalid session");
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let runners: Vec<_> = state
        .list_runners_with_connection_status()
        .await
        .into_iter()
        .map(|entry| RunnerInfoResponse {
            runner_id: entry.runner.runner_id,
            name: entry
                .runner
                .workspaces
                .first()
                .and_then(|workspace| workspace.display_name.clone())
                .or_else(|| {
                    entry
                        .runner
                        .capabilities
                        .agent_providers
                        .first()
                        .map(|provider| provider.provider_id.to_string())
                })
                .unwrap_or_else(|| entry.runner.runner_id.to_string()),
            status: if entry.connected { "online" } else { "offline" },
            last_seen_at: None,
        })
        .collect();
    tracing::debug!(runner_count = runners.len(), "listed runners");
    Json(runners).into_response()
}

async fn list_runner_workspaces(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(runner_id): Path<agenter_core::RunnerId>,
) -> Response {
    if authenticated_user_from_headers(&state, &headers)
        .await
        .is_none()
    {
        tracing::debug!(%runner_id, "list runner workspaces rejected missing or invalid session");
        return StatusCode::UNAUTHORIZED.into_response();
    }

    match state.list_runner_workspaces(runner_id).await {
        Some(workspaces) => {
            tracing::debug!(%runner_id, workspace_count = workspaces.len(), "listed runner workspaces");
            Json(workspaces).into_response()
        }
        None => {
            tracing::warn!(%runner_id, "runner workspaces not found");
            StatusCode::NOT_FOUND.into_response()
        }
    }
}

async fn list_sessions(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(user) = authenticated_user_from_headers(&state, &headers).await else {
        tracing::debug!("list sessions rejected missing or invalid session");
        return StatusCode::UNAUTHORIZED.into_response();
    };

    let sessions = state.list_sessions(user.user_id).await;
    tracing::debug!(user_id = %user.user_id, session_count = sessions.len(), "listed sessions");
    Json(sessions).into_response()
}

async fn get_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<agenter_core::SessionId>,
) -> Response {
    let Some(user) = authenticated_user_from_headers(&state, &headers).await else {
        tracing::debug!(%session_id, "get session rejected missing or invalid session");
        return StatusCode::UNAUTHORIZED.into_response();
    };

    let Some(session) = state.session(user.user_id, session_id).await else {
        tracing::warn!(user_id = %user.user_id, %session_id, "session not found or forbidden");
        return StatusCode::NOT_FOUND.into_response();
    };

    Json(agenter_core::SessionInfo {
        session_id: session.session_id,
        owner_user_id: session.owner_user_id,
        runner_id: session.runner_id,
        workspace_id: session.workspace.workspace_id,
        provider_id: session.provider_id,
        status: session.status,
        external_session_id: session.external_session_id,
        title: session.title,
        created_at: Some(session.created_at),
        updated_at: Some(session.updated_at),
    })
    .into_response()
}

async fn update_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<agenter_core::SessionId>,
    Json(request): Json<UpdateSessionRequest>,
) -> Response {
    let Some(user) = authenticated_user_from_headers(&state, &headers).await else {
        tracing::debug!(%session_id, "update session rejected missing or invalid session");
        return StatusCode::UNAUTHORIZED.into_response();
    };

    let title = request.title.and_then(|title| {
        let trimmed = title.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    });
    let Some(session) = state
        .update_session_title(user.user_id, session_id, title)
        .await
    else {
        tracing::warn!(user_id = %user.user_id, %session_id, "session update not found or forbidden");
        return StatusCode::NOT_FOUND.into_response();
    };

    Json(session).into_response()
}

async fn create_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateSessionRequest>,
) -> Response {
    let Some(user) = authenticated_user_from_headers(&state, &headers).await else {
        tracing::debug!("create session rejected missing or invalid session");
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let workspace_id = request.workspace_id;
    let provider_id = request.provider_id;

    let Some((runner_id, workspace)) = state
        .resolve_runner_workspace(workspace_id, &provider_id)
        .await
    else {
        tracing::warn!(
            user_id = %user.user_id,
            %workspace_id,
            provider_id = %provider_id,
            "session creation failed: no connected runner workspace/provider match"
        );
        return StatusCode::NOT_FOUND.into_response();
    };
    let session_id = agenter_core::SessionId::new();
    if provider_id.as_str() != agenter_core::AgentProviderId::CODEX {
        let session = state
            .register_session(SessionRegistration {
                session_id,
                owner_user_id: user.user_id,
                runner_id,
                workspace,
                provider_id,
                title: request.title,
                external_session_id: None,
                turn_settings: None,
            })
            .await;
        let info = agenter_core::SessionInfo {
            session_id: session.session_id,
            owner_user_id: session.owner_user_id,
            runner_id: session.runner_id,
            workspace_id: session.workspace.workspace_id,
            provider_id: session.provider_id,
            status: session.status,
            external_session_id: session.external_session_id,
            title: session.title,
            created_at: Some(session.created_at),
            updated_at: Some(session.updated_at),
        };
        state
            .publish_event(
                session.session_id,
                agenter_core::AppEvent::SessionStarted(info.clone()),
            )
            .await;
        return (StatusCode::CREATED, Json(info)).into_response();
    }

    let request_id = agenter_protocol::RequestId::from(Uuid::new_v4().to_string());
    let title = request.title;
    let command = agenter_protocol::runner::RunnerServerMessage::Command(Box::new(
        agenter_protocol::runner::RunnerCommandEnvelope {
            request_id: request_id.clone(),
            command: agenter_protocol::runner::RunnerCommand::CreateSession(
                agenter_protocol::runner::CreateSessionCommand {
                    session_id,
                    workspace: workspace.clone(),
                    provider_id: provider_id.clone(),
                    initial_input: None,
                },
            ),
        },
    ));
    let outcome = match state
        .send_runner_command_and_wait(
            runner_id,
            request_id,
            command,
            RUNNER_COMMAND_RESPONSE_TIMEOUT,
        )
        .await
    {
        Ok(outcome) => outcome,
        Err(error) => {
            tracing::warn!(
                user_id = %user.user_id,
                %session_id,
                %runner_id,
                ?error,
                "session creation failed while waiting for runner"
            );
            return runner_wait_error_status(error).into_response();
        }
    };
    let external_session_id = match outcome {
        agenter_protocol::runner::RunnerResponseOutcome::Ok {
            result:
                agenter_protocol::runner::RunnerCommandResult::SessionCreated {
                    session_id: returned_session_id,
                    external_session_id,
                },
        } if returned_session_id == session_id => external_session_id,
        agenter_protocol::runner::RunnerResponseOutcome::Error { error } => {
            tracing::warn!(
                user_id = %user.user_id,
                %session_id,
                %runner_id,
                code = %error.code,
                message = %error.message,
                "runner rejected session creation"
            );
            return StatusCode::BAD_GATEWAY.into_response();
        }
        other => {
            tracing::warn!(
                user_id = %user.user_id,
                %session_id,
                %runner_id,
                ?other,
                "runner returned unexpected create-session response"
            );
            return StatusCode::BAD_GATEWAY.into_response();
        }
    };

    let session = state
        .register_session(SessionRegistration {
            session_id,
            owner_user_id: user.user_id,
            runner_id,
            workspace,
            provider_id,
            title,
            external_session_id: Some(external_session_id),
            turn_settings: None,
        })
        .await;
    tracing::info!(
        user_id = %user.user_id,
        session_id = %session.session_id,
        runner_id = %session.runner_id,
        workspace_id = %session.workspace.workspace_id,
        provider_id = %session.provider_id,
        "session created"
    );

    let info = agenter_core::SessionInfo {
        session_id: session.session_id,
        owner_user_id: session.owner_user_id,
        runner_id: session.runner_id,
        workspace_id: session.workspace.workspace_id,
        provider_id: session.provider_id,
        status: session.status,
        external_session_id: session.external_session_id,
        title: session.title,
        created_at: Some(session.created_at),
        updated_at: Some(session.updated_at),
    };
    state
        .publish_event(
            session.session_id,
            agenter_core::AppEvent::SessionStarted(info.clone()),
        )
        .await;

    (StatusCode::CREATED, Json(info)).into_response()
}

async fn send_session_message(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<agenter_core::SessionId>,
    Json(request): Json<SendMessageRequest>,
) -> Response {
    let Some(user) = authenticated_user_from_headers(&state, &headers).await else {
        tracing::debug!(%session_id, "send session message rejected missing or invalid session");
        return StatusCode::UNAUTHORIZED.into_response();
    };

    let Some(session) = state.session(user.user_id, session_id).await else {
        tracing::warn!(user_id = %user.user_id, %session_id, "send session message rejected session not found");
        return StatusCode::NOT_FOUND.into_response();
    };
    let content = request.content.trim();
    if content.is_empty() {
        tracing::debug!(user_id = %user.user_id, %session_id, "send session message rejected empty content");
        return StatusCode::BAD_REQUEST.into_response();
    }
    let settings = state.session_turn_settings(user.user_id, session_id).await;

    let message = agenter_protocol::runner::RunnerServerMessage::Command(Box::new(
        agenter_protocol::runner::RunnerCommandEnvelope {
            request_id: agenter_protocol::RequestId::from(Uuid::new_v4().to_string()),
            command: agenter_protocol::runner::RunnerCommand::AgentSendInput(
                agenter_protocol::runner::AgentInputCommand {
                    session_id,
                    external_session_id: session.external_session_id,
                    settings,
                    input: agenter_protocol::runner::AgentInput::UserMessage {
                        payload: agenter_core::UserMessageEvent {
                            session_id,
                            message_id: Some(Uuid::new_v4().to_string()),
                            author_user_id: Some(user.user_id),
                            content: content.to_owned(),
                        },
                    },
                },
            ),
        },
    ));
    let request_id = match &message {
        agenter_protocol::runner::RunnerServerMessage::Command(command) => {
            command.request_id.clone()
        }
        _ => unreachable!("constructed runner command"),
    };

    match state
        .send_runner_command_and_wait(
            session.runner_id,
            request_id,
            message,
            RUNNER_COMMAND_RESPONSE_TIMEOUT,
        )
        .await
    {
        Ok(agenter_protocol::runner::RunnerResponseOutcome::Ok {
            result: agenter_protocol::runner::RunnerCommandResult::Accepted,
        }) => {
            tracing::info!(
                user_id = %user.user_id,
                %session_id,
                runner_id = %session.runner_id,
                content_len = content.len(),
                "session message accepted by runner channel"
            );
            StatusCode::ACCEPTED.into_response()
        }
        Ok(agenter_protocol::runner::RunnerResponseOutcome::Error { error }) => {
            tracing::warn!(
                user_id = %user.user_id,
                %session_id,
                runner_id = %session.runner_id,
                code = %error.code,
                message = %error.message,
                "session message rejected by runner"
            );
            StatusCode::BAD_GATEWAY.into_response()
        }
        Ok(other) => {
            tracing::warn!(
                user_id = %user.user_id,
                %session_id,
                runner_id = %session.runner_id,
                ?other,
                "session message received unexpected runner response"
            );
            StatusCode::BAD_GATEWAY.into_response()
        }
        Err(error) => {
            tracing::warn!(
                user_id = %user.user_id,
                %session_id,
                runner_id = %session.runner_id,
                ?error,
                "session message failed because runner is unavailable"
            );
            runner_wait_error_status(error).into_response()
        }
    }
}

fn runner_wait_error_status(error: RunnerCommandWaitError) -> StatusCode {
    match error {
        RunnerCommandWaitError::NotConnected
        | RunnerCommandWaitError::Closed
        | RunnerCommandWaitError::StaleApproval => StatusCode::SERVICE_UNAVAILABLE,
        RunnerCommandWaitError::TimedOut => StatusCode::GATEWAY_TIMEOUT,
    }
}

async fn session_agent_options(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<agenter_core::SessionId>,
) -> Response {
    let Some(user) = authenticated_user_from_headers(&state, &headers).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let Some(session) = state.session(user.user_id, session_id).await else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let request_id = agenter_protocol::RequestId::from(Uuid::new_v4().to_string());
    let command = agenter_protocol::runner::RunnerServerMessage::Command(Box::new(
        agenter_protocol::runner::RunnerCommandEnvelope {
            request_id: request_id.clone(),
            command: agenter_protocol::runner::RunnerCommand::GetAgentOptions(
                agenter_protocol::runner::GetAgentOptionsCommand {
                    session_id,
                    provider_id: session.provider_id,
                },
            ),
        },
    ));

    match state
        .send_runner_command_and_wait(
            session.runner_id,
            request_id,
            command,
            RUNNER_COMMAND_RESPONSE_TIMEOUT,
        )
        .await
    {
        Ok(agenter_protocol::runner::RunnerResponseOutcome::Ok {
            result: agenter_protocol::runner::RunnerCommandResult::AgentOptions { options },
        }) => Json(options).into_response(),
        Ok(agenter_protocol::runner::RunnerResponseOutcome::Error { error }) => {
            tracing::warn!(
                %session_id,
                code = %error.code,
                message = %error.message,
                "runner could not load agent options"
            );
            Json(agenter_core::AgentOptions::default()).into_response()
        }
        Ok(other) => {
            tracing::warn!(%session_id, ?other, "unexpected runner options response");
            Json(agenter_core::AgentOptions::default()).into_response()
        }
        Err(error) => {
            tracing::warn!(%session_id, ?error, "agent options unavailable");
            Json(agenter_core::AgentOptions::default()).into_response()
        }
    }
}

async fn get_session_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<agenter_core::SessionId>,
) -> Response {
    let Some(user) = authenticated_user_from_headers(&state, &headers).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if !state.can_access_session(user.user_id, session_id).await {
        return StatusCode::NOT_FOUND.into_response();
    }
    Json(
        state
            .session_turn_settings(user.user_id, session_id)
            .await
            .unwrap_or_default(),
    )
    .into_response()
}

async fn update_session_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<agenter_core::SessionId>,
    Json(request): Json<UpdateSessionSettingsRequest>,
) -> Response {
    let Some(user) = authenticated_user_from_headers(&state, &headers).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let Some(settings) = state
        .update_session_turn_settings(user.user_id, session_id, request.settings)
        .await
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    Json(settings).into_response()
}

async fn session_history(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<agenter_core::SessionId>,
) -> Response {
    let Some(user) = authenticated_user_from_headers(&state, &headers).await else {
        tracing::debug!(%session_id, "session history rejected missing or invalid session");
        return StatusCode::UNAUTHORIZED.into_response();
    };

    let Some(history) = state.session_history(user.user_id, session_id).await else {
        tracing::warn!(user_id = %user.user_id, %session_id, "session history rejected session not found");
        return StatusCode::NOT_FOUND.into_response();
    };

    tracing::debug!(user_id = %user.user_id, %session_id, event_count = history.len(), "returned session history");
    Json(history).into_response()
}

async fn list_approvals(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if authenticated_user_from_headers(&state, &headers)
        .await
        .is_none()
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    Json(Vec::<agenter_protocol::browser::BrowserEventEnvelope>::new()).into_response()
}

async fn decide_approval(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(approval_id): Path<agenter_core::ApprovalId>,
    Json(decision): Json<agenter_core::ApprovalDecision>,
) -> Response {
    let Some(user) = authenticated_user_from_headers(&state, &headers).await else {
        tracing::debug!(%approval_id, "approval decision rejected missing or invalid session");
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let session_id = match state.begin_approval_resolution(approval_id).await {
        ApprovalResolutionStart::Started { session_id } => session_id,
        ApprovalResolutionStart::AlreadyResolved {
            session_id,
            envelope,
        } => {
            if state.session(user.user_id, session_id).await.is_none() {
                return StatusCode::NOT_FOUND.into_response();
            }
            return Json(*envelope).into_response();
        }
        ApprovalResolutionStart::InProgress => {
            tracing::warn!(%approval_id, "approval decision rejected because resolution is already in progress");
            return StatusCode::CONFLICT.into_response();
        }
        ApprovalResolutionStart::Missing => {
            tracing::warn!(%approval_id, "approval decision rejected missing approval");
            return StatusCode::NOT_FOUND.into_response();
        }
    };
    let Some(session) = state.session(user.user_id, session_id).await else {
        state.cancel_approval_resolution(approval_id).await;
        tracing::warn!(user_id = %user.user_id, %approval_id, %session_id, "approval decision rejected session not found");
        return StatusCode::NOT_FOUND.into_response();
    };

    let command = agenter_protocol::runner::RunnerServerMessage::Command(Box::new(
        agenter_protocol::runner::RunnerCommandEnvelope {
            request_id: agenter_protocol::RequestId::from(Uuid::new_v4().to_string()),
            command: agenter_protocol::runner::RunnerCommand::AnswerApproval(
                agenter_protocol::runner::ApprovalAnswerCommand {
                    session_id,
                    approval_id,
                    decision: decision.clone(),
                },
            ),
        },
    ));
    match state.send_runner_message(session.runner_id, command).await {
        Ok(()) => {}
        Err(RunnerSendError::NotConnected | RunnerSendError::Closed) => {
            state.cancel_approval_resolution(approval_id).await;
            tracing::warn!(
                user_id = %user.user_id,
                %approval_id,
                %session_id,
                runner_id = %session.runner_id,
                "approval decision failed because runner is unavailable"
            );
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        Err(RunnerSendError::StaleApproval) => {
            tracing::warn!(%approval_id, %session_id, "approval decision raced with runner-side resolution");
            return match state.begin_approval_resolution(approval_id).await {
                ApprovalResolutionStart::AlreadyResolved {
                    session_id,
                    envelope,
                } => {
                    if state.session(user.user_id, session_id).await.is_none() {
                        StatusCode::NOT_FOUND.into_response()
                    } else {
                        Json(*envelope).into_response()
                    }
                }
                _ => StatusCode::CONFLICT.into_response(),
            };
        }
    }

    let resolved = agenter_core::AppEvent::ApprovalResolved(agenter_core::ApprovalResolvedEvent {
        session_id,
        approval_id,
        decision: decision.clone(),
        resolved_by_user_id: Some(user.user_id),
        resolved_at: Utc::now(),
        provider_payload: None,
    });
    let Some(envelope) = state
        .finish_approval_resolution(approval_id, session_id, resolved)
        .await
    else {
        tracing::warn!(%approval_id, %session_id, "approval decision could not finish resolution");
        return StatusCode::CONFLICT.into_response();
    };

    tracing::info!(user_id = %user.user_id, %approval_id, %session_id, "approval decision resolved");
    Json(envelope).into_response()
}

async fn answer_question(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(question_id): Path<agenter_core::QuestionId>,
    Json(mut answer): Json<agenter_core::AgentQuestionAnswer>,
) -> Response {
    let Some(user) = authenticated_user_from_headers(&state, &headers).await else {
        tracing::debug!(%question_id, "question answer rejected missing or invalid session");
        return StatusCode::UNAUTHORIZED.into_response();
    };
    answer.question_id = question_id;
    let Some(session_id) = state.question_session(question_id).await else {
        tracing::warn!(%question_id, "question answer rejected missing question");
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(session) = state.session(user.user_id, session_id).await else {
        tracing::warn!(user_id = %user.user_id, %question_id, %session_id, "question answer rejected session not found");
        return StatusCode::NOT_FOUND.into_response();
    };
    let request_id = agenter_protocol::RequestId::from(Uuid::new_v4().to_string());
    let command = agenter_protocol::runner::RunnerServerMessage::Command(Box::new(
        agenter_protocol::runner::RunnerCommandEnvelope {
            request_id: request_id.clone(),
            command: agenter_protocol::runner::RunnerCommand::AnswerQuestion(
                agenter_protocol::runner::QuestionAnswerCommand {
                    session_id,
                    answer: answer.clone(),
                },
            ),
        },
    ));

    match state
        .send_runner_command_and_wait(
            session.runner_id,
            request_id,
            command,
            RUNNER_COMMAND_RESPONSE_TIMEOUT,
        )
        .await
    {
        Ok(agenter_protocol::runner::RunnerResponseOutcome::Ok {
            result: agenter_protocol::runner::RunnerCommandResult::Accepted,
        }) => {
            let envelope = state.finish_question_answer(session_id, answer).await;
            Json(envelope).into_response()
        }
        Ok(agenter_protocol::runner::RunnerResponseOutcome::Error { error }) => {
            tracing::warn!(
                user_id = %user.user_id,
                %question_id,
                code = %error.code,
                message = %error.message,
                "question answer rejected by runner"
            );
            StatusCode::BAD_GATEWAY.into_response()
        }
        Ok(other) => {
            tracing::warn!(user_id = %user.user_id, %question_id, ?other, "question answer received unexpected runner response");
            StatusCode::BAD_GATEWAY.into_response()
        }
        Err(error) => {
            tracing::warn!(user_id = %user.user_id, %question_id, ?error, "question answer failed because runner is unavailable");
            runner_wait_error_status(error).into_response()
        }
    }
}

async fn browser_ws_authenticated(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let Some(user) = authenticated_user_from_headers(&state, &headers).await else {
        tracing::debug!("browser websocket rejected missing or invalid session");
        return StatusCode::UNAUTHORIZED.into_response();
    };

    tracing::debug!(user_id = %user.user_id, "browser websocket accepted");
    browser_ws::handler(ws, State(state), user.user_id).await
}

async fn authenticated_user_from_headers(
    state: &AppState,
    headers: &HeaderMap,
) -> Option<auth::AuthenticatedUser> {
    let token = auth::session_token_from_headers(headers)?;
    state.authenticated_user(token).await
}

#[cfg(test)]
mod tests {
    use agenter_core::{
        AgentCapabilities, AgentMessageDeltaEvent, AgentProviderId, AppEvent, ApprovalDecision,
        ApprovalId, ApprovalKind, ApprovalRequestEvent, RunnerId, SessionId, SessionInfo,
        WorkspaceId,
    };
    use agenter_protocol::{
        browser::{
            BrowserClientMessage, BrowserEventEnvelope, BrowserServerMessage, SubscribeSession,
        },
        runner::{
            AgentEvent, AgentProviderAdvertisement, RunnerCapabilities, RunnerClientMessage,
            RunnerEvent, RunnerEventEnvelope, RunnerHello, RunnerServerMessage, PROTOCOL_VERSION,
        },
        RequestId,
    };
    use axum::{
        body::{to_bytes, Body},
        http::{header, Request, StatusCode},
    };
    use futures_util::{SinkExt, StreamExt};
    use tokio::net::TcpListener;
    use tokio::time::{timeout, Duration};
    use tokio_tungstenite::{
        connect_async,
        tungstenite::{client::IntoClientRequest, Message},
    };
    use tower::ServiceExt;

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
                        format!("{}={browser_session_token}", auth::SESSION_COOKIE_NAME),
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

        let cookie = format!("{}={browser_session_token}", auth::SESSION_COOKIE_NAME);
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

        let cookie = format!("{}={browser_session_token}", auth::SESSION_COOKIE_NAME);
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

        let cookie = format!("{}={browser_session_token}", auth::SESSION_COOKIE_NAME);
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

        let cookie = format!("{}={browser_session_token}", auth::SESSION_COOKIE_NAME);
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

        let approval_id = ApprovalId::new();
        send_runner_app_event(
            &mut runner_sender,
            Some(message_command.request_id.clone()),
            session.session_id,
            AppEvent::UserMessage(agenter_core::UserMessageEvent {
                session_id: session.session_id,
                message_id: Some("user-full-1".to_owned()),
                author_user_id: None,
                content: "full pipeline message".to_owned(),
            }),
        )
        .await;
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

        let mut saw_user = false;
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
                        format!("{}={browser_session_token}", auth::SESSION_COOKIE_NAME),
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
                        format!("{}={browser_session_token}", auth::SESSION_COOKIE_NAME),
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
                        format!("{}={browser_session_token}", auth::SESSION_COOKIE_NAME),
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
                        format!("{}={browser_session_token}", auth::SESSION_COOKIE_NAME),
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
            format!("{}={browser_session_token}", auth::SESSION_COOKIE_NAME)
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

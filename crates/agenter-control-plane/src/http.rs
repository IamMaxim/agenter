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
        .route(
            "/api/workspaces/{workspace_id}/providers/{provider_id}/sessions/refresh",
            post(refresh_workspace_provider_sessions),
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
            "/api/sessions/{session_id}/slash-commands",
            get(list_slash_commands).post(execute_slash_command),
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

async fn refresh_workspace_provider_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((workspace_id, provider_id)): Path<(agenter_core::WorkspaceId, String)>,
) -> Response {
    let Some(user) = authenticated_user_from_headers(&state, &headers).await else {
        tracing::debug!(%workspace_id, provider_id, "session refresh rejected missing or invalid session");
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let provider_id = agenter_core::AgentProviderId::from(provider_id);
    let Some((runner_id, workspace)) = state
        .resolve_runner_workspace(workspace_id, &provider_id)
        .await
    else {
        tracing::warn!(
            user_id = %user.user_id,
            %workspace_id,
            provider_id = %provider_id,
            "session refresh failed: no connected runner workspace/provider match"
        );
        return StatusCode::NOT_FOUND.into_response();
    };

    let request_id = agenter_protocol::RequestId::from(Uuid::new_v4().to_string());
    let command = agenter_protocol::runner::RunnerServerMessage::Command(Box::new(
        agenter_protocol::runner::RunnerCommandEnvelope {
            request_id: request_id.clone(),
            command: agenter_protocol::runner::RunnerCommand::RefreshSessions(
                agenter_protocol::runner::RefreshSessionsCommand {
                    workspace,
                    provider_id,
                },
            ),
        },
    ));
    match state
        .send_runner_command_and_wait(
            runner_id,
            request_id.clone(),
            command,
            RUNNER_COMMAND_RESPONSE_TIMEOUT,
        )
        .await
    {
        Ok(agenter_protocol::runner::RunnerResponseOutcome::Ok { .. }) => {
            let summary = state
                .take_refresh_summary(&request_id)
                .await
                .unwrap_or_default();
            Json(summary).into_response()
        }
        Ok(agenter_protocol::runner::RunnerResponseOutcome::Error { error }) => {
            tracing::warn!(
                user_id = %user.user_id,
                %workspace_id,
                code = %error.code,
                message = %error.message,
                "session refresh failed in runner"
            );
            (StatusCode::BAD_GATEWAY, Json(error)).into_response()
        }
        Err(error) => runner_wait_error_status(error).into_response(),
    }
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
    let user_message = agenter_core::UserMessageEvent {
        session_id,
        message_id: Some(Uuid::new_v4().to_string()),
        author_user_id: Some(user.user_id),
        content: content.to_owned(),
    };
    state
        .publish_event(
            session_id,
            agenter_core::AppEvent::UserMessage(user_message.clone()),
        )
        .await;

    let message = agenter_protocol::runner::RunnerServerMessage::Command(Box::new(
        agenter_protocol::runner::RunnerCommandEnvelope {
            request_id: agenter_protocol::RequestId::from(Uuid::new_v4().to_string()),
            command: agenter_protocol::runner::RunnerCommand::AgentSendInput(
                agenter_protocol::runner::AgentInputCommand {
                    session_id,
                    external_session_id: session.external_session_id,
                    settings,
                    input: agenter_protocol::runner::AgentInput::UserMessage {
                        payload: user_message,
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
            request_id.clone(),
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
            publish_runner_error_event(
                &state,
                session_id,
                "send_session_message",
                Some(&request_id),
                &error.code,
                &error.message,
            )
            .await;
            (StatusCode::BAD_GATEWAY, Json(error)).into_response()
        }
        Ok(other) => {
            tracing::warn!(
                user_id = %user.user_id,
                %session_id,
                runner_id = %session.runner_id,
                ?other,
                "session message received unexpected runner response"
            );
            let detail = format!("unexpected runner response: {other:?}");
            publish_runner_error_event(
                &state,
                session_id,
                "send_session_message",
                Some(&request_id),
                "unexpected_runner_response",
                &detail,
            )
            .await;
            (
                StatusCode::BAD_GATEWAY,
                Json(agenter_protocol::runner::RunnerError {
                    code: "unexpected_runner_response".to_owned(),
                    message: detail,
                }),
            )
                .into_response()
        }
        Err(error) => {
            tracing::warn!(
                user_id = %user.user_id,
                %session_id,
                runner_id = %session.runner_id,
                ?error,
                "session message failed because runner is unavailable"
            );
            let status = runner_wait_error_status(error);
            let code = runner_wait_error_code(error);
            let message = runner_wait_error_message(error);
            publish_runner_error_event(
                &state,
                session_id,
                "send_session_message",
                Some(&request_id),
                code,
                message,
            )
            .await;
            (
                status,
                Json(agenter_protocol::runner::RunnerError {
                    code: code.to_owned(),
                    message: message.to_owned(),
                }),
            )
                .into_response()
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

async fn list_slash_commands(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<agenter_core::SessionId>,
) -> Response {
    let Some(user) = authenticated_user_from_headers(&state, &headers).await else {
        tracing::debug!(%session_id, "slash command list rejected missing or invalid session");
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let Some(session) = state.session(user.user_id, session_id).await else {
        tracing::warn!(user_id = %user.user_id, %session_id, "slash command list rejected session not found");
        return StatusCode::NOT_FOUND.into_response();
    };

    let mut commands = local_slash_commands(&session.provider_id);
    let request_id = agenter_protocol::RequestId::from(Uuid::new_v4().to_string());
    let message = agenter_protocol::runner::RunnerServerMessage::Command(Box::new(
        agenter_protocol::runner::RunnerCommandEnvelope {
            request_id: request_id.clone(),
            command: agenter_protocol::runner::RunnerCommand::ListProviderCommands(
                agenter_protocol::runner::ListProviderCommandsCommand {
                    session_id,
                    provider_id: session.provider_id.clone(),
                },
            ),
        },
    ));
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
            result:
                agenter_protocol::runner::RunnerCommandResult::ProviderCommands {
                    commands: provider_commands,
                },
        }) => commands.extend(provider_commands),
        Ok(other) => {
            tracing::warn!(%session_id, ?other, "runner returned unexpected slash command manifest response");
        }
        Err(error) => {
            tracing::debug!(%session_id, ?error, "provider slash command manifest unavailable");
        }
    }
    let mut seen = std::collections::HashSet::new();
    commands.retain(|command| seen.insert(command.id.clone()));

    Json(commands).into_response()
}

async fn execute_slash_command(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<agenter_core::SessionId>,
    Json(request): Json<agenter_core::SlashCommandRequest>,
) -> Response {
    let Some(user) = authenticated_user_from_headers(&state, &headers).await else {
        tracing::debug!(%session_id, "slash command rejected missing or invalid session");
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let Some(session) = state.session(user.user_id, session_id).await else {
        tracing::warn!(user_id = %user.user_id, %session_id, "slash command rejected session not found");
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(definition) = local_slash_commands(&session.provider_id)
        .into_iter()
        .find(|command| command.id == request.command_id)
    else {
        return (
            StatusCode::BAD_REQUEST,
            Json(agenter_core::SlashCommandResult {
                accepted: false,
                message: format!("Unknown slash command `{}`.", request.raw_input),
                session: None,
                provider_payload: None,
            }),
        )
            .into_response();
    };
    if matches!(
        definition.danger_level,
        agenter_core::SlashCommandDangerLevel::Confirm
            | agenter_core::SlashCommandDangerLevel::Dangerous
    ) && !request.confirmed
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(agenter_core::SlashCommandResult {
                accepted: false,
                message: format!("Command /{} requires confirmation.", definition.name),
                session: None,
                provider_payload: None,
            }),
        )
            .into_response();
    }
    publish_slash_user_echo(&state, user.user_id, session.session_id, &request).await;

    if request.command_id.starts_with("local.") {
        return execute_local_slash_command(state, user.user_id, session, request, definition)
            .await;
    }
    if request.command_id == "runner.interrupt" {
        return execute_runner_interrupt_slash_command(state, session, request, definition).await;
    }
    if !request
        .command_id
        .starts_with(&format!("{}.", session.provider_id.as_str()))
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(agenter_core::SlashCommandResult {
                accepted: false,
                message: format!("Unknown slash command `{}`.", request.raw_input),
                session: None,
                provider_payload: None,
            }),
        )
            .into_response();
    }

    execute_provider_slash_command(state, user.user_id, session, request, definition).await
}

async fn execute_local_slash_command(
    state: AppState,
    user_id: agenter_core::UserId,
    session: crate::state::RegisteredSession,
    request: agenter_core::SlashCommandRequest,
    definition: agenter_core::SlashCommandDefinition,
) -> Response {
    let session_id = session.session_id;
    match request.command_id.as_str() {
        "local.title" => {
            let title = request
                .arguments
                .get("title")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|title| !title.is_empty())
                .map(str::to_owned);
            let Some(session) = state
                .update_session_title(user_id, session.session_id, title)
                .await
            else {
                return StatusCode::NOT_FOUND.into_response();
            };
            let result = agenter_core::SlashCommandResult {
                accepted: true,
                message: "Session title updated.".to_owned(),
                session: Some(session),
                provider_payload: None,
            };
            slash_result_response(
                &state,
                session_id,
                &request,
                &definition,
                StatusCode::OK,
                result,
            )
            .await
        }
        "local.model" | "local.mode" | "local.reasoning" => {
            let mut settings = state
                .session_turn_settings(user_id, session.session_id)
                .await
                .unwrap_or_default();
            match request.command_id.as_str() {
                "local.model" => {
                    settings.model = request
                        .arguments
                        .get("model")
                        .and_then(serde_json::Value::as_str)
                        .filter(|value| !value.is_empty())
                        .map(str::to_owned);
                }
                "local.mode" => {
                    settings.collaboration_mode = request
                        .arguments
                        .get("mode")
                        .and_then(serde_json::Value::as_str)
                        .filter(|value| !value.is_empty())
                        .map(str::to_owned);
                }
                "local.reasoning" => {
                    settings.reasoning_effort = request
                        .arguments
                        .get("effort")
                        .and_then(serde_json::Value::as_str)
                        .and_then(agent_reasoning_effort_from_str);
                }
                _ => {}
            }
            let Some(settings) = state
                .update_session_turn_settings(user_id, session.session_id, settings)
                .await
            else {
                return StatusCode::NOT_FOUND.into_response();
            };
            let result = agenter_core::SlashCommandResult {
                accepted: true,
                message: "Session settings updated.".to_owned(),
                session: None,
                provider_payload: Some(serde_json::to_value(settings).unwrap_or_default()),
            };
            slash_result_response(
                &state,
                session.session_id,
                &request,
                &definition,
                StatusCode::OK,
                result,
            )
            .await
        }
        "local.help" => {
            let result = agenter_core::SlashCommandResult {
                accepted: true,
                message: "Type / to browse commands. Provider commands are marked with their provider and dangerous commands require confirmation.".to_owned(),
                session: None,
                provider_payload: None,
            };
            slash_result_response(
                &state,
                session.session_id,
                &request,
                &definition,
                StatusCode::OK,
                result,
            )
            .await
        }
        "local.new" => {
            let title = request
                .arguments
                .get("title")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|title| !title.is_empty())
                .map(str::to_owned);
            create_session_from_slash(state, user_id, session, request, definition, title).await
        }
        "local.refresh" => {
            refresh_workspace_provider_sessions_for_user(
                state,
                user_id,
                session.workspace.workspace_id,
                session.provider_id,
                Some((session.session_id, request, definition)),
            )
            .await
        }
        _ => (
            StatusCode::BAD_REQUEST,
            Json(agenter_core::SlashCommandResult {
                accepted: false,
                message: format!("Unknown slash command `{}`.", request.raw_input),
                session: None,
                provider_payload: None,
            }),
        )
            .into_response(),
    }
}

async fn create_session_from_slash(
    state: AppState,
    user_id: agenter_core::UserId,
    source: crate::state::RegisteredSession,
    request: agenter_core::SlashCommandRequest,
    definition: agenter_core::SlashCommandDefinition,
    title: Option<String>,
) -> Response {
    let source_session_id = source.session_id;
    let session_id = agenter_core::SessionId::new();
    let external_session_id = if source.provider_id.as_str() == agenter_core::AgentProviderId::CODEX
    {
        let request_id = agenter_protocol::RequestId::from(Uuid::new_v4().to_string());
        let command = agenter_protocol::runner::RunnerServerMessage::Command(Box::new(
            agenter_protocol::runner::RunnerCommandEnvelope {
                request_id: request_id.clone(),
                command: agenter_protocol::runner::RunnerCommand::CreateSession(
                    agenter_protocol::runner::CreateSessionCommand {
                        session_id,
                        workspace: source.workspace.clone(),
                        provider_id: source.provider_id.clone(),
                        initial_input: None,
                    },
                ),
            },
        ));
        match state
            .send_runner_command_and_wait(
                source.runner_id,
                request_id,
                command,
                RUNNER_COMMAND_RESPONSE_TIMEOUT,
            )
            .await
        {
            Ok(agenter_protocol::runner::RunnerResponseOutcome::Ok {
                result:
                    agenter_protocol::runner::RunnerCommandResult::SessionCreated {
                        session_id: returned_session_id,
                        external_session_id,
                    },
            }) if returned_session_id == session_id => Some(external_session_id),
            Ok(agenter_protocol::runner::RunnerResponseOutcome::Error { error }) => {
                let result = agenter_core::SlashCommandResult {
                    accepted: false,
                    message: error.message.clone(),
                    session: None,
                    provider_payload: Some(serde_json::json!({ "code": error.code })),
                };
                return slash_result_response(
                    &state,
                    source_session_id,
                    &request,
                    &definition,
                    StatusCode::BAD_GATEWAY,
                    result,
                )
                .await;
            }
            Ok(other) => {
                tracing::warn!(
                    ?other,
                    "unexpected runner create-session response for slash /new"
                );
                let result = agenter_core::SlashCommandResult {
                    accepted: false,
                    message: format!("Unexpected runner response: {other:?}"),
                    session: None,
                    provider_payload: None,
                };
                return slash_result_response(
                    &state,
                    source_session_id,
                    &request,
                    &definition,
                    StatusCode::BAD_GATEWAY,
                    result,
                )
                .await;
            }
            Err(error) => {
                let result = agenter_core::SlashCommandResult {
                    accepted: false,
                    message: runner_wait_error_message(error).to_owned(),
                    session: None,
                    provider_payload: Some(serde_json::json!({
                        "code": runner_wait_error_code(error),
                    })),
                };
                return slash_result_response(
                    &state,
                    source_session_id,
                    &request,
                    &definition,
                    runner_wait_error_status(error),
                    result,
                )
                .await;
            }
        }
    } else {
        None
    };

    let session = state
        .register_session(SessionRegistration {
            session_id,
            owner_user_id: user_id,
            runner_id: source.runner_id,
            workspace: source.workspace,
            provider_id: source.provider_id,
            title,
            external_session_id,
            turn_settings: source.turn_settings,
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
            info.session_id,
            agenter_core::AppEvent::SessionStarted(info.clone()),
        )
        .await;
    let result = agenter_core::SlashCommandResult {
        accepted: true,
        message: "New session created.".to_owned(),
        session: Some(info),
        provider_payload: None,
    };
    slash_result_response(
        &state,
        source_session_id,
        &request,
        &definition,
        StatusCode::OK,
        result,
    )
    .await
}

async fn execute_runner_interrupt_slash_command(
    state: AppState,
    session: crate::state::RegisteredSession,
    request: agenter_core::SlashCommandRequest,
    definition: agenter_core::SlashCommandDefinition,
) -> Response {
    let request_id = agenter_protocol::RequestId::from(Uuid::new_v4().to_string());
    let message = agenter_protocol::runner::RunnerServerMessage::Command(Box::new(
        agenter_protocol::runner::RunnerCommandEnvelope {
            request_id: request_id.clone(),
            command: agenter_protocol::runner::RunnerCommand::InterruptSession {
                session_id: session.session_id,
            },
        },
    ));
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
            let result = agenter_core::SlashCommandResult {
                accepted: true,
                message: "Interrupt requested.".to_owned(),
                session: None,
                provider_payload: None,
            };
            slash_result_response(
                &state,
                session.session_id,
                &request,
                &definition,
                StatusCode::OK,
                result,
            )
            .await
        }
        Ok(agenter_protocol::runner::RunnerResponseOutcome::Error { error }) => {
            let result = agenter_core::SlashCommandResult {
                accepted: false,
                message: error.message.clone(),
                session: None,
                provider_payload: Some(serde_json::json!({ "code": error.code })),
            };
            slash_result_response(
                &state,
                session.session_id,
                &request,
                &definition,
                StatusCode::BAD_GATEWAY,
                result,
            )
            .await
        }
        Ok(other) => {
            tracing::warn!(session_id = %session.session_id, ?other, "runner returned unexpected interrupt slash response");
            let result = agenter_core::SlashCommandResult {
                accepted: false,
                message: format!("Unexpected runner response: {other:?}"),
                session: None,
                provider_payload: None,
            };
            slash_result_response(
                &state,
                session.session_id,
                &request,
                &definition,
                StatusCode::BAD_GATEWAY,
                result,
            )
            .await
        }
        Err(error) => {
            let result = agenter_core::SlashCommandResult {
                accepted: false,
                message: runner_wait_error_message(error).to_owned(),
                session: None,
                provider_payload: Some(serde_json::json!({
                    "code": runner_wait_error_code(error),
                })),
            };
            slash_result_response(
                &state,
                session.session_id,
                &request,
                &definition,
                runner_wait_error_status(error),
                result,
            )
            .await
        }
    }
}

async fn execute_provider_slash_command(
    state: AppState,
    user_id: agenter_core::UserId,
    session: crate::state::RegisteredSession,
    request: agenter_core::SlashCommandRequest,
    definition: agenter_core::SlashCommandDefinition,
) -> Response {
    let provider_definition = provider_slash_commands(&session.provider_id)
        .into_iter()
        .find(|command| command.id == request.command_id);
    if provider_definition.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(agenter_core::SlashCommandResult {
                accepted: false,
                message: format!("Unknown slash command `{}`.", request.raw_input),
                session: None,
                provider_payload: None,
            }),
        )
            .into_response();
    }

    let request_id = agenter_protocol::RequestId::from(Uuid::new_v4().to_string());
    let command_id = request.command_id.clone();
    let command_request = request.clone();
    let message = agenter_protocol::runner::RunnerServerMessage::Command(Box::new(
        agenter_protocol::runner::RunnerCommandEnvelope {
            request_id: request_id.clone(),
            command: agenter_protocol::runner::RunnerCommand::ExecuteProviderCommand(
                agenter_protocol::runner::ProviderCommandExecutionCommand {
                    session_id: session.session_id,
                    external_session_id: session.external_session_id.clone(),
                    provider_id: session.provider_id.clone(),
                    command: command_request,
                },
            ),
        },
    ));
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
            result:
                agenter_protocol::runner::RunnerCommandResult::ProviderCommandExecuted { mut result },
        }) => {
            if command_id == "codex.fork" {
                if let Some(forked) = register_forked_session_from_provider_result(
                    &state,
                    user_id,
                    &session,
                    result.provider_payload.as_ref(),
                )
                .await
                {
                    result.session = Some(forked);
                }
            }
            if let Some(status) = provider_slash_session_status(&command_id) {
                if let Some(updated) = state
                    .update_session_status(user_id, session.session_id, status.clone())
                    .await
                {
                    state
                        .publish_event(
                            session.session_id,
                            agenter_core::AppEvent::SessionStatusChanged(
                                agenter_core::SessionStatusChangedEvent {
                                    session_id: session.session_id,
                                    status,
                                    reason: Some(format!(
                                        "Updated by /{}.",
                                        command_id
                                            .strip_prefix("codex.")
                                            .unwrap_or(command_id.as_str())
                                    )),
                                },
                            ),
                        )
                        .await;
                    result.session = Some(updated);
                }
            }
            slash_result_response(
                &state,
                session.session_id,
                &request,
                &definition,
                StatusCode::OK,
                result,
            )
            .await
        }
        Ok(agenter_protocol::runner::RunnerResponseOutcome::Error { error }) => {
            let result = agenter_core::SlashCommandResult {
                accepted: false,
                message: error.message.clone(),
                session: None,
                provider_payload: Some(serde_json::json!({ "code": error.code })),
            };
            slash_result_response(
                &state,
                session.session_id,
                &request,
                &definition,
                StatusCode::BAD_GATEWAY,
                result,
            )
            .await
        }
        Ok(other) => {
            tracing::warn!(?other, "runner returned unexpected provider slash response");
            let result = agenter_core::SlashCommandResult {
                accepted: false,
                message: format!("Unexpected runner response: {other:?}"),
                session: None,
                provider_payload: None,
            };
            slash_result_response(
                &state,
                session.session_id,
                &request,
                &definition,
                StatusCode::BAD_GATEWAY,
                result,
            )
            .await
        }
        Err(error) => {
            let result = agenter_core::SlashCommandResult {
                accepted: false,
                message: runner_wait_error_message(error).to_owned(),
                session: None,
                provider_payload: Some(serde_json::json!({
                    "code": runner_wait_error_code(error),
                })),
            };
            slash_result_response(
                &state,
                session.session_id,
                &request,
                &definition,
                runner_wait_error_status(error),
                result,
            )
            .await
        }
    }
}

async fn publish_slash_user_echo(
    state: &AppState,
    user_id: agenter_core::UserId,
    session_id: agenter_core::SessionId,
    request: &agenter_core::SlashCommandRequest,
) {
    state
        .publish_event(
            session_id,
            agenter_core::AppEvent::UserMessage(agenter_core::UserMessageEvent {
                session_id,
                message_id: Some(Uuid::new_v4().to_string()),
                author_user_id: Some(user_id),
                content: request.raw_input.clone(),
            }),
        )
        .await;
}

async fn slash_result_response(
    state: &AppState,
    session_id: agenter_core::SessionId,
    request: &agenter_core::SlashCommandRequest,
    definition: &agenter_core::SlashCommandDefinition,
    status: StatusCode,
    result: agenter_core::SlashCommandResult,
) -> Response {
    publish_slash_result_event(state, session_id, request, definition, &result).await;
    (status, Json(result)).into_response()
}

async fn publish_slash_result_event(
    state: &AppState,
    session_id: agenter_core::SessionId,
    request: &agenter_core::SlashCommandRequest,
    definition: &agenter_core::SlashCommandDefinition,
    result: &agenter_core::SlashCommandResult,
) {
    state
        .publish_event(
            session_id,
            agenter_core::AppEvent::ProviderEvent(agenter_core::ProviderEvent {
                session_id,
                provider_id: definition
                    .provider_id
                    .clone()
                    .unwrap_or_else(|| agenter_core::AgentProviderId::from("local")),
                event_id: Some(format!(
                    "slash-{}-{}",
                    request.command_id.replace('.', "-"),
                    Uuid::new_v4()
                )),
                category: "slash_command".to_owned(),
                title: format!("/{}", definition.name),
                detail: Some(result.message.clone()),
                status: Some(
                    if result.accepted {
                        "accepted"
                    } else {
                        "rejected"
                    }
                    .to_owned(),
                ),
                provider_payload: Some(serde_json::json!({
                    "command_id": request.command_id,
                    "raw_input": request.raw_input,
                    "target": definition.target,
                    "danger_level": definition.danger_level,
                    "arguments": request.arguments,
                    "accepted": result.accepted,
                    "message": result.message,
                    "provider_payload": result.provider_payload,
                })),
            }),
        )
        .await;
}

fn provider_slash_session_status(command_id: &str) -> Option<agenter_core::SessionStatus> {
    match command_id {
        "codex.archive" => Some(agenter_core::SessionStatus::Archived),
        "codex.unarchive" => Some(agenter_core::SessionStatus::Running),
        _ => None,
    }
}

async fn register_forked_session_from_provider_result(
    state: &AppState,
    user_id: agenter_core::UserId,
    source: &crate::state::RegisteredSession,
    provider_payload: Option<&serde_json::Value>,
) -> Option<agenter_core::SessionInfo> {
    let external_session_id = provider_payload
        .and_then(|payload| {
            string_pointer(
                payload,
                &[
                    "/result/thread/id",
                    "/result/threadId",
                    "/thread/id",
                    "/threadId",
                ],
            )
        })?
        .to_owned();
    let session = state
        .register_session(SessionRegistration {
            session_id: agenter_core::SessionId::new(),
            owner_user_id: user_id,
            runner_id: source.runner_id,
            workspace: source.workspace.clone(),
            provider_id: source.provider_id.clone(),
            title: Some(format!(
                "Fork of {}",
                source.title.as_deref().unwrap_or("session")
            )),
            external_session_id: Some(external_session_id),
            turn_settings: source.turn_settings.clone(),
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
            info.session_id,
            agenter_core::AppEvent::SessionStarted(info.clone()),
        )
        .await;
    Some(info)
}

async fn refresh_workspace_provider_sessions_for_user(
    state: AppState,
    user_id: agenter_core::UserId,
    workspace_id: agenter_core::WorkspaceId,
    provider_id: agenter_core::AgentProviderId,
    slash_context: Option<(
        agenter_core::SessionId,
        agenter_core::SlashCommandRequest,
        agenter_core::SlashCommandDefinition,
    )>,
) -> Response {
    let Some((runner_id, workspace)) = state
        .resolve_runner_workspace(workspace_id, &provider_id)
        .await
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let request_id = agenter_protocol::RequestId::from(Uuid::new_v4().to_string());
    let command = agenter_protocol::runner::RunnerServerMessage::Command(Box::new(
        agenter_protocol::runner::RunnerCommandEnvelope {
            request_id: request_id.clone(),
            command: agenter_protocol::runner::RunnerCommand::RefreshSessions(
                agenter_protocol::runner::RefreshSessionsCommand {
                    workspace,
                    provider_id: provider_id.clone(),
                },
            ),
        },
    ));
    match state
        .send_runner_command_and_wait(
            runner_id,
            request_id.clone(),
            command,
            RUNNER_COMMAND_RESPONSE_TIMEOUT,
        )
        .await
    {
        Ok(agenter_protocol::runner::RunnerResponseOutcome::Ok { .. }) => {
            let summary = state
                .take_refresh_summary(&request_id)
                .await
                .unwrap_or_default();
            let result = agenter_core::SlashCommandResult {
                accepted: true,
                message: format!(
                    "Refresh complete: {} discovered, {} cache refreshed, {} skipped.",
                    summary.discovered_count,
                    summary.refreshed_cache_count,
                    summary.skipped_failed_count
                ),
                session: None,
                provider_payload: Some(serde_json::to_value(summary).unwrap_or_default()),
            };
            if let Some((session_id, request, definition)) = slash_context {
                slash_result_response(
                    &state,
                    session_id,
                    &request,
                    &definition,
                    StatusCode::OK,
                    result,
                )
                .await
            } else {
                Json(result).into_response()
            }
        }
        Ok(agenter_protocol::runner::RunnerResponseOutcome::Error { error }) => {
            tracing::warn!(%user_id, %workspace_id, code = %error.code, message = %error.message, "slash refresh failed in runner");
            let result = agenter_core::SlashCommandResult {
                accepted: false,
                message: error.message.clone(),
                session: None,
                provider_payload: Some(serde_json::json!({ "code": error.code })),
            };
            if let Some((session_id, request, definition)) = slash_context {
                slash_result_response(
                    &state,
                    session_id,
                    &request,
                    &definition,
                    StatusCode::BAD_GATEWAY,
                    result,
                )
                .await
            } else {
                (StatusCode::BAD_GATEWAY, Json(error)).into_response()
            }
        }
        Err(error) => {
            let result = agenter_core::SlashCommandResult {
                accepted: false,
                message: runner_wait_error_message(error).to_owned(),
                session: None,
                provider_payload: Some(serde_json::json!({
                    "code": runner_wait_error_code(error),
                })),
            };
            if let Some((session_id, request, definition)) = slash_context {
                slash_result_response(
                    &state,
                    session_id,
                    &request,
                    &definition,
                    runner_wait_error_status(error),
                    result,
                )
                .await
            } else {
                runner_wait_error_status(error).into_response()
            }
        }
    }
}

fn local_slash_commands(
    provider_id: &agenter_core::AgentProviderId,
) -> Vec<agenter_core::SlashCommandDefinition> {
    let mut commands: Vec<agenter_core::SlashCommandDefinition> = vec![
        slash_command(
            "local.help",
            "help",
            "Show available slash commands.",
            "local",
            agenter_core::SlashCommandTarget::Local,
        ),
        slash_command(
            "local.model",
            "model",
            "Set the session model.",
            "settings",
            agenter_core::SlashCommandTarget::Local,
        )
        .with_argument(
            "model",
            agenter_core::SlashCommandArgumentKind::String,
            true,
            "Model id",
        ),
        slash_command(
            "local.mode",
            "mode",
            "Set collaboration mode.",
            "settings",
            agenter_core::SlashCommandTarget::Local,
        )
        .with_argument(
            "mode",
            agenter_core::SlashCommandArgumentKind::String,
            true,
            "Mode id",
        ),
        slash_command(
            "local.reasoning",
            "reasoning",
            "Set reasoning effort.",
            "settings",
            agenter_core::SlashCommandTarget::Local,
        )
        .with_argument(
            "effort",
            agenter_core::SlashCommandArgumentKind::Enum,
            true,
            "Reasoning effort",
        )
        .with_choices(["none", "minimal", "low", "medium", "high", "xhigh"]),
        slash_command(
            "local.title",
            "title",
            "Rename this session.",
            "session",
            agenter_core::SlashCommandTarget::Local,
        )
        .with_argument(
            "title",
            agenter_core::SlashCommandArgumentKind::Rest,
            true,
            "New title",
        ),
        slash_command(
            "local.refresh",
            "refresh",
            "Refresh provider sessions from persistence.",
            "session",
            agenter_core::SlashCommandTarget::Local,
        ),
        slash_command(
            "local.new",
            "new",
            "Create a new session in this workspace.",
            "session",
            agenter_core::SlashCommandTarget::Local,
        )
        .with_argument(
            "title",
            agenter_core::SlashCommandArgumentKind::Rest,
            false,
            "Optional title",
        ),
        slash_command(
            "runner.interrupt",
            "interrupt",
            "Interrupt the current provider turn.",
            "runner",
            agenter_core::SlashCommandTarget::Runner,
        ),
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    commands.extend(provider_slash_commands(provider_id));
    commands
}

fn provider_slash_commands(
    provider_id: &agenter_core::AgentProviderId,
) -> Vec<agenter_core::SlashCommandDefinition> {
    if provider_id.as_str() != agenter_core::AgentProviderId::CODEX {
        return Vec::new();
    }
    vec![
        slash_command(
            "codex.compact",
            "compact",
            "Start native Codex context compaction.",
            "provider",
            agenter_core::SlashCommandTarget::Provider,
        )
        .into(),
        slash_command(
            "codex.review",
            "review",
            "Start a native Codex code review.",
            "provider",
            agenter_core::SlashCommandTarget::Provider,
        )
        .with_argument(
            "target",
            agenter_core::SlashCommandArgumentKind::Rest,
            false,
            "Review target flags",
        )
        .into(),
        slash_command(
            "codex.steer",
            "steer",
            "Steer the active Codex turn.",
            "provider",
            agenter_core::SlashCommandTarget::Provider,
        )
        .with_argument(
            "input",
            agenter_core::SlashCommandArgumentKind::Rest,
            true,
            "Text to steer with",
        )
        .into(),
        slash_command(
            "codex.fork",
            "fork",
            "Fork the current Codex thread.",
            "provider",
            agenter_core::SlashCommandTarget::Provider,
        )
        .into(),
        slash_command(
            "codex.archive",
            "archive",
            "Archive the current Codex thread.",
            "provider",
            agenter_core::SlashCommandTarget::Provider,
        )
        .into(),
        slash_command(
            "codex.unarchive",
            "unarchive",
            "Unarchive the current Codex thread.",
            "provider",
            agenter_core::SlashCommandTarget::Provider,
        )
        .into(),
        slash_command(
            "codex.rollback",
            "rollback",
            "Drop recent turns from Codex history. Does not revert files.",
            "provider",
            agenter_core::SlashCommandTarget::Provider,
        )
        .danger(agenter_core::SlashCommandDangerLevel::Dangerous)
        .with_argument(
            "numTurns",
            agenter_core::SlashCommandArgumentKind::Number,
            true,
            "Number of turns",
        )
        .into(),
        slash_command(
            "codex.shell",
            "shell",
            "Run an unsandboxed provider-native shell command.",
            "provider",
            agenter_core::SlashCommandTarget::Provider,
        )
        .danger(agenter_core::SlashCommandDangerLevel::Dangerous)
        .with_alias("sh")
        .with_argument(
            "command",
            agenter_core::SlashCommandArgumentKind::Rest,
            true,
            "Shell command",
        )
        .into(),
    ]
}

struct SlashCommandBuilder(agenter_core::SlashCommandDefinition);

impl SlashCommandBuilder {
    fn with_argument(
        mut self,
        name: &str,
        kind: agenter_core::SlashCommandArgumentKind,
        required: bool,
        description: &str,
    ) -> Self {
        self.0.arguments.push(agenter_core::SlashCommandArgument {
            name: name.to_owned(),
            kind,
            required,
            description: Some(description.to_owned()),
            choices: Vec::new(),
        });
        self
    }

    fn with_choices<const N: usize>(mut self, choices: [&str; N]) -> Self {
        if let Some(argument) = self.0.arguments.last_mut() {
            argument.choices = choices.into_iter().map(str::to_owned).collect();
        }
        self
    }

    fn with_alias(mut self, alias: &str) -> Self {
        self.0.aliases.push(alias.to_owned());
        self
    }

    fn danger(mut self, danger_level: agenter_core::SlashCommandDangerLevel) -> Self {
        self.0.danger_level = danger_level;
        self
    }
}

impl From<SlashCommandBuilder> for agenter_core::SlashCommandDefinition {
    fn from(builder: SlashCommandBuilder) -> Self {
        builder.0
    }
}

fn slash_command(
    id: &str,
    name: &str,
    description: &str,
    category: &str,
    target: agenter_core::SlashCommandTarget,
) -> SlashCommandBuilder {
    SlashCommandBuilder(agenter_core::SlashCommandDefinition {
        id: id.to_owned(),
        name: name.to_owned(),
        aliases: Vec::new(),
        description: description.to_owned(),
        category: category.to_owned(),
        provider_id: id.split_once('.').and_then(|(prefix, _)| {
            (!matches!(prefix, "local" | "runner"))
                .then(|| agenter_core::AgentProviderId::from(prefix))
        }),
        target,
        danger_level: agenter_core::SlashCommandDangerLevel::Safe,
        arguments: Vec::new(),
        examples: Vec::new(),
    })
}

fn agent_reasoning_effort_from_str(value: &str) -> Option<agenter_core::AgentReasoningEffort> {
    match value {
        "none" => Some(agenter_core::AgentReasoningEffort::None),
        "minimal" => Some(agenter_core::AgentReasoningEffort::Minimal),
        "low" => Some(agenter_core::AgentReasoningEffort::Low),
        "medium" => Some(agenter_core::AgentReasoningEffort::Medium),
        "high" => Some(agenter_core::AgentReasoningEffort::High),
        "xhigh" => Some(agenter_core::AgentReasoningEffort::Xhigh),
        _ => None,
    }
}

fn string_pointer<'a>(value: &'a serde_json::Value, pointers: &[&str]) -> Option<&'a str> {
    pointers
        .iter()
        .find_map(|pointer| value.pointer(pointer).and_then(serde_json::Value::as_str))
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

    #[test]
    fn slash_command_registry_is_provider_heavy_but_marks_dangerous_commands() {
        let commands = local_slash_commands(&AgentProviderId::from(AgentProviderId::CODEX));

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

        let qwen_commands = local_slash_commands(&AgentProviderId::from(AgentProviderId::QWEN));
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

        let cookie = format!("{}={browser_session_token}", auth::SESSION_COOKIE_NAME);
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

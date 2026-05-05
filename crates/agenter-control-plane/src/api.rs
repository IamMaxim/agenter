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
    use super::*;

    #[test]
    fn api_router_builds() {
        let _ = app(crate::state::AppState::new(
            "runner-token".to_owned(),
            crate::auth::CookieSecurity::DevelopmentInsecure,
        ));
    }
}

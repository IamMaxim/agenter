use agenter_core::RunnerId;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub(super) struct RunnerInfoResponse {
    pub(super) runner_id: agenter_core::RunnerId,
    pub(super) name: String,
    pub(super) status: &'static str,
    pub(super) last_seen_at: Option<String>,
}

pub(super) fn router() -> Router<crate::state::AppState> {
    Router::new()
        .route("/", get(list_runners))
        .route("/{runner_id}/workspaces", get(list_runner_workspaces))
}

pub(super) async fn list_runners(
    state: State<crate::state::AppState>,
    headers: HeaderMap,
) -> Response {
    let state = state.0;
    if super::authenticated_user_from_headers(&state, &headers)
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
    axum::Json(runners).into_response()
}

pub(super) async fn list_runner_workspaces(
    state: State<crate::state::AppState>,
    headers: HeaderMap,
    Path(runner_id): Path<RunnerId>,
) -> Response {
    let state = state.0;
    if super::authenticated_user_from_headers(&state, &headers)
        .await
        .is_none()
    {
        tracing::debug!(%runner_id, "list runner workspaces rejected missing or invalid session");
        return StatusCode::UNAUTHORIZED.into_response();
    }

    match state.list_runner_workspaces(runner_id).await {
        Some(workspaces) => {
            tracing::debug!(%runner_id, workspace_count = workspaces.len(), "listed runner workspaces");
            axum::Json(workspaces).into_response()
        }
        None => {
            tracing::warn!(%runner_id, "runner workspaces not found");
            StatusCode::NOT_FOUND.into_response()
        }
    }
}

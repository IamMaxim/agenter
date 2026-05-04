use agenter_core::{AgentProviderId, WorkspaceId};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub(super) struct ListApprovalRulesQuery {
    pub workspace_id: WorkspaceId,
    pub provider_id: AgentProviderId,
}

#[derive(Debug, Serialize)]
struct ApprovalRuleResponse {
    rule_id: Uuid,
    workspace_id: WorkspaceId,
    provider_id: AgentProviderId,
    kind: agenter_core::ApprovalKind,
    label: String,
    matcher: serde_json::Value,
    decision: agenter_core::ApprovalDecision,
    disabled_at: Option<chrono::DateTime<chrono::Utc>>,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

pub(super) fn router() -> Router<crate::state::AppState> {
    Router::new()
        .route("/", get(list_approval_rules))
        .route("/{rule_id}/disable", post(disable_approval_rule))
}

async fn list_approval_rules(
    state: State<crate::state::AppState>,
    headers: HeaderMap,
    Query(query): Query<ListApprovalRulesQuery>,
) -> Response {
    let state = state.0;
    let Some(user) = super::authenticated_user_from_headers(&state, &headers).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let Some(pool) = state.db_pool() else {
        return Json(Vec::<ApprovalRuleResponse>::new()).into_response();
    };
    let rules = match agenter_db::list_active_approval_policy_rules(
        pool,
        user.user_id,
        query.workspace_id,
        &query.provider_id,
    )
    .await
    {
        Ok(rules) => rules,
        Err(error) => {
            tracing::warn!(%error, "failed to list approval policy rules");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    Json(
        rules
            .into_iter()
            .map(ApprovalRuleResponse::from)
            .collect::<Vec<_>>(),
    )
    .into_response()
}

async fn disable_approval_rule(
    state: State<crate::state::AppState>,
    headers: HeaderMap,
    Path(rule_id): Path<Uuid>,
) -> Response {
    let state = state.0;
    let Some(user) = super::authenticated_user_from_headers(&state, &headers).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let Some(pool) = state.db_pool() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match agenter_db::disable_approval_policy_rule(pool, user.user_id, rule_id, user.user_id).await
    {
        Ok(Some(rule)) => Json(ApprovalRuleResponse::from(rule)).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => {
            tracing::warn!(%error, %rule_id, "failed to disable approval policy rule");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

impl From<agenter_db::models::ApprovalPolicyRule> for ApprovalRuleResponse {
    fn from(rule: agenter_db::models::ApprovalPolicyRule) -> Self {
        Self {
            rule_id: rule.rule_id,
            workspace_id: rule.workspace_id,
            provider_id: rule.provider_id,
            kind: rule.kind,
            label: rule.label,
            matcher: rule.matcher,
            decision: rule.decision,
            disabled_at: rule.disabled_at,
            created_at: rule.created_at,
            updated_at: rule.updated_at,
        }
    }
}

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{ApprovalId, SessionId, UserId};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum ApprovalDecision {
    Accept,
    AcceptForSession,
    Decline,
    Cancel,
    ProviderSpecific { payload: serde_json::Value },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalKind {
    Command,
    FileChange,
    Tool,
    ProviderSpecific,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ApprovalRequestEvent {
    pub session_id: SessionId,
    pub approval_id: ApprovalId,
    pub kind: ApprovalKind,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_payload: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ApprovalResolvedEvent {
    pub session_id: SessionId,
    pub approval_id: ApprovalId,
    pub decision: ApprovalDecision,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_by_user_id: Option<UserId>,
    pub resolved_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_payload: Option<serde_json::Value>,
}

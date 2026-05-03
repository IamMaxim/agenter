use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{ApprovalId, ItemId, NativeRef, SessionId, TurnId, UserId};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum ApprovalDecision {
    Accept,
    AcceptForSession,
    Decline,
    Cancel,
    ProviderSpecific { payload: serde_json::Value },
}

impl ApprovalDecision {
    #[must_use]
    pub fn canonical_option_id(&self) -> &'static str {
        match self {
            Self::Accept => "approve_once",
            Self::AcceptForSession => "approve_always",
            Self::Decline => "deny",
            Self::Cancel => "cancel_turn",
            Self::ProviderSpecific { .. } => "provider_specific",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalKind {
    Command,
    FileChange,
    Tool,
    ProviderSpecific,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalResolutionState {
    Pending,
    Resolving,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    Pending,
    Presented,
    Resolving,
    Approved,
    Denied,
    Cancelled,
    Expired,
    Orphaned,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalRisk {
    Low,
    Medium,
    High,
    Unknown,
}

impl ApprovalRisk {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyAction {
    Allow,
    Ask,
    Deny,
    Rewrite,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ApprovalPolicyMetadata {
    pub action: PolicyAction,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rewritten_request: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ApprovalRequest {
    pub approval_id: ApprovalId,
    pub session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_id: Option<ItemId>,
    pub kind: ApprovalKind,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<ApprovalOption>,
    pub status: ApprovalStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub risk: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_request_id: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub native_blocking: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy: Option<ApprovalPolicyMetadata>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native: Option<NativeRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ApprovalOption {
    pub option_id: String,
    pub kind: ApprovalOptionKind,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_option_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalOptionKind {
    ApproveOnce,
    ApproveAlways,
    Deny,
    DenyWithFeedback,
    CancelTurn,
    ProviderSpecific,
}

impl ApprovalOption {
    #[must_use]
    pub fn approve_once() -> Self {
        Self {
            option_id: "approve_once".to_owned(),
            kind: ApprovalOptionKind::ApproveOnce,
            label: "Approve once".to_owned(),
            description: None,
            scope: Some("turn".to_owned()),
            native_option_id: Some("accept".to_owned()),
        }
    }

    #[must_use]
    pub fn approve_always() -> Self {
        Self {
            option_id: "approve_always".to_owned(),
            kind: ApprovalOptionKind::ApproveAlways,
            label: "Approve always".to_owned(),
            description: None,
            scope: Some("session".to_owned()),
            native_option_id: Some("accept_for_session".to_owned()),
        }
    }

    #[must_use]
    pub fn deny() -> Self {
        Self {
            option_id: "deny".to_owned(),
            kind: ApprovalOptionKind::Deny,
            label: "Deny".to_owned(),
            description: None,
            scope: None,
            native_option_id: Some("decline".to_owned()),
        }
    }

    #[must_use]
    pub fn deny_with_feedback() -> Self {
        Self {
            option_id: "deny_with_feedback".to_owned(),
            kind: ApprovalOptionKind::DenyWithFeedback,
            label: "Deny with feedback".to_owned(),
            description: None,
            scope: None,
            native_option_id: Some("decline".to_owned()),
        }
    }

    #[must_use]
    pub fn cancel_turn() -> Self {
        Self {
            option_id: "cancel_turn".to_owned(),
            kind: ApprovalOptionKind::CancelTurn,
            label: "Cancel turn".to_owned(),
            description: None,
            scope: Some("turn".to_owned()),
            native_option_id: Some("cancel".to_owned()),
        }
    }

    #[must_use]
    pub fn canonical_defaults() -> Vec<Self> {
        vec![
            Self::approve_once(),
            Self::approve_always(),
            Self::deny(),
            Self::deny_with_feedback(),
            Self::cancel_turn(),
        ]
    }
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
    /// Provider-neutral UI hints (Codex-correlated today). Browsers render rich approval cards from this shape.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presentation: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution_state: Option<ApprovalResolutionState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolving_decision: Option<ApprovalDecision>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<ApprovalStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_id: Option<ItemId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<ApprovalOption>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub risk: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_request_id: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub native_blocking: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy: Option<ApprovalPolicyMetadata>,
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

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{ApprovalId, ItemId, NativeRef, QuestionId, SessionId, TurnId};

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
    Detached,
}

impl ApprovalStatus {
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Approved
                | Self::Denied
                | Self::Cancelled
                | Self::Expired
                | Self::Orphaned
                | Self::Detached
        )
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentObligationKind {
    Approval,
    Question,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentObligationStatus {
    Pending,
    Presented,
    Resolving,
    DeliveredToRunner,
    AcceptedByNative,
    Resolved,
    Orphaned,
    Expired,
    Detached,
}

impl AgentObligationStatus {
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Resolved | Self::Orphaned | Self::Expired | Self::Detached
        )
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AgentObligation {
    pub obligation_id: String,
    pub session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    pub kind: AgentObligationKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_id: Option<ApprovalId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub question_id: Option<QuestionId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_request_id: Option<String>,
    pub status: AgentObligationStatus,
    #[serde(default)]
    pub delivery_generation: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution_command_id: Option<crate::CommandId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<DateTime<Utc>>,
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
pub struct ApprovalPolicyRulePreview {
    pub kind: ApprovalKind,
    pub matcher: serde_json::Value,
    pub decision: ApprovalDecision,
    pub label: String,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolving_decision: Option<ApprovalDecision>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_rule: Option<ApprovalPolicyRulePreview>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalOptionKind {
    ApproveOnce,
    ApproveAlways,
    PersistApprovalRule,
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
            policy_rule: None,
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
            policy_rule: None,
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
            policy_rule: None,
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
            policy_rule: None,
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
            policy_rule: None,
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

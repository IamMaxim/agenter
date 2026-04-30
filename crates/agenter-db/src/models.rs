use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use agenter_core::{
    AgentProviderId, ApprovalDecision, ApprovalId, ApprovalKind, RunnerId, SessionId,
    SessionStatus, UserId, WorkspaceId,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct User {
    pub user_id: UserId,
    pub email: String,
    pub display_name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Runner {
    pub runner_id: RunnerId,
    pub name: String,
    pub version: Option<String>,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Workspace {
    pub workspace_id: WorkspaceId,
    pub runner_id: RunnerId,
    pub path: String,
    pub display_name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentSession {
    pub session_id: SessionId,
    pub owner_user_id: UserId,
    pub runner_id: RunnerId,
    pub workspace_id: WorkspaceId,
    pub provider_id: AgentProviderId,
    pub external_session_id: Option<String>,
    pub status: SessionStatus,
    pub title: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentSessionWithWorkspace {
    pub session: AgentSession,
    pub workspace: Workspace,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CachedEvent {
    pub event_id: uuid::Uuid,
    pub session_id: SessionId,
    pub event_index: i64,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PendingApproval {
    pub approval_id: ApprovalId,
    pub session_id: SessionId,
    pub kind: ApprovalKind,
    pub title: String,
    pub details: Option<String>,
    pub provider_payload: Option<serde_json::Value>,
    pub expires_at: Option<DateTime<Utc>>,
    pub resolved_decision: Option<ApprovalDecision>,
    pub resolved_by_user_id: Option<UserId>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OidcProvider {
    pub provider_id: String,
    pub display_name: String,
    pub issuer_url: String,
    pub client_id: String,
    pub client_secret_ciphertext: Option<String>,
    pub scopes: Vec<String>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OidcLoginState {
    pub state: String,
    pub provider_id: String,
    pub nonce: String,
    pub pkce_verifier: Option<String>,
    pub return_to: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub consumed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConnectorAccount {
    pub connector_account_id: uuid::Uuid,
    pub user_id: UserId,
    pub connector_id: String,
    pub external_account_id: String,
    pub display_name: Option<String>,
    pub linked_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConnectorLinkCode {
    pub code: String,
    pub user_id: Option<UserId>,
    pub connector_id: String,
    pub external_account_id: String,
    pub display_name: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub consumed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

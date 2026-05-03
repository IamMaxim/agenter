use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use agenter_core::{
    AgentProviderId, AgentTurnSettings, ApprovalDecision, ApprovalId, ApprovalKind, ApprovalOption,
    ApprovalStatus as UniversalApprovalStatus, CommandId, ItemId, NativeRef, RunnerId, SessionId,
    SessionSnapshot, SessionStatus, SessionUsageSnapshot, TurnId, UniversalEventKind,
    UniversalEventSource, UniversalSeq, UserId, WorkspaceId,
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
    pub usage_snapshot: Option<SessionUsageSnapshot>,
    pub turn_settings: Option<AgentTurnSettings>,
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
pub struct AgentEvent {
    pub seq: UniversalSeq,
    pub event_id: uuid::Uuid,
    pub workspace_id: WorkspaceId,
    pub session_id: SessionId,
    pub turn_id: Option<TurnId>,
    pub item_id: Option<ItemId>,
    pub event_type: String,
    pub event: UniversalEventKind,
    pub native: Option<NativeRef>,
    pub source: UniversalEventSource,
    pub command_id: Option<CommandId>,
    pub created_at: DateTime<Utc>,
}

impl AgentEvent {
    #[must_use]
    pub fn envelope(&self) -> agenter_core::UniversalEventEnvelope {
        agenter_core::UniversalEventEnvelope {
            event_id: self.event_id.to_string(),
            seq: self.seq,
            session_id: self.session_id,
            turn_id: self.turn_id,
            item_id: self.item_id,
            ts: self.created_at,
            source: self.source.clone(),
            native: self.native.clone(),
            event: self.event.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoredSessionSnapshot {
    pub session_id: SessionId,
    pub latest_seq: UniversalSeq,
    pub snapshot: SessionSnapshot,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UniversalAppendOutcome {
    pub event: AgentEvent,
    pub snapshot: StoredSessionSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandIdempotencyStatus {
    Pending,
    Succeeded,
    Failed,
    Conflict,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommandIdempotencyRecord {
    pub idempotency_key: String,
    pub command_id: CommandId,
    pub session_id: Option<SessionId>,
    pub status: CommandIdempotencyStatus,
    pub response_json: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PendingApproval {
    pub approval_id: ApprovalId,
    pub session_id: SessionId,
    pub kind: ApprovalKind,
    pub title: String,
    pub details: Option<String>,
    pub provider_payload: Option<serde_json::Value>,
    pub universal_status: UniversalApprovalStatus,
    pub native_request_id: Option<String>,
    pub canonical_options: Vec<ApprovalOption>,
    pub risk: Option<String>,
    pub subject: Option<String>,
    pub native_summary: Option<String>,
    pub native: Option<NativeRef>,
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
pub struct BrowserAuthSession {
    pub session_token_hash: String,
    pub user_id: UserId,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
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

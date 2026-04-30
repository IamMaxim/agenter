use serde::{Deserialize, Serialize};

use crate::{RunnerId, SessionId, UserId, WorkspaceId};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct AgentProviderId(String);

impl AgentProviderId {
    pub const CODEX: &'static str = "codex";
    pub const QWEN: &'static str = "qwen";

    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for AgentProviderId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for AgentProviderId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl std::fmt::Display for AgentProviderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Starting,
    Running,
    WaitingForInput,
    WaitingForApproval,
    Completed,
    Interrupted,
    Degraded,
    Failed,
    Archived,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentCapabilities {
    pub streaming: bool,
    pub session_resume: bool,
    pub session_history: bool,
    pub approvals: bool,
    pub file_changes: bool,
    pub command_execution: bool,
    pub plan_updates: bool,
    pub interrupt: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SessionInfo {
    pub session_id: SessionId,
    pub owner_user_id: UserId,
    pub runner_id: RunnerId,
    pub workspace_id: WorkspaceId,
    pub provider_id: AgentProviderId,
    pub status: SessionStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

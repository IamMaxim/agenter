use serde::{Deserialize, Serialize};

use chrono::{DateTime, Utc};

use crate::QuestionId;

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
    #[serde(default)]
    pub model_selection: bool,
    #[serde(default)]
    pub reasoning_effort: bool,
    #[serde(default)]
    pub collaboration_modes: bool,
    #[serde(default)]
    pub tool_user_input: bool,
    #[serde(default)]
    pub mcp_elicitation: bool,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Box<SessionUsageSnapshot>>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct SessionUsageContext {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_percent: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct SessionUsageWindow {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_percent: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining_percent: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining_text_hint: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct SessionUsageSnapshot {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<AgentReasoningEffort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<SessionUsageContext>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_5h: Option<SessionUsageWindow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub week: Option<SessionUsageWindow>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentReasoningEffort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentTurnSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<AgentReasoningEffort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collaboration_mode: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentOptions {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<AgentModelOption>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub collaboration_modes: Vec<AgentCollaborationMode>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentModelOption {
    pub id: String,
    pub display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub is_default: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_reasoning_effort: Option<AgentReasoningEffort>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supported_reasoning_efforts: Vec<AgentReasoningEffort>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input_modalities: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentCollaborationMode {
    pub id: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<AgentReasoningEffort>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct QuestionRequestedEvent {
    pub session_id: crate::SessionId,
    pub question_id: QuestionId,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<AgentQuestionField>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_payload: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct QuestionAnsweredEvent {
    pub session_id: crate::SessionId,
    pub question_id: QuestionId,
    pub answer: AgentQuestionAnswer,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_payload: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentQuestionField {
    pub id: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    pub kind: String,
    pub required: bool,
    pub secret: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<AgentQuestionChoice>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub default_answers: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentQuestionChoice {
    pub value: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentQuestionAnswer {
    pub question_id: QuestionId,
    #[serde(default)]
    pub answers: std::collections::BTreeMap<String, Vec<String>>,
}

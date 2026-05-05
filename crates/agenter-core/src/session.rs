use serde::{Deserialize, Serialize};

use chrono::{DateTime, Utc};

use std::collections::BTreeMap;

use crate::{
    ApprovalId, ApprovalRequest, ArtifactId, ArtifactState, DiffId, DiffState, ItemId, ItemState,
    NativeRef, PlanId, PlanState, QuestionId, RunnerId, SessionId, TurnId, UniversalSeq, UserId,
    WorkspaceId,
};

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct AgentProviderId(String);

impl AgentProviderId {
    pub const QWEN: &'static str = "qwen";
    pub const GEMINI: &'static str = "gemini";
    pub const OPENCODE: &'static str = "opencode";

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
    Idle,
    Stopped,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provider_details: Vec<ProviderCapabilityDetail>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderCapabilityDetail {
    pub key: String,
    pub status: ProviderCapabilityStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub methods: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCapabilityStatus {
    Supported,
    Degraded,
    Unsupported,
    NotApplicable,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CapabilitySet {
    #[serde(default)]
    pub protocol: ProtocolCapabilities,
    #[serde(default)]
    pub content: ContentCapabilities,
    #[serde(default)]
    pub tools: ToolCapabilities,
    #[serde(default)]
    pub approvals: ApprovalCapabilities,
    #[serde(default)]
    pub plan: PlanCapabilities,
    #[serde(default)]
    pub modes: ModeCapabilities,
    #[serde(default)]
    pub integration: IntegrationCapabilities,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provider_details: Vec<ProviderCapabilityDetail>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProtocolCapabilities {
    #[serde(default)]
    pub streaming: bool,
    #[serde(default)]
    pub session_resume: bool,
    #[serde(default)]
    pub session_history: bool,
    #[serde(default)]
    pub interrupt: bool,
    #[serde(default)]
    pub snapshots: bool,
    #[serde(default)]
    pub after_seq_replay: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ContentCapabilities {
    #[serde(default)]
    pub text: bool,
    #[serde(default)]
    pub images: bool,
    #[serde(default)]
    pub file_changes: bool,
    #[serde(default)]
    pub diffs: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolCapabilities {
    #[serde(default)]
    pub command_execution: bool,
    #[serde(default)]
    pub tool_user_input: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ApprovalCapabilities {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub per_session_allow: bool,
    #[serde(default)]
    pub deny_with_feedback: bool,
    #[serde(default)]
    pub cancel_turn: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlanCapabilities {
    #[serde(default)]
    pub updates: bool,
    #[serde(default)]
    pub approval: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModeCapabilities {
    #[serde(default)]
    pub model_selection: bool,
    #[serde(default)]
    pub reasoning_effort: bool,
    #[serde(default)]
    pub collaboration_modes: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct IntegrationCapabilities {
    #[serde(default)]
    pub mcp_elicitation: bool,
}

impl From<AgentCapabilities> for CapabilitySet {
    fn from(value: AgentCapabilities) -> Self {
        Self {
            protocol: ProtocolCapabilities {
                streaming: value.streaming,
                session_resume: value.session_resume,
                session_history: value.session_history,
                interrupt: value.interrupt,
                snapshots: false,
                after_seq_replay: false,
            },
            content: ContentCapabilities {
                text: true,
                images: false,
                file_changes: value.file_changes,
                diffs: value.file_changes,
            },
            tools: ToolCapabilities {
                command_execution: value.command_execution,
                tool_user_input: value.tool_user_input,
            },
            approvals: ApprovalCapabilities {
                enabled: value.approvals,
                per_session_allow: value.approvals,
                deny_with_feedback: value.approvals,
                cancel_turn: value.interrupt,
            },
            plan: PlanCapabilities {
                updates: value.plan_updates,
                approval: value.plan_updates && value.approvals,
            },
            modes: ModeCapabilities {
                model_selection: value.model_selection,
                reasoning_effort: value.reasoning_effort,
                collaboration_modes: value.collaboration_modes,
            },
            integration: IntegrationCapabilities {
                mcp_elicitation: value.mcp_elicitation,
            },
            provider_details: value.provider_details,
        }
    }
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionStatus {
    Pending,
    Answered,
    Cancelled,
    Expired,
    Orphaned,
    Detached,
}

impl QuestionStatus {
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Answered | Self::Cancelled | Self::Expired | Self::Orphaned | Self::Detached
        )
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct QuestionState {
    pub question_id: QuestionId,
    pub session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<AgentQuestionField>,
    pub status: QuestionStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer: Option<AgentQuestionAnswer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native: Option<NativeRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answered_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SessionSnapshot {
    pub session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_seq: Option<UniversalSeq>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub info: Option<SessionInfo>,
    #[serde(default)]
    pub capabilities: CapabilitySet,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub turns: BTreeMap<TurnId, TurnState>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub items: BTreeMap<ItemId, ItemState>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub approvals: BTreeMap<ApprovalId, ApprovalRequest>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub questions: BTreeMap<QuestionId, QuestionState>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub plans: BTreeMap<PlanId, PlanState>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub diffs: BTreeMap<DiffId, DiffState>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub artifacts: BTreeMap<ArtifactId, ArtifactState>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub active_turns: Vec<TurnId>,
}

impl Default for SessionSnapshot {
    fn default() -> Self {
        Self {
            session_id: SessionId::nil(),
            latest_seq: None,
            info: None,
            capabilities: CapabilitySet::default(),
            turns: BTreeMap::new(),
            items: BTreeMap::new(),
            approvals: BTreeMap::new(),
            questions: BTreeMap::new(),
            plans: BTreeMap::new(),
            diffs: BTreeMap::new(),
            artifacts: BTreeMap::new(),
            active_turns: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TurnState {
    pub turn_id: TurnId,
    pub session_id: SessionId,
    pub status: TurnStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnStatus {
    Starting,
    Running,
    WaitingForInput,
    WaitingForApproval,
    Interrupting,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
    Detached,
}

#[cfg(test)]
mod tests {
    use crate::{
        AgentCapabilities, CapabilitySet, ProviderCapabilityDetail, ProviderCapabilityStatus,
    };

    #[test]
    fn converts_source_agent_capabilities_to_nested_capability_set() {
        let capabilities = CapabilitySet::from(AgentCapabilities {
            streaming: true,
            session_resume: true,
            session_history: true,
            approvals: true,
            file_changes: true,
            command_execution: true,
            plan_updates: true,
            interrupt: true,
            model_selection: true,
            reasoning_effort: true,
            collaboration_modes: true,
            tool_user_input: true,
            mcp_elicitation: true,
            provider_details: vec![ProviderCapabilityDetail {
                key: "dynamic_tools".to_owned(),
                status: ProviderCapabilityStatus::Degraded,
                methods: vec!["item/tool/call".to_owned()],
                reason: Some("Visible but not executed remotely.".to_owned()),
            }],
        });

        assert!(capabilities.protocol.streaming);
        assert!(capabilities.protocol.session_resume);
        assert!(!capabilities.protocol.snapshots);
        assert!(!capabilities.protocol.after_seq_replay);
        assert!(capabilities.content.file_changes);
        assert!(capabilities.tools.command_execution);
        assert!(capabilities.approvals.enabled);
        assert!(capabilities.plan.updates);
        assert!(capabilities.modes.model_selection);
        assert!(capabilities.modes.reasoning_effort);
        assert!(capabilities.integration.mcp_elicitation);
        assert_eq!(
            capabilities.provider_details,
            vec![ProviderCapabilityDetail {
                key: "dynamic_tools".to_owned(),
                status: ProviderCapabilityStatus::Degraded,
                methods: vec!["item/tool/call".to_owned()],
                reason: Some("Visible but not executed remotely.".to_owned()),
            }]
        );
    }

    #[test]
    fn approvals_do_not_advertise_cancel_without_interrupt_support() {
        let capabilities = CapabilitySet::from(AgentCapabilities {
            approvals: true,
            interrupt: false,
            ..AgentCapabilities::default()
        });

        assert!(capabilities.approvals.enabled);
        assert!(!capabilities.approvals.cancel_turn);
        assert!(!capabilities.protocol.interrupt);
    }

    #[test]
    fn capabilities_serialize_legacy_booleans_and_provider_details() {
        let capabilities = AgentCapabilities {
            approvals: true,
            provider_details: vec![ProviderCapabilityDetail {
                key: "realtime".to_owned(),
                status: ProviderCapabilityStatus::Unsupported,
                methods: vec!["thread/realtime/started".to_owned()],
                reason: None,
            }],
            ..AgentCapabilities::default()
        };

        let json = serde_json::to_value(&capabilities).expect("serialize capabilities");
        assert_eq!(json["approvals"], true);
        assert_eq!(json["provider_details"][0]["key"], "realtime");
        assert_eq!(json["provider_details"][0]["status"], "unsupported");

        let decoded: AgentCapabilities =
            serde_json::from_value(json).expect("deserialize capabilities");
        assert_eq!(decoded, capabilities);
    }
}

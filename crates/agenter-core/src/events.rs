use chrono::{DateTime, Utc};
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

use crate::{
    AgentProviderId, AgentQuestionAnswer, AgentTurnSettings, ApprovalId, ApprovalRequest,
    ArtifactId, CommandId, DiffId, ItemId, PlanId, QuestionId, QuestionState, SessionId,
    SessionInfo, SessionStatus, SlashCommandRequest, TurnId, TurnState, UserId, WorkspaceRef,
};

pub const UNIVERSAL_PROTOCOL_VERSION: &str = "uap/2";

fn universal_protocol_version() -> String {
    UNIVERSAL_PROTOCOL_VERSION.to_owned()
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct UserMessageEvent {
    pub session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author_user_id: Option<UserId>,
    pub content: String,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlanEntry {
    pub label: String,
    pub status: PlanEntryStatus,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanEntryStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ToolEvent {
    pub session_id: SessionId,
    pub tool_call_id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_payload: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolProjection {
    pub kind: ToolProjectionKind,
    pub name: String,
    pub title: String,
    pub status: ItemStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<ToolCommandProjection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent: Option<ToolSubagentProjection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp: Option<ToolMcpProjection>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolProjectionKind {
    Command,
    Subagent,
    Mcp,
    Tool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolCommandProjection {
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<ToolActionProjection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub success: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolActionProjection {
    pub kind: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolSubagentProjection {
    pub operation: ToolSubagentOperation,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agent_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub states: Vec<ToolSubagentStateProjection>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolSubagentOperation {
    Spawn,
    Wait,
    Close,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolSubagentStateProjection {
    pub agent_id: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolMcpProjection {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    pub tool: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_summary: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeKind {
    Create,
    Modify,
    Delete,
    Rename,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct UniversalSeq(i64);

impl UniversalSeq {
    #[must_use]
    pub const fn new(value: i64) -> Self {
        assert!(value >= 0, "universal sequence must be non-negative");
        Self(value)
    }

    #[must_use]
    pub const fn zero() -> Self {
        Self(0)
    }

    #[must_use]
    pub const fn as_i64(self) -> i64 {
        self.0
    }
}

impl Serialize for UniversalSeq {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for UniversalSeq {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let value = value.parse::<i64>().map_err(de::Error::custom)?;
        if value < 0 {
            return Err(de::Error::custom("universal sequence must be non-negative"));
        }
        Ok(Self(value))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct UniversalEventEnvelope {
    pub event_id: String,
    pub seq: UniversalSeq,
    pub session_id: SessionId,
    pub turn_id: Option<TurnId>,
    pub item_id: Option<ItemId>,
    pub ts: DateTime<Utc>,
    pub source: UniversalEventSource,
    pub native: Option<NativeRef>,
    pub event: UniversalEventKind,
}

#[derive(Deserialize, Serialize)]
struct UniversalEventEnvelopeWire {
    #[serde(default = "universal_protocol_version")]
    protocol_version: String,
    event_id: String,
    seq: UniversalSeq,
    session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    turn_id: Option<TurnId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    item_id: Option<ItemId>,
    ts: DateTime<Utc>,
    source: UniversalEventSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    native: Option<NativeRef>,
    event: UniversalEventKind,
}

impl Serialize for UniversalEventEnvelope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        UniversalEventEnvelopeWire {
            protocol_version: universal_protocol_version(),
            event_id: self.event_id.clone(),
            seq: self.seq,
            session_id: self.session_id,
            turn_id: self.turn_id,
            item_id: self.item_id,
            ts: self.ts,
            source: self.source.clone(),
            native: self.native.clone(),
            event: self.event.clone(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for UniversalEventEnvelope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = UniversalEventEnvelopeWire::deserialize(deserializer)?;
        if wire.protocol_version != UNIVERSAL_PROTOCOL_VERSION {
            return Err(de::Error::custom(format!(
                "unsupported universal protocol version: {}",
                wire.protocol_version
            )));
        }
        Ok(Self {
            event_id: wire.event_id,
            seq: wire.seq,
            session_id: wire.session_id,
            turn_id: wire.turn_id,
            item_id: wire.item_id,
            ts: wire.ts,
            source: wire.source,
            native: wire.native,
            event: wire.event,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UniversalEventSource {
    ControlPlane,
    Runner,
    Browser,
    Connector,
    Native,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NativeRef {
    pub protocol: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pointer: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", content = "data")]
pub enum UniversalEventKind {
    #[serde(rename = "session.created")]
    SessionCreated { session: Box<SessionInfo> },
    #[serde(rename = "session.status_changed")]
    SessionStatusChanged {
        status: SessionStatus,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    #[serde(rename = "session.metadata_changed")]
    SessionMetadataChanged {
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    #[serde(rename = "turn.started")]
    TurnStarted { turn: TurnState },
    #[serde(rename = "turn.status_changed")]
    TurnStatusChanged { turn: TurnState },
    #[serde(rename = "turn.completed")]
    TurnCompleted { turn: TurnState },
    #[serde(rename = "turn.failed")]
    TurnFailed { turn: TurnState },
    #[serde(rename = "turn.cancelled")]
    TurnCancelled { turn: TurnState },
    #[serde(rename = "turn.interrupted")]
    TurnInterrupted { turn: TurnState },
    #[serde(rename = "turn.detached")]
    TurnDetached { turn: TurnState },
    #[serde(rename = "item.created")]
    ItemCreated { item: Box<ItemState> },
    #[serde(rename = "content.delta")]
    ContentDelta {
        block_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        kind: Option<ContentBlockKind>,
        delta: String,
    },
    #[serde(rename = "content.completed")]
    ContentCompleted {
        block_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        kind: Option<ContentBlockKind>,
        #[serde(skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    #[serde(rename = "approval.requested")]
    ApprovalRequested { approval: Box<ApprovalRequest> },
    #[serde(rename = "approval.resolved")]
    ApprovalResolved {
        approval_id: ApprovalId,
        status: crate::ApprovalStatus,
        resolved_at: DateTime<Utc>,
        #[serde(skip_serializing_if = "Option::is_none")]
        resolved_by_user_id: Option<UserId>,
        #[serde(skip_serializing_if = "Option::is_none")]
        native: Option<NativeRef>,
    },
    #[serde(rename = "question.requested")]
    QuestionRequested { question: Box<QuestionState> },
    #[serde(rename = "question.answered")]
    QuestionAnswered { question: Box<QuestionState> },
    #[serde(rename = "plan.updated")]
    PlanUpdated { plan: PlanState },
    #[serde(rename = "diff.updated")]
    DiffUpdated { diff: DiffState },
    #[serde(rename = "artifact.created")]
    ArtifactCreated { artifact: ArtifactState },
    #[serde(rename = "usage.updated")]
    UsageUpdated {
        usage: Box<crate::SessionUsageSnapshot>,
    },
    #[serde(rename = "error.reported")]
    ErrorReported {
        #[serde(skip_serializing_if = "Option::is_none")]
        code: Option<String>,
        message: String,
    },
    #[serde(rename = "provider.notification")]
    ProviderNotification { notification: ProviderNotification },
    #[serde(rename = "native.unknown")]
    NativeUnknown {
        #[serde(skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderNotification {
    pub category: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<ProviderNotificationSeverity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderNotificationSeverity {
    Debug,
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UniversalCommandEnvelope {
    pub command_id: CommandId,
    pub idempotency_key: String,
    pub session_id: Option<SessionId>,
    pub turn_id: Option<TurnId>,
    pub command: UniversalCommand,
}

#[derive(Deserialize, Serialize)]
struct UniversalCommandEnvelopeWire {
    #[serde(default = "universal_protocol_version")]
    protocol_version: String,
    command_id: CommandId,
    idempotency_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<SessionId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    turn_id: Option<TurnId>,
    command: UniversalCommand,
}

impl Serialize for UniversalCommandEnvelope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        UniversalCommandEnvelopeWire {
            protocol_version: universal_protocol_version(),
            command_id: self.command_id,
            idempotency_key: self.idempotency_key.clone(),
            session_id: self.session_id,
            turn_id: self.turn_id,
            command: self.command.clone(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for UniversalCommandEnvelope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = UniversalCommandEnvelopeWire::deserialize(deserializer)?;
        if wire.protocol_version != UNIVERSAL_PROTOCOL_VERSION {
            return Err(de::Error::custom(format!(
                "unsupported universal protocol version: {}",
                wire.protocol_version
            )));
        }
        Ok(Self {
            command_id: wire.command_id,
            idempotency_key: wire.idempotency_key,
            session_id: wire.session_id,
            turn_id: wire.turn_id,
            command: wire.command,
        })
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UniversalCommand {
    StartSession {
        session_id: SessionId,
        workspace: WorkspaceRef,
        provider_id: AgentProviderId,
        #[serde(skip_serializing_if = "Option::is_none")]
        initial_input: Option<UserInput>,
    },
    LoadSession {
        session_id: SessionId,
        workspace: WorkspaceRef,
        provider_id: AgentProviderId,
        external_session_id: String,
    },
    CloseSession,
    StartTurn {
        input: UserInput,
        #[serde(skip_serializing_if = "Option::is_none")]
        settings: Option<AgentTurnSettings>,
    },
    CancelTurn {
        #[serde(skip_serializing_if = "Option::is_none")]
        request: Option<SlashCommandRequest>,
    },
    SendUserInput {
        input: UserInput,
    },
    ResolveApproval {
        approval_id: ApprovalId,
        option_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        feedback: Option<String>,
    },
    AnswerQuestion {
        question_id: QuestionId,
        answer: AgentQuestionAnswer,
    },
    SetMode {
        mode: String,
    },
    SetModel {
        model: String,
    },
    SetTurnSettings {
        settings: AgentTurnSettings,
    },
    ExecuteProviderCommand {
        command: SlashCommandRequest,
    },
    RequestDiff {
        #[serde(skip_serializing_if = "Option::is_none")]
        diff_id: Option<DiffId>,
    },
    RevertChange {
        diff_id: DiffId,
        #[serde(skip_serializing_if = "Option::is_none")]
        change_id: Option<String>,
    },
    Subscribe {
        session_id: SessionId,
        #[serde(skip_serializing_if = "Option::is_none")]
        after_seq: Option<UniversalSeq>,
        #[serde(default, skip_serializing_if = "is_false")]
        include_snapshot: bool,
    },
    GetSnapshot {
        session_id: SessionId,
        #[serde(skip_serializing_if = "Option::is_none")]
        after_seq: Option<UniversalSeq>,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UserInput {
    Text { text: String },
    Blocks { blocks: Vec<ContentBlock> },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ItemState {
    pub item_id: ItemId,
    pub session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    pub role: ItemRole,
    pub status: ItemStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content: Vec<ContentBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<ToolProjection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native: Option<NativeRef>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemRole {
    User,
    Assistant,
    Tool,
    System,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    Created,
    Streaming,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ContentBlock {
    pub block_id: String,
    pub kind: ContentBlockKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_id: Option<ArtifactId>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentBlockKind {
    Text,
    Reasoning,
    ToolCall,
    ToolResult,
    CommandOutput,
    TerminalInput,
    FileDiff,
    Image,
    Native,
    Warning,
    ProviderStatus,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlanState {
    pub plan_id: PlanId,
    pub session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    pub status: PlanStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<UniversalPlanEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_refs: Vec<ArtifactId>,
    #[serde(default)]
    pub source: PlanSource,
    #[serde(default, skip_serializing_if = "is_false")]
    pub partial: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    None,
    Discovering,
    Draft,
    AwaitingApproval,
    RevisionRequested,
    Approved,
    Implementing,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanSource {
    #[default]
    NativeStructured,
    MarkdownFile,
    TodoTool,
    Synthetic,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct UniversalPlanEntry {
    pub entry_id: String,
    pub label: String,
    pub status: PlanEntryStatus,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DiffState {
    pub diff_id: DiffId,
    pub session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<DiffFile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DiffFile {
    pub path: String,
    pub status: FileChangeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ArtifactState {
    pub artifact_id: ArtifactId,
    pub session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    pub kind: ArtifactKind,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    File,
    Image,
    Plan,
    Diff,
    Link,
    Native,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ApprovalKind, ApprovalStatus, RunnerId, WorkspaceId};

    #[test]
    fn universal_event_serializes_with_uap2_version() {
        let session_id = SessionId::new();
        let event = UniversalEventEnvelope {
            event_id: "event-1".to_owned(),
            seq: UniversalSeq::new(7),
            session_id,
            turn_id: None,
            item_id: None,
            ts: Utc::now(),
            source: UniversalEventSource::Runner,
            native: None,
            event: UniversalEventKind::ProviderNotification {
                notification: ProviderNotification {
                    category: "test".to_owned(),
                    title: "Test".to_owned(),
                    detail: None,
                    status: None,
                    severity: Some(ProviderNotificationSeverity::Info),
                    subject: None,
                },
            },
        };

        let json = serde_json::to_value(&event).expect("json");
        assert_eq!(json["protocol_version"], UNIVERSAL_PROTOCOL_VERSION);
        assert_eq!(json["event"]["type"], "provider.notification");
    }

    #[test]
    fn approval_request_carries_resolving_decision() {
        let request = ApprovalRequest {
            approval_id: ApprovalId::new(),
            session_id: SessionId::new(),
            turn_id: None,
            item_id: None,
            kind: ApprovalKind::Command,
            title: "Run command".to_owned(),
            details: None,
            options: Vec::new(),
            status: ApprovalStatus::Resolving,
            risk: None,
            subject: None,
            native_request_id: None,
            native_blocking: true,
            policy: None,
            native: None,
            requested_at: None,
            resolved_at: None,
            resolving_decision: Some(crate::ApprovalDecision::Accept),
        };

        let json = serde_json::to_value(&request).expect("json");
        assert_eq!(json["resolving_decision"]["decision"], "accept");
    }

    #[test]
    fn session_created_event_keeps_provider_open_string() {
        let session_id = SessionId::new();
        let event = UniversalEventKind::SessionCreated {
            session: Box::new(SessionInfo {
                session_id,
                owner_user_id: UserId::new(),
                runner_id: RunnerId::new(),
                workspace_id: WorkspaceId::new(),
                provider_id: AgentProviderId::from("qwen"),
                status: SessionStatus::Running,
                external_session_id: None,
                title: Some("Qwen".to_owned()),
                created_at: None,
                updated_at: None,
                usage: None,
            }),
        };
        let json = serde_json::to_value(&event).expect("json");
        assert_eq!(json["data"]["session"]["provider_id"], "qwen");
    }
}

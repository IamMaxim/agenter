use chrono::{DateTime, Utc};
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

use crate::{
    AgentProviderId, AgentQuestionAnswer, AgentTurnSettings, ApprovalId, ApprovalRequest,
    ApprovalRequestEvent, ApprovalResolvedEvent, ArtifactId, CommandId, DiffId, ItemId, PlanId,
    QuestionAnsweredEvent, QuestionId, QuestionRequestedEvent, QuestionState, SessionId,
    SessionInfo, SessionStatus, SlashCommandRequest, TurnId, TurnState, UserId, WorkspaceRef,
};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum NormalizedEvent {
    SessionStarted(SessionInfo),
    SessionStatusChanged(SessionStatusChangedEvent),
    UserMessage(UserMessageEvent),
    AgentMessageDelta(AgentMessageDeltaEvent),
    AgentMessageCompleted(MessageCompletedEvent),
    PlanUpdated(PlanEvent),
    ToolStarted(ToolEvent),
    ToolUpdated(ToolEvent),
    ToolCompleted(ToolEvent),
    CommandStarted(CommandEvent),
    CommandOutputDelta(CommandOutputEvent),
    CommandCompleted(CommandCompletedEvent),
    FileChangeProposed(FileChangeEvent),
    FileChangeApplied(FileChangeEvent),
    FileChangeRejected(FileChangeEvent),
    ApprovalRequested(ApprovalRequestEvent),
    ApprovalResolved(ApprovalResolvedEvent),
    QuestionRequested(QuestionRequestedEvent),
    QuestionAnswered(QuestionAnsweredEvent),
    TurnStatusChanged(TurnState),
    TurnFailed(TurnState),
    TurnCancelled(TurnState),
    TurnInterrupted(TurnState),
    TurnDiffUpdated(NativeNotification),
    ItemReasoning(NativeNotification),
    ServerRequestResolved(NativeNotification),
    McpToolCallProgress(NativeNotification),
    ThreadRealtimeEvent(NativeNotification),
    NativeNotification(NativeNotification),
    Error(AgentErrorEvent),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SessionStatusChangedEvent {
    pub session_id: SessionId,
    pub status: SessionStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
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

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AgentMessageDeltaEvent {
    pub session_id: SessionId,
    pub message_id: String,
    pub delta: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_payload: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MessageCompletedEvent {
    pub session_id: SessionId,
    pub message_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_payload: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PlanEvent {
    pub session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<PlanEntry>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub append: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_payload: Option<serde_json::Value>,
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

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CommandEvent {
    pub session_id: SessionId,
    pub command_id: String,
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<CommandAction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_payload: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CommandAction {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
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

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CommandOutputEvent {
    pub session_id: SessionId,
    pub command_id: String,
    pub stream: CommandOutputStream,
    pub delta: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_payload: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandOutputStream {
    Stdout,
    Stderr,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CommandCompletedEvent {
    pub session_id: SessionId,
    pub command_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_payload: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct FileChangeEvent {
    pub session_id: SessionId,
    pub path: String,
    pub change_kind: FileChangeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_payload: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeKind {
    Create,
    Modify,
    Delete,
    Rename,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct NativeNotification {
    pub session_id: SessionId,
    pub provider_id: AgentProviderId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    pub method: String,
    pub category: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_payload: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AgentErrorEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_payload: Option<serde_json::Value>,
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

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct UniversalEventEnvelope {
    pub event_id: String,
    pub seq: UniversalSeq,
    pub session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_id: Option<ItemId>,
    pub ts: DateTime<Utc>,
    pub source: UniversalEventSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native: Option<NativeRef>,
    pub event: UniversalEventKind,
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
    #[serde(rename = "native.unknown")]
    NativeUnknown {
        #[serde(skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct UniversalCommandEnvelope {
    pub command_id: CommandId,
    pub idempotency_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    pub command: UniversalCommand,
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
    FileDiff,
    Image,
    Native,
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
    use std::str::FromStr;

    use chrono::{TimeZone, Utc};

    use crate::{
        AgentCapabilities, AgentCollaborationMode, AgentModelOption, AgentProviderId,
        AgentQuestionAnswer, AgentQuestionChoice, AgentQuestionField, AgentReasoningEffort,
        AgentTurnSettings, ApprovalDecision, ApprovalId, ApprovalKind, ApprovalOption,
        ApprovalOptionKind, ApprovalRequest, ApprovalRequestEvent, ApprovalResolvedEvent,
        CommandEvent, CommandId, ContentBlock, ContentBlockKind, ItemId, NativeRef,
        NormalizedEvent, PlanEntryStatus, PlanId, PlanSource, PlanState, PlanStatus, QuestionId,
        QuestionRequestedEvent, RunnerId, SessionId, SessionInfo, SessionStatus, TurnId,
        UniversalCommand, UniversalCommandEnvelope, UniversalEventEnvelope, UniversalEventKind,
        UniversalEventSource, UniversalPlanEntry, UniversalSeq, UserId, UserInput,
        UserMessageEvent, WorkspaceId,
    };

    #[test]
    fn serializes_user_message_event_with_adjacent_tag() {
        let event = NormalizedEvent::UserMessage(UserMessageEvent {
            session_id: SessionId::nil(),
            message_id: Some("msg-1".to_owned()),
            author_user_id: None,
            content: "hello".to_owned(),
        });

        let json = serde_json::to_value(event).expect("serialize event");

        assert_eq!(json["type"], "user_message");
        assert_eq!(json["payload"]["session_id"], SessionId::nil().to_string());
        assert_eq!(json["payload"]["message_id"], "msg-1");
        assert_eq!(json["payload"]["content"], "hello");
    }

    #[test]
    fn serializes_command_started_event_with_stable_shape() {
        let event = NormalizedEvent::CommandStarted(CommandEvent {
            session_id: SessionId::nil(),
            command_id: "cmd-1".to_owned(),
            command: "cargo test -p agenter-core".to_owned(),
            cwd: Some("/work/agenter".to_owned()),
            source: None,
            process_id: None,
            actions: Vec::new(),
            provider_payload: None,
        });

        let json = serde_json::to_value(event).expect("serialize event");

        assert_eq!(json["type"], "command_started");
        assert_eq!(json["payload"]["command_id"], "cmd-1");
        assert_eq!(json["payload"]["cwd"], "/work/agenter");
        assert!(json["payload"].get("provider_payload").is_none());
    }

    #[test]
    fn round_trips_native_notification_with_raw_payload() {
        let event = NormalizedEvent::NativeNotification(crate::NativeNotification {
            session_id: SessionId::nil(),
            provider_id: AgentProviderId::from(AgentProviderId::CODEX),
            event_id: Some("compact-1".to_owned()),
            method: "thread/compacted".to_owned(),
            category: "compaction".to_owned(),
            title: "Context compacted".to_owned(),
            detail: Some("Codex compacted the active thread context".to_owned()),
            status: Some("completed".to_owned()),
            provider_payload: Some(serde_json::json!({
                "method": "thread/compacted",
                "params": {"threadId": "thread-1", "turnId": "turn-1"}
            })),
        });

        let json = serde_json::to_value(&event).expect("serialize event");
        let decoded: NormalizedEvent =
            serde_json::from_value(json.clone()).expect("deserialize event");

        assert_eq!(json["type"], "native_notification");
        assert_eq!(json["payload"]["provider_id"], AgentProviderId::CODEX);
        assert_eq!(json["payload"]["category"], "compaction");
        assert_eq!(json["payload"]["status"], "completed");
        assert_eq!(decoded, event);
    }

    #[test]
    fn serializes_plan_delta_marker_only_when_append_is_true() {
        let event = NormalizedEvent::PlanUpdated(crate::PlanEvent {
            session_id: SessionId::nil(),
            plan_id: Some("plan-1".to_owned()),
            title: Some("Implementation plan".to_owned()),
            content: Some("next words".to_owned()),
            entries: Vec::new(),
            append: true,
            provider_payload: None,
        });

        let json = serde_json::to_value(event).expect("serialize event");

        assert_eq!(json["type"], "plan_updated");
        assert_eq!(json["payload"]["append"], true);
    }

    #[test]
    fn serializes_universal_plan_state_status_source_and_partial_marker() {
        let plan = PlanState {
            plan_id: PlanId::nil(),
            session_id: SessionId::nil(),
            turn_id: None,
            status: PlanStatus::AwaitingApproval,
            title: Some("Implementation plan".to_owned()),
            content: Some("Review this plan".to_owned()),
            entries: vec![UniversalPlanEntry {
                entry_id: "entry-1".to_owned(),
                label: "Inspect".to_owned(),
                status: PlanEntryStatus::Failed,
            }],
            artifact_refs: Vec::new(),
            source: PlanSource::MarkdownFile,
            partial: true,
            updated_at: None,
        };

        let json = serde_json::to_value(&plan).expect("serialize plan state");
        let decoded: PlanState = serde_json::from_value(json.clone()).expect("deserialize plan");

        assert_eq!(json["status"], "awaiting_approval");
        assert_eq!(json["source"], "markdown_file");
        assert_eq!(json["partial"], true);
        assert_eq!(json["entries"][0]["status"], "failed");
        assert_eq!(decoded, plan);
    }

    #[test]
    fn serializes_failed_and_cancelled_plan_statuses() {
        let failed = serde_json::to_value(PlanStatus::Failed).expect("serialize failed status");
        let cancelled_entry =
            serde_json::to_value(PlanEntryStatus::Cancelled).expect("serialize entry status");

        assert_eq!(failed, "failed");
        assert_eq!(cancelled_entry, "cancelled");
    }

    #[test]
    fn serializes_approval_requested_event_with_uuid_ids() {
        let approval_id = ApprovalId::nil();
        let event = NormalizedEvent::ApprovalRequested(ApprovalRequestEvent {
            session_id: SessionId::nil(),
            approval_id,
            kind: ApprovalKind::Command,
            title: "Run command".to_owned(),
            details: Some("cargo test".to_owned()),
            expires_at: None,
            presentation: None,
            resolution_state: None,
            resolving_decision: None,
            status: None,
            turn_id: None,
            item_id: None,
            options: Vec::new(),
            risk: None,
            subject: None,
            native_request_id: Some("approval-1".to_owned()),
            native_blocking: true,
            policy: None,
            provider_payload: Some(serde_json::json!({ "native_id": "approval-1" })),
        });

        let json = serde_json::to_value(event).expect("serialize event");

        assert_eq!(json["type"], "approval_requested");
        assert_eq!(json["payload"]["approval_id"], approval_id.to_string());
        assert_eq!(json["payload"]["kind"], "command");
        assert_eq!(
            json["payload"]["provider_payload"]["native_id"],
            "approval-1"
        );

        let with_pres = NormalizedEvent::ApprovalRequested(ApprovalRequestEvent {
            session_id: SessionId::nil(),
            approval_id,
            kind: ApprovalKind::FileChange,
            title: "Patch".to_owned(),
            details: Some("delta".to_owned()),
            expires_at: None,
            presentation: Some(
                serde_json::json!({"variant": "codex_file_change", "paths": ["a.rs"]}),
            ),
            resolution_state: None,
            resolving_decision: None,
            status: None,
            turn_id: None,
            item_id: None,
            options: Vec::new(),
            risk: None,
            subject: None,
            native_request_id: None,
            native_blocking: true,
            policy: None,
            provider_payload: None,
        });
        let pj = serde_json::to_value(&with_pres).expect("serialize approval with presentation");
        assert!(pj["payload"]["presentation"]["paths"][0].is_string());
    }

    #[test]
    fn round_trips_session_started_event_with_provider_context() {
        let event = NormalizedEvent::SessionStarted(SessionInfo {
            session_id: SessionId::nil(),
            owner_user_id: UserId::nil(),
            runner_id: RunnerId::nil(),
            workspace_id: WorkspaceId::nil(),
            provider_id: AgentProviderId::from(AgentProviderId::CODEX),
            status: SessionStatus::Running,
            external_session_id: Some("thread-1".to_owned()),
            title: Some("Initial setup".to_owned()),
            created_at: None,
            updated_at: None,
            usage: None,
        });

        let json = serde_json::to_value(&event).expect("serialize event");
        let decoded: NormalizedEvent =
            serde_json::from_value(json.clone()).expect("deserialize event");

        assert_eq!(json["type"], "session_started");
        assert_eq!(json["payload"]["provider_id"], AgentProviderId::CODEX);
        assert_eq!(json["payload"]["status"], "running");
        assert_eq!(decoded, event);

        let capabilities = AgentCapabilities {
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
            provider_details: Vec::new(),
        };
        let capabilities_json = serde_json::to_value(capabilities).expect("serialize caps");
        assert_eq!(capabilities_json["session_history"], true);
    }

    #[test]
    fn round_trips_agent_options_and_turn_settings() {
        let model = AgentModelOption {
            id: "gpt-5.4".to_owned(),
            display_name: "GPT-5.4".to_owned(),
            description: Some("Balanced coding model".to_owned()),
            is_default: true,
            default_reasoning_effort: Some(AgentReasoningEffort::Medium),
            supported_reasoning_efforts: vec![
                AgentReasoningEffort::Low,
                AgentReasoningEffort::Medium,
                AgentReasoningEffort::High,
            ],
            input_modalities: vec!["text".to_owned(), "image".to_owned()],
        };
        let mode = AgentCollaborationMode {
            id: "plan".to_owned(),
            label: "Plan".to_owned(),
            model: Some("gpt-5.4".to_owned()),
            reasoning_effort: Some(AgentReasoningEffort::High),
        };
        let settings = AgentTurnSettings {
            model: Some(model.id.clone()),
            reasoning_effort: Some(AgentReasoningEffort::High),
            collaboration_mode: Some("plan".to_owned()),
        };

        let json = serde_json::json!({
            "model": model,
            "mode": mode,
            "settings": settings
        });

        assert_eq!(json["model"]["id"], "gpt-5.4");
        assert_eq!(json["model"]["default_reasoning_effort"], "medium");
        assert_eq!(json["mode"]["id"], "plan");
        assert_eq!(json["settings"]["reasoning_effort"], "high");
        assert_eq!(json["settings"]["collaboration_mode"], "plan");
    }

    #[test]
    fn round_trips_question_requested_event_with_multi_select() {
        let question_id = QuestionId::nil();
        let event = NormalizedEvent::QuestionRequested(QuestionRequestedEvent {
            session_id: SessionId::nil(),
            question_id,
            title: "Need a choice".to_owned(),
            description: Some("Pick one or more targets".to_owned()),
            fields: vec![AgentQuestionField {
                id: "targets".to_owned(),
                label: "Targets".to_owned(),
                prompt: Some("Which targets should be updated?".to_owned()),
                kind: "multi_select".to_owned(),
                required: true,
                secret: false,
                choices: vec![
                    AgentQuestionChoice {
                        value: "web".to_owned(),
                        label: "Web".to_owned(),
                        description: Some("Browser UI".to_owned()),
                    },
                    AgentQuestionChoice {
                        value: "runner".to_owned(),
                        label: "Runner".to_owned(),
                        description: None,
                    },
                ],
                default_answers: vec!["web".to_owned()],
            }],
            provider_payload: None,
        });

        let answer = AgentQuestionAnswer {
            question_id,
            answers: std::collections::BTreeMap::from([(
                "targets".to_owned(),
                vec!["web".to_owned(), "runner".to_owned()],
            )]),
        };

        let event_json = serde_json::to_value(&event).expect("serialize question");
        let answer_json = serde_json::to_value(answer).expect("serialize answer");
        let decoded: NormalizedEvent =
            serde_json::from_value(event_json.clone()).expect("decode event");

        assert_eq!(event_json["type"], "question_requested");
        assert_eq!(event_json["payload"]["fields"][0]["kind"], "multi_select");
        assert_eq!(answer_json["answers"]["targets"][1], "runner");
        assert_eq!(decoded, event);
    }

    #[test]
    fn round_trips_approval_resolved_with_provider_specific_decision() {
        let event = NormalizedEvent::ApprovalResolved(ApprovalResolvedEvent {
            session_id: SessionId::nil(),
            approval_id: ApprovalId::nil(),
            decision: ApprovalDecision::ProviderSpecific {
                payload: serde_json::json!({
                    "native_decision": "reject_once",
                    "option_id": "deny-1"
                }),
            },
            resolved_by_user_id: Some(UserId::nil()),
            resolved_at: Utc
                .with_ymd_and_hms(2026, 4, 30, 12, 0, 0)
                .single()
                .expect("valid timestamp"),
            provider_payload: None,
        });

        let json = serde_json::to_value(&event).expect("serialize event");
        let decoded: NormalizedEvent =
            serde_json::from_value(json.clone()).expect("deserialize event");

        assert_eq!(json["type"], "approval_resolved");
        assert_eq!(
            json["payload"]["decision"],
            serde_json::json!({
                "decision": "provider_specific",
                "payload": {
                    "native_decision": "reject_once",
                    "option_id": "deny-1"
                }
            })
        );
        assert_eq!(decoded, event);
    }

    #[test]
    fn parses_and_serializes_uuid_id_newtypes_without_default_generation() {
        let raw = "00000000-0000-0000-0000-000000000042";
        let session_id = SessionId::from_str(raw).expect("parse session id");
        let json = serde_json::to_value(session_id).expect("serialize session id");
        let decoded: SessionId = serde_json::from_value(json.clone()).expect("deserialize id");

        assert_eq!(json, raw);
        assert_eq!(decoded.to_string(), raw);
    }

    #[test]
    fn universal_seq_serializes_as_json_string() {
        let seq = UniversalSeq::new(9_007_199_254_740_993);

        let json = serde_json::to_value(seq).expect("serialize seq");
        let decoded: UniversalSeq = serde_json::from_value(json.clone()).expect("deserialize seq");

        assert_eq!(json, "9007199254740993");
        assert_eq!(decoded, seq);
    }

    #[test]
    fn universal_seq_zero_is_from_beginning_sentinel() {
        let seq = UniversalSeq::zero();

        let json = serde_json::to_value(seq).expect("serialize seq");
        let decoded: UniversalSeq = serde_json::from_value(json.clone()).expect("deserialize seq");

        assert_eq!(json, "0");
        assert_eq!(decoded.as_i64(), 0);
    }

    #[test]
    fn universal_seq_rejects_negative_json_strings() {
        let error = serde_json::from_value::<UniversalSeq>(serde_json::json!("-1"))
            .expect_err("negative sequence should fail");

        assert!(error
            .to_string()
            .contains("universal sequence must be non-negative"));
    }

    #[test]
    #[should_panic(expected = "universal sequence must be non-negative")]
    fn universal_seq_rejects_negative_construction() {
        let _ = UniversalSeq::new(-1);
    }

    #[test]
    fn round_trips_universal_event_envelope_with_dot_event_name() {
        let turn_id = TurnId::nil();
        let item_id = ItemId::nil();
        let event = UniversalEventEnvelope {
            event_id: "evt-1".to_owned(),
            seq: UniversalSeq::new(42),
            session_id: SessionId::nil(),
            turn_id: Some(turn_id),
            item_id: Some(item_id),
            ts: Utc
                .with_ymd_and_hms(2026, 5, 3, 12, 0, 0)
                .single()
                .expect("valid timestamp"),
            source: UniversalEventSource::Runner,
            native: Some(NativeRef {
                protocol: "codex".to_owned(),
                method: Some("turn/item/updated".to_owned()),
                kind: Some("message_delta".to_owned()),
                native_id: Some("native-item-1".to_owned()),
                summary: Some("assistant text delta".to_owned()),
                hash: Some("sha256:abc123".to_owned()),
                pointer: Some("runner-log://codex/session-1#42".to_owned()),
            }),
            event: UniversalEventKind::ContentDelta {
                block_id: "block-1".to_owned(),
                kind: None,
                delta: "hello".to_owned(),
            },
        };

        let json = serde_json::to_value(&event).expect("serialize universal event");
        let decoded: UniversalEventEnvelope =
            serde_json::from_value(json.clone()).expect("deserialize universal event");

        assert_eq!(json["seq"], "42");
        assert_eq!(json["source"], "runner");
        assert_eq!(json["event"]["type"], "content.delta");
        assert_eq!(json["native"]["summary"], "assistant text delta");
        assert_eq!(decoded, event);
    }

    #[test]
    fn representative_universal_event_variants_use_dot_names() {
        let approval = ApprovalRequest {
            approval_id: ApprovalId::nil(),
            session_id: SessionId::nil(),
            turn_id: Some(TurnId::nil()),
            item_id: Some(ItemId::nil()),
            kind: ApprovalKind::Command,
            title: "Run tests".to_owned(),
            details: Some("cargo test".to_owned()),
            options: vec![ApprovalOption {
                option_id: "approve_once".to_owned(),
                kind: ApprovalOptionKind::ApproveOnce,
                label: "Approve once".to_owned(),
                description: None,
                scope: None,
                native_option_id: None,
                policy_rule: None,
            }],
            status: crate::ApprovalStatus::Pending,
            risk: Some("medium".to_owned()),
            subject: Some("cargo test".to_owned()),
            native_request_id: Some("approval-1".to_owned()),
            native_blocking: true,
            policy: None,
            native: None,
            requested_at: None,
            resolved_at: None,
        };
        let events = [
            UniversalEventKind::SessionCreated {
                session: Box::new(SessionInfo {
                    session_id: SessionId::nil(),
                    owner_user_id: UserId::nil(),
                    runner_id: RunnerId::nil(),
                    workspace_id: WorkspaceId::nil(),
                    provider_id: AgentProviderId::from(AgentProviderId::CODEX),
                    status: SessionStatus::Running,
                    external_session_id: None,
                    title: None,
                    created_at: None,
                    updated_at: None,
                    usage: None,
                }),
            },
            UniversalEventKind::TurnStarted {
                turn: crate::TurnState {
                    turn_id: TurnId::nil(),
                    session_id: SessionId::nil(),
                    status: crate::TurnStatus::Running,
                    started_at: None,
                    completed_at: None,
                    model: None,
                    mode: None,
                },
            },
            UniversalEventKind::ItemCreated {
                item: Box::new(crate::ItemState {
                    item_id: ItemId::nil(),
                    session_id: SessionId::nil(),
                    turn_id: Some(TurnId::nil()),
                    role: crate::ItemRole::Assistant,
                    status: crate::ItemStatus::Streaming,
                    content: vec![ContentBlock {
                        block_id: "block-1".to_owned(),
                        kind: ContentBlockKind::Text,
                        text: Some("hello".to_owned()),
                        mime_type: None,
                        artifact_id: None,
                    }],
                    tool: None,
                    native: None,
                }),
            },
            UniversalEventKind::ApprovalRequested {
                approval: Box::new(approval),
            },
            UniversalEventKind::NativeUnknown {
                summary: Some("unmodeled native notification".to_owned()),
            },
        ];

        let names: Vec<_> = events
            .into_iter()
            .map(|event| serde_json::to_value(event).expect("serialize event")["type"].clone())
            .collect();

        assert_eq!(
            names,
            vec![
                "session.created",
                "turn.started",
                "item.created",
                "approval.requested",
                "native.unknown"
            ]
        );
    }

    #[test]
    fn approval_option_serialization_is_payload_free() {
        let option = ApprovalOption {
            option_id: "native-allow".to_owned(),
            kind: ApprovalOptionKind::ProviderSpecific,
            label: "Allow".to_owned(),
            description: Some("Provider-specific safe option label".to_owned()),
            scope: Some("session".to_owned()),
            native_option_id: Some("allow_for_session".to_owned()),
            policy_rule: None,
        };

        let json = serde_json::to_value(option).expect("serialize option");

        assert_eq!(json["kind"], "provider_specific");
        assert_eq!(json["native_option_id"], "allow_for_session");
        assert!(json.get("decision").is_none());
        assert!(json.get("payload").is_none());
    }

    #[test]
    fn round_trips_universal_command_envelope() {
        let command = UniversalCommandEnvelope {
            command_id: CommandId::nil(),
            idempotency_key: "send-input-1".to_owned(),
            session_id: Some(SessionId::nil()),
            turn_id: Some(TurnId::nil()),
            command: UniversalCommand::SendUserInput {
                input: UserInput::Text {
                    text: "continue".to_owned(),
                },
            },
        };

        let json = serde_json::to_value(&command).expect("serialize command");
        let decoded: UniversalCommandEnvelope =
            serde_json::from_value(json.clone()).expect("deserialize command");

        assert_eq!(json["command_id"], CommandId::nil().to_string());
        assert_eq!(json["command"]["type"], "send_user_input");
        assert_eq!(json["command"]["input"]["type"], "text");
        assert_eq!(decoded, command);
    }

    #[test]
    fn universal_resolve_approval_command_is_payload_free() {
        let command = UniversalCommandEnvelope {
            command_id: CommandId::nil(),
            idempotency_key: "approval-1".to_owned(),
            session_id: Some(SessionId::nil()),
            turn_id: Some(TurnId::nil()),
            command: UniversalCommand::ResolveApproval {
                approval_id: ApprovalId::nil(),
                option_id: "approve_once".to_owned(),
                feedback: Some("Looks fine".to_owned()),
            },
        };

        let json = serde_json::to_value(&command).expect("serialize command");
        let decoded: UniversalCommandEnvelope =
            serde_json::from_value(json.clone()).expect("deserialize command");

        assert_eq!(json["command"]["type"], "resolve_approval");
        assert_eq!(
            json["command"]["approval_id"],
            ApprovalId::nil().to_string()
        );
        assert_eq!(json["command"]["option_id"], "approve_once");
        assert_eq!(json["command"]["feedback"], "Looks fine");
        assert!(json["command"].get("decision").is_none());
        assert!(json["command"].get("payload").is_none());
        assert_eq!(decoded, command);
    }
}

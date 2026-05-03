use agenter_core::{
    AgentCapabilities, AgentOptions, AgentProviderId, AgentQuestionAnswer, AgentTurnSettings,
    ApprovalDecision, ApprovalId, FileChangeKind, ItemId, NativeRef, SessionId,
    SlashCommandDefinition, SlashCommandRequest, SlashCommandResult, TurnId, UniversalEventKind,
    UniversalEventSource, UserMessageEvent, WorkspaceRef,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use crate::RequestId;

pub const PROTOCOL_VERSION: &str = "0.1";

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunnerClientMessage {
    #[serde(rename = "runner_hello")]
    Hello(RunnerHello),
    #[serde(rename = "runner_heartbeat")]
    Heartbeat(RunnerHeartbeat),
    #[serde(rename = "runner_response")]
    Response(RunnerResponseEnvelope),
    #[serde(rename = "runner_event")]
    Event(Box<RunnerEventEnvelope>),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunnerServerMessage {
    #[serde(rename = "runner_command")]
    Command(Box<RunnerCommandEnvelope>),
    #[serde(rename = "runner_heartbeat_ack")]
    HeartbeatAck(RunnerHeartbeatAck),
    #[serde(rename = "runner_event_ack")]
    EventAck(RunnerEventAck),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RunnerHello {
    pub runner_id: agenter_core::RunnerId,
    pub protocol_version: String,
    pub token: String,
    pub capabilities: RunnerCapabilities,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acked_runner_event_seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay_from_runner_event_seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspaces: Vec<WorkspaceRef>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunnerCapabilities {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agent_providers: Vec<AgentProviderAdvertisement>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transports: Vec<String>,
    pub workspace_discovery: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentProviderAdvertisement {
    pub provider_id: AgentProviderId,
    pub capabilities: AgentCapabilities,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RunnerHeartbeat {
    pub sequence: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspaces: Vec<WorkspaceRef>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunnerHeartbeatAck {
    pub sequence: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunnerEventAck {
    pub runner_event_seq: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RunnerCommandEnvelope {
    pub request_id: RequestId,
    pub command: RunnerCommand,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunnerCommand {
    CreateSession(CreateSessionCommand),
    ResumeSession(ResumeSessionCommand),
    RefreshSessions(RefreshSessionsCommand),
    ListProviderCommands(ListProviderCommandsCommand),
    GetAgentOptions(GetAgentOptionsCommand),
    AgentSendInput(AgentInputCommand),
    ExecuteProviderCommand(ProviderCommandExecutionCommand),
    InterruptSession { session_id: SessionId },
    AnswerApproval(ApprovalAnswerCommand),
    AnswerQuestion(QuestionAnswerCommand),
    ShutdownSession(ShutdownSessionCommand),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CreateSessionCommand {
    pub session_id: SessionId,
    pub workspace: WorkspaceRef,
    pub provider_id: AgentProviderId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_input: Option<AgentInput>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResumeSessionCommand {
    pub session_id: SessionId,
    pub workspace: WorkspaceRef,
    pub provider_id: AgentProviderId,
    pub external_session_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RefreshSessionsCommand {
    pub workspace: WorkspaceRef,
    pub provider_id: AgentProviderId,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ListProviderCommandsCommand {
    pub session_id: SessionId,
    pub provider_id: AgentProviderId,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AgentInputCommand {
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<AgentProviderId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<AgentTurnSettings>,
    pub input: AgentInput,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GetAgentOptionsCommand {
    pub session_id: SessionId,
    pub provider_id: AgentProviderId,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ProviderCommandExecutionCommand {
    pub session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_session_id: Option<String>,
    pub provider_id: AgentProviderId,
    pub command: SlashCommandRequest,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentInput {
    Text { text: String },
    UserMessage { payload: UserMessageEvent },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ApprovalAnswerCommand {
    pub session_id: SessionId,
    pub approval_id: ApprovalId,
    pub decision: ApprovalDecision,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QuestionAnswerCommand {
    pub session_id: SessionId,
    pub answer: AgentQuestionAnswer,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ShutdownSessionCommand {
    pub session_id: SessionId,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RunnerResponseEnvelope {
    pub request_id: RequestId,
    #[serde(flatten)]
    pub outcome: RunnerResponseOutcome,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RunnerResponseOutcome {
    Ok { result: RunnerCommandResult },
    Error { error: RunnerError },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunnerCommandResult {
    Accepted,
    AgentOptions {
        options: AgentOptions,
    },
    ProviderCommands {
        commands: Vec<SlashCommandDefinition>,
    },
    ProviderCommandExecuted {
        result: SlashCommandResult,
    },
    SessionCreated {
        session_id: SessionId,
        external_session_id: String,
    },
    SessionResumed {
        session_id: SessionId,
        external_session_id: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunnerError {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RunnerEventEnvelope {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<RequestId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner_event_seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acked_runner_event_seq: Option<u64>,
    pub event: RunnerEvent,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunnerEvent {
    AgentEvent(Box<AgentUniversalEvent>),
    HealthChanged(RunnerHealthChanged),
    OperationUpdated(RunnerOperationUpdate),
    SessionsDiscovered(DiscoveredSessions),
    Error(RunnerError),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RunnerOperationUpdate {
    pub operation_id: RequestId,
    pub kind: RunnerOperationKind,
    pub status: RunnerOperationStatus,
    pub stage_label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<RunnerOperationProgress>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub level: RunnerOperationLogLevel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ts: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunnerOperationKind {
    SessionRefresh,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunnerOperationStatus {
    Queued,
    Accepted,
    Discovering,
    ReadingHistory,
    SendingResults,
    Importing,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunnerOperationProgress {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percent: Option<u8>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunnerOperationLogLevel {
    Debug,
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AgentUniversalEvent {
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<ItemId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ts: Option<DateTime<Utc>>,
    pub source: UniversalEventSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native: Option<NativeRef>,
    pub event: UniversalEventKind,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DiscoveredSessions {
    pub workspace: WorkspaceRef,
    pub provider_id: AgentProviderId,
    pub sessions: Vec<DiscoveredSession>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DiscoveredSession {
    pub external_session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub history_status: DiscoveredSessionHistoryStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<DiscoveredSessionHistoryItem>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DiscoveredSessionHistoryStatus {
    #[default]
    Loaded,
    NotLoaded,
    Failed {
        message: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DiscoveredSessionHistoryItem {
    UserMessage {
        #[serde(skip_serializing_if = "Option::is_none")]
        message_id: Option<String>,
        content: String,
    },
    AgentMessage {
        message_id: String,
        content: String,
    },
    Plan {
        plan_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_payload: Option<Value>,
    },
    Tool {
        tool_call_id: String,
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        status: DiscoveredToolStatus,
        #[serde(skip_serializing_if = "Option::is_none")]
        input: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        output: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_payload: Option<Value>,
    },
    Command {
        command_id: String,
        command: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        source: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        process_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        actions: Vec<DiscoveredCommandAction>,
        #[serde(skip_serializing_if = "Option::is_none")]
        output: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
        success: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_payload: Option<Value>,
    },
    FileChange {
        change_id: String,
        path: String,
        change_kind: FileChangeKind,
        status: DiscoveredFileChangeStatus,
        #[serde(skip_serializing_if = "Option::is_none")]
        diff: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_payload: Option<Value>,
    },
    NativeNotification {
        #[serde(skip_serializing_if = "Option::is_none")]
        event_id: Option<String>,
        category: String,
        title: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_payload: Option<Value>,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DiscoveredCommandAction {
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
    pub provider_payload: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveredToolStatus {
    Running,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveredFileChangeStatus {
    Proposed,
    Applied,
    Rejected,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunnerHealthChanged {
    pub status: RunnerHealthStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunnerHealthStatus {
    Ready,
    Degraded,
    Draining,
}

#[cfg(test)]
mod tests {
    use agenter_core::{
        AgentCapabilities, AgentProviderId, AgentQuestionAnswer, AgentReasoningEffort,
        AgentTurnSettings, ApprovalDecision, ApprovalId, QuestionId, RunnerId, SessionId,
        UserMessageEvent, WorkspaceId, WorkspaceRef,
    };

    use super::*;

    #[test]
    fn round_trips_runner_hello() {
        let message = RunnerClientMessage::Hello(RunnerHello {
            runner_id: RunnerId::nil(),
            protocol_version: PROTOCOL_VERSION.to_owned(),
            token: "runner-token".to_owned(),
            capabilities: RunnerCapabilities {
                agent_providers: vec![AgentProviderAdvertisement {
                    provider_id: AgentProviderId::from(AgentProviderId::CODEX),
                    capabilities: AgentCapabilities {
                        streaming: true,
                        approvals: true,
                        ..AgentCapabilities::default()
                    },
                }],
                transports: vec!["stdio".to_owned()],
                workspace_discovery: false,
            },
            acked_runner_event_seq: Some(40),
            replay_from_runner_event_seq: Some(41),
            workspaces: vec![WorkspaceRef {
                workspace_id: WorkspaceId::nil(),
                runner_id: RunnerId::nil(),
                path: "/work/agenter".to_owned(),
                display_name: Some("agenter".to_owned()),
            }],
        });

        let json = serde_json::to_value(&message).expect("serialize hello");
        let decoded: RunnerClientMessage =
            serde_json::from_value(json.clone()).expect("deserialize hello");

        assert_eq!(json["type"], "runner_hello");
        assert_eq!(json["runner_id"], RunnerId::nil().to_string());
        assert_eq!(json["protocol_version"], PROTOCOL_VERSION);
        assert_eq!(json["acked_runner_event_seq"], 40);
        assert_eq!(json["replay_from_runner_event_seq"], 41);
        assert_eq!(
            json["capabilities"]["agent_providers"][0]["provider_id"],
            "codex"
        );
        assert_eq!(decoded, message);
    }

    #[test]
    fn round_trips_agent_input_command_with_request_id() {
        let message = RunnerServerMessage::Command(Box::new(RunnerCommandEnvelope {
            request_id: RequestId::from("req-1"),
            command: RunnerCommand::AgentSendInput(AgentInputCommand {
                session_id: SessionId::nil(),
                provider_id: Some(AgentProviderId::from(AgentProviderId::CODEX)),
                external_session_id: Some("thread-1".to_owned()),
                settings: None,
                input: AgentInput::Text {
                    text: "Run tests".to_owned(),
                },
            }),
        }));

        let json = serde_json::to_value(&message).expect("serialize command");
        let decoded: RunnerServerMessage =
            serde_json::from_value(json.clone()).expect("deserialize command");

        assert_eq!(json["type"], "runner_command");
        assert_eq!(json["request_id"], "req-1");
        assert_eq!(json["command"]["type"], "agent_send_input");
        assert_eq!(json["command"]["input"]["type"], "text");
        assert_eq!(decoded, message);
    }

    #[test]
    fn round_trips_approval_answer_command_with_request_id() {
        let message = RunnerServerMessage::Command(Box::new(RunnerCommandEnvelope {
            request_id: RequestId::from("req-2"),
            command: RunnerCommand::AnswerApproval(ApprovalAnswerCommand {
                session_id: SessionId::nil(),
                approval_id: ApprovalId::nil(),
                decision: ApprovalDecision::AcceptForSession,
            }),
        }));

        let json = serde_json::to_value(&message).expect("serialize approval answer");
        let decoded: RunnerServerMessage =
            serde_json::from_value(json.clone()).expect("deserialize approval answer");

        assert_eq!(json["type"], "runner_command");
        assert_eq!(json["command"]["type"], "answer_approval");
        assert_eq!(
            json["command"]["decision"]["decision"],
            "accept_for_session"
        );
        assert_eq!(decoded, message);
    }

    #[test]
    fn round_trips_refresh_sessions_command() {
        let workspace = WorkspaceRef {
            workspace_id: WorkspaceId::nil(),
            runner_id: RunnerId::nil(),
            path: "/work/agenter".to_owned(),
            display_name: Some("agenter".to_owned()),
        };
        let message = RunnerServerMessage::Command(Box::new(RunnerCommandEnvelope {
            request_id: RequestId::from("refresh-1"),
            command: RunnerCommand::RefreshSessions(RefreshSessionsCommand {
                workspace: workspace.clone(),
                provider_id: AgentProviderId::from(AgentProviderId::CODEX),
            }),
        }));

        let json = serde_json::to_value(&message).expect("serialize refresh command");
        let decoded: RunnerServerMessage =
            serde_json::from_value(json.clone()).expect("deserialize refresh command");

        assert_eq!(json["type"], "runner_command");
        assert_eq!(json["command"]["type"], "refresh_sessions");
        assert_eq!(json["command"]["provider_id"], AgentProviderId::CODEX);
        assert_eq!(json["command"]["workspace"]["path"], workspace.path);
        assert_eq!(decoded, message);
    }

    #[test]
    fn round_trips_provider_command_manifest_and_execution() {
        let manifest = RunnerServerMessage::Command(Box::new(RunnerCommandEnvelope {
            request_id: RequestId::from("provider-commands-1"),
            command: RunnerCommand::ListProviderCommands(ListProviderCommandsCommand {
                session_id: SessionId::nil(),
                provider_id: AgentProviderId::from(AgentProviderId::CODEX),
            }),
        }));
        let manifest_json = serde_json::to_value(&manifest).expect("serialize manifest request");
        let decoded_manifest: RunnerServerMessage =
            serde_json::from_value(manifest_json.clone()).expect("deserialize manifest request");
        assert_eq!(manifest_json["command"]["type"], "list_provider_commands");
        assert_eq!(decoded_manifest, manifest);

        let execution = RunnerServerMessage::Command(Box::new(RunnerCommandEnvelope {
            request_id: RequestId::from("provider-exec-1"),
            command: RunnerCommand::ExecuteProviderCommand(ProviderCommandExecutionCommand {
                session_id: SessionId::nil(),
                external_session_id: Some("thread-1".to_owned()),
                provider_id: AgentProviderId::from(AgentProviderId::CODEX),
                command: agenter_core::SlashCommandRequest {
                    command_id: "codex.compact".to_owned(),
                    universal_command_id: None,
                    idempotency_key: None,
                    arguments: serde_json::json!({}),
                    raw_input: "/compact".to_owned(),
                    confirmed: false,
                },
            }),
        }));
        let execution_json = serde_json::to_value(&execution).expect("serialize execution request");
        let decoded_execution: RunnerServerMessage =
            serde_json::from_value(execution_json.clone()).expect("deserialize execution request");
        assert_eq!(
            execution_json["command"]["type"],
            "execute_provider_command"
        );
        assert_eq!(decoded_execution, execution);

        let result = RunnerResponseOutcome::Ok {
            result: RunnerCommandResult::ProviderCommandExecuted {
                result: agenter_core::SlashCommandResult {
                    accepted: true,
                    message: "Compaction started.".to_owned(),
                    session: None,
                    provider_payload: None,
                },
            },
        };
        let result_json = serde_json::to_value(&result).expect("serialize result");
        let decoded_result: RunnerResponseOutcome =
            serde_json::from_value(result_json).expect("deserialize result");
        assert_eq!(decoded_result, result);
    }

    #[test]
    fn round_trips_agent_event() {
        let message = RunnerClientMessage::Event(Box::new(RunnerEventEnvelope {
            request_id: Some(RequestId::from("event-1")),
            runner_event_seq: Some(123),
            acked_runner_event_seq: Some(122),
            event: RunnerEvent::AgentEvent(Box::new(AgentUniversalEvent {
                session_id: SessionId::nil(),
                event_id: Some("11111111-1111-1111-1111-111111111111".to_owned()),
                turn_id: None,
                item_id: None,
                ts: None,
                source: UniversalEventSource::Native,
                native: Some(NativeRef {
                    protocol: "codex-app-server".to_owned(),
                    method: Some("thread/item".to_owned()),
                    kind: Some("codex".to_owned()),
                    native_id: Some("native-msg-1".to_owned()),
                    summary: Some("native message".to_owned()),
                    hash: None,
                    pointer: None,
                }),
                event: UniversalEventKind::NativeUnknown {
                    summary: Some("native message".to_owned()),
                },
            })),
        }));

        let json = serde_json::to_value(&message).expect("serialize event");
        let decoded: RunnerClientMessage =
            serde_json::from_value(json.clone()).expect("deserialize event");

        assert_eq!(json["type"], "runner_event");
        assert_eq!(json["request_id"], "event-1");
        assert_eq!(json["runner_event_seq"], 123);
        assert_eq!(json["acked_runner_event_seq"], 122);
        assert_eq!(json["event"]["type"], "agent_event");
        assert_eq!(json["event"]["native"]["protocol"], "codex-app-server");
        assert_eq!(json["event"]["event"]["type"], "native.unknown");
        assert_eq!(decoded, message);
    }

    #[test]
    fn round_trips_runner_event_ack() {
        let message = RunnerServerMessage::EventAck(RunnerEventAck {
            runner_event_seq: 123,
        });

        let json = serde_json::to_value(&message).expect("serialize ack");
        let decoded: RunnerServerMessage =
            serde_json::from_value(json.clone()).expect("deserialize ack");

        assert_eq!(json["type"], "runner_event_ack");
        assert_eq!(json["runner_event_seq"], 123);
        assert_eq!(decoded, message);
    }

    #[test]
    fn round_trips_discovered_sessions_event() {
        let workspace = WorkspaceRef {
            workspace_id: WorkspaceId::nil(),
            runner_id: RunnerId::nil(),
            path: "/work/agenter".to_owned(),
            display_name: Some("agenter".to_owned()),
        };
        let message = RunnerClientMessage::Event(Box::new(RunnerEventEnvelope {
            request_id: None,
            runner_event_seq: None,
            acked_runner_event_seq: None,
            event: RunnerEvent::SessionsDiscovered(DiscoveredSessions {
                workspace: workspace.clone(),
                provider_id: AgentProviderId::from(AgentProviderId::CODEX),
                sessions: vec![DiscoveredSession {
                    external_session_id: "codex-thread-1".to_owned(),
                    title: Some("Existing Codex Thread".to_owned()),
                    updated_at: None,
                    history_status: DiscoveredSessionHistoryStatus::Loaded,
                    history: vec![
                        DiscoveredSessionHistoryItem::UserMessage {
                            message_id: Some("user-1".to_owned()),
                            content: "hello".to_owned(),
                        },
                        DiscoveredSessionHistoryItem::AgentMessage {
                            message_id: "agent-1".to_owned(),
                            content: "hi".to_owned(),
                        },
                    ],
                }],
            }),
        }));

        let json = serde_json::to_value(&message).expect("serialize discovered sessions");
        let decoded: RunnerClientMessage =
            serde_json::from_value(json.clone()).expect("deserialize discovered sessions");

        assert_eq!(json["type"], "runner_event");
        assert_eq!(json["event"]["type"], "sessions_discovered");
        assert_eq!(json["event"]["provider_id"], AgentProviderId::CODEX);
        assert_eq!(json["event"]["workspace"]["path"], workspace.path);
        assert_eq!(
            json["event"]["sessions"][0]["external_session_id"],
            "codex-thread-1"
        );
        assert_eq!(
            json["event"]["sessions"][0]["history"][0]["type"],
            "user_message"
        );
        assert_eq!(decoded, message);
    }

    #[test]
    fn round_trips_discovered_session_history_failure() {
        let workspace = WorkspaceRef {
            workspace_id: WorkspaceId::nil(),
            runner_id: RunnerId::nil(),
            path: "/work/agenter".to_owned(),
            display_name: None,
        };
        let message = RunnerClientMessage::Event(Box::new(RunnerEventEnvelope {
            request_id: Some(RequestId::from("refresh-1")),
            runner_event_seq: None,
            acked_runner_event_seq: None,
            event: RunnerEvent::SessionsDiscovered(DiscoveredSessions {
                workspace,
                provider_id: AgentProviderId::from(AgentProviderId::CODEX),
                sessions: vec![DiscoveredSession {
                    external_session_id: "codex-thread-failed".to_owned(),
                    title: None,
                    updated_at: None,
                    history_status: DiscoveredSessionHistoryStatus::Failed {
                        message: "thread/read failed".to_owned(),
                    },
                    history: Vec::new(),
                }],
            }),
        }));

        let json = serde_json::to_value(&message).expect("serialize discovered sessions");
        let decoded: RunnerClientMessage =
            serde_json::from_value(json.clone()).expect("deserialize discovered sessions");

        assert_eq!(
            json["event"]["sessions"][0]["history_status"]["status"],
            "failed"
        );
        assert_eq!(
            json["event"]["sessions"][0]["history_status"]["message"],
            "thread/read failed"
        );
        assert_eq!(decoded, message);
    }

    #[test]
    fn round_trips_operation_updated_event() {
        let message = RunnerClientMessage::Event(Box::new(RunnerEventEnvelope {
            request_id: Some(RequestId::from("refresh-1")),
            runner_event_seq: Some(7),
            acked_runner_event_seq: None,
            event: RunnerEvent::OperationUpdated(RunnerOperationUpdate {
                operation_id: RequestId::from("refresh-1"),
                kind: RunnerOperationKind::SessionRefresh,
                status: RunnerOperationStatus::ReadingHistory,
                stage_label: "Reading Codex history".to_owned(),
                progress: Some(RunnerOperationProgress {
                    current: Some(2),
                    total: Some(5),
                    percent: Some(40),
                }),
                message: Some("Read 2 of 5 sessions".to_owned()),
                level: RunnerOperationLogLevel::Info,
                ts: Some(Utc::now()),
            }),
        }));

        let json = serde_json::to_value(&message).expect("serialize operation update");
        let decoded: RunnerClientMessage =
            serde_json::from_value(json.clone()).expect("deserialize operation update");

        assert_eq!(json["event"]["type"], "operation_updated");
        assert_eq!(json["event"]["kind"], "session_refresh");
        assert_eq!(json["event"]["status"], "reading_history");
        assert_eq!(json["event"]["progress"]["current"], 2);
        assert_eq!(json["event"]["progress"]["total"], 5);
        assert_eq!(json["event"]["progress"]["percent"], 40);
        assert_eq!(decoded, message);
    }

    #[test]
    fn agent_input_command_supports_structured_user_message_payload() {
        let command = RunnerCommand::AgentSendInput(AgentInputCommand {
            session_id: SessionId::nil(),
            provider_id: Some(AgentProviderId::from(AgentProviderId::CODEX)),
            external_session_id: None,
            settings: None,
            input: AgentInput::UserMessage {
                payload: UserMessageEvent {
                    session_id: SessionId::nil(),
                    message_id: Some("user-msg-1".to_owned()),
                    author_user_id: None,
                    content: "hello".to_owned(),
                },
            },
        });

        let json = serde_json::to_value(command).expect("serialize structured input");

        assert_eq!(json["type"], "agent_send_input");
        assert_eq!(json["input"]["type"], "user_message");
        assert_eq!(json["input"]["payload"]["content"], "hello");
    }

    #[test]
    fn round_trips_agent_input_settings_without_breaking_absent_settings() {
        let with_settings = RunnerCommand::AgentSendInput(AgentInputCommand {
            session_id: SessionId::nil(),
            provider_id: Some(AgentProviderId::from(AgentProviderId::CODEX)),
            external_session_id: Some("thread-1".to_owned()),
            settings: Some(AgentTurnSettings {
                model: Some("gpt-5.4".to_owned()),
                reasoning_effort: Some(AgentReasoningEffort::High),
                collaboration_mode: Some("plan".to_owned()),
            }),
            input: AgentInput::Text {
                text: "Plan this".to_owned(),
            },
        });
        let without_settings = serde_json::json!({
            "type": "agent_send_input",
            "session_id": SessionId::nil(),
            "external_session_id": "thread-1",
            "input": {"type": "text", "text": "hello"}
        });

        let json = serde_json::to_value(&with_settings).expect("serialize settings");
        let decoded_old: RunnerCommand =
            serde_json::from_value(without_settings).expect("deserialize old command");

        assert_eq!(json["settings"]["model"], "gpt-5.4");
        assert_eq!(json["settings"]["reasoning_effort"], "high");
        assert_eq!(json["settings"]["collaboration_mode"], "plan");
        match decoded_old {
            RunnerCommand::AgentSendInput(command) => assert_eq!(command.settings, None),
            other => panic!("unexpected command {other:?}"),
        }
    }

    #[test]
    fn round_trips_provider_options_and_question_answer_commands() {
        let options = RunnerServerMessage::Command(Box::new(RunnerCommandEnvelope {
            request_id: RequestId::from("options-1"),
            command: RunnerCommand::GetAgentOptions(GetAgentOptionsCommand {
                session_id: SessionId::nil(),
                provider_id: AgentProviderId::from(AgentProviderId::CODEX),
            }),
        }));
        let answer = RunnerServerMessage::Command(Box::new(RunnerCommandEnvelope {
            request_id: RequestId::from("question-1"),
            command: RunnerCommand::AnswerQuestion(QuestionAnswerCommand {
                session_id: SessionId::nil(),
                answer: AgentQuestionAnswer {
                    question_id: QuestionId::nil(),
                    answers: std::collections::BTreeMap::from([(
                        "targets".to_owned(),
                        vec!["web".to_owned(), "runner".to_owned()],
                    )]),
                },
            }),
        }));

        let options_json = serde_json::to_value(&options).expect("serialize options command");
        let answer_json = serde_json::to_value(&answer).expect("serialize answer command");
        let decoded_answer: RunnerServerMessage =
            serde_json::from_value(answer_json.clone()).expect("decode answer command");

        assert_eq!(options_json["command"]["type"], "get_agent_options");
        assert_eq!(answer_json["command"]["type"], "answer_question");
        assert_eq!(
            answer_json["command"]["answer"]["answers"]["targets"][1],
            "runner"
        );
        assert_eq!(decoded_answer, answer);
    }
}

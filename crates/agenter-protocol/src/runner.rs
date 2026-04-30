use agenter_core::{
    AgentCapabilities, AgentProviderId, AppEvent, ApprovalDecision, ApprovalId, SessionId,
    UserMessageEvent, WorkspaceRef,
};
use serde::{Deserialize, Serialize};

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
    Event(RunnerEventEnvelope),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunnerServerMessage {
    #[serde(rename = "runner_command")]
    Command(Box<RunnerCommandEnvelope>),
    #[serde(rename = "runner_heartbeat_ack")]
    HeartbeatAck(RunnerHeartbeatAck),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RunnerHello {
    pub runner_id: agenter_core::RunnerId,
    pub protocol_version: String,
    pub token: String,
    pub capabilities: RunnerCapabilities,
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
    AgentSendInput(AgentInputCommand),
    InterruptSession { session_id: SessionId },
    AnswerApproval(ApprovalAnswerCommand),
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

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AgentInputCommand {
    pub session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_session_id: Option<String>,
    pub input: AgentInput,
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
    pub event: RunnerEvent,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunnerEvent {
    AgentEvent(AgentEvent),
    HealthChanged(RunnerHealthChanged),
    Error(RunnerError),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AgentEvent {
    pub session_id: SessionId,
    pub event: AppEvent,
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
        AgentCapabilities, AgentMessageDeltaEvent, AgentProviderId, AppEvent, ApprovalDecision,
        ApprovalId, RunnerId, SessionId, UserMessageEvent, WorkspaceId, WorkspaceRef,
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
                external_session_id: Some("thread-1".to_owned()),
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
    fn round_trips_agent_event() {
        let message = RunnerClientMessage::Event(RunnerEventEnvelope {
            request_id: Some(RequestId::from("event-1")),
            event: RunnerEvent::AgentEvent(AgentEvent {
                session_id: SessionId::nil(),
                event: AppEvent::AgentMessageDelta(AgentMessageDeltaEvent {
                    session_id: SessionId::nil(),
                    message_id: "msg-1".to_owned(),
                    delta: "hello".to_owned(),
                    provider_payload: None,
                }),
            }),
        });

        let json = serde_json::to_value(&message).expect("serialize event");
        let decoded: RunnerClientMessage =
            serde_json::from_value(json.clone()).expect("deserialize event");

        assert_eq!(json["type"], "runner_event");
        assert_eq!(json["request_id"], "event-1");
        assert_eq!(json["event"]["type"], "agent_event");
        assert_eq!(json["event"]["event"]["type"], "agent_message_delta");
        assert_eq!(decoded, message);
    }

    #[test]
    fn agent_input_command_supports_structured_user_message_payload() {
        let command = RunnerCommand::AgentSendInput(AgentInputCommand {
            session_id: SessionId::nil(),
            external_session_id: None,
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
}

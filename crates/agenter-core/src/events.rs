use serde::{Deserialize, Serialize};

use crate::{
    AgentProviderId, ApprovalRequestEvent, ApprovalResolvedEvent, QuestionAnsweredEvent,
    QuestionRequestedEvent, SessionId, SessionInfo, SessionStatus, UserId,
};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum AppEvent {
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
    ProviderEvent(ProviderEvent),
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
pub struct ProviderEvent {
    pub session_id: SessionId,
    pub provider_id: AgentProviderId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
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

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use chrono::{TimeZone, Utc};

    use crate::{
        AgentCapabilities, AgentCollaborationMode, AgentModelOption, AgentProviderId,
        AgentQuestionAnswer, AgentQuestionChoice, AgentQuestionField, AgentReasoningEffort,
        AgentTurnSettings, AppEvent, ApprovalDecision, ApprovalId, ApprovalKind,
        ApprovalRequestEvent, ApprovalResolvedEvent, CommandEvent, QuestionId,
        QuestionRequestedEvent, RunnerId, SessionId, SessionInfo, SessionStatus, UserId,
        UserMessageEvent, WorkspaceId,
    };

    #[test]
    fn serializes_user_message_event_with_adjacent_tag() {
        let event = AppEvent::UserMessage(UserMessageEvent {
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
        let event = AppEvent::CommandStarted(CommandEvent {
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
    fn round_trips_provider_event_with_raw_payload() {
        let event = AppEvent::ProviderEvent(crate::ProviderEvent {
            session_id: SessionId::nil(),
            provider_id: AgentProviderId::from(AgentProviderId::CODEX),
            event_id: Some("compact-1".to_owned()),
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
        let decoded: AppEvent = serde_json::from_value(json.clone()).expect("deserialize event");

        assert_eq!(json["type"], "provider_event");
        assert_eq!(json["payload"]["provider_id"], AgentProviderId::CODEX);
        assert_eq!(json["payload"]["category"], "compaction");
        assert_eq!(json["payload"]["status"], "completed");
        assert_eq!(decoded, event);
    }

    #[test]
    fn serializes_plan_delta_marker_only_when_append_is_true() {
        let event = AppEvent::PlanUpdated(crate::PlanEvent {
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
    fn serializes_approval_requested_event_with_uuid_ids() {
        let approval_id = ApprovalId::nil();
        let event = AppEvent::ApprovalRequested(ApprovalRequestEvent {
            session_id: SessionId::nil(),
            approval_id,
            kind: ApprovalKind::Command,
            title: "Run command".to_owned(),
            details: Some("cargo test".to_owned()),
            expires_at: None,
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
    }

    #[test]
    fn round_trips_session_started_event_with_provider_context() {
        let event = AppEvent::SessionStarted(SessionInfo {
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
        let decoded: AppEvent = serde_json::from_value(json.clone()).expect("deserialize event");

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
        let event = AppEvent::QuestionRequested(QuestionRequestedEvent {
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
        let decoded: AppEvent = serde_json::from_value(event_json.clone()).expect("decode event");

        assert_eq!(event_json["type"], "question_requested");
        assert_eq!(event_json["payload"]["fields"][0]["kind"], "multi_select");
        assert_eq!(answer_json["answers"]["targets"][1], "runner");
        assert_eq!(decoded, event);
    }

    #[test]
    fn round_trips_approval_resolved_with_provider_specific_decision() {
        let event = AppEvent::ApprovalResolved(ApprovalResolvedEvent {
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
        let decoded: AppEvent = serde_json::from_value(json.clone()).expect("deserialize event");

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
}

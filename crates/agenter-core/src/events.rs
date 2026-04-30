use serde::{Deserialize, Serialize};

use crate::{
    ApprovalRequestEvent, ApprovalResolvedEvent, SessionId, SessionInfo, SessionStatus, UserId,
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
    pub entries: Vec<PlanEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_payload: Option<serde_json::Value>,
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
        AgentCapabilities, AgentProviderId, AppEvent, ApprovalDecision, ApprovalId, ApprovalKind,
        ApprovalRequestEvent, ApprovalResolvedEvent, CommandEvent, RunnerId, SessionId,
        SessionInfo, SessionStatus, UserId, UserMessageEvent, WorkspaceId,
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
            provider_payload: None,
        });

        let json = serde_json::to_value(event).expect("serialize event");

        assert_eq!(json["type"], "command_started");
        assert_eq!(json["payload"]["command_id"], "cmd-1");
        assert_eq!(json["payload"]["cwd"], "/work/agenter");
        assert!(json["payload"].get("provider_payload").is_none());
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
        };
        let capabilities_json = serde_json::to_value(capabilities).expect("serialize caps");
        assert_eq!(capabilities_json["session_history"], true);
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

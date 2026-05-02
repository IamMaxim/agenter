//! Protocol types shared by Agenter services and clients.

pub mod browser;
pub mod runner;
pub mod runner_transport;

use serde::{Deserialize, Serialize};

pub use browser::{BrowserClientMessage, BrowserEventEnvelope, BrowserServerMessage};
pub use runner::{
    AgentEvent, AgentInput, AgentInputCommand, AgentProviderAdvertisement, ApprovalAnswerCommand,
    CreateSessionCommand, DiscoveredCommandAction, DiscoveredFileChangeStatus, DiscoveredSession,
    DiscoveredSessionHistoryItem, DiscoveredSessionHistoryStatus, DiscoveredSessions,
    DiscoveredToolStatus, ListProviderCommandsCommand, ProviderCommandExecutionCommand,
    RefreshSessionsCommand, ResumeSessionCommand, RunnerCapabilities, RunnerClientMessage,
    RunnerCommand, RunnerCommandEnvelope, RunnerCommandResult, RunnerError, RunnerEvent,
    RunnerEventEnvelope, RunnerHeartbeat, RunnerHeartbeatAck, RunnerHello, RunnerResponseEnvelope,
    RunnerResponseOutcome, RunnerServerMessage, ShutdownSessionCommand,
};
pub use runner_transport::{
    chunk_message, reassemble_message, RunnerTransportChunkFrame, RunnerTransportChunkReassembler,
    RunnerTransportOutboundFrame,
};

macro_rules! string_id {
    ($name:ident) => {
        #[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            #[must_use]
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self::new(value)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self::new(value)
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

string_id!(RequestId);
string_id!(EventId);

#[cfg(test)]
mod runner_transport_tests {
    use agenter_core::{
        AgentProviderId, FileChangeKind, RunnerId, SessionId, WorkspaceId, WorkspaceRef,
    };
    use serde::{de::DeserializeOwned, Serialize};

    use crate::{
        runner::{
            DiscoveredFileChangeStatus, DiscoveredSession, DiscoveredSessionHistoryItem,
            DiscoveredSessionHistoryStatus, DiscoveredSessions, RunnerClientMessage, RunnerEvent,
            RunnerEventEnvelope, RunnerServerMessage,
        },
        runner_transport::{
            chunk_message, reassemble_message, RunnerTransportChunkReassembler,
            RunnerTransportOutboundFrame,
        },
        RequestId,
    };

    fn oversized_discovered_sessions_message(bytes: usize) -> RunnerClientMessage {
        RunnerClientMessage::Event(RunnerEventEnvelope {
            request_id: None,
            event: RunnerEvent::SessionsDiscovered(DiscoveredSessions {
                workspace: WorkspaceRef {
                    workspace_id: WorkspaceId::nil(),
                    runner_id: RunnerId::nil(),
                    path: "/work/agenter".to_owned(),
                    display_name: Some("agenter".to_owned()),
                },
                provider_id: AgentProviderId::from(AgentProviderId::CODEX),
                sessions: vec![DiscoveredSession {
                    external_session_id: "thread-large".to_owned(),
                    title: None,
                    updated_at: None,
                    history_status: DiscoveredSessionHistoryStatus::Loaded,
                    history: vec![DiscoveredSessionHistoryItem::FileChange {
                        change_id: "change-1".to_owned(),
                        path: "huge.patch".to_owned(),
                        change_kind: FileChangeKind::Modify,
                        status: DiscoveredFileChangeStatus::Proposed,
                        diff: Some("x".repeat(bytes)),
                        provider_payload: Some(serde_json::json!({
                            "raw": "y".repeat(bytes)
                        })),
                    }],
                }],
            }),
        })
    }

    fn frame_json_len(frame: &RunnerTransportOutboundFrame) -> usize {
        match frame {
            RunnerTransportOutboundFrame::Text(text) => text.len(),
        }
    }

    fn round_trip<T>(frames: Vec<RunnerTransportOutboundFrame>, max_bytes: usize) -> T
    where
        T: DeserializeOwned,
    {
        let mut reassembler = RunnerTransportChunkReassembler::new(max_bytes);
        for frame in frames {
            let RunnerTransportOutboundFrame::Text(text) = frame;
            if let Some(message) =
                reassemble_message::<T>(&mut reassembler, &text).expect("reassemble frame")
            {
                return message;
            }
        }
        panic!("chunked message did not complete");
    }

    #[test]
    fn chunked_runner_client_message_round_trips_without_truncating_payloads() {
        let message = oversized_discovered_sessions_message(96 * 1024);
        let expected_json = serde_json::to_string(&message).expect("serialize original");

        let frames = chunk_message(&message, 8 * 1024).expect("chunk message");

        assert!(frames.len() > 3);
        assert!(frames.iter().all(|frame| frame_json_len(frame) < 16 * 1024));

        let decoded: RunnerClientMessage = round_trip(frames, 4 * 1024 * 1024);
        let decoded_json = serde_json::to_string(&decoded).expect("serialize decoded");

        assert_eq!(decoded_json, expected_json);
        assert!(!decoded_json.contains("agenter_truncated"));
    }

    #[test]
    fn reassembler_rejects_corrupt_chunk_digest() {
        let message = oversized_discovered_sessions_message(32 * 1024);
        let mut frames = chunk_message(&message, 8 * 1024).expect("chunk message");
        let RunnerTransportOutboundFrame::Text(data) = frames.get_mut(1).expect("first data frame");
        *data = data.replacen("eHh4", "eHl4", 1);

        let mut reassembler = RunnerTransportChunkReassembler::new(4 * 1024 * 1024);
        let mut error = None;
        for frame in frames {
            let RunnerTransportOutboundFrame::Text(text) = frame;
            if let Err(next_error) =
                reassemble_message::<RunnerClientMessage>(&mut reassembler, &text)
            {
                error = Some(next_error);
                break;
            }
        }

        assert!(error
            .expect("corrupt payload should fail")
            .to_string()
            .contains("digest mismatch"));
    }

    #[test]
    fn chunked_runner_server_message_round_trips() {
        let message =
            RunnerServerMessage::Command(Box::new(crate::runner::RunnerCommandEnvelope {
                request_id: RequestId::from("large-command"),
                command: crate::runner::RunnerCommand::AgentSendInput(
                    crate::runner::AgentInputCommand {
                        session_id: SessionId::nil(),
                        external_session_id: Some("thread-large".to_owned()),
                        settings: None,
                        input: crate::runner::AgentInput::Text {
                            text: "z".repeat(96 * 1024),
                        },
                    },
                ),
            }));
        let expected_json = serde_json::to_string(&message).expect("serialize original");

        let frames = chunk_message(&message, 8 * 1024).expect("chunk message");
        let decoded: RunnerServerMessage = round_trip(frames, 4 * 1024 * 1024);
        let decoded_json = serde_json::to_string(&decoded).expect("serialize decoded");

        assert_eq!(decoded_json, expected_json);
    }

    #[test]
    fn reassembler_rejects_missing_chunks() {
        let message = oversized_discovered_sessions_message(32 * 1024);
        let mut frames = chunk_message(&message, 8 * 1024).expect("chunk message");
        frames.remove(1);

        let mut reassembler = RunnerTransportChunkReassembler::new(4 * 1024 * 1024);
        let mut error = None;
        for frame in frames {
            let RunnerTransportOutboundFrame::Text(text) = frame;
            if let Err(next_error) =
                reassemble_message::<RunnerClientMessage>(&mut reassembler, &text)
            {
                error = Some(next_error);
                break;
            }
        }

        assert!(error
            .expect("missing chunk should fail")
            .to_string()
            .contains("incomplete"));
    }

    #[test]
    fn reassembler_rejects_messages_over_limit() {
        let message = oversized_discovered_sessions_message(32 * 1024);
        let frames = chunk_message(&message, 8 * 1024).expect("chunk message");
        let mut reassembler = RunnerTransportChunkReassembler::new(1024);

        let RunnerTransportOutboundFrame::Text(text) = &frames[0];
        let error = reassemble_message::<RunnerClientMessage>(&mut reassembler, text)
            .expect_err("start frame should exceed configured limit");

        assert!(error.to_string().contains("exceeds maximum"));
    }

    #[allow(dead_code)]
    fn assert_serde<T: Serialize + DeserializeOwned>() {}
}

use std::env;

use agenter_core::{
    AgentCapabilities, AgentErrorEvent, AgentMessageDeltaEvent, AgentProviderId, AppEvent,
    ApprovalId, ApprovalKind, ApprovalRequestEvent, CommandCompletedEvent, CommandEvent,
    CommandOutputEvent, CommandOutputStream, FileChangeEvent, FileChangeKind,
    MessageCompletedEvent, RunnerId, SessionId, ToolEvent, UserMessageEvent, WorkspaceId,
    WorkspaceRef,
};
use agenter_protocol::runner::{
    AgentInput, AgentProviderAdvertisement, RunnerCapabilities, RunnerClientMessage, RunnerCommand,
    RunnerCommandResult, RunnerEvent, RunnerEventEnvelope, RunnerHello, RunnerResponseEnvelope,
    RunnerResponseOutcome, PROTOCOL_VERSION,
};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use uuid::Uuid;

const DEFAULT_CONTROL_PLANE_WS: &str = "ws://127.0.0.1:7777/api/runner/ws";
const DEFAULT_DEV_RUNNER_TOKEN: &str = "dev-runner-token";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = tracing_subscriber::fmt::try_init();

    if fake_mode_requested() {
        run_fake_runner().await?;
    } else {
        println!("agenter runner");
    }

    Ok(())
}

fn fake_mode_requested() -> bool {
    env::args().any(|arg| arg == "fake" || arg == "--fake")
        || env::var("AGENTER_RUNNER_MODE").is_ok_and(|mode| mode == "fake")
}

async fn run_fake_runner() -> anyhow::Result<()> {
    let url = env::var("AGENTER_CONTROL_PLANE_WS")
        .unwrap_or_else(|_| DEFAULT_CONTROL_PLANE_WS.to_owned());
    let token = env::var("AGENTER_DEV_RUNNER_TOKEN")
        .unwrap_or_else(|_| DEFAULT_DEV_RUNNER_TOKEN.to_owned());
    let (socket, _) = connect_async(&url).await?;
    let (mut sender, mut receiver) = socket.split();

    send_runner_message(&mut sender, RunnerClientMessage::Hello(fake_hello(token))).await?;

    while let Some(message) = receiver.next().await {
        let Message::Text(text) = message? else {
            continue;
        };
        let Ok(agenter_protocol::RunnerServerMessage::Command(envelope)) =
            serde_json::from_str::<agenter_protocol::RunnerServerMessage>(&text)
        else {
            continue;
        };

        if let RunnerCommand::AgentSendInput(command) = envelope.command {
            send_runner_message(
                &mut sender,
                RunnerClientMessage::Response(RunnerResponseEnvelope {
                    request_id: envelope.request_id.clone(),
                    outcome: RunnerResponseOutcome::Ok {
                        result: RunnerCommandResult::Accepted,
                    },
                }),
            )
            .await?;

            for event in deterministic_fake_events(command.session_id, &command.input) {
                send_runner_message(
                    &mut sender,
                    RunnerClientMessage::Event(RunnerEventEnvelope {
                        request_id: Some(envelope.request_id.clone()),
                        event: RunnerEvent::AgentEvent(agenter_protocol::AgentEvent {
                            session_id: command.session_id,
                            event,
                        }),
                    }),
                )
                .await?;
            }
        }
    }

    Ok(())
}

async fn send_runner_message(
    sender: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    message: RunnerClientMessage,
) -> anyhow::Result<()> {
    sender
        .send(Message::Text(serde_json::to_string(&message)?.into()))
        .await?;
    Ok(())
}

fn fake_hello(token: String) -> RunnerHello {
    let runner_id = fake_runner_id();
    RunnerHello {
        runner_id,
        protocol_version: PROTOCOL_VERSION.to_owned(),
        token,
        capabilities: RunnerCapabilities {
            agent_providers: vec![AgentProviderAdvertisement {
                provider_id: AgentProviderId::from(AgentProviderId::CODEX),
                capabilities: AgentCapabilities {
                    streaming: true,
                    approvals: true,
                    file_changes: true,
                    command_execution: true,
                    ..AgentCapabilities::default()
                },
            }],
            transports: vec!["fake".to_owned()],
            workspace_discovery: false,
        },
        workspaces: vec![WorkspaceRef {
            workspace_id: WorkspaceId::from_uuid(Uuid::from_u128(
                0x22222222222222222222222222222222,
            )),
            runner_id,
            path: env::current_dir()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|_| ".".to_owned()),
            display_name: Some("fake workspace".to_owned()),
        }],
    }
}

fn fake_runner_id() -> RunnerId {
    RunnerId::from_uuid(Uuid::from_u128(0x33333333333333333333333333333333))
}

fn deterministic_fake_events(session_id: SessionId, input: &AgentInput) -> Vec<AppEvent> {
    let content = match input {
        AgentInput::Text { text } => text.clone(),
        AgentInput::UserMessage { payload } => payload.content.clone(),
    };
    let response = format!("fake runner received: {content}");

    vec![
        AppEvent::UserMessage(UserMessageEvent {
            session_id,
            message_id: Some("fake-user-1".to_owned()),
            author_user_id: None,
            content,
        }),
        AppEvent::CommandStarted(CommandEvent {
            session_id,
            command_id: "fake-command-1".to_owned(),
            command: "printf fake-runner".to_owned(),
            cwd: Some(".".to_owned()),
            provider_payload: None,
        }),
        AppEvent::CommandOutputDelta(CommandOutputEvent {
            session_id,
            command_id: "fake-command-1".to_owned(),
            stream: CommandOutputStream::Stdout,
            delta: "fake-runner\n".to_owned(),
            provider_payload: None,
        }),
        AppEvent::CommandCompleted(CommandCompletedEvent {
            session_id,
            command_id: "fake-command-1".to_owned(),
            exit_code: Some(0),
            success: true,
            provider_payload: None,
        }),
        AppEvent::ToolStarted(ToolEvent {
            session_id,
            tool_call_id: "fake-tool-1".to_owned(),
            name: "fake_lookup".to_owned(),
            title: Some("Fake lookup".to_owned()),
            input: Some(serde_json::json!({ "query": response.clone() })),
            output: None,
            provider_payload: None,
        }),
        AppEvent::ToolCompleted(ToolEvent {
            session_id,
            tool_call_id: "fake-tool-1".to_owned(),
            name: "fake_lookup".to_owned(),
            title: Some("Fake lookup".to_owned()),
            input: None,
            output: Some(serde_json::json!({ "ok": true })),
            provider_payload: None,
        }),
        AppEvent::FileChangeProposed(FileChangeEvent {
            session_id,
            path: "fake-output.txt".to_owned(),
            change_kind: FileChangeKind::Modify,
            diff: Some("-old\n+fake runner output\n".to_owned()),
            provider_payload: None,
        }),
        AppEvent::FileChangeApplied(FileChangeEvent {
            session_id,
            path: "fake-output.txt".to_owned(),
            change_kind: FileChangeKind::Modify,
            diff: None,
            provider_payload: None,
        }),
        AppEvent::ApprovalRequested(ApprovalRequestEvent {
            session_id,
            approval_id: ApprovalId::from_uuid(Uuid::from_u128(0x44444444444444444444444444444444)),
            kind: ApprovalKind::Command,
            title: "Approve fake command".to_owned(),
            details: Some("This is an in-memory approval stub.".to_owned()),
            expires_at: None,
            provider_payload: None,
        }),
        AppEvent::AgentMessageDelta(AgentMessageDeltaEvent {
            session_id,
            message_id: "fake-agent-1".to_owned(),
            delta: response.clone(),
            provider_payload: None,
        }),
        AppEvent::AgentMessageCompleted(MessageCompletedEvent {
            session_id,
            message_id: "fake-agent-1".to_owned(),
            content: Some(response),
            provider_payload: None,
        }),
        AppEvent::Error(AgentErrorEvent {
            session_id: Some(session_id),
            code: Some("fake_notice".to_owned()),
            message: "Fake runner diagnostic card".to_owned(),
            provider_payload: None,
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_events_are_deterministic() {
        let session_id = SessionId::nil();
        let events = deterministic_fake_events(
            session_id,
            &AgentInput::Text {
                text: "hello".to_owned(),
            },
        );

        assert_eq!(events.len(), 12);
        assert!(matches!(events[0], AppEvent::UserMessage(_)));
        assert!(matches!(events[1], AppEvent::CommandStarted(_)));
        assert!(matches!(events[9], AppEvent::AgentMessageDelta(_)));
        assert!(matches!(events[10], AppEvent::AgentMessageCompleted(_)));
    }
}

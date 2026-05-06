use std::{env, path::PathBuf};

use agenter_core::{
    AgentProviderId, ApprovalId, ApprovalKind, ApprovalRequest, ApprovalStatus, ContentBlock,
    ContentBlockKind, DiffFile, DiffId, DiffState, FileChangeKind, ItemId, ItemRole, ItemState,
    ItemStatus, NativeRef, RunnerId, SessionId, ToolCommandProjection, ToolProjection,
    ToolProjectionKind, TurnId, TurnState, TurnStatus, UniversalEventKind, WorkspaceId,
    WorkspaceRef,
};
use agenter_protocol::runner::{
    AgentInput, AgentProviderAdvertisement, RunnerCapabilities, RunnerCommand, RunnerCommandResult,
    RunnerEvent, RunnerHello, RunnerResponseOutcome, PROTOCOL_VERSION,
};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::agents::adapter::AdapterEvent;
use crate::runner_host::{
    background_event_channel, run_runner_host, RunnerBackgroundEventSender, RunnerCommandHandler,
    RunnerHostBoxFuture, RunnerHostConfig,
};

pub(crate) async fn run() -> anyhow::Result<()> {
    let url = env::var("AGENTER_CONTROL_PLANE_WS")
        .unwrap_or_else(|_| crate::DEFAULT_CONTROL_PLANE_WS.to_owned());
    let token = env::var("AGENTER_DEV_RUNNER_TOKEN")
        .unwrap_or_else(|_| crate::DEFAULT_DEV_RUNNER_TOKEN.to_owned());
    let workspace_path = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let hello_template = fake_hello(token);
    tracing::info!(url = %url, runner_id = %hello_template.runner_id, "starting fake runner");
    let (_adapter_event_sender, adapter_event_receiver) = mpsc::unbounded_channel::<AdapterEvent>();
    let (background_sender, background_receiver) = background_event_channel();
    run_runner_host(
        RunnerHostConfig::new("fake", url, workspace_path, hello_template),
        adapter_event_receiver,
        background_sender,
        background_receiver,
        FakeMode,
    )
    .await
}

struct FakeMode;

impl RunnerCommandHandler for FakeMode {
    fn handle_command<'a>(
        &'a mut self,
        envelope: Box<agenter_protocol::runner::RunnerCommandEnvelope>,
        background_sender: RunnerBackgroundEventSender,
    ) -> RunnerHostBoxFuture<'a, RunnerResponseOutcome> {
        Box::pin(async move {
            match envelope.command {
                RunnerCommand::AgentSendInput(command) => {
                    tracing::info!(session_id = %command.session_id, request_id = %envelope.request_id, "fake runner received agent input");
                    for event in deterministic_fake_events(command.session_id, &command.input) {
                        tracing::debug!(session_id = %command.session_id, "fake runner emitting event");
                        let Some(agent_event) = event.universal_projection_for_wal() else {
                            continue;
                        };
                        background_sender
                            .send((
                                Some(envelope.request_id.clone()),
                                RunnerEvent::AgentEvent(Box::new(agent_event)),
                            ))
                            .ok();
                    }
                    Ok(RunnerResponseOutcome::Ok {
                        result: RunnerCommandResult::Accepted,
                    })
                }
                RunnerCommand::ListProviderCommands(_) => Ok(RunnerResponseOutcome::Ok {
                    result: RunnerCommandResult::ProviderCommands {
                        commands: Vec::new(),
                    },
                }),
                RunnerCommand::ExecuteProviderCommand(command) => Ok(RunnerResponseOutcome::Ok {
                    result: RunnerCommandResult::ProviderCommandExecuted {
                        result: agenter_core::SlashCommandResult {
                            accepted: true,
                            message: format!(
                                "Fake provider command {} accepted.",
                                command.command.command_id
                            ),
                            session: None,
                            provider_payload: None,
                        },
                    },
                }),
                _ => Ok(RunnerResponseOutcome::Ok {
                    result: RunnerCommandResult::Accepted,
                }),
            }
        })
    }
}

fn fake_hello(token: String) -> RunnerHello {
    let runner_id = fake_runner_id();
    RunnerHello {
        runner_id,
        protocol_version: PROTOCOL_VERSION.to_owned(),
        token,
        capabilities: RunnerCapabilities {
            agent_providers: vec![AgentProviderAdvertisement {
                provider_id: AgentProviderId::from("fake"),
                capabilities: crate::default_agent_capabilities(false, false),
            }],
            transports: vec!["fake".to_owned()],
            workspace_discovery: false,
        },
        acked_runner_event_seq: None,
        replay_from_runner_event_seq: None,
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

fn deterministic_fake_events(session_id: SessionId, input: &AgentInput) -> Vec<AdapterEvent> {
    let content = match input {
        AgentInput::Text { text } => text.clone(),
        AgentInput::UserMessage { payload } => payload.content.clone(),
    };
    let response = format!("fake runner received: {content}");
    let turn_id = fake_turn_id("fake-turn-1");
    let user_item_id = fake_item_id("fake-user-1");
    let command_item_id = fake_item_id("fake-command-1");
    let tool_item_id = fake_item_id("fake-tool-1");
    let assistant_item_id = fake_item_id("fake-agent-1");

    vec![
        AdapterEvent::from_universal(
            session_id,
            Some(turn_id),
            None,
            Some(fake_native(
                "turn/started",
                "fake-turn-1",
                "fake turn started",
            )),
            UniversalEventKind::TurnStarted {
                turn: fake_turn_state(session_id, turn_id, TurnStatus::Running),
            },
        ),
        AdapterEvent::from_universal(
            session_id,
            Some(turn_id),
            Some(user_item_id),
            Some(fake_native("item/created", "fake-user-1", "user message")),
            UniversalEventKind::ItemCreated {
                item: Box::new(ItemState {
                    item_id: user_item_id,
                    session_id,
                    turn_id: Some(turn_id),
                    role: ItemRole::User,
                    status: ItemStatus::Completed,
                    content: vec![ContentBlock {
                        block_id: "fake-user-text".to_owned(),
                        kind: ContentBlockKind::Text,
                        text: Some(content),
                        mime_type: None,
                        artifact_id: None,
                    }],
                    tool: None,
                    native: Some(fake_native("item/created", "fake-user-1", "user message")),
                }),
            },
        ),
        AdapterEvent::from_universal(
            session_id,
            Some(turn_id),
            Some(command_item_id),
            Some(fake_native(
                "command/started",
                "fake-command-1",
                "command started",
            )),
            UniversalEventKind::ItemCreated {
                item: Box::new(fake_command_item(
                    session_id,
                    turn_id,
                    command_item_id,
                    ItemStatus::Streaming,
                    None,
                )),
            },
        ),
        AdapterEvent::from_universal(
            session_id,
            Some(turn_id),
            Some(command_item_id),
            Some(fake_native(
                "command/output",
                "fake-command-1",
                "command output",
            )),
            UniversalEventKind::ContentDelta {
                block_id: "fake-command-stdout".to_owned(),
                kind: Some(ContentBlockKind::CommandOutput),
                delta: "fake-runner\n".to_owned(),
            },
        ),
        AdapterEvent::from_universal(
            session_id,
            Some(turn_id),
            Some(command_item_id),
            Some(fake_native(
                "command/completed",
                "fake-command-1",
                "command completed",
            )),
            UniversalEventKind::ItemCreated {
                item: Box::new(fake_command_item(
                    session_id,
                    turn_id,
                    command_item_id,
                    ItemStatus::Completed,
                    Some("fake-runner\n".to_owned()),
                )),
            },
        ),
        AdapterEvent::from_universal(
            session_id,
            Some(turn_id),
            Some(tool_item_id),
            Some(fake_native("tool/started", "fake-tool-1", "tool started")),
            UniversalEventKind::ItemCreated {
                item: Box::new(fake_tool_item(
                    session_id,
                    turn_id,
                    tool_item_id,
                    ItemStatus::Streaming,
                    Some(serde_json::json!({ "query": response.clone() })),
                    None,
                )),
            },
        ),
        AdapterEvent::from_universal(
            session_id,
            Some(turn_id),
            Some(tool_item_id),
            Some(fake_native(
                "tool/completed",
                "fake-tool-1",
                "tool completed",
            )),
            UniversalEventKind::ItemCreated {
                item: Box::new(fake_tool_item(
                    session_id,
                    turn_id,
                    tool_item_id,
                    ItemStatus::Completed,
                    None,
                    Some(serde_json::json!({ "ok": true })),
                )),
            },
        ),
        AdapterEvent::from_universal(
            session_id,
            Some(turn_id),
            None,
            Some(fake_native("diff/proposed", "fake-diff-1", "diff proposed")),
            UniversalEventKind::DiffUpdated {
                diff: fake_diff(
                    session_id,
                    turn_id,
                    "fake-output.txt",
                    Some("-old\n+fake runner output\n".to_owned()),
                ),
            },
        ),
        AdapterEvent::from_universal(
            session_id,
            Some(turn_id),
            None,
            Some(fake_native("diff/applied", "fake-diff-1", "diff applied")),
            UniversalEventKind::DiffUpdated {
                diff: fake_diff(session_id, turn_id, "fake-output.txt", None),
            },
        ),
        AdapterEvent::from_universal(
            session_id,
            Some(turn_id),
            None,
            Some(fake_native(
                "approval/requested",
                "fake-approval",
                "approval requested",
            )),
            UniversalEventKind::ApprovalRequested {
                approval: Box::new(ApprovalRequest {
                    approval_id: ApprovalId::from_uuid(Uuid::from_u128(
                        0x44444444444444444444444444444444,
                    )),
                    session_id,
                    turn_id: Some(turn_id),
                    item_id: Some(command_item_id),
                    kind: ApprovalKind::Command,
                    title: "Approve fake command".to_owned(),
                    details: Some("This is an in-memory approval stub.".to_owned()),
                    options: agenter_core::ApprovalOption::canonical_defaults(),
                    status: ApprovalStatus::Pending,
                    risk: Some("medium".to_owned()),
                    subject: Some("This is an in-memory approval stub.".to_owned()),
                    native_request_id: Some("fake-approval".to_owned()),
                    native_blocking: true,
                    policy: None,
                    native: Some(fake_native(
                        "approval/requested",
                        "fake-approval",
                        "approval requested",
                    )),
                    requested_at: None,
                    resolved_at: None,
                    resolving_decision: None,
                }),
            },
        ),
        AdapterEvent::from_universal(
            session_id,
            Some(turn_id),
            Some(assistant_item_id),
            Some(fake_native(
                "item/created",
                "fake-agent-1",
                "assistant message",
            )),
            UniversalEventKind::ItemCreated {
                item: Box::new(ItemState {
                    item_id: assistant_item_id,
                    session_id,
                    turn_id: Some(turn_id),
                    role: ItemRole::Assistant,
                    status: ItemStatus::Streaming,
                    content: vec![ContentBlock {
                        block_id: "fake-agent-text".to_owned(),
                        kind: ContentBlockKind::Text,
                        text: None,
                        mime_type: None,
                        artifact_id: None,
                    }],
                    tool: None,
                    native: Some(fake_native(
                        "item/created",
                        "fake-agent-1",
                        "assistant message",
                    )),
                }),
            },
        ),
        AdapterEvent::from_universal(
            session_id,
            Some(turn_id),
            Some(assistant_item_id),
            Some(fake_native(
                "content/delta",
                "fake-agent-1",
                "assistant text delta",
            )),
            UniversalEventKind::ContentDelta {
                block_id: "fake-agent-text".to_owned(),
                kind: Some(ContentBlockKind::Text),
                delta: response.clone(),
            },
        ),
        AdapterEvent::from_universal(
            session_id,
            Some(turn_id),
            Some(assistant_item_id),
            Some(fake_native(
                "content/completed",
                "fake-agent-1",
                "assistant text completed",
            )),
            UniversalEventKind::ContentCompleted {
                block_id: "fake-agent-text".to_owned(),
                kind: Some(ContentBlockKind::Text),
                text: Some(response),
            },
        ),
        AdapterEvent::from_universal(
            session_id,
            Some(turn_id),
            None,
            Some(fake_native("error", "fake-notice", "fake diagnostic")),
            UniversalEventKind::ErrorReported {
                code: Some("fake_notice".to_owned()),
                message: "Fake runner diagnostic card".to_owned(),
            },
        ),
        AdapterEvent::from_universal(
            session_id,
            Some(turn_id),
            None,
            Some(fake_native(
                "turn/completed",
                "fake-turn-1",
                "fake turn completed",
            )),
            UniversalEventKind::TurnCompleted {
                turn: fake_turn_state(session_id, turn_id, TurnStatus::Completed),
            },
        ),
    ]
}

fn fake_runner_id() -> RunnerId {
    RunnerId::from_uuid(Uuid::from_u128(0x33333333333333333333333333333333))
}

fn fake_turn_id(value: &str) -> TurnId {
    TurnId::from_uuid(Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("agenter:fake:turn:{value}").as_bytes(),
    ))
}

fn fake_item_id(value: &str) -> ItemId {
    ItemId::from_uuid(Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("agenter:fake:item:{value}").as_bytes(),
    ))
}

fn fake_diff_id(value: &str) -> DiffId {
    DiffId::from_uuid(Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("agenter:fake:diff:{value}").as_bytes(),
    ))
}

fn fake_native(method: &str, native_id: &str, summary: &str) -> NativeRef {
    NativeRef {
        protocol: "fake-runner".to_owned(),
        method: Some(method.to_owned()),
        kind: Some("fake".to_owned()),
        native_id: Some(native_id.to_owned()),
        summary: Some(summary.to_owned()),
        hash: None,
        pointer: None,
        raw_payload: None,
    }
}

fn fake_turn_state(session_id: SessionId, turn_id: TurnId, status: TurnStatus) -> TurnState {
    TurnState {
        turn_id,
        session_id,
        status,
        started_at: None,
        completed_at: None,
        model: None,
        mode: None,
    }
}

fn fake_command_item(
    session_id: SessionId,
    turn_id: TurnId,
    item_id: ItemId,
    status: ItemStatus,
    output: Option<String>,
) -> ItemState {
    ItemState {
        item_id,
        session_id,
        turn_id: Some(turn_id),
        role: ItemRole::Tool,
        status: status.clone(),
        content: vec![ContentBlock {
            block_id: "fake-command".to_owned(),
            kind: ContentBlockKind::ToolCall,
            text: Some("printf fake-runner".to_owned()),
            mime_type: None,
            artifact_id: None,
        }],
        tool: Some(ToolProjection {
            kind: ToolProjectionKind::Command,
            subkind: Some("command".to_owned()),
            name: "command".to_owned(),
            title: "printf fake-runner".to_owned(),
            status,
            detail: Some(".".to_owned()),
            input_summary: Some("printf fake-runner".to_owned()),
            output_summary: output.clone(),
            command: Some(ToolCommandProjection {
                command: "printf fake-runner".to_owned(),
                cwd: Some(".".to_owned()),
                source: None,
                process_id: None,
                actions: Vec::new(),
                exit_code: output.as_ref().map(|_| 0),
                duration_ms: None,
                success: output.as_ref().map(|_| true),
            }),
            subagent: None,
            mcp: None,
        }),
        native: Some(fake_native(
            "command/item",
            "fake-command-1",
            "command item",
        )),
    }
}

fn fake_tool_item(
    session_id: SessionId,
    turn_id: TurnId,
    item_id: ItemId,
    status: ItemStatus,
    input: Option<serde_json::Value>,
    output: Option<serde_json::Value>,
) -> ItemState {
    ItemState {
        item_id,
        session_id,
        turn_id: Some(turn_id),
        role: ItemRole::Tool,
        status: status.clone(),
        content: vec![ContentBlock {
            block_id: "fake-tool".to_owned(),
            kind: if output.is_some() {
                ContentBlockKind::ToolResult
            } else {
                ContentBlockKind::ToolCall
            },
            text: Some("Fake lookup".to_owned()),
            mime_type: None,
            artifact_id: None,
        }],
        tool: Some(ToolProjection {
            kind: ToolProjectionKind::Tool,
            subkind: None,
            name: "fake_lookup".to_owned(),
            title: "Fake lookup".to_owned(),
            status,
            detail: None,
            input_summary: input
                .as_ref()
                .and_then(|value| serde_json::to_string(value).ok()),
            output_summary: output
                .as_ref()
                .and_then(|value| serde_json::to_string(value).ok()),
            command: None,
            subagent: None,
            mcp: None,
        }),
        native: Some(fake_native("tool/item", "fake-tool-1", "tool item")),
    }
}

fn fake_diff(
    session_id: SessionId,
    turn_id: TurnId,
    path: &str,
    diff: Option<String>,
) -> DiffState {
    DiffState {
        diff_id: fake_diff_id("fake-diff-1"),
        session_id,
        turn_id: Some(turn_id),
        title: Some(path.to_owned()),
        files: vec![DiffFile {
            path: path.to_owned(),
            status: FileChangeKind::Modify,
            diff,
        }],
        updated_at: None,
    }
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

        assert_eq!(events.len(), 15);
        assert!(matches!(
            events[0].universal.event,
            UniversalEventKind::TurnStarted { .. }
        ));
        assert!(matches!(
            events[1].universal.event,
            UniversalEventKind::ItemCreated { ref item }
                if item.role == ItemRole::User
        ));
        assert!(matches!(
            events[2].universal.event,
            UniversalEventKind::ItemCreated { ref item }
                if item.tool.as_ref().is_some_and(|tool| tool.kind == ToolProjectionKind::Command)
        ));
        assert!(matches!(
            events[10].universal.event,
            UniversalEventKind::ItemCreated { ref item }
                if item.role == ItemRole::Assistant
        ));
        assert!(matches!(
            events[12].universal.event,
            UniversalEventKind::ContentCompleted { .. }
        ));
        assert!(events.iter().all(|event| event.universal.turn_id.is_some()));
    }
}

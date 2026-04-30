mod agents;

use std::{collections::HashMap, env, path::PathBuf, sync::Arc};

use agenter_core::{
    AgentCapabilities, AgentErrorEvent, AgentMessageDeltaEvent, AgentProviderId, AppEvent,
    ApprovalId, ApprovalKind, ApprovalRequestEvent, CommandCompletedEvent, CommandEvent,
    CommandOutputEvent, CommandOutputStream, FileChangeEvent, FileChangeKind,
    MessageCompletedEvent, RunnerId, SessionId, ToolEvent, UserMessageEvent, WorkspaceId,
    WorkspaceRef,
};
use agenter_protocol::runner::{
    AgentInput, AgentProviderAdvertisement, RunnerCapabilities, RunnerClientMessage, RunnerCommand,
    RunnerCommandResult, RunnerError, RunnerEvent, RunnerEventEnvelope, RunnerHello,
    RunnerResponseEnvelope, RunnerResponseOutcome, PROTOCOL_VERSION,
};
use agents::codex::{
    run_codex_turn_on_server, CodexAppServer, CodexTurnRequest, PendingCodexApproval,
};
use agents::qwen_acp::{run_qwen_turn, PendingQwenApproval, QwenTurnRequest};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use uuid::Uuid;

const DEFAULT_CONTROL_PLANE_WS: &str = "ws://127.0.0.1:7777/api/runner/ws";
const DEFAULT_DEV_RUNNER_TOKEN: &str = "dev-runner-token";

#[derive(Clone)]
struct CodexRunnerRuntime {
    workspace_path: PathBuf,
    server: Arc<Mutex<Option<CodexAppServer>>>,
}

impl CodexRunnerRuntime {
    fn new(workspace_path: PathBuf) -> Self {
        Self {
            workspace_path,
            server: Arc::new(Mutex::new(None)),
        }
    }

    async fn create_session(&self) -> anyhow::Result<String> {
        let mut guard = self.server.lock().await;
        let server = ensure_codex_server(&mut guard, self.workspace_path.clone()).await?;
        server.start_thread(&self.workspace_path).await
    }

    async fn resume_session(&self, external_session_id: &str) -> anyhow::Result<String> {
        let mut guard = self.server.lock().await;
        let server = ensure_codex_server(&mut guard, self.workspace_path.clone()).await?;
        server
            .resume_thread(external_session_id, &self.workspace_path)
            .await?;
        Ok(external_session_id.to_owned())
    }

    async fn run_turn(
        &self,
        request: CodexTurnRequest,
        event_sender: mpsc::UnboundedSender<AppEvent>,
        pending_approvals: Arc<Mutex<HashMap<ApprovalId, PendingCodexApproval>>>,
    ) -> anyhow::Result<()> {
        let mut guard = self.server.lock().await;
        let server = ensure_codex_server(&mut guard, self.workspace_path.clone()).await?;
        if let Some(thread_id) = &request.external_session_id {
            server.set_active_thread(thread_id.clone());
        } else {
            let thread_id = server.start_thread(&self.workspace_path).await?;
            server.set_active_thread(thread_id);
        }
        run_codex_turn_on_server(server, request, event_sender, pending_approvals).await
    }
}

async fn ensure_codex_server(
    server: &mut Option<CodexAppServer>,
    workspace_path: PathBuf,
) -> anyhow::Result<&mut CodexAppServer> {
    if server.is_none() {
        let mut app_server = CodexAppServer::spawn(workspace_path)?;
        app_server.initialize().await?;
        *server = Some(app_server);
    }
    Ok(server.as_mut().expect("codex server was initialized"))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    agenter_core::logging::init_tracing("agenter-runner");

    if fake_mode_requested() {
        tracing::info!("starting fake runner mode");
        run_fake_runner().await?;
    } else if codex_mode_requested() {
        tracing::info!("starting codex runner mode");
        run_codex_runner().await?;
    } else if qwen_mode_requested() {
        tracing::info!("starting qwen runner mode");
        run_qwen_runner().await?;
    } else {
        println!("agenter runner");
    }

    Ok(())
}

fn fake_mode_requested() -> bool {
    env::args().any(|arg| arg == "fake" || arg == "--fake")
        || env::var("AGENTER_RUNNER_MODE").is_ok_and(|mode| mode == "fake")
}

fn codex_mode_requested() -> bool {
    env::args().any(|arg| arg == "codex" || arg == "--codex")
        || env::var("AGENTER_RUNNER_MODE").is_ok_and(|mode| mode == "codex")
}

fn qwen_mode_requested() -> bool {
    env::args().any(|arg| arg == "qwen" || arg == "--qwen")
        || env::var("AGENTER_RUNNER_MODE").is_ok_and(|mode| mode == "qwen")
}

async fn run_codex_runner() -> anyhow::Result<()> {
    let url = env::var("AGENTER_CONTROL_PLANE_WS")
        .unwrap_or_else(|_| DEFAULT_CONTROL_PLANE_WS.to_owned());
    let token = env::var("AGENTER_DEV_RUNNER_TOKEN")
        .unwrap_or_else(|_| DEFAULT_DEV_RUNNER_TOKEN.to_owned());
    let workspace_path = env::var("AGENTER_WORKSPACE")
        .map(PathBuf::from)
        .unwrap_or(env::current_dir()?);
    let workspace_path = workspace_path.canonicalize().unwrap_or(workspace_path);
    tracing::info!(url = %url, workspace = %workspace_path.display(), "connecting codex runner to control plane");
    let (socket, _) = connect_async(&url).await?;
    let (mut sender, mut receiver) = socket.split();
    let hello = codex_hello(token, workspace_path.clone());
    tracing::info!(runner_id = %hello.runner_id, "sending codex runner hello");
    send_runner_message(&mut sender, RunnerClientMessage::Hello(hello)).await?;

    let pending_approvals: Arc<Mutex<HashMap<ApprovalId, PendingCodexApproval>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let (codex_event_sender, mut codex_event_receiver) = mpsc::unbounded_channel::<AppEvent>();
    let codex_runtime = CodexRunnerRuntime::new(workspace_path.clone());

    loop {
        tokio::select! {
            event = codex_event_receiver.recv() => {
                let Some(event) = event else {
                    continue;
                };
                let Some(session_id) = app_event_session_id(&event) else {
                    continue;
                };
                send_runner_message(
                    &mut sender,
                    RunnerClientMessage::Event(RunnerEventEnvelope {
                        request_id: None,
                        event: RunnerEvent::AgentEvent(agenter_protocol::AgentEvent {
                            session_id,
                            event,
                        }),
                    }),
                )
                .await?;
            }
            message = receiver.next() => {
                let Some(message) = message else {
                    tracing::info!("control plane websocket closed for codex runner");
                    break;
                };
                let Message::Text(text) = message? else {
                    continue;
                };
                let Ok(agenter_protocol::RunnerServerMessage::Command(envelope)) =
                    serde_json::from_str::<agenter_protocol::RunnerServerMessage>(&text)
                else {
                    tracing::warn!("codex runner ignored undecodable control-plane message");
                    continue;
                };

                match envelope.command {
                    RunnerCommand::CreateSession(command) => {
                        tracing::info!(session_id = %command.session_id, request_id = %envelope.request_id, "codex runner received create session");
                        let outcome = match codex_runtime.create_session().await {
                            Ok(external_session_id) => RunnerResponseOutcome::Ok {
                                result: RunnerCommandResult::SessionCreated {
                                    session_id: command.session_id,
                                    external_session_id,
                                },
                            },
                            Err(error) => RunnerResponseOutcome::Error {
                                error: runner_error("codex_create_session_failed", error),
                            },
                        };
                        send_runner_message(
                            &mut sender,
                            RunnerClientMessage::Response(RunnerResponseEnvelope {
                                request_id: envelope.request_id,
                                outcome,
                            }),
                        )
                        .await?;
                    }
                    RunnerCommand::ResumeSession(command) => {
                        tracing::info!(session_id = %command.session_id, request_id = %envelope.request_id, "codex runner received resume session");
                        let outcome = match codex_runtime.resume_session(&command.external_session_id).await {
                            Ok(external_session_id) => RunnerResponseOutcome::Ok {
                                result: RunnerCommandResult::SessionResumed {
                                    session_id: command.session_id,
                                    external_session_id,
                                },
                            },
                            Err(error) => RunnerResponseOutcome::Error {
                                error: runner_error("codex_resume_session_failed", error),
                            },
                        };
                        send_runner_message(
                            &mut sender,
                            RunnerClientMessage::Response(RunnerResponseEnvelope {
                                request_id: envelope.request_id,
                                outcome,
                            }),
                        )
                        .await?;
                    }
                    RunnerCommand::AgentSendInput(command) => {
                        tracing::info!(session_id = %command.session_id, request_id = %envelope.request_id, "codex runner received agent input");
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
                        let prompt = agent_input_text(&command.input);
                        let request = CodexTurnRequest {
                            session_id: command.session_id,
                            workspace_path: workspace_path.clone(),
                            external_session_id: command.external_session_id,
                            prompt,
                        };
                        let event_sender = codex_event_sender.clone();
                        let pending = pending_approvals.clone();
                        let runtime = codex_runtime.clone();
                        let session_id = request.session_id;
                        tokio::spawn(async move {
                            if let Err(error) = runtime.run_turn(request, event_sender.clone(), pending).await {
                                tracing::error!(%session_id, %error, "codex turn failed");
                                event_sender.send(AppEvent::Error(AgentErrorEvent {
                                    session_id: Some(session_id),
                                    code: Some("codex_adapter_error".to_owned()),
                                    message: error.to_string(),
                                    provider_payload: None,
                                })).ok();
                            }
                        });
                    }
                    RunnerCommand::AnswerApproval(command) => {
                        tracing::info!(session_id = %command.session_id, approval_id = %command.approval_id, "codex runner received approval answer");
                        let pending = pending_approvals.lock().await.remove(&command.approval_id);
                        let outcome = if let Some(pending) = pending {
                            pending.response.send(command.decision).ok();
                            RunnerResponseOutcome::Ok {
                                result: RunnerCommandResult::Accepted,
                            }
                        } else {
                            tracing::warn!(approval_id = %command.approval_id, "codex approval answer had no pending provider request");
                            RunnerResponseOutcome::Error {
                                error: agenter_protocol::runner::RunnerError {
                                    code: "approval_not_found".to_owned(),
                                    message: "approval is no longer pending in the Codex adapter".to_owned(),
                                },
                            }
                        };
                        send_runner_message(
                            &mut sender,
                            RunnerClientMessage::Response(RunnerResponseEnvelope {
                                request_id: envelope.request_id,
                                outcome,
                            }),
                        )
                        .await?;
                    }
                    RunnerCommand::InterruptSession { .. }
                    | RunnerCommand::ShutdownSession(_) => {
                        tracing::debug!(request_id = %envelope.request_id, "codex runner accepted lifecycle command placeholder");
                        send_runner_message(
                            &mut sender,
                            RunnerClientMessage::Response(RunnerResponseEnvelope {
                                request_id: envelope.request_id,
                                outcome: RunnerResponseOutcome::Ok {
                                    result: RunnerCommandResult::Accepted,
                                },
                            }),
                        )
                        .await?;
                    }
                }
            }
        }
    }

    Ok(())
}

async fn run_qwen_runner() -> anyhow::Result<()> {
    let url = env::var("AGENTER_CONTROL_PLANE_WS")
        .unwrap_or_else(|_| DEFAULT_CONTROL_PLANE_WS.to_owned());
    let token = env::var("AGENTER_DEV_RUNNER_TOKEN")
        .unwrap_or_else(|_| DEFAULT_DEV_RUNNER_TOKEN.to_owned());
    let workspace_path = env::var("AGENTER_WORKSPACE")
        .map(PathBuf::from)
        .unwrap_or(env::current_dir()?);
    let workspace_path = workspace_path.canonicalize().unwrap_or(workspace_path);
    tracing::info!(url = %url, workspace = %workspace_path.display(), "connecting qwen runner to control plane");
    let (socket, _) = connect_async(&url).await?;
    let (mut sender, mut receiver) = socket.split();
    let hello = qwen_hello(token, workspace_path.clone());
    tracing::info!(runner_id = %hello.runner_id, "sending qwen runner hello");
    send_runner_message(&mut sender, RunnerClientMessage::Hello(hello)).await?;

    let pending_approvals: Arc<Mutex<HashMap<ApprovalId, PendingQwenApproval>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let (qwen_event_sender, mut qwen_event_receiver) = mpsc::unbounded_channel::<AppEvent>();

    loop {
        tokio::select! {
            event = qwen_event_receiver.recv() => {
                let Some(event) = event else {
                    continue;
                };
                let Some(session_id) = app_event_session_id(&event) else {
                    continue;
                };
                send_runner_message(
                    &mut sender,
                    RunnerClientMessage::Event(RunnerEventEnvelope {
                        request_id: None,
                        event: RunnerEvent::AgentEvent(agenter_protocol::AgentEvent {
                            session_id,
                            event,
                        }),
                    }),
                )
                .await?;
            }
            message = receiver.next() => {
                let Some(message) = message else {
                    tracing::info!("control plane websocket closed for qwen runner");
                    break;
                };
                let Message::Text(text) = message? else {
                    continue;
                };
                let Ok(agenter_protocol::RunnerServerMessage::Command(envelope)) =
                    serde_json::from_str::<agenter_protocol::RunnerServerMessage>(&text)
                else {
                    tracing::warn!("qwen runner ignored undecodable control-plane message");
                    continue;
                };

                match envelope.command {
                    RunnerCommand::AgentSendInput(command) => {
                        tracing::info!(session_id = %command.session_id, request_id = %envelope.request_id, "qwen runner received agent input");
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
                        let request = QwenTurnRequest {
                            session_id: command.session_id,
                            workspace_path: workspace_path.clone(),
                            external_session_id: command.external_session_id,
                            prompt: agent_input_text(&command.input),
                        };
                        let event_sender = qwen_event_sender.clone();
                        let pending = pending_approvals.clone();
                        let session_id = request.session_id;
                        tokio::spawn(async move {
                            if let Err(error) = run_qwen_turn(request, event_sender.clone(), pending).await {
                                tracing::error!(%session_id, %error, "qwen turn failed");
                                event_sender.send(AppEvent::Error(AgentErrorEvent {
                                    session_id: Some(session_id),
                                    code: Some("qwen_adapter_error".to_owned()),
                                    message: error.to_string(),
                                    provider_payload: None,
                                })).ok();
                            }
                        });
                    }
                    RunnerCommand::AnswerApproval(command) => {
                        tracing::info!(session_id = %command.session_id, approval_id = %command.approval_id, "qwen runner received approval answer");
                        let pending = pending_approvals.lock().await.remove(&command.approval_id);
                        let outcome = if let Some(pending) = pending {
                            pending.response.send(command.decision).ok();
                            RunnerResponseOutcome::Ok {
                                result: RunnerCommandResult::Accepted,
                            }
                        } else {
                            tracing::warn!(approval_id = %command.approval_id, "qwen approval answer had no pending provider request");
                            RunnerResponseOutcome::Error {
                                error: agenter_protocol::runner::RunnerError {
                                    code: "approval_not_found".to_owned(),
                                    message: "approval is no longer pending in the Qwen adapter".to_owned(),
                                },
                            }
                        };
                        send_runner_message(
                            &mut sender,
                            RunnerClientMessage::Response(RunnerResponseEnvelope {
                                request_id: envelope.request_id,
                                outcome,
                            }),
                        )
                        .await?;
                    }
                    RunnerCommand::CreateSession(_)
                    | RunnerCommand::ResumeSession(_)
                    | RunnerCommand::InterruptSession { .. }
                    | RunnerCommand::ShutdownSession(_) => {
                        tracing::debug!(request_id = %envelope.request_id, "qwen runner accepted lifecycle command placeholder");
                        send_runner_message(
                            &mut sender,
                            RunnerClientMessage::Response(RunnerResponseEnvelope {
                                request_id: envelope.request_id,
                                outcome: RunnerResponseOutcome::Ok {
                                    result: RunnerCommandResult::Accepted,
                                },
                            }),
                        )
                        .await?;
                    }
                }
            }
        }
    }

    Ok(())
}

async fn run_fake_runner() -> anyhow::Result<()> {
    let url = env::var("AGENTER_CONTROL_PLANE_WS")
        .unwrap_or_else(|_| DEFAULT_CONTROL_PLANE_WS.to_owned());
    let token = env::var("AGENTER_DEV_RUNNER_TOKEN")
        .unwrap_or_else(|_| DEFAULT_DEV_RUNNER_TOKEN.to_owned());
    tracing::info!(url = %url, "connecting fake runner to control plane");
    let (socket, _) = connect_async(&url).await?;
    let (mut sender, mut receiver) = socket.split();

    let hello = fake_hello(token);
    tracing::info!(runner_id = %hello.runner_id, "sending fake runner hello");
    send_runner_message(&mut sender, RunnerClientMessage::Hello(hello)).await?;

    while let Some(message) = receiver.next().await {
        let Message::Text(text) = message? else {
            continue;
        };
        let Ok(agenter_protocol::RunnerServerMessage::Command(envelope)) =
            serde_json::from_str::<agenter_protocol::RunnerServerMessage>(&text)
        else {
            tracing::warn!("fake runner ignored undecodable control-plane message");
            continue;
        };

        if let RunnerCommand::AgentSendInput(command) = envelope.command {
            tracing::info!(session_id = %command.session_id, request_id = %envelope.request_id, "fake runner received agent input");
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
                tracing::debug!(session_id = %command.session_id, "fake runner emitting event");
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

fn runner_error(code: &str, error: anyhow::Error) -> RunnerError {
    RunnerError {
        code: code.to_owned(),
        message: error.to_string(),
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

fn codex_hello(token: String, workspace_path: PathBuf) -> RunnerHello {
    provider_hello(
        token,
        workspace_path,
        AgentProviderId::from(AgentProviderId::CODEX),
        "codex-app-server",
        "codex workspace",
        true,
    )
}

fn qwen_hello(token: String, workspace_path: PathBuf) -> RunnerHello {
    provider_hello(
        token,
        workspace_path,
        AgentProviderId::from(AgentProviderId::QWEN),
        "qwen-acp",
        "qwen workspace",
        false,
    )
}

fn provider_hello(
    token: String,
    workspace_path: PathBuf,
    provider_id: AgentProviderId,
    transport: &str,
    fallback_name: &str,
    session_resume: bool,
) -> RunnerHello {
    let runner_id = RunnerId::new();
    RunnerHello {
        runner_id,
        protocol_version: PROTOCOL_VERSION.to_owned(),
        token,
        capabilities: RunnerCapabilities {
            agent_providers: vec![AgentProviderAdvertisement {
                provider_id,
                capabilities: AgentCapabilities {
                    streaming: true,
                    approvals: true,
                    file_changes: true,
                    command_execution: true,
                    session_resume,
                    ..AgentCapabilities::default()
                },
            }],
            transports: vec![transport.to_owned()],
            workspace_discovery: false,
        },
        workspaces: vec![WorkspaceRef {
            workspace_id: WorkspaceId::new(),
            runner_id,
            path: workspace_path.display().to_string(),
            display_name: workspace_path
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
                .or_else(|| Some(fallback_name.to_owned())),
        }],
    }
}

fn fake_runner_id() -> RunnerId {
    RunnerId::from_uuid(Uuid::from_u128(0x33333333333333333333333333333333))
}

fn agent_input_text(input: &AgentInput) -> String {
    match input {
        AgentInput::Text { text } => text.clone(),
        AgentInput::UserMessage { payload } => payload.content.clone(),
    }
}

fn app_event_session_id(event: &AppEvent) -> Option<SessionId> {
    match event {
        AppEvent::SessionStarted(info) => Some(info.session_id),
        AppEvent::SessionStatusChanged(event) => Some(event.session_id),
        AppEvent::UserMessage(event) => Some(event.session_id),
        AppEvent::AgentMessageDelta(event) => Some(event.session_id),
        AppEvent::AgentMessageCompleted(event) => Some(event.session_id),
        AppEvent::PlanUpdated(event) => Some(event.session_id),
        AppEvent::ToolStarted(event)
        | AppEvent::ToolUpdated(event)
        | AppEvent::ToolCompleted(event) => Some(event.session_id),
        AppEvent::CommandStarted(event) => Some(event.session_id),
        AppEvent::CommandOutputDelta(event) => Some(event.session_id),
        AppEvent::CommandCompleted(event) => Some(event.session_id),
        AppEvent::FileChangeProposed(event)
        | AppEvent::FileChangeApplied(event)
        | AppEvent::FileChangeRejected(event) => Some(event.session_id),
        AppEvent::ApprovalRequested(event) => Some(event.session_id),
        AppEvent::ApprovalResolved(event) => Some(event.session_id),
        AppEvent::Error(event) => event.session_id,
    }
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

mod agents;

use std::{
    collections::{HashMap, HashSet},
    env,
    path::{Path, PathBuf},
    sync::Arc,
};

use agenter_core::{
    AgentCapabilities, AgentErrorEvent, AgentMessageDeltaEvent, AgentProviderId, AppEvent,
    ApprovalDecision, ApprovalId, ApprovalKind, ApprovalRequestEvent, CommandCompletedEvent,
    CommandEvent, CommandOutputEvent, CommandOutputStream, FileChangeEvent, FileChangeKind,
    MessageCompletedEvent, QuestionId, RunnerId, SessionId, SessionStatus,
    SessionStatusChangedEvent, ToolEvent, UserMessageEvent, WorkspaceId, WorkspaceRef,
};
use agenter_protocol::runner::{
    AgentInput, AgentProviderAdvertisement, DiscoveredSession, DiscoveredSessionHistoryStatus,
    DiscoveredSessions, RunnerCapabilities, RunnerClientMessage, RunnerCommand,
    RunnerCommandResult, RunnerError, RunnerEvent, RunnerEventEnvelope, RunnerHello,
    RunnerResponseEnvelope, RunnerResponseOutcome, RunnerServerMessage, PROTOCOL_VERSION,
};
use agenter_protocol::{
    chunk_message, reassemble_message, RunnerTransportChunkFrame, RunnerTransportChunkReassembler,
    RunnerTransportOutboundFrame,
};
use agents::acp::{AcpProviderProfile, AcpRunnerRuntime, AcpTurnRequest, PendingAcpApproval};
use agents::approval_state::{PendingApprovalSubmitError, PendingProviderApproval};
use agents::codex::{
    codex_provider_slash_commands, is_codex_no_rollout_resume_error,
    is_codex_thread_not_found_error, run_codex_turn_on_server, CodexAppServer, CodexTurnRequest,
    PendingCodexApproval, PendingCodexQuestion,
};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use uuid::Uuid;

const DEFAULT_CONTROL_PLANE_WS: &str = "ws://127.0.0.1:7777/api/runner/ws";
const DEFAULT_DEV_RUNNER_TOKEN: &str = "dev-runner-token";
const DEFAULT_RUNNER_WS_CHUNK_BYTES: usize = 1024 * 1024;
const DEFAULT_RUNNER_WS_MAX_MESSAGE_BYTES: usize = 512 * 1024 * 1024;

#[derive(Clone)]
struct CodexRunnerRuntime {
    workspace_path: PathBuf,
    sessions: Arc<Mutex<HashMap<SessionId, Arc<Mutex<CodexSessionRuntime>>>>>,
}

struct CodexSessionRuntime {
    server: CodexAppServer,
    live_thread_ids: HashSet<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum CodexTurnThreadAction {
    StartNew,
    UseLive(String),
    ResumeExisting(String),
}

async fn answer_pending_provider_approval(
    approval_id: ApprovalId,
    decision: ApprovalDecision,
    pending: PendingProviderApproval,
    provider_label: &'static str,
) -> RunnerResponseOutcome {
    match pending.submit(decision).await {
        Ok(()) => {
            tracing::info!(
                %approval_id,
                provider = provider_label,
                "provider approval decision acknowledged"
            );
            RunnerResponseOutcome::Ok {
                result: RunnerCommandResult::Accepted,
            }
        }
        Err(PendingApprovalSubmitError::ConflictingDecision) => RunnerResponseOutcome::Error {
            error: RunnerError {
                code: "approval_conflicting_decision".to_owned(),
                message: format!(
                    "{provider_label} approval {approval_id} is already resolving with a different decision"
                ),
            },
        },
        Err(PendingApprovalSubmitError::ProviderWaiterDropped) => RunnerResponseOutcome::Error {
            error: RunnerError {
                code: format!("{}_approval_response_failed", provider_label.to_lowercase()),
                message: format!(
                    "{provider_label} approval waiter was dropped before the decision could be delivered"
                ),
            },
        },
        Err(PendingApprovalSubmitError::ProviderRejected(message)) => RunnerResponseOutcome::Error {
            error: RunnerError {
                code: format!("{}_approval_response_failed", provider_label.to_lowercase()),
                message,
            },
        },
        Err(PendingApprovalSubmitError::AcknowledgementDropped) => RunnerResponseOutcome::Error {
            error: RunnerError {
                code: format!("{}_approval_response_failed", provider_label.to_lowercase()),
                message: format!(
                    "{provider_label} approval response acknowledgement was dropped"
                ),
            },
        },
    }
}

fn codex_turn_thread_action(
    external_session_id: Option<&str>,
    live_thread_ids: &HashSet<String>,
) -> CodexTurnThreadAction {
    match external_session_id {
        Some(thread_id) if live_thread_ids.contains(thread_id) => {
            CodexTurnThreadAction::UseLive(thread_id.to_owned())
        }
        Some(thread_id) => CodexTurnThreadAction::ResumeExisting(thread_id.to_owned()),
        None => CodexTurnThreadAction::StartNew,
    }
}

impl CodexRunnerRuntime {
    fn new(workspace_path: PathBuf) -> Self {
        Self {
            workspace_path,
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn create_session(&self, session_id: SessionId) -> anyhow::Result<String> {
        let mut server = spawn_codex_server(self.workspace_path.clone()).await?;
        let thread_id = server.start_thread(&self.workspace_path).await?;
        let mut live_thread_ids = HashSet::new();
        live_thread_ids.insert(thread_id.clone());
        self.sessions.lock().await.insert(
            session_id,
            Arc::new(Mutex::new(CodexSessionRuntime {
                server,
                live_thread_ids,
            })),
        );
        Ok(thread_id)
    }

    async fn resume_session(
        &self,
        session_id: SessionId,
        external_session_id: &str,
    ) -> anyhow::Result<String> {
        let mut server = spawn_codex_server(self.workspace_path.clone()).await?;
        server
            .resume_thread(external_session_id, &self.workspace_path)
            .await?;
        let mut live_thread_ids = HashSet::new();
        live_thread_ids.insert(external_session_id.to_owned());
        self.sessions.lock().await.insert(
            session_id,
            Arc::new(Mutex::new(CodexSessionRuntime {
                server,
                live_thread_ids,
            })),
        );
        Ok(external_session_id.to_owned())
    }

    async fn discover_sessions(&self) -> anyhow::Result<Vec<DiscoveredSession>> {
        let mut server = spawn_codex_server(self.workspace_path.clone()).await?;
        let threads = server.list_threads(&self.workspace_path).await?;
        let mut discovered = Vec::with_capacity(threads.len());
        for thread in threads {
            let (history_status, history) = match server
                .read_thread_history(&thread.external_session_id)
                .await
            {
                Ok(history) => (DiscoveredSessionHistoryStatus::Loaded, history),
                Err(error) => {
                    tracing::warn!(
                        external_session_id = %thread.external_session_id,
                        %error,
                        "failed to read codex thread history during discovery"
                    );
                    (
                        DiscoveredSessionHistoryStatus::Failed {
                            message: error.to_string(),
                        },
                        Vec::new(),
                    )
                }
            };
            discovered.push(DiscoveredSession {
                external_session_id: thread.external_session_id,
                title: thread.title,
                updated_at: thread.updated_at,
                history_status,
                history,
            });
        }
        Ok(discovered)
    }

    async fn agent_options(&self) -> anyhow::Result<agenter_core::AgentOptions> {
        let mut server = spawn_codex_server(self.workspace_path.clone()).await?;
        server.agent_options().await
    }

    async fn execute_provider_command(
        &self,
        external_session_id: Option<String>,
        command: agenter_core::SlashCommandRequest,
    ) -> anyhow::Result<agenter_core::SlashCommandResult> {
        let mut server = spawn_codex_server(self.workspace_path.clone()).await?;
        if let Some(thread_id) = external_session_id {
            server
                .resume_thread(&thread_id, &self.workspace_path)
                .await?;
        }
        server
            .execute_provider_command(&command, &self.workspace_path)
            .await
    }

    async fn run_turn(
        &self,
        request: CodexTurnRequest,
        event_sender: mpsc::UnboundedSender<AppEvent>,
        pending_approvals: Arc<Mutex<HashMap<ApprovalId, PendingCodexApproval>>>,
        pending_questions: Arc<Mutex<HashMap<QuestionId, PendingCodexQuestion>>>,
    ) -> anyhow::Result<()> {
        let session_runtime = self
            .ensure_session_runtime(request.session_id, request.external_session_id.clone())
            .await?;
        let mut session_runtime = session_runtime.lock().await;
        let thread_action = {
            codex_turn_thread_action(
                request.external_session_id.as_deref(),
                &session_runtime.live_thread_ids,
            )
        };
        let existing_thread_id = match thread_action {
            CodexTurnThreadAction::UseLive(thread_id) => {
                session_runtime.server.set_active_thread(thread_id.clone());
                Some(thread_id)
            }
            CodexTurnThreadAction::ResumeExisting(thread_id) => {
                match session_runtime
                    .server
                    .resume_thread(&thread_id, &self.workspace_path)
                    .await
                {
                    Ok(()) => {
                        session_runtime.live_thread_ids.insert(thread_id.clone());
                    }
                    Err(error) if is_codex_no_rollout_resume_error(error.as_ref()) => {
                        tracing::warn!(
                            session_id = %request.session_id,
                            provider_thread_id = %thread_id,
                            "codex resume reported no rollout; treating thread as a pre-first-turn live thread"
                        );
                        session_runtime.server.set_active_thread(thread_id.clone());
                        session_runtime.live_thread_ids.insert(thread_id.clone());
                    }
                    Err(error) => return Err(error),
                }
                Some(thread_id)
            }
            CodexTurnThreadAction::StartNew => {
                let thread_id = session_runtime
                    .server
                    .start_thread(&self.workspace_path)
                    .await?;
                session_runtime.server.set_active_thread(thread_id.clone());
                session_runtime.live_thread_ids.insert(thread_id);
                None
            }
        };
        let result = run_codex_turn_on_server(
            &mut session_runtime.server,
            request.clone(),
            event_sender.clone(),
            pending_approvals.clone(),
            pending_questions.clone(),
        )
        .await;
        if let (Err(error), Some(thread_id)) = (&result, existing_thread_id.as_deref()) {
            if is_codex_thread_not_found_error(error.as_ref()) {
                tracing::warn!(
                    session_id = %request.session_id,
                    provider_thread_id = %thread_id,
                    "codex turn/start reported missing thread after resume; retrying resume once"
                );
                session_runtime
                    .server
                    .resume_thread(thread_id, &self.workspace_path)
                    .await?;
                return run_codex_turn_on_server(
                    &mut session_runtime.server,
                    request,
                    event_sender,
                    pending_approvals,
                    pending_questions,
                )
                .await;
            }
        }
        result
    }

    async fn shutdown_session(&self, session_id: SessionId) -> bool {
        self.sessions.lock().await.remove(&session_id).is_some()
    }

    async fn ensure_session_runtime(
        &self,
        session_id: SessionId,
        external_session_id: Option<String>,
    ) -> anyhow::Result<Arc<Mutex<CodexSessionRuntime>>> {
        if let Some(session) = self.sessions.lock().await.get(&session_id).cloned() {
            return Ok(session);
        }
        if let Some(external_session_id) = external_session_id {
            self.resume_session(session_id, &external_session_id)
                .await?;
        } else {
            self.create_session(session_id).await?;
        }
        self.sessions
            .lock()
            .await
            .get(&session_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("codex session runtime was not available"))
    }
}

async fn spawn_codex_server(workspace_path: PathBuf) -> anyhow::Result<CodexAppServer> {
    let mut app_server = CodexAppServer::spawn(workspace_path)?;
    app_server.initialize().await?;
    Ok(app_server)
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
    } else if acp_mode_requested() {
        tracing::info!("starting multi-provider ACP runner mode");
        run_acp_runner(AcpProviderProfile::available_all(), false).await?;
    } else if qwen_mode_requested() {
        tracing::info!("starting qwen ACP runner mode");
        run_acp_runner(vec![AcpProviderProfile::qwen()], false).await?;
    } else if gemini_mode_requested() {
        tracing::info!("starting gemini ACP runner mode");
        run_acp_runner(vec![AcpProviderProfile::gemini()], false).await?;
    } else if opencode_mode_requested() {
        tracing::info!("starting opencode ACP runner mode");
        run_acp_runner(vec![AcpProviderProfile::opencode()], false).await?;
    } else {
        tracing::info!("starting unified runner mode");
        run_acp_runner(AcpProviderProfile::available_all(), codex_available()).await?;
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

fn acp_mode_requested() -> bool {
    env::args().any(|arg| arg == "acp" || arg == "--acp")
        || env::var("AGENTER_RUNNER_MODE").is_ok_and(|mode| mode == "acp")
}

fn gemini_mode_requested() -> bool {
    env::args().any(|arg| arg == "gemini" || arg == "--gemini")
        || env::var("AGENTER_RUNNER_MODE").is_ok_and(|mode| mode == "gemini")
}

fn opencode_mode_requested() -> bool {
    env::args().any(|arg| arg == "opencode" || arg == "--opencode")
        || env::var("AGENTER_RUNNER_MODE").is_ok_and(|mode| mode == "opencode")
}

fn codex_available() -> bool {
    std::process::Command::new("codex")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
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
    let mut server_message_reassembler =
        RunnerTransportChunkReassembler::new(runner_ws_max_message_bytes());
    let hello = codex_hello(token, workspace_path.clone());
    let advertised_workspace = hello.workspaces.first().cloned();
    tracing::info!(runner_id = %hello.runner_id, "sending codex runner hello");
    send_runner_message(&mut sender, RunnerClientMessage::Hello(hello)).await?;

    let pending_approvals: Arc<Mutex<HashMap<ApprovalId, PendingCodexApproval>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let pending_questions: Arc<Mutex<HashMap<QuestionId, PendingCodexQuestion>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let (codex_event_sender, mut codex_event_receiver) = mpsc::unbounded_channel::<AppEvent>();
    let codex_runtime = CodexRunnerRuntime::new(workspace_path.clone());
    if let Some(workspace) = advertised_workspace {
        match codex_runtime.discover_sessions().await {
            Ok(sessions) if !sessions.is_empty() => {
                tracing::info!(
                    session_count = sessions.len(),
                    "sending discovered codex sessions to control plane"
                );
                send_runner_message(
                    &mut sender,
                    RunnerClientMessage::Event(RunnerEventEnvelope {
                        request_id: None,
                        event: RunnerEvent::SessionsDiscovered(DiscoveredSessions {
                            workspace,
                            provider_id: AgentProviderId::from(AgentProviderId::CODEX),
                            sessions,
                        }),
                    }),
                )
                .await?;
            }
            Ok(_) => {
                tracing::debug!("codex discovery found no native threads");
            }
            Err(error) => {
                tracing::warn!(%error, "codex native session discovery failed");
            }
        }
    }

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
                let Some(RunnerServerMessage::Command(envelope)) =
                    next_runner_server_message(&mut server_message_reassembler, &text)?
                else {
                    continue;
                };

                match envelope.command {
                    RunnerCommand::CreateSession(command) => {
                        tracing::info!(session_id = %command.session_id, request_id = %envelope.request_id, "codex runner received create session");
                        let outcome = match codex_runtime.create_session(command.session_id).await {
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
                        let outcome = match codex_runtime.resume_session(command.session_id, &command.external_session_id).await {
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
                    RunnerCommand::RefreshSessions(command) => {
                        tracing::info!(request_id = %envelope.request_id, workspace = %command.workspace.path, provider_id = %command.provider_id, "codex runner received refresh sessions");
                        let outcome = match codex_runtime.discover_sessions().await {
                            Ok(sessions) => {
                                send_runner_message(
                                    &mut sender,
                                    RunnerClientMessage::Event(RunnerEventEnvelope {
                                        request_id: Some(envelope.request_id.clone()),
                                        event: RunnerEvent::SessionsDiscovered(DiscoveredSessions {
                                            workspace: command.workspace,
                                            provider_id: command.provider_id,
                                            sessions,
                                        }),
                                    }),
                                )
                                .await?;
                                RunnerResponseOutcome::Ok {
                                    result: RunnerCommandResult::Accepted,
                                }
                            }
                            Err(error) => RunnerResponseOutcome::Error {
                                error: runner_error("codex_refresh_sessions_failed", error),
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
                    RunnerCommand::GetAgentOptions(command) => {
                        tracing::info!(session_id = %command.session_id, request_id = %envelope.request_id, "codex runner received agent options request");
                        let outcome = match codex_runtime.agent_options().await {
                            Ok(options) => RunnerResponseOutcome::Ok {
                                result: RunnerCommandResult::AgentOptions { options },
                            },
                            Err(error) => RunnerResponseOutcome::Error {
                                error: runner_error("codex_agent_options_failed", error),
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
                    RunnerCommand::ListProviderCommands(command) => {
                        tracing::info!(session_id = %command.session_id, request_id = %envelope.request_id, provider_id = %command.provider_id, "codex runner received provider command manifest request");
                        send_runner_message(
                            &mut sender,
                            RunnerClientMessage::Response(RunnerResponseEnvelope {
                                request_id: envelope.request_id,
                                outcome: RunnerResponseOutcome::Ok {
                                    result: RunnerCommandResult::ProviderCommands {
                                        commands: codex_provider_slash_commands(),
                                    },
                                },
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
                            settings: command.settings,
                        };
                        let event_sender = codex_event_sender.clone();
                        let pending = pending_approvals.clone();
                        let pending_question_answers = pending_questions.clone();
                        let runtime = codex_runtime.clone();
                        let session_id = request.session_id;
                        tokio::spawn(async move {
                            if let Err(error) = runtime.run_turn(request, event_sender.clone(), pending, pending_question_answers).await {
                                tracing::error!(%session_id, %error, "codex turn failed");
                                event_sender.send(AppEvent::SessionStatusChanged(SessionStatusChangedEvent {
                                    session_id,
                                    status: SessionStatus::Failed,
                                    reason: Some(error.to_string()),
                                })).ok();
                                event_sender.send(AppEvent::Error(AgentErrorEvent {
                                    session_id: Some(session_id),
                                    code: Some("codex_adapter_error".to_owned()),
                                    message: error.to_string(),
                                    provider_payload: None,
                                })).ok();
                            }
                        });
                    }
                    RunnerCommand::ExecuteProviderCommand(command) => {
                        tracing::info!(session_id = %command.session_id, request_id = %envelope.request_id, command_id = %command.command.command_id, "codex runner received provider command");
                        let outcome = match codex_runtime
                            .execute_provider_command(command.external_session_id, command.command)
                            .await
                        {
                            Ok(result) => RunnerResponseOutcome::Ok {
                                result: RunnerCommandResult::ProviderCommandExecuted { result },
                            },
                            Err(error) => RunnerResponseOutcome::Error {
                                error: runner_error("codex_provider_command_failed", error),
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
                    RunnerCommand::AnswerApproval(command) => {
                        tracing::info!(session_id = %command.session_id, approval_id = %command.approval_id, "codex runner received approval answer");
                        let pending = pending_approvals
                            .lock()
                            .await
                            .get(&command.approval_id)
                            .cloned();
                        let outcome = if let Some(pending) = pending {
                            answer_pending_provider_approval(
                                command.approval_id,
                                command.decision,
                                pending,
                                "Codex",
                            )
                            .await
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
                    RunnerCommand::AnswerQuestion(command) => {
                        tracing::info!(session_id = %command.session_id, question_id = %command.answer.question_id, "codex runner received question answer");
                        let pending = pending_questions.lock().await.remove(&command.answer.question_id);
                        let outcome = if let Some(pending) = pending {
                            pending.response.send(command.answer).ok();
                            RunnerResponseOutcome::Ok {
                                result: RunnerCommandResult::Accepted,
                            }
                        } else {
                            tracing::warn!(question_id = %command.answer.question_id, "codex question answer had no pending provider request");
                            RunnerResponseOutcome::Error {
                                error: agenter_protocol::runner::RunnerError {
                                    code: "question_not_found".to_owned(),
                                    message: "question is no longer pending in the Codex adapter".to_owned(),
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
                    RunnerCommand::InterruptSession { .. } => {
                        tracing::debug!(request_id = %envelope.request_id, "codex runner accepted interrupt placeholder");
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
                    RunnerCommand::ShutdownSession(command) => {
                        tracing::info!(session_id = %command.session_id, request_id = %envelope.request_id, "codex runner shutting down session runtime");
                        codex_runtime.shutdown_session(command.session_id).await;
                        codex_event_sender
                            .send(AppEvent::SessionStatusChanged(SessionStatusChangedEvent {
                                session_id: command.session_id,
                                status: SessionStatus::Stopped,
                                reason: Some("Codex session runtime stopped.".to_owned()),
                            }))
                            .ok();
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

async fn run_acp_runner(
    profiles: Vec<AcpProviderProfile>,
    include_codex: bool,
) -> anyhow::Result<()> {
    if profiles.is_empty() && !include_codex {
        anyhow::bail!(
            "no provider commands are available; install codex, qwen, gemini, or opencode"
        );
    }
    let url = env::var("AGENTER_CONTROL_PLANE_WS")
        .unwrap_or_else(|_| DEFAULT_CONTROL_PLANE_WS.to_owned());
    let token = env::var("AGENTER_DEV_RUNNER_TOKEN")
        .unwrap_or_else(|_| DEFAULT_DEV_RUNNER_TOKEN.to_owned());
    let workspace_path = env::var("AGENTER_WORKSPACE")
        .map(PathBuf::from)
        .unwrap_or(env::current_dir()?);
    let workspace_path = workspace_path.canonicalize().unwrap_or(workspace_path);
    tracing::info!(
        url = %url,
        workspace = %workspace_path.display(),
        provider_count = profiles.len(),
        include_codex,
        "connecting multi-provider runner to control plane"
    );
    let (socket, _) = connect_async(&url).await?;
    let (mut sender, mut receiver) = socket.split();
    let mut server_message_reassembler =
        RunnerTransportChunkReassembler::new(runner_ws_max_message_bytes());
    let hello = acp_hello(token, workspace_path.clone(), &profiles, include_codex);
    let advertised_workspace = hello.workspaces.first().cloned();
    tracing::info!(runner_id = %hello.runner_id, "sending ACP runner hello");
    send_runner_message(&mut sender, RunnerClientMessage::Hello(hello)).await?;

    let profiles_by_id = profiles
        .into_iter()
        .map(|profile| (profile.provider_id.clone(), profile))
        .collect::<HashMap<_, _>>();
    let mut session_profiles = HashMap::<SessionId, AgentProviderId>::new();
    let pending_approvals: Arc<Mutex<HashMap<ApprovalId, PendingAcpApproval>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let pending_codex_approvals: Arc<Mutex<HashMap<ApprovalId, PendingCodexApproval>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let pending_codex_questions: Arc<Mutex<HashMap<QuestionId, PendingCodexQuestion>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let (acp_event_sender, mut acp_event_receiver) = mpsc::unbounded_channel::<AppEvent>();
    let acp_runtime = AcpRunnerRuntime::new(workspace_path.clone());
    let codex_runtime = include_codex.then(|| CodexRunnerRuntime::new(workspace_path.clone()));

    loop {
        tokio::select! {
            event = acp_event_receiver.recv() => {
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
                    tracing::info!("control plane websocket closed for ACP runner");
                    break;
                };
                let Message::Text(text) = message? else {
                    continue;
                };
                let Some(RunnerServerMessage::Command(envelope)) =
                    next_runner_server_message(&mut server_message_reassembler, &text)?
                else {
                    continue;
                };

                match envelope.command {
                    RunnerCommand::CreateSession(command) => {
                        tracing::info!(session_id = %command.session_id, request_id = %envelope.request_id, provider_id = %command.provider_id, "multi-provider runner received create session");
                        if command.provider_id.as_str() == AgentProviderId::CODEX {
                            let outcome = match &codex_runtime {
                                Some(runtime) => match runtime.create_session(command.session_id).await {
                                    Ok(external_session_id) => {
                                        session_profiles.insert(command.session_id, command.provider_id);
                                        RunnerResponseOutcome::Ok {
                                            result: RunnerCommandResult::SessionCreated {
                                                session_id: command.session_id,
                                                external_session_id,
                                            },
                                        }
                                    }
                                    Err(error) => RunnerResponseOutcome::Error {
                                        error: runner_error("codex_create_session_failed", error),
                                    },
                                },
                                None => RunnerResponseOutcome::Error {
                                    error: agenter_protocol::runner::RunnerError {
                                        code: "codex_provider_not_available".to_owned(),
                                        message: "Codex is not available in this runner".to_owned(),
                                    },
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
                            continue;
                        }
                        let Some(profile) = profiles_by_id.get(&command.provider_id).cloned() else {
                            send_runner_message(
                                &mut sender,
                                RunnerClientMessage::Response(RunnerResponseEnvelope {
                                    request_id: envelope.request_id,
                                    outcome: RunnerResponseOutcome::Error {
                                        error: agenter_protocol::runner::RunnerError {
                                            code: "acp_provider_not_available".to_owned(),
                                            message: format!("ACP provider `{}` is not available in this runner", command.provider_id),
                                        },
                                    },
                                }),
                            )
                            .await?;
                            continue;
                        };
                        let outcome = match acp_runtime.create_session(command.session_id, profile).await {
                            Ok(external_session_id) => {
                                session_profiles.insert(command.session_id, command.provider_id);
                                RunnerResponseOutcome::Ok {
                                    result: RunnerCommandResult::SessionCreated {
                                        session_id: command.session_id,
                                        external_session_id,
                                    },
                                }
                            }
                            Err(error) => RunnerResponseOutcome::Error {
                                error: runner_error("acp_create_session_failed", error),
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
                        tracing::info!(session_id = %command.session_id, request_id = %envelope.request_id, provider_id = %command.provider_id, "multi-provider runner received resume session");
                        if command.provider_id.as_str() == AgentProviderId::CODEX {
                            let outcome = match &codex_runtime {
                                Some(runtime) => match runtime
                                    .resume_session(command.session_id, &command.external_session_id)
                                    .await
                                {
                                    Ok(external_session_id) => {
                                        session_profiles.insert(command.session_id, command.provider_id);
                                        RunnerResponseOutcome::Ok {
                                            result: RunnerCommandResult::SessionResumed {
                                                session_id: command.session_id,
                                                external_session_id,
                                            },
                                        }
                                    }
                                    Err(error) => RunnerResponseOutcome::Error {
                                        error: runner_error("codex_resume_session_failed", error),
                                    },
                                },
                                None => RunnerResponseOutcome::Error {
                                    error: agenter_protocol::runner::RunnerError {
                                        code: "codex_provider_not_available".to_owned(),
                                        message: "Codex is not available in this runner".to_owned(),
                                    },
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
                            continue;
                        }
                        let Some(profile) = profiles_by_id.get(&command.provider_id).cloned() else {
                            send_runner_message(
                                &mut sender,
                                RunnerClientMessage::Response(RunnerResponseEnvelope {
                                    request_id: envelope.request_id,
                                    outcome: RunnerResponseOutcome::Error {
                                        error: agenter_protocol::runner::RunnerError {
                                            code: "acp_provider_not_available".to_owned(),
                                            message: format!("ACP provider `{}` is not available in this runner", command.provider_id),
                                        },
                                    },
                                }),
                            )
                            .await?;
                            continue;
                        };
                        let outcome = match acp_runtime
                            .resume_session(command.session_id, profile, command.external_session_id)
                            .await
                        {
                            Ok(external_session_id) => {
                                session_profiles.insert(command.session_id, command.provider_id);
                                RunnerResponseOutcome::Ok {
                                    result: RunnerCommandResult::SessionResumed {
                                        session_id: command.session_id,
                                        external_session_id,
                                    },
                                }
                            }
                            Err(error) => RunnerResponseOutcome::Error {
                                error: runner_error("acp_resume_session_failed", error),
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
                    RunnerCommand::RefreshSessions(command) => {
                        tracing::info!(request_id = %envelope.request_id, workspace = %command.workspace.path, provider_id = %command.provider_id, "multi-provider runner received refresh sessions");
                        if command.provider_id.as_str() == AgentProviderId::CODEX {
                            let outcome = match &codex_runtime {
                                Some(runtime) => match runtime.discover_sessions().await {
                                    Ok(sessions) => {
                                        send_runner_message(
                                            &mut sender,
                                            RunnerClientMessage::Event(RunnerEventEnvelope {
                                                request_id: Some(envelope.request_id.clone()),
                                                event: RunnerEvent::SessionsDiscovered(DiscoveredSessions {
                                                    workspace: command.workspace,
                                                    provider_id: command.provider_id,
                                                    sessions,
                                                }),
                                            }),
                                        )
                                        .await?;
                                        RunnerResponseOutcome::Ok {
                                            result: RunnerCommandResult::Accepted,
                                        }
                                    }
                                    Err(error) => RunnerResponseOutcome::Error {
                                        error: runner_error("codex_refresh_sessions_failed", error),
                                    },
                                },
                                None => RunnerResponseOutcome::Error {
                                    error: agenter_protocol::runner::RunnerError {
                                        code: "codex_provider_not_available".to_owned(),
                                        message: "Codex is not available in this runner".to_owned(),
                                    },
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
                            continue;
                        }
                        let Some(profile) = profiles_by_id.get(&command.provider_id).cloned() else {
                            send_runner_message(
                                &mut sender,
                                RunnerClientMessage::Response(RunnerResponseEnvelope {
                                    request_id: envelope.request_id,
                                    outcome: RunnerResponseOutcome::Error {
                                        error: agenter_protocol::runner::RunnerError {
                                            code: "acp_provider_not_available".to_owned(),
                                            message: format!("ACP provider `{}` is not available in this runner", command.provider_id),
                                        },
                                    },
                                }),
                            )
                            .await?;
                            continue;
                        };
                        let outcome = match acp_runtime.discover_sessions(profile).await {
                            Ok(sessions) => {
                                send_runner_message(
                                    &mut sender,
                                    RunnerClientMessage::Event(RunnerEventEnvelope {
                                        request_id: Some(envelope.request_id.clone()),
                                        event: RunnerEvent::SessionsDiscovered(DiscoveredSessions {
                                            workspace: command.workspace,
                                            provider_id: command.provider_id,
                                            sessions,
                                        }),
                                    }),
                                )
                                .await?;
                                RunnerResponseOutcome::Ok {
                                    result: RunnerCommandResult::Accepted,
                                }
                            }
                            Err(error) => RunnerResponseOutcome::Error {
                                error: runner_error("acp_refresh_sessions_failed", error),
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
                        tracing::info!(session_id = %command.session_id, request_id = %envelope.request_id, "multi-provider runner received agent input");
                        let provider_id = session_profiles
                            .get(&command.session_id)
                            .cloned()
                            .or_else(|| command.provider_id.clone())
                            .or_else(|| profiles_by_id.keys().next().cloned());
                        let Some(provider_id) = provider_id else {
                            send_runner_message(
                                &mut sender,
                                RunnerClientMessage::Response(RunnerResponseEnvelope {
                                    request_id: envelope.request_id,
                                    outcome: RunnerResponseOutcome::Error {
                                        error: agenter_protocol::runner::RunnerError {
                                            code: "acp_provider_not_available".to_owned(),
                                            message: "No ACP provider is available for this session.".to_owned(),
                                        },
                                    },
                                }),
                            )
                            .await?;
                            continue;
                        };
                        if provider_id.as_str() == AgentProviderId::CODEX {
                            let Some(runtime) = codex_runtime.clone() else {
                                send_runner_message(
                                    &mut sender,
                                    RunnerClientMessage::Response(RunnerResponseEnvelope {
                                        request_id: envelope.request_id,
                                        outcome: RunnerResponseOutcome::Error {
                                            error: agenter_protocol::runner::RunnerError {
                                                code: "codex_provider_not_available".to_owned(),
                                                message: "Codex is not available in this runner".to_owned(),
                                            },
                                        },
                                    }),
                                )
                                .await?;
                                continue;
                            };
                            session_profiles.insert(command.session_id, provider_id);
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
                                settings: command.settings,
                            };
                            let event_sender = acp_event_sender.clone();
                            let pending = pending_codex_approvals.clone();
                            let pending_question_answers = pending_codex_questions.clone();
                            let session_id = request.session_id;
                            tokio::spawn(async move {
                                if let Err(error) = runtime
                                    .run_turn(
                                        request,
                                        event_sender.clone(),
                                        pending,
                                        pending_question_answers,
                                    )
                                    .await
                                {
                                    tracing::error!(%session_id, %error, "codex turn failed");
                                    event_sender
                                        .send(AppEvent::SessionStatusChanged(
                                            SessionStatusChangedEvent {
                                                session_id,
                                                status: SessionStatus::Failed,
                                                reason: Some(error.to_string()),
                                            },
                                        ))
                                        .ok();
                                    event_sender
                                        .send(AppEvent::Error(AgentErrorEvent {
                                            session_id: Some(session_id),
                                            code: Some("codex_adapter_error".to_owned()),
                                            message: error.to_string(),
                                            provider_payload: None,
                                        }))
                                        .ok();
                                }
                            });
                            continue;
                        }
                        let Some(profile) = profiles_by_id.get(&provider_id).cloned() else {
                            send_runner_message(
                                &mut sender,
                                RunnerClientMessage::Response(RunnerResponseEnvelope {
                                    request_id: envelope.request_id,
                                    outcome: RunnerResponseOutcome::Error {
                                        error: agenter_protocol::runner::RunnerError {
                                            code: "acp_provider_not_available".to_owned(),
                                            message: format!("ACP provider `{provider_id}` is not available in this runner"),
                                        },
                                    },
                                }),
                            )
                            .await?;
                            continue;
                        };
                        session_profiles.insert(command.session_id, provider_id);
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
                        let request = AcpTurnRequest {
                            session_id: command.session_id,
                            external_session_id: command.external_session_id,
                            prompt: agent_input_text(&command.input),
                        };
                        let event_sender = acp_event_sender.clone();
                        let pending = pending_approvals.clone();
                        let runtime = acp_runtime.clone();
                        let session_id = request.session_id;
                        tokio::spawn(async move {
                            if let Err(error) = runtime.run_turn(request, profile, event_sender.clone(), pending).await {
                                tracing::error!(%session_id, %error, "ACP turn failed");
                                event_sender.send(AppEvent::SessionStatusChanged(SessionStatusChangedEvent {
                                    session_id,
                                    status: SessionStatus::Failed,
                                    reason: Some(error.to_string()),
                                })).ok();
                                event_sender.send(AppEvent::Error(AgentErrorEvent {
                                    session_id: Some(session_id),
                                    code: Some("acp_adapter_error".to_owned()),
                                    message: error.to_string(),
                                    provider_payload: None,
                                })).ok();
                            }
                        });
                    }
                    RunnerCommand::AnswerApproval(command) => {
                        tracing::info!(session_id = %command.session_id, approval_id = %command.approval_id, "multi-provider runner received approval answer");
                        let pending = pending_approvals
                            .lock()
                            .await
                            .get(&command.approval_id)
                            .cloned();
                        let outcome = if let Some(pending) = pending {
                            answer_pending_provider_approval(
                                command.approval_id,
                                command.decision,
                                pending,
                                "ACP",
                            )
                            .await
                        } else if let Some(pending) = pending_codex_approvals
                            .lock()
                            .await
                            .get(&command.approval_id)
                            .cloned()
                        {
                            answer_pending_provider_approval(
                                command.approval_id,
                                command.decision,
                                pending,
                                "Codex",
                            )
                            .await
                        } else {
                            RunnerResponseOutcome::Error {
                                error: agenter_protocol::runner::RunnerError {
                                    code: "approval_not_found".to_owned(),
                                    message: "approval is no longer pending in the provider adapter".to_owned(),
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
                    RunnerCommand::ListProviderCommands(command) => {
                        let commands = if command.provider_id.as_str() == AgentProviderId::CODEX {
                            codex_provider_slash_commands()
                        } else {
                            Vec::new()
                        };
                        send_runner_message(
                            &mut sender,
                            RunnerClientMessage::Response(RunnerResponseEnvelope {
                                request_id: envelope.request_id,
                                outcome: RunnerResponseOutcome::Ok {
                                    result: RunnerCommandResult::ProviderCommands {
                                        commands,
                                    },
                                },
                            }),
                        )
                        .await?;
                    }
                    RunnerCommand::ExecuteProviderCommand(command) => {
                        if command.provider_id.as_str() == AgentProviderId::CODEX {
                            let outcome = match &codex_runtime {
                                Some(runtime) => match runtime
                                    .execute_provider_command(
                                        command.external_session_id,
                                        command.command,
                                    )
                                    .await
                                {
                                    Ok(result) => RunnerResponseOutcome::Ok {
                                        result: RunnerCommandResult::ProviderCommandExecuted {
                                            result,
                                        },
                                    },
                                    Err(error) => RunnerResponseOutcome::Error {
                                        error: runner_error("codex_provider_command_failed", error),
                                    },
                                },
                                None => RunnerResponseOutcome::Error {
                                    error: agenter_protocol::runner::RunnerError {
                                        code: "codex_provider_not_available".to_owned(),
                                        message: "Codex is not available in this runner".to_owned(),
                                    },
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
                            continue;
                        }
                        send_runner_message(
                            &mut sender,
                            RunnerClientMessage::Response(RunnerResponseEnvelope {
                                request_id: envelope.request_id,
                                outcome: RunnerResponseOutcome::Error {
                                    error: agenter_protocol::runner::RunnerError {
                                        code: "acp_provider_command_unsupported".to_owned(),
                                        message: format!("ACP provider command `{}` is not implemented yet.", command.command.command_id),
                                    },
                                },
                            }),
                        )
                        .await?;
                    }
                    RunnerCommand::GetAgentOptions(command) => {
                        let outcome = if command.provider_id.as_str() == AgentProviderId::CODEX {
                            match &codex_runtime {
                                Some(runtime) => match runtime.agent_options().await {
                                    Ok(options) => RunnerResponseOutcome::Ok {
                                        result: RunnerCommandResult::AgentOptions { options },
                                    },
                                    Err(error) => RunnerResponseOutcome::Error {
                                        error: runner_error("codex_agent_options_failed", error),
                                    },
                                },
                                None => RunnerResponseOutcome::Ok {
                                    result: RunnerCommandResult::AgentOptions {
                                        options: agenter_core::AgentOptions::default(),
                                    },
                                },
                            }
                        } else {
                            RunnerResponseOutcome::Ok {
                                result: RunnerCommandResult::AgentOptions {
                                    options: agenter_core::AgentOptions::default(),
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
                    RunnerCommand::AnswerQuestion(command) => {
                        let pending = pending_codex_questions
                            .lock()
                            .await
                            .remove(&command.answer.question_id);
                        let outcome = if let Some(pending) = pending {
                            pending.response.send(command.answer).ok();
                            RunnerResponseOutcome::Ok {
                                result: RunnerCommandResult::Accepted,
                            }
                        } else {
                            RunnerResponseOutcome::Ok {
                                result: RunnerCommandResult::Accepted,
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
                    RunnerCommand::InterruptSession { .. } => {
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
                    RunnerCommand::ShutdownSession(command) => {
                        acp_runtime.shutdown_session(command.session_id).await;
                        if let Some(runtime) = &codex_runtime {
                            runtime.shutdown_session(command.session_id).await;
                        }
                        acp_event_sender
                            .send(AppEvent::SessionStatusChanged(SessionStatusChangedEvent {
                                session_id: command.session_id,
                                status: SessionStatus::Stopped,
                                reason: Some("ACP session runtime stopped.".to_owned()),
                            }))
                            .ok();
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

    if let Some(workspace) = advertised_workspace {
        tracing::debug!(workspace = %workspace.path, "ACP runner closed");
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
    let mut server_message_reassembler =
        RunnerTransportChunkReassembler::new(runner_ws_max_message_bytes());

    let hello = fake_hello(token);
    tracing::info!(runner_id = %hello.runner_id, "sending fake runner hello");
    send_runner_message(&mut sender, RunnerClientMessage::Hello(hello)).await?;

    while let Some(message) = receiver.next().await {
        let Message::Text(text) = message? else {
            continue;
        };
        let Some(RunnerServerMessage::Command(envelope)) =
            next_runner_server_message(&mut server_message_reassembler, &text)?
        else {
            continue;
        };

        match envelope.command {
            RunnerCommand::AgentSendInput(command) => {
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
            RunnerCommand::ListProviderCommands(_) => {
                send_runner_message(
                    &mut sender,
                    RunnerClientMessage::Response(RunnerResponseEnvelope {
                        request_id: envelope.request_id,
                        outcome: RunnerResponseOutcome::Ok {
                            result: RunnerCommandResult::ProviderCommands {
                                commands: codex_provider_slash_commands(),
                            },
                        },
                    }),
                )
                .await?;
            }
            RunnerCommand::ExecuteProviderCommand(command) => {
                send_runner_message(
                    &mut sender,
                    RunnerClientMessage::Response(RunnerResponseEnvelope {
                        request_id: envelope.request_id,
                        outcome: RunnerResponseOutcome::Ok {
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
                        },
                    }),
                )
                .await?;
            }
            _ => {
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
    let chunk_bytes = runner_ws_chunk_bytes();
    let frames = chunk_message(&message, chunk_bytes)?;
    if frames.len() > 1 {
        let json = serde_json::to_string(&message)?;
        let chunk_start = runner_transport_chunk_start(&frames);
        tracing::warn!(
            direction = "runner_to_control_plane",
            message_type = runner_client_message_type(&message),
            transfer_id = chunk_start.as_ref().map(|start| start.transfer_id.as_str()),
            total_bytes = chunk_start.as_ref().map(|start| start.total_bytes),
            message_bytes = json.len(),
            chunk_bytes,
            frame_count = frames.len(),
            total_chunks = chunk_start.as_ref().map(|start| start.total_chunks),
            "sending chunked runner websocket message"
        );
    } else {
        let RunnerTransportOutboundFrame::Text(json) = &frames[0];
        tracing::debug!(
            message_bytes = json.len(),
            "sending runner websocket message"
        );
    }

    for frame in frames {
        let RunnerTransportOutboundFrame::Text(text) = frame;
        sender.send(Message::Text(text.into())).await?;
    }
    Ok(())
}

fn runner_client_message_type(message: &RunnerClientMessage) -> &'static str {
    match message {
        RunnerClientMessage::Hello(_) => "runner_hello",
        RunnerClientMessage::Heartbeat(_) => "runner_heartbeat",
        RunnerClientMessage::Response(_) => "runner_response",
        RunnerClientMessage::Event(_) => "runner_event",
    }
}

fn runner_transport_chunk_start(
    frames: &[RunnerTransportOutboundFrame],
) -> Option<agenter_protocol::runner_transport::RunnerChunkStart> {
    let RunnerTransportOutboundFrame::Text(text) = frames.first()?;
    let Ok(RunnerTransportChunkFrame::Start(start)) =
        serde_json::from_str::<RunnerTransportChunkFrame>(text)
    else {
        return None;
    };
    Some(start)
}

fn next_runner_server_message(
    reassembler: &mut RunnerTransportChunkReassembler,
    text: &str,
) -> anyhow::Result<Option<RunnerServerMessage>> {
    match reassemble_message(reassembler, text) {
        Ok(message) => Ok(message),
        Err(error) => {
            tracing::warn!(%error, "runner ignored undecodable control-plane message");
            Ok(None)
        }
    }
}

fn runner_ws_chunk_bytes() -> usize {
    env_usize(
        "AGENTER_RUNNER_WS_CHUNK_BYTES",
        DEFAULT_RUNNER_WS_CHUNK_BYTES,
    )
}

fn runner_ws_max_message_bytes() -> usize {
    env_usize(
        "AGENTER_RUNNER_WS_MAX_MESSAGE_BYTES",
        DEFAULT_RUNNER_WS_MAX_MESSAGE_BYTES,
    )
}

fn env_usize(name: &str, default: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
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
                    model_selection: true,
                    reasoning_effort: true,
                    collaboration_modes: true,
                    tool_user_input: true,
                    mcp_elicitation: true,
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

fn acp_hello(
    token: String,
    workspace_path: PathBuf,
    profiles: &[AcpProviderProfile],
    include_codex: bool,
) -> RunnerHello {
    let provider_id = if include_codex {
        AgentProviderId::from("multi")
    } else {
        AgentProviderId::from("acp")
    };
    let runner_id = configured_runner_id(&provider_id, &workspace_path);
    let workspace_id = configured_workspace_id(&provider_id, &workspace_path);
    let mut agent_providers = Vec::new();
    if include_codex {
        agent_providers.push(AgentProviderAdvertisement {
            provider_id: AgentProviderId::from(AgentProviderId::CODEX),
            capabilities: AgentCapabilities {
                streaming: true,
                approvals: true,
                file_changes: true,
                command_execution: true,
                session_resume: true,
                model_selection: true,
                reasoning_effort: true,
                collaboration_modes: true,
                tool_user_input: true,
                mcp_elicitation: true,
                ..AgentCapabilities::default()
            },
        });
    }
    agent_providers.extend(profiles.iter().map(|profile| AgentProviderAdvertisement {
        provider_id: profile.provider_id.clone(),
        capabilities: profile.advertised_capabilities(),
    }));
    RunnerHello {
        runner_id,
        protocol_version: PROTOCOL_VERSION.to_owned(),
        token,
        capabilities: RunnerCapabilities {
            agent_providers,
            transports: if include_codex {
                vec!["codex-app-server".to_owned(), "acp-stdio".to_owned()]
            } else {
                vec!["acp-stdio".to_owned()]
            },
            workspace_discovery: false,
        },
        workspaces: vec![WorkspaceRef {
            workspace_id,
            runner_id,
            path: workspace_path.display().to_string(),
            display_name: workspace_path
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
                .or_else(|| Some("acp workspace".to_owned())),
        }],
    }
}

fn provider_hello(
    token: String,
    workspace_path: PathBuf,
    provider_id: AgentProviderId,
    transport: &str,
    fallback_name: &str,
    session_resume: bool,
) -> RunnerHello {
    let runner_id = configured_runner_id(&provider_id, &workspace_path);
    let workspace_id = configured_workspace_id(&provider_id, &workspace_path);
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
                    model_selection: session_resume,
                    reasoning_effort: session_resume,
                    collaboration_modes: session_resume,
                    tool_user_input: session_resume,
                    mcp_elicitation: session_resume,
                    ..AgentCapabilities::default()
                },
            }],
            transports: vec![transport.to_owned()],
            workspace_discovery: false,
        },
        workspaces: vec![WorkspaceRef {
            workspace_id,
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

fn configured_runner_id(provider_id: &AgentProviderId, workspace_path: &Path) -> RunnerId {
    env::var("AGENTER_RUNNER_ID")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| {
            RunnerId::from_uuid(uuid::Uuid::new_v5(
                &uuid::Uuid::NAMESPACE_URL,
                format!("agenter:runner:{provider_id}:{}", workspace_path.display()).as_bytes(),
            ))
        })
}

fn configured_workspace_id(provider_id: &AgentProviderId, workspace_path: &Path) -> WorkspaceId {
    env::var("AGENTER_WORKSPACE_ID")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| {
            WorkspaceId::from_uuid(uuid::Uuid::new_v5(
                &uuid::Uuid::NAMESPACE_URL,
                format!(
                    "agenter:workspace:{provider_id}:{}",
                    workspace_path.display()
                )
                .as_bytes(),
            ))
        })
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
        AppEvent::QuestionRequested(event) => Some(event.session_id),
        AppEvent::QuestionAnswered(event) => Some(event.session_id),
        AppEvent::TurnDiffUpdated(event)
        | AppEvent::ItemReasoning(event)
        | AppEvent::ServerRequestResolved(event)
        | AppEvent::McpToolCallProgress(event)
        | AppEvent::ThreadRealtimeEvent(event)
        | AppEvent::ProviderEvent(event) => Some(event.session_id),
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
            source: None,
            process_id: None,
            actions: Vec::new(),
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
            duration_ms: None,
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
            presentation: None,
            resolution_state: None,
            resolving_decision: None,
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

    #[test]
    fn codex_turn_uses_resume_for_existing_provider_thread() {
        let mut live_threads = std::collections::HashSet::new();
        live_threads.insert("live-thread-1".to_owned());

        assert_eq!(
            codex_turn_thread_action(Some("live-thread-1"), &live_threads),
            CodexTurnThreadAction::UseLive("live-thread-1".to_owned())
        );
        assert_eq!(
            codex_turn_thread_action(Some("thread-1"), &live_threads),
            CodexTurnThreadAction::ResumeExisting("thread-1".to_owned())
        );
        assert_eq!(
            codex_turn_thread_action(None, &live_threads),
            CodexTurnThreadAction::StartNew
        );
    }

    #[test]
    fn unified_hello_advertises_codex_and_acp_providers() {
        let workspace = PathBuf::from("/tmp/agenter-workspace");
        let hello = acp_hello(
            "token".to_owned(),
            workspace,
            &[AcpProviderProfile::qwen(), AcpProviderProfile::gemini()],
            true,
        );
        let providers = hello
            .capabilities
            .agent_providers
            .iter()
            .map(|provider| provider.provider_id.as_str().to_owned())
            .collect::<Vec<_>>();

        assert_eq!(providers, vec!["codex", "qwen", "gemini"]);
        assert!(hello
            .capabilities
            .transports
            .iter()
            .any(|transport| transport == "codex-app-server"));
        assert!(hello
            .capabilities
            .transports
            .iter()
            .any(|transport| transport == "acp-stdio"));
    }
}

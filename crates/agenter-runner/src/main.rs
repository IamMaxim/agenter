mod agents;
mod wal;

use std::{
    collections::{HashMap, HashSet},
    env,
    path::{Path, PathBuf},
    sync::Arc,
};

use agenter_core::{
    AgentCapabilities, AgentErrorEvent, AgentMessageDeltaEvent, AgentProviderId, ApprovalDecision,
    ApprovalId, ApprovalKind, ApprovalRequestEvent, CommandCompletedEvent, CommandEvent,
    CommandOutputEvent, CommandOutputStream, FileChangeEvent, FileChangeKind,
    MessageCompletedEvent, NormalizedEvent, ProviderCapabilityDetail, ProviderCapabilityStatus,
    QuestionId, RunnerId, SessionId, SessionStatus, ToolEvent, UserMessageEvent, WorkspaceId,
    WorkspaceRef,
};
use agenter_protocol::runner::{
    AgentInput, AgentProviderAdvertisement, DiscoveredSession, DiscoveredSessionHistoryStatus,
    DiscoveredSessions, RunnerCapabilities, RunnerClientMessage, RunnerCommand,
    RunnerCommandResult, RunnerError, RunnerEvent, RunnerEventEnvelope, RunnerHello,
    RunnerOperationKind, RunnerOperationLogLevel, RunnerOperationProgress, RunnerOperationStatus,
    RunnerOperationUpdate, RunnerResponseEnvelope, RunnerResponseOutcome, RunnerServerMessage,
    PROTOCOL_VERSION,
};
use agenter_protocol::{
    chunk_message, reassemble_message, RequestId, RunnerTransportChunkFrame,
    RunnerTransportChunkReassembler, RunnerTransportOutboundFrame,
};
use agents::acp::{AcpProviderProfile, AcpRunnerRuntime, AcpTurnRequest, PendingAcpApproval};
use agents::adapter::{AdapterEvent, AdapterProviderRegistration, AdapterRuntime};
use agents::approval_state::{PendingApprovalSubmitError, PendingProviderApproval};
use agents::codex::{
    codex_provider_slash_commands, is_codex_no_rollout_resume_error,
    is_codex_thread_not_found_error, run_codex_turn_on_server, CodexAppServer, CodexTurnRequest,
    PendingCodexApproval, PendingCodexQuestion,
};
use agents::codex_protocol_coverage::{
    CodexProtocolCoverage, CodexProtocolDirection, CodexProtocolSupport, CODEX_PROTOCOL_COVERAGE,
};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::watch;
use tokio::sync::{mpsc, Mutex};
use tokio::time::{sleep, Duration};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use uuid::Uuid;
use wal::{RunnerWal, RunnerWalRecord};

const DEFAULT_CONTROL_PLANE_WS: &str = "ws://127.0.0.1:7777/api/runner/ws";
const DEFAULT_DEV_RUNNER_TOKEN: &str = "dev-runner-token";
const DEFAULT_RUNNER_WS_CHUNK_BYTES: usize = 1024 * 1024;
const DEFAULT_RUNNER_WS_MAX_MESSAGE_BYTES: usize = 512 * 1024 * 1024;

#[derive(Clone)]
struct CodexRunnerRuntime {
    workspace_path: PathBuf,
    sessions: Arc<Mutex<HashMap<SessionId, Arc<Mutex<CodexSessionRuntime>>>>>,
    turn_interrupt_senders: Arc<Mutex<HashMap<SessionId, watch::Sender<bool>>>>,
}

#[derive(Clone)]
struct RunnerOperationReporter {
    request_id: RequestId,
    sender: mpsc::UnboundedSender<(Option<RequestId>, RunnerEvent)>,
}

struct CodexSessionRuntime {
    server: CodexAppServer,
    live_thread_ids: HashSet<String>,
    turn_interrupt_rx: watch::Receiver<bool>,
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

async fn cancel_pending_provider_approvals_for_session(
    session_id: SessionId,
    approvals: Arc<Mutex<HashMap<ApprovalId, PendingProviderApproval>>>,
    provider_label: &'static str,
) -> usize {
    let candidates = {
        let approvals = approvals.lock().await;
        approvals
            .iter()
            .map(|(&approval_id, approval)| (approval_id, approval.clone()))
            .collect::<Vec<_>>()
    };
    let mut pending = Vec::new();
    for (approval_id, approval) in candidates {
        if approval.session_id().await == session_id && approval.is_live().await {
            pending.push((approval_id, approval));
        }
    }
    let mut cancelled = 0;
    for (approval_id, approval) in pending {
        match answer_pending_provider_approval(
            approval_id,
            ApprovalDecision::Cancel,
            approval,
            provider_label,
        )
        .await
        {
            RunnerResponseOutcome::Ok { .. } => cancelled += 1,
            RunnerResponseOutcome::Error { error } => {
                tracing::warn!(
                    %session_id,
                    %approval_id,
                    code = %error.code,
                    message = %error.message,
                    "failed to cancel blocked provider approval"
                );
            }
        }
    }
    cancelled
}

fn provider_cancel_unsupported(provider_label: &'static str) -> RunnerResponseOutcome {
    RunnerResponseOutcome::Error {
        error: RunnerError {
            code: "provider_cancel_not_supported".to_owned(),
            message: format!(
                "{provider_label} cannot interrupt the current turn in this runner path."
            ),
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
            turn_interrupt_senders: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn create_session(&self, session_id: SessionId) -> anyhow::Result<String> {
        let mut server = spawn_codex_server(self.workspace_path.clone()).await?;
        let thread_id = server.start_thread(&self.workspace_path).await?;
        let (turn_interrupt_tx, turn_interrupt_rx) = watch::channel(false);
        self.turn_interrupt_senders
            .lock()
            .await
            .insert(session_id, turn_interrupt_tx);
        let mut live_thread_ids = HashSet::new();
        live_thread_ids.insert(thread_id.clone());
        self.sessions.lock().await.insert(
            session_id,
            Arc::new(Mutex::new(CodexSessionRuntime {
                server,
                live_thread_ids,
                turn_interrupt_rx,
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
        let (turn_interrupt_tx, turn_interrupt_rx) = watch::channel(false);
        self.turn_interrupt_senders
            .lock()
            .await
            .insert(session_id, turn_interrupt_tx);
        let mut live_thread_ids = HashSet::new();
        live_thread_ids.insert(external_session_id.to_owned());
        self.sessions.lock().await.insert(
            session_id,
            Arc::new(Mutex::new(CodexSessionRuntime {
                server,
                live_thread_ids,
                turn_interrupt_rx,
            })),
        );
        Ok(external_session_id.to_owned())
    }

    async fn signal_turn_interrupt(&self, session_id: SessionId) -> bool {
        let Some(sender) = self
            .turn_interrupt_senders
            .lock()
            .await
            .get(&session_id)
            .cloned()
        else {
            return false;
        };
        sender.send(true).is_ok()
    }

    async fn turn_interrupt_sender(&self, session_id: SessionId) -> Option<watch::Sender<bool>> {
        self.turn_interrupt_senders
            .lock()
            .await
            .get(&session_id)
            .cloned()
    }

    async fn discover_sessions(
        &self,
        include_history: bool,
        reporter: Option<RunnerOperationReporter>,
    ) -> anyhow::Result<Vec<DiscoveredSession>> {
        if let Some(reporter) = &reporter {
            reporter.info(
                RunnerOperationStatus::Discovering,
                "Discovering sessions",
                None,
                Some("Listing Codex threads".to_owned()),
            );
        }
        let mut server = spawn_codex_server(self.workspace_path.clone()).await?;
        let threads = server.list_threads(&self.workspace_path).await?;
        let mut discovered = Vec::with_capacity(threads.len());
        let total = u64::try_from(threads.len()).ok();
        for (index, thread) in threads.into_iter().enumerate() {
            let (history_status, history) = if include_history {
                if let Some(reporter) = &reporter {
                    let current = u64::try_from(index).ok();
                    reporter.info(
                        RunnerOperationStatus::ReadingHistory,
                        "Reading Codex history",
                        Some(RunnerOperationProgress {
                            current,
                            total,
                            percent: percent_progress(current, total),
                        }),
                        Some(format!(
                            "Reading {}",
                            thread.title.as_deref().unwrap_or("session")
                        )),
                    );
                }
                match server
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
                }
            } else {
                (DiscoveredSessionHistoryStatus::NotLoaded, Vec::new())
            };
            discovered.push(DiscoveredSession {
                external_session_id: thread.external_session_id,
                title: thread.title,
                updated_at: thread.updated_at,
                history_status,
                history,
            });
        }
        if let Some(reporter) = &reporter {
            reporter.info(
                RunnerOperationStatus::SendingResults,
                "Sending refresh results",
                Some(RunnerOperationProgress {
                    current: total,
                    total,
                    percent: percent_progress(total, total),
                }),
                Some(format!("Sending {} discovered sessions", discovered.len())),
            );
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
        event_sender: mpsc::UnboundedSender<AdapterEvent>,
        pending_approvals: Arc<Mutex<HashMap<ApprovalId, PendingCodexApproval>>>,
        pending_questions: Arc<Mutex<HashMap<QuestionId, PendingCodexQuestion>>>,
    ) -> anyhow::Result<()> {
        let session_id = request.session_id;
        let result = async {
            let session_runtime = self
                .ensure_session_runtime(session_id, request.external_session_id.clone())
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
                                session_id = %session_id,
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
            let interrupt_rx = session_runtime.turn_interrupt_rx.clone();
            let Some(turn_interrupt_tx) = self.turn_interrupt_sender(session_id).await else {
                return Err(anyhow::anyhow!(
                    "codex turn interrupt signal was not available for session {session_id}"
                ));
            };
            let result = run_codex_turn_on_server(
                &mut session_runtime.server,
                request.clone(),
                event_sender.clone(),
                pending_approvals.clone(),
                pending_questions.clone(),
                turn_interrupt_tx.clone(),
                interrupt_rx.clone(),
            )
            .await;
            if let (Err(error), Some(thread_id)) = (&result, existing_thread_id.as_deref()) {
                if is_codex_thread_not_found_error(error.as_ref()) {
                    tracing::warn!(
                        session_id = %session_id,
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
                        turn_interrupt_tx,
                        interrupt_rx.clone(),
                    )
                    .await;
                }
            }
            result
        }
        .await;
        result
    }

    async fn shutdown_session(&self, session_id: SessionId) -> bool {
        self.turn_interrupt_senders.lock().await.remove(&session_id);
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

impl RunnerOperationReporter {
    fn info(
        &self,
        status: RunnerOperationStatus,
        stage_label: &str,
        progress: Option<RunnerOperationProgress>,
        message: Option<String>,
    ) {
        self.send(
            status,
            stage_label,
            progress,
            message,
            RunnerOperationLogLevel::Info,
        );
    }

    fn error(&self, status: RunnerOperationStatus, stage_label: &str, message: Option<String>) {
        self.send(
            status,
            stage_label,
            None,
            message,
            RunnerOperationLogLevel::Error,
        );
    }

    fn send(
        &self,
        status: RunnerOperationStatus,
        stage_label: &str,
        progress: Option<RunnerOperationProgress>,
        message: Option<String>,
        level: RunnerOperationLogLevel,
    ) {
        self.sender
            .send((
                Some(self.request_id.clone()),
                RunnerEvent::OperationUpdated(RunnerOperationUpdate {
                    operation_id: self.request_id.clone(),
                    kind: RunnerOperationKind::SessionRefresh,
                    status,
                    stage_label: stage_label.to_owned(),
                    progress,
                    message,
                    level,
                    ts: None,
                }),
            ))
            .ok();
    }
}

fn percent_progress(current: Option<u64>, total: Option<u64>) -> Option<u8> {
    let (Some(current), Some(total)) = (current, total) else {
        return None;
    };
    if total == 0 {
        return Some(100);
    }
    Some(((current.saturating_mul(100)) / total).min(100) as u8)
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
    let hello_template = codex_hello(token, workspace_path.clone());
    let wal = RunnerWal::open(runner_wal_path(hello_template.runner_id, &workspace_path)).await?;
    tracing::info!(
        url = %url,
        runner_id = %hello_template.runner_id,
        workspace = %workspace_path.display(),
        "starting reconnect-stable codex runner"
    );
    let advertised_workspace = hello_template.workspaces.first().cloned();

    let pending_approvals: Arc<Mutex<HashMap<ApprovalId, PendingCodexApproval>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let pending_questions: Arc<Mutex<HashMap<QuestionId, PendingCodexQuestion>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let (codex_event_sender, mut codex_event_receiver) = mpsc::unbounded_channel::<AdapterEvent>();
    let (background_event_sender, mut background_event_receiver) =
        mpsc::unbounded_channel::<(Option<RequestId>, RunnerEvent)>();
    let codex_runtime = CodexRunnerRuntime::new(workspace_path.clone());
    if let Some(workspace) = advertised_workspace.clone() {
        let runtime = codex_runtime.clone();
        let sender = background_event_sender.clone();
        tokio::spawn(async move {
            match runtime.discover_sessions(false, None).await {
                Ok(sessions) if !sessions.is_empty() => {
                    tracing::info!(
                        session_count = sessions.len(),
                        "sending discovered codex session metadata to control plane"
                    );
                    sender
                        .send((
                            None,
                            RunnerEvent::SessionsDiscovered(DiscoveredSessions {
                                workspace,
                                provider_id: AgentProviderId::from(AgentProviderId::CODEX),
                                sessions,
                            }),
                        ))
                        .ok();
                }
                Ok(_) => {
                    tracing::debug!("codex discovery found no native threads");
                }
                Err(error) => {
                    tracing::warn!(%error, "codex native session discovery failed");
                }
            }
        });
    }

    loop {
        tracing::info!(url = %url, runner_id = %hello_template.runner_id, "connecting codex runner to control plane");
        let (socket, _) = match connect_async(&url).await {
            Ok(socket) => socket,
            Err(error) => {
                tracing::warn!(%error, "codex runner websocket connect failed; retrying");
                sleep(runner_reconnect_delay()).await;
                continue;
            }
        };
        let (mut sender, mut receiver) = socket.split();
        let mut server_message_reassembler =
            RunnerTransportChunkReassembler::new(runner_ws_max_message_bytes());
        let mut hello = hello_template.clone();
        let acked = wal.acked_seq().await;
        hello.acked_runner_event_seq = (acked > 0).then_some(acked);
        hello.replay_from_runner_event_seq = wal
            .unacked()
            .await
            .first()
            .map(|record| record.runner_event_seq);
        tracing::info!(runner_id = %hello.runner_id, "sending codex runner hello");
        if let Err(error) =
            send_runner_message(&mut sender, RunnerClientMessage::Hello(hello)).await
        {
            tracing::warn!(%error, "failed to send codex runner hello; reconnecting");
            sleep(runner_reconnect_delay()).await;
            continue;
        }
        if let Err(error) = replay_unacked_wal(&mut sender, &wal).await {
            tracing::warn!(%error, "failed to replay codex runner WAL; reconnecting");
            sleep(runner_reconnect_delay()).await;
            continue;
        }

        let transport_result: anyhow::Result<()> = async {
            loop {
        tokio::select! {
            background = background_event_receiver.recv() => {
                let Some((request_id, event)) = background else {
                    continue;
                };
                if let Err(error) = send_wal_event(
                    &mut sender,
                    &wal,
                    request_id,
                    None,
                    event,
                )
                .await {
                    tracing::warn!(%error, "failed to send codex background event; reconnecting");
                    break;
                }
            }
            event = codex_event_receiver.recv() => {
                let Some(event) = event else {
                    continue;
                };
                let Some(agent_event) = event.universal_projection_for_wal() else {
                    continue;
                };
                let session_id = agent_event.session_id;
                if let Err(error) = send_wal_event(
                    &mut sender,
                    &wal,
                    None,
                    Some(session_id),
                    RunnerEvent::AgentEvent(Box::new(agent_event)),
                )
                .await {
                    tracing::warn!(%error, "failed to send codex agent event; reconnecting");
                    break;
                }
            }
            message = receiver.next() => {
                let Some(message) = message else {
                    tracing::info!("control plane websocket closed for codex runner");
                    break;
                };
                let Message::Text(text) = message? else {
                    continue;
                };
                let Some(message) = next_runner_server_message(&mut server_message_reassembler, &text)? else {
                    continue;
                };
                let Some(envelope) = handle_runner_server_message(&wal, message).await else {
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
                        let request_id = envelope.request_id.clone();
                        let runtime = codex_runtime.clone();
                        let background_sender = background_event_sender.clone();
                        tokio::spawn(async move {
                            let reporter = RunnerOperationReporter {
                                request_id: request_id.clone(),
                                sender: background_sender.clone(),
                            };
                            reporter.info(
                                RunnerOperationStatus::Accepted,
                                "Refresh accepted",
                                None,
                                Some("Codex refresh task started".to_owned()),
                            );
                            let event = match runtime.discover_sessions(true, Some(reporter.clone())).await {
                                Ok(sessions) => RunnerEvent::SessionsDiscovered(DiscoveredSessions {
                                    workspace: command.workspace,
                                    provider_id: command.provider_id,
                                    sessions,
                                }),
                                Err(error) => {
                                    reporter.error(
                                        RunnerOperationStatus::Failed,
                                        "Refresh failed",
                                        Some(error.to_string()),
                                    );
                                    RunnerEvent::Error(runner_error(
                                        "codex_refresh_sessions_failed",
                                        error,
                                    ))
                                }
                            };
                            background_sender.send((Some(request_id), event)).ok();
                        });
                        let outcome = RunnerResponseOutcome::Ok {
                            result: RunnerCommandResult::Accepted,
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
                                event_sender.send(AdapterEvent::session_status(
                                    AgentProviderId::from(AgentProviderId::CODEX),
                                    "codex-app-server",
                                    None,
                                    session_id,
                                    SessionStatus::Failed,
                                    Some(error.to_string()),
                                )).ok();
                                event_sender.send(AdapterEvent::error(
                                    AgentProviderId::from(AgentProviderId::CODEX),
                                    "codex-app-server",
                                    None,
                                    session_id,
                                    Some("codex_adapter_error".to_owned()),
                                    error.to_string(),
                                )).ok();
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
                    RunnerCommand::InterruptSession { session_id } => {
                        let interrupt_signaled =
                            codex_runtime.signal_turn_interrupt(session_id).await;
                        let cancelled = cancel_pending_provider_approvals_for_session(
                            session_id,
                            pending_approvals.clone(),
                            "Codex",
                        )
                        .await;
                        let outcome = if interrupt_signaled || cancelled > 0 {
                            RunnerResponseOutcome::Ok {
                                result: RunnerCommandResult::Accepted,
                            }
                        } else {
                            provider_cancel_unsupported("Codex")
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
                    RunnerCommand::ShutdownSession(command) => {
                        tracing::info!(session_id = %command.session_id, request_id = %envelope.request_id, "codex runner shutting down session runtime");
                        codex_runtime.shutdown_session(command.session_id).await;
                        codex_event_sender
                            .send(AdapterEvent::session_status(
                                AgentProviderId::from(AgentProviderId::CODEX),
                                "codex-app-server",
                                None,
                                command.session_id,
                                SessionStatus::Stopped,
                                Some("Codex session runtime stopped.".to_owned()),
                            ))
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

            #[allow(unreachable_code)]
            Ok(())
        }
        .await;
        if let Err(error) = transport_result {
            tracing::warn!(%error, "codex runner transport session failed; reconnecting");
        }

        sleep(runner_reconnect_delay()).await;
    }

    #[allow(unreachable_code)]
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
    let hello_template = acp_hello(token, workspace_path.clone(), &profiles, include_codex);
    let wal = RunnerWal::open(runner_wal_path(hello_template.runner_id, &workspace_path)).await?;
    let advertised_workspace = hello_template.workspaces.first().cloned();
    tracing::info!(
        url = %url,
        runner_id = %hello_template.runner_id,
        workspace = %workspace_path.display(),
        provider_count = profiles.len(),
        include_codex,
        "starting reconnect-stable multi-provider runner"
    );

    let profiles_by_id = profiles
        .into_iter()
        .map(|profile| (profile.provider_id.clone(), profile))
        .collect::<HashMap<_, _>>();
    let mut adapter_runtime = AdapterRuntime::new();
    for profile in profiles_by_id.values() {
        adapter_runtime.register_provider(AdapterProviderRegistration {
            provider_id: profile.provider_id.clone(),
            capabilities: profile.advertised_capabilities().into(),
        });
    }
    if include_codex {
        adapter_runtime.register_provider(AdapterProviderRegistration {
            provider_id: AgentProviderId::from(AgentProviderId::CODEX),
            capabilities: codex_agent_capabilities(true, true).into(),
        });
    }
    let mut session_profiles = HashMap::<SessionId, AgentProviderId>::new();
    let pending_approvals: Arc<Mutex<HashMap<ApprovalId, PendingAcpApproval>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let pending_codex_approvals: Arc<Mutex<HashMap<ApprovalId, PendingCodexApproval>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let pending_codex_questions: Arc<Mutex<HashMap<QuestionId, PendingCodexQuestion>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let (acp_event_sender, mut acp_event_receiver) = mpsc::unbounded_channel::<AdapterEvent>();
    let (codex_event_sender, mut codex_event_receiver) = mpsc::unbounded_channel::<AdapterEvent>();
    let (background_event_sender, mut background_event_receiver) =
        mpsc::unbounded_channel::<(Option<RequestId>, RunnerEvent)>();
    let acp_runtime = AcpRunnerRuntime::new(workspace_path.clone());
    let codex_runtime = include_codex.then(|| CodexRunnerRuntime::new(workspace_path.clone()));

    loop {
        tracing::info!(url = %url, runner_id = %hello_template.runner_id, "connecting multi-provider runner to control plane");
        let (socket, _) = match connect_async(&url).await {
            Ok(socket) => socket,
            Err(error) => {
                tracing::warn!(%error, "multi-provider runner websocket connect failed; retrying");
                sleep(runner_reconnect_delay()).await;
                continue;
            }
        };
        let (mut sender, mut receiver) = socket.split();
        let mut server_message_reassembler =
            RunnerTransportChunkReassembler::new(runner_ws_max_message_bytes());
        let mut hello = hello_template.clone();
        let acked = wal.acked_seq().await;
        hello.acked_runner_event_seq = (acked > 0).then_some(acked);
        hello.replay_from_runner_event_seq = wal
            .unacked()
            .await
            .first()
            .map(|record| record.runner_event_seq);
        tracing::info!(runner_id = %hello.runner_id, "sending ACP runner hello");
        if let Err(error) =
            send_runner_message(&mut sender, RunnerClientMessage::Hello(hello)).await
        {
            tracing::warn!(%error, "failed to send multi-provider runner hello; reconnecting");
            sleep(runner_reconnect_delay()).await;
            continue;
        }
        if let Err(error) = replay_unacked_wal(&mut sender, &wal).await {
            tracing::warn!(%error, "failed to replay multi-provider runner WAL; reconnecting");
            sleep(runner_reconnect_delay()).await;
            continue;
        }

        let transport_result: anyhow::Result<()> = async {
            loop {
        tokio::select! {
            background = background_event_receiver.recv() => {
                let Some((request_id, event)) = background else {
                    continue;
                };
                if let Err(error) = send_wal_event(
                    &mut sender,
                    &wal,
                    request_id,
                    None,
                    event,
                )
                .await {
                    tracing::warn!(%error, "failed to send multi-provider background event; reconnecting");
                    break;
                }
            }
            event = acp_event_receiver.recv() => {
                let Some(adapter_event) = event else {
                    continue;
                };
                let Some(agent_event) = adapter_event.universal_projection_for_wal() else {
                    continue;
                };
                let session_id = agent_event.session_id;
                if let Err(error) = send_wal_event(
                    &mut sender,
                    &wal,
                    None,
                    Some(session_id),
                    RunnerEvent::AgentEvent(Box::new(agent_event)),
                )
                .await {
                    tracing::warn!(%error, "failed to send ACP agent event; reconnecting");
                    break;
                }
            }
            event = codex_event_receiver.recv() => {
                let Some(event) = event else {
                    continue;
                };
                let Some(agent_event) = event.universal_projection_for_wal() else {
                    continue;
                };
                let session_id = agent_event.session_id;
                if let Err(error) = send_wal_event(
                    &mut sender,
                    &wal,
                    None,
                    Some(session_id),
                    RunnerEvent::AgentEvent(Box::new(agent_event)),
                )
                .await {
                    tracing::warn!(%error, "failed to send Codex agent event from multi-provider runner; reconnecting");
                    break;
                }
            }
            message = receiver.next() => {
                let Some(message) = message else {
                    tracing::info!("control plane websocket closed for ACP runner");
                    break;
                };
                let Message::Text(text) = message? else {
                    continue;
                };
                let Some(message) = next_runner_server_message(&mut server_message_reassembler, &text)? else {
                    continue;
                };
                let Some(envelope) = handle_runner_server_message(&wal, message).await else {
                    continue;
                };

                match envelope.command {
                    RunnerCommand::CreateSession(command) => {
                        tracing::info!(session_id = %command.session_id, request_id = %envelope.request_id, provider_id = %command.provider_id, "multi-provider runner received create session");
                        if command.provider_id.as_str() == AgentProviderId::CODEX {
                            let outcome = match &codex_runtime {
                                Some(runtime) => match runtime.create_session(command.session_id).await {
                                    Ok(external_session_id) => {
                                        adapter_runtime.bind_session(command.session_id, command.provider_id.clone());
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
                                adapter_runtime.bind_session(command.session_id, command.provider_id.clone());
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
                                        adapter_runtime.bind_session(command.session_id, command.provider_id.clone());
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
                                adapter_runtime.bind_session(command.session_id, command.provider_id.clone());
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
                                Some(runtime) => {
                                    let request_id = envelope.request_id.clone();
                                    let runtime = runtime.clone();
                                    let background_sender = background_event_sender.clone();
                                    tokio::spawn(async move {
                                        let reporter = RunnerOperationReporter {
                                            request_id: request_id.clone(),
                                            sender: background_sender.clone(),
                                        };
                                        reporter.info(
                                            RunnerOperationStatus::Accepted,
                                            "Refresh accepted",
                                            None,
                                            Some("Codex refresh task started".to_owned()),
                                        );
                                        let event = match runtime
                                            .discover_sessions(true, Some(reporter.clone()))
                                            .await
                                        {
                                            Ok(sessions) => {
                                                RunnerEvent::SessionsDiscovered(DiscoveredSessions {
                                                    workspace: command.workspace,
                                                    provider_id: command.provider_id,
                                                    sessions,
                                                })
                                            }
                                            Err(error) => {
                                                reporter.error(
                                                    RunnerOperationStatus::Failed,
                                                    "Refresh failed",
                                                    Some(error.to_string()),
                                                );
                                                RunnerEvent::Error(runner_error(
                                                    "codex_refresh_sessions_failed",
                                                    error,
                                                ))
                                            }
                                        };
                                        background_sender.send((Some(request_id), event)).ok();
                                    });
                                    RunnerResponseOutcome::Ok {
                                        result: RunnerCommandResult::Accepted,
                                    }
                                }
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
                        let request_id = envelope.request_id.clone();
                        let background_sender = background_event_sender.clone();
                        let runtime = acp_runtime.clone();
                        tokio::spawn(async move {
                            let reporter = RunnerOperationReporter {
                                request_id: request_id.clone(),
                                sender: background_sender.clone(),
                            };
                            reporter.info(
                                RunnerOperationStatus::Accepted,
                                "Refresh accepted",
                                None,
                                Some(format!("{} refresh task started", command.provider_id)),
                            );
                            reporter.info(
                                RunnerOperationStatus::Discovering,
                                "Discovering sessions",
                                None,
                                Some("Listing ACP sessions".to_owned()),
                            );
                            let event = match runtime.discover_sessions(profile).await {
                                Ok(sessions) => {
                                    reporter.info(
                                        RunnerOperationStatus::SendingResults,
                                        "Sending refresh results",
                                        None,
                                        Some(format!("Sending {} discovered sessions", sessions.len())),
                                    );
                                    RunnerEvent::SessionsDiscovered(DiscoveredSessions {
                                        workspace: command.workspace,
                                        provider_id: command.provider_id,
                                        sessions,
                                    })
                                }
                                Err(error) => {
                                    reporter.error(
                                        RunnerOperationStatus::Failed,
                                        "Refresh failed",
                                        Some(error.to_string()),
                                    );
                                    RunnerEvent::Error(runner_error(
                                        "acp_refresh_sessions_failed",
                                        error,
                                    ))
                                }
                            };
                            background_sender.send((Some(request_id), event)).ok();
                        });
                        let outcome = RunnerResponseOutcome::Ok {
                            result: RunnerCommandResult::Accepted,
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
                        let provider_id = adapter_runtime
                            .resolve_provider(Some(command.session_id), command.provider_id.as_ref())
                            .map(|registration| registration.provider_id.clone())
                            .or_else(|| session_profiles.get(&command.session_id).cloned())
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
                            adapter_runtime.bind_session(command.session_id, provider_id.clone());
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
                            let event_sender = codex_event_sender.clone();
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
                                        .send(AdapterEvent::session_status(
                                            AgentProviderId::from(AgentProviderId::CODEX),
                                            "codex-app-server",
                                            None,
                                            session_id,
                                            SessionStatus::Failed,
                                            Some(error.to_string()),
                                        ))
                                        .ok();
                                    event_sender
                                        .send(AdapterEvent::error(
                                            AgentProviderId::from(AgentProviderId::CODEX),
                                            "codex-app-server",
                                            None,
                                            session_id,
                                            Some("codex_adapter_error".to_owned()),
                                            error.to_string(),
                                        ))
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
                        adapter_runtime.bind_session(command.session_id, provider_id.clone());
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
                                event_sender
                                    .send(AdapterEvent::session_status(
                                        AgentProviderId::from("acp"),
                                        "acp-stdio",
                                        None,
                                        session_id,
                                        SessionStatus::Failed,
                                        Some(error.to_string()),
                                    ))
                                    .ok();
                                event_sender
                                    .send(AdapterEvent::error(
                                        AgentProviderId::from("acp"),
                                        "acp-stdio",
                                        None,
                                        session_id,
                                        Some("acp_adapter_error".to_owned()),
                                        error.to_string(),
                                    ))
                                    .ok();
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
                    RunnerCommand::InterruptSession { session_id } => {
                        let mut interrupt_signaled = false;
                        if let Some(runtime) = &codex_runtime {
                            interrupt_signaled = runtime.signal_turn_interrupt(session_id).await;
                        }
                        let acp_cancelled = cancel_pending_provider_approvals_for_session(
                            session_id,
                            pending_approvals.clone(),
                            "ACP",
                        )
                        .await;
                        let codex_cancelled = cancel_pending_provider_approvals_for_session(
                            session_id,
                            pending_codex_approvals.clone(),
                            "Codex",
                        )
                        .await;
                        let outcome = if interrupt_signaled || acp_cancelled + codex_cancelled > 0 {
                            RunnerResponseOutcome::Ok {
                                result: RunnerCommandResult::Accepted,
                            }
                        } else {
                            provider_cancel_unsupported("provider")
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
                    RunnerCommand::ShutdownSession(command) => {
                        acp_runtime.shutdown_session(command.session_id).await;
                        if let Some(runtime) = &codex_runtime {
                            runtime.shutdown_session(command.session_id).await;
                        }
                        adapter_runtime.unbind_session(command.session_id);
                        acp_event_sender
                            .send(AdapterEvent::session_status(
                                AgentProviderId::from("acp"),
                                "acp-stdio",
                                None,
                                command.session_id,
                                SessionStatus::Stopped,
                                Some("ACP session runtime stopped.".to_owned()),
                            ))
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

            #[allow(unreachable_code)]
            Ok(())
        }
        .await;

        if let Some(ref workspace) = advertised_workspace {
            tracing::debug!(workspace = %workspace.path, "ACP runner closed");
        }
        if let Err(error) = transport_result {
            tracing::warn!(%error, "multi-provider runner transport session failed; reconnecting");
        }
        sleep(runner_reconnect_delay()).await;
    }

    #[allow(unreachable_code)]
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

    let mut hello = fake_hello(token);
    let workspace_path = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let wal = RunnerWal::open(runner_wal_path(hello.runner_id, &workspace_path)).await?;
    let acked = wal.acked_seq().await;
    hello.acked_runner_event_seq = (acked > 0).then_some(acked);
    hello.replay_from_runner_event_seq = wal
        .unacked()
        .await
        .first()
        .map(|record| record.runner_event_seq);
    tracing::info!(runner_id = %hello.runner_id, "sending fake runner hello");
    send_runner_message(&mut sender, RunnerClientMessage::Hello(hello)).await?;
    replay_unacked_wal(&mut sender, &wal).await?;

    while let Some(message) = receiver.next().await {
        let Message::Text(text) = message? else {
            continue;
        };
        let Some(message) = next_runner_server_message(&mut server_message_reassembler, &text)?
        else {
            continue;
        };
        let Some(envelope) = handle_runner_server_message(&wal, message).await else {
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
                    let Some(agent_event) = AdapterEvent::from_normalized_event(
                        AgentProviderId::from("fake"),
                        "fake-runner",
                        None,
                        event,
                    )
                    .universal_projection_for_wal() else {
                        continue;
                    };
                    send_wal_event(
                        &mut sender,
                        &wal,
                        Some(envelope.request_id.clone()),
                        Some(command.session_id),
                        RunnerEvent::AgentEvent(Box::new(agent_event)),
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

async fn send_wal_event(
    sender: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    wal: &RunnerWal,
    request_id: Option<agenter_protocol::RequestId>,
    session_id: Option<SessionId>,
    event: RunnerEvent,
) -> anyhow::Result<()> {
    if !wal::event_is_replayable(&event) {
        return send_runner_event(sender, request_id, None, event).await;
    }
    let record = wal.append(request_id, session_id, event).await?;
    send_wal_record(sender, wal, &record).await
}

async fn send_runner_event(
    sender: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    request_id: Option<agenter_protocol::RequestId>,
    acked_runner_event_seq: Option<u64>,
    event: RunnerEvent,
) -> anyhow::Result<()> {
    send_runner_message(
        sender,
        RunnerClientMessage::Event(Box::new(RunnerEventEnvelope {
            request_id,
            runner_event_seq: None,
            acked_runner_event_seq,
            event,
        })),
    )
    .await
}

async fn send_wal_record(
    sender: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    wal: &RunnerWal,
    record: &RunnerWalRecord,
) -> anyhow::Result<()> {
    let acked = wal.acked_seq().await;
    send_runner_message(
        sender,
        RunnerClientMessage::Event(Box::new(RunnerEventEnvelope {
            request_id: record.request_id.clone(),
            runner_event_seq: Some(record.runner_event_seq),
            acked_runner_event_seq: (acked > 0).then_some(acked),
            event: record.event.clone(),
        })),
    )
    .await
}

async fn replay_unacked_wal(
    sender: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    wal: &RunnerWal,
) -> anyhow::Result<()> {
    for record in wal.unacked().await {
        send_wal_record(sender, wal, &record).await?;
    }
    Ok(())
}

fn runner_wal_path(runner_id: RunnerId, workspace_path: &Path) -> PathBuf {
    env::var("AGENTER_RUNNER_WAL")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            workspace_path
                .join(".agenter")
                .join(format!("runner-{runner_id}-events.jsonl"))
        })
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

async fn handle_runner_server_message(
    wal: &RunnerWal,
    message: RunnerServerMessage,
) -> Option<Box<agenter_protocol::runner::RunnerCommandEnvelope>> {
    match message {
        RunnerServerMessage::Command(envelope) => Some(envelope),
        RunnerServerMessage::EventAck(ack) => {
            if let Err(error) = wal.ack(ack.runner_event_seq).await {
                tracing::warn!(
                    runner_event_seq = ack.runner_event_seq,
                    %error,
                    "failed to persist runner event ack"
                );
            }
            None
        }
        RunnerServerMessage::HeartbeatAck(_) => None,
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

fn runner_reconnect_delay() -> Duration {
    Duration::from_secs(env_usize("AGENTER_RUNNER_RECONNECT_SECONDS", 2) as u64)
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

fn codex_agent_capabilities(session_resume: bool, interrupt: bool) -> AgentCapabilities {
    AgentCapabilities {
        streaming: true,
        approvals: true,
        file_changes: true,
        command_execution: true,
        session_resume,
        interrupt,
        model_selection: session_resume,
        reasoning_effort: session_resume,
        collaboration_modes: session_resume,
        tool_user_input: session_resume,
        mcp_elicitation: session_resume,
        provider_details: codex_provider_capability_details(),
        ..AgentCapabilities::default()
    }
}

fn codex_provider_capability_details() -> Vec<ProviderCapabilityDetail> {
    const FAMILIES: &[CodexCapabilityFamily] = &[
        CodexCapabilityFamily {
            key: "thread_lifecycle_history",
            reason: "Core thread start, resume, fork, archive, history read, and selected metadata commands are supported; advanced metadata and memory operations remain deferred.",
            matches: |entry| {
                entry.method.starts_with("thread/")
                    && !entry.method.starts_with("thread/realtime/")
                    && !matches!(
                        entry.method,
                        "turn/start" | "turn/steer" | "turn/interrupt"
                    )
            },
        },
        CodexCapabilityFamily {
            key: "turn_control",
            reason: "Turn start, steer, interrupt, lifecycle notifications, plans, and diffs are supported.",
            matches: |entry| {
                entry.method.starts_with("turn/")
                    || matches!(
                        entry.method,
                        "review/start"
                            | "item/agentMessage/delta"
                            | "item/plan/delta"
                            | "item/reasoning/summaryTextDelta"
                            | "item/reasoning/summaryPartAdded"
                            | "item/reasoning/textDelta"
                    )
            },
        },
        CodexCapabilityFamily {
            key: "approvals_questions",
            reason: "Codex approval and input server requests are routed through Agenter; dynamic tools and account refresh are visible capability gaps.",
            matches: |entry| {
                entry.direction == CodexProtocolDirection::ServerRequest
                    || entry.method == "serverRequest/resolved"
                    || entry.method.contains("Approval")
                    || entry.method.contains("requestApproval")
                    || entry.method.contains("requestUserInput")
                    || entry.method.contains("elicitation")
            },
        },
        CodexCapabilityFamily {
            key: "dynamic_tools",
            reason: "Dynamic client-side tool execution is visible but not executed remotely.",
            matches: |entry| entry.method.starts_with("item/tool/"),
        },
        CodexCapabilityFamily {
            key: "mcp",
            reason: "MCP elicitation is supported; status, progress, OAuth, and reload surfaces are still partial.",
            matches: |entry| entry.method.starts_with("mcpServer/")
                || entry.method.starts_with("item/mcpToolCall/")
                || entry.method == "mcpServerStatus/list"
                || entry.method == "config/mcpServer/reload",
        },
        CodexCapabilityFamily {
            key: "realtime",
            reason: "Codex realtime sessions are not exposed by the remote runner yet.",
            matches: |entry| entry.method.starts_with("thread/realtime/"),
        },
        CodexCapabilityFamily {
            key: "fuzzy_search",
            reason: "Fuzzy file search is a local TUI affordance, not a remote runner surface.",
            matches: |entry| entry.method.starts_with("fuzzyFileSearch/"),
        },
        CodexCapabilityFamily {
            key: "usage_account",
            reason: "Usage and rate-limit reads are surfaced; provider login, logout, account mutation, and billing nudges remain runner-host-local or out of scope.",
            matches: |entry| entry.method.starts_with("account/"),
        },
        CodexCapabilityFamily {
            key: "filesystem",
            reason: "File-change projection is supported; direct remote filesystem mutation and watch APIs need an approved design.",
            matches: |entry| {
                entry.method.starts_with("fs/")
                    || entry.method.starts_with("item/fileChange/")
                    || entry.method == "applyPatchApproval"
            },
        },
        CodexCapabilityFamily {
            key: "config",
            reason: "Config warnings are visible; remote config mutation/import commands are not exposed yet.",
            matches: |entry| {
                entry.method.starts_with("config/")
                    || entry.method.starts_with("externalAgentConfig/")
                    || entry.method == "configWarning"
            },
        },
        CodexCapabilityFamily {
            key: "plugins_marketplace_skills",
            reason: "Plugin, marketplace, app, and skills inventory is not exposed as a supported remote management surface; mutation is unsupported.",
            matches: |entry| {
                entry.method.starts_with("plugin/")
                    || entry.method.starts_with("marketplace/")
                    || entry.method.starts_with("app/")
                    || entry.method.starts_with("skills/")
            },
        },
        CodexCapabilityFamily {
            key: "command_exec",
            reason: "Turn-owned command execution is supported; one-off terminal sessions are not exposed remotely.",
            matches: |entry| {
                entry.method.starts_with("command/exec")
                    || entry.method.starts_with("item/commandExecution/")
                    || entry.method == "execCommandApproval"
            },
        },
        CodexCapabilityFamily {
            key: "device_keys",
            reason: "Device-key operations are not exposed through Agenter's remote browser surface.",
            matches: |entry| entry.method.starts_with("device/key/"),
        },
        CodexCapabilityFamily {
            key: "feedback",
            reason: "Provider feedback upload and billing nudges are outside Agenter's remote runner scope.",
            matches: |entry| {
                entry.method == "feedback/upload"
                    || entry.method == "account/sendAddCreditsNudgeEmail"
            },
        },
        CodexCapabilityFamily {
            key: "server_requests",
            reason: "Approval and input server requests are supported; dynamic tools and account refresh are degraded.",
            matches: |entry| entry.direction == CodexProtocolDirection::ServerRequest,
        },
        CodexCapabilityFamily {
            key: "notifications",
            reason: "High-value Codex notifications are projected; remaining notifications are native, deferred, or local-only.",
            matches: |entry| entry.direction == CodexProtocolDirection::ServerNotification,
        },
        CodexCapabilityFamily {
            key: "deprecated_apis",
            reason: "Deprecated Codex APIs are only retained where needed for legacy approvals or compatibility.",
            matches: |entry| {
                matches!(
                    entry.method,
                    "getConversationSummary"
                        | "gitDiffToRemote"
                        | "getAuthStatus"
                        | "fuzzyFileSearch"
                        | "applyPatchApproval"
                        | "execCommandApproval"
                        | "thread/compacted"
                )
            },
        },
    ];

    FAMILIES.iter().map(codex_capability_detail).collect()
}

struct CodexCapabilityFamily {
    key: &'static str,
    reason: &'static str,
    matches: fn(&CodexProtocolCoverage) -> bool,
}

fn codex_capability_detail(family: &CodexCapabilityFamily) -> ProviderCapabilityDetail {
    let entries = CODEX_PROTOCOL_COVERAGE
        .iter()
        .filter(|entry| (family.matches)(entry))
        .collect::<Vec<_>>();
    ProviderCapabilityDetail {
        key: family.key.to_owned(),
        status: provider_capability_status(&entries),
        methods: entries
            .iter()
            .map(|entry| entry.method.to_owned())
            .collect::<Vec<_>>(),
        reason: Some(family.reason.to_owned()),
    }
}

fn provider_capability_status(entries: &[&CodexProtocolCoverage]) -> ProviderCapabilityStatus {
    let mut has_supported = false;
    let mut has_degraded = false;
    let mut has_unsupported = false;
    let mut has_not_applicable = false;

    for entry in entries {
        match entry.support {
            CodexProtocolSupport::Supported => has_supported = true,
            CodexProtocolSupport::Degraded => has_degraded = true,
            CodexProtocolSupport::Unsupported | CodexProtocolSupport::Deferred => {
                has_unsupported = true;
            }
            CodexProtocolSupport::Ignored | CodexProtocolSupport::NotApplicable => {
                has_not_applicable = true;
            }
        }
    }

    if has_degraded || (has_supported && (has_unsupported || has_not_applicable)) {
        ProviderCapabilityStatus::Degraded
    } else if has_supported {
        ProviderCapabilityStatus::Supported
    } else if has_unsupported {
        ProviderCapabilityStatus::Unsupported
    } else {
        ProviderCapabilityStatus::NotApplicable
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
                capabilities: codex_agent_capabilities(false, false),
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

fn codex_hello(token: String, workspace_path: PathBuf) -> RunnerHello {
    provider_hello(
        token,
        workspace_path,
        AgentProviderId::from(AgentProviderId::CODEX),
        "codex-app-server",
        "codex workspace",
        true,
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
            capabilities: codex_agent_capabilities(true, true),
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
        acked_runner_event_seq: None,
        replay_from_runner_event_seq: None,
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
    interrupt: bool,
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
                capabilities: codex_agent_capabilities(session_resume, interrupt),
            }],
            transports: vec![transport.to_owned()],
            workspace_discovery: false,
        },
        acked_runner_event_seq: None,
        replay_from_runner_event_seq: None,
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

fn deterministic_fake_events(session_id: SessionId, input: &AgentInput) -> Vec<NormalizedEvent> {
    let content = match input {
        AgentInput::Text { text } => text.clone(),
        AgentInput::UserMessage { payload } => payload.content.clone(),
    };
    let response = format!("fake runner received: {content}");

    vec![
        NormalizedEvent::UserMessage(UserMessageEvent {
            session_id,
            message_id: Some("fake-user-1".to_owned()),
            author_user_id: None,
            content,
        }),
        NormalizedEvent::CommandStarted(CommandEvent {
            session_id,
            command_id: "fake-command-1".to_owned(),
            command: "printf fake-runner".to_owned(),
            cwd: Some(".".to_owned()),
            source: None,
            process_id: None,
            actions: Vec::new(),
            provider_payload: None,
        }),
        NormalizedEvent::CommandOutputDelta(CommandOutputEvent {
            session_id,
            command_id: "fake-command-1".to_owned(),
            stream: CommandOutputStream::Stdout,
            delta: "fake-runner\n".to_owned(),
            provider_payload: None,
        }),
        NormalizedEvent::CommandCompleted(CommandCompletedEvent {
            session_id,
            command_id: "fake-command-1".to_owned(),
            exit_code: Some(0),
            duration_ms: None,
            success: true,
            provider_payload: None,
        }),
        NormalizedEvent::ToolStarted(ToolEvent {
            session_id,
            tool_call_id: "fake-tool-1".to_owned(),
            name: "fake_lookup".to_owned(),
            title: Some("Fake lookup".to_owned()),
            input: Some(serde_json::json!({ "query": response.clone() })),
            output: None,
            provider_payload: None,
        }),
        NormalizedEvent::ToolCompleted(ToolEvent {
            session_id,
            tool_call_id: "fake-tool-1".to_owned(),
            name: "fake_lookup".to_owned(),
            title: Some("Fake lookup".to_owned()),
            input: None,
            output: Some(serde_json::json!({ "ok": true })),
            provider_payload: None,
        }),
        NormalizedEvent::FileChangeProposed(FileChangeEvent {
            session_id,
            path: "fake-output.txt".to_owned(),
            change_kind: FileChangeKind::Modify,
            diff: Some("-old\n+fake runner output\n".to_owned()),
            provider_payload: None,
        }),
        NormalizedEvent::FileChangeApplied(FileChangeEvent {
            session_id,
            path: "fake-output.txt".to_owned(),
            change_kind: FileChangeKind::Modify,
            diff: None,
            provider_payload: None,
        }),
        NormalizedEvent::ApprovalRequested(ApprovalRequestEvent {
            session_id,
            approval_id: ApprovalId::from_uuid(Uuid::from_u128(0x44444444444444444444444444444444)),
            kind: ApprovalKind::Command,
            title: "Approve fake command".to_owned(),
            details: Some("This is an in-memory approval stub.".to_owned()),
            expires_at: None,
            presentation: None,
            resolution_state: None,
            resolving_decision: None,
            status: None,
            turn_id: None,
            item_id: None,
            options: agenter_core::ApprovalOption::canonical_defaults(),
            risk: Some("medium".to_owned()),
            subject: Some("This is an in-memory approval stub.".to_owned()),
            native_request_id: Some("fake-approval".to_owned()),
            native_blocking: true,
            policy: None,
            provider_payload: None,
        }),
        NormalizedEvent::AgentMessageDelta(AgentMessageDeltaEvent {
            session_id,
            message_id: "fake-agent-1".to_owned(),
            delta: response.clone(),
            provider_payload: None,
        }),
        NormalizedEvent::AgentMessageCompleted(MessageCompletedEvent {
            session_id,
            message_id: "fake-agent-1".to_owned(),
            content: Some(response),
            provider_payload: None,
        }),
        NormalizedEvent::Error(AgentErrorEvent {
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
        assert!(matches!(events[0], NormalizedEvent::UserMessage(_)));
        assert!(matches!(events[1], NormalizedEvent::CommandStarted(_)));
        assert!(matches!(events[9], NormalizedEvent::AgentMessageDelta(_)));
        assert!(matches!(
            events[10],
            NormalizedEvent::AgentMessageCompleted(_)
        ));
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
    fn unified_hello_advertises_codex_capabilities_and_acp_providers() {
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
        let codex = hello
            .capabilities
            .agent_providers
            .iter()
            .find(|provider| provider.provider_id.as_str() == "codex")
            .expect("codex provider");
        assert!(codex.capabilities.interrupt);
        assert!(codex.capabilities.model_selection);
        assert!(codex.capabilities.reasoning_effort);
        assert!(codex.capabilities.collaboration_modes);
        assert!(codex
            .capabilities
            .provider_details
            .iter()
            .any(|detail| detail.key == "dynamic_tools"
                && detail.status == ProviderCapabilityStatus::Degraded
                && detail
                    .methods
                    .iter()
                    .any(|method| method == "item/tool/call")));
        assert!(codex
            .capabilities
            .provider_details
            .iter()
            .any(|detail| detail.key == "usage_account"
                && detail.status == ProviderCapabilityStatus::Degraded
                && detail
                    .methods
                    .iter()
                    .any(|method| method == "account/rateLimits/read")));
    }

    #[tokio::test]
    async fn interrupt_cancels_blocked_approval_for_same_session() {
        let session_id = SessionId::new();
        let other_session_id = SessionId::new();
        let approval_id = ApprovalId::new();
        let other_approval_id = ApprovalId::new();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let (other_sender, _other_receiver) = tokio::sync::oneshot::channel();
        let approvals = Arc::new(Mutex::new(HashMap::from([
            (
                approval_id,
                PendingProviderApproval::new(session_id, sender),
            ),
            (
                other_approval_id,
                PendingProviderApproval::new(other_session_id, other_sender),
            ),
        ])));

        let cancel = tokio::spawn(cancel_pending_provider_approvals_for_session(
            session_id,
            approvals.clone(),
            "test",
        ));
        let provider_decision = receiver.await.expect("provider decision");
        assert_eq!(provider_decision.decision, ApprovalDecision::Cancel);
        provider_decision
            .acknowledged
            .send(Ok(()))
            .expect("ack cancel");

        assert_eq!(cancel.await.expect("cancel task"), 1);
    }

    #[tokio::test]
    async fn interrupt_does_not_count_completed_approval_cancel_replay_as_new_cancel() {
        let session_id = SessionId::new();
        let approval_id = ApprovalId::new();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let pending = PendingProviderApproval::new(session_id, sender);
        let first_cancel = tokio::spawn({
            let pending = pending.clone();
            async move { pending.submit(ApprovalDecision::Cancel).await }
        });
        let provider_decision = receiver.await.expect("provider decision");
        assert_eq!(provider_decision.decision, ApprovalDecision::Cancel);
        provider_decision
            .acknowledged
            .send(Ok(()))
            .expect("ack cancel");
        assert_eq!(first_cancel.await.expect("first cancel"), Ok(()));

        let approvals = Arc::new(Mutex::new(HashMap::from([(approval_id, pending)])));
        let cancelled =
            cancel_pending_provider_approvals_for_session(session_id, approvals, "test").await;
        assert_eq!(cancelled, 0);
    }
}

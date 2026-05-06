mod agents;
mod wal;

use std::{
    collections::HashMap,
    env,
    path::{Path, PathBuf},
    sync::Arc,
};

use agenter_core::{
    AgentCapabilities, AgentProviderId, ApprovalDecision, ApprovalId, ApprovalKind,
    ApprovalRequest, ApprovalStatus, ContentBlock, ContentBlockKind, DiffFile, DiffId, DiffState,
    FileChangeKind, ItemId, ItemRole, ItemState, ItemStatus, NativeRef, RunnerId, SessionId,
    SessionStatus, ToolCommandProjection, ToolProjection, ToolProjectionKind, TurnId, TurnState,
    TurnStatus, UniversalEventKind, WorkspaceId, WorkspaceRef,
};
use agenter_protocol::runner::{
    AgentInput, AgentProviderAdvertisement, DiscoveredSessions, RunnerCapabilities,
    RunnerClientMessage, RunnerCommand, RunnerCommandResult, RunnerError, RunnerEvent,
    RunnerEventEnvelope, RunnerHello, RunnerOperationKind, RunnerOperationLogLevel,
    RunnerOperationProgress, RunnerOperationStatus, RunnerOperationUpdate, RunnerResponseEnvelope,
    RunnerResponseOutcome, RunnerServerMessage, PROTOCOL_VERSION,
};
use agenter_protocol::{
    chunk_message, reassemble_message, RequestId, RunnerTransportChunkFrame,
    RunnerTransportChunkReassembler, RunnerTransportOutboundFrame,
};
use agents::acp::{AcpProviderProfile, AcpRunnerRuntime, AcpTurnRequest, PendingAcpApproval};
use agents::adapter::{AdapterEvent, AdapterProviderRegistration, AdapterRuntime};
use agents::approval_state::{PendingApprovalSubmitError, PendingProviderApproval};
use agents::codex::runtime::{codex_provider_commands, CodexRunnerRuntime};
use futures_util::{SinkExt, StreamExt};
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
struct RunnerOperationReporter {
    request_id: RequestId,
    sender: mpsc::UnboundedSender<(Option<RequestId>, RunnerEvent)>,
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    agenter_core::logging::init_tracing("agenter-runner");

    if fake_mode_requested() {
        tracing::info!("starting fake runner mode");
        run_fake_runner().await?;
    } else if codex_mode_requested() {
        tracing::info!("starting codex app-server runner mode");
        run_codex_runner().await?;
    } else if acp_mode_requested() {
        tracing::info!("starting multi-provider ACP runner mode");
        run_acp_runner(AcpProviderProfile::available_all()).await?;
    } else if qwen_mode_requested() {
        tracing::info!("starting qwen ACP runner mode");
        run_acp_runner(vec![AcpProviderProfile::qwen()]).await?;
    } else if gemini_mode_requested() {
        tracing::info!("starting gemini ACP runner mode");
        run_acp_runner(vec![AcpProviderProfile::gemini()]).await?;
    } else if opencode_mode_requested() {
        tracing::info!("starting opencode ACP runner mode");
        run_acp_runner(vec![AcpProviderProfile::opencode()]).await?;
    } else {
        tracing::info!("starting unified runner mode");
        run_acp_runner(AcpProviderProfile::available_all()).await?
    }

    Ok(())
}

fn fake_mode_requested() -> bool {
    env::args().any(|arg| arg == "fake" || arg == "--fake")
        || env::var("AGENTER_RUNNER_MODE").is_ok_and(|mode| mode == "fake")
}

fn qwen_mode_requested() -> bool {
    env::args().any(|arg| arg == "qwen" || arg == "--qwen")
        || env::var("AGENTER_RUNNER_MODE").is_ok_and(|mode| mode == "qwen")
}

fn acp_mode_requested() -> bool {
    env::args().any(|arg| arg == "acp" || arg == "--acp")
        || env::var("AGENTER_RUNNER_MODE").is_ok_and(|mode| mode == "acp")
}

fn codex_mode_requested() -> bool {
    env::args().any(|arg| arg == "codex" || arg == "--codex")
        || env::var("AGENTER_RUNNER_MODE").is_ok_and(|mode| mode == "codex")
}

fn gemini_mode_requested() -> bool {
    env::args().any(|arg| arg == "gemini" || arg == "--gemini")
        || env::var("AGENTER_RUNNER_MODE").is_ok_and(|mode| mode == "gemini")
}

fn opencode_mode_requested() -> bool {
    env::args().any(|arg| arg == "opencode" || arg == "--opencode")
        || env::var("AGENTER_RUNNER_MODE").is_ok_and(|mode| mode == "opencode")
}

async fn run_acp_runner(profiles: Vec<AcpProviderProfile>) -> anyhow::Result<()> {
    if profiles.is_empty() {
        anyhow::bail!("no provider commands are available; install qwen, gemini, or opencode");
    }
    let url = env::var("AGENTER_CONTROL_PLANE_WS")
        .unwrap_or_else(|_| DEFAULT_CONTROL_PLANE_WS.to_owned());
    let token = env::var("AGENTER_DEV_RUNNER_TOKEN")
        .unwrap_or_else(|_| DEFAULT_DEV_RUNNER_TOKEN.to_owned());
    let workspace_path = env::var("AGENTER_WORKSPACE")
        .map(PathBuf::from)
        .unwrap_or(env::current_dir()?);
    let workspace_path = workspace_path.canonicalize().unwrap_or(workspace_path);
    let hello_template = acp_hello(token, workspace_path.clone(), &profiles);
    let wal = RunnerWal::open(runner_wal_path(hello_template.runner_id, &workspace_path)).await?;
    let advertised_workspace = hello_template.workspaces.first().cloned();
    tracing::info!(
        url = %url,
        runner_id = %hello_template.runner_id,
        workspace = %workspace_path.display(),
        provider_count = profiles.len(),
        "starting reconnect-stable ACP runner"
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
    let mut session_profiles = HashMap::<SessionId, AgentProviderId>::new();
    let pending_approvals: Arc<Mutex<HashMap<ApprovalId, PendingAcpApproval>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let (acp_event_sender, mut acp_event_receiver) = mpsc::unbounded_channel::<AdapterEvent>();
    let (background_event_sender, mut background_event_receiver) =
        mpsc::unbounded_channel::<(Option<RequestId>, RunnerEvent)>();
    let acp_runtime = AcpRunnerRuntime::new(workspace_path.clone());

    loop {
        tracing::info!(url = %url, runner_id = %hello_template.runner_id, "connecting ACP runner to control plane");
        let (socket, _) = match connect_async(&url).await {
            Ok(socket) => socket,
            Err(error) => {
                tracing::warn!(%error, "ACP runner websocket connect failed; retrying");
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
            tracing::warn!(%error, "failed to send ACP runner hello; reconnecting");
            sleep(runner_reconnect_delay()).await;
            continue;
        }
        if let Err(error) = replay_unacked_wal(&mut sender, &wal).await {
            tracing::warn!(%error, "failed to replay ACP runner WAL; reconnecting");
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
                            tracing::warn!(%error, "failed to send ACP background event; reconnecting");
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
                                tracing::info!(session_id = %command.session_id, request_id = %envelope.request_id, provider_id = %command.provider_id, "ACP runner received create session");
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
                                tracing::info!(session_id = %command.session_id, request_id = %envelope.request_id, provider_id = %command.provider_id, "ACP runner received resume session");
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
                                tracing::info!(request_id = %envelope.request_id, workspace = %command.workspace.path, provider_id = %command.provider_id, "ACP runner received refresh sessions");
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
                                            RunnerEvent::Error(runner_error("acp_refresh_sessions_failed", error))
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
                                tracing::info!(session_id = %command.session_id, request_id = %envelope.request_id, "ACP runner received agent options request");
                                let _ = command;
                                let outcome = RunnerResponseOutcome::Ok {
                                    result: RunnerCommandResult::AgentOptions {
                                        options: agenter_core::AgentOptions::default(),
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
                                tracing::info!(session_id = %command.session_id, request_id = %envelope.request_id, "ACP runner received agent input");
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
                                tracing::info!(session_id = %command.session_id, approval_id = %command.approval_id, "ACP runner received approval answer");
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
                                tracing::info!(session_id = %command.session_id, request_id = %envelope.request_id, provider_id = %command.provider_id, "ACP runner received provider command manifest request");
                                send_runner_message(
                                    &mut sender,
                                    RunnerClientMessage::Response(RunnerResponseEnvelope {
                                        request_id: envelope.request_id,
                                        outcome: RunnerResponseOutcome::Ok {
                                            result: RunnerCommandResult::ProviderCommands {
                                                commands: Vec::new(),
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
                            RunnerCommand::AnswerQuestion(command) => {
                                tracing::info!(session_id = %command.session_id, question_id = %command.answer.question_id, "ACP runner received question answer");
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
                            RunnerCommand::InterruptSession { session_id } => {
                                let interrupted = cancel_pending_provider_approvals_for_session(
                                    session_id,
                                    pending_approvals.clone(),
                                    "ACP",
                                )
                                .await;
                                let outcome = if interrupted > 0 {
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
            tracing::warn!(%error, "ACP runner transport session failed; reconnecting");
        }
        sleep(runner_reconnect_delay()).await;
    }

    #[allow(unreachable_code)]
    Ok(())
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
    let advertised_workspace = hello_template.workspaces.first().cloned();
    let provider_id = AgentProviderId::from(AgentProviderId::CODEX);
    let mut adapter_runtime = AdapterRuntime::new();
    adapter_runtime.register_provider(AdapterProviderRegistration {
        provider_id: provider_id.clone(),
        capabilities: CodexRunnerRuntime::registration().capability_set,
    });
    let (codex_event_sender, mut codex_event_receiver) = mpsc::unbounded_channel::<AdapterEvent>();
    let (background_event_sender, mut background_event_receiver) =
        mpsc::unbounded_channel::<(Option<RequestId>, RunnerEvent)>();
    let runtime = CodexRunnerRuntime::spawn(workspace_path.clone(), codex_event_sender.clone())?;

    loop {
        tracing::info!(url = %url, runner_id = %hello_template.runner_id, "connecting Codex runner to control plane");
        let (socket, _) = match connect_async(&url).await {
            Ok(socket) => socket,
            Err(error) => {
                tracing::warn!(%error, "Codex runner websocket connect failed; retrying");
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
        send_runner_message(&mut sender, RunnerClientMessage::Hello(hello)).await?;
        replay_unacked_wal(&mut sender, &wal).await?;

        let transport_result: anyhow::Result<()> = async {
            loop {
                tokio::select! {
                    background = background_event_receiver.recv() => {
                        let Some((request_id, event)) = background else {
                            continue;
                        };
                        if let Err(error) = send_wal_event(&mut sender, &wal, request_id, None, event).await {
                            tracing::warn!(%error, "failed to send Codex background event; reconnecting");
                            break;
                        }
                    }
                    event = codex_event_receiver.recv() => {
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
                            tracing::warn!(%error, "failed to send Codex agent event; reconnecting");
                            break;
                        }
                    }
                    message = receiver.next() => {
                        let Some(message) = message else {
                            tracing::info!("control plane websocket closed for Codex runner");
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
                                if command.provider_id != provider_id {
                                    send_runner_message(
                                        &mut sender,
                                        RunnerClientMessage::Response(RunnerResponseEnvelope {
                                            request_id: envelope.request_id,
                                            outcome: RunnerResponseOutcome::Error {
                                                error: RunnerError {
                                                    code: "codex_provider_not_available".to_owned(),
                                                    message: format!("Codex runner cannot create provider `{}`", command.provider_id),
                                                },
                                            },
                                        }),
                                    ).await?;
                                    continue;
                                }
                                let outcome = match runtime.create_session(command.session_id, command.initial_input).await {
                                    Ok(handle) => {
                                        adapter_runtime.bind_session(handle.session_id, provider_id.clone());
                                        RunnerResponseOutcome::Ok {
                                            result: RunnerCommandResult::SessionCreated {
                                                session_id: handle.session_id,
                                                external_session_id: handle.external_session_id,
                                            },
                                        }
                                    }
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
                                ).await?;
                            }
                            RunnerCommand::ResumeSession(command) => {
                                let outcome = match runtime
                                    .resume_session(command.session_id, command.external_session_id)
                                    .await
                                {
                                    Ok(handle) => {
                                        adapter_runtime.bind_session(handle.session_id, provider_id.clone());
                                        RunnerResponseOutcome::Ok {
                                            result: RunnerCommandResult::SessionResumed {
                                                session_id: handle.session_id,
                                                external_session_id: handle.external_session_id,
                                            },
                                        }
                                    }
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
                                ).await?;
                            }
                            RunnerCommand::RefreshSessions(command) => {
                                let request_id = envelope.request_id.clone();
                                let background_sender = background_event_sender.clone();
                                let runtime = runtime.clone();
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
                                    reporter.info(
                                        RunnerOperationStatus::Discovering,
                                        "Discovering Codex threads",
                                        None,
                                        Some("Listing Codex app-server threads".to_owned()),
                                    );
                                    let event = match runtime.refresh_sessions(command.workspace.clone()).await {
                                        Ok(sessions) => {
                                            reporter.info(
                                                RunnerOperationStatus::SendingResults,
                                                "Sending refresh results",
                                                None,
                                                Some(format!("Sending {} discovered Codex sessions", sessions.len())),
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
                                            RunnerEvent::Error(runner_error("codex_refresh_sessions_failed", error))
                                        }
                                    };
                                    background_sender.send((Some(request_id), event)).ok();
                                });
                                send_runner_message(
                                    &mut sender,
                                    RunnerClientMessage::Response(RunnerResponseEnvelope {
                                        request_id: envelope.request_id,
                                        outcome: RunnerResponseOutcome::Ok {
                                            result: RunnerCommandResult::Accepted,
                                        },
                                    }),
                                ).await?;
                            }
                            RunnerCommand::GetAgentOptions(_command) => {
                                let runtime = runtime.clone();
                                let outcome = match runtime.agent_options().await {
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
                                ).await?;
                            }
                            RunnerCommand::AgentSendInput(command) => {
                                adapter_runtime.bind_session(command.session_id, provider_id.clone());
                                send_runner_message(
                                    &mut sender,
                                    RunnerClientMessage::Response(RunnerResponseEnvelope {
                                        request_id: envelope.request_id.clone(),
                                        outcome: RunnerResponseOutcome::Ok {
                                            result: RunnerCommandResult::Accepted,
                                        },
                                    }),
                                ).await?;
                                let runtime = runtime.clone();
                                let event_sender = codex_event_sender.clone();
                                tokio::spawn(async move {
                                    let session_id = command.session_id;
                                    if let Err(error) = runtime
                                        .send_input(session_id, command.external_session_id, command.input, command.settings)
                                        .await
                                    {
                                        tracing::error!(%session_id, %error, "Codex turn failed");
                                        event_sender
                                            .send(AdapterEvent::session_status(
                                                AgentProviderId::from(AgentProviderId::CODEX),
                                                "codex/app-server",
                                                None,
                                                session_id,
                                                SessionStatus::Failed,
                                                Some(error.to_string()),
                                            ))
                                            .ok();
                                        event_sender
                                            .send(AdapterEvent::error(
                                                AgentProviderId::from(AgentProviderId::CODEX),
                                                "codex/app-server",
                                                None,
                                                session_id,
                                                Some("codex_adapter_error".to_owned()),
                                                error.to_string(),
                                            ))
                                            .ok();
                                    }
                                });
                            }
                            RunnerCommand::AnswerApproval(command) => {
                                let outcome = match runtime
                                    .answer_approval(command.approval_id, command.decision)
                                    .await
                                {
                                    Ok(()) => RunnerResponseOutcome::Ok {
                                        result: RunnerCommandResult::Accepted,
                                    },
                                    Err(error) => RunnerResponseOutcome::Error {
                                        error: runner_error("codex_approval_response_failed", error),
                                    },
                                };
                                send_runner_message(
                                    &mut sender,
                                    RunnerClientMessage::Response(RunnerResponseEnvelope {
                                        request_id: envelope.request_id,
                                        outcome,
                                    }),
                                ).await?;
                            }
                            RunnerCommand::ListProviderCommands(_command) => {
                                send_runner_message(
                                    &mut sender,
                                    RunnerClientMessage::Response(RunnerResponseEnvelope {
                                        request_id: envelope.request_id,
                                        outcome: RunnerResponseOutcome::Ok {
                                            result: RunnerCommandResult::ProviderCommands {
                                                commands: codex_provider_commands(),
                                            },
                                        },
                                    }),
                                ).await?;
                            }
                            RunnerCommand::ExecuteProviderCommand(command) => {
                                let outcome = match runtime
                                    .execute_provider_command(command.session_id, command.external_session_id, command.command)
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
                                ).await?;
                            }
                            RunnerCommand::AnswerQuestion(command) => {
                                let outcome = match runtime.answer_question(command.answer).await {
                                    Ok(()) => RunnerResponseOutcome::Ok {
                                        result: RunnerCommandResult::Accepted,
                                    },
                                    Err(error) => RunnerResponseOutcome::Error {
                                        error: runner_error("codex_question_response_failed", error),
                                    },
                                };
                                send_runner_message(
                                    &mut sender,
                                    RunnerClientMessage::Response(RunnerResponseEnvelope {
                                        request_id: envelope.request_id,
                                        outcome,
                                    }),
                                ).await?;
                            }
                            RunnerCommand::InterruptSession { session_id } => {
                                let outcome = match runtime.interrupt_session(session_id).await {
                                    Ok(()) => RunnerResponseOutcome::Ok {
                                        result: RunnerCommandResult::Accepted,
                                    },
                                    Err(error) => RunnerResponseOutcome::Error {
                                        error: runner_error("codex_interrupt_failed", error),
                                    },
                                };
                                send_runner_message(
                                    &mut sender,
                                    RunnerClientMessage::Response(RunnerResponseEnvelope {
                                        request_id: envelope.request_id,
                                        outcome,
                                    }),
                                ).await?;
                            }
                            RunnerCommand::ShutdownSession(command) => {
                                let outcome = match runtime.shutdown_session(command.session_id).await {
                                    Ok(()) => {
                                        adapter_runtime.unbind_session(command.session_id);
                                        RunnerResponseOutcome::Ok {
                                            result: RunnerCommandResult::Accepted,
                                        }
                                    }
                                    Err(error) => RunnerResponseOutcome::Error {
                                        error: runner_error("codex_shutdown_failed", error),
                                    },
                                };
                                send_runner_message(
                                    &mut sender,
                                    RunnerClientMessage::Response(RunnerResponseEnvelope {
                                        request_id: envelope.request_id,
                                        outcome,
                                    }),
                                ).await?;
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
            tracing::debug!(workspace = %workspace.path, "Codex runner closed");
        }
        if let Err(error) = transport_result {
            tracing::warn!(%error, "Codex runner transport session failed; reconnecting");
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
                    let Some(agent_event) = event.universal_projection_for_wal() else {
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
                                commands: Vec::new(),
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

fn default_agent_capabilities(session_resume: bool, interrupt: bool) -> AgentCapabilities {
    AgentCapabilities {
        session_resume,
        interrupt,
        ..AgentCapabilities::default()
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
                capabilities: default_agent_capabilities(false, false),
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

fn acp_hello(
    token: String,
    workspace_path: PathBuf,
    profiles: &[AcpProviderProfile],
) -> RunnerHello {
    let provider_id = AgentProviderId::from("acp");
    let runner_id = configured_runner_id(&provider_id, &workspace_path);
    let workspace_id = configured_workspace_id(&provider_id, &workspace_path);
    let agent_providers = profiles
        .iter()
        .map(|profile| AgentProviderAdvertisement {
            provider_id: profile.provider_id.clone(),
            capabilities: profile.advertised_capabilities(),
        })
        .collect();
    RunnerHello {
        runner_id,
        protocol_version: PROTOCOL_VERSION.to_owned(),
        token,
        capabilities: RunnerCapabilities {
            agent_providers,
            transports: vec!["acp-stdio".to_owned()],
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

fn codex_hello(token: String, workspace_path: PathBuf) -> RunnerHello {
    let registration = CodexRunnerRuntime::registration();
    let runner_id = configured_runner_id(&registration.provider_id, &workspace_path);
    let workspace_id = configured_workspace_id(&registration.provider_id, &workspace_path);
    RunnerHello {
        runner_id,
        protocol_version: PROTOCOL_VERSION.to_owned(),
        token,
        capabilities: RunnerCapabilities {
            agent_providers: vec![AgentProviderAdvertisement {
                provider_id: registration.provider_id,
                capabilities: registration.capabilities,
            }],
            transports: vec!["codex-app-server".to_owned()],
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
                .or_else(|| Some("codex workspace".to_owned())),
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

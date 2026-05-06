use std::{collections::HashMap, env, path::PathBuf, sync::Arc};

use agenter_core::{AgentProviderId, ApprovalId, SessionId, SessionStatus};
use agenter_protocol::runner::{
    DiscoveredSessions, RunnerCommand, RunnerCommandResult, RunnerError, RunnerEvent,
    RunnerResponseOutcome,
};
use tokio::sync::{mpsc, Mutex};

use crate::agents::acp::{
    AcpProviderProfile, AcpRunnerRuntime, AcpTurnRequest, PendingAcpApproval,
};
use crate::agents::adapter::{AdapterEvent, AdapterProviderRegistration, AdapterRuntime};
use crate::runner_host::{
    background_event_channel, run_runner_host, RunnerBackgroundEventSender, RunnerCommandHandler,
    RunnerHostBoxFuture, RunnerHostConfig, RunnerOperationReporter,
};

pub(crate) async fn run(profiles: Vec<AcpProviderProfile>) -> anyhow::Result<()> {
    if profiles.is_empty() {
        anyhow::bail!("no provider commands are available; install qwen, gemini, or opencode");
    }
    let url = env::var("AGENTER_CONTROL_PLANE_WS")
        .unwrap_or_else(|_| crate::DEFAULT_CONTROL_PLANE_WS.to_owned());
    let token = env::var("AGENTER_DEV_RUNNER_TOKEN")
        .unwrap_or_else(|_| crate::DEFAULT_DEV_RUNNER_TOKEN.to_owned());
    let workspace_path = env::var("AGENTER_WORKSPACE")
        .map(PathBuf::from)
        .unwrap_or(env::current_dir()?);
    let workspace_path = workspace_path.canonicalize().unwrap_or(workspace_path);
    let hello_template = crate::acp_hello(token, workspace_path.clone(), &profiles);
    tracing::info!(
        url = %url,
        runner_id = %hello_template.runner_id,
        workspace = %workspace_path.display(),
        provider_count = profiles.len(),
        "starting reconnect-stable ACP runner"
    );

    let (adapter_event_sender, adapter_event_receiver) = mpsc::unbounded_channel::<AdapterEvent>();
    let (background_sender, background_receiver) = background_event_channel();
    let handler = AcpMode::new(profiles, workspace_path.clone(), adapter_event_sender);
    run_runner_host(
        RunnerHostConfig::new("ACP", url, workspace_path, hello_template),
        adapter_event_receiver,
        background_sender,
        background_receiver,
        handler,
    )
    .await
}

struct AcpMode {
    profiles_by_id: HashMap<AgentProviderId, AcpProviderProfile>,
    adapter_runtime: AdapterRuntime,
    session_profiles: HashMap<SessionId, AgentProviderId>,
    pending_approvals: Arc<Mutex<HashMap<ApprovalId, PendingAcpApproval>>>,
    event_sender: mpsc::UnboundedSender<AdapterEvent>,
    runtime: AcpRunnerRuntime,
}

impl AcpMode {
    fn new(
        profiles: Vec<AcpProviderProfile>,
        workspace_path: PathBuf,
        event_sender: mpsc::UnboundedSender<AdapterEvent>,
    ) -> Self {
        let profiles_by_id = profiles
            .into_iter()
            .map(|profile| (profile.provider_id.clone(), profile))
            .collect::<HashMap<_, _>>();
        let mut adapter_runtime = AdapterRuntime::new();
        for profile in profiles_by_id.values() {
            adapter_runtime.register_provider(AdapterProviderRegistration {
                provider_id: profile.provider_id.clone(),
            });
        }
        Self {
            profiles_by_id,
            adapter_runtime,
            session_profiles: HashMap::new(),
            pending_approvals: Arc::new(Mutex::new(HashMap::new())),
            event_sender,
            runtime: AcpRunnerRuntime::new(workspace_path),
        }
    }

    fn provider_not_available(&self, provider_id: &AgentProviderId) -> RunnerResponseOutcome {
        RunnerResponseOutcome::Error {
            error: RunnerError {
                code: "acp_provider_not_available".to_owned(),
                message: format!("ACP provider `{provider_id}` is not available in this runner"),
            },
        }
    }

    fn resolve_send_provider(
        &self,
        session_id: SessionId,
        requested_provider: Option<&AgentProviderId>,
    ) -> Option<AgentProviderId> {
        self.adapter_runtime
            .resolve_provider(Some(session_id), requested_provider)
            .map(|registration| registration.provider_id.clone())
            .or_else(|| self.session_profiles.get(&session_id).cloned())
            .or_else(|| self.profiles_by_id.keys().next().cloned())
    }
}

impl RunnerCommandHandler for AcpMode {
    fn handle_command<'a>(
        &'a mut self,
        envelope: Box<agenter_protocol::runner::RunnerCommandEnvelope>,
        background_sender: RunnerBackgroundEventSender,
    ) -> RunnerHostBoxFuture<'a, RunnerResponseOutcome> {
        Box::pin(async move {
            match envelope.command {
                RunnerCommand::CreateSession(command) => {
                    tracing::info!(session_id = %command.session_id, request_id = %envelope.request_id, provider_id = %command.provider_id, "ACP runner received create session");
                    let Some(profile) = self.profiles_by_id.get(&command.provider_id).cloned()
                    else {
                        return Ok(self.provider_not_available(&command.provider_id));
                    };
                    let outcome = match self
                        .runtime
                        .create_session(command.session_id, profile)
                        .await
                    {
                        Ok(external_session_id) => {
                            self.adapter_runtime
                                .bind_session(command.session_id, command.provider_id.clone());
                            self.session_profiles
                                .insert(command.session_id, command.provider_id);
                            RunnerResponseOutcome::Ok {
                                result: RunnerCommandResult::SessionCreated {
                                    session_id: command.session_id,
                                    external_session_id,
                                },
                            }
                        }
                        Err(error) => RunnerResponseOutcome::Error {
                            error: crate::runner_error("acp_create_session_failed", error),
                        },
                    };
                    Ok(outcome)
                }
                RunnerCommand::ResumeSession(command) => {
                    tracing::info!(session_id = %command.session_id, request_id = %envelope.request_id, provider_id = %command.provider_id, "ACP runner received resume session");
                    let Some(profile) = self.profiles_by_id.get(&command.provider_id).cloned()
                    else {
                        return Ok(self.provider_not_available(&command.provider_id));
                    };
                    let outcome = match self
                        .runtime
                        .resume_session(command.session_id, profile, command.external_session_id)
                        .await
                    {
                        Ok(external_session_id) => {
                            self.adapter_runtime
                                .bind_session(command.session_id, command.provider_id.clone());
                            self.session_profiles
                                .insert(command.session_id, command.provider_id);
                            RunnerResponseOutcome::Ok {
                                result: RunnerCommandResult::SessionResumed {
                                    session_id: command.session_id,
                                    external_session_id,
                                },
                            }
                        }
                        Err(error) => RunnerResponseOutcome::Error {
                            error: crate::runner_error("acp_resume_session_failed", error),
                        },
                    };
                    Ok(outcome)
                }
                RunnerCommand::RefreshSessions(command) => {
                    tracing::info!(request_id = %envelope.request_id, workspace = %command.workspace.path, provider_id = %command.provider_id, "ACP runner received refresh sessions");
                    let Some(profile) = self.profiles_by_id.get(&command.provider_id).cloned()
                    else {
                        return Ok(self.provider_not_available(&command.provider_id));
                    };
                    let request_id = envelope.request_id.clone();
                    let runtime = self.runtime.clone();
                    tokio::spawn(async move {
                        let reporter = RunnerOperationReporter::new(
                            request_id.clone(),
                            background_sender.clone(),
                        );
                        reporter.info(
                            agenter_protocol::runner::RunnerOperationStatus::Accepted,
                            "Refresh accepted",
                            None,
                            Some(format!("{} refresh task started", command.provider_id)),
                        );
                        reporter.info(
                            agenter_protocol::runner::RunnerOperationStatus::Discovering,
                            "Discovering sessions",
                            None,
                            Some("Listing ACP sessions".to_owned()),
                        );
                        let event = match runtime.discover_sessions(profile).await {
                            Ok(sessions) => {
                                reporter.info(
                                    agenter_protocol::runner::RunnerOperationStatus::SendingResults,
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
                                    agenter_protocol::runner::RunnerOperationStatus::Failed,
                                    "Refresh failed",
                                    Some(error.to_string()),
                                );
                                RunnerEvent::Error(crate::runner_error(
                                    "acp_refresh_sessions_failed",
                                    error,
                                ))
                            }
                        };
                        background_sender.send((Some(request_id), event)).ok();
                    });
                    Ok(RunnerResponseOutcome::Ok {
                        result: RunnerCommandResult::Accepted,
                    })
                }
                RunnerCommand::GetAgentOptions(command) => {
                    tracing::info!(session_id = %command.session_id, request_id = %envelope.request_id, "ACP runner received agent options request");
                    Ok(RunnerResponseOutcome::Ok {
                        result: RunnerCommandResult::AgentOptions {
                            options: agenter_core::AgentOptions::default(),
                        },
                    })
                }
                RunnerCommand::AgentSendInput(command) => {
                    tracing::info!(session_id = %command.session_id, request_id = %envelope.request_id, "ACP runner received agent input");
                    let Some(provider_id) = self
                        .resolve_send_provider(command.session_id, command.provider_id.as_ref())
                    else {
                        return Ok(RunnerResponseOutcome::Error {
                            error: RunnerError {
                                code: "acp_provider_not_available".to_owned(),
                                message: "No ACP provider is available for this session."
                                    .to_owned(),
                            },
                        });
                    };
                    let Some(profile) = self.profiles_by_id.get(&provider_id).cloned() else {
                        return Ok(self.provider_not_available(&provider_id));
                    };
                    self.adapter_runtime
                        .bind_session(command.session_id, provider_id.clone());
                    self.session_profiles
                        .insert(command.session_id, provider_id);

                    let request = AcpTurnRequest {
                        session_id: command.session_id,
                        external_session_id: command.external_session_id,
                        prompt: crate::agent_input_text(&command.input),
                    };
                    let event_sender = self.event_sender.clone();
                    let pending = self.pending_approvals.clone();
                    let runtime = self.runtime.clone();
                    let session_id = request.session_id;
                    tokio::spawn(async move {
                        if let Err(error) = runtime
                            .run_turn(request, profile, event_sender.clone(), pending)
                            .await
                        {
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
                    Ok(RunnerResponseOutcome::Ok {
                        result: RunnerCommandResult::Accepted,
                    })
                }
                RunnerCommand::AnswerApproval(command) => {
                    tracing::info!(session_id = %command.session_id, approval_id = %command.approval_id, "ACP runner received approval answer");
                    let pending = self
                        .pending_approvals
                        .lock()
                        .await
                        .get(&command.approval_id)
                        .cloned();
                    let outcome = if let Some(pending) = pending {
                        crate::answer_pending_provider_approval(
                            command.approval_id,
                            command.decision,
                            pending,
                            "ACP",
                        )
                        .await
                    } else {
                        RunnerResponseOutcome::Error {
                            error: RunnerError {
                                code: "approval_not_found".to_owned(),
                                message: "approval is no longer pending in the provider adapter"
                                    .to_owned(),
                            },
                        }
                    };
                    Ok(outcome)
                }
                RunnerCommand::ListProviderCommands(command) => {
                    tracing::info!(session_id = %command.session_id, request_id = %envelope.request_id, provider_id = %command.provider_id, "ACP runner received provider command manifest request");
                    Ok(RunnerResponseOutcome::Ok {
                        result: RunnerCommandResult::ProviderCommands {
                            commands: Vec::new(),
                        },
                    })
                }
                RunnerCommand::ExecuteProviderCommand(command) => {
                    Ok(RunnerResponseOutcome::Error {
                        error: RunnerError {
                            code: "acp_provider_command_unsupported".to_owned(),
                            message: format!(
                                "ACP provider command `{}` is not implemented yet.",
                                command.command.command_id
                            ),
                        },
                    })
                }
                RunnerCommand::AnswerQuestion(command) => {
                    tracing::info!(session_id = %command.session_id, question_id = %command.answer.question_id, "ACP runner received question answer");
                    Ok(RunnerResponseOutcome::Ok {
                        result: RunnerCommandResult::Accepted,
                    })
                }
                RunnerCommand::InterruptSession { session_id } => {
                    let interrupted = crate::cancel_pending_provider_approvals_for_session(
                        session_id,
                        self.pending_approvals.clone(),
                        "ACP",
                    )
                    .await;
                    let outcome = if interrupted > 0 {
                        RunnerResponseOutcome::Ok {
                            result: RunnerCommandResult::Accepted,
                        }
                    } else {
                        crate::provider_cancel_unsupported("provider")
                    };
                    Ok(outcome)
                }
                RunnerCommand::ShutdownSession(command) => {
                    self.runtime.shutdown_session(command.session_id).await;
                    self.adapter_runtime.unbind_session(command.session_id);
                    self.event_sender
                        .send(AdapterEvent::session_status(
                            AgentProviderId::from("acp"),
                            "acp-stdio",
                            None,
                            command.session_id,
                            SessionStatus::Stopped,
                            Some("ACP session runtime stopped.".to_owned()),
                        ))
                        .ok();
                    Ok(RunnerResponseOutcome::Ok {
                        result: RunnerCommandResult::Accepted,
                    })
                }
            }
        })
    }
}

use std::{env, path::PathBuf};

use agenter_core::{AgentProviderId, SessionStatus};
use agenter_protocol::runner::{
    DiscoveredSessions, RunnerCommand, RunnerCommandResult, RunnerError, RunnerEvent,
    RunnerResponseOutcome,
};
use tokio::sync::mpsc;

use crate::agents::adapter::{AdapterEvent, AdapterProviderRegistration, AdapterRuntime};
use crate::agents::codex::runtime::{codex_provider_commands, CodexRunnerRuntime};
use crate::runner_host::{
    background_event_channel, run_runner_host, RunnerBackgroundEventSender, RunnerCommandHandler,
    RunnerHostBoxFuture, RunnerHostConfig, RunnerOperationReporter,
};

pub(crate) async fn run() -> anyhow::Result<()> {
    let url = env::var("AGENTER_CONTROL_PLANE_WS")
        .unwrap_or_else(|_| crate::DEFAULT_CONTROL_PLANE_WS.to_owned());
    let token = env::var("AGENTER_DEV_RUNNER_TOKEN")
        .unwrap_or_else(|_| crate::DEFAULT_DEV_RUNNER_TOKEN.to_owned());
    let workspace_path = env::var("AGENTER_WORKSPACE")
        .map(PathBuf::from)
        .unwrap_or(env::current_dir()?);
    let workspace_path = workspace_path.canonicalize().unwrap_or(workspace_path);
    let hello_template = crate::codex_hello(token, workspace_path.clone());
    let (adapter_event_sender, adapter_event_receiver) = mpsc::unbounded_channel::<AdapterEvent>();
    let (background_sender, background_receiver) = background_event_channel();
    let runtime = CodexRunnerRuntime::spawn(workspace_path.clone(), adapter_event_sender.clone())?;
    let handler = CodexMode::new(runtime, adapter_event_sender);
    run_runner_host(
        RunnerHostConfig::new("Codex", url, workspace_path, hello_template),
        adapter_event_receiver,
        background_sender,
        background_receiver,
        handler,
    )
    .await
}

struct CodexMode {
    provider_id: AgentProviderId,
    adapter_runtime: AdapterRuntime,
    event_sender: mpsc::UnboundedSender<AdapterEvent>,
    runtime: CodexRunnerRuntime,
}

impl CodexMode {
    fn new(runtime: CodexRunnerRuntime, event_sender: mpsc::UnboundedSender<AdapterEvent>) -> Self {
        let provider_id = AgentProviderId::from(AgentProviderId::CODEX);
        let mut adapter_runtime = AdapterRuntime::new();
        adapter_runtime.register_provider(AdapterProviderRegistration {
            provider_id: provider_id.clone(),
        });
        Self {
            provider_id,
            adapter_runtime,
            event_sender,
            runtime,
        }
    }
}

impl RunnerCommandHandler for CodexMode {
    fn handle_command<'a>(
        &'a mut self,
        envelope: Box<agenter_protocol::runner::RunnerCommandEnvelope>,
        background_sender: RunnerBackgroundEventSender,
    ) -> RunnerHostBoxFuture<'a, RunnerResponseOutcome> {
        Box::pin(async move {
            match envelope.command {
                RunnerCommand::CreateSession(command) => {
                    if command.provider_id != self.provider_id {
                        return Ok(RunnerResponseOutcome::Error {
                            error: RunnerError {
                                code: "codex_provider_not_available".to_owned(),
                                message: format!(
                                    "Codex runner cannot create provider `{}`",
                                    command.provider_id
                                ),
                            },
                        });
                    }
                    let outcome = match self
                        .runtime
                        .create_session(command.session_id, command.initial_input)
                        .await
                    {
                        Ok(handle) => {
                            self.adapter_runtime
                                .bind_session(handle.session_id, self.provider_id.clone());
                            RunnerResponseOutcome::Ok {
                                result: RunnerCommandResult::SessionCreated {
                                    session_id: handle.session_id,
                                    external_session_id: handle.external_session_id,
                                },
                            }
                        }
                        Err(error) => RunnerResponseOutcome::Error {
                            error: crate::runner_error("codex_create_session_failed", error),
                        },
                    };
                    Ok(outcome)
                }
                RunnerCommand::ResumeSession(command) => {
                    let outcome = match self
                        .runtime
                        .resume_session(command.session_id, command.external_session_id)
                        .await
                    {
                        Ok(handle) => {
                            self.adapter_runtime
                                .bind_session(handle.session_id, self.provider_id.clone());
                            RunnerResponseOutcome::Ok {
                                result: RunnerCommandResult::SessionResumed {
                                    session_id: handle.session_id,
                                    external_session_id: handle.external_session_id,
                                },
                            }
                        }
                        Err(error) => RunnerResponseOutcome::Error {
                            error: crate::runner_error("codex_resume_session_failed", error),
                        },
                    };
                    Ok(outcome)
                }
                RunnerCommand::RefreshSessions(command) => {
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
                            Some("Codex refresh task started".to_owned()),
                        );
                        reporter.info(
                            agenter_protocol::runner::RunnerOperationStatus::Discovering,
                            "Discovering Codex threads",
                            None,
                            Some("Listing Codex app-server threads".to_owned()),
                        );
                        let event = match runtime.refresh_sessions(command.workspace.clone()).await
                        {
                            Ok(sessions) => {
                                reporter.info(
                                    agenter_protocol::runner::RunnerOperationStatus::SendingResults,
                                    "Sending refresh results",
                                    None,
                                    Some(format!(
                                        "Sending {} discovered Codex sessions",
                                        sessions.len()
                                    )),
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
                                    "codex_refresh_sessions_failed",
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
                RunnerCommand::GetAgentOptions(_command) => {
                    let outcome = match self.runtime.agent_options().await {
                        Ok(options) => RunnerResponseOutcome::Ok {
                            result: RunnerCommandResult::AgentOptions { options },
                        },
                        Err(error) => RunnerResponseOutcome::Error {
                            error: crate::runner_error("codex_agent_options_failed", error),
                        },
                    };
                    Ok(outcome)
                }
                RunnerCommand::AgentSendInput(command) => {
                    self.adapter_runtime
                        .bind_session(command.session_id, self.provider_id.clone());
                    let runtime = self.runtime.clone();
                    let event_sender = self.event_sender.clone();
                    tokio::spawn(async move {
                        let session_id = command.session_id;
                        if let Err(error) = runtime
                            .send_input(
                                session_id,
                                command.external_session_id,
                                command.input,
                                command.settings,
                            )
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
                    Ok(RunnerResponseOutcome::Ok {
                        result: RunnerCommandResult::Accepted,
                    })
                }
                RunnerCommand::AnswerApproval(command) => {
                    let outcome = match self
                        .runtime
                        .answer_approval(command.approval_id, command.decision)
                        .await
                    {
                        Ok(()) => RunnerResponseOutcome::Ok {
                            result: RunnerCommandResult::Accepted,
                        },
                        Err(error) => RunnerResponseOutcome::Error {
                            error: crate::runner_error("codex_approval_response_failed", error),
                        },
                    };
                    Ok(outcome)
                }
                RunnerCommand::ListProviderCommands(_command) => Ok(RunnerResponseOutcome::Ok {
                    result: RunnerCommandResult::ProviderCommands {
                        commands: codex_provider_commands(),
                    },
                }),
                RunnerCommand::ExecuteProviderCommand(command) => {
                    let outcome = match self
                        .runtime
                        .execute_provider_command(
                            command.session_id,
                            command.external_session_id,
                            command.command,
                        )
                        .await
                    {
                        Ok(result) => RunnerResponseOutcome::Ok {
                            result: RunnerCommandResult::ProviderCommandExecuted { result },
                        },
                        Err(error) => RunnerResponseOutcome::Error {
                            error: crate::runner_error("codex_provider_command_failed", error),
                        },
                    };
                    Ok(outcome)
                }
                RunnerCommand::AnswerQuestion(command) => {
                    let outcome = match self.runtime.answer_question(command.answer).await {
                        Ok(()) => RunnerResponseOutcome::Ok {
                            result: RunnerCommandResult::Accepted,
                        },
                        Err(error) => RunnerResponseOutcome::Error {
                            error: crate::runner_error("codex_question_response_failed", error),
                        },
                    };
                    Ok(outcome)
                }
                RunnerCommand::InterruptSession { session_id } => {
                    let outcome = match self.runtime.interrupt_session(session_id).await {
                        Ok(()) => RunnerResponseOutcome::Ok {
                            result: RunnerCommandResult::Accepted,
                        },
                        Err(error) => RunnerResponseOutcome::Error {
                            error: crate::runner_error("codex_interrupt_failed", error),
                        },
                    };
                    Ok(outcome)
                }
                RunnerCommand::ShutdownSession(command) => {
                    let outcome = match self.runtime.shutdown_session(command.session_id).await {
                        Ok(()) => {
                            self.adapter_runtime.unbind_session(command.session_id);
                            RunnerResponseOutcome::Ok {
                                result: RunnerCommandResult::Accepted,
                            }
                        }
                        Err(error) => RunnerResponseOutcome::Error {
                            error: crate::runner_error("codex_shutdown_failed", error),
                        },
                    };
                    Ok(outcome)
                }
            }
        })
    }
}

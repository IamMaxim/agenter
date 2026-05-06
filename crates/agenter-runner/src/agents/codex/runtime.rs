use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use agenter_core::{
    AgentCapabilities, AgentCollaborationMode, AgentModelOption, AgentOptions, AgentProviderId,
    AgentQuestionAnswer, AgentReasoningEffort, AgentTurnSettings, ApprovalDecision, ApprovalId,
    ItemId, NativeRef, ProviderCapabilityDetail, ProviderCapabilityStatus, QuestionId, SessionId,
    SessionStatus, SlashCommandArgument, SlashCommandArgumentKind, SlashCommandDangerLevel,
    SlashCommandDefinition, SlashCommandRequest, SlashCommandResult, SlashCommandTarget, TurnId,
    UniversalEventKind, UniversalEventSource, UserInput, WorkspaceRef,
};
use agenter_protocol::runner::{
    AgentInput, DiscoveredFileChangeStatus, DiscoveredSession, DiscoveredSessionHistoryItem,
    DiscoveredSessionHistoryStatus, DiscoveredToolStatus,
};
use anyhow::{anyhow, Context};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use tokio::sync::{mpsc, oneshot};

use crate::agents::adapter::{AdapterEvent, AdapterEventSender};

use super::{
    codec::{
        native_ref_for_decoded_frame, CodexDecodedFrame, RequestId, CODEX_APP_SERVER_PROTOCOL,
    },
    id_map::CodexIdMap,
    obligations::{
        codex_approval_response, codex_mcp_elicitation_response, codex_tool_user_input_response,
        unsupported_notification, CodexObligationMapper, CodexServerRequestOutput,
    },
    provider_commands::{
        provider_capability_details, provider_command, provider_command_manifest,
        CodexProviderCommand, CodexProviderCommandAvailability, CodexProviderCommandCategory,
    },
    reducer::{CodexReducer, CodexReducerOutput},
    session::{
        CodexSessionClient, CodexThread, CodexThreadListRequest, CodexThreadResumeRequest,
        CodexThreadStartRequest, CodexThreadTurnsListRequest,
    },
    state::{codex_process_exit_outputs, CodexProcessExit},
    transport::{
        app_server_config_for_workspace, CodexTransport, CodexTransportConfig, CodexTransportEvent,
    },
    turns::{
        interrupt_request_from_universal, start_request_from_universal,
        steer_request_from_universal, CodexTurnClient,
    },
};

#[derive(Clone)]
pub struct CodexRunnerRuntime {
    command_sender: mpsc::UnboundedSender<CodexRuntimeCommand>,
}

#[derive(Clone, Debug)]
pub struct CodexRunnerRegistration {
    pub provider_id: AgentProviderId,
    pub capabilities: AgentCapabilities,
}

#[derive(Clone, Debug)]
pub struct CodexSessionHandle {
    pub session_id: SessionId,
    pub external_session_id: String,
}

#[derive(Debug)]
enum CodexRuntimeCommand {
    CreateSession {
        session_id: SessionId,
        initial_input: Option<AgentInput>,
        respond_to: oneshot::Sender<anyhow::Result<CodexSessionHandle>>,
    },
    ResumeSession {
        session_id: SessionId,
        external_session_id: String,
        respond_to: oneshot::Sender<anyhow::Result<CodexSessionHandle>>,
    },
    SendInput {
        session_id: SessionId,
        external_session_id: Option<String>,
        input: AgentInput,
        settings: Option<AgentTurnSettings>,
        respond_to: oneshot::Sender<anyhow::Result<()>>,
    },
    InterruptSession {
        session_id: SessionId,
        respond_to: oneshot::Sender<anyhow::Result<()>>,
    },
    AnswerApproval {
        approval_id: ApprovalId,
        decision: ApprovalDecision,
        respond_to: oneshot::Sender<anyhow::Result<()>>,
    },
    AnswerQuestion {
        answer: AgentQuestionAnswer,
        respond_to: oneshot::Sender<anyhow::Result<()>>,
    },
    RefreshSessions {
        workspace: WorkspaceRef,
        respond_to: oneshot::Sender<anyhow::Result<Vec<DiscoveredSession>>>,
    },
    GetAgentOptions {
        respond_to: oneshot::Sender<anyhow::Result<AgentOptions>>,
    },
    ShutdownSession {
        session_id: SessionId,
        respond_to: oneshot::Sender<anyhow::Result<()>>,
    },
    ExecuteProviderCommand {
        session_id: SessionId,
        external_session_id: Option<String>,
        command: SlashCommandRequest,
        respond_to: oneshot::Sender<anyhow::Result<SlashCommandResult>>,
    },
}

#[derive(Clone, Debug)]
struct PendingCodexApproval {
    request_id: RequestId,
    method: String,
    raw_payload: Value,
}

#[derive(Clone, Debug)]
struct PendingCodexQuestion {
    request_id: RequestId,
    method: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexModelListResponse {
    #[serde(default)]
    data: Vec<CodexModel>,
    next_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexModel {
    #[serde(default)]
    model: String,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    hidden: bool,
    #[serde(default)]
    is_default: bool,
    default_reasoning_effort: Option<String>,
    #[serde(default)]
    supported_reasoning_efforts: Vec<CodexReasoningEffortOption>,
    #[serde(default)]
    input_modalities: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexReasoningEffortOption {
    reasoning_effort: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexCollaborationModeListResponse {
    #[serde(default)]
    data: Vec<CodexCollaborationModeMask>,
}

#[derive(Debug, Deserialize)]
struct CodexCollaborationModeMask {
    name: String,
    mode: Option<String>,
    model: Option<String>,
    #[serde(default)]
    reasoning_effort: Option<Option<String>>,
}

struct CodexRuntimeActor {
    workspace_path: PathBuf,
    transport: CodexTransport,
    event_sender: AdapterEventSender,
    command_receiver: mpsc::UnboundedReceiver<CodexRuntimeCommand>,
    session_threads: HashMap<SessionId, String>,
    thread_sessions: HashMap<String, SessionId>,
    active_turns: HashMap<SessionId, String>,
    pending_approvals: HashMap<ApprovalId, PendingCodexApproval>,
    pending_questions: HashMap<QuestionId, PendingCodexQuestion>,
}

impl CodexRunnerRuntime {
    pub fn spawn(
        workspace_path: PathBuf,
        event_sender: AdapterEventSender,
    ) -> anyhow::Result<Self> {
        Self::spawn_with_config(
            app_server_config_for_workspace(&workspace_path),
            event_sender,
        )
    }

    pub fn spawn_with_config(
        config: CodexTransportConfig,
        event_sender: AdapterEventSender,
    ) -> anyhow::Result<Self> {
        let workspace_path = config.workspace_path.clone();
        let transport = CodexTransport::spawn(config)?;
        let (command_sender, command_receiver) = mpsc::unbounded_channel();
        let actor = CodexRuntimeActor {
            workspace_path,
            transport,
            event_sender,
            command_receiver,
            session_threads: HashMap::new(),
            thread_sessions: HashMap::new(),
            active_turns: HashMap::new(),
            pending_approvals: HashMap::new(),
            pending_questions: HashMap::new(),
        };
        tokio::spawn(actor.run());
        Ok(Self { command_sender })
    }

    #[must_use]
    pub fn registration() -> CodexRunnerRegistration {
        let capabilities = codex_capabilities();
        CodexRunnerRegistration {
            provider_id: AgentProviderId::from(AgentProviderId::CODEX),
            capabilities,
        }
    }

    pub async fn create_session(
        &self,
        session_id: SessionId,
        initial_input: Option<AgentInput>,
    ) -> anyhow::Result<CodexSessionHandle> {
        self.request(|respond_to| CodexRuntimeCommand::CreateSession {
            session_id,
            initial_input,
            respond_to,
        })
        .await
    }

    pub async fn resume_session(
        &self,
        session_id: SessionId,
        external_session_id: String,
    ) -> anyhow::Result<CodexSessionHandle> {
        self.request(|respond_to| CodexRuntimeCommand::ResumeSession {
            session_id,
            external_session_id,
            respond_to,
        })
        .await
    }

    pub async fn send_input(
        &self,
        session_id: SessionId,
        external_session_id: Option<String>,
        input: AgentInput,
        settings: Option<AgentTurnSettings>,
    ) -> anyhow::Result<()> {
        self.request(|respond_to| CodexRuntimeCommand::SendInput {
            session_id,
            external_session_id,
            input,
            settings,
            respond_to,
        })
        .await
    }

    pub async fn interrupt_session(&self, session_id: SessionId) -> anyhow::Result<()> {
        self.request(|respond_to| CodexRuntimeCommand::InterruptSession {
            session_id,
            respond_to,
        })
        .await
    }

    pub async fn answer_approval(
        &self,
        approval_id: ApprovalId,
        decision: ApprovalDecision,
    ) -> anyhow::Result<()> {
        self.request(|respond_to| CodexRuntimeCommand::AnswerApproval {
            approval_id,
            decision,
            respond_to,
        })
        .await
    }

    pub async fn answer_question(&self, answer: AgentQuestionAnswer) -> anyhow::Result<()> {
        self.request(|respond_to| CodexRuntimeCommand::AnswerQuestion { answer, respond_to })
            .await
    }

    pub async fn refresh_sessions(
        &self,
        workspace: WorkspaceRef,
    ) -> anyhow::Result<Vec<DiscoveredSession>> {
        self.request(|respond_to| CodexRuntimeCommand::RefreshSessions {
            workspace,
            respond_to,
        })
        .await
    }

    pub async fn agent_options(&self) -> anyhow::Result<AgentOptions> {
        self.request(|respond_to| CodexRuntimeCommand::GetAgentOptions { respond_to })
            .await
    }

    pub async fn shutdown_session(&self, session_id: SessionId) -> anyhow::Result<()> {
        self.request(|respond_to| CodexRuntimeCommand::ShutdownSession {
            session_id,
            respond_to,
        })
        .await
    }

    pub async fn execute_provider_command(
        &self,
        session_id: SessionId,
        external_session_id: Option<String>,
        command: SlashCommandRequest,
    ) -> anyhow::Result<SlashCommandResult> {
        self.request(|respond_to| CodexRuntimeCommand::ExecuteProviderCommand {
            session_id,
            external_session_id,
            command,
            respond_to,
        })
        .await
    }

    async fn request<T>(
        &self,
        build: impl FnOnce(oneshot::Sender<anyhow::Result<T>>) -> CodexRuntimeCommand,
    ) -> anyhow::Result<T> {
        let (respond_to, response) = oneshot::channel();
        self.command_sender
            .send(build(respond_to))
            .map_err(|_| anyhow!("Codex runtime actor is stopped"))?;
        response
            .await
            .map_err(|_| anyhow!("Codex runtime actor dropped response"))?
    }
}

impl CodexRuntimeActor {
    async fn run(mut self) {
        match self.transport.initialize(true).await {
            Ok(response) => {
                tracing::info!(
                    raw = %response.raw_payload,
                    "Codex app-server initialized"
                );
            }
            Err(error) => {
                tracing::error!(%error, "failed to initialize Codex app-server");
            }
        }

        loop {
            tokio::select! {
                Some(command) = self.command_receiver.recv() => {
                    self.handle_command(command).await;
                }
                event = self.transport.next_event() => {
                    match event {
                        Ok(event) => {
                            let process_exited = matches!(
                                event,
                                CodexTransportEvent::ProcessExited { .. }
                            );
                            self.handle_transport_event(event).await;
                            if process_exited {
                                tracing::warn!("Codex runtime actor stopped after app-server exit");
                                break;
                            }
                        }
                        Err(error) => {
                            tracing::warn!(%error, "Codex app-server transport event failed");
                            self.emit_error_to_known_sessions("codex_transport_error", error.to_string());
                        }
                    }
                }
            }
        }
    }

    async fn handle_command(&mut self, command: CodexRuntimeCommand) {
        match command {
            CodexRuntimeCommand::CreateSession {
                session_id,
                initial_input,
                respond_to,
            } => {
                let result = self.create_session(session_id, initial_input).await;
                respond_to.send(result).ok();
            }
            CodexRuntimeCommand::ResumeSession {
                session_id,
                external_session_id,
                respond_to,
            } => {
                let result = self.resume_session(session_id, external_session_id).await;
                respond_to.send(result).ok();
            }
            CodexRuntimeCommand::SendInput {
                session_id,
                external_session_id,
                input,
                settings,
                respond_to,
            } => {
                let result = self
                    .send_input(session_id, external_session_id, input, settings)
                    .await;
                respond_to.send(result).ok();
            }
            CodexRuntimeCommand::InterruptSession {
                session_id,
                respond_to,
            } => {
                let result = self.interrupt_session(session_id).await;
                respond_to.send(result).ok();
            }
            CodexRuntimeCommand::AnswerApproval {
                approval_id,
                decision,
                respond_to,
            } => {
                let result = self.answer_approval(approval_id, decision).await;
                respond_to.send(result).ok();
            }
            CodexRuntimeCommand::AnswerQuestion { answer, respond_to } => {
                let result = self.answer_question(answer).await;
                respond_to.send(result).ok();
            }
            CodexRuntimeCommand::RefreshSessions {
                workspace,
                respond_to,
            } => {
                let result = self.refresh_sessions(workspace).await;
                respond_to.send(result).ok();
            }
            CodexRuntimeCommand::GetAgentOptions { respond_to } => {
                let result = self.agent_options().await;
                respond_to.send(result).ok();
            }
            CodexRuntimeCommand::ShutdownSession {
                session_id,
                respond_to,
            } => {
                let result = self.shutdown_session(session_id).await;
                respond_to.send(result).ok();
            }
            CodexRuntimeCommand::ExecuteProviderCommand {
                session_id,
                external_session_id,
                command,
                respond_to,
            } => {
                let result = self
                    .execute_provider_command(session_id, external_session_id, command)
                    .await;
                respond_to.send(result).ok();
            }
        }
    }

    async fn create_session(
        &mut self,
        session_id: SessionId,
        initial_input: Option<AgentInput>,
    ) -> anyhow::Result<CodexSessionHandle> {
        let mut id_map = CodexIdMap::for_session(session_id);
        let operation = CodexSessionClient::new(&mut self.transport)
            .start_thread(
                CodexThreadStartRequest {
                    cwd: Some(self.workspace_path.display().to_string()),
                    persist_extended_history: true,
                    ..Default::default()
                },
                &mut id_map,
            )
            .await?;
        let native_thread_id = operation.thread.native_thread_id.clone();
        self.bind_session(session_id, native_thread_id.clone());
        self.emit_history(session_id, &operation.thread);
        self.emit_lifecycle_status(
            session_id,
            operation.native.clone(),
            operation.thread.status.clone(),
            Some("Codex thread created".to_owned()),
        );

        if let Some(input) = initial_input {
            self.send_input(session_id, Some(native_thread_id.clone()), input, None)
                .await?;
        }

        Ok(CodexSessionHandle {
            session_id,
            external_session_id: native_thread_id,
        })
    }

    async fn resume_session(
        &mut self,
        session_id: SessionId,
        external_session_id: String,
    ) -> anyhow::Result<CodexSessionHandle> {
        let mut id_map = CodexIdMap::for_session(session_id);
        let operation = CodexSessionClient::new(&mut self.transport)
            .resume_thread(
                CodexThreadResumeRequest {
                    thread_id: external_session_id.clone(),
                    cwd: Some(self.workspace_path.display().to_string()),
                    persist_extended_history: true,
                    ..Default::default()
                },
                &mut id_map,
            )
            .await?;
        let native_thread_id = operation.thread.native_thread_id.clone();
        self.bind_session(session_id, native_thread_id.clone());
        self.emit_history(session_id, &operation.thread);
        self.emit_lifecycle_status(
            session_id,
            operation.native.clone(),
            operation.thread.status.clone(),
            Some("Codex thread resumed".to_owned()),
        );
        Ok(CodexSessionHandle {
            session_id,
            external_session_id: native_thread_id,
        })
    }

    async fn send_input(
        &mut self,
        session_id: SessionId,
        external_session_id: Option<String>,
        input: AgentInput,
        settings: Option<AgentTurnSettings>,
    ) -> anyhow::Result<()> {
        let native_thread_id = self
            .ensure_thread_resumed(session_id, external_session_id.clone(), false)
            .await?;
        let input = user_input_from_agent_input(input);
        let settings = self.normalize_turn_settings(settings).await?;
        if let Some(active_turn_id) = self.active_turns.get(&session_id).cloned() {
            let result = self
                .steer_turn_for_session(
                    session_id,
                    native_thread_id.clone(),
                    active_turn_id,
                    &input,
                )
                .await;
            match result {
                Ok(()) => return Ok(()),
                Err(error) if is_codex_thread_not_found_error(&error, "turn/steer") => {
                    self.active_turns.remove(&session_id);
                    self.unbind_session(session_id);
                    let native_thread_id = self
                        .ensure_thread_resumed(
                            session_id,
                            external_session_id.or(Some(native_thread_id)),
                            true,
                        )
                        .await?;
                    return self
                        .start_turn_for_session(session_id, native_thread_id, &input, settings)
                        .await;
                }
                Err(error) => return Err(error),
            }
        }

        let result = self
            .start_turn_for_session(
                session_id,
                native_thread_id.clone(),
                &input,
                settings.clone(),
            )
            .await;
        match result {
            Ok(()) => Ok(()),
            Err(error) if is_codex_thread_not_found_error(&error, "turn/start") => {
                self.unbind_session(session_id);
                let native_thread_id = self
                    .ensure_thread_resumed(
                        session_id,
                        external_session_id.or(Some(native_thread_id)),
                        true,
                    )
                    .await?;
                self.start_turn_for_session(session_id, native_thread_id, &input, settings)
                    .await
            }
            Err(error) => Err(error),
        }
    }

    async fn ensure_thread_resumed(
        &mut self,
        session_id: SessionId,
        external_session_id: Option<String>,
        force: bool,
    ) -> anyhow::Result<String> {
        if !force {
            if let Some(native_thread_id) = self.session_threads.get(&session_id) {
                return Ok(native_thread_id.clone());
            }
        }
        let thread_id = external_session_id
            .or_else(|| self.session_threads.get(&session_id).cloned())
            .context("Codex session has no native thread id")?;
        let mut id_map = CodexIdMap::for_session(session_id);
        let operation = CodexSessionClient::new(&mut self.transport)
            .resume_thread(
                CodexThreadResumeRequest {
                    thread_id: thread_id.clone(),
                    cwd: Some(self.workspace_path.display().to_string()),
                    persist_extended_history: true,
                    ..Default::default()
                },
                &mut id_map,
            )
            .await?;
        let native_thread_id = operation.thread.native_thread_id.clone();
        self.bind_session(session_id, native_thread_id.clone());
        self.emit_history(session_id, &operation.thread);
        self.emit_lifecycle_status(
            session_id,
            operation.native.clone(),
            operation.thread.status.clone(),
            Some("Codex thread resumed for send".to_owned()),
        );
        Ok(native_thread_id)
    }

    async fn steer_turn_for_session(
        &mut self,
        session_id: SessionId,
        native_thread_id: String,
        active_turn_id: String,
        input: &UserInput,
    ) -> anyhow::Result<()> {
        let mut id_map = CodexIdMap::for_session(session_id);
        let result = CodexTurnClient::new(&mut self.transport)
            .steer_turn(
                steer_request_from_universal(native_thread_id, active_turn_id, input, Map::new()),
                &mut id_map,
            )
            .await?;
        self.active_turns
            .insert(session_id, result.native_turn_id.clone());
        self.emit_codex_output(CodexReducerOutput {
            session_id,
            turn_id: Some(result.turn_id),
            item_id: None,
            ts: chrono::Utc::now(),
            native: result.native,
            event: UniversalEventKind::ProviderNotification {
                notification: agenter_core::ProviderNotification {
                    category: "turn".to_owned(),
                    title: "Codex turn steered".to_owned(),
                    detail: Some(result.native_turn_id),
                    status: Some("accepted".to_owned()),
                    severity: None,
                    subject: None,
                },
            },
        });
        Ok(())
    }

    async fn start_turn_for_session(
        &mut self,
        session_id: SessionId,
        native_thread_id: String,
        input: &UserInput,
        settings: Option<AgentTurnSettings>,
    ) -> anyhow::Result<()> {
        let mut id_map = CodexIdMap::for_session(session_id);
        let result = CodexTurnClient::new(&mut self.transport)
            .start_turn(
                start_request_from_universal(
                    native_thread_id,
                    input,
                    settings.as_ref(),
                    Map::new(),
                ),
                &mut id_map,
            )
            .await?;
        self.active_turns
            .insert(session_id, result.native_turn_id.clone());
        let mut reducer = CodexReducer::new(session_id, id_map);
        for item in result
            .turn
            .get("items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            for output in reducer.reduce_history_item(
                &result.native_thread_id,
                &result.native_turn_id,
                item.clone(),
            ) {
                self.emit_codex_output(output);
            }
        }
        Ok(())
    }

    fn unbind_session(&mut self, session_id: SessionId) {
        if let Some(thread_id) = self.session_threads.remove(&session_id) {
            self.thread_sessions.remove(&thread_id);
        }
    }

    async fn normalize_turn_settings(
        &mut self,
        settings: Option<AgentTurnSettings>,
    ) -> anyhow::Result<Option<AgentTurnSettings>> {
        let Some(mut settings) = settings else {
            return Ok(None);
        };
        let Some(mode_id) = settings.collaboration_mode.clone() else {
            return Ok(Some(settings));
        };
        if settings.model.is_some() {
            return Ok(Some(settings));
        }

        let options = self.agent_options().await?;
        if let Some(mode) = options
            .collaboration_modes
            .iter()
            .find(|mode| mode.id == mode_id)
        {
            settings.model = mode.model.clone();
            if settings.reasoning_effort.is_none() {
                settings.reasoning_effort = mode.reasoning_effort.clone();
            }
        }
        if settings.model.is_none() {
            settings.model = options
                .models
                .iter()
                .find(|model| model.is_default)
                .or_else(|| options.models.first())
                .map(|model| model.id.clone());
        }
        Ok(Some(settings))
    }

    async fn interrupt_session(&mut self, session_id: SessionId) -> anyhow::Result<()> {
        let native_thread_id = self
            .session_threads
            .get(&session_id)
            .cloned()
            .context("Codex session has no native thread id")?;
        let native_turn_id = self
            .active_turns
            .get(&session_id)
            .cloned()
            .context("Codex session has no active turn id")?;
        CodexTurnClient::new(&mut self.transport)
            .interrupt_turn(interrupt_request_from_universal(
                native_thread_id,
                native_turn_id,
            ))
            .await?;
        self.active_turns.remove(&session_id);
        Ok(())
    }

    async fn answer_approval(
        &mut self,
        approval_id: ApprovalId,
        decision: ApprovalDecision,
    ) -> anyhow::Result<()> {
        let pending = self
            .pending_approvals
            .remove(&approval_id)
            .context("Codex approval is no longer pending")?;
        let response = codex_approval_response(
            pending.request_id,
            &pending.method,
            &decision,
            Some(&pending.raw_payload),
        );
        self.respond_with_full_payload(response).await
    }

    async fn answer_question(&mut self, answer: AgentQuestionAnswer) -> anyhow::Result<()> {
        let pending = self
            .pending_questions
            .remove(&answer.question_id)
            .context("Codex question is no longer pending")?;
        let response = if pending.method == "mcpServer/elicitation/request" {
            let action = answer
                .answers
                .get("action")
                .and_then(|answers| answers.first())
                .map(String::as_str)
                .unwrap_or("decline");
            let content = answer
                .answers
                .get("content")
                .and_then(|answers| answers.first())
                .and_then(|value| serde_json::from_str::<Value>(value).ok())
                .or_else(|| {
                    answer
                        .answers
                        .get("content")
                        .and_then(|answers| answers.first())
                        .map(|value| json!({ "value": value }))
                });
            codex_mcp_elicitation_response(
                pending.request_id,
                action,
                content,
                Some(json!({"client": "agenter"})),
            )
        } else {
            codex_tool_user_input_response(pending.request_id, &answer)
        };
        self.respond_with_full_payload(response).await
    }

    async fn refresh_sessions(
        &mut self,
        workspace: WorkspaceRef,
    ) -> anyhow::Result<Vec<DiscoveredSession>> {
        let mut id_map = CodexIdMap::with_namespace(uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_URL,
            format!("agenter:codex-refresh:{}", workspace.path).as_bytes(),
        ));
        let listed = CodexSessionClient::new(&mut self.transport)
            .list_threads(
                CodexThreadListRequest {
                    limit: Some(50),
                    cwd: Some(json!(workspace.path)),
                    ..Default::default()
                },
                &mut id_map,
            )
            .await?;

        let mut sessions = Vec::new();
        for thread in listed.threads {
            let mut history = history_items_for_thread(&thread);
            if history.is_empty() {
                if let Ok(page) = CodexSessionClient::new(&mut self.transport)
                    .list_turns(
                        CodexThreadTurnsListRequest {
                            thread_id: thread.native_thread_id.clone(),
                            limit: Some(50),
                            ..Default::default()
                        },
                        &mut id_map,
                    )
                    .await
                {
                    for turn in page.turns {
                        for item in turn.items {
                            history.push(history_item_from_raw(
                                item.native_item_id,
                                item.kind,
                                item.raw_payload,
                            ));
                        }
                    }
                    history = canonicalize_plan_history_items(history);
                }
            }
            sessions.push(DiscoveredSession {
                external_session_id: thread.native_thread_id,
                title: thread.title.or(thread.name).or(thread.preview),
                updated_at: thread.updated_at.map(|ts| ts.to_rfc3339()),
                history_status: DiscoveredSessionHistoryStatus::Loaded,
                history,
            });
        }
        Ok(sessions)
    }

    async fn shutdown_session(&mut self, session_id: SessionId) -> anyhow::Result<()> {
        if let Some(thread_id) = self.session_threads.remove(&session_id) {
            self.thread_sessions.remove(&thread_id);
            CodexSessionClient::new(&mut self.transport)
                .unsubscribe_thread(&thread_id)
                .await
                .ok();
        }
        self.active_turns.remove(&session_id);
        self.emit_lifecycle_status(
            session_id,
            native_ref_for_runtime("thread/unsubscribe", None),
            SessionStatus::Stopped,
            Some("Codex session runtime stopped.".to_owned()),
        );
        Ok(())
    }

    async fn agent_options(&mut self) -> anyhow::Result<AgentOptions> {
        let mut models = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let response = self
                .transport
                .request_response(
                    "model/list",
                    json!({
                        "cursor": cursor,
                        "limit": null,
                        "includeHidden": true,
                    }),
                )
                .await
                .context("Codex model/list failed")?;
            let page: CodexModelListResponse = serde_json::from_value(response.result)
                .context("Codex model/list decode failed")?;
            models.extend(
                page.data
                    .into_iter()
                    .filter_map(agent_model_option_from_codex),
            );
            if page.next_cursor.is_none() {
                break;
            }
            cursor = page.next_cursor;
        }

        let collaboration_modes = match self
            .transport
            .request_response("collaborationMode/list", json!({}))
            .await
        {
            Ok(response) => {
                let response: CodexCollaborationModeListResponse =
                    serde_json::from_value(response.result)
                        .context("Codex collaborationMode/list decode failed")?;
                response
                    .data
                    .into_iter()
                    .map(agent_collaboration_mode_from_codex)
                    .collect()
            }
            Err(error) => {
                tracing::warn!(%error, "Codex collaborationMode/list failed; using empty mode list");
                Vec::new()
            }
        };

        Ok(AgentOptions {
            models,
            collaboration_modes,
        })
    }

    async fn execute_provider_command(
        &mut self,
        session_id: SessionId,
        external_session_id: Option<String>,
        request: SlashCommandRequest,
    ) -> anyhow::Result<SlashCommandResult> {
        let method = method_from_command_id(&request.command_id);
        let Some(command) = provider_command(&method) else {
            return Ok(unsupported_command_result(
                &request.command_id,
                "Codex provider command is not known.",
            ));
        };
        if !provider_command_execution_allowed(command, request.confirmed) {
            return Ok(unsupported_command_result(
                &request.command_id,
                "Codex provider command is guarded or unsupported in this live pass.",
            ));
        }
        if command.category == CodexProviderCommandCategory::ThreadMaintenance {
            self.ensure_thread_resumed(session_id, external_session_id.clone(), false)
                .await?;
        } else if let Some(external_session_id) = external_session_id {
            self.bind_session(session_id, external_session_id);
        }
        let params = provider_command_params(
            request.arguments.clone(),
            self.session_threads.get(&session_id).map(String::as_str),
        );
        let response = self
            .transport
            .request_response(method.clone(), params.clone())
            .await?;
        Ok(SlashCommandResult {
            accepted: true,
            message: format!("Codex `{method}` executed."),
            session: None,
            provider_payload: Some(json!({
                "method": method,
                "request": params,
                "response": response.raw_payload,
            })),
        })
    }

    async fn respond_with_full_payload(&mut self, response: Value) -> anyhow::Result<()> {
        response
            .get("id")
            .context("Codex response payload is missing id")?;
        self.transport.respond_raw(response).await
    }

    async fn handle_transport_event(&mut self, event: CodexTransportEvent) {
        match event {
            CodexTransportEvent::Frame(frame) => self.handle_frame(frame).await,
            CodexTransportEvent::ProcessExited {
                status,
                stderr_excerpt,
            } => {
                let sessions = self.session_threads.keys().copied().collect::<Vec<_>>();
                for session_id in sessions {
                    for output in codex_process_exit_outputs(
                        session_id,
                        CodexProcessExit {
                            status: status.map(|status| status.to_string()),
                            stderr_excerpt: stderr_excerpt.clone(),
                        },
                    ) {
                        self.emit_codex_output(output);
                    }
                }
            }
        }
    }

    async fn handle_frame(&mut self, frame: CodexDecodedFrame) {
        match frame {
            CodexDecodedFrame::ServerNotification(notification) => {
                let Some(session_id) = self.session_for_payload(&notification.raw_payload) else {
                    self.emit_native_to_fallback(
                        notification.raw_payload,
                        "Codex notification without mapped session",
                    );
                    return;
                };
                let id_map = CodexIdMap::for_session(session_id);
                let mut reducer = CodexReducer::new(session_id, id_map);
                for output in reducer.reduce_server_notification(&notification) {
                    if let UniversalEventKind::TurnCompleted { turn }
                    | UniversalEventKind::TurnFailed { turn }
                    | UniversalEventKind::TurnCancelled { turn }
                    | UniversalEventKind::TurnInterrupted { turn } = &output.event
                    {
                        if output.turn_id == Some(turn.turn_id) {
                            self.active_turns.remove(&session_id);
                        }
                    }
                    self.emit_codex_output(output);
                }
            }
            CodexDecodedFrame::ServerRequest(request) => {
                let Some(session_id) = self.session_for_payload(&request.raw_payload) else {
                    let response = super::obligations::codex_unsupported_error_response(
                        request.request_id.clone(),
                        "Agenter could not map Codex request to a session.".to_owned(),
                    );
                    self.respond_with_full_payload(response).await.ok();
                    return;
                };
                let mut mapper = CodexObligationMapper::new(session_id);
                let output = mapper.map_server_request(&request);
                match output {
                    CodexServerRequestOutput::ApprovalRequested(approval) => {
                        self.pending_approvals.insert(
                            approval.approval_id,
                            PendingCodexApproval {
                                request_id: request.request_id,
                                method: request.method,
                                raw_payload: request.raw_payload,
                            },
                        );
                        self.emit_event(
                            session_id,
                            approval.turn_id,
                            approval.item_id,
                            approval.native.clone(),
                            UniversalEventKind::ApprovalRequested {
                                approval: Box::new(approval),
                            },
                        );
                    }
                    CodexServerRequestOutput::QuestionRequested(question) => {
                        self.pending_questions.insert(
                            question.question_id,
                            PendingCodexQuestion {
                                request_id: request.request_id,
                                method: request.method,
                            },
                        );
                        self.emit_event(
                            session_id,
                            question.turn_id,
                            None,
                            question.native.clone(),
                            UniversalEventKind::QuestionRequested {
                                question: Box::new(question),
                            },
                        );
                    }
                    ref unsupported @ CodexServerRequestOutput::NativeUnsupported {
                        ref response,
                        ref native,
                        ..
                    } => {
                        self.respond_with_full_payload(response.clone()).await.ok();
                        if let Some(event) = unsupported_notification(unsupported) {
                            self.emit_event(session_id, None, None, Some(native.clone()), event);
                        }
                    }
                }
            }
            CodexDecodedFrame::Malformed(malformed) => {
                self.emit_native_to_fallback(
                    json!({
                        "line": malformed.line,
                        "error": malformed.error,
                    }),
                    "Malformed Codex app-server frame",
                );
            }
            other => {
                self.emit_native_to_fallback(
                    native_ref_for_decoded_frame(&other)
                        .raw_payload
                        .unwrap_or_else(|| json!({ "kind": "codex_frame" })),
                    "Codex app-server frame",
                );
            }
        }
    }

    fn bind_session(&mut self, session_id: SessionId, native_thread_id: String) {
        self.session_threads
            .insert(session_id, native_thread_id.clone());
        self.thread_sessions.insert(native_thread_id, session_id);
    }

    fn emit_history(&mut self, session_id: SessionId, thread: &CodexThread) {
        let mut reducer = CodexReducer::new(session_id, CodexIdMap::for_session(session_id));
        for turn in &thread.turns {
            if turn
                .status
                .as_deref()
                .is_some_and(|status| !is_terminal_native_turn_status(status))
            {
                self.active_turns
                    .insert(session_id, turn.native_turn_id.clone());
            }
            for item in &turn.items {
                for output in reducer.reduce_history_item(
                    &thread.native_thread_id,
                    &turn.native_turn_id,
                    item.raw_payload.clone(),
                ) {
                    self.emit_codex_output(output);
                }
            }
        }
    }

    fn emit_lifecycle_status(
        &self,
        session_id: SessionId,
        native: NativeRef,
        status: SessionStatus,
        reason: Option<String>,
    ) {
        self.emit_event(
            session_id,
            None,
            None,
            Some(native),
            UniversalEventKind::SessionStatusChanged { status, reason },
        );
    }

    fn emit_codex_output(&self, output: CodexReducerOutput) {
        self.emit_event(
            output.session_id,
            output.turn_id,
            output.item_id,
            Some(output.native),
            output.event,
        );
    }

    fn emit_event(
        &self,
        session_id: SessionId,
        turn_id: Option<TurnId>,
        item_id: Option<ItemId>,
        native: Option<NativeRef>,
        event: UniversalEventKind,
    ) {
        self.event_sender
            .send(AdapterEvent::new(
                Some(session_id),
                turn_id,
                item_id,
                UniversalEventSource::Native,
                native,
                event,
            ))
            .ok();
    }

    fn emit_error_to_known_sessions(&self, code: &str, message: String) {
        for session_id in self.session_threads.keys() {
            self.emit_event(
                *session_id,
                None,
                None,
                Some(native_ref_for_runtime(
                    "transport/error",
                    Some(json!({ "message": message })),
                )),
                UniversalEventKind::ErrorReported {
                    code: Some(code.to_owned()),
                    message: message.clone(),
                },
            );
        }
    }

    fn emit_native_to_fallback(&self, raw_payload: Value, summary: &str) {
        if let Some(session_id) = self.single_session() {
            self.emit_event(
                session_id,
                None,
                None,
                Some(native_ref_for_runtime("native/frame", Some(raw_payload))),
                UniversalEventKind::NativeUnknown {
                    summary: Some(summary.to_owned()),
                },
            );
        }
    }

    fn session_for_payload(&self, payload: &Value) -> Option<SessionId> {
        payload
            .get("params")
            .and_then(|params| params.get("threadId"))
            .and_then(Value::as_str)
            .and_then(|thread_id| self.thread_sessions.get(thread_id).copied())
            .or_else(|| self.single_session())
    }

    fn single_session(&self) -> Option<SessionId> {
        (self.session_threads.len() == 1)
            .then(|| self.session_threads.keys().next().copied())
            .flatten()
    }
}

#[must_use]
pub fn codex_provider_commands() -> Vec<SlashCommandDefinition> {
    provider_command_manifest()
        .iter()
        .map(slash_definition_for_provider_command)
        .collect()
}

#[must_use]
pub fn codex_capabilities() -> AgentCapabilities {
    let mut provider_details = provider_capability_details();
    provider_details.push(ProviderCapabilityDetail {
        key: "dynamic_tools".to_owned(),
        status: ProviderCapabilityStatus::Unsupported,
        methods: vec!["item/tool/call".to_owned()],
        reason: Some(
            "Dynamic client tools are visible but not executed by Agenter yet.".to_owned(),
        ),
    });
    AgentCapabilities {
        streaming: true,
        session_resume: true,
        session_history: true,
        approvals: true,
        file_changes: true,
        command_execution: true,
        plan_updates: true,
        interrupt: true,
        model_selection: true,
        reasoning_effort: true,
        collaboration_modes: true,
        tool_user_input: true,
        mcp_elicitation: true,
        provider_details,
    }
}

fn slash_definition_for_provider_command(command: &CodexProviderCommand) -> SlashCommandDefinition {
    SlashCommandDefinition {
        id: command_id_for_method(command.method),
        name: command.method.to_owned(),
        aliases: Vec::new(),
        description: command.label.to_owned(),
        category: format!("codex/{}", command.category.as_str()),
        provider_id: Some(AgentProviderId::from(AgentProviderId::CODEX)),
        target: SlashCommandTarget::Provider,
        danger_level: danger_level_for_command(command),
        arguments: vec![SlashCommandArgument {
            name: "params".to_owned(),
            kind: SlashCommandArgumentKind::Rest,
            required: false,
            description: Some(format!(
                "JSON params for `{}`; schema {} -> {}",
                command.method, command.schema.params_type, command.schema.response_type
            )),
            choices: Vec::new(),
        }],
        examples: vec![format!("/{}", command.method)],
    }
}

fn command_id_for_method(method: &str) -> String {
    format!("codex.{}", method.replace('/', "."))
}

fn method_from_command_id(command_id: &str) -> String {
    let stripped = command_id.strip_prefix("codex.").unwrap_or(command_id);
    if provider_command(stripped).is_some() {
        stripped.to_owned()
    } else {
        stripped.replace('.', "/")
    }
}

fn danger_level_for_command(command: &CodexProviderCommand) -> SlashCommandDangerLevel {
    match command.availability {
        CodexProviderCommandAvailability::Supported => SlashCommandDangerLevel::Safe,
        CodexProviderCommandAvailability::Guarded
        | CodexProviderCommandAvailability::Experimental
        | CodexProviderCommandAvailability::PlatformGated => SlashCommandDangerLevel::Confirm,
        CodexProviderCommandAvailability::Disabled
        | CodexProviderCommandAvailability::Unsupported => SlashCommandDangerLevel::Dangerous,
    }
}

fn provider_command_execution_allowed(command: &CodexProviderCommand, confirmed: bool) -> bool {
    match command.availability {
        CodexProviderCommandAvailability::Supported => true,
        CodexProviderCommandAvailability::Guarded => {
            confirmed && command.category == CodexProviderCommandCategory::ThreadMaintenance
        }
        CodexProviderCommandAvailability::Experimental
        | CodexProviderCommandAvailability::PlatformGated
        | CodexProviderCommandAvailability::Disabled
        | CodexProviderCommandAvailability::Unsupported => false,
    }
}

fn provider_command_params(arguments: Value, native_thread_id: Option<&str>) -> Value {
    let mut params = match arguments {
        Value::Object(object) => object,
        Value::String(value) if !value.trim().is_empty() => {
            serde_json::from_str::<Map<String, Value>>(&value).unwrap_or_default()
        }
        _ => Map::new(),
    };
    if !params.contains_key("threadId") {
        if let Some(thread_id) = native_thread_id {
            params.insert("threadId".to_owned(), Value::String(thread_id.to_owned()));
        }
    }
    Value::Object(params)
}

fn agent_model_option_from_codex(model: CodexModel) -> Option<AgentModelOption> {
    if model.model.trim().is_empty() {
        return None;
    }
    let supported_reasoning_efforts = model
        .supported_reasoning_efforts
        .into_iter()
        .filter_map(|effort| agent_reasoning_effort(&effort.reasoning_effort))
        .collect::<Vec<_>>();
    Some(AgentModelOption {
        id: model.model.clone(),
        display_name: non_empty_or(model.display_name, &model.model),
        description: non_empty_description(model.description, model.hidden),
        is_default: model.is_default,
        default_reasoning_effort: model
            .default_reasoning_effort
            .as_deref()
            .and_then(agent_reasoning_effort),
        supported_reasoning_efforts,
        input_modalities: model.input_modalities,
    })
}

fn agent_collaboration_mode_from_codex(mask: CodexCollaborationModeMask) -> AgentCollaborationMode {
    let id = mask
        .mode
        .as_deref()
        .filter(|mode| !mode.trim().is_empty())
        .unwrap_or(&mask.name)
        .to_owned();
    AgentCollaborationMode {
        id,
        label: non_empty_or(mask.name, "Custom"),
        model: mask.model,
        reasoning_effort: mask
            .reasoning_effort
            .flatten()
            .as_deref()
            .and_then(agent_reasoning_effort),
    }
}

fn agent_reasoning_effort(value: &str) -> Option<AgentReasoningEffort> {
    match value {
        "none" => Some(AgentReasoningEffort::None),
        "minimal" => Some(AgentReasoningEffort::Minimal),
        "low" => Some(AgentReasoningEffort::Low),
        "medium" => Some(AgentReasoningEffort::Medium),
        "high" => Some(AgentReasoningEffort::High),
        "xhigh" => Some(AgentReasoningEffort::Xhigh),
        _ => None,
    }
}

fn non_empty_or(value: String, fallback: &str) -> String {
    if value.trim().is_empty() {
        fallback.to_owned()
    } else {
        value
    }
}

fn non_empty_description(value: String, hidden: bool) -> Option<String> {
    if value.trim().is_empty() && !hidden {
        None
    } else if hidden && value.trim().is_empty() {
        Some("Hidden Codex model.".to_owned())
    } else {
        Some(value)
    }
}

fn unsupported_command_result(command_id: &str, message: &str) -> SlashCommandResult {
    SlashCommandResult {
        accepted: false,
        message: message.to_owned(),
        session: None,
        provider_payload: Some(json!({
            "command_id": command_id,
            "status": "unsupported",
            "message": message,
        })),
    }
}

fn user_input_from_agent_input(input: AgentInput) -> UserInput {
    match input {
        AgentInput::Text { text } => UserInput::Text { text },
        AgentInput::UserMessage { payload } => UserInput::Text {
            text: payload.content,
        },
    }
}

fn history_items_for_thread(thread: &CodexThread) -> Vec<DiscoveredSessionHistoryItem> {
    let items = thread
        .turns
        .iter()
        .flat_map(|turn| {
            turn.items.iter().map(|item| {
                history_item_from_raw(
                    item.native_item_id.clone(),
                    item.kind.clone(),
                    item.raw_payload.clone(),
                )
            })
        })
        .collect();
    canonicalize_plan_history_items(items)
}

fn history_item_from_raw(
    native_item_id: String,
    kind: Option<String>,
    raw_payload: Value,
) -> DiscoveredSessionHistoryItem {
    let kind = kind.unwrap_or_else(|| {
        raw_payload
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("native")
            .to_owned()
    });
    match kind.as_str() {
        "agentMessage" => DiscoveredSessionHistoryItem::AgentMessage {
            message_id: native_item_id,
            content: raw_payload
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
        },
        "plan" => DiscoveredSessionHistoryItem::Plan {
            plan_id: native_item_id,
            title: Some("Codex plan".to_owned()),
            content: plan_text_from_raw_payload(&raw_payload),
            provider_payload: Some(raw_payload),
        },
        "commandExecution" => DiscoveredSessionHistoryItem::Command {
            command_id: native_item_id,
            command: raw_payload
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or("codex command")
                .to_owned(),
            cwd: raw_payload
                .get("cwd")
                .and_then(Value::as_str)
                .map(str::to_owned),
            source: Some("codex".to_owned()),
            process_id: None,
            duration_ms: None,
            actions: Vec::new(),
            output: raw_payload
                .get("output")
                .and_then(Value::as_str)
                .map(str::to_owned),
            exit_code: None,
            success: true,
            provider_payload: Some(raw_payload),
        },
        "fileChange" => DiscoveredSessionHistoryItem::FileChange {
            change_id: native_item_id,
            path: raw_payload
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("Codex file change")
                .to_owned(),
            change_kind: agenter_core::FileChangeKind::Modify,
            status: DiscoveredFileChangeStatus::Applied,
            diff: raw_payload
                .get("diff")
                .and_then(Value::as_str)
                .map(str::to_owned),
            provider_payload: Some(raw_payload),
        },
        "mcpToolCall" => DiscoveredSessionHistoryItem::Tool {
            tool_call_id: native_item_id,
            name: raw_payload
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("Codex tool")
                .to_owned(),
            title: Some("Codex tool".to_owned()),
            status: DiscoveredToolStatus::Completed,
            input: raw_payload.get("arguments").cloned(),
            output: raw_payload.get("result").cloned(),
            provider_payload: Some(raw_payload),
        },
        _ => DiscoveredSessionHistoryItem::NativeNotification {
            event_id: Some(native_item_id),
            category: "codex_thread_item".to_owned(),
            title: format!("Codex {kind}"),
            detail: None,
            status: None,
            provider_payload: Some(raw_payload),
        },
    }
}

fn plan_text_from_raw_payload(raw_payload: &Value) -> String {
    raw_payload
        .get("text")
        .and_then(Value::as_str)
        .or_else(|| raw_payload.get("content").and_then(Value::as_str))
        .unwrap_or("")
        .to_owned()
}

fn canonicalize_plan_history_items(
    items: Vec<DiscoveredSessionHistoryItem>,
) -> Vec<DiscoveredSessionHistoryItem> {
    let plan_fingerprints = items
        .iter()
        .filter_map(|item| match item {
            DiscoveredSessionHistoryItem::Plan { content, .. } => {
                normalized_history_text(content).filter(|fingerprint| !fingerprint.is_empty())
            }
            _ => None,
        })
        .collect::<HashSet<_>>();

    if plan_fingerprints.is_empty() {
        return items;
    }

    items
        .into_iter()
        .filter(|item| match item {
            DiscoveredSessionHistoryItem::AgentMessage { content, .. } => {
                match normalized_history_text(content) {
                    Some(fingerprint) => !plan_fingerprints.contains(&fingerprint),
                    None => true,
                }
            }
            _ => true,
        })
        .collect()
}

fn normalized_history_text(text: &str) -> Option<String> {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (!normalized.is_empty()).then_some(normalized)
}

fn native_ref_for_runtime(method: &str, raw_payload: Option<Value>) -> NativeRef {
    NativeRef {
        protocol: CODEX_APP_SERVER_PROTOCOL.to_owned(),
        method: Some(method.to_owned()),
        kind: Some("runtime".to_owned()),
        native_id: None,
        summary: Some("Codex runtime event".to_owned()),
        hash: None,
        pointer: None,
        raw_payload,
    }
}

fn is_terminal_native_turn_status(status: &str) -> bool {
    matches!(
        status,
        "completed" | "failed" | "cancelled" | "canceled" | "interrupted"
    )
}

fn is_codex_thread_not_found_error(error: &anyhow::Error, method: &str) -> bool {
    let message = error.to_string();
    message.contains(&format!("Codex app-server `{method}` request"))
        && message.contains("thread not found:")
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use agenter_core::{ApprovalStatus, QuestionStatus};
    use serde_json::json;

    use crate::agents::codex::transport::CodexTransportConfig;

    use super::*;

    fn fake_runtime(script: &str, event_sender: AdapterEventSender) -> CodexRunnerRuntime {
        let config = CodexTransportConfig::command(
            "/bin/sh",
            ["-c", script],
            std::env::current_dir().expect("test process should have current dir"),
        )
        .with_request_timeout(Duration::from_millis(500));
        CodexRunnerRuntime::spawn_with_config(config, event_sender).expect("runtime should spawn")
    }

    fn create_response(id: i64, thread_id: &str) -> String {
        serde_json::to_string(&json!({
            "id": id,
            "result": {
                "thread": {
                    "id": thread_id,
                    "preview": "Codex test",
                    "name": "Codex test",
                    "modelProvider": "openai",
                    "createdAt": 1_700_000_000,
                    "updatedAt": 1_700_000_001,
                    "status": { "type": "idle" },
                    "turns": []
                }
            }
        }))
        .unwrap()
    }

    fn turn_response(id: i64, turn_id: &str) -> String {
        serde_json::to_string(&json!({
            "id": id,
            "result": {
                "turn": {
                    "id": turn_id,
                    "items": []
                }
            }
        }))
        .unwrap()
    }

    #[test]
    fn codex_force_reload_projects_plan_history_text_without_json_wrapping() {
        let raw_payload = json!({
            "type": "plan",
            "id": "plan-1",
            "text": "# Scratch-Direction Exercise\n\n## Summary\nUse the current scratch workspace.",
        });

        let item = history_item_from_raw(
            "plan-1".to_owned(),
            Some("plan".to_owned()),
            raw_payload.clone(),
        );

        let DiscoveredSessionHistoryItem::Plan {
            content,
            provider_payload,
            ..
        } = item
        else {
            panic!("expected plan history item");
        };
        assert_eq!(
            content,
            "# Scratch-Direction Exercise\n\n## Summary\nUse the current scratch workspace."
        );
        assert_eq!(provider_payload, Some(raw_payload));
    }

    #[test]
    fn codex_force_reload_drops_agent_message_that_duplicates_plan_text() {
        let plan_payload = json!({
            "type": "plan",
            "id": "plan-1",
            "text": "# Plan\n\nImplement it.",
        });
        let items = canonicalize_plan_history_items(vec![
            history_item_from_raw(
                "agent-1".to_owned(),
                Some("agentMessage".to_owned()),
                json!({ "type": "agentMessage", "id": "agent-1", "text": "# Plan\r\n\r\nImplement it." }),
            ),
            history_item_from_raw("plan-1".to_owned(), Some("plan".to_owned()), plan_payload),
            history_item_from_raw(
                "agent-2".to_owned(),
                Some("agentMessage".to_owned()),
                json!({ "type": "agentMessage", "id": "agent-2", "text": "A separate assistant note." }),
            ),
        ]);

        assert_eq!(items.len(), 2);
        assert!(matches!(
            items[0],
            DiscoveredSessionHistoryItem::Plan { .. }
        ));
        assert!(matches!(
            &items[1],
            DiscoveredSessionHistoryItem::AgentMessage { content, .. } if content == "A separate assistant note."
        ));
    }

    #[tokio::test]
    async fn codex_force_reload_fallback_turns_list_drops_duplicate_agent_plan_echo() {
        let thread = json!({
            "id": "thread-1",
            "preview": "Codex test",
            "status": { "type": "idle" },
            "turns": []
        });
        let turn = json!({
            "id": "turn-1",
            "items": [
                { "type": "agentMessage", "id": "agent-plan", "text": "# Plan\n\nImplement it." },
                { "type": "plan", "id": "plan-1", "text": "# Plan\n\nImplement it." }
            ],
            "status": { "type": "completed" }
        });
        let list_response = serde_json::to_string(&json!({
            "id": 2,
            "result": {
                "data": [thread],
                "nextCursor": null,
                "backwardsCursor": null
            }
        }))
        .unwrap();
        let turns_response = serde_json::to_string(&json!({
            "id": 3,
            "result": {
                "data": [turn],
                "nextCursor": null,
                "backwardsCursor": null
            }
        }))
        .unwrap();
        let script = format!(
            concat!(
                "read line\nprintf '%s\\n' '{{\"id\":1,\"result\":{{\"ok\":true}}}}'\n",
                "python3 -c 'import json,sys; req=json.loads(sys.stdin.readline()); ",
                "assert req[\"method\"]==\"thread/list\", req; ",
                "print({list_response:?}, flush=True)'\n",
                "python3 -c 'import json,sys; req=json.loads(sys.stdin.readline()); ",
                "assert req[\"method\"]==\"thread/turns/list\", req; ",
                "print({turns_response:?}, flush=True)'\n"
            ),
            list_response = list_response,
            turns_response = turns_response,
        );
        let (sender, _receiver) = mpsc::unbounded_channel();
        let runtime = fake_runtime(&script, sender);

        let sessions = runtime
            .refresh_sessions(WorkspaceRef {
                workspace_id: agenter_core::WorkspaceId::new(),
                runner_id: agenter_core::RunnerId::new(),
                path: "/workspace".to_owned(),
                display_name: None,
            })
            .await
            .unwrap();

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].history.len(), 1);
        assert!(matches!(
            sessions[0].history[0],
            DiscoveredSessionHistoryItem::Plan { .. }
        ));
    }

    #[tokio::test]
    async fn codex_runtime_hello_capabilities_and_manifest_are_codex() {
        let registration = CodexRunnerRuntime::registration();
        assert_eq!(registration.provider_id.as_str(), AgentProviderId::CODEX);
        assert!(registration.capabilities.session_resume);
        assert!(registration.capabilities.interrupt);
        assert!(!codex_provider_commands().is_empty());
        assert!(codex_provider_commands()
            .iter()
            .any(|command| command.id == "codex.thread.name.set"));
    }

    #[tokio::test]
    async fn codex_runtime_create_session_maps_native_thread() {
        let script = format!(
            "read line\nprintf '%s\\n' '{{\"id\":1,\"result\":{{\"ok\":true}}}}'\nread line\nprintf '%s\\n' '{}'\n",
            create_response(2, "thread-1")
        );
        let (sender, _receiver) = mpsc::unbounded_channel();
        let runtime = fake_runtime(&script, sender);

        let session_id = SessionId::new();
        let handle = runtime.create_session(session_id, None).await.unwrap();

        assert_eq!(handle.session_id, session_id);
        assert_eq!(handle.external_session_id, "thread-1");
    }

    #[tokio::test]
    async fn codex_runtime_cold_send_resumes_external_thread_before_turn_start() {
        let script = format!(
            concat!(
                "read line\nprintf '%s\\n' '{{\"id\":1,\"result\":{{\"ok\":true}}}}'\n",
                "python3 -c 'import json,sys; req=json.loads(sys.stdin.readline()); ",
                "assert req[\"method\"]==\"thread/resume\", req; ",
                "assert req[\"params\"][\"threadId\"]==\"thread-1\", req; ",
                "print({resume:?}, flush=True)'\n",
                "python3 -c 'import json,sys; req=json.loads(sys.stdin.readline()); ",
                "assert req[\"method\"]==\"turn/start\", req; ",
                "assert req[\"params\"][\"threadId\"]==\"thread-1\", req; ",
                "print({turn:?}, flush=True)'\n"
            ),
            resume = create_response(2, "thread-1"),
            turn = turn_response(3, "turn-1"),
        );
        let (sender, _receiver) = mpsc::unbounded_channel();
        let runtime = fake_runtime(&script, sender);
        let session_id = SessionId::new();

        runtime
            .send_input(
                session_id,
                Some("thread-1".to_owned()),
                AgentInput::Text {
                    text: "after restart".to_owned(),
                },
                None,
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn codex_runtime_retries_start_after_thread_not_found_by_resuming() {
        let script = format!(
            concat!(
                "read line\nprintf '%s\\n' '{{\"id\":1,\"result\":{{\"ok\":true}}}}'\n",
                "read line\nprintf '%s\\n' '{create}'\n",
                "python3 -c 'import json,sys; req=json.loads(sys.stdin.readline()); ",
                "assert req[\"method\"]==\"turn/start\", req; ",
                "print(\"{{\\\"id\\\":3,\\\"error\\\":{{\\\"code\\\":-32600,\\\"message\\\":\\\"thread not found: thread-1\\\"}}}}\", flush=True)'\n",
                "python3 -c 'import json,sys; req=json.loads(sys.stdin.readline()); ",
                "assert req[\"method\"]==\"thread/resume\", req; ",
                "assert req[\"params\"][\"threadId\"]==\"thread-1\", req; ",
                "print({resume:?}, flush=True)'\n",
                "python3 -c 'import json,sys; req=json.loads(sys.stdin.readline()); ",
                "assert req[\"method\"]==\"turn/start\", req; ",
                "assert req[\"params\"][\"threadId\"]==\"thread-1\", req; ",
                "print({turn:?}, flush=True)'\n"
            ),
            create = create_response(2, "thread-1"),
            resume = create_response(4, "thread-1"),
            turn = turn_response(5, "turn-1"),
        );
        let (sender, _receiver) = mpsc::unbounded_channel();
        let runtime = fake_runtime(&script, sender);
        let session_id = SessionId::new();
        runtime.create_session(session_id, None).await.unwrap();

        runtime
            .send_input(
                session_id,
                None,
                AgentInput::Text {
                    text: "retry me".to_owned(),
                },
                None,
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn codex_runtime_reports_missing_thread_when_forced_resume_fails() {
        let script = format!(
            concat!(
                "read line\nprintf '%s\\n' '{{\"id\":1,\"result\":{{\"ok\":true}}}}'\n",
                "read line\nprintf '%s\\n' '{create}'\n",
                "read line\nprintf '%s\\n' '{{\"id\":3,\"error\":{{\"code\":-32600,\"message\":\"thread not found: thread-1\"}}}}'\n",
                "read line\nprintf '%s\\n' '{{\"id\":4,\"error\":{{\"code\":-32600,\"message\":\"thread not found: thread-1\"}}}}'\n"
            ),
            create = create_response(2, "thread-1"),
        );
        let (sender, _receiver) = mpsc::unbounded_channel();
        let runtime = fake_runtime(&script, sender);
        let session_id = SessionId::new();
        runtime.create_session(session_id, None).await.unwrap();

        let error = runtime
            .send_input(
                session_id,
                None,
                AgentInput::Text {
                    text: "missing".to_owned(),
                },
                None,
            )
            .await
            .expect_err("missing native thread should be reported");

        assert!(error.to_string().contains("thread not found: thread-1"));
    }

    #[tokio::test]
    async fn codex_runtime_server_request_emits_pending_approval() {
        let script = concat!(
            "read line\nprintf '%s\n' '{\"id\":1,\"result\":{\"ok\":true}}'\n",
            "read line\nprintf '%s\n' '{\"id\":2,\"result\":{\"thread\":{\"id\":\"thread-1\",\"preview\":\"Codex\",\"status\":{\"type\":\"active\",\"activeFlags\":[\"waitingOnApproval\"]},\"turns\":[]}}}'\n",
            "printf '%s\n' '{\"id\":\"approval-1\",\"method\":\"item/commandExecution/requestApproval\",\"params\":{\"threadId\":\"thread-1\",\"turnId\":\"turn-1\",\"itemId\":\"item-1\",\"command\":\"printf test\"}}'\n",
            "read line\n"
        );
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let runtime = fake_runtime(script, sender);
        let session_id = SessionId::new();
        runtime.create_session(session_id, None).await.unwrap();

        for _ in 0..4 {
            let event = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
                .await
                .unwrap()
                .unwrap();

            if let UniversalEventKind::ApprovalRequested { approval } = event.universal.event {
                assert_eq!(approval.session_id, session_id);
                assert_eq!(approval.status, ApprovalStatus::Pending);
                assert_eq!(approval.native_request_id.as_deref(), Some("approval-1"));
                assert!(approval.native.unwrap().raw_payload.is_some());
                return;
            }
        }
        panic!("expected approval request event");
    }

    #[tokio::test]
    async fn codex_runtime_approval_answer_writes_exactly_one_native_response() {
        let script = concat!(
            "read line\nprintf '%s\n' '{\"id\":1,\"result\":{\"ok\":true}}'\n",
            "read line\nprintf '%s\n' '{\"id\":2,\"result\":{\"thread\":{\"id\":\"thread-1\",\"preview\":\"Codex\",\"status\":{\"type\":\"active\",\"activeFlags\":[\"waitingOnApproval\"]},\"turns\":[]}}}'\n",
            "printf '%s\n' '{\"id\":\"approval-1\",\"method\":\"item/commandExecution/requestApproval\",\"params\":{\"threadId\":\"thread-1\",\"turnId\":\"turn-1\",\"itemId\":\"item-1\",\"approvalId\":\"native-approval\",\"command\":\"printf test\"}}'\n",
            "python3 -c 'import json,sys; resp=json.loads(sys.stdin.readline()); ",
            "assert resp[\"id\"]==\"approval-1\", resp; ",
            "assert resp[\"result\"][\"decision\"]==\"accept\", resp; ",
            "print(\"{\\\"id\\\":3,\\\"method\\\":\\\"thread/updated\\\",\\\"params\\\":{\\\"threadId\\\":\\\"thread-1\\\"}}\", flush=True)'\n",
        );
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let runtime = fake_runtime(script, sender);
        let session_id = SessionId::new();
        runtime.create_session(session_id, None).await.unwrap();

        let approval = loop {
            let event = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
                .await
                .unwrap()
                .unwrap();
            if let UniversalEventKind::ApprovalRequested { approval } = event.universal.event {
                break approval;
            }
        };

        runtime
            .answer_approval(approval.approval_id, ApprovalDecision::Accept)
            .await
            .unwrap();
        let error = runtime
            .answer_approval(approval.approval_id, ApprovalDecision::Accept)
            .await
            .expect_err("completed Codex approval is removed from the runtime pending map");
        assert!(error.to_string().contains("no longer pending"));
    }

    #[tokio::test]
    async fn codex_runtime_provider_command_guarding_is_explicit() {
        let (sender, _receiver) = mpsc::unbounded_channel();
        let runtime = fake_runtime(
            "read line\nprintf '%s\\n' '{\"id\":1,\"result\":{\"ok\":true}}'\n",
            sender,
        );
        let result = runtime
            .execute_provider_command(
                SessionId::new(),
                None,
                SlashCommandRequest {
                    command_id: "codex.fs.writeFile".to_owned(),
                    universal_command_id: None,
                    idempotency_key: None,
                    arguments: json!({}),
                    raw_input: "/fs/writeFile".to_owned(),
                    confirmed: true,
                },
            )
            .await
            .unwrap();

        assert!(!result.accepted);
        assert_eq!(result.provider_payload.unwrap()["status"], "unsupported");
    }

    #[tokio::test]
    async fn codex_runtime_loads_agent_options_from_app_server() {
        let script = concat!(
            "read line\nprintf '%s\n' '{\"id\":1,\"result\":{\"ok\":true}}'\n",
            "read line\nprintf '%s\n' '{\"id\":2,\"result\":{\"data\":[{\"id\":\"m-1\",\"model\":\"gpt-5.2\",\"displayName\":\"GPT-5.2\",\"description\":\"Balanced\",\"hidden\":false,\"isDefault\":true,\"defaultReasoningEffort\":\"medium\",\"supportedReasoningEfforts\":[{\"reasoningEffort\":\"low\",\"description\":\"Fast\"},{\"reasoningEffort\":\"medium\",\"description\":\"Balanced\"}],\"inputModalities\":[\"text\",\"image\"]}],\"nextCursor\":null}}'\n",
            "read line\nprintf '%s\n' '{\"id\":3,\"result\":{\"data\":[{\"name\":\"Auto\",\"mode\":\"auto\",\"model\":\"gpt-5.2\",\"reasoning_effort\":\"medium\"},{\"name\":\"Plan\",\"mode\":\"plan\",\"model\":null,\"reasoning_effort\":null}]}}'\n"
        );
        let (sender, _receiver) = mpsc::unbounded_channel();
        let runtime = fake_runtime(script, sender);

        let options = runtime.agent_options().await.unwrap();

        assert_eq!(options.models.len(), 1);
        assert_eq!(options.models[0].id, "gpt-5.2");
        assert_eq!(options.models[0].display_name, "GPT-5.2");
        assert!(options.models[0].is_default);
        assert_eq!(
            options.models[0].default_reasoning_effort,
            Some(AgentReasoningEffort::Medium)
        );
        assert_eq!(
            options.models[0].supported_reasoning_efforts,
            vec![AgentReasoningEffort::Low, AgentReasoningEffort::Medium]
        );
        assert_eq!(options.models[0].input_modalities, vec!["text", "image"]);
        assert_eq!(options.collaboration_modes.len(), 2);
        assert_eq!(options.collaboration_modes[0].id, "auto");
        assert_eq!(
            options.collaboration_modes[0].model.as_deref(),
            Some("gpt-5.2")
        );
        assert_eq!(
            options.collaboration_modes[0].reasoning_effort,
            Some(AgentReasoningEffort::Medium)
        );
        assert_eq!(options.collaboration_modes[1].id, "plan");
        assert_eq!(options.collaboration_modes[1].reasoning_effort, None);
    }

    #[tokio::test]
    async fn codex_runtime_normalizes_sparse_collaboration_mode_before_turn_start() {
        let script = concat!(
            "read line\nprintf '%s\n' '{\"id\":1,\"result\":{\"ok\":true}}'\n",
            "read line\nprintf '%s\n' '{\"id\":2,\"result\":{\"thread\":{\"id\":\"thread-1\",\"preview\":\"Codex\",\"status\":{\"type\":\"idle\"},\"turns\":[]}}}'\n",
            "read line\nprintf '%s\n' '{\"id\":3,\"result\":{\"data\":[{\"id\":\"m-1\",\"model\":\"gpt-5.2\",\"displayName\":\"GPT-5.2\",\"description\":\"Balanced\",\"hidden\":false,\"isDefault\":true,\"defaultReasoningEffort\":\"medium\",\"supportedReasoningEfforts\":[{\"reasoningEffort\":\"medium\",\"description\":\"Balanced\"}],\"inputModalities\":[\"text\"]}],\"nextCursor\":null}}'\n",
            "read line\nprintf '%s\n' '{\"id\":4,\"result\":{\"data\":[{\"name\":\"Plan\",\"mode\":\"plan\",\"model\":null,\"reasoning_effort\":null}]}}'\n",
            "python3 -c 'import json,sys; req=json.loads(sys.stdin.readline()); params=req[\"params\"]; assert req[\"method\"]==\"turn/start\", req; assert params[\"collaborationMode\"]=={\"mode\":\"plan\",\"settings\":{\"model\":\"gpt-5.2\",\"reasoning_effort\":None,\"developer_instructions\":None}}, params; print(json.dumps({\"id\": req[\"id\"], \"result\": {\"turn\": {\"id\":\"turn-1\", \"items\": []}}}), flush=True)'\n"
        );
        let (sender, _receiver) = mpsc::unbounded_channel();
        let runtime = fake_runtime(script, sender);
        let session_id = SessionId::new();
        runtime.create_session(session_id, None).await.unwrap();

        runtime
            .send_input(
                session_id,
                None,
                AgentInput::Text {
                    text: "implement".to_owned(),
                },
                Some(AgentTurnSettings {
                    model: None,
                    reasoning_effort: None,
                    collaboration_mode: Some("plan".to_owned()),
                    approval_mode: None,
                }),
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn codex_runtime_question_response_is_routed() {
        let script = concat!(
            "read line\nprintf '%s\n' '{\"id\":1,\"result\":{\"ok\":true}}'\n",
            "read line\nprintf '%s\n' '{\"id\":2,\"result\":{\"thread\":{\"id\":\"thread-1\",\"preview\":\"Codex\",\"status\":{\"type\":\"active\",\"activeFlags\":[\"waitingOnUserInput\"]},\"turns\":[]}}}'\n",
            "printf '%s\n' '{\"id\":\"question-1\",\"method\":\"item/tool/requestUserInput\",\"params\":{\"threadId\":\"thread-1\",\"turnId\":\"turn-1\",\"itemId\":\"item-1\",\"questions\":[{\"id\":\"name\",\"label\":\"Name\",\"type\":\"text\"}]}}'\n",
            "read line\npython3 -c 'import json,sys; req=json.loads(sys.stdin.readline()); assert req[\"id\"]==\"question-1\"; assert req[\"result\"][\"answers\"][\"name\"][\"answers\"]==[\"Max\"]; print(json.dumps({\"ok\": True}), flush=True)'\n"
        );
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let runtime = fake_runtime(script, sender);
        let session_id = SessionId::new();
        runtime.create_session(session_id, None).await.unwrap();

        let mut question_id = None;
        for _ in 0..4 {
            let event = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
                .await
                .unwrap()
                .unwrap();
            if let UniversalEventKind::QuestionRequested { question } = event.universal.event {
                assert_eq!(question.status, QuestionStatus::Pending);
                question_id = Some(question.question_id);
                break;
            }
        }
        let question_id = question_id.expect("expected question request event");

        runtime
            .answer_question(AgentQuestionAnswer {
                question_id,
                answers: [("name".to_owned(), vec!["Max".to_owned()])]
                    .into_iter()
                    .collect(),
            })
            .await
            .unwrap();
    }
}

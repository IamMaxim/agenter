use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use agenter_core::{
    AgentCollaborationMode, AgentErrorEvent, AgentMessageDeltaEvent, AgentModelOption,
    AgentOptions, AgentQuestionAnswer, AgentQuestionChoice, AgentQuestionField,
    AgentReasoningEffort, AgentTurnSettings, AppEvent, ApprovalDecision, ApprovalId, ApprovalKind,
    ApprovalRequestEvent, CommandCompletedEvent, CommandEvent, FileChangeEvent, FileChangeKind,
    MessageCompletedEvent, QuestionId, QuestionRequestedEvent, SessionId,
};
use agenter_protocol::{
    DiscoveredCommandAction, DiscoveredFileChangeStatus, DiscoveredSessionHistoryItem,
    DiscoveredToolStatus,
};
use anyhow::{anyhow, Context};
use serde_json::{json, Value};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::{mpsc, oneshot},
    time::timeout,
};

const STARTUP_RESPONSE_TIMEOUT: Duration = Duration::from_secs(15);
const RECENT_STDERR_LINES: usize = 20;

#[derive(Debug)]
pub struct CodexTurnRequest {
    pub session_id: SessionId,
    pub workspace_path: PathBuf,
    pub external_session_id: Option<String>,
    pub prompt: String,
    pub settings: Option<AgentTurnSettings>,
}

#[derive(Debug)]
pub struct PendingCodexApproval {
    pub response: oneshot::Sender<ApprovalDecision>,
}

#[derive(Debug)]
pub struct PendingCodexQuestion {
    pub response: oneshot::Sender<AgentQuestionAnswer>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodexDiscoveredThread {
    pub external_session_id: String,
    pub title: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodexApprovalKind {
    Command,
    FileChange,
    Permissions,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodexQuestionKind {
    ToolUserInput,
    McpElicitation,
}

#[derive(Debug)]
pub struct CodexAppServer {
    _child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    thread_id: Option<String>,
    turn_id: Option<String>,
    initialized: bool,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
}

impl CodexAppServer {
    pub fn spawn(workspace_path: PathBuf) -> anyhow::Result<Self> {
        tracing::info!(workspace = %workspace_path.display(), "spawning codex app-server");
        let mut child = Command::new("codex")
            .args(["app-server", "--listen", "stdio://"])
            .current_dir(&workspace_path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("failed to start `codex app-server --listen stdio://`")?;

        let stdin = child.stdin.take().context("codex stdin was not piped")?;
        let stdout = child.stdout.take().context("codex stdout was not piped")?;
        let stderr_tail = Arc::new(Mutex::new(VecDeque::new()));
        if let Some(stderr) = child.stderr.take() {
            let stderr_tail = stderr_tail.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Ok(mut tail) = stderr_tail.lock() {
                        if tail.len() == RECENT_STDERR_LINES {
                            tail.pop_front();
                        }
                        tail.push_back(line.clone());
                    }
                    tracing::warn!(target: "codex-stderr", "{line}");
                }
            });
        }

        Ok(Self {
            _child: child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
            thread_id: None,
            turn_id: None,
            initialized: false,
            stderr_tail,
        })
    }

    pub async fn initialize(&mut self) -> anyhow::Result<()> {
        if self.initialized {
            return Ok(());
        }
        let initialize_id = self
            .send_request(
                "initialize",
                json!({
                    "clientInfo": {
                        "name": "agenter-runner",
                        "title": "Agenter Runner",
                        "version": env!("CARGO_PKG_VERSION")
                    },
                    "capabilities": {"experimentalApi": true}
                }),
            )
            .await?;
        self.read_response(initialize_id, "initialize").await?;
        self.initialized = true;
        Ok(())
    }

    pub async fn start_thread(&mut self, workspace_path: &PathBuf) -> anyhow::Result<String> {
        self.initialize().await?;
        tracing::info!("starting codex thread");
        let start_id = self
            .send_request("thread/start", codex_thread_start_params(workspace_path))
            .await?;
        let response = self.read_response(start_id, "thread/start").await?;
        if let Some(thread_id) = codex_thread_id(&response) {
            self.thread_id = Some(thread_id.to_owned());
        }
        let Some(thread_id) = self.thread_id.clone() else {
            return Err(anyhow!(missing_thread_id_error(
                "thread/start",
                &response,
                &self.recent_stderr()
            )));
        };
        Ok(thread_id)
    }

    pub async fn resume_thread(
        &mut self,
        thread_id: &str,
        workspace_path: &PathBuf,
    ) -> anyhow::Result<()> {
        self.initialize().await?;
        self.thread_id = Some(thread_id.to_owned());
        tracing::info!(provider_thread_id = %thread_id, "resuming codex thread");
        let resume_id = self
            .send_request(
                "thread/resume",
                json!({
                    "threadId": thread_id,
                    "cwd": workspace_path,
                    "approvalPolicy": "on-request",
                    "approvalsReviewer": "user",
                    "excludeTurns": false
                }),
            )
            .await?;
        self.read_response(resume_id, "thread/resume").await?;
        Ok(())
    }

    pub async fn list_threads(
        &mut self,
        workspace_path: &PathBuf,
    ) -> anyhow::Result<Vec<CodexDiscoveredThread>> {
        self.initialize().await?;
        let list_id = self
            .send_request(
                "thread/list",
                json!({
                    "cwd": workspace_path,
                    "includeTurns": false,
                    "useStateDbOnly": false
                }),
            )
            .await?;
        let response = self.read_response(list_id, "thread/list").await?;
        Ok(codex_threads_from_list_response(&response))
    }

    pub async fn read_thread_history(
        &mut self,
        thread_id: &str,
    ) -> anyhow::Result<Vec<DiscoveredSessionHistoryItem>> {
        self.initialize().await?;
        let read_id = self
            .send_request(
                "thread/read",
                json!({
                    "threadId": thread_id,
                    "includeTurns": true
                }),
            )
            .await?;
        let response = self.read_response(read_id, "thread/read").await?;
        Ok(codex_history_from_thread_read_response(&response))
    }

    pub async fn agent_options(&mut self) -> anyhow::Result<AgentOptions> {
        self.initialize().await?;
        let models_id = self
            .send_request("model/list", json!({"includeHidden": false}))
            .await?;
        let models = self.read_response(models_id, "model/list").await?;
        let modes_id = self
            .send_request("collaborationMode/list", json!({}))
            .await?;
        let modes = self
            .read_response(modes_id, "collaborationMode/list")
            .await?;
        Ok(codex_agent_options_from_responses(&models, &modes))
    }

    pub fn set_active_thread(&mut self, thread_id: impl Into<String>) {
        self.thread_id = Some(thread_id.into());
        self.turn_id = None;
    }

    pub async fn send_turn(&mut self, request: &CodexTurnRequest) -> anyhow::Result<()> {
        let Some(thread_id) = &self.thread_id else {
            return Err(anyhow!(
                "codex thread id was not observed before turn start"
            ));
        };
        tracing::info!(
            session_id = %request.session_id,
            provider_thread_id = %thread_id,
            prompt_len = request.prompt.len(),
            payload_preview = agenter_core::logging::payload_preview(
                &json!({"input": &request.prompt}),
                agenter_core::logging::payload_logging_enabled()
            ).as_deref(),
            "starting codex turn"
        );
        let turn_start_id = self
            .send_request("turn/start", codex_turn_start_params(thread_id, request))
            .await?;
        let response = self.read_response(turn_start_id, "turn/start").await?;
        if let Some(turn_id) = codex_turn_id(&response) {
            self.turn_id = Some(turn_id.to_owned());
        }
        Ok(())
    }

    pub async fn next_message(&mut self) -> anyhow::Result<Option<Value>> {
        let mut line = String::new();
        if self.stdout.read_line(&mut line).await? == 0 {
            return Ok(None);
        }
        let message = serde_json::from_str::<Value>(line.trim())
            .with_context(|| format!("codex emitted invalid JSON-RPC line: {line}"))?;
        tracing::debug!(
            method = message.get("method").and_then(serde_json::Value::as_str),
            id = ?message.get("id"),
            payload_preview = agenter_core::logging::payload_preview(
                &message,
                agenter_core::logging::payload_logging_enabled()
            ).as_deref(),
            "received codex json-rpc message"
        );
        if let Some(thread_id) = codex_thread_id(&message) {
            self.thread_id = Some(thread_id.to_owned());
            tracing::info!(provider_thread_id = %thread_id, "observed codex thread id");
        }
        if let Some(turn_id) = codex_turn_id(&message) {
            self.turn_id = Some(turn_id.to_owned());
            tracing::debug!(provider_turn_id = %turn_id, "observed codex turn id");
        }
        Ok(Some(message))
    }

    pub async fn send_approval_response(
        &mut self,
        native_request_id: Value,
        approval_kind: CodexApprovalKind,
        decision: ApprovalDecision,
    ) -> anyhow::Result<()> {
        tracing::info!(
            native_request_id = ?native_request_id,
            ?approval_kind,
            ?decision,
            "sending codex approval response"
        );
        write_json(
            &mut self.stdin,
            &json!({
                "id": native_request_id,
                "result": approval_response_for_decision(approval_kind, decision)
            }),
        )
        .await
    }

    pub async fn send_question_response(
        &mut self,
        native_request_id: Value,
        kind: CodexQuestionKind,
        answer: AgentQuestionAnswer,
    ) -> anyhow::Result<()> {
        tracing::info!(
            native_request_id = ?native_request_id,
            ?kind,
            question_id = %answer.question_id,
            "sending codex question response"
        );
        write_json(
            &mut self.stdin,
            &json!({
                "id": native_request_id,
                "result": question_response_for_answer(kind, answer)
            }),
        )
        .await
    }

    async fn send_request(&mut self, method: &str, params: Value) -> anyhow::Result<u64> {
        let id = self.next_id;
        self.next_id += 1;
        tracing::debug!(
            id,
            method,
            payload_preview = agenter_core::logging::payload_preview(
                &params,
                agenter_core::logging::payload_logging_enabled()
            )
            .as_deref(),
            "sending codex json-rpc request"
        );
        write_json(
            &mut self.stdin,
            &json!({
                "id": id,
                "method": method,
                "params": params
            }),
        )
        .await?;
        Ok(id)
    }

    async fn read_response(&mut self, request_id: u64, method: &str) -> anyhow::Result<Value> {
        loop {
            let message = timeout(STARTUP_RESPONSE_TIMEOUT, self.next_message())
                .await
                .with_context(|| {
                    startup_error_with_stderr(
                        format!("timed out waiting for codex {method} response"),
                        &self.recent_stderr(),
                    )
                })??;
            let Some(message) = message else {
                return Err(anyhow!(startup_error_with_stderr(
                    format!("codex exited before {method} response"),
                    &self.recent_stderr()
                )));
            };
            if message.get("id").and_then(Value::as_u64) != Some(request_id) {
                continue;
            }
            if let Some(summary) = codex_jsonrpc_error_summary(method, &message) {
                return Err(anyhow!(startup_error_with_stderr(
                    summary,
                    &self.recent_stderr()
                )));
            }
            return Ok(message);
        }
    }

    fn recent_stderr(&self) -> Vec<String> {
        self.stderr_tail
            .lock()
            .map(|tail| tail.iter().cloned().collect())
            .unwrap_or_default()
    }
}

pub fn codex_thread_start_params(workspace_path: &PathBuf) -> Value {
    json!({
        "cwd": workspace_path,
        "approvalPolicy": "on-request",
        "approvalsReviewer": "user",
        "sandbox": "read-only",
        "sessionStartSource": "startup"
    })
}

pub fn codex_turn_start_params(thread_id: &str, request: &CodexTurnRequest) -> Value {
    let mut params = json!({
        "threadId": thread_id,
        "cwd": request.workspace_path,
        "approvalPolicy": "on-request",
        "approvalsReviewer": "user",
        "sandboxPolicy": {"type": "readOnly", "networkAccess": false},
        "input": [{"type": "text", "text": request.prompt}]
    });
    let Some(settings) = &request.settings else {
        return params;
    };
    if let Some(model) = &settings.model {
        params["model"] = json!(model);
    }
    if let Some(effort) = &settings.reasoning_effort {
        params["effort"] = json!(codex_reasoning_effort(effort));
    }
    if let Some(mode) = &settings.collaboration_mode {
        let mut mode_payload = json!({
            "mode": mode,
            "settings": {
                "model": settings.model.as_deref().unwrap_or(""),
                "reasoning_effort": settings
                    .reasoning_effort
                    .as_ref()
                    .map(codex_reasoning_effort)
            }
        });
        if settings.model.is_none() {
            mode_payload["settings"]["model"] = Value::Null;
        }
        params["collaborationMode"] = mode_payload;
    }
    params
}

pub async fn run_codex_turn_on_server(
    server: &mut CodexAppServer,
    request: CodexTurnRequest,
    event_sender: mpsc::UnboundedSender<AppEvent>,
    pending_approvals: std::sync::Arc<
        tokio::sync::Mutex<HashMap<ApprovalId, PendingCodexApproval>>,
    >,
    pending_questions: std::sync::Arc<
        tokio::sync::Mutex<HashMap<QuestionId, PendingCodexQuestion>>,
    >,
) -> anyhow::Result<()> {
    while server.thread_id.is_none() {
        let Some(message) = server.next_message().await? else {
            return Err(anyhow!("codex exited before returning a thread id"));
        };
        for event in normalize_codex_message(request.session_id, &message) {
            event_sender.send(event).ok();
        }
    }

    server.send_turn(&request).await?;
    let mut scope = CodexTurnScope {
        thread_id: server.thread_id.clone(),
        turn_id: server.turn_id.clone(),
    };
    while let Some(message) = server.next_message().await? {
        scope.observe(&message);
        if !codex_message_belongs_to_scope(&message, &scope) {
            tracing::debug!(
                provider_thread_id = message_thread_id(&message),
                provider_turn_id = message_turn_id(&message),
                "ignored codex message outside active turn scope"
            );
            continue;
        }
        if let Some((approval_id, native_request_id, approval_kind, event)) =
            normalize_codex_approval_request(request.session_id, &message)
        {
            let (sender, receiver) = oneshot::channel();
            pending_approvals
                .lock()
                .await
                .insert(approval_id, PendingCodexApproval { response: sender });
            tracing::info!(
                session_id = %request.session_id,
                %approval_id,
                native_request_id = ?native_request_id,
                ?approval_kind,
                "codex approval request pending"
            );
            event_sender.send(event).ok();
            if let Ok(decision) = receiver.await {
                server
                    .send_approval_response(native_request_id, approval_kind, decision)
                    .await?;
            }
            continue;
        }
        if let Some((question_id, native_request_id, event)) =
            normalize_codex_question_request(request.session_id, &message)
        {
            let Some(question_kind) = codex_question_kind(&message) else {
                continue;
            };
            let (sender, receiver) = oneshot::channel();
            pending_questions
                .lock()
                .await
                .insert(question_id, PendingCodexQuestion { response: sender });
            tracing::info!(
                session_id = %request.session_id,
                %question_id,
                ?question_kind,
                "codex question request pending"
            );
            event_sender.send(event).ok();
            if let Ok(answer) = receiver.await {
                server
                    .send_question_response(native_request_id, question_kind, answer)
                    .await?;
            }
            continue;
        }

        for event in normalize_codex_message_for_scope(request.session_id, &message, &scope) {
            let completed = jsonrpc_method(&message) == Some("turn/completed");
            event_sender.send(event).ok();
            if completed {
                tracing::info!(session_id = %request.session_id, "codex turn completed");
                return Ok(());
            }
        }
    }

    Ok(())
}

pub fn codex_agent_options_from_responses(models: &Value, modes: &Value) -> AgentOptions {
    AgentOptions {
        models: codex_model_options(models),
        collaboration_modes: codex_collaboration_modes(modes),
    }
}

fn codex_model_options(message: &Value) -> Vec<AgentModelOption> {
    message
        .pointer("/result/data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|model| !bool_at(model, &["/hidden"]).unwrap_or(false))
        .filter_map(|model| {
            let id = string_at(model, &["/id", "/model"])?;
            Some(AgentModelOption {
                id: id.to_owned(),
                display_name: string_at(model, &["/displayName", "/display_name", "/model"])
                    .unwrap_or(id)
                    .to_owned(),
                description: string_at(model, &["/description"]).map(str::to_owned),
                is_default: bool_at(model, &["/isDefault", "/is_default"]).unwrap_or(false),
                default_reasoning_effort: string_at(
                    model,
                    &["/defaultReasoningEffort", "/default_reasoning_effort"],
                )
                .and_then(agent_reasoning_effort),
                supported_reasoning_efforts: model
                    .get("supportedReasoningEfforts")
                    .or_else(|| model.get("supported_reasoning_efforts"))
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|effort| {
                        effort
                            .as_str()
                            .or_else(|| string_at(effort, &["/effort", "/id", "/value"]))
                            .and_then(agent_reasoning_effort)
                    })
                    .collect(),
                input_modalities: model
                    .get("inputModalities")
                    .or_else(|| model.get("input_modalities"))
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect(),
            })
        })
        .collect()
}

fn codex_collaboration_modes(message: &Value) -> Vec<AgentCollaborationMode> {
    message
        .pointer("/result/data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|mode| {
            let id = string_at(mode, &["/mode", "/id", "/name"])?;
            Some(AgentCollaborationMode {
                id: id.to_owned(),
                label: string_at(mode, &["/name", "/label"])
                    .unwrap_or(id)
                    .to_owned(),
                model: string_at(mode, &["/model"]).map(str::to_owned),
                reasoning_effort: string_at(mode, &["/reasoning_effort", "/reasoningEffort"])
                    .and_then(agent_reasoning_effort),
            })
        })
        .collect()
}

async fn write_json(stdin: &mut ChildStdin, message: &Value) -> anyhow::Result<()> {
    let mut encoded = serde_json::to_vec(message)?;
    encoded.push(b'\n');
    stdin.write_all(&encoded).await?;
    stdin.flush().await?;
    Ok(())
}

pub fn normalize_codex_message(session_id: SessionId, message: &Value) -> Vec<AppEvent> {
    normalize_codex_message_inner(session_id, message)
}

fn normalize_codex_message_for_scope(
    session_id: SessionId,
    message: &Value,
    scope: &CodexTurnScope,
) -> Vec<AppEvent> {
    if !codex_message_belongs_to_scope(message, scope) {
        return Vec::new();
    }
    normalize_codex_message_inner(session_id, message)
}

fn normalize_codex_message_inner(session_id: SessionId, message: &Value) -> Vec<AppEvent> {
    let Some(method) = jsonrpc_method(message) else {
        return Vec::new();
    };
    match method {
        "agentMessage/delta" | "item/agentMessage/delta" => {
            text_delta(session_id, message).into_iter().collect()
        }
        "agentMessage/completed" | "agentMessage/complete" => {
            message_completed(session_id, message).into_iter().collect()
        }
        "item/started" => item_started(session_id, message).into_iter().collect(),
        "item/completed" => item_completed(session_id, message).into_iter().collect(),
        "turn/completed" => vec![AppEvent::AgentMessageCompleted(MessageCompletedEvent {
            session_id,
            message_id: string_at(message, &["/params/turnId", "/params/id"])
                .unwrap_or("codex-turn")
                .to_owned(),
            content: None,
            provider_payload: Some(message.clone()),
        })],
        "error" => vec![AppEvent::Error(AgentErrorEvent {
            session_id: Some(session_id),
            code: string_at(message, &["/params/code"]).map(str::to_owned),
            message: string_at(message, &["/params/message"])
                .unwrap_or("Codex reported an error")
                .to_owned(),
            provider_payload: Some(message.clone()),
        })],
        _ => Vec::new(),
    }
}

pub fn normalize_codex_approval_request(
    session_id: SessionId,
    message: &Value,
) -> Option<(ApprovalId, Value, CodexApprovalKind, AppEvent)> {
    let method = jsonrpc_method(message)?;
    let (kind, approval_kind) = match method {
        "item/commandExecution/requestApproval" => {
            (ApprovalKind::Command, CodexApprovalKind::Command)
        }
        "item/fileChange/requestApproval" => {
            (ApprovalKind::FileChange, CodexApprovalKind::FileChange)
        }
        "item/permissions/requestApproval" => (
            ApprovalKind::ProviderSpecific,
            CodexApprovalKind::Permissions,
        ),
        _ => return None,
    };
    let native_request_id = message.get("id")?.clone();
    let approval_id = ApprovalId::new();
    let title = match kind {
        ApprovalKind::Command => "Approve Codex command",
        ApprovalKind::FileChange => "Approve Codex file change",
        ApprovalKind::ProviderSpecific | ApprovalKind::Tool => "Approve Codex permission",
    };
    let details = string_at(
        message,
        &[
            "/params/command",
            "/params/item/command",
            "/params/path",
            "/params/item/path",
            "/params/description",
        ],
    )
    .map(str::to_owned)
    .or_else(|| serde_json::to_string(message.get("params").unwrap_or(&Value::Null)).ok());
    let event = AppEvent::ApprovalRequested(ApprovalRequestEvent {
        session_id,
        approval_id,
        kind,
        title: title.to_owned(),
        details,
        expires_at: None,
        provider_payload: Some(message.clone()),
    });
    Some((approval_id, native_request_id, approval_kind, event))
}

pub fn normalize_codex_question_request(
    session_id: SessionId,
    message: &Value,
) -> Option<(QuestionId, Value, AppEvent)> {
    let method = jsonrpc_method(message)?;
    let native_request_id = message.get("id")?.clone();
    let question_id = QuestionId::new();
    let (title, description, fields) = match method {
        "item/tool/requestUserInput" => (
            "Codex needs input".to_owned(),
            None,
            tool_user_input_fields(message),
        ),
        "mcpServer/elicitation/request" => {
            let params = message.get("params").unwrap_or(&Value::Null);
            (
                string_at(params, &["/serverName"])
                    .unwrap_or("MCP input")
                    .to_owned(),
                string_at(params, &["/message"]).map(str::to_owned),
                mcp_elicitation_fields(params),
            )
        }
        _ => return None,
    };
    let event = AppEvent::QuestionRequested(QuestionRequestedEvent {
        session_id,
        question_id,
        title,
        description,
        fields,
        provider_payload: Some(message.clone()),
    });
    Some((question_id, native_request_id, event))
}

fn codex_question_kind(message: &Value) -> Option<CodexQuestionKind> {
    match jsonrpc_method(message)? {
        "item/tool/requestUserInput" => Some(CodexQuestionKind::ToolUserInput),
        "mcpServer/elicitation/request" => Some(CodexQuestionKind::McpElicitation),
        _ => None,
    }
}

fn tool_user_input_fields(message: &Value) -> Vec<AgentQuestionField> {
    message
        .pointer("/params/questions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|question| {
            let id = string_at(question, &["/id"])?;
            let choices: Vec<_> = question
                .get("options")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|option| {
                    let label = string_at(option, &["/label"])?;
                    Some(AgentQuestionChoice {
                        value: label.to_owned(),
                        label: label.to_owned(),
                        description: string_at(option, &["/description"]).map(str::to_owned),
                    })
                })
                .collect();
            Some(AgentQuestionField {
                id: id.to_owned(),
                label: string_at(question, &["/header"]).unwrap_or(id).to_owned(),
                prompt: string_at(question, &["/question"]).map(str::to_owned),
                kind: if choices.is_empty() {
                    "text".to_owned()
                } else {
                    "single_select".to_owned()
                },
                required: true,
                secret: bool_at(question, &["/isSecret"]).unwrap_or(false),
                choices,
                default_answers: Vec::new(),
            })
        })
        .collect()
}

fn mcp_elicitation_fields(params: &Value) -> Vec<AgentQuestionField> {
    let required: std::collections::BTreeSet<_> = params
        .pointer("/requestedSchema/required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    let Some(properties) = params
        .pointer("/requestedSchema/properties")
        .and_then(Value::as_object)
    else {
        return Vec::new();
    };
    properties
        .iter()
        .map(|(id, schema)| {
            let (kind, choices) = mcp_field_kind_and_choices(schema);
            AgentQuestionField {
                id: id.clone(),
                label: string_at(schema, &["/title"]).unwrap_or(id).to_owned(),
                prompt: string_at(schema, &["/description"]).map(str::to_owned),
                kind,
                required: required.contains(id),
                secret: false,
                choices,
                default_answers: default_answers(schema),
            }
        })
        .collect()
}

fn mcp_field_kind_and_choices(schema: &Value) -> (String, Vec<AgentQuestionChoice>) {
    if schema.get("type").and_then(Value::as_str) == Some("array") {
        return (
            "multi_select".to_owned(),
            enum_choices(
                schema
                    .pointer("/items/enum")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten(),
            ),
        );
    }
    if let Some(one_of) = schema.get("oneOf").and_then(Value::as_array) {
        return (
            "single_select".to_owned(),
            one_of
                .iter()
                .filter_map(|option| {
                    let value = string_at(option, &["/const"])?;
                    Some(AgentQuestionChoice {
                        value: value.to_owned(),
                        label: string_at(option, &["/title"]).unwrap_or(value).to_owned(),
                        description: None,
                    })
                })
                .collect(),
        );
    }
    if let Some(enum_values) = schema.get("enum").and_then(Value::as_array) {
        return ("single_select".to_owned(), enum_choices(enum_values.iter()));
    }
    match schema.get("type").and_then(Value::as_str) {
        Some("boolean") => ("boolean".to_owned(), Vec::new()),
        Some("number" | "integer") => ("number".to_owned(), Vec::new()),
        _ => ("text".to_owned(), Vec::new()),
    }
}

fn enum_choices<'a>(values: impl Iterator<Item = &'a Value>) -> Vec<AgentQuestionChoice> {
    values
        .filter_map(Value::as_str)
        .map(|value| AgentQuestionChoice {
            value: value.to_owned(),
            label: value.to_owned(),
            description: None,
        })
        .collect()
}

fn default_answers(schema: &Value) -> Vec<String> {
    match schema.get("default") {
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        Some(Value::String(value)) => vec![value.clone()],
        Some(Value::Bool(value)) => vec![value.to_string()],
        Some(Value::Number(value)) => vec![value.to_string()],
        _ => Vec::new(),
    }
}

pub fn question_response_for_answer(kind: CodexQuestionKind, answer: AgentQuestionAnswer) -> Value {
    match kind {
        CodexQuestionKind::ToolUserInput => {
            let answers = answer
                .answers
                .into_iter()
                .map(|(id, answers)| (id, json!({ "answers": answers })))
                .collect::<serde_json::Map<_, _>>();
            json!({ "answers": answers })
        }
        CodexQuestionKind::McpElicitation => {
            let content = answer
                .answers
                .into_iter()
                .map(|(id, answers)| {
                    let value = if answers.len() == 1 {
                        json!(answers[0])
                    } else {
                        json!(answers)
                    };
                    (id, value)
                })
                .collect::<serde_json::Map<_, _>>();
            json!({ "action": "accept", "content": content, "_meta": null })
        }
    }
}

pub fn approval_response_for_decision(
    approval_kind: CodexApprovalKind,
    decision: ApprovalDecision,
) -> Value {
    if approval_kind == CodexApprovalKind::Permissions {
        return match decision {
            ApprovalDecision::ProviderSpecific { payload } => payload,
            ApprovalDecision::Accept
            | ApprovalDecision::AcceptForSession
            | ApprovalDecision::Decline
            | ApprovalDecision::Cancel => {
                json!({"permissions": {"fileSystem": null, "network": null}, "scope": "turn"})
            }
        };
    }

    match decision {
        ApprovalDecision::Accept => json!({"decision": "accept"}),
        ApprovalDecision::AcceptForSession => json!({"decision": "acceptForSession"}),
        ApprovalDecision::Decline => json!({"decision": "decline"}),
        ApprovalDecision::Cancel => json!({"decision": "cancel"}),
        ApprovalDecision::ProviderSpecific { payload } => payload,
    }
}

fn text_delta(session_id: SessionId, message: &Value) -> Option<AppEvent> {
    let delta = string_at(
        message,
        &[
            "/params/delta",
            "/params/text",
            "/params/content",
            "/params/item/delta",
            "/params/item/text",
        ],
    )?;
    Some(AppEvent::AgentMessageDelta(AgentMessageDeltaEvent {
        session_id,
        message_id: message_id(message),
        delta: delta.to_owned(),
        provider_payload: Some(message.clone()),
    }))
}

fn message_completed(session_id: SessionId, message: &Value) -> Option<AppEvent> {
    Some(AppEvent::AgentMessageCompleted(MessageCompletedEvent {
        session_id,
        message_id: message_id(message),
        content: string_at(
            message,
            &[
                "/params/content",
                "/params/text",
                "/params/message",
                "/params/item/text",
                "/params/item/content",
            ],
        )
        .map(str::to_owned),
        provider_payload: Some(message.clone()),
    }))
}

fn item_started(session_id: SessionId, message: &Value) -> Option<AppEvent> {
    if should_ignore_item_event(message) {
        return None;
    }

    if let Some(command) = string_at(message, &["/params/command", "/params/item/command"]) {
        return Some(AppEvent::CommandStarted(CommandEvent {
            session_id,
            command_id: item_id(message),
            command: command.to_owned(),
            cwd: string_at(message, &["/params/cwd", "/params/item/cwd"]).map(str::to_owned),
            source: string_at(message, &["/params/source", "/params/item/source"])
                .map(str::to_owned),
            process_id: string_at(message, &["/params/processId", "/params/item/processId"])
                .map(str::to_owned),
            actions: Vec::new(),
            provider_payload: Some(message.clone()),
        }));
    }

    Some(AppEvent::ToolStarted(agenter_core::ToolEvent {
        session_id,
        tool_call_id: item_id(message),
        name: string_at(message, &["/params/name", "/params/item/name"])
            .unwrap_or("codex_item")
            .to_owned(),
        title: string_at(message, &["/params/title", "/params/item/title"]).map(str::to_owned),
        input: message.pointer("/params/input").cloned(),
        output: None,
        provider_payload: Some(message.clone()),
    }))
}

fn item_completed(session_id: SessionId, message: &Value) -> Option<AppEvent> {
    if item_type(message) == Some("agentMessage") {
        return message_completed(session_id, message);
    }
    if should_ignore_item_event(message) {
        return None;
    }

    if string_at(message, &["/params/command", "/params/item/command"]).is_some() {
        return Some(AppEvent::CommandCompleted(CommandCompletedEvent {
            session_id,
            command_id: item_id(message),
            exit_code: integer_at(message, &["/params/exitCode", "/params/item/exitCode"])
                .map(|value| value as i32),
            duration_ms: integer_at(message, &["/params/durationMs", "/params/item/durationMs"])
                .and_then(|value| value.try_into().ok()),
            success: bool_at(message, &["/params/success", "/params/item/success"]).unwrap_or(true),
            provider_payload: Some(message.clone()),
        }));
    }

    if let Some(path) = string_at(message, &["/params/path", "/params/item/path"]) {
        return Some(AppEvent::FileChangeProposed(FileChangeEvent {
            session_id,
            path: path.to_owned(),
            change_kind: file_change_kind(message),
            diff: string_at(message, &["/params/diff", "/params/item/diff"]).map(str::to_owned),
            provider_payload: Some(message.clone()),
        }));
    }

    Some(AppEvent::ToolCompleted(agenter_core::ToolEvent {
        session_id,
        tool_call_id: item_id(message),
        name: string_at(message, &["/params/name", "/params/item/name"])
            .unwrap_or("codex_item")
            .to_owned(),
        title: string_at(message, &["/params/title", "/params/item/title"]).map(str::to_owned),
        input: None,
        output: message.pointer("/params/output").cloned(),
        provider_payload: Some(message.clone()),
    }))
}

fn file_change_kind(message: &Value) -> FileChangeKind {
    match string_at(message, &["/params/changeKind", "/params/item/changeKind"]) {
        Some("create" | "add") => FileChangeKind::Create,
        Some("delete" | "remove") => FileChangeKind::Delete,
        Some("rename" | "move") => FileChangeKind::Rename,
        _ => FileChangeKind::Modify,
    }
}

fn jsonrpc_method(message: &Value) -> Option<&str> {
    message.get("method")?.as_str()
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct CodexTurnScope {
    thread_id: Option<String>,
    turn_id: Option<String>,
}

impl CodexTurnScope {
    fn observe(&mut self, message: &Value) {
        if self.thread_id.is_none() {
            self.thread_id = codex_thread_id(message).map(str::to_owned);
        }
        if self.turn_id.is_none() {
            self.turn_id = codex_turn_id(message).map(str::to_owned);
        }
    }
}

fn codex_message_belongs_to_scope(message: &Value, scope: &CodexTurnScope) -> bool {
    if let (Some(expected), Some(actual)) = (scope.thread_id.as_deref(), message_thread_id(message))
    {
        if actual != expected {
            return false;
        }
    }
    if let (Some(expected), Some(actual)) = (scope.turn_id.as_deref(), message_turn_id(message)) {
        if actual != expected {
            return false;
        }
    }
    true
}

fn message_thread_id(message: &Value) -> Option<&str> {
    string_at(
        message,
        &[
            "/params/threadId",
            "/params/thread/id",
            "/result/threadId",
            "/result/thread/id",
        ],
    )
}

fn message_turn_id(message: &Value) -> Option<&str> {
    string_at(
        message,
        &[
            "/params/turnId",
            "/params/turn/id",
            "/result/turnId",
            "/result/turn/id",
        ],
    )
}

fn codex_thread_id(message: &Value) -> Option<&str> {
    message
        .pointer("/result/thread/id")
        .and_then(Value::as_str)
        .or_else(|| message.pointer("/result/id").and_then(Value::as_str))
        .or_else(|| message.pointer("/result/threadId").and_then(Value::as_str))
        .or_else(|| message.pointer("/params/thread/id").and_then(Value::as_str))
        .or_else(|| message.pointer("/params/threadId").and_then(Value::as_str))
}

fn codex_threads_from_list_response(message: &Value) -> Vec<CodexDiscoveredThread> {
    let Some(array) = message
        .pointer("/result/data")
        .or_else(|| message.pointer("/result/threads"))
        .or_else(|| message.pointer("/result/items"))
        .or_else(|| message.pointer("/threads"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };

    array
        .iter()
        .filter_map(|thread| {
            let external_session_id = string_at(thread, &["/id", "/threadId", "/thread/id"])?;
            Some(CodexDiscoveredThread {
                external_session_id: external_session_id.to_owned(),
                title: string_at(thread, &["/title", "/name", "/summary", "/preview"])
                    .map(str::to_owned),
            })
        })
        .collect()
}

fn codex_history_from_thread_read_response(message: &Value) -> Vec<DiscoveredSessionHistoryItem> {
    let mut items = Vec::new();
    if let Some(turns) = message
        .pointer("/result/thread/turns")
        .and_then(Value::as_array)
    {
        for turn in turns {
            if let Some(turn_items) = turn.get("items").and_then(Value::as_array) {
                for item in turn_items {
                    collect_codex_history_item(item, &mut items);
                }
            }
        }
    } else {
        collect_codex_history_items(message, &mut items);
    }
    items
}

fn collect_codex_history_items(value: &Value, items: &mut Vec<DiscoveredSessionHistoryItem>) {
    match value {
        Value::Object(object) => {
            if object.contains_key("type") {
                let before = items.len();
                collect_codex_history_item(value, items);
                if items.len() != before {
                    return;
                }
            }

            for child in object.values() {
                collect_codex_history_items(child, items);
            }
        }
        Value::Array(array) => {
            for child in array {
                collect_codex_history_items(child, items);
            }
        }
        _ => {}
    }
}

fn collect_codex_history_item(value: &Value, items: &mut Vec<DiscoveredSessionHistoryItem>) {
    match value.get("type").and_then(Value::as_str) {
        Some("userMessage") => {
            if let Some(content) = codex_text_content(value) {
                items.push(DiscoveredSessionHistoryItem::UserMessage {
                    message_id: string_at(value, &["/id", "/messageId"]).map(str::to_owned),
                    content,
                });
            }
        }
        Some("agentMessage") => {
            if let Some(content) = codex_text_content(value) {
                items.push(DiscoveredSessionHistoryItem::AgentMessage {
                    message_id: string_at(value, &["/id", "/messageId"])
                        .unwrap_or("codex-agent-message")
                        .to_owned(),
                    content,
                });
            }
        }
        Some("commandExecution") => {
            if let Some(command) = string_at(value, &["/command"]) {
                let status = string_at(value, &["/status"]);
                let exit_code = integer_at(value, &["/exitCode"]).map(|value| value as i32);
                let success = exit_code
                    .map(|code| code == 0)
                    .unwrap_or(status != Some("failed"));
                items.push(DiscoveredSessionHistoryItem::Command {
                    command_id: string_at(value, &["/id"])
                        .unwrap_or("codex-command")
                        .to_owned(),
                    command: command.to_owned(),
                    cwd: string_at(value, &["/cwd"]).map(str::to_owned),
                    source: string_at(value, &["/source"]).map(str::to_owned),
                    process_id: string_at(value, &["/processId"]).map(str::to_owned),
                    duration_ms: integer_at(value, &["/durationMs"])
                        .and_then(|value| value.try_into().ok()),
                    actions: codex_history_command_actions(value),
                    output: string_at(value, &["/aggregatedOutput"]).map(str::to_owned),
                    exit_code,
                    success,
                    provider_payload: Some(value.clone()),
                });
            }
        }
        Some("fileChange") => {
            if let Some(changes) = value.get("changes").and_then(Value::as_array) {
                for (index, change) in changes.iter().enumerate() {
                    let Some(path) = string_at(change, &["/path"]) else {
                        continue;
                    };
                    let change_id = format!(
                        "{}:{index}",
                        string_at(value, &["/id"]).unwrap_or("codex-file-change")
                    );
                    items.push(DiscoveredSessionHistoryItem::FileChange {
                        change_id,
                        path: path.to_owned(),
                        change_kind: codex_history_file_change_kind(change),
                        status: codex_history_file_change_status(value),
                        diff: string_at(change, &["/diff"]).map(str::to_owned),
                        provider_payload: Some(value.clone()),
                    });
                }
            }
        }
        Some("collabAgentToolCall") => {
            let status = codex_history_tool_status(value);
            items.push(DiscoveredSessionHistoryItem::Tool {
                tool_call_id: string_at(value, &["/id"])
                    .unwrap_or("codex-tool")
                    .to_owned(),
                name: string_at(value, &["/tool"])
                    .unwrap_or("codex_tool")
                    .to_owned(),
                title: string_at(value, &["/tool"]).map(str::to_owned),
                status,
                input: Some(value.clone()),
                output: value.get("agentsStates").cloned(),
                provider_payload: Some(value.clone()),
            });
        }
        Some("plan") => {
            if let Some(content) = string_at(value, &["/text", "/content"]) {
                items.push(DiscoveredSessionHistoryItem::Plan {
                    plan_id: string_at(value, &["/id"])
                        .unwrap_or("codex-plan")
                        .to_owned(),
                    title: Some("Implementation plan".to_owned()),
                    content: content.to_owned(),
                    provider_payload: Some(value.clone()),
                });
            }
        }
        _ => {}
    }
}

fn codex_history_command_actions(value: &Value) -> Vec<DiscoveredCommandAction> {
    value
        .get("commandActions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|action| DiscoveredCommandAction {
            kind: string_at(action, &["/type"])
                .unwrap_or("unknown")
                .to_owned(),
            command: string_at(action, &["/command"]).map(str::to_owned),
            path: string_at(action, &["/path"]).map(str::to_owned),
            name: string_at(action, &["/name"]).map(str::to_owned),
            query: string_at(action, &["/query"]).map(str::to_owned),
            provider_payload: Some(action.clone()),
        })
        .collect()
}

fn codex_history_tool_status(value: &Value) -> DiscoveredToolStatus {
    match string_at(value, &["/status"]) {
        Some("completed") => DiscoveredToolStatus::Completed,
        Some("failed") => DiscoveredToolStatus::Failed,
        _ => DiscoveredToolStatus::Running,
    }
}

fn codex_history_file_change_status(value: &Value) -> DiscoveredFileChangeStatus {
    match string_at(value, &["/status"]) {
        Some("applied" | "completed") => DiscoveredFileChangeStatus::Applied,
        Some("rejected" | "failed") => DiscoveredFileChangeStatus::Rejected,
        _ => DiscoveredFileChangeStatus::Proposed,
    }
}

fn codex_history_file_change_kind(value: &Value) -> FileChangeKind {
    match string_at(value, &["/kind/type", "/changeKind"]) {
        Some("add" | "create") => FileChangeKind::Create,
        Some("delete" | "remove") => FileChangeKind::Delete,
        Some("rename" | "move") => FileChangeKind::Rename,
        _ => FileChangeKind::Modify,
    }
}

fn codex_text_content(value: &Value) -> Option<String> {
    string_at(
        value,
        &[
            "/text",
            "/content",
            "/message",
            "/item/text",
            "/item/content",
        ],
    )
    .map(str::to_owned)
    .or_else(|| {
        value
            .pointer("/content")
            .and_then(Value::as_array)
            .map(|parts| {
                parts
                    .iter()
                    .filter_map(|part| string_at(part, &["/text", "/content"]).map(str::to_owned))
                    .collect::<Vec<_>>()
                    .join("")
            })
            .filter(|content| !content.is_empty())
    })
}

fn codex_turn_id(message: &Value) -> Option<&str> {
    message_turn_id(message)
}

fn codex_jsonrpc_error_summary(method: &str, message: &Value) -> Option<String> {
    let error = message.get("error")?;
    let code = error
        .get("code")
        .map(Value::to_string)
        .unwrap_or_else(|| "unknown".to_owned());
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("unknown provider error");
    Some(format!("codex {method} failed: {code} {message}"))
}

fn missing_thread_id_error(method: &str, message: &Value, stderr: &[String]) -> String {
    let mut error = format!("codex {method} response did not include a thread id");
    let stderr_label = recent_stderr_label(stderr);
    if !stderr_label.is_empty() {
        error.push_str("; ");
        error.push_str(&stderr_label);
    } else if let Some(payload) = agenter_core::logging::payload_preview(
        message,
        agenter_core::logging::payload_logging_enabled(),
    ) {
        error.push_str("; response preview: ");
        error.push_str(&payload);
    }
    error
}

fn recent_stderr_label(stderr: &[String]) -> String {
    stderr
        .last()
        .map(|line| format!("recent stderr: {line}"))
        .unwrap_or_default()
}

fn startup_error_with_stderr(message: String, stderr: &[String]) -> String {
    let stderr_label = recent_stderr_label(stderr);
    if stderr_label.is_empty() {
        message
    } else {
        format!("{message}; {stderr_label}")
    }
}

fn message_id(message: &Value) -> String {
    string_at(
        message,
        &[
            "/params/messageId",
            "/params/message_id",
            "/params/itemId",
            "/params/item/id",
            "/params/id",
            "/params/turnId",
        ],
    )
    .unwrap_or("codex-message")
    .to_owned()
}

fn item_id(message: &Value) -> String {
    string_at(
        message,
        &[
            "/params/item/id",
            "/params/id",
            "/params/itemId",
            "/params/item_id",
        ],
    )
    .unwrap_or("codex-item")
    .to_owned()
}

fn item_type(message: &Value) -> Option<&str> {
    string_at(message, &["/params/item/type", "/params/type"])
}

fn should_ignore_item_event(message: &Value) -> bool {
    matches!(item_type(message), Some("userMessage" | "reasoning"))
}

fn string_at<'a>(message: &'a Value, pointers: &[&str]) -> Option<&'a str> {
    pointers
        .iter()
        .find_map(|pointer| message.pointer(pointer).and_then(Value::as_str))
}

fn integer_at(message: &Value, pointers: &[&str]) -> Option<i64> {
    pointers
        .iter()
        .find_map(|pointer| message.pointer(pointer).and_then(Value::as_i64))
}

fn bool_at(message: &Value, pointers: &[&str]) -> Option<bool> {
    pointers
        .iter()
        .find_map(|pointer| message.pointer(pointer).and_then(Value::as_bool))
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

fn codex_reasoning_effort(effort: &AgentReasoningEffort) -> &'static str {
    match effort {
        AgentReasoningEffort::None => "none",
        AgentReasoningEffort::Minimal => "minimal",
        AgentReasoningEffort::Low => "low",
        AgentReasoningEffort::Medium => "medium",
        AgentReasoningEffort::High => "high",
        AgentReasoningEffort::Xhigh => "xhigh",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_codex_message_delta_fixture() {
        let message = json!({
            "method": "agentMessage/delta",
            "params": {
                "messageId": "msg-1",
                "delta": "hello"
            }
        });

        let events = normalize_codex_message(SessionId::nil(), &message);

        assert_eq!(events.len(), 1);
        let AppEvent::AgentMessageDelta(delta) = &events[0] else {
            panic!("expected message delta");
        };
        assert_eq!(delta.message_id, "msg-1");
        assert_eq!(delta.delta, "hello");
    }

    #[test]
    fn normalizes_live_codex_item_agent_message_delta() {
        let message = json!({
            "method": "item/agentMessage/delta",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "itemId": "msg-live-1",
                "delta": "Under"
            }
        });

        let events = normalize_codex_message(SessionId::nil(), &message);

        assert_eq!(events.len(), 1);
        let AppEvent::AgentMessageDelta(delta) = &events[0] else {
            panic!("expected message delta");
        };
        assert_eq!(delta.message_id, "msg-live-1");
        assert_eq!(delta.delta, "Under");
    }

    #[test]
    fn normalizes_live_codex_completed_agent_message_item() {
        let message = json!({
            "method": "item/completed",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "item": {
                    "id": "msg-live-1",
                    "type": "agentMessage",
                    "text": "Done."
                }
            }
        });

        let events = normalize_codex_message(SessionId::nil(), &message);

        assert_eq!(events.len(), 1);
        let AppEvent::AgentMessageCompleted(completed) = &events[0] else {
            panic!("expected message completed");
        };
        assert_eq!(completed.message_id, "msg-live-1");
        assert_eq!(completed.content.as_deref(), Some("Done."));
    }

    #[test]
    fn ignores_live_codex_user_and_reasoning_items() {
        let user_message = json!({
            "method": "item/started",
            "params": {
                "item": {
                    "id": "user-1",
                    "type": "userMessage",
                    "content": [{"type": "text", "text": "hello"}]
                }
            }
        });
        let reasoning = json!({
            "method": "item/completed",
            "params": {
                "item": {
                    "id": "reasoning-1",
                    "type": "reasoning",
                    "content": []
                }
            }
        });

        assert!(normalize_codex_message(SessionId::nil(), &user_message).is_empty());
        assert!(normalize_codex_message(SessionId::nil(), &reasoning).is_empty());
    }

    #[test]
    fn filters_live_codex_messages_to_target_thread_and_turn() {
        let target = CodexTurnScope {
            thread_id: Some("thread-1".to_owned()),
            turn_id: Some("turn-1".to_owned()),
        };
        let matching = json!({
            "method": "item/agentMessage/delta",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "itemId": "msg-live-1",
                "delta": "ok"
            }
        });
        let other_thread = json!({
            "method": "item/agentMessage/delta",
            "params": {
                "threadId": "thread-2",
                "turnId": "turn-1",
                "itemId": "msg-live-2",
                "delta": "wrong"
            }
        });
        let other_turn = json!({
            "method": "item/agentMessage/delta",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-2",
                "itemId": "msg-live-3",
                "delta": "wrong"
            }
        });

        assert_eq!(
            normalize_codex_message_for_scope(SessionId::nil(), &matching, &target).len(),
            1
        );
        assert!(
            normalize_codex_message_for_scope(SessionId::nil(), &other_thread, &target).is_empty()
        );
        assert!(
            normalize_codex_message_for_scope(SessionId::nil(), &other_turn, &target).is_empty()
        );
    }

    #[test]
    fn normalizes_codex_command_approval_fixture() {
        let message = json!({
            "id": "approval-1",
            "method": "item/commandExecution/requestApproval",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "command": "cargo test"
            }
        });

        let (_approval_id, native_request_id, approval_kind, event) =
            normalize_codex_approval_request(SessionId::nil(), &message)
                .expect("approval should normalize");

        assert_eq!(native_request_id, json!("approval-1"));
        assert_eq!(approval_kind, CodexApprovalKind::Command);
        let AppEvent::ApprovalRequested(request) = event else {
            panic!("expected approval request");
        };
        assert_eq!(request.kind, ApprovalKind::Command);
        assert_eq!(request.details.as_deref(), Some("cargo test"));
    }

    #[test]
    fn extracts_codex_thread_id_from_start_response_and_notification_shapes() {
        let start_response = json!({
            "id": 2,
            "result": {
                "thread": {
                    "id": "thread-from-start-response"
                }
            }
        });
        let started_notification = json!({
            "method": "thread/started",
            "params": {
                "thread": {
                    "id": "thread-from-notification"
                }
            }
        });
        let flat_result = json!({
            "id": 2,
            "result": {
                "id": "thread-from-flat-result"
            }
        });

        assert_eq!(
            codex_thread_id(&start_response),
            Some("thread-from-start-response")
        );
        assert_eq!(
            codex_thread_id(&started_notification),
            Some("thread-from-notification")
        );
        assert_eq!(
            codex_thread_id(&flat_result),
            Some("thread-from-flat-result")
        );
    }

    #[test]
    fn extracts_threads_and_history_from_codex_read_shapes() {
        let list_response = json!({
            "id": 4,
            "result": {
                "threads": [
                    {"id": "thread-1", "title": "Imported Thread"}
                ]
            }
        });
        let read_response = json!({
            "id": 5,
            "result": {
                "thread": {
                    "turns": [
                        {
                            "items": [
                                {"type": "userMessage", "id": "user-1", "text": "hello"},
                                {"type": "agentMessage", "id": "agent-1", "text": "hi"},
                                {
                                    "type": "commandExecution",
                                    "id": "cmd-1",
                                    "command": "cargo test",
                                    "cwd": "/work/agenter",
                                    "status": "completed",
                                    "exitCode": 0,
                                    "durationMs": 17,
                                    "source": "unifiedExecStartup",
                                    "processId": "123",
                                    "aggregatedOutput": "ok",
                                    "commandActions": [
                                        {
                                            "type": "read",
                                            "command": "sed -n '1,20p' /tmp/skills/demo/SKILL.md",
                                            "name": "SKILL.md",
                                            "path": "/tmp/skills/demo/SKILL.md"
                                        }
                                    ]
                                },
                                {
                                    "type": "fileChange",
                                    "id": "file-1",
                                    "status": "completed",
                                    "changes": [
                                        {
                                            "path": "/work/agenter/README.md",
                                            "kind": {"type": "add"},
                                            "diff": "+hello"
                                        }
                                    ]
                                },
                                {
                                    "type": "collabAgentToolCall",
                                    "id": "tool-1",
                                    "tool": "spawnAgent",
                                    "status": "completed",
                                    "prompt": "Implement task"
                                },
                                {
                                    "type": "plan",
                                    "id": "plan-1",
                                    "text": "# Plan\n\n1. Test"
                                }
                            ]
                        }
                    ]
                }
            }
        });

        assert_eq!(
            codex_threads_from_list_response(&list_response),
            vec![CodexDiscoveredThread {
                external_session_id: "thread-1".to_owned(),
                title: Some("Imported Thread".to_owned()),
            }]
        );

        let observed_list_response = json!({
            "id": 4,
            "result": {
                "data": [
                    {
                        "id": "019ddf92-1e65-7e72-b656-c317a83e0b93",
                        "preview": "Let's revamp the frontend.",
                        "cwd": "/Users/maxim/work/agenter",
                        "source": "cli"
                    }
                ],
                "nextCursor": null,
                "backwardsCursor": "2026-04-30T18:06:28.547Z"
            }
        });

        assert_eq!(
            codex_threads_from_list_response(&observed_list_response),
            vec![CodexDiscoveredThread {
                external_session_id: "019ddf92-1e65-7e72-b656-c317a83e0b93".to_owned(),
                title: Some("Let's revamp the frontend.".to_owned()),
            }]
        );

        assert_eq!(
            codex_history_from_thread_read_response(&read_response),
            vec![
                DiscoveredSessionHistoryItem::UserMessage {
                    message_id: Some("user-1".to_owned()),
                    content: "hello".to_owned(),
                },
                DiscoveredSessionHistoryItem::AgentMessage {
                    message_id: "agent-1".to_owned(),
                    content: "hi".to_owned(),
                },
                DiscoveredSessionHistoryItem::Command {
                    command_id: "cmd-1".to_owned(),
                    command: "cargo test".to_owned(),
                    cwd: Some("/work/agenter".to_owned()),
                    source: Some("unifiedExecStartup".to_owned()),
                    process_id: Some("123".to_owned()),
                    duration_ms: Some(17),
                    actions: vec![DiscoveredCommandAction {
                        kind: "read".to_owned(),
                        command: Some("sed -n '1,20p' /tmp/skills/demo/SKILL.md".to_owned()),
                        path: Some("/tmp/skills/demo/SKILL.md".to_owned()),
                        name: Some("SKILL.md".to_owned()),
                        query: None,
                        provider_payload: Some(json!({
                            "type": "read",
                            "command": "sed -n '1,20p' /tmp/skills/demo/SKILL.md",
                            "name": "SKILL.md",
                            "path": "/tmp/skills/demo/SKILL.md"
                        })),
                    }],
                    output: Some("ok".to_owned()),
                    exit_code: Some(0),
                    success: true,
                    provider_payload: Some(json!({
                        "type": "commandExecution",
                        "id": "cmd-1",
                        "command": "cargo test",
                        "cwd": "/work/agenter",
                        "status": "completed",
                        "exitCode": 0,
                        "durationMs": 17,
                        "source": "unifiedExecStartup",
                        "processId": "123",
                        "aggregatedOutput": "ok",
                        "commandActions": [
                            {
                                "type": "read",
                                "command": "sed -n '1,20p' /tmp/skills/demo/SKILL.md",
                                "name": "SKILL.md",
                                "path": "/tmp/skills/demo/SKILL.md"
                            }
                        ]
                    })),
                },
                DiscoveredSessionHistoryItem::FileChange {
                    change_id: "file-1:0".to_owned(),
                    path: "/work/agenter/README.md".to_owned(),
                    change_kind: FileChangeKind::Create,
                    status: DiscoveredFileChangeStatus::Applied,
                    diff: Some("+hello".to_owned()),
                    provider_payload: Some(json!({
                        "type": "fileChange",
                        "id": "file-1",
                        "status": "completed",
                        "changes": [
                            {
                                "path": "/work/agenter/README.md",
                                "kind": {"type": "add"},
                                "diff": "+hello"
                            }
                        ]
                    })),
                },
                DiscoveredSessionHistoryItem::Tool {
                    tool_call_id: "tool-1".to_owned(),
                    name: "spawnAgent".to_owned(),
                    title: Some("spawnAgent".to_owned()),
                    status: DiscoveredToolStatus::Completed,
                    input: Some(json!({
                        "type": "collabAgentToolCall",
                        "id": "tool-1",
                        "tool": "spawnAgent",
                        "status": "completed",
                        "prompt": "Implement task"
                    })),
                    output: None,
                    provider_payload: Some(json!({
                        "type": "collabAgentToolCall",
                        "id": "tool-1",
                        "tool": "spawnAgent",
                        "status": "completed",
                        "prompt": "Implement task"
                    })),
                },
                DiscoveredSessionHistoryItem::Plan {
                    plan_id: "plan-1".to_owned(),
                    title: Some("Implementation plan".to_owned()),
                    content: "# Plan\n\n1. Test".to_owned(),
                    provider_payload: Some(json!({
                        "type": "plan",
                        "id": "plan-1",
                        "text": "# Plan\n\n1. Test"
                    })),
                },
            ]
        );
    }

    #[test]
    fn thread_start_request_uses_provider_owned_startup_source() {
        let params = codex_thread_start_params(&PathBuf::from("/work/agenter"));

        assert_eq!(params["sessionStartSource"], "startup");
        assert_ne!(params["sessionStartSource"], "agenter");
    }

    #[test]
    fn summarizes_codex_jsonrpc_errors_with_method_context() {
        let message = json!({
            "id": 2,
            "error": {
                "code": -32602,
                "message": "invalid thread/start params"
            }
        });

        assert_eq!(
            codex_jsonrpc_error_summary("thread/start", &message),
            Some("codex thread/start failed: -32602 invalid thread/start params".to_owned())
        );
    }

    #[test]
    fn missing_thread_id_error_includes_recent_stderr() {
        let message = json!({
            "id": 2,
            "result": {
                "thread": {
                    "status": "failed"
                }
            }
        });
        let stderr = vec!["Failed to create shell snapshot".to_owned()];

        assert_eq!(
            missing_thread_id_error("thread/start", &message, &stderr),
            "codex thread/start response did not include a thread id; recent stderr: Failed to create shell snapshot"
        );
    }

    #[test]
    fn maps_approval_decisions_to_codex_results() {
        assert_eq!(
            approval_response_for_decision(CodexApprovalKind::Command, ApprovalDecision::Accept),
            json!({"decision": "accept"})
        );
        assert_eq!(
            approval_response_for_decision(
                CodexApprovalKind::Command,
                ApprovalDecision::AcceptForSession
            ),
            json!({"decision": "acceptForSession"})
        );
        assert_eq!(
            approval_response_for_decision(CodexApprovalKind::Command, ApprovalDecision::Decline),
            json!({"decision": "decline"})
        );
        assert_eq!(
            approval_response_for_decision(
                CodexApprovalKind::Permissions,
                ApprovalDecision::Accept
            ),
            json!({"permissions": {"fileSystem": null, "network": null}, "scope": "turn"})
        );
    }

    #[test]
    fn maps_codex_model_and_collaboration_mode_lists() {
        let models = json!({
            "id": 6,
            "result": {
                "data": [
                    {
                        "id": "gpt-5.4",
                        "model": "gpt-5.4",
                        "displayName": "GPT-5.4",
                        "description": "Balanced",
                        "hidden": false,
                        "isDefault": true,
                        "defaultReasoningEffort": "medium",
                        "supportedReasoningEfforts": [
                            {"effort": "low"},
                            {"effort": "medium"},
                            {"effort": "high"}
                        ],
                        "inputModalities": ["text", "image"]
                    }
                ]
            }
        });
        let modes = json!({
            "id": 7,
            "result": {
                "data": [
                    {
                        "name": "Planning",
                        "mode": "plan",
                        "model": "gpt-5.4",
                        "reasoning_effort": "high"
                    }
                ]
            }
        });

        let options = codex_agent_options_from_responses(&models, &modes);

        assert_eq!(options.models[0].id, "gpt-5.4");
        assert_eq!(options.models[0].display_name, "GPT-5.4");
        assert_eq!(
            options.models[0].default_reasoning_effort,
            Some(agenter_core::AgentReasoningEffort::Medium)
        );
        assert_eq!(options.collaboration_modes[0].id, "plan");
        assert_eq!(
            options.collaboration_modes[0].reasoning_effort,
            Some(agenter_core::AgentReasoningEffort::High)
        );
    }

    #[test]
    fn turn_start_params_include_model_effort_and_plan_mode() {
        let request = CodexTurnRequest {
            session_id: SessionId::nil(),
            workspace_path: PathBuf::from("/work/agenter"),
            external_session_id: Some("thread-1".to_owned()),
            prompt: "Plan this".to_owned(),
            settings: Some(agenter_core::AgentTurnSettings {
                model: Some("gpt-5.4".to_owned()),
                reasoning_effort: Some(agenter_core::AgentReasoningEffort::High),
                collaboration_mode: Some("plan".to_owned()),
            }),
        };

        let params = codex_turn_start_params("thread-1", &request);

        assert_eq!(params["model"], "gpt-5.4");
        assert_eq!(params["effort"], "high");
        assert_eq!(params["collaborationMode"]["mode"], "plan");
        assert_eq!(params["collaborationMode"]["settings"]["model"], "gpt-5.4");
        assert_eq!(
            params["collaborationMode"]["settings"]["reasoning_effort"],
            "high"
        );
    }

    #[test]
    fn normalizes_tool_user_input_request_with_multiple_answer_values() {
        let message = json!({
            "id": 99,
            "method": "item/tool/requestUserInput",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "itemId": "item-1",
                "questions": [
                    {
                        "id": "target",
                        "header": "Target",
                        "question": "Choose targets",
                        "options": [
                            {"label": "Web", "description": "Browser UI"},
                            {"label": "Runner", "description": "Runner daemon"}
                        ]
                    }
                ]
            }
        });

        let Some((question_id, native_request_id, event)) =
            normalize_codex_question_request(SessionId::nil(), &message)
        else {
            panic!("expected question request");
        };

        assert_eq!(native_request_id, json!(99));
        if let AppEvent::QuestionRequested(payload) = event {
            assert_eq!(payload.question_id, question_id);
            assert_eq!(payload.fields[0].kind, "single_select");
            assert_eq!(payload.fields[0].choices[1].value, "Runner");
        } else {
            panic!("unexpected event");
        }
    }

    #[test]
    fn normalizes_mcp_elicitation_multi_select_form() {
        let message = json!({
            "id": "mcp-1",
            "method": "mcpServer/elicitation/request",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "serverName": "demo",
                "mode": "form",
                "message": "Pick targets",
                "requestedSchema": {
                    "type": "object",
                    "required": ["targets"],
                    "properties": {
                        "targets": {
                            "type": "array",
                            "title": "Targets",
                            "description": "Choose one or more",
                            "items": {
                                "type": "string",
                                "enum": ["web", "runner"]
                            },
                            "default": ["web"]
                        }
                    }
                }
            }
        });

        let Some((_question_id, _native_request_id, event)) =
            normalize_codex_question_request(SessionId::nil(), &message)
        else {
            panic!("expected question request");
        };

        if let AppEvent::QuestionRequested(payload) = event {
            assert_eq!(payload.title, "demo");
            assert_eq!(payload.description.as_deref(), Some("Pick targets"));
            assert_eq!(payload.fields[0].kind, "multi_select");
            assert_eq!(payload.fields[0].default_answers, vec!["web"]);
            assert_eq!(payload.fields[0].choices[1].value, "runner");
        } else {
            panic!("unexpected event");
        }
    }
}

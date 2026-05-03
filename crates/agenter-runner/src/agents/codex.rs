use std::{
    collections::{HashMap, VecDeque},
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::agents::adapter::AdapterEvent;
use crate::agents::approval_state::PendingProviderApproval;
use crate::agents::codex_approval_context::{
    presentation_for_command_execution_approval, sparse_file_change_fallback_details,
    CodexApprovalItemCache,
};
use agenter_core::{
    AgentCollaborationMode, AgentErrorEvent, AgentMessageDeltaEvent, AgentModelOption,
    AgentOptions, AgentProviderId, AgentQuestionAnswer, AgentQuestionChoice, AgentQuestionField,
    AgentReasoningEffort, AgentTurnSettings, ApprovalDecision, ApprovalId, ApprovalKind,
    ApprovalRequestEvent, CommandCompletedEvent, CommandEvent, CommandOutputEvent,
    CommandOutputStream, FileChangeEvent, FileChangeKind, MessageCompletedEvent,
    NativeNotification, NormalizedEvent, PlanEntry, PlanEntryStatus, PlanEvent, QuestionId,
    QuestionRequestedEvent, SessionId, SessionStatus, SessionStatusChangedEvent,
    SlashCommandArgument, SlashCommandArgumentKind, SlashCommandDangerLevel,
    SlashCommandDefinition, SlashCommandRequest, SlashCommandResult, SlashCommandTarget,
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
const DEFAULT_CODEX_RAW_LOG_DIR: &str = "tmp/agenter-logs/codex-wire";

/// Shown when Codex emits `account/chatgptAuthTokens/refresh` — browser cannot authenticate the runner host.
pub const CODEX_AUTH_REFRESH_OPERATOR_MESSAGE: &str = "Codex login or token refresh is required on the runner host (for example HTTP 401 from the Codex backend). SSH into the machine running `agenter-runner`, sign in using the Codex CLI in that environment, then retry this chat.";

#[derive(Clone, Debug)]
pub struct CodexTurnRequest {
    pub session_id: SessionId,
    pub workspace_path: PathBuf,
    pub external_session_id: Option<String>,
    pub prompt: String,
    pub settings: Option<AgentTurnSettings>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CodexRequestId {
    Integer(i64),
    String(String),
}

impl CodexRequestId {
    fn as_value(&self) -> Value {
        match self {
            Self::Integer(value) => json!(*value),
            Self::String(value) => json!(value),
        }
    }

    fn numeric(value: i64) -> Self {
        Self::Integer(value)
    }
}

fn codex_request_id_from_value(id: &Value) -> Option<CodexRequestId> {
    match id {
        Value::Number(value) => value.as_i64().map(CodexRequestId::numeric),
        Value::String(value) => Some(CodexRequestId::String(value.to_owned())),
        _ => None,
    }
}

pub type PendingCodexApproval = PendingProviderApproval;

#[derive(Debug)]
pub struct PendingCodexQuestion {
    pub response: oneshot::Sender<AgentQuestionAnswer>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodexDiscoveredThread {
    pub external_session_id: String,
    pub title: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodexApprovalKind {
    Command,
    FileChange,
    Permissions,
    /// Wire method `execCommandApproval` (`ServerRequest::ExecCommandApproval`).
    ExecCommandApproval,
    /// Wire method `applyPatchApproval` (`ServerRequest::ApplyPatchApproval`).
    ApplyPatchApproval,
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
    workspace_path: PathBuf,
    active_session_id: Option<SessionId>,
    next_id: i64,
    thread_id: Option<String>,
    turn_id: Option<String>,
    initialized: bool,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    /// Responses/notifications/read while awaiting a synchronous request; drained by `next_message`.
    interleaved_messages: VecDeque<Value>,
    initialize_capabilities: Vec<String>,
    wire_logger: CodexWireLogger,
}

#[derive(Clone, Debug)]
struct CodexWireLogger {
    file: Option<Arc<Mutex<File>>>,
}

#[derive(Clone, Debug)]
struct CodexWireLogRecord {
    direction: &'static str,
    classification: &'static str,
    session_id: Option<SessionId>,
    workspace: Option<String>,
    runtime_thread_id: Option<String>,
    runtime_turn_id: Option<String>,
    reason: Option<&'static str>,
    message: Option<Value>,
    stderr_line: Option<String>,
    scope: Option<CodexScopeLogContext>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CodexScopeLogContext {
    expected_thread_id: Option<String>,
    expected_turn_id: Option<String>,
    actual_thread_id: Option<String>,
    actual_turn_id: Option<String>,
    scope_match: bool,
    reason: Option<String>,
}

impl CodexWireLogger {
    fn from_env(workspace_path: &Path) -> Self {
        if !env_flag_enabled("AGENTER_CODEX_RAW_LOG") {
            return Self::disabled();
        }
        match Self::open_from_env(workspace_path) {
            Ok(logger) => logger,
            Err(error) => {
                tracing::warn!(%error, "failed to initialize codex raw wire log");
                Self::disabled()
            }
        }
    }

    fn disabled() -> Self {
        Self { file: None }
    }

    fn open_from_env(workspace_path: &Path) -> anyhow::Result<Self> {
        let dir = std::env::var("AGENTER_CODEX_RAW_LOG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(DEFAULT_CODEX_RAW_LOG_DIR));
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create codex raw log dir {}", dir.display()))?;
        let workspace_label = raw_log_workspace_label(workspace_path);
        let path = dir.join(format!(
            "codex-wire-{workspace_label}-{}.jsonl",
            std::process::id()
        ));
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("failed to open codex raw log {}", path.display()))?;
        tracing::info!(path = %path.display(), "codex raw wire log enabled");
        Ok(Self {
            file: Some(Arc::new(Mutex::new(file))),
        })
    }

    #[cfg(test)]
    fn for_test_file(path: PathBuf) -> Self {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("test log file should open");
        Self {
            file: Some(Arc::new(Mutex::new(file))),
        }
    }

    fn record(&self, record: CodexWireLogRecord) -> anyhow::Result<()> {
        let Some(file) = &self.file else {
            return Ok(());
        };
        let mut output = json!({
            "ts": unix_timestamp_millis(),
            "direction": record.direction,
            "classification": record.classification,
        });
        insert_optional_string(
            &mut output,
            "session_id",
            record.session_id.map(|id| id.to_string()),
        );
        insert_optional_string(&mut output, "workspace", record.workspace);
        insert_optional_string(&mut output, "runtime_thread_id", record.runtime_thread_id);
        insert_optional_string(&mut output, "runtime_turn_id", record.runtime_turn_id);
        insert_optional_string(
            &mut output,
            "provider_thread_id",
            record
                .message
                .as_ref()
                .and_then(message_thread_id)
                .map(str::to_owned),
        );
        insert_optional_string(
            &mut output,
            "provider_turn_id",
            record
                .message
                .as_ref()
                .and_then(message_turn_id)
                .map(str::to_owned),
        );
        insert_optional_string(
            &mut output,
            "jsonrpc_id",
            record
                .message
                .as_ref()
                .map(codex_jsonrpc_request_id_summary),
        );
        insert_optional_string(
            &mut output,
            "method",
            record
                .message
                .as_ref()
                .and_then(jsonrpc_method)
                .map(str::to_owned),
        );
        insert_optional_string(&mut output, "reason", record.reason.map(str::to_owned));
        if let Some(scope) = record.scope {
            output["expected_thread_id"] = option_string_value(scope.expected_thread_id);
            output["expected_turn_id"] = option_string_value(scope.expected_turn_id);
            output["actual_thread_id"] = option_string_value(scope.actual_thread_id);
            output["actual_turn_id"] = option_string_value(scope.actual_turn_id);
            output["scope_match"] = Value::Bool(scope.scope_match);
            output["scope_reason"] = option_string_value(scope.reason);
        }
        if let Some(message) = record.message {
            output["payload"] = message;
        }
        if let Some(line) = record.stderr_line {
            output["stderr"] = Value::String(line);
        }

        let encoded = serde_json::to_vec(&output)?;
        let mut file = file
            .lock()
            .map_err(|_| anyhow!("codex raw wire log file lock poisoned"))?;
        file.write_all(&encoded)?;
        file.write_all(b"\n")?;
        file.flush()?;
        Ok(())
    }
}

impl CodexScopeLogContext {
    fn from_message(message: &Value, scope: &CodexTurnScope) -> Self {
        let expected_thread_id = scope.thread_id.clone();
        let expected_turn_id = scope.turn_id.clone();
        let actual_thread_id = message_thread_id(message).map(str::to_owned);
        let actual_turn_id = message_turn_id(message).map(str::to_owned);
        let scope_match = codex_message_belongs_to_scope(message, scope);
        let reason = if !scope_match {
            Some("thread_id_mismatch".to_owned())
        } else {
            None
        };
        Self {
            expected_thread_id,
            expected_turn_id,
            actual_thread_id,
            actual_turn_id,
            scope_match,
            reason,
        }
    }
}

impl CodexAppServer {
    pub fn spawn(workspace_path: PathBuf) -> anyhow::Result<Self> {
        Self::spawn_with_initialize_capabilities(workspace_path, Vec::new())
    }

    pub fn spawn_with_initialize_capabilities(
        workspace_path: PathBuf,
        opt_out_notification_methods: Vec<String>,
    ) -> anyhow::Result<Self> {
        tracing::info!(workspace = %workspace_path.display(), "spawning codex app-server");
        let wire_logger = CodexWireLogger::from_env(&workspace_path);
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
            let stderr_wire_logger = wire_logger.clone();
            let stderr_workspace = workspace_path.display().to_string();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Ok(mut tail) = stderr_tail.lock() {
                        if tail.len() == RECENT_STDERR_LINES {
                            tail.pop_front();
                        }
                        tail.push_back(line.clone());
                    }
                    if let Err(error) = stderr_wire_logger.record(CodexWireLogRecord {
                        direction: "stderr",
                        classification: "provider_stderr",
                        session_id: None,
                        workspace: Some(stderr_workspace.clone()),
                        runtime_thread_id: None,
                        runtime_turn_id: None,
                        reason: None,
                        message: None,
                        stderr_line: Some(line.clone()),
                        scope: None,
                    }) {
                        tracing::warn!(%error, "failed to write codex stderr wire log record");
                    }
                    tracing::warn!(target: "codex-stderr", "{line}");
                }
            });
        }

        Ok(Self {
            _child: child,
            stdin,
            stdout: BufReader::new(stdout),
            workspace_path,
            active_session_id: None,
            next_id: 1,
            thread_id: None,
            turn_id: None,
            initialized: false,
            stderr_tail,
            interleaved_messages: VecDeque::new(),
            initialize_capabilities: opt_out_notification_methods,
            wire_logger,
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
                    "capabilities": self.initialize_capabilities_payload(),
                }),
            )
            .await?;
        self.read_response(&initialize_id, "initialize").await?;
        self.initialized = true;
        Ok(())
    }

    pub async fn start_thread(&mut self, workspace_path: &PathBuf) -> anyhow::Result<String> {
        self.initialize().await?;
        tracing::info!("starting codex thread");
        let start_id = self
            .send_request("thread/start", codex_thread_start_params(workspace_path))
            .await?;
        let response = self.read_response(&start_id, "thread/start").await?;
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
        self.read_response(&resume_id, "thread/resume").await?;
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
        let response = self.read_response(&list_id, "thread/list").await?;
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
        let response = self.read_response(&read_id, "thread/read").await?;
        Ok(codex_history_from_thread_read_response(&response))
    }

    pub async fn agent_options(&mut self) -> anyhow::Result<AgentOptions> {
        self.initialize().await?;
        let models_id = self
            .send_request("model/list", json!({"includeHidden": false}))
            .await?;
        let models = self.read_response(&models_id, "model/list").await?;
        let modes_id = self
            .send_request("collaborationMode/list", json!({}))
            .await?;
        let modes = self
            .read_response(&modes_id, "collaborationMode/list")
            .await?;
        Ok(codex_agent_options_from_responses(&models, &modes))
    }

    pub async fn execute_provider_command(
        &mut self,
        request: &SlashCommandRequest,
        workspace_path: &PathBuf,
    ) -> anyhow::Result<SlashCommandResult> {
        self.initialize().await?;
        let Some(thread_id) = self.thread_id.clone() else {
            return Err(anyhow!(
                "codex thread id was not observed before provider command"
            ));
        };
        let (method, params) = codex_provider_command_request(
            &thread_id,
            request,
            self.turn_id.as_deref(),
            workspace_path,
        )?;
        tracing::info!(provider_thread_id = %thread_id, method, command_id = %request.command_id, "executing codex provider command");
        let request_id = self.send_request(method, params).await?;
        let response = self.read_response(&request_id, method).await?;
        if let Some(thread_id) = codex_thread_id(&response) {
            self.thread_id = Some(thread_id.to_owned());
        }
        Ok(SlashCommandResult {
            accepted: true,
            message: codex_provider_command_message(&request.command_id).to_owned(),
            session: None,
            provider_payload: Some(response),
        })
    }

    pub fn set_active_thread(&mut self, thread_id: impl Into<String>) {
        self.thread_id = Some(thread_id.into());
        self.turn_id = None;
    }

    pub async fn send_turn(&mut self, request: &CodexTurnRequest) -> anyhow::Result<()> {
        self.active_session_id = Some(request.session_id);
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
        let response = self.read_response(&turn_start_id, "turn/start").await?;
        if let Some(turn_id) = codex_turn_id(&response) {
            self.turn_id = Some(turn_id.to_owned());
        }
        Ok(())
    }

    pub async fn next_message(&mut self) -> anyhow::Result<Option<Value>> {
        let message = match self.interleaved_messages.pop_front() {
            Some(m) => {
                self.record_wire_message(
                    "internal",
                    "interleaved_drained",
                    Some("dequeued_for_turn_loop"),
                    &m,
                    None,
                );
                m
            }
            None => match self.read_codex_stdio_json_line().await? {
                Some(m) => {
                    Self::observe_codex_thread_turn_targets(self, &m);
                    m
                }
                None => return Ok(None),
            },
        };

        tracing::debug!(
            method = message.get("method").and_then(serde_json::Value::as_str),
            id = ?message.get("id"),
            payload_preview = agenter_core::logging::payload_preview(
                &message,
                agenter_core::logging::payload_logging_enabled()
            )
            .as_deref(),
            "received codex json-rpc message"
        );
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
        let response = json!({
            "id": native_request_id,
            "result": approval_response_for_decision(approval_kind, decision)
        });
        self.record_wire_message(
            "send",
            "client_response_sent",
            Some("approval_answer"),
            &response,
            None,
        );
        write_json(&mut self.stdin, &response).await
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
        let response = json!({
            "id": native_request_id,
            "result": question_response_for_answer(kind, answer)
        });
        self.record_wire_message(
            "send",
            "client_response_sent",
            Some("question_answer"),
            &response,
            None,
        );
        write_json(&mut self.stdin, &response).await
    }

    pub async fn send_unsupported_request_response(
        &mut self,
        native_request_id: Value,
        method: &str,
    ) -> anyhow::Result<()> {
        tracing::warn!(
            native_request_id = ?native_request_id,
            method,
            "rejecting unsupported codex server request"
        );
        let response = unsupported_request_response(native_request_id, method);
        self.record_wire_message(
            "send",
            "client_response_sent",
            Some("unsupported_request"),
            &response,
            None,
        );
        write_json(&mut self.stdin, &response).await
    }

    pub async fn send_jsonrpc_application_error_response(
        &mut self,
        native_request_id: Value,
        code: i64,
        message: &str,
        data: Option<Value>,
    ) -> anyhow::Result<()> {
        let mut envelope = json!({
            "id": native_request_id,
            "error": {
                "code": code,
                "message": message,
            }
        });
        if let Some(ref d) = data {
            envelope["error"]["data"] = d.clone();
        }
        tracing::warn!(%code, %message, "sending codex json-rpc application error response");
        self.record_wire_message(
            "send",
            "client_response_sent",
            Some("application_error"),
            &envelope,
            None,
        );
        write_json(&mut self.stdin, &envelope).await
    }

    async fn send_request(
        &mut self,
        method: &str,
        params: Value,
    ) -> anyhow::Result<CodexRequestId> {
        let id = self.next_id;
        self.next_id += 1;
        let request_id = CodexRequestId::numeric(id);
        tracing::debug!(
            request_id = ?request_id,
            method,
            payload_preview = agenter_core::logging::payload_preview(
                &params,
                agenter_core::logging::payload_logging_enabled()
            )
            .as_deref(),
            "sending codex json-rpc request"
        );
        let request = json!({
            "id": request_id.as_value(),
            "method": method,
            "params": params
        });
        self.record_wire_message(
            "send",
            "client_request_sent",
            Some("jsonrpc_request"),
            &request,
            None,
        );
        write_json(&mut self.stdin, &request).await?;
        Ok(request_id)
    }

    fn initialize_capabilities_payload(&self) -> Value {
        let mut capabilities = json!({"experimentalApi": true});
        if !self.initialize_capabilities.is_empty() {
            capabilities["optOutNotificationMethods"] = Value::Array(
                self.initialize_capabilities
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect(),
            );
        }
        capabilities
    }

    async fn read_response(
        &mut self,
        request_id: &CodexRequestId,
        method: &str,
    ) -> anyhow::Result<Value> {
        let message = timeout(
            STARTUP_RESPONSE_TIMEOUT,
            self.take_pending_codex_jsonrpc_response(request_id),
        )
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
        if let Some(summary) = codex_jsonrpc_error_summary(method, &message) {
            return Err(anyhow!(startup_error_with_stderr(
                summary,
                &self.recent_stderr()
            )));
        }
        Ok(message)
    }

    async fn take_pending_codex_jsonrpc_response(
        &mut self,
        request_id: &CodexRequestId,
    ) -> anyhow::Result<Option<Value>> {
        loop {
            for i in 0..self.interleaved_messages.len() {
                let Some(candidate_ref) = self.interleaved_messages.get(i) else {
                    continue;
                };
                if codex_rpc_is_response_matching_request(candidate_ref, request_id) {
                    let matched = self
                        .interleaved_messages
                        .remove(i)
                        .expect("interleaved dequeue index bounded by iteration");
                    Self::observe_codex_thread_turn_targets(self, &matched);
                    return Ok(Some(matched));
                }
            }
            match self.read_codex_stdio_json_line().await? {
                None => return Ok(None),
                Some(m) => {
                    if let Some(incoming_request_id) =
                        m.get("id").and_then(codex_request_id_from_value)
                    {
                        tracing::trace!(request_id = ?incoming_request_id, "read codex json-rpc message");
                    }
                    if codex_rpc_is_response_matching_request(&m, request_id) {
                        Self::observe_codex_thread_turn_targets(self, &m);
                        return Ok(Some(m));
                    }
                    Self::observe_codex_thread_turn_targets(self, &m);
                    self.record_wire_message(
                        "internal",
                        "interleaved_queued",
                        Some("waiting_for_matching_response"),
                        &m,
                        None,
                    );
                    self.interleaved_messages.push_back(m);
                }
            }
        }
    }

    async fn read_codex_stdio_json_line(&mut self) -> anyhow::Result<Option<Value>> {
        let mut line = String::new();
        if self.stdout.read_line(&mut line).await? == 0 {
            return Ok(None);
        }
        let message = serde_json::from_str::<Value>(line.trim())
            .with_context(|| format!("codex emitted invalid JSON-RPC line: {line}"))?;
        self.record_wire_message(
            "recv",
            codex_wire_classification(&message),
            None,
            &message,
            None,
        );
        Ok(Some(message))
    }

    fn record_wire_message(
        &self,
        direction: &'static str,
        classification: &'static str,
        reason: Option<&'static str>,
        message: &Value,
        scope: Option<CodexScopeLogContext>,
    ) {
        if let Err(error) = self.wire_logger.record(CodexWireLogRecord {
            direction,
            classification,
            session_id: self.active_session_id,
            workspace: Some(self.workspace_path.display().to_string()),
            runtime_thread_id: self.thread_id.clone(),
            runtime_turn_id: self.turn_id.clone(),
            reason,
            message: Some(message.clone()),
            stderr_line: None,
            scope,
        }) {
            tracing::warn!(%error, "failed to write codex wire log record");
        }
    }

    fn observe_codex_thread_turn_targets(server: &mut CodexAppServer, message: &Value) {
        if let Some(thread_id) = codex_thread_id(message) {
            server.thread_id = Some(thread_id.to_owned());
            tracing::info!(provider_thread_id = %thread_id, "observed codex thread id");
        }
        if let Some(turn_id) = codex_turn_id(message) {
            server.turn_id = Some(turn_id.to_owned());
            tracing::debug!(provider_turn_id = %turn_id, "observed codex turn id");
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

pub fn codex_provider_slash_commands() -> Vec<SlashCommandDefinition> {
    vec![
        codex_slash_command(
            "codex.compact",
            "compact",
            "Start native Codex context compaction.",
        ),
        codex_slash_command(
            "codex.review",
            "review",
            "Start a native Codex code review.",
        )
        .with_argument(
            "target",
            SlashCommandArgumentKind::Rest,
            false,
            "Review target flags",
        ),
        codex_slash_command("codex.steer", "steer", "Steer the active Codex turn.").with_argument(
            "input",
            SlashCommandArgumentKind::Rest,
            true,
            "Text to steer with",
        ),
        codex_slash_command("codex.fork", "fork", "Fork the current Codex thread."),
        codex_slash_command(
            "codex.archive",
            "archive",
            "Archive the current Codex thread.",
        ),
        codex_slash_command(
            "codex.unarchive",
            "unarchive",
            "Unarchive the current Codex thread.",
        ),
        codex_slash_command(
            "codex.rollback",
            "rollback",
            "Drop recent turns from Codex history. Does not revert files.",
        )
        .danger(SlashCommandDangerLevel::Dangerous)
        .with_argument(
            "numTurns",
            SlashCommandArgumentKind::Number,
            true,
            "Number of turns",
        ),
        codex_slash_command(
            "codex.shell",
            "shell",
            "Run an unsandboxed provider-native shell command.",
        )
        .danger(SlashCommandDangerLevel::Dangerous)
        .with_alias("sh")
        .with_argument(
            "command",
            SlashCommandArgumentKind::Rest,
            true,
            "Shell command",
        ),
    ]
    .into_iter()
    .map(Into::into)
    .collect()
}

fn codex_provider_command_request(
    thread_id: &str,
    request: &SlashCommandRequest,
    turn_id: Option<&str>,
    workspace_path: &PathBuf,
) -> anyhow::Result<(&'static str, Value)> {
    match request.command_id.as_str() {
        "codex.compact" => Ok(("thread/compact/start", json!({"threadId": thread_id}))),
        "codex.review" => Ok((
            "review/start",
            json!({
                "threadId": thread_id,
                "target": codex_review_target(&request.arguments),
                "delivery": codex_review_delivery(&request.arguments)
            }),
        )),
        "codex.steer" => {
            let Some(turn_id) = turn_id else {
                return Err(anyhow!("codex /steer requires an active turn"));
            };
            let input = string_argument(&request.arguments, "input")?;
            Ok((
                "turn/steer",
                json!({
                    "threadId": thread_id,
                    "expectedTurnId": turn_id,
                    "input": [{"type": "text", "text": input}]
                }),
            ))
        }
        "codex.fork" => Ok((
            "thread/fork",
            json!({
                "threadId": thread_id,
                "cwd": workspace_path,
                "approvalPolicy": "on-request",
                "approvalsReviewer": "user",
                "excludeTurns": false,
                "persistExtendedHistory": true
            }),
        )),
        "codex.archive" => Ok(("thread/archive", json!({"threadId": thread_id}))),
        "codex.unarchive" => Ok(("thread/unarchive", json!({"threadId": thread_id}))),
        "codex.rollback" => Ok((
            "thread/rollback",
            json!({
                "threadId": thread_id,
                "numTurns": number_argument(&request.arguments, "numTurns")?
            }),
        )),
        "codex.shell" => Ok((
            "thread/shellCommand",
            json!({
                "threadId": thread_id,
                "command": string_argument(&request.arguments, "command")?
            }),
        )),
        other => Err(anyhow!("unsupported codex provider command `{other}`")),
    }
}

fn codex_provider_command_message(command_id: &str) -> &'static str {
    match command_id {
        "codex.compact" => "Codex compaction started.",
        "codex.review" => "Codex review started.",
        "codex.steer" => "Codex turn steering submitted.",
        "codex.fork" => "Codex thread forked.",
        "codex.archive" => "Codex thread archived.",
        "codex.unarchive" => "Codex thread unarchived.",
        "codex.rollback" => "Codex rollback completed.",
        "codex.shell" => "Codex shell command submitted.",
        _ => "Codex provider command executed.",
    }
}

fn codex_review_target(arguments: &Value) -> Value {
    if let Some(branch) = string_at(arguments, &["/base", "/branch"]) {
        return json!({"type": "baseBranch", "branch": branch});
    }
    if let Some(sha) = string_at(arguments, &["/commit", "/sha"]) {
        return json!({"type": "commit", "sha": sha});
    }
    if let Some(instructions) = string_at(arguments, &["/custom", "/instructions", "/target"]) {
        if !instructions.trim().is_empty() {
            return json!({"type": "custom", "instructions": instructions});
        }
    }
    json!({"type": "uncommittedChanges"})
}

fn codex_review_delivery(arguments: &Value) -> Value {
    if bool_at(arguments, &["/detached"]).unwrap_or(false) {
        json!("detached")
    } else {
        json!("inline")
    }
}

fn string_argument(arguments: &Value, name: &str) -> anyhow::Result<String> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("missing required argument `{name}`"))
}

fn number_argument(arguments: &Value, name: &str) -> anyhow::Result<u64> {
    arguments
        .get(name)
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .ok_or_else(|| anyhow!("missing required positive integer argument `{name}`"))
}

struct CodexSlashCommandBuilder(SlashCommandDefinition);

impl CodexSlashCommandBuilder {
    fn with_argument(
        mut self,
        name: &str,
        kind: SlashCommandArgumentKind,
        required: bool,
        description: &str,
    ) -> Self {
        self.0.arguments.push(SlashCommandArgument {
            name: name.to_owned(),
            kind,
            required,
            description: Some(description.to_owned()),
            choices: Vec::new(),
        });
        self
    }

    fn with_alias(mut self, alias: &str) -> Self {
        self.0.aliases.push(alias.to_owned());
        self
    }

    fn danger(mut self, danger_level: SlashCommandDangerLevel) -> Self {
        self.0.danger_level = danger_level;
        self
    }
}

impl From<CodexSlashCommandBuilder> for SlashCommandDefinition {
    fn from(builder: CodexSlashCommandBuilder) -> Self {
        builder.0
    }
}

fn codex_slash_command(id: &str, name: &str, description: &str) -> CodexSlashCommandBuilder {
    CodexSlashCommandBuilder(SlashCommandDefinition {
        id: id.to_owned(),
        name: name.to_owned(),
        aliases: Vec::new(),
        description: description.to_owned(),
        category: "provider".to_owned(),
        provider_id: Some(AgentProviderId::from(AgentProviderId::CODEX)),
        target: SlashCommandTarget::Provider,
        danger_level: SlashCommandDangerLevel::Safe,
        arguments: Vec::new(),
        examples: Vec::new(),
    })
}

pub async fn run_codex_turn_on_server(
    server: &mut CodexAppServer,
    request: CodexTurnRequest,
    event_sender: mpsc::UnboundedSender<AdapterEvent>,
    pending_approvals: std::sync::Arc<
        tokio::sync::Mutex<HashMap<ApprovalId, PendingCodexApproval>>,
    >,
    pending_questions: std::sync::Arc<
        tokio::sync::Mutex<HashMap<QuestionId, PendingCodexQuestion>>,
    >,
) -> anyhow::Result<()> {
    let mut codex_approval_ctx = CodexApprovalItemCache::default();
    while server.thread_id.is_none() {
        let Some(message) = server.next_message().await? else {
            return Err(anyhow!("codex exited before returning a thread id"));
        };
        codex_approval_ctx.observe_jsonrpc_message(&message);
        let method = jsonrpc_method(&message);
        for event in normalize_codex_message(request.session_id, &message) {
            send_codex_event(&event_sender, method, event);
        }
    }

    server.send_turn(&request).await?;
    let mut scope = CodexTurnScope {
        thread_id: server.thread_id.clone(),
        turn_id: server.turn_id.clone(),
    };
    while let Some(message) = server.next_message().await? {
        scope.observe(&message);
        codex_approval_ctx.observe_jsonrpc_message(&message);
        let is_server_request = codex_rpc_is_codex_server_to_client_request(&message);
        let is_server_response = codex_rpc_is_codex_server_to_client_response(&message);

        if is_server_request {
            if jsonrpc_method(&message) == Some("account/chatgptAuthTokens/refresh") {
                let Some(native_request_id) = message.get("id").cloned() else {
                    continue;
                };
                send_codex_event(
                    &event_sender,
                    jsonrpc_method(&message),
                    NormalizedEvent::Error(AgentErrorEvent {
                        session_id: Some(request.session_id),
                        code: Some("codex_auth_refresh_required".to_owned()),
                        message: CODEX_AUTH_REFRESH_OPERATOR_MESSAGE.to_owned(),
                        provider_payload: Some(message.clone()),
                    }),
                );
                server
                    .send_jsonrpc_application_error_response(
                        native_request_id,
                        -32002,
                        "Remote runner cannot refresh Codex auth tokens; authenticate on the runner host.",
                        Some(json!({ "agenter.reason": "auth_refresh_unreachable_remotely" })),
                    )
                    .await?;
                continue;
            }

            if jsonrpc_method(&message) == Some("item/tool/call") {
                let Some(native_request_id) = message.get("id").cloned() else {
                    continue;
                };
                send_codex_event(
                    &event_sender,
                    jsonrpc_method(&message),
                    NormalizedEvent::Error(AgentErrorEvent {
                        session_id: Some(request.session_id),
                        code: Some("codex_dynamic_tool_unsupported".to_owned()),
                        message: "Codex requested a dynamic tool call that Agenter cannot execute remotely."
                            .to_owned(),
                        provider_payload: Some(message.clone()),
                    }),
                );
                server
                    .send_unsupported_request_response(native_request_id, "item/tool/call")
                    .await?;
                continue;
            }

            if let Some((approval_id, native_request_id, approval_kind, event)) =
                normalize_codex_approval_request(
                    request.session_id,
                    &message,
                    Some(&codex_approval_ctx),
                )
            {
                let (sender, receiver) = oneshot::channel();
                pending_approvals.lock().await.insert(
                    approval_id,
                    PendingCodexApproval::new(request.session_id, sender),
                );
                send_codex_event(
                    &event_sender,
                    jsonrpc_method(&message),
                    session_status_event(
                        request.session_id,
                        SessionStatus::WaitingForApproval,
                        Some("Codex is waiting for approval.".to_owned()),
                    ),
                );
                tracing::info!(
                    session_id = %request.session_id,
                    %approval_id,
                    native_request_id = ?native_request_id,
                    ?approval_kind,
                    provider_thread_id = message_thread_id(&message),
                    provider_turn_id = message_turn_id(&message),
                    method = jsonrpc_method(&message),
                    "codex approval request pending"
                );
                send_codex_event(&event_sender, jsonrpc_method(&message), event);
                if let Ok(answer) = receiver.await {
                    tracing::info!(
                        session_id = %request.session_id,
                        %approval_id,
                        native_request_id = ?native_request_id,
                        ?approval_kind,
                        decision = ?answer.decision,
                        "delivering codex approval response"
                    );
                    let result = server
                        .send_approval_response(native_request_id, approval_kind, answer.decision)
                        .await
                        .map_err(|error| error.to_string());
                    answer.acknowledged.send(result.clone()).ok();
                    result.map_err(anyhow::Error::msg)?;
                    send_codex_event(
                        &event_sender,
                        jsonrpc_method(&message),
                        session_status_event(
                            request.session_id,
                            SessionStatus::Running,
                            Some("Approval answered.".to_owned()),
                        ),
                    );
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
                send_codex_event(
                    &event_sender,
                    jsonrpc_method(&message),
                    session_status_event(
                        request.session_id,
                        SessionStatus::WaitingForInput,
                        Some("Codex is waiting for input.".to_owned()),
                    ),
                );
                tracing::info!(
                    session_id = %request.session_id,
                    %question_id,
                    ?question_kind,
                    native_request_id = ?native_request_id,
                    provider_thread_id = message_thread_id(&message),
                    provider_turn_id = message_turn_id(&message),
                    method = jsonrpc_method(&message),
                    "codex question request pending"
                );
                send_codex_event(&event_sender, jsonrpc_method(&message), event);
                if let Ok(answer) = receiver.await {
                    tracing::info!(
                        session_id = %request.session_id,
                        %question_id,
                        native_request_id = ?native_request_id,
                        ?question_kind,
                        "delivering codex question response"
                    );
                    server
                        .send_question_response(native_request_id, question_kind, answer)
                        .await?;
                    send_codex_event(
                        &event_sender,
                        jsonrpc_method(&message),
                        session_status_event(
                            request.session_id,
                            SessionStatus::Running,
                            Some("Input answered.".to_owned()),
                        ),
                    );
                }
                continue;
            }
            if let Some((native_request_id, method)) = unsupported_codex_server_request(&message) {
                tracing::warn!(
                    session_id = %request.session_id,
                    method,
                    request_id = ?native_request_id,
                    provider_thread_id = message_thread_id(&message),
                    provider_turn_id = message_turn_id(&message),
                    "unsupported codex server request observed in turn loop"
                );
                let event_method = jsonrpc_method(&message);
                for event in normalize_codex_message(request.session_id, &message) {
                    send_codex_event(&event_sender, event_method, event);
                }
                if let Err(err) = server
                    .send_unsupported_request_response(native_request_id.clone(), method)
                    .await
                {
                    tracing::warn!(
                        ?err,
                        method,
                        request_id = ?native_request_id,
                        "failed to send unsupported codex request response"
                    );
                }
                continue;
            }
        }

        if is_server_response {
            tracing::warn!(
                provider_thread_id = message_thread_id(&message),
                provider_turn_id = message_turn_id(&message),
                method = jsonrpc_method(&message),
                jsonrpc_request_id = %codex_jsonrpc_request_id_summary(&message),
                "received unexpected codex response while in turn loop; dropping"
            );
            continue;
        }

        if !codex_message_belongs_to_scope(&message, &scope) {
            let scope_context = CodexScopeLogContext::from_message(&message, &scope);
            server.record_wire_message(
                "internal",
                "scope_dropped",
                Some("thread_id_mismatch"),
                &message,
                Some(scope_context.clone()),
            );
            tracing::debug!(
                method = jsonrpc_method(&message),
                jsonrpc_request_id = %codex_jsonrpc_request_id_summary(&message),
                classification = codex_wire_classification(&message),
                expected_thread_id = scope_context.expected_thread_id.as_deref(),
                expected_turn_id = scope_context.expected_turn_id.as_deref(),
                actual_thread_id = scope_context.actual_thread_id.as_deref(),
                actual_turn_id = scope_context.actual_turn_id.as_deref(),
                scope_match = scope_context.scope_match,
                reason = scope_context.reason.as_deref(),
                "ignored codex message outside active turn scope"
            );
            continue;
        }

        let completed = jsonrpc_method(&message) == Some("turn/completed");
        let event_method = jsonrpc_method(&message);
        for event in normalize_codex_message_for_scope(request.session_id, &message, &scope) {
            send_codex_event(&event_sender, event_method, event);
        }
        if completed {
            tracing::info!(session_id = %request.session_id, "codex turn completed");
            return Ok(());
        }
    }

    Ok(())
}

fn codex_adapter_event(method: Option<&str>, event: NormalizedEvent) -> AdapterEvent {
    AdapterEvent::from_normalized_event(
        AgentProviderId::from(AgentProviderId::CODEX),
        "codex-app-server",
        method,
        event,
    )
}

fn send_codex_event(
    event_sender: &mpsc::UnboundedSender<AdapterEvent>,
    method: Option<&str>,
    event: NormalizedEvent,
) {
    event_sender.send(codex_adapter_event(method, event)).ok();
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

fn codex_wire_classification(message: &Value) -> &'static str {
    if codex_rpc_is_codex_server_to_client_request(message) {
        "server_request_received"
    } else if codex_rpc_is_codex_server_to_client_response(message) {
        "server_response_received"
    } else if jsonrpc_method(message).is_some() {
        "server_notification_received"
    } else {
        "unknown_received"
    }
}

fn env_flag_enabled(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "True"))
}

fn raw_log_workspace_label(path: &Path) -> String {
    let raw = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("workspace");
    raw.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn unix_timestamp_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn option_string_value(value: Option<String>) -> Value {
    value.map(Value::String).unwrap_or(Value::Null)
}

fn insert_optional_string(output: &mut Value, key: &str, value: Option<String>) {
    if let Some(value) = value {
        output[key] = Value::String(value);
    }
}

pub fn normalize_codex_message(session_id: SessionId, message: &Value) -> Vec<NormalizedEvent> {
    normalize_codex_message_inner(session_id, message)
}

#[allow(dead_code)]
pub mod codex_codec {
    use serde_json::Value;

    #[must_use]
    pub fn method(message: &Value) -> Option<&str> {
        super::jsonrpc_method(message)
    }
}

#[allow(dead_code)]
pub mod codex_reducer {
    use agenter_core::{AgentProviderId, SessionId};
    use serde_json::Value;

    use crate::agents::adapter::{normalized_events_to_adapter_events, AdapterEvent};

    #[must_use]
    pub fn reduce_native_message(session_id: SessionId, message: &Value) -> Vec<AdapterEvent> {
        let method = super::codex_codec::method(message);
        normalized_events_to_adapter_events(
            AgentProviderId::from(AgentProviderId::CODEX),
            "codex-app-server",
            method,
            super::normalize_codex_message(session_id, message),
        )
    }
}

fn normalize_codex_message_for_scope(
    session_id: SessionId,
    message: &Value,
    scope: &CodexTurnScope,
) -> Vec<NormalizedEvent> {
    if !codex_message_belongs_to_scope(message, scope) {
        return Vec::new();
    }
    normalize_codex_message_inner(session_id, message)
}

fn normalize_codex_message_inner(session_id: SessionId, message: &Value) -> Vec<NormalizedEvent> {
    if let Some(events) = normalize_codex_non_jsonrpc_message(session_id, message) {
        return events;
    }
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
        "turn/started" => turn_started(session_id, message),
        "thread/status/changed" => thread_status_changed(session_id, message),
        "thread/tokenUsage/updated" => vec![token_usage_updated(session_id, message)],
        "account/rateLimits/updated" => vec![rate_limits_updated(session_id, message)],
        "thread/compacted" => vec![NormalizedEvent::NativeNotification(native_notification(
            session_id,
            message,
            "thread/compacted",
            "compaction",
            "Context compacted",
            Some("completed"),
        ))],
        "turn/diff/updated" => vec![NormalizedEvent::TurnDiffUpdated(native_notification(
            session_id,
            message,
            "turn/diff/updated",
            native_notification_category("turn/diff/updated"),
            native_notification_title("turn/diff/updated"),
            None,
        ))],
        "item/reasoning/summaryTextDelta"
        | "item/reasoning/summaryPartAdded"
        | "item/reasoning/textDelta" => {
            vec![NormalizedEvent::ItemReasoning(native_notification(
                session_id,
                message,
                method,
                native_notification_category(method),
                native_notification_title(method),
                None,
            ))]
        }
        "serverRequest/resolved" => {
            vec![NormalizedEvent::ServerRequestResolved(native_notification(
                session_id,
                message,
                "serverRequest/resolved",
                native_notification_category("serverRequest/resolved"),
                native_notification_title("serverRequest/resolved"),
                None,
            ))]
        }
        "item/mcpToolCall/progress" => {
            vec![NormalizedEvent::McpToolCallProgress(native_notification(
                session_id,
                message,
                "item/mcpToolCall/progress",
                native_notification_category("item/mcpToolCall/progress"),
                native_notification_title("item/mcpToolCall/progress"),
                None,
            ))]
        }
        method if method.starts_with("mcpServer/") => {
            vec![NormalizedEvent::McpToolCallProgress(native_notification(
                session_id,
                message,
                method,
                native_notification_category(method),
                native_notification_title(method),
                None,
            ))]
        }
        "turn/plan/updated" => turn_plan_updated(session_id, message).into_iter().collect(),
        "item/plan/delta" => plan_delta(session_id, message).into_iter().collect(),
        "item/commandExecution/outputDelta" | "command/exec/outputDelta" => {
            command_output_delta(session_id, message)
                .into_iter()
                .collect()
        }
        "item/fileChange/outputDelta" | "item/fileChange/patchUpdated" => {
            vec![NormalizedEvent::NativeNotification(native_notification(
                session_id,
                message,
                method,
                "file",
                native_notification_title(method),
                Some("updated"),
            ))]
        }
        method if method.starts_with("thread/realtime/") => {
            vec![NormalizedEvent::ThreadRealtimeEvent(native_notification(
                session_id,
                message,
                method,
                native_notification_category(method),
                native_notification_title(method),
                Some("updated"),
            ))]
        }
        "item/started" => item_started(session_id, message).into_iter().collect(),
        "item/completed" => item_completed(session_id, message).into_iter().collect(),
        "turn/completed" => vec![
            NormalizedEvent::AgentMessageCompleted(MessageCompletedEvent {
                session_id,
                message_id: string_at(message, &["/params/turnId", "/params/id"])
                    .unwrap_or("codex-turn")
                    .to_owned(),
                content: None,
                provider_payload: Some(message.clone()),
            }),
            session_status_event(
                session_id,
                SessionStatus::Idle,
                Some("Codex turn completed.".to_owned()),
            ),
        ],
        "error" => vec![NormalizedEvent::Error(AgentErrorEvent {
            session_id: Some(session_id),
            code: string_at(message, &["/params/code"]).map(str::to_owned),
            message: string_at(message, &["/params/message"])
                .unwrap_or("Codex reported an error")
                .to_owned(),
            provider_payload: Some(message.clone()),
        })],
        _ => vec![NormalizedEvent::NativeNotification(native_notification(
            session_id,
            message,
            method,
            native_notification_category(method),
            native_notification_title(method),
            None,
        ))],
    }
}

pub fn normalize_codex_approval_request(
    session_id: SessionId,
    message: &Value,
    cache: Option<&CodexApprovalItemCache>,
) -> Option<(ApprovalId, Value, CodexApprovalKind, NormalizedEvent)> {
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
        // Older Codex server request aliases.
        "execCommandApproval" => (
            ApprovalKind::Command,
            CodexApprovalKind::ExecCommandApproval,
        ),
        "applyPatchApproval" => (
            ApprovalKind::FileChange,
            CodexApprovalKind::ApplyPatchApproval,
        ),
        _ => return None,
    };
    let native_request_id = message.get("id")?.clone();
    let approval_id = ApprovalId::new();
    let title = match approval_kind {
        CodexApprovalKind::ExecCommandApproval => "Approve Codex command",
        CodexApprovalKind::ApplyPatchApproval => "Approve Codex file change",
        _ => match kind {
            ApprovalKind::Command => "Approve Codex command",
            ApprovalKind::FileChange => "Approve Codex file change",
            ApprovalKind::ProviderSpecific | ApprovalKind::Tool => "Approve Codex permission",
        },
    };

    let params = message.get("params").unwrap_or(&Value::Null);
    let (presentation, details) = match approval_kind {
        CodexApprovalKind::ExecCommandApproval => synthesize_codex_exec_approval_details(params),
        CodexApprovalKind::ApplyPatchApproval => synthesize_codex_patch_approval_details(params),
        _ => synthesize_codex_approval_details(&kind, params, cache, message),
    };
    let event = NormalizedEvent::ApprovalRequested(ApprovalRequestEvent {
        session_id,
        approval_id,
        kind,
        title: title.to_string(),
        details: details.clone(),
        expires_at: None,
        presentation,
        resolution_state: None,
        resolving_decision: None,
        status: None,
        turn_id: None,
        item_id: None,
        options: agenter_core::ApprovalOption::canonical_defaults(),
        risk: None,
        subject: details.clone(),
        native_request_id: Some(native_request_id.to_string()),
        native_blocking: true,
        policy: None,
        provider_payload: Some(message.clone()),
    });
    Some((approval_id, native_request_id, approval_kind, event))
}

fn synthesize_codex_approval_details(
    kind: &ApprovalKind,
    params: &Value,
    cache: Option<&CodexApprovalItemCache>,
    raw_message: &Value,
) -> (Option<Value>, Option<String>) {
    match kind {
        ApprovalKind::FileChange => {
            let presentation = cache.and_then(|c| c.presentation_for_file_change_approval(params));

            let body = presentation
                .as_ref()
                .and_then(|p| p.get("paths").and_then(Value::as_array))
                .filter(|paths| !paths.is_empty())
                .map(|paths| {
                    let bullets = paths
                        .iter()
                        .filter_map(Value::as_str)
                        .map(|path| format!("• {path}"))
                        .collect::<Vec<_>>()
                        .join("\n");
                    format!("Files:\n{bullets}")
                })
                .or_else(|| codex_fallback_item_hint(params));

            let details = join_sparse_prelude_then_body(params, body)
                .or_else(|| Some("Codex proposes file edits.".to_owned()));
            (presentation, details)
        }
        ApprovalKind::Command => {
            let presentation = presentation_for_command_execution_approval(params);
            let details = string_at(
                raw_message,
                &[
                    "/params/command",
                    "/params/item/command",
                    "/params/description",
                    "/params/reason",
                ],
            )
            .map(str::to_owned)
            .or_else(|| Some("Approve this shell command.".to_owned()));
            (presentation, details)
        }
        ApprovalKind::ProviderSpecific | ApprovalKind::Tool => {
            let extracted = [
                "/params/description",
                "/params/reason",
                "/params/request/summary",
            ]
            .iter()
            .find_map(|p| {
                raw_message
                    .pointer(p)
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            });

            (None, extracted.or_else(|| codex_fallback_item_hint(params)))
        }
    }
}

fn synthesize_codex_exec_approval_details(params: &Value) -> (Option<Value>, Option<String>) {
    let argv = params
        .get("command")
        .and_then(Value::as_array)
        .map(|parts| parts.iter().filter_map(Value::as_str).collect::<Vec<_>>())
        .unwrap_or_default();
    let cmdline = argv.join(" ");
    let cwd = params
        .pointer("/cwd")
        .and_then(|v| v.as_str().map(str::to_owned))
        .or_else(|| {
            params
                .get("cwd")
                .and_then(|v| v.as_str().map(str::to_owned))
        });
    let reason = params
        .pointer("/reason")
        .and_then(|v| v.as_str())
        .or_else(|| params.get("reason").and_then(Value::as_str));
    let body = Some(cmdline.trim().to_owned())
        .filter(|c| !c.is_empty())
        .map(|cmdline| {
            let mut blob = cmdline.to_owned();
            if let Some(cwd_line) = &cwd {
                blob.push('\n');
                blob.push_str("cwd: ");
                blob.push_str(cwd_line);
            }
            if let Some(reason) = reason.filter(|t| !t.is_empty()) {
                blob.push_str("\n\n");
                blob.push_str(reason);
            }
            blob
        });
    (
        None,
        body.or_else(|| Some("Approve this shell command.".to_owned())),
    )
}

fn synthesize_codex_patch_approval_details(params: &Value) -> (Option<Value>, Option<String>) {
    let mut paths: Vec<String> = params
        .pointer("/fileChanges")
        .and_then(Value::as_object)
        .or_else(|| params.get("fileChanges").and_then(Value::as_object))
        .into_iter()
        .flat_map(|paths| paths.keys().cloned())
        .collect();
    paths.sort();
    paths.dedup();
    let bullets = paths
        .into_iter()
        .map(|p| format!("• {p}"))
        .collect::<Vec<_>>();
    let head = (!bullets.is_empty()).then(|| format!("Paths:\n{}", bullets.join("\n")));
    let reason = params
        .pointer("/reason")
        .and_then(Value::as_str)
        .or_else(|| params.get("reason").and_then(Value::as_str));
    let tail = reason
        .filter(|t| !t.is_empty())
        .map(|r| format!("Reason: {r}"));
    let details = match (head, tail) {
        (Some(h), Some(t)) => Some(format!("{h}\n\n{t}")),
        (Some(h), None) => Some(h),
        (None, Some(t)) => Some(t),
        (None, None) => Some("Codex proposes file edits.".to_owned()),
    };
    (None, details)
}

fn codex_fallback_item_hint(params: &Value) -> Option<String> {
    let item_id = string_at(
        params,
        &[
            "/itemId",
            "/item/id",
            "/item_id",
            "/approvalId",
            "/approval_id",
        ],
    )?;
    Some(format!(
        "Codex is waiting on approval linked to `{item_id}`."
    ))
}

fn join_sparse_prelude_then_body(params: &Value, tail: Option<String>) -> Option<String> {
    let head = sparse_file_change_fallback_details(params);
    match (head, tail) {
        (Some(h), Some(t)) => Some(format!("{h}\n\n{t}")),
        (Some(h), None) => Some(h),
        (None, Some(t)) => Some(t),
        (None, None) => None,
    }
}

pub fn normalize_codex_question_request(
    session_id: SessionId,
    message: &Value,
) -> Option<(QuestionId, Value, NormalizedEvent)> {
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
    let event = NormalizedEvent::QuestionRequested(QuestionRequestedEvent {
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

fn unsupported_codex_server_request(message: &Value) -> Option<(Value, &str)> {
    if !codex_rpc_is_codex_server_to_client_request(message) {
        return None;
    }
    let native_request_id = message.get("id")?.clone();
    let method = jsonrpc_method(message)?;
    Some((native_request_id, method))
}

fn unsupported_request_response(native_request_id: Value, method: &str) -> Value {
    json!({
        "id": native_request_id,
        "error": {
            "code": -32601,
            "message": format!("unsupported Codex server request method: {method}")
        }
    })
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

    if matches!(
        approval_kind,
        CodexApprovalKind::ExecCommandApproval | CodexApprovalKind::ApplyPatchApproval
    ) {
        return json!({ "decision": codex_review_decision_wire(&decision) });
    }

    match decision {
        ApprovalDecision::Accept => json!({"decision": "accept"}),
        ApprovalDecision::AcceptForSession => json!({"decision": "acceptForSession"}),
        ApprovalDecision::Decline => json!({"decision": "decline"}),
        ApprovalDecision::Cancel => json!({"decision": "cancel"}),
        ApprovalDecision::ProviderSpecific { payload } => payload,
    }
}

fn codex_review_decision_wire(decision: &ApprovalDecision) -> &'static str {
    match decision {
        ApprovalDecision::Accept => "approved",
        ApprovalDecision::AcceptForSession => "approved_for_session",
        ApprovalDecision::Decline => "denied",
        ApprovalDecision::Cancel => "abort",
        ApprovalDecision::ProviderSpecific { .. } => "denied",
    }
}

fn text_delta(session_id: SessionId, message: &Value) -> Option<NormalizedEvent> {
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
    Some(NormalizedEvent::AgentMessageDelta(AgentMessageDeltaEvent {
        session_id,
        message_id: message_id(message),
        delta: delta.to_owned(),
        provider_payload: Some(message.clone()),
    }))
}

fn message_completed(session_id: SessionId, message: &Value) -> Option<NormalizedEvent> {
    Some(NormalizedEvent::AgentMessageCompleted(
        MessageCompletedEvent {
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
        },
    ))
}

fn turn_plan_updated(session_id: SessionId, message: &Value) -> Option<NormalizedEvent> {
    let entries = message
        .pointer("/params/plan")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let label = string_at(entry, &["/step", "/label", "/content", "/text"])?;
            Some(PlanEntry {
                label: label.to_owned(),
                status: match string_at(entry, &["/status"]) {
                    Some("completed") => PlanEntryStatus::Completed,
                    Some("inProgress" | "in_progress") => PlanEntryStatus::InProgress,
                    Some("failed" | "error") => PlanEntryStatus::Failed,
                    Some("cancelled" | "canceled") => PlanEntryStatus::Cancelled,
                    _ => PlanEntryStatus::Pending,
                },
            })
        })
        .collect::<Vec<_>>();
    if entries.is_empty() && string_at(message, &["/params/explanation"]).is_none() {
        return None;
    }
    Some(NormalizedEvent::PlanUpdated(PlanEvent {
        session_id,
        plan_id: string_at(message, &["/params/turnId", "/params/id"]).map(str::to_owned),
        title: Some("Implementation plan".to_owned()),
        content: string_at(message, &["/params/explanation"]).map(str::to_owned),
        entries,
        append: false,
        provider_payload: Some(message.clone()),
    }))
}

fn plan_delta(session_id: SessionId, message: &Value) -> Option<NormalizedEvent> {
    let content = string_at(
        message,
        &["/params/delta", "/params/text", "/params/content"],
    )?;
    Some(NormalizedEvent::PlanUpdated(PlanEvent {
        session_id,
        plan_id: string_at(
            message,
            &[
                "/params/turnId",
                "/params/item/turnId",
                "/params/itemId",
                "/params/item/id",
                "/params/id",
            ],
        )
        .map(str::to_owned),
        title: Some("Implementation plan".to_owned()),
        content: Some(content.to_owned()),
        entries: Vec::new(),
        append: true,
        provider_payload: Some(message.clone()),
    }))
}

fn command_output_delta(session_id: SessionId, message: &Value) -> Option<NormalizedEvent> {
    let delta = string_at(
        message,
        &["/params/delta", "/params/text", "/params/output"],
    )?;
    Some(NormalizedEvent::CommandOutputDelta(CommandOutputEvent {
        session_id,
        command_id: item_id(message),
        stream: match string_at(message, &["/params/stream"]) {
            Some("stderr") => CommandOutputStream::Stderr,
            _ => CommandOutputStream::Stdout,
        },
        delta: delta.to_owned(),
        provider_payload: Some(message.clone()),
    }))
}

fn normalize_codex_non_jsonrpc_message(
    session_id: SessionId,
    message: &Value,
) -> Option<Vec<NormalizedEvent>> {
    if message.get("method").is_some() {
        return None;
    }
    if message.get("command_id").is_some() && message.get("raw_input").is_some() {
        return Some(vec![codex_slash_command_result(session_id, message)]);
    }
    if item_type(message) == Some("contextCompaction") {
        return Some(vec![codex_context_compaction_event(session_id, message)]);
    }
    None
}

fn codex_slash_command_result(session_id: SessionId, message: &Value) -> NormalizedEvent {
    let accepted = bool_at(message, &["/accepted"]).unwrap_or(false);
    NormalizedEvent::NativeNotification(NativeNotification {
        session_id,
        provider_id: AgentProviderId::from(AgentProviderId::CODEX),
        event_id: string_at(message, &["/command_id"]).map(str::to_owned),
        method: "codex/slash_command_result".to_owned(),
        category: "slash_command".to_owned(),
        title: string_at(message, &["/raw_input", "/command_id"])
            .unwrap_or("Provider command")
            .to_owned(),
        detail: string_at(message, &["/message"]).map(str::to_owned),
        status: Some(if accepted { "accepted" } else { "rejected" }.to_owned()),
        provider_payload: Some(message.clone()),
    })
}

fn codex_context_compaction_event(session_id: SessionId, message: &Value) -> NormalizedEvent {
    NormalizedEvent::NativeNotification(NativeNotification {
        session_id,
        provider_id: AgentProviderId::from(AgentProviderId::CODEX),
        event_id: string_at(message, &["/id", "/itemId", "/item/id"]).map(str::to_owned),
        method: "item/contextCompaction".to_owned(),
        category: "compaction".to_owned(),
        title: "Context compacted".to_owned(),
        detail: None,
        status: Some("completed".to_owned()),
        provider_payload: Some(message.clone()),
    })
}

fn thread_status_changed(session_id: SessionId, message: &Value) -> Vec<NormalizedEvent> {
    let provider_status = string_at(message, &["/params/status/type"])
        .or_else(|| string_at(message, &["/params/status"]))
        .unwrap_or("updated");
    let mut events = Vec::new();
    match provider_status {
        "active" => events.push(session_status_event(
            session_id,
            SessionStatus::Running,
            Some("Codex thread is active.".to_owned()),
        )),
        "idle" => events.push(session_status_event(
            session_id,
            SessionStatus::Idle,
            Some("Codex thread is idle.".to_owned()),
        )),
        _ => {}
    }
    events.push(NormalizedEvent::NativeNotification(NativeNotification {
        session_id,
        provider_id: AgentProviderId::from(AgentProviderId::CODEX),
        event_id: string_at(message, &["/params/threadId"]).map(str::to_owned),
        method: "thread/status/changed".to_owned(),
        category: "thread".to_owned(),
        title: "Thread status changed".to_owned(),
        detail: Some(format!("status: {provider_status}")),
        status: Some(provider_status.to_owned()),
        provider_payload: Some(message.clone()),
    }));
    events
}

fn turn_started(session_id: SessionId, message: &Value) -> Vec<NormalizedEvent> {
    let status = string_at(message, &["/params/turn/status"]).unwrap_or("running");
    let mut events = Vec::new();
    if matches!(status, "inProgress" | "running") {
        events.push(session_status_event(
            session_id,
            SessionStatus::Running,
            Some("Codex turn started.".to_owned()),
        ));
    }
    events.push(NormalizedEvent::NativeNotification(NativeNotification {
        session_id,
        provider_id: AgentProviderId::from(AgentProviderId::CODEX),
        event_id: string_at(message, &["/params/turn/id", "/params/turnId"]).map(str::to_owned),
        method: "turn/started".to_owned(),
        category: "turn".to_owned(),
        title: "Turn started".to_owned(),
        detail: None,
        status: Some(status.to_owned()),
        provider_payload: Some(message.clone()),
    }));
    events
}

fn token_usage_updated(session_id: SessionId, message: &Value) -> NormalizedEvent {
    let last = integer_at(message, &["/params/tokenUsage/last/totalTokens"]);
    let total = integer_at(message, &["/params/tokenUsage/total/totalTokens"]);
    let window = integer_at(message, &["/params/tokenUsage/modelContextWindow"]);
    NormalizedEvent::NativeNotification(NativeNotification {
        session_id,
        provider_id: AgentProviderId::from(AgentProviderId::CODEX),
        event_id: string_at(message, &["/params/turnId", "/params/threadId"]).map(str::to_owned),
        method: "thread/tokenUsage/updated".to_owned(),
        category: "token_usage".to_owned(),
        title: "Token usage updated".to_owned(),
        detail: Some(
            [
                last.map(|value| format!("last {value}")),
                total.map(|value| format!("total {value}")),
                window.map(|value| format!("window {value}")),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · "),
        )
        .filter(|detail| !detail.is_empty()),
        status: Some("updated".to_owned()),
        provider_payload: Some(message.clone()),
    })
}

fn rate_limits_updated(session_id: SessionId, message: &Value) -> NormalizedEvent {
    let plan = string_at(message, &["/params/rateLimits/planType"]);
    let primary = integer_at(message, &["/params/rateLimits/primary/usedPercent"]);
    let secondary = integer_at(message, &["/params/rateLimits/secondary/usedPercent"]);
    let reached = string_at(message, &["/params/rateLimits/rateLimitReachedType"]);
    NormalizedEvent::NativeNotification(NativeNotification {
        session_id,
        provider_id: AgentProviderId::from(AgentProviderId::CODEX),
        event_id: string_at(message, &["/params/rateLimits/limitId"]).map(str::to_owned),
        method: "account/rateLimits/updated".to_owned(),
        category: "rate_limits".to_owned(),
        title: "Rate limits updated".to_owned(),
        detail: Some(
            [
                plan.map(str::to_owned),
                primary.map(|value| format!("primary {value}%")),
                secondary.map(|value| format!("secondary {value}%")),
                reached.map(|value| format!("reached {value}")),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · "),
        )
        .filter(|detail| !detail.is_empty()),
        status: Some("updated".to_owned()),
        provider_payload: Some(message.clone()),
    })
}

fn session_status_event(
    session_id: SessionId,
    status: SessionStatus,
    reason: Option<String>,
) -> NormalizedEvent {
    NormalizedEvent::SessionStatusChanged(SessionStatusChangedEvent {
        session_id,
        status,
        reason,
    })
}

fn native_notification(
    session_id: SessionId,
    message: &Value,
    method: &str,
    category: impl Into<String>,
    title: impl Into<String>,
    status: Option<&str>,
) -> NativeNotification {
    NativeNotification {
        session_id,
        provider_id: AgentProviderId::from(AgentProviderId::CODEX),
        method: method.to_owned(),
        event_id: string_at(
            message,
            &[
                "/params/item/id",
                "/params/itemId",
                "/params/id",
                "/params/turnId",
                "/params/turn/id",
                "/params/threadId",
            ],
        )
        .map(str::to_owned),
        category: category.into(),
        title: title.into(),
        detail: native_notification_detail(message),
        status: status.map(str::to_owned),
        provider_payload: Some(message.clone()),
    }
}

fn item_started(session_id: SessionId, message: &Value) -> Option<NormalizedEvent> {
    if should_ignore_item_event(message) {
        return None;
    }

    if let Some(command) = string_at(message, &["/params/command", "/params/item/command"]) {
        return Some(NormalizedEvent::CommandStarted(CommandEvent {
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

    Some(NormalizedEvent::ToolStarted(agenter_core::ToolEvent {
        session_id,
        tool_call_id: item_id(message),
        name: string_at(message, &["/params/name", "/params/item/name"])
            .or_else(|| item_type(message))
            .unwrap_or("tool")
            .to_owned(),
        title: string_at(message, &["/params/title", "/params/item/title"]).map(str::to_owned),
        input: message.pointer("/params/input").cloned(),
        output: None,
        provider_payload: Some(message.clone()),
    }))
}

fn item_completed(session_id: SessionId, message: &Value) -> Option<NormalizedEvent> {
    if item_type(message) == Some("agentMessage") {
        return message_completed(session_id, message);
    }
    if item_type(message) == Some("contextCompaction") {
        return Some(NormalizedEvent::NativeNotification(native_notification(
            session_id,
            message,
            "item/contextCompaction",
            "compaction",
            "Context compacted",
            Some("completed"),
        )));
    }
    if should_ignore_item_event(message) {
        return None;
    }

    if string_at(message, &["/params/command", "/params/item/command"]).is_some() {
        return Some(NormalizedEvent::CommandCompleted(CommandCompletedEvent {
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
        return Some(NormalizedEvent::FileChangeProposed(FileChangeEvent {
            session_id,
            path: path.to_owned(),
            change_kind: file_change_kind(message),
            diff: string_at(message, &["/params/diff", "/params/item/diff"]).map(str::to_owned),
            provider_payload: Some(message.clone()),
        }));
    }

    Some(NormalizedEvent::ToolCompleted(agenter_core::ToolEvent {
        session_id,
        tool_call_id: item_id(message),
        name: string_at(message, &["/params/name", "/params/item/name"])
            .or_else(|| item_type(message))
            .unwrap_or("tool")
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

/// Inbound JSON-RPC from Codex with `method`, `id`, and no top-level `result` / `error`.
fn codex_rpc_is_codex_server_to_client_request(message: &Value) -> bool {
    jsonrpc_method(message).is_some()
        && message.get("id").is_some()
        && message.get("result").is_none()
        && message.get("error").is_none()
}

/// Inbound JSON-RPC from Codex with `id` and top-level `result` or `error`.
fn codex_rpc_is_codex_server_to_client_response(message: &Value) -> bool {
    message.get("id").is_some()
        && (message.get("result").is_some() || message.get("error").is_some())
}

fn codex_jsonrpc_request_id_summary(message: &Value) -> String {
    match message.get("id") {
        Some(id) if id.is_string() => id.as_str().unwrap_or("<non-scalar id>").to_owned(),
        Some(id) if id.is_number() => id.to_string(),
        Some(id) => id.to_string(),
        None => "<missing id>".to_owned(),
    }
}

fn codex_jsonrpc_request_ids_equal(id: Option<&Value>, expected: &CodexRequestId) -> bool {
    let Some(id) = id else {
        return false;
    };
    match id {
        Value::Number(n) => match expected {
            CodexRequestId::Integer(value) => n.as_i64() == Some(*value),
            CodexRequestId::String(value) => n.to_string() == *value,
        },
        Value::String(s) => match expected {
            CodexRequestId::Integer(value) => {
                s.trim().parse::<i64>().ok() == Some(*value) || s.trim() == value.to_string()
            }
            CodexRequestId::String(value) => s.trim() == value,
        },
        _ => false,
    }
}

fn codex_rpc_is_response_matching_request(
    message: &Value,
    pending_client_request_id: &CodexRequestId,
) -> bool {
    codex_jsonrpc_request_ids_equal(message.get("id"), pending_client_request_id)
        && (message.get("result").is_some() || message.get("error").is_some())
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
                updated_at: codex_thread_updated_at(thread),
            })
        })
        .collect()
}

fn codex_thread_updated_at(value: &Value) -> Option<String> {
    string_at(
        value,
        &[
            "/updatedAt",
            "/updated_at",
            "/lastActivityAt",
            "/last_activity_at",
            "/activityAt",
            "/activity_at",
            "/timestamp",
            "/createdAt",
            "/created_at",
            "/threadUpdatedAt",
            "/thread_updated_at",
        ],
    )
    .map(str::to_owned)
    .or_else(|| {
        integer_at(
            value,
            &[
                "/updatedAt",
                "/updated_at",
                "/lastActivityAt",
                "/last_activity_at",
                "/timestamp",
                "/createdAt",
                "/created_at",
                "/threadUpdatedAt",
                "/thread_updated_at",
            ],
        )
        .map(|value| value.to_string())
    })
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

fn codex_history_message_has_fallback_content(value: &Value) -> bool {
    match value.get("content") {
        Some(Value::Array(parts)) => !parts.is_empty(),
        Some(Value::Object(_)) => true,
        _ => false,
    }
}

fn codex_history_plan_text_content(value: &Value) -> Option<String> {
    codex_text_content(value).or_else(|| {
        string_at(value, &["/text"])
            .map(str::to_owned)
            .filter(|s| !s.is_empty())
    })
}

fn codex_history_command_line(value: &Value) -> Option<String> {
    string_at(
        value,
        &[
            "/command",
            "/cmdLine",
            "/cmd",
            "/executable",
            "/shellCommand",
        ],
    )
    .map(str::to_owned)
    .or_else(|| {
        value
            .pointer("/argv")
            .and_then(Value::as_array)
            .map(|parts| {
                parts
                    .iter()
                    .filter_map(|part| match part {
                        Value::String(s) => Some(s.as_str()),
                        _ => part.as_str(),
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            })
    })
    .filter(|line| !line.is_empty())
}

fn codex_history_thread_item_native_notification(
    value: &Value,
    wire_type: &str,
) -> DiscoveredSessionHistoryItem {
    let detail = string_at(value, &["/summary", "/prompt", "/message"]).map(str::to_owned);
    DiscoveredSessionHistoryItem::NativeNotification {
        event_id: string_at(value, &["/id", "/messageId"]).map(str::to_owned),
        category: "codex_thread_item".to_owned(),
        title: wire_type.to_owned(),
        detail,
        status: string_at(value, &["/status"]).map(str::to_owned),
        provider_payload: Some(value.clone()),
    }
}

fn collect_codex_history_item(value: &Value, items: &mut Vec<DiscoveredSessionHistoryItem>) {
    let Some(wire_type) = value.get("type").and_then(Value::as_str) else {
        return;
    };

    match wire_type {
        "userMessage" => {
            if let Some(content) = codex_text_content(value) {
                items.push(DiscoveredSessionHistoryItem::UserMessage {
                    message_id: string_at(value, &["/id", "/messageId"]).map(str::to_owned),
                    content,
                });
            } else if codex_history_message_has_fallback_content(value) {
                items.push(codex_history_thread_item_native_notification(
                    value, wire_type,
                ));
            }
        }
        "agentMessage" => {
            if let Some(content) = codex_text_content(value) {
                items.push(DiscoveredSessionHistoryItem::AgentMessage {
                    message_id: string_at(value, &["/id", "/messageId"])
                        .unwrap_or("codex-agent-message")
                        .to_owned(),
                    content,
                });
            } else if codex_history_message_has_fallback_content(value) {
                items.push(codex_history_thread_item_native_notification(
                    value, wire_type,
                ));
            }
        }
        "commandExecution" => {
            if let Some(command) = codex_history_command_line(value) {
                let status = string_at(value, &["/status"]);
                let exit_code = integer_at(value, &["/exitCode"]).map(|code| code as i32);
                let success = exit_code
                    .map(|code| code == 0)
                    .unwrap_or(status != Some("failed"));
                items.push(DiscoveredSessionHistoryItem::Command {
                    command_id: string_at(value, &["/id"])
                        .unwrap_or("codex-command")
                        .to_owned(),
                    command,
                    cwd: string_at(value, &["/cwd"]).map(str::to_owned),
                    source: string_at(value, &["/source"]).map(str::to_owned),
                    process_id: string_at(value, &["/processId"]).map(str::to_owned),
                    duration_ms: integer_at(value, &["/durationMs"])
                        .and_then(|dur| dur.try_into().ok()),
                    actions: codex_history_command_actions(value),
                    output: string_at(value, &["/aggregatedOutput"]).map(str::to_owned),
                    exit_code,
                    success,
                    provider_payload: Some(value.clone()),
                });
            } else {
                items.push(codex_history_thread_item_native_notification(
                    value, wire_type,
                ));
            }
        }
        "fileChange" => {
            let mut pushed = false;
            if let Some(changes) = value.get("changes").and_then(Value::as_array) {
                for (index, change) in changes.iter().enumerate() {
                    let Some(path) = string_at(change, &["/path"]) else {
                        continue;
                    };
                    pushed = true;
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
            if !pushed {
                items.push(codex_history_thread_item_native_notification(
                    value, wire_type,
                ));
            }
        }
        "collabAgentToolCall" => {
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
        "mcpToolCall" => {
            let status = codex_history_tool_status(value);
            let server_name = value
                .get("serverInfo")
                .and_then(|info| string_at(info, &["/name"]));
            let name = string_at(value, &["/tool", "/name"])
                .or(server_name)
                .unwrap_or("mcp_tool");
            items.push(DiscoveredSessionHistoryItem::Tool {
                tool_call_id: string_at(value, &["/id"])
                    .unwrap_or("codex-mcp-tool")
                    .to_owned(),
                name: name.to_owned(),
                title: Some(name.to_owned()),
                status,
                input: Some(value.clone()),
                output: value
                    .get("output")
                    .cloned()
                    .or_else(|| value.get("result").cloned()),
                provider_payload: Some(value.clone()),
            });
        }
        "plan" => {
            if let Some(content) = codex_history_plan_text_content(value) {
                items.push(DiscoveredSessionHistoryItem::Plan {
                    plan_id: string_at(value, &["/id"])
                        .unwrap_or("codex-plan")
                        .to_owned(),
                    title: Some("Implementation plan".to_owned()),
                    content,
                    provider_payload: Some(value.clone()),
                });
            } else {
                items.push(codex_history_thread_item_native_notification(
                    value, wire_type,
                ));
            }
        }
        "contextCompaction" => {
            items.push(DiscoveredSessionHistoryItem::NativeNotification {
                event_id: string_at(value, &["/id"]).map(str::to_owned),
                category: "compaction".to_owned(),
                title: "Context compacted".to_owned(),
                detail: None,
                status: Some("completed".to_owned()),
                provider_payload: Some(value.clone()),
            });
        }
        _ => {
            items.push(codex_history_thread_item_native_notification(
                value, wire_type,
            ));
        }
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

pub fn is_codex_thread_not_found_error(error: &(dyn std::error::Error + 'static)) -> bool {
    let message = error.to_string();
    message.contains("codex turn/start failed") && message.contains("thread not found")
}

pub fn is_codex_no_rollout_resume_error(error: &(dyn std::error::Error + 'static)) -> bool {
    let message = error.to_string();
    message.contains("codex thread/resume failed") && message.contains("no rollout found")
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
    string_at(message, &["/params/item/type", "/params/type", "/type"])
}

fn should_ignore_item_event(message: &Value) -> bool {
    matches!(item_type(message), Some("userMessage" | "reasoning"))
}

fn native_notification_category(method: &str) -> &'static str {
    match method {
        "error" | "warning" | "guardianWarning" | "deprecationNotice" | "configWarning" => {
            "warning"
        }
        method if method.starts_with("thread/realtime/") => "realtime",
        method if method.starts_with("thread/") => "thread",
        method if method.starts_with("turn/") => "turn",
        method if method.starts_with("item/reasoning/") => "reasoning",
        method if method.starts_with("item/mcpToolCall/") || method.starts_with("mcpServer/") => {
            "mcp"
        }
        method if method.starts_with("model/") => "model",
        method if method.starts_with("hook/") => "hook",
        method if method.starts_with("account/") => "account",
        method if method.starts_with("fuzzyFileSearch/") => "search",
        method if method.starts_with("windows") => "environment",
        _ => "provider",
    }
}

fn native_notification_title(method: &str) -> String {
    match method {
        "model/rerouted" => "Model rerouted".to_owned(),
        "model/verification" => "Model verification".to_owned(),
        "warning" => "Warning".to_owned(),
        "guardianWarning" => "Guardian warning".to_owned(),
        "deprecationNotice" => "Deprecation notice".to_owned(),
        "configWarning" => "Configuration warning".to_owned(),
        "item/fileChange/outputDelta" => "File change output".to_owned(),
        "item/fileChange/patchUpdated" => "File patch updated".to_owned(),
        _ => method
            .split('/')
            .filter(|part| !part.is_empty())
            .map(|part| {
                let mut chars = part.chars();
                match chars.next() {
                    Some(first) => {
                        let mut word = first.to_uppercase().collect::<String>();
                        word.push_str(chars.as_str());
                        word
                    }
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

fn native_notification_detail(message: &Value) -> Option<String> {
    string_at(
        message,
        &[
            "/params/message",
            "/params/detail",
            "/params/statusMessage",
            "/params/error/message",
            "/params/reason",
        ],
    )
    .map(str::to_owned)
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
        let NormalizedEvent::AgentMessageDelta(delta) = &events[0] else {
            panic!("expected message delta");
        };
        assert_eq!(delta.message_id, "msg-1");
        assert_eq!(delta.delta, "hello");
    }

    #[test]
    fn codex_semantic_reducer_emits_universal_projection() {
        let session_id = SessionId::nil();
        let message = json!({
            "method": "agentMessage/delta",
            "params": {
                "messageId": "msg-1",
                "delta": "hello"
            }
        });

        let normalized = normalize_codex_message(session_id, &message);
        let semantic = super::codex_reducer::reduce_native_message(session_id, &message);

        assert_eq!(semantic.len(), normalized.len());
        assert_eq!(semantic[0].universal.session_id, Some(session_id));
        let native = semantic[0].universal.native.as_ref().expect("native ref");
        assert_eq!(native.protocol, "codex-app-server");
        assert_eq!(native.method.as_deref(), Some("agentMessage/delta"));
        assert_eq!(native.kind.as_deref(), Some(AgentProviderId::CODEX));
        assert_eq!(native.native_id.as_deref(), Some("msg-1"));
        assert_eq!(native.summary.as_deref(), Some("assistant message delta"));
        let agenter_core::UniversalEventKind::ContentDelta {
            block_id,
            kind,
            delta,
        } = &semantic[0].universal.event
        else {
            panic!("expected universal content delta");
        };
        assert_eq!(block_id, "text-msg-1");
        assert_eq!(kind, &Some(agenter_core::ContentBlockKind::Text));
        assert_eq!(delta, "hello");
    }

    #[test]
    fn codex_stage6_golden_trace_maps_plan_approval_command_and_diff() {
        let session_id = SessionId::nil();
        let trace: Vec<Value> =
            serde_json::from_str(include_str!("../../tests/fixtures/codex_stage6_trace.json"))
                .expect("fixture parses");
        let semantic = trace
            .iter()
            .flat_map(|message| super::codex_reducer::reduce_native_message(session_id, message))
            .collect::<Vec<_>>();

        let turn = semantic
            .iter()
            .find_map(|event| match &event.universal.event {
                agenter_core::UniversalEventKind::TurnStarted { turn } => Some(turn),
                _ => None,
            })
            .expect("turn start");
        let turn_id = turn.turn_id;
        assert_eq!(turn.turn_id, turn_id);
        assert_eq!(turn.status, agenter_core::TurnStatus::Running);

        let plan = semantic
            .iter()
            .find_map(|event| match &event.universal.event {
                agenter_core::UniversalEventKind::PlanUpdated { plan } => Some(plan),
                _ => None,
            })
            .expect("plan");
        assert_eq!(plan.turn_id, Some(turn_id));
        assert_eq!(plan.entries[0].label, "Map events");
        assert_eq!(plan.entries[1].status, PlanEntryStatus::InProgress);

        let command_item = semantic
            .iter()
            .find_map(|event| match &event.universal.event {
                agenter_core::UniversalEventKind::ItemCreated { item }
                    if item.content[0].kind == agenter_core::ContentBlockKind::ToolCall =>
                {
                    Some(item)
                }
                _ => None,
            })
            .expect("command item");
        assert_eq!(command_item.turn_id, Some(turn_id));
        assert_eq!(command_item.content[0].block_id, "command-cmd-1");
        assert_eq!(command_item.content[0].text.as_deref(), Some("cargo test"));

        let output = semantic
            .iter()
            .find_map(|event| match &event.universal.event {
                agenter_core::UniversalEventKind::ContentDelta {
                    block_id,
                    kind,
                    delta,
                } => Some((block_id, kind, delta)),
                _ => None,
            })
            .expect("output delta");
        assert_eq!(output.0, "command-cmd-1-stdout");
        assert_eq!(
            output.1,
            &Some(agenter_core::ContentBlockKind::CommandOutput)
        );
        assert_eq!(output.2, "running tests\n");

        let status = semantic
            .iter()
            .find_map(|event| match &event.universal.event {
                agenter_core::UniversalEventKind::ContentCompleted {
                    block_id,
                    kind,
                    text,
                } => Some((block_id, kind, text)),
                _ => None,
            })
            .expect("command status");
        assert_eq!(status.0, "command-cmd-1-status");
        assert_eq!(
            status.1,
            &Some(agenter_core::ContentBlockKind::CommandOutput)
        );
        assert_eq!(status.2.as_deref(), Some("command completed"));

        assert!(semantic.iter().any(|event| matches!(
            &event.universal.event,
            agenter_core::UniversalEventKind::DiffUpdated { .. }
        )));
        assert!(!semantic.iter().any(|event| matches!(
            &event.universal.event,
            agenter_core::UniversalEventKind::ArtifactCreated { .. }
        )));

        let approval = json!({
            "id": "approval-1",
            "method": "item/commandExecution/requestApproval",
            "params": {
                "turnId": "turn-1",
                "itemId": "cmd-1",
                "command": "cargo test"
            }
        });
        let (_approval_id, _native_request_id, _kind, normalized) =
            normalize_codex_approval_request(session_id, &approval, None).expect("approval");
        let semantic = crate::agents::adapter::AdapterEvent::from_normalized_event(
            AgentProviderId::from(AgentProviderId::CODEX),
            "codex-app-server",
            Some("item/commandExecution/requestApproval"),
            normalized,
        );
        assert!(matches!(
            semantic.universal.event,
            agenter_core::UniversalEventKind::ApprovalRequested { .. }
        ));
    }

    #[test]
    fn codex_stage10_conformance_trace_preserves_expected_milestones() {
        let session_id = SessionId::nil();
        let trace: Vec<Value> = serde_json::from_str(include_str!(
            "../../tests/fixtures/codex_stage10_trace.json"
        ))
        .expect("fixture parses");
        let semantic = trace
            .iter()
            .flat_map(|message| super::codex_reducer::reduce_native_message(session_id, message))
            .collect::<Vec<_>>();

        let milestones = semantic
            .iter()
            .filter_map(|event| match &event.universal.event {
                agenter_core::UniversalEventKind::TurnStarted { .. } => Some("turn.started"),
                agenter_core::UniversalEventKind::PlanUpdated { .. } => Some("plan.updated"),
                agenter_core::UniversalEventKind::ItemCreated { item }
                    if item
                        .content
                        .iter()
                        .any(|block| block.kind == agenter_core::ContentBlockKind::ToolCall) =>
                {
                    Some("item.tool")
                }
                agenter_core::UniversalEventKind::ContentDelta {
                    kind: Some(agenter_core::ContentBlockKind::CommandOutput),
                    ..
                } => Some("content.command_delta"),
                agenter_core::UniversalEventKind::ContentCompleted {
                    kind: Some(agenter_core::ContentBlockKind::CommandOutput),
                    ..
                } => Some("content.command_completed"),
                agenter_core::UniversalEventKind::DiffUpdated { .. } => Some("diff.updated"),
                agenter_core::UniversalEventKind::TurnCompleted { .. } => Some("turn.completed"),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(
            milestones,
            vec![
                "turn.started",
                "plan.updated",
                "item.tool",
                "content.command_delta",
                "content.command_completed",
                "diff.updated",
                "turn.completed",
            ]
        );

        for approval in trace.iter().filter(|message| {
            matches!(
                jsonrpc_method(message),
                Some("item/commandExecution/requestApproval" | "item/fileChange/requestApproval")
            )
        }) {
            let (_approval_id, native_request_id, _kind, normalized) =
                normalize_codex_approval_request(session_id, approval, None)
                    .expect("approval should normalize");
            assert!(native_request_id.is_string());
            let semantic = crate::agents::adapter::AdapterEvent::from_normalized_event(
                AgentProviderId::from(AgentProviderId::CODEX),
                "codex-app-server",
                jsonrpc_method(approval),
                normalized,
            );
            let agenter_core::UniversalEventKind::ApprovalRequested { approval } =
                semantic.universal.event
            else {
                panic!("expected universal approval");
            };
            assert!(
                approval
                    .options
                    .iter()
                    .any(|option| option.option_id == "cancel_turn"),
                "all approval fixtures should expose terminal cancel semantics"
            );
        }
    }

    #[test]
    fn codex_stage6_unknown_native_event_stays_safe_native_unknown() {
        let session_id = SessionId::nil();
        let message = json!({
            "method": "turn/newFutureThing",
            "params": {
                "turnId": "turn-1",
                "secret": "must-not-be-copied"
            }
        });
        let semantic = super::codex_reducer::reduce_native_message(session_id, &message);

        assert_eq!(semantic.len(), 1);
        let agenter_core::UniversalEventKind::NativeUnknown { summary } =
            &semantic[0].universal.event
        else {
            panic!("expected unknown native event");
        };
        assert_eq!(summary.as_deref(), Some("native notification"));
        assert_eq!(
            semantic[0]
                .universal
                .native
                .as_ref()
                .and_then(|native| native.method.as_deref()),
            Some("turn/newFutureThing")
        );
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
        let NormalizedEvent::AgentMessageDelta(delta) = &events[0] else {
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
        let NormalizedEvent::AgentMessageCompleted(completed) = &events[0] else {
            panic!("expected message completed");
        };
        assert_eq!(completed.message_id, "msg-live-1");
        assert_eq!(completed.content.as_deref(), Some("Done."));
    }

    #[test]
    fn normalizes_codex_thread_compacted_as_native_notification() {
        let message = json!({
            "method": "thread/compacted",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1"
            }
        });

        let events = normalize_codex_message(SessionId::nil(), &message);

        assert_eq!(events.len(), 1);
        let NormalizedEvent::NativeNotification(event) = &events[0] else {
            panic!("expected native notification");
        };
        assert_eq!(event.provider_id.as_str(), AgentProviderId::CODEX);
        assert_eq!(event.category, "compaction");
        assert_eq!(event.title, "Context compacted");
        assert_eq!(event.status.as_deref(), Some("completed"));
    }

    #[test]
    fn normalizes_raw_codex_context_compaction_item_as_native_notification() {
        let message = json!({
            "id": "item-237",
            "type": "contextCompaction"
        });

        let events = normalize_codex_message(SessionId::nil(), &message);

        assert_eq!(events.len(), 1);
        let NormalizedEvent::NativeNotification(event) = &events[0] else {
            panic!("expected native notification");
        };
        assert_eq!(event.event_id.as_deref(), Some("item-237"));
        assert_eq!(event.category, "compaction");
        assert_eq!(event.title, "Context compacted");
        assert_eq!(event.status.as_deref(), Some("completed"));
        assert_eq!(event.provider_payload.as_ref(), Some(&message));
    }

    #[test]
    fn normalizes_codex_context_compaction_item_as_native_notification() {
        let message = json!({
            "method": "item/completed",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "item": {
                    "id": "compact-1",
                    "type": "contextCompaction"
                }
            }
        });

        let events = normalize_codex_message(SessionId::nil(), &message);

        assert_eq!(events.len(), 1);
        let NormalizedEvent::NativeNotification(event) = &events[0] else {
            panic!("expected native notification");
        };
        assert_eq!(event.event_id.as_deref(), Some("compact-1"));
        assert_eq!(event.category, "compaction");
        assert_eq!(event.title, "Context compacted");
    }

    #[test]
    fn normalizes_codex_slash_command_result_object_as_native_notification() {
        let message = json!({
            "accepted": true,
            "arguments": {},
            "command_id": "codex.compact",
            "danger_level": "safe",
            "message": "Codex compaction started.",
            "provider_payload": {
                "id": 12,
                "result": {}
            },
            "raw_input": "/compact",
            "target": "provider"
        });

        let events = normalize_codex_message(SessionId::nil(), &message);

        assert_eq!(events.len(), 1);
        let NormalizedEvent::NativeNotification(event) = &events[0] else {
            panic!("expected native notification");
        };
        assert_eq!(event.category, "slash_command");
        assert_eq!(event.title, "/compact");
        assert_eq!(event.detail.as_deref(), Some("Codex compaction started."));
        assert_eq!(event.status.as_deref(), Some("accepted"));
        assert_eq!(event.provider_payload.as_ref(), Some(&message));
    }

    #[test]
    fn normalizes_active_thread_status_as_running_status_and_native_notification() {
        let message = json!({
            "method": "thread/status/changed",
            "params": {
                "status": {
                    "activeFlags": [],
                    "type": "active"
                },
                "threadId": "019dddf9-a2d8-7510-91b8-9e351bd666dc"
            }
        });

        let events = normalize_codex_message(SessionId::nil(), &message);

        assert_eq!(events.len(), 2);
        let NormalizedEvent::SessionStatusChanged(status) = &events[0] else {
            panic!("expected session status");
        };
        assert_eq!(status.status, agenter_core::SessionStatus::Running);
        let NormalizedEvent::NativeNotification(event) = &events[1] else {
            panic!("expected native notification");
        };
        assert_eq!(event.category, "thread");
        assert_eq!(event.status.as_deref(), Some("active"));
    }

    #[test]
    fn normalizes_idle_thread_status_as_completed_status_and_native_notification() {
        let message = json!({
            "method": "thread/status/changed",
            "params": {
                "status": {
                    "type": "idle"
                },
                "threadId": "019dddf9-a2d8-7510-91b8-9e351bd666dc"
            }
        });

        let events = normalize_codex_message(SessionId::nil(), &message);

        assert_eq!(events.len(), 2);
        let NormalizedEvent::SessionStatusChanged(status) = &events[0] else {
            panic!("expected session status");
        };
        assert_eq!(status.status, agenter_core::SessionStatus::Idle);
        let NormalizedEvent::NativeNotification(event) = &events[1] else {
            panic!("expected native notification");
        };
        assert_eq!(event.category, "thread");
        assert_eq!(event.status.as_deref(), Some("idle"));
    }

    #[test]
    fn normalizes_turn_started_as_running_status_and_native_notification() {
        let message = json!({
            "method": "turn/started",
            "params": {
                "threadId": "019dddf9-a2d8-7510-91b8-9e351bd666dc",
                "turn": {
                    "completedAt": null,
                    "durationMs": null,
                    "error": null,
                    "id": "019de387-4cb9-7e51-bbeb-35f5e1c7d0bd",
                    "items": [],
                    "startedAt": 1777638788,
                    "status": "inProgress"
                }
            }
        });

        let events = normalize_codex_message(SessionId::nil(), &message);

        assert_eq!(events.len(), 2);
        let NormalizedEvent::SessionStatusChanged(status) = &events[0] else {
            panic!("expected session status");
        };
        assert_eq!(status.status, agenter_core::SessionStatus::Running);
        let NormalizedEvent::NativeNotification(event) = &events[1] else {
            panic!("expected native notification");
        };
        assert_eq!(
            event.event_id.as_deref(),
            Some("019de387-4cb9-7e51-bbeb-35f5e1c7d0bd")
        );
        assert_eq!(event.category, "turn");
        assert_eq!(event.status.as_deref(), Some("inProgress"));
    }

    #[test]
    fn normalizes_token_usage_update_as_native_notification_with_summary() {
        let message = json!({
            "method": "thread/tokenUsage/updated",
            "params": {
                "threadId": "019dddf9-a2d8-7510-91b8-9e351bd666dc",
                "tokenUsage": {
                    "last": {
                        "cachedInputTokens": 0,
                        "inputTokens": 0,
                        "outputTokens": 0,
                        "reasoningOutputTokens": 0,
                        "totalTokens": 10421
                    },
                    "modelContextWindow": 258400,
                    "total": {
                        "cachedInputTokens": 137309056,
                        "inputTokens": 140097909,
                        "outputTokens": 301095,
                        "reasoningOutputTokens": 44763,
                        "totalTokens": 140399004
                    }
                },
                "turnId": "019de387-4cb9-7e51-bbeb-35f5e1c7d0bd"
            }
        });

        let events = normalize_codex_message(SessionId::nil(), &message);

        assert_eq!(events.len(), 1);
        let NormalizedEvent::NativeNotification(event) = &events[0] else {
            panic!("expected native notification");
        };
        assert_eq!(event.category, "token_usage");
        assert_eq!(event.title, "Token usage updated");
        assert!(event
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("last 10421"));
        assert!(event
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("total 140399004"));
        assert!(event
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("window 258400"));
        assert_eq!(event.provider_payload.as_ref(), Some(&message));
    }

    #[test]
    fn normalizes_rate_limit_update_as_native_notification_with_summary() {
        let message = json!({
            "method": "account/rateLimits/updated",
            "params": {
                "rateLimits": {
                    "credits": null,
                    "limitId": "codex",
                    "limitName": null,
                    "planType": "prolite",
                    "primary": {
                        "resetsAt": 1777640533,
                        "usedPercent": 57,
                        "windowDurationMins": 300
                    },
                    "rateLimitReachedType": null,
                    "secondary": {
                        "resetsAt": 1777968663,
                        "usedPercent": 26,
                        "windowDurationMins": 10080
                    }
                }
            }
        });

        let events = normalize_codex_message(SessionId::nil(), &message);

        assert_eq!(events.len(), 1);
        let NormalizedEvent::NativeNotification(event) = &events[0] else {
            panic!("expected native notification");
        };
        assert_eq!(event.category, "rate_limits");
        assert_eq!(event.title, "Rate limits updated");
        assert!(event
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("prolite"));
        assert!(event
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("primary 57%"));
        assert!(event
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("secondary 26%"));
        assert_eq!(event.provider_payload.as_ref(), Some(&message));
    }

    #[test]
    fn normalizes_unknown_codex_notification_as_native_notification() {
        let message = json!({
            "method": "model/rerouted",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "from": "gpt-5.4",
                "to": "gpt-5.4-mini"
            }
        });

        let events = normalize_codex_message(SessionId::nil(), &message);

        assert_eq!(events.len(), 1);
        let NormalizedEvent::NativeNotification(event) = &events[0] else {
            panic!("expected native notification");
        };
        assert_eq!(event.category, "model");
        assert_eq!(event.title, "Model rerouted");
        assert_eq!(event.provider_payload.as_ref(), Some(&message));
    }

    #[test]
    fn normalizes_turn_diff_updated_as_turn_diff_event() {
        let message = json!({
            "method": "turn/diff/updated",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "message": "Added 3 lines"
            }
        });

        let events = normalize_codex_message(SessionId::nil(), &message);

        let NormalizedEvent::TurnDiffUpdated(event) = &events[0] else {
            panic!("expected turn diff event");
        };
        assert_eq!(event.method, "turn/diff/updated");
        assert_eq!(event.title, "Turn Diff Updated");
        assert_eq!(event.status.as_deref(), None);
    }

    #[test]
    fn normalizes_reasoning_updates_as_reasoning_events() {
        let message = json!({
            "method": "item/reasoning/textDelta",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "message": "Thinking..."
            }
        });

        let events = normalize_codex_message(SessionId::nil(), &message);

        let NormalizedEvent::ItemReasoning(event) = &events[0] else {
            panic!("expected item reasoning event");
        };
        assert_eq!(event.method, "item/reasoning/textDelta");
        assert_eq!(event.title, "Item Reasoning TextDelta");
    }

    #[test]
    fn normalizes_server_request_resolved_as_server_request_event() {
        let message = json!({
            "method": "serverRequest/resolved",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1"
            }
        });

        let events = normalize_codex_message(SessionId::nil(), &message);

        let NormalizedEvent::ServerRequestResolved(event) = &events[0] else {
            panic!("expected server request resolved event");
        };
        assert_eq!(event.method, "serverRequest/resolved");
        assert_eq!(event.title, "ServerRequest Resolved");
    }

    #[test]
    fn normalizes_mcp_tool_call_progress_as_mcp_tool_call_progress_event() {
        let message = json!({
            "method": "item/mcpToolCall/progress",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "message": "Connecting"
            }
        });

        let events = normalize_codex_message(SessionId::nil(), &message);

        let NormalizedEvent::McpToolCallProgress(event) = &events[0] else {
            panic!("expected mcp tool call progress event");
        };
        assert_eq!(event.method, "item/mcpToolCall/progress");
        assert_eq!(event.title, "Item McpToolCall Progress");
    }

    #[test]
    fn normalizes_thread_realtime_events_as_thread_realtime_event() {
        let message = json!({
            "method": "thread/realtime/update",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "message": "streaming"
            }
        });

        let events = normalize_codex_message(SessionId::nil(), &message);

        let NormalizedEvent::ThreadRealtimeEvent(event) = &events[0] else {
            panic!("expected thread realtime event");
        };
        assert_eq!(event.method, "thread/realtime/update");
        assert_eq!(event.title, "Thread Realtime Update");
    }

    #[test]
    fn builds_error_response_for_unsupported_codex_server_request() {
        let message = json!({
            "id": "tool-call-1",
            "method": "item/tool/call",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "itemId": "tool-1"
            }
        });

        let Some((native_request_id, method)) = unsupported_codex_server_request(&message) else {
            panic!("expected unsupported server request");
        };

        assert_eq!(native_request_id, json!("tool-call-1"));
        assert_eq!(method, "item/tool/call");
        assert_eq!(
            unsupported_request_response(native_request_id, method),
            json!({
                "id": "tool-call-1",
                "error": {
                    "code": -32601,
                    "message": "unsupported Codex server request method: item/tool/call"
                }
            })
        );
    }

    #[test]
    fn normalizes_codex_plan_update_notification() {
        let message = json!({
            "method": "turn/plan/updated",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "explanation": "Implement in phases",
                "plan": [
                    {"step": "Add tests", "status": "completed"},
                    {"step": "Implement mapping", "status": "inProgress"},
                    {"step": "Verify", "status": "pending"}
                ]
            }
        });

        let events = normalize_codex_message(SessionId::nil(), &message);

        assert_eq!(events.len(), 1);
        let NormalizedEvent::PlanUpdated(plan) = &events[0] else {
            panic!("expected plan update");
        };
        assert_eq!(plan.plan_id.as_deref(), Some("turn-1"));
        assert_eq!(plan.content.as_deref(), Some("Implement in phases"));
        assert_eq!(plan.entries[1].label, "Implement mapping");
        assert_eq!(
            plan.entries[1].status,
            agenter_core::PlanEntryStatus::InProgress
        );
    }

    #[test]
    fn normalizes_codex_plan_delta_against_turn_plan_when_turn_id_is_present() {
        let message = json!({
            "method": "item/plan/delta",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "itemId": "plan-item-1",
                "delta": "Add the reducer"
            }
        });

        let events = normalize_codex_message(SessionId::nil(), &message);

        assert_eq!(events.len(), 1);
        let NormalizedEvent::PlanUpdated(plan) = &events[0] else {
            panic!("expected plan update");
        };
        assert_eq!(plan.plan_id.as_deref(), Some("turn-1"));
        assert_eq!(plan.content.as_deref(), Some("Add the reducer"));
        assert!(plan.append);
    }

    #[test]
    fn normalizes_codex_command_output_delta() {
        let message = json!({
            "method": "item/commandExecution/outputDelta",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "itemId": "cmd-1",
                "stream": "stderr",
                "delta": "warning\n"
            }
        });

        let events = normalize_codex_message(SessionId::nil(), &message);

        assert_eq!(events.len(), 1);
        let NormalizedEvent::CommandOutputDelta(output) = &events[0] else {
            panic!("expected command output");
        };
        assert_eq!(output.command_id, "cmd-1");
        assert_eq!(output.stream, agenter_core::CommandOutputStream::Stderr);
        assert_eq!(output.delta, "warning\n");
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
    fn filters_live_codex_messages_to_target_thread_but_not_turn() {
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
        let same_thread_other_turn = json!({
            "method": "item/agentMessage/delta",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-2",
                "itemId": "msg-live-2",
                "delta": "still same thread"
            }
        });

        assert_eq!(
            normalize_codex_message_for_scope(SessionId::nil(), &matching, &target).len(),
            1
        );
        assert!(
            normalize_codex_message_for_scope(SessionId::nil(), &other_thread, &target).is_empty()
        );
        assert_eq!(
            normalize_codex_message_for_scope(SessionId::nil(), &same_thread_other_turn, &target)
                .len(),
            1
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
            normalize_codex_approval_request(SessionId::nil(), &message, None)
                .expect("approval should normalize");

        assert_eq!(native_request_id, json!("approval-1"));
        assert_eq!(approval_kind, CodexApprovalKind::Command);
        let NormalizedEvent::ApprovalRequested(request) = event else {
            panic!("expected approval request");
        };
        assert_eq!(request.kind, ApprovalKind::Command);
        assert_eq!(request.details.as_deref(), Some("cargo test"));
        let pres = request.presentation.expect("command presentation");
        assert_eq!(pres.get("variant"), Some(&json!("codex_command")));
        assert_eq!(pres.get("command"), Some(&json!("cargo test")));
    }

    #[test]
    fn enriches_file_change_approval_via_cache() {
        let started = json!({
            "method": "item/started",
            "params": {
                "threadId": "t1",
                "turnId": "u1",
                "item": {
                    "id": "call_git",
                    "type": "fileChange",
                    "changes": [{"path": "README.md", "kind": {"type": "Update"}, "diff": "@@ hi"}]
                }
            }
        });
        let mut cache = CodexApprovalItemCache::default();
        cache.observe_jsonrpc_message(&started);

        let message = json!({
            "id": 1,
            "method": "item/fileChange/requestApproval",
            "params": {
                "threadId": "t1",
                "turnId": "u1",
                "itemId": "call_git",
                "reason": "edit readme"
            }
        });

        let (_id, rid, ak, ev) =
            normalize_codex_approval_request(SessionId::nil(), &message, Some(&cache))
                .expect("approval");
        assert_eq!(rid, json!(1));
        assert_eq!(ak, CodexApprovalKind::FileChange);

        let NormalizedEvent::ApprovalRequested(request) = ev else {
            panic!("expected approval");
        };
        let pres = request.presentation.expect("presentation");
        assert_eq!(pres["variant"], "codex_file_change");
        assert_eq!(pres["paths"], json!(["README.md"]));
        let details = request.details.expect("details");
        assert!(
            details.contains("README.md"),
            "details missing path: {details}"
        );
        assert!(
            details.contains("Reason"),
            "missing reason prelude: {details}"
        );
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
                    {
                        "id": "thread-1",
                        "title": "Imported Thread",
                        "updatedAt": "2026-01-01T12:00:00Z"
                    }
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
                                },
                                {
                                    "type": "contextCompaction",
                                    "id": "compact-1"
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
                updated_at: Some("2026-01-01T12:00:00Z".to_owned()),
            }]
        );

        let observed_list_response = json!({
            "id": 4,
            "result": {
                "data": [
                    {
                        "id": "019ddf92-1e65-7e72-b656-c317a83e0b93",
                        "preview": "Let's revamp the frontend.",
                        "updated_at": 1700000000,
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
                updated_at: Some("1700000000".to_owned()),
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
                DiscoveredSessionHistoryItem::NativeNotification {
                    event_id: Some("compact-1".to_owned()),
                    category: "compaction".to_owned(),
                    title: "Context compacted".to_owned(),
                    detail: None,
                    status: Some("completed".to_owned()),
                    provider_payload: Some(json!({
                        "type": "contextCompaction",
                        "id": "compact-1"
                    })),
                },
            ]
        );
    }

    #[test]
    fn codex_history_preserves_argv_commands_mcp_tools_and_unknown_types() {
        let read_response = json!({
            "id": 6,
            "result": {
                "thread": {
                    "turns": [
                        {
                            "items": [
                                {
                                    "type": "commandExecution",
                                    "id": "argv-cmd",
                                    "argv": ["git", "status"],
                                    "cwd": "/repo",
                                    "status": "completed",
                                    "exitCode": 0,
                                    "aggregatedOutput": "clean"
                                },
                                {
                                    "type": "mcpToolCall",
                                    "id": "mcp-1",
                                    "name": "read_file",
                                    "status": "completed",
                                    "arguments": {"path": "README.md"}
                                },
                                {
                                    "type": "orphanGadget",
                                    "id": "og-1",
                                    "status": "done",
                                    "summary": "experimental row"
                                }
                            ]
                        }
                    ]
                }
            }
        });

        assert_eq!(
            codex_history_from_thread_read_response(&read_response),
            vec![
                DiscoveredSessionHistoryItem::Command {
                    command_id: "argv-cmd".to_owned(),
                    command: "git status".to_owned(),
                    cwd: Some("/repo".to_owned()),
                    source: None,
                    process_id: None,
                    duration_ms: None,
                    actions: vec![],
                    output: Some("clean".to_owned()),
                    exit_code: Some(0),
                    success: true,
                    provider_payload: Some(json!({
                        "type": "commandExecution",
                        "id": "argv-cmd",
                        "argv": ["git", "status"],
                        "cwd": "/repo",
                        "status": "completed",
                        "exitCode": 0,
                        "aggregatedOutput": "clean"
                    })),
                },
                DiscoveredSessionHistoryItem::Tool {
                    tool_call_id: "mcp-1".to_owned(),
                    name: "read_file".to_owned(),
                    title: Some("read_file".to_owned()),
                    status: DiscoveredToolStatus::Completed,
                    input: Some(json!({
                        "type": "mcpToolCall",
                        "id": "mcp-1",
                        "name": "read_file",
                        "status": "completed",
                        "arguments": {"path": "README.md"}
                    })),
                    output: None,
                    provider_payload: Some(json!({
                        "type": "mcpToolCall",
                        "id": "mcp-1",
                        "name": "read_file",
                        "status": "completed",
                        "arguments": {"path": "README.md"}
                    })),
                },
                DiscoveredSessionHistoryItem::NativeNotification {
                    event_id: Some("og-1".to_owned()),
                    category: "codex_thread_item".to_owned(),
                    title: "orphanGadget".to_owned(),
                    detail: Some("experimental row".to_owned()),
                    status: Some("done".to_owned()),
                    provider_payload: Some(json!({
                        "type": "orphanGadget",
                        "id": "og-1",
                        "status": "done",
                        "summary": "experimental row"
                    })),
                },
            ]
        );
    }

    #[test]
    fn codex_history_user_message_multimodal_falls_back_to_native_notification() {
        let read_response = json!({
            "result": {
                "thread": {
                    "turns": [
                        {
                            "items": [
                                {
                                    "type": "userMessage",
                                    "id": "u-img",
                                    "content": [{"type": "input_image", "imageId": "img-9"}]
                                }
                            ]
                        }
                    ]
                }
            }
        });

        assert_eq!(
            codex_history_from_thread_read_response(&read_response),
            vec![DiscoveredSessionHistoryItem::NativeNotification {
                event_id: Some("u-img".to_owned()),
                category: "codex_thread_item".to_owned(),
                title: "userMessage".to_owned(),
                detail: None,
                status: None,
                provider_payload: Some(json!({
                    "type": "userMessage",
                    "id": "u-img",
                    "content": [{"type": "input_image", "imageId": "img-9"}]
                })),
            },]
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
    fn detects_codex_turn_start_thread_not_found_errors() {
        let error = anyhow!("codex turn/start failed: -32600 thread not found: thread-1");

        assert!(is_codex_thread_not_found_error(error.as_ref()));
        assert!(!is_codex_thread_not_found_error(
            anyhow!("codex turn/start failed: -32600 model not found").as_ref()
        ));
        assert!(!is_codex_thread_not_found_error(
            anyhow!("codex thread/resume failed: -32600 thread not found: thread-1").as_ref()
        ));
    }

    #[test]
    fn detects_codex_no_rollout_resume_errors() {
        assert!(is_codex_no_rollout_resume_error(
            anyhow!(
                "codex thread/resume failed: -32600 no rollout found for thread id 019de8a3-e4d4-78e3-8382-458afaddbe13"
            )
            .as_ref()
        ));
        assert!(!is_codex_no_rollout_resume_error(
            anyhow!("codex thread/resume failed: -32600 thread not found: thread-1").as_ref()
        ));
        assert!(!is_codex_no_rollout_resume_error(
            anyhow!(
                "codex turn/start failed: -32600 no rollout found for thread id 019de8a3-e4d4-78e3-8382-458afaddbe13"
            )
            .as_ref()
        ));
    }

    #[test]
    fn codex_provider_slash_command_manifest_marks_dangerous_commands() {
        let commands = codex_provider_slash_commands();

        let shell = commands
            .iter()
            .find(|command| command.id == "codex.shell")
            .expect("shell command");
        assert_eq!(shell.name, "shell");
        assert_eq!(shell.aliases, vec!["sh"]);
        assert_eq!(shell.danger_level, SlashCommandDangerLevel::Dangerous);
        assert_eq!(shell.arguments[0].kind, SlashCommandArgumentKind::Rest);

        let compact = commands
            .iter()
            .find(|command| command.id == "codex.compact")
            .expect("compact command");
        assert_eq!(compact.danger_level, SlashCommandDangerLevel::Safe);
    }

    #[test]
    fn maps_codex_provider_slash_commands_to_jsonrpc_requests() {
        let workspace = PathBuf::from("/work/agenter");
        let compact = SlashCommandRequest {
            command_id: "codex.compact".to_owned(),
            universal_command_id: None,
            idempotency_key: None,
            arguments: json!({}),
            raw_input: "/compact".to_owned(),
            confirmed: false,
        };
        let (method, params) =
            codex_provider_command_request("thread-1", &compact, Some("turn-1"), &workspace)
                .expect("compact maps");
        assert_eq!(method, "thread/compact/start");
        assert_eq!(params, json!({"threadId": "thread-1"}));

        let steer = SlashCommandRequest {
            command_id: "codex.steer".to_owned(),
            universal_command_id: None,
            idempotency_key: None,
            arguments: json!({"input": "please focus"}),
            raw_input: "/steer please focus".to_owned(),
            confirmed: false,
        };
        let (method, params) =
            codex_provider_command_request("thread-1", &steer, Some("turn-1"), &workspace)
                .expect("steer maps");
        assert_eq!(method, "turn/steer");
        assert_eq!(params["expectedTurnId"], "turn-1");
        assert_eq!(params["input"][0]["text"], "please focus");

        let shell = SlashCommandRequest {
            command_id: "codex.shell".to_owned(),
            universal_command_id: None,
            idempotency_key: None,
            arguments: json!({"command": "pwd | cat"}),
            raw_input: "/shell pwd | cat".to_owned(),
            confirmed: true,
        };
        let (method, params) =
            codex_provider_command_request("thread-1", &shell, Some("turn-1"), &workspace)
                .expect("shell maps");
        assert_eq!(method, "thread/shellCommand");
        assert_eq!(params["command"], "pwd | cat");
    }

    #[test]
    fn maps_codex_review_and_rollback_arguments() {
        let workspace = PathBuf::from("/work/agenter");
        let review = SlashCommandRequest {
            command_id: "codex.review".to_owned(),
            universal_command_id: None,
            idempotency_key: None,
            arguments: json!({"base": "main", "detached": true}),
            raw_input: "/review --base main --detached".to_owned(),
            confirmed: false,
        };
        let (method, params) =
            codex_provider_command_request("thread-1", &review, None, &workspace)
                .expect("review maps");
        assert_eq!(method, "review/start");
        assert_eq!(
            params["target"],
            json!({"type": "baseBranch", "branch": "main"})
        );
        assert_eq!(params["delivery"], "detached");

        let rollback = SlashCommandRequest {
            command_id: "codex.rollback".to_owned(),
            universal_command_id: None,
            idempotency_key: None,
            arguments: json!({"numTurns": 2}),
            raw_input: "/rollback 2".to_owned(),
            confirmed: true,
        };
        let (method, params) =
            codex_provider_command_request("thread-1", &rollback, None, &workspace)
                .expect("rollback maps");
        assert_eq!(method, "thread/rollback");
        assert_eq!(params["numTurns"], 2);
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
        assert_eq!(
            approval_response_for_decision(
                CodexApprovalKind::ExecCommandApproval,
                ApprovalDecision::Accept
            ),
            json!({"decision": "approved"})
        );
        assert_eq!(
            approval_response_for_decision(
                CodexApprovalKind::ApplyPatchApproval,
                ApprovalDecision::Decline
            ),
            json!({"decision": "denied"})
        );
    }

    #[test]
    fn normalizes_exec_command_approval_alias_fixture() {
        let msg = json!({
            "method": "execCommandApproval",
            "id": "req-exec-alias",
            "params": {
                "conversationId": "conv-1",
                "callId": "call-1",
                "command": ["echo", "alias"],
                "cwd": "/tmp/ws",
                "reason": "verify mapping"
            }
        });
        let Some((_, _, codex_kind, NormalizedEvent::ApprovalRequested(req))) =
            normalize_codex_approval_request(SessionId::nil(), &msg, None)
        else {
            panic!("expected exec approval mapping");
        };
        assert_eq!(codex_kind, CodexApprovalKind::ExecCommandApproval);
        let details = req.details.expect("exec details");
        assert!(details.contains("echo alias"));
        assert!(details.contains("/tmp/ws"));
    }

    #[test]
    fn normalizes_apply_patch_approval_alias_fixture() {
        let msg = json!({
            "method": "applyPatchApproval",
            "id": 991,
            "params": {
                "conversationId": "conv-1",
                "callId": "call-42",
                "fileChanges": {
                    "/proj/a.rs": {},
                    "/proj/b.rs": {}
                },
                "reason": "touch two files"
            }
        });
        let Some((_, _, codex_kind, NormalizedEvent::ApprovalRequested(req))) =
            normalize_codex_approval_request(SessionId::nil(), &msg, None)
        else {
            panic!("expected patch approval mapping");
        };
        assert_eq!(codex_kind, CodexApprovalKind::ApplyPatchApproval);
        let details = req.details.expect("patch details");
        assert!(details.contains("a.rs"));
        assert!(details.contains("b.rs"));
    }

    #[test]
    fn codex_jsonrpc_request_ids_equal_accepts_numbers_and_numeric_strings() {
        let id = CodexRequestId::numeric(4);
        assert!(codex_jsonrpc_request_ids_equal(Some(&json!(4)), &id));
        assert!(codex_jsonrpc_request_ids_equal(Some(&json!("4")), &id));
        assert!(!codex_jsonrpc_request_ids_equal(Some(&json!("5")), &id));

        let string_id = CodexRequestId::String("tool-call-1".to_owned());
        assert!(codex_jsonrpc_request_ids_equal(
            Some(&json!("tool-call-1")),
            &string_id
        ));
        assert!(!codex_jsonrpc_request_ids_equal(
            Some(&json!("other-tool-call")),
            &string_id
        ));
    }

    #[test]
    fn codex_rpc_classifies_responses_and_server_requests() {
        let id = CodexRequestId::numeric(2);
        assert!(codex_rpc_is_response_matching_request(
            &json!({"id": 2, "result": {}}),
            &id
        ));
        assert!(!codex_rpc_is_codex_server_to_client_request(
            &json!({"id": 2, "result": {}})
        ));
        assert!(unsupported_codex_server_request(&json!({"id": 2, "result": {}})).is_none());
        assert!(codex_rpc_is_codex_server_to_client_request(
            &json!({"id": 2, "method": "item/tool/requestUserInput", "params": {}})
        ));
        assert!(codex_rpc_is_codex_server_to_client_response(
            &json!({"id": "2", "result": {"ok": true}})
        ));
        assert_eq!(codex_jsonrpc_request_id_summary(&json!({"id": 12})), "12");
        assert_eq!(
            codex_jsonrpc_request_id_summary(&json!({"id": "turn-1"})),
            "turn-1"
        );
        assert_eq!(
            codex_jsonrpc_request_id_summary(&json!({"id": null})),
            "null"
        );
        assert_eq!(codex_jsonrpc_request_id_summary(&json!({})), "<missing id>");
    }

    #[test]
    fn scope_allows_thread_notifications_without_turn_id_when_turn_scope_is_known() {
        let target = CodexTurnScope {
            thread_id: Some("thread-1".to_owned()),
            turn_id: Some("turn-1".to_owned()),
        };
        let thread_only = json!({
            "method": "thread/status/changed",
            "params": {
                "threadId": "thread-1",
                "status": {"type": "active"}
            }
        });

        assert_eq!(
            normalize_codex_message_for_scope(SessionId::nil(), &thread_only, &target).len(),
            2
        );
    }

    #[test]
    fn codex_wire_classification_names_requests_responses_and_notifications() {
        assert_eq!(
            codex_wire_classification(&json!({"id": 1, "method": "turn/start", "params": {}})),
            "server_request_received"
        );
        assert_eq!(
            codex_wire_classification(&json!({"id": 1, "result": {"ok": true}})),
            "server_response_received"
        );
        assert_eq!(
            codex_wire_classification(&json!({"method": "turn/completed", "params": {}})),
            "server_notification_received"
        );
        assert_eq!(
            codex_wire_classification(&json!({"jsonrpc": "2.0"})),
            "unknown_received"
        );
    }

    #[test]
    fn codex_scope_context_reports_expected_and_actual_targets() {
        let scope = CodexTurnScope {
            thread_id: Some("thread-1".to_owned()),
            turn_id: Some("turn-1".to_owned()),
        };
        let message = json!({
            "id": "request-1",
            "method": "item/agentMessage/delta",
            "params": {
                "threadId": "thread-2",
                "turnId": "turn-9",
                "delta": "wrong thread"
            }
        });

        let context = CodexScopeLogContext::from_message(&message, &scope);

        assert_eq!(context.expected_thread_id.as_deref(), Some("thread-1"));
        assert_eq!(context.expected_turn_id.as_deref(), Some("turn-1"));
        assert_eq!(context.actual_thread_id.as_deref(), Some("thread-2"));
        assert_eq!(context.actual_turn_id.as_deref(), Some("turn-9"));
        assert!(!context.scope_match);
        assert_eq!(context.reason.as_deref(), Some("thread_id_mismatch"));
    }

    #[test]
    fn codex_wire_logger_is_disabled_by_default() {
        let logger = CodexWireLogger::disabled();

        logger
            .record(CodexWireLogRecord {
                direction: "recv",
                classification: "server_notification_received",
                session_id: None,
                workspace: None,
                runtime_thread_id: None,
                runtime_turn_id: None,
                reason: None,
                message: Some(json!({"method": "turn/completed"})),
                stderr_line: None,
                scope: None,
            })
            .expect("disabled logger should be a no-op");
    }

    #[test]
    fn codex_wire_logger_writes_jsonl_records_with_payload_and_context() {
        let path = std::env::temp_dir().join(format!(
            "agenter-codex-wire-log-test-{}.jsonl",
            uuid::Uuid::new_v4()
        ));
        let logger = CodexWireLogger::for_test_file(path.clone());
        let session_id = SessionId::new();
        let message = json!({
            "id": "native-1",
            "method": "item/fileChange/requestApproval",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "itemId": "item-1"
            }
        });

        logger
            .record(CodexWireLogRecord {
                direction: "recv",
                classification: "server_request_received",
                session_id: Some(session_id),
                workspace: Some("/tmp/workspace".into()),
                runtime_thread_id: Some("thread-runtime".into()),
                runtime_turn_id: Some("turn-runtime".into()),
                reason: Some("approval_pending"),
                message: Some(message.clone()),
                stderr_line: None,
                scope: None,
            })
            .expect("record write should succeed");

        let line = std::fs::read_to_string(&path).expect("wire log file should exist");
        let record: Value = serde_json::from_str(line.trim()).expect("jsonl record should parse");
        assert_eq!(record["direction"], "recv");
        assert_eq!(record["classification"], "server_request_received");
        assert_eq!(record["session_id"], session_id.to_string());
        assert_eq!(record["workspace"], "/tmp/workspace");
        assert_eq!(record["provider_thread_id"], "thread-1");
        assert_eq!(record["provider_turn_id"], "turn-1");
        assert_eq!(record["runtime_thread_id"], "thread-runtime");
        assert_eq!(record["runtime_turn_id"], "turn-runtime");
        assert_eq!(record["jsonrpc_id"], "native-1");
        assert_eq!(record["method"], "item/fileChange/requestApproval");
        assert_eq!(record["reason"], "approval_pending");
        assert_eq!(record["payload"], message);

        let _ = std::fs::remove_file(path);
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
    fn turn_start_params_round_trip_default_mode_with_no_developer_instructions() {
        // Regression test for the "Implement the plan." handoff: the browser
        // sets `collaboration_mode = "default"` via `settings_override` and
        // the runner must forward `collaborationMode.mode = "default"` so
        // Codex's app-server normalizer can fill in the Default preset's
        // developer instructions. If the field is missing, Codex retains the
        // thread's previous Plan mode and re-emits the plan.
        let request = CodexTurnRequest {
            session_id: SessionId::nil(),
            workspace_path: PathBuf::from("/work/agenter"),
            external_session_id: Some("thread-1".to_owned()),
            prompt: "Implement the plan.".to_owned(),
            settings: Some(agenter_core::AgentTurnSettings {
                model: None,
                reasoning_effort: None,
                collaboration_mode: Some("default".to_owned()),
            }),
        };

        let params = codex_turn_start_params("thread-1", &request);

        assert_eq!(params["collaborationMode"]["mode"], "default");
        // Without explicit overrides the runner should still produce a
        // payload Codex will accept; model nulls out and reasoning_effort is
        // omitted so the app-server normalizer fills in the preset.
        assert_eq!(
            params["collaborationMode"]["settings"]["model"],
            Value::Null
        );
        assert!(
            params["collaborationMode"]
                .as_object()
                .expect("collaboration mode payload object")
                .get("developer_instructions")
                .is_none(),
            "developer_instructions must be omitted so Codex's normalizer fills the Default preset"
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
        if let NormalizedEvent::QuestionRequested(payload) = event {
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

        if let NormalizedEvent::QuestionRequested(payload) = event {
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

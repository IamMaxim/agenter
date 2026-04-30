use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use agenter_core::{
    AgentErrorEvent, AgentMessageDeltaEvent, AppEvent, ApprovalDecision, ApprovalId, ApprovalKind,
    ApprovalRequestEvent, CommandCompletedEvent, CommandEvent, FileChangeEvent, FileChangeKind,
    MessageCompletedEvent, SessionId,
};
use anyhow::{anyhow, Context};
use serde_json::{json, Value};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::{mpsc, oneshot},
    time::timeout,
};

const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const STARTUP_RESPONSE_TIMEOUT: Duration = Duration::from_secs(15);
const RECENT_STDERR_LINES: usize = 20;

#[derive(Debug)]
pub struct CodexTurnRequest {
    pub session_id: SessionId,
    pub workspace_path: PathBuf,
    pub external_session_id: Option<String>,
    pub prompt: String,
}

#[derive(Debug)]
pub struct PendingCodexApproval {
    pub response: oneshot::Sender<ApprovalDecision>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodexApprovalKind {
    Command,
    FileChange,
    Permissions,
}

#[derive(Debug)]
pub struct CodexAppServer {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    thread_id: Option<String>,
    turn_id: Option<String>,
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
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
            thread_id: None,
            turn_id: None,
            stderr_tail,
        })
    }

    pub async fn initialize_and_start_thread(
        &mut self,
        request: &CodexTurnRequest,
    ) -> anyhow::Result<()> {
        tracing::debug!(
            session_id = %request.session_id,
            has_external_session_id = request.external_session_id.is_some(),
            "initializing codex app-server"
        );
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

        if let Some(thread_id) = &request.external_session_id {
            self.thread_id = Some(thread_id.clone());
            tracing::info!(session_id = %request.session_id, provider_thread_id = %thread_id, "resuming codex thread");
            let resume_id = self
                .send_request(
                    "thread/resume",
                    json!({
                        "threadId": thread_id,
                        "cwd": request.workspace_path,
                        "approvalPolicy": "on-request",
                        "approvalsReviewer": "user",
                        "excludeTurns": false
                    }),
                )
                .await?;
            self.read_response(resume_id, "thread/resume").await?;
        } else {
            tracing::info!(session_id = %request.session_id, "starting codex thread");
            let start_id = self
                .send_request(
                    "thread/start",
                    json!({
                        "cwd": request.workspace_path,
                        "approvalPolicy": "on-request",
                        "approvalsReviewer": "user",
                        "sandbox": "read-only",
                        "sessionStartSource": "agenter"
                    }),
                )
                .await?;
            let response = self.read_response(start_id, "thread/start").await?;
            if let Some(thread_id) = codex_thread_id(&response) {
                self.thread_id = Some(thread_id.to_owned());
            }
            if self.thread_id.is_none() {
                return Err(anyhow!(missing_thread_id_error(
                    "thread/start",
                    &response,
                    &self.recent_stderr()
                )));
            }
        }

        Ok(())
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
            .send_request(
                "turn/start",
                json!({
                    "threadId": thread_id,
                    "cwd": request.workspace_path,
                    "approvalPolicy": "on-request",
                    "approvalsReviewer": "user",
                    "sandboxPolicy": {"type": "readOnly", "networkAccess": false},
                    "input": [{"type": "text", "text": request.prompt}]
                }),
            )
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

    pub async fn shutdown(mut self) -> anyhow::Result<()> {
        tracing::debug!("shutting down codex app-server");
        self.stdin.shutdown().await.ok();
        match timeout(SHUTDOWN_TIMEOUT, self.child.wait()).await {
            Ok(result) => {
                result?;
                tracing::debug!("codex app-server exited");
            }
            Err(_) => {
                self.child.kill().await.ok();
                tracing::warn!("killed codex app-server after shutdown timeout");
            }
        }
        Ok(())
    }
}

pub async fn run_codex_turn(
    request: CodexTurnRequest,
    event_sender: mpsc::UnboundedSender<AppEvent>,
    pending_approvals: std::sync::Arc<
        tokio::sync::Mutex<HashMap<ApprovalId, PendingCodexApproval>>,
    >,
) -> anyhow::Result<()> {
    tracing::info!(session_id = %request.session_id, "codex turn task started");
    let mut server = CodexAppServer::spawn(request.workspace_path.clone())?;
    server.initialize_and_start_thread(&request).await?;

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

        for event in normalize_codex_message_for_scope(request.session_id, &message, &scope) {
            let completed = jsonrpc_method(&message) == Some("turn/completed");
            event_sender.send(event).ok();
            if completed {
                tracing::info!(session_id = %request.session_id, "codex turn completed");
                server.shutdown().await?;
                return Ok(());
            }
        }
    }

    Ok(())
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
}

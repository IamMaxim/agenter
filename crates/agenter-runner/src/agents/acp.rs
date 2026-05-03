use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use crate::agents::approval_state::PendingProviderApproval;
use agenter_core::{
    AgentCapabilities, AgentErrorEvent, AgentMessageDeltaEvent, AgentProviderId, AppEvent,
    ApprovalDecision, ApprovalId, ApprovalKind, ApprovalRequestEvent, CommandCompletedEvent,
    CommandEvent, CommandOutputEvent, CommandOutputStream, FileChangeEvent, FileChangeKind,
    MessageCompletedEvent, PlanEntry, PlanEntryStatus, PlanEvent, ProviderEvent, SessionId,
    SessionStatus, SessionStatusChangedEvent, ToolEvent,
};
use anyhow::{anyhow, Context};
use serde_json::{json, Value};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::{mpsc, oneshot, Mutex},
    time::timeout,
};
use uuid::Uuid;

const ACP_RESPONSE_TIMEOUT: Duration = Duration::from_secs(20);
const GEMINI_ACP_RESPONSE_TIMEOUT: Duration = Duration::from_secs(60);
const ACP_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const ACP_STDERR_EXCERPT_LINES: usize = 12;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AcpProviderProfile {
    pub provider_id: AgentProviderId,
    pub title: &'static str,
    command: &'static str,
    args: &'static [&'static str],
    cwd_arg: Option<&'static str>,
}

impl AcpProviderProfile {
    #[must_use]
    pub fn qwen() -> Self {
        Self {
            provider_id: AgentProviderId::from(AgentProviderId::QWEN),
            title: "Qwen Code",
            command: "qwen",
            args: &["--acp", "--approval-mode", "default"],
            cwd_arg: None,
        }
    }

    #[must_use]
    pub fn gemini() -> Self {
        Self {
            provider_id: AgentProviderId::from(AgentProviderId::GEMINI),
            title: "Gemini CLI",
            command: "gemini",
            args: &["--acp"],
            cwd_arg: None,
        }
    }

    #[must_use]
    pub fn opencode() -> Self {
        Self {
            provider_id: AgentProviderId::from(AgentProviderId::OPENCODE),
            title: "OpenCode",
            command: "opencode",
            args: &["acp"],
            cwd_arg: Some("--cwd"),
        }
    }

    #[must_use]
    pub fn all() -> Vec<Self> {
        vec![Self::qwen(), Self::gemini(), Self::opencode()]
    }

    #[must_use]
    pub fn available_all() -> Vec<Self> {
        Self::all()
            .into_iter()
            .filter(|profile| command_available(profile.command))
            .collect()
    }

    #[must_use]
    pub fn command_line(&self, workspace_path: &Path) -> (String, Vec<String>) {
        let mut args = self
            .args
            .iter()
            .map(|arg| (*arg).to_owned())
            .collect::<Vec<_>>();
        if let Some(cwd_arg) = self.cwd_arg {
            args.push(cwd_arg.to_owned());
            args.push(workspace_path.display().to_string());
        }
        (self.command.to_owned(), args)
    }

    #[must_use]
    pub fn response_timeout(&self) -> Duration {
        if self.provider_id.as_str() == AgentProviderId::GEMINI {
            GEMINI_ACP_RESPONSE_TIMEOUT
        } else {
            ACP_RESPONSE_TIMEOUT
        }
    }

    #[must_use]
    pub fn advertised_capabilities(&self) -> AgentCapabilities {
        let session_capabilities = match self.provider_id.as_str() {
            AgentProviderId::GEMINI => AcpInitializeCapabilities {
                load_session: true,
                session_list: false,
                session_resume: false,
                session_fork: false,
            },
            _ => AcpInitializeCapabilities {
                load_session: true,
                session_list: true,
                session_resume: true,
                session_fork: self.provider_id.as_str() == AgentProviderId::OPENCODE,
            },
        };
        session_capabilities.to_agent_capabilities()
    }
}

fn command_available(command: &str) -> bool {
    std::process::Command::new(command)
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AcpInitializeCapabilities {
    pub load_session: bool,
    pub session_list: bool,
    pub session_resume: bool,
    pub session_fork: bool,
}

impl AcpInitializeCapabilities {
    #[must_use]
    pub fn from_initialize(message: &Value) -> Self {
        let capabilities = message
            .pointer("/result/agentCapabilities")
            .unwrap_or(&Value::Null);
        let session_capabilities = capabilities
            .pointer("/sessionCapabilities")
            .unwrap_or(&Value::Null);
        Self {
            load_session: capabilities
                .pointer("/loadSession")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            session_list: session_capabilities.get("list").is_some(),
            session_resume: session_capabilities.get("resume").is_some(),
            session_fork: session_capabilities.get("fork").is_some(),
        }
    }

    #[must_use]
    pub fn supports_session_list(&self) -> bool {
        self.session_list
    }

    #[must_use]
    pub fn to_agent_capabilities(&self) -> AgentCapabilities {
        AgentCapabilities {
            streaming: true,
            session_resume: self.load_session || self.session_resume,
            session_history: self.session_list,
            approvals: true,
            file_changes: true,
            command_execution: true,
            plan_updates: true,
            interrupt: false,
            ..AgentCapabilities::default()
        }
    }
}

fn acp_timeout_error(provider_id: &AgentProviderId, method: &str, stderr_excerpt: &str) -> String {
    let mut message =
        format!("timed out waiting for ACP `{method}` response from provider `{provider_id}`");
    if !stderr_excerpt.is_empty() {
        message.push_str("; recent stderr: ");
        message.push_str(stderr_excerpt);
    }
    if provider_id.as_str() == AgentProviderId::GEMINI {
        message.push_str(
            "; Gemini setup hint: run Gemini outside a restrictive sandbox, then authenticate and trust the workspace locally",
        );
    }
    message
}

fn acp_exit_error(provider_id: &AgentProviderId, method: &str, stderr_excerpt: &str) -> String {
    let mut message = format!("ACP provider `{provider_id}` exited before `{method}` response");
    if !stderr_excerpt.is_empty() {
        message.push_str("; recent stderr: ");
        message.push_str(stderr_excerpt);
    }
    if provider_id.as_str() == AgentProviderId::GEMINI {
        message.push_str(
            "; Gemini setup hint: run Gemini outside a restrictive sandbox, then authenticate and trust the workspace locally",
        );
    }
    message
}

fn acp_error_response(provider_id: &AgentProviderId, method: &str, error: &Value) -> String {
    format!("ACP `{method}` failed for provider `{provider_id}`: {error}")
}

pub type PendingAcpApproval = PendingProviderApproval;

#[derive(Clone)]
pub struct AcpRunnerRuntime {
    workspace_path: PathBuf,
    sessions: Arc<Mutex<HashMap<SessionId, AcpSession>>>,
    terminals: AcpTerminalService,
}

struct AcpSession {
    profile: AcpProviderProfile,
    provider_session_id: String,
    _capabilities: AcpInitializeCapabilities,
    client: AcpClient,
}

impl AcpRunnerRuntime {
    #[must_use]
    pub fn new(workspace_path: PathBuf) -> Self {
        Self {
            workspace_path: workspace_path.clone(),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            terminals: AcpTerminalService::new(workspace_path),
        }
    }

    pub async fn create_session(
        &self,
        session_id: SessionId,
        profile: AcpProviderProfile,
    ) -> anyhow::Result<String> {
        let mut client = AcpClient::spawn(profile.clone(), self.workspace_path.clone())?;
        let initialize = client.initialize().await?;
        let capabilities = AcpInitializeCapabilities::from_initialize(&initialize);
        let response = client
            .request_response(
                "session/new",
                json!({
                    "cwd": self.workspace_path.display().to_string(),
                    "mcpServers": []
                }),
            )
            .await?;
        let provider_session_id = acp_session_id(&response)
            .ok_or_else(|| anyhow!("ACP session/new response did not include sessionId"))?
            .to_owned();
        client.provider_session_id = Some(provider_session_id.clone());
        self.sessions.lock().await.insert(
            session_id,
            AcpSession {
                profile,
                provider_session_id: provider_session_id.clone(),
                _capabilities: capabilities,
                client,
            },
        );
        Ok(provider_session_id)
    }

    pub async fn resume_session(
        &self,
        session_id: SessionId,
        profile: AcpProviderProfile,
        provider_session_id: String,
    ) -> anyhow::Result<String> {
        let mut client = AcpClient::spawn(profile.clone(), self.workspace_path.clone())?;
        let initialize = client.initialize().await?;
        let capabilities = AcpInitializeCapabilities::from_initialize(&initialize);
        let response = client
            .request_response(
                "session/load",
                json!({
                    "sessionId": provider_session_id,
                    "cwd": self.workspace_path.display().to_string()
                }),
            )
            .await?;
        let observed_session_id = acp_session_id(&response)
            .unwrap_or(provider_session_id.as_str())
            .to_owned();
        client.provider_session_id = Some(observed_session_id.clone());
        self.sessions.lock().await.insert(
            session_id,
            AcpSession {
                profile,
                provider_session_id: observed_session_id.clone(),
                _capabilities: capabilities,
                client,
            },
        );
        Ok(observed_session_id)
    }

    pub async fn discover_sessions(
        &self,
        profile: AcpProviderProfile,
    ) -> anyhow::Result<Vec<agenter_protocol::runner::DiscoveredSession>> {
        let mut client = AcpClient::spawn(profile.clone(), self.workspace_path.clone())?;
        let initialize = client.initialize().await?;
        let capabilities = AcpInitializeCapabilities::from_initialize(&initialize);
        if !capabilities.supports_session_list() {
            tracing::info!(
                provider_id = %profile.provider_id,
                "ACP provider does not advertise session/list; returning empty discovered session list"
            );
            client.shutdown().await.ok();
            return Ok(Vec::new());
        }
        let response = client
            .request_response(
                "session/list",
                json!({
                    "cwd": self.workspace_path.display().to_string(),
                    "cursor": null
                }),
            )
            .await?;
        let sessions = response
            .pointer("/result/sessions")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        let external_session_id =
                            string_at(item, &["/sessionId", "/id"])?.to_owned();
                        Some(agenter_protocol::runner::DiscoveredSession {
                            external_session_id,
                            title: string_at(item, &["/title", "/name"]).map(str::to_owned),
                            updated_at: string_at(item, &["/updatedAt", "/updated_at"])
                                .map(str::to_owned),
                            history_status:
                                agenter_protocol::runner::DiscoveredSessionHistoryStatus::Loaded,
                            history: Vec::new(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        client.shutdown().await.ok();
        Ok(sessions)
    }

    pub async fn shutdown_session(&self, session_id: SessionId) -> bool {
        self.sessions.lock().await.remove(&session_id).is_some()
    }

    pub async fn run_turn(
        &self,
        request: AcpTurnRequest,
        profile: AcpProviderProfile,
        event_sender: mpsc::UnboundedSender<AppEvent>,
        pending_approvals: Arc<Mutex<HashMap<ApprovalId, PendingAcpApproval>>>,
    ) -> anyhow::Result<()> {
        let mut session = {
            let mut sessions = self.sessions.lock().await;
            sessions.remove(&request.session_id)
        };
        if session.is_none() {
            if let Some(external_session_id) = request.external_session_id.clone() {
                self.resume_session(request.session_id, profile.clone(), external_session_id)
                    .await?;
            } else {
                let provider_session_id = self
                    .create_session(request.session_id, profile.clone())
                    .await?;
                tracing::debug!(%provider_session_id, "created ACP session for turn without external id");
            }
            session = self.sessions.lock().await.remove(&request.session_id);
        }
        let mut session = session.ok_or_else(|| anyhow!("ACP session was not available"))?;
        let prompt_request_id = session
            .client
            .send_request(
                "session/prompt",
                json!({
                    "sessionId": session.provider_session_id,
                    "prompt": [{"type": "text", "text": request.prompt}]
                }),
            )
            .await?;
        while let Some(message) = session.client.next_message().await? {
            if is_response_to(&message, &prompt_request_id) {
                event_sender
                    .send(normalize_acp_prompt_response(
                        request.session_id,
                        session.profile.provider_id.clone(),
                        &message,
                    ))
                    .ok();
                event_sender
                    .send(AppEvent::SessionStatusChanged(SessionStatusChangedEvent {
                        session_id: request.session_id,
                        status: SessionStatus::Idle,
                        reason: Some(format!("{} prompt completed.", session.profile.title)),
                    }))
                    .ok();
                self.sessions
                    .lock()
                    .await
                    .insert(request.session_id, session);
                return Ok(());
            }

            if let Some(id) = message.get("id").cloned() {
                let response = self
                    .handle_client_request(
                        request.session_id,
                        session.profile.provider_id.clone(),
                        &message,
                        event_sender.clone(),
                        pending_approvals.clone(),
                    )
                    .await?;
                session.client.respond(id, response).await?;
                continue;
            }

            for event in normalize_acp_message(
                request.session_id,
                session.profile.provider_id.clone(),
                &message,
            ) {
                event_sender.send(event).ok();
            }
        }
        Err(anyhow!("ACP provider exited before prompt completed"))
    }

    async fn handle_client_request(
        &self,
        session_id: SessionId,
        provider_id: AgentProviderId,
        message: &Value,
        event_sender: mpsc::UnboundedSender<AppEvent>,
        pending_approvals: Arc<Mutex<HashMap<ApprovalId, PendingAcpApproval>>>,
    ) -> anyhow::Result<Value> {
        match jsonrpc_method(message).unwrap_or_default() {
            "session/request_permission" => {
                let (approval_id, event) =
                    normalize_acp_permission_request(session_id, provider_id, message)
                        .ok_or_else(|| anyhow!("invalid ACP permission request"))?;
                let (sender, receiver) = oneshot::channel();
                pending_approvals
                    .lock()
                    .await
                    .insert(approval_id, PendingAcpApproval::new(session_id, sender));
                event_sender
                    .send(AppEvent::SessionStatusChanged(SessionStatusChangedEvent {
                        session_id,
                        status: SessionStatus::WaitingForApproval,
                        reason: Some("ACP provider is waiting for approval.".to_owned()),
                    }))
                    .ok();
                event_sender.send(event).ok();
                let answer = receiver
                    .await
                    .map_err(|_| anyhow!("ACP approval waiter was dropped"))?;
                let response = acp_permission_response(message, answer.decision);
                answer.acknowledged.send(Ok(())).ok();
                event_sender
                    .send(AppEvent::SessionStatusChanged(SessionStatusChangedEvent {
                        session_id,
                        status: SessionStatus::Running,
                        reason: Some("Approval answered.".to_owned()),
                    }))
                    .ok();
                Ok(response)
            }
            "fs/read_text_file" => {
                let content = AcpWorkspaceFileService::new(self.workspace_path.clone())
                    .read_text_file(message)
                    .await?;
                Ok(json!({ "content": content }))
            }
            "fs/write_text_file" => {
                let path = AcpWorkspaceFileService::new(self.workspace_path.clone())
                    .write_text_file(message)
                    .await?;
                event_sender
                    .send(AppEvent::FileChangeApplied(FileChangeEvent {
                        session_id,
                        path,
                        change_kind: FileChangeKind::Modify,
                        diff: None,
                        provider_payload: Some(message.clone()),
                    }))
                    .ok();
                Ok(json!({}))
            }
            "terminal/create" => {
                self.terminals
                    .create_terminal(session_id, message, event_sender)
                    .await
            }
            "terminal/output" => self.terminals.output(message).await,
            "terminal/wait_for_exit" => self.terminals.wait_for_exit(message).await,
            "terminal/kill" => self.terminals.kill(message).await,
            "terminal/release" => self.terminals.release(message).await,
            _ => Ok(json!({})),
        }
    }
}

#[derive(Debug)]
pub struct AcpTurnRequest {
    pub session_id: SessionId,
    pub external_session_id: Option<String>,
    pub prompt: String,
}

struct AcpClient {
    profile: AcpProviderProfile,
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    provider_session_id: Option<String>,
    recent_stderr: Arc<Mutex<VecDeque<String>>>,
}

impl AcpClient {
    fn spawn(profile: AcpProviderProfile, workspace_path: PathBuf) -> anyhow::Result<Self> {
        let (command, args) = profile.command_line(&workspace_path);
        tracing::info!(
            provider_id = %profile.provider_id,
            command,
            args = ?args,
            workspace = %workspace_path.display(),
            "spawning ACP provider"
        );
        let mut child = Command::new(&command)
            .args(&args)
            .current_dir(&workspace_path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .with_context(|| format!("failed to start `{command}` for ACP provider"))?;
        let stdin = child.stdin.take().context("ACP stdin was not piped")?;
        let stdout = child.stdout.take().context("ACP stdout was not piped")?;
        let recent_stderr = Arc::new(Mutex::new(VecDeque::new()));
        if let Some(stderr) = child.stderr.take() {
            let recent_stderr_for_task = recent_stderr.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::warn!(target: "acp-stderr", "{line}");
                    let mut guard = recent_stderr_for_task.lock().await;
                    guard.push_back(line);
                    while guard.len() > ACP_STDERR_EXCERPT_LINES {
                        guard.pop_front();
                    }
                }
            });
        }
        Ok(Self {
            profile,
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
            provider_session_id: None,
            recent_stderr,
        })
    }

    async fn initialize(&mut self) -> anyhow::Result<Value> {
        self.request_response(
            "initialize",
            json!({
                "protocolVersion": 1,
                "clientInfo": {
                    "name": "agenter-runner",
                    "title": "Agenter Runner",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "clientCapabilities": {
                    "fs": {"readTextFile": true, "writeTextFile": true},
                    "terminal": true
                }
            }),
        )
        .await
    }

    async fn request_response(&mut self, method: &str, params: Value) -> anyhow::Result<Value> {
        let request_id = self.send_request(method, params).await?;
        let response = timeout(self.profile.response_timeout(), async {
            loop {
                let Some(message) = self.next_message().await? else {
                    let stderr_excerpt = self.stderr_excerpt().await;
                    return Err(anyhow!(acp_exit_error(
                        &self.profile.provider_id,
                        method,
                        &stderr_excerpt
                    )));
                };
                if is_response_to(&message, &request_id) {
                    if let Some(error) = message.get("error") {
                        return Err(anyhow!(acp_error_response(
                            &self.profile.provider_id,
                            method,
                            error
                        )));
                    }
                    return Ok(message);
                }
            }
        })
        .await;
        let response = match response {
            Ok(response) => response?,
            Err(_) => {
                let stderr_excerpt = self.stderr_excerpt().await;
                return Err(anyhow!(acp_timeout_error(
                    &self.profile.provider_id,
                    method,
                    &stderr_excerpt
                )));
            }
        };
        Ok(response)
    }

    async fn stderr_excerpt(&self) -> String {
        self.recent_stderr
            .lock()
            .await
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }

    async fn send_request(&mut self, method: &str, params: Value) -> anyhow::Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        write_json(
            &mut self.stdin,
            &json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": method,
                "params": params
            }),
        )
        .await?;
        Ok(json!(id))
    }

    async fn respond(&mut self, id: Value, result: Value) -> anyhow::Result<()> {
        write_json(
            &mut self.stdin,
            &json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": result
            }),
        )
        .await
    }

    async fn next_message(&mut self) -> anyhow::Result<Option<Value>> {
        let mut line = String::new();
        if self.stdout.read_line(&mut line).await? == 0 {
            return Ok(None);
        }
        let message = serde_json::from_str::<Value>(line.trim())
            .with_context(|| format!("ACP provider emitted invalid JSON-RPC line: {line}"))?;
        if let Some(session_id) = acp_session_id(&message) {
            self.provider_session_id = Some(session_id.to_owned());
        }
        Ok(Some(message))
    }

    async fn shutdown(mut self) -> anyhow::Result<()> {
        self.stdin.shutdown().await.ok();
        match timeout(ACP_SHUTDOWN_TIMEOUT, self.child.wait()).await {
            Ok(result) => {
                result?;
            }
            Err(_) => {
                self.child.kill().await.ok();
            }
        }
        Ok(())
    }
}

async fn write_json(stdin: &mut ChildStdin, message: &Value) -> anyhow::Result<()> {
    let mut encoded = serde_json::to_vec(message)?;
    encoded.push(b'\n');
    stdin.write_all(&encoded).await?;
    stdin.flush().await?;
    Ok(())
}

pub fn normalize_acp_message(
    session_id: SessionId,
    provider_id: AgentProviderId,
    message: &Value,
) -> Vec<AppEvent> {
    if jsonrpc_method(message) != Some("session/update") {
        return Vec::new();
    }
    let update_type = acp_update_type(message).unwrap_or("session_update");
    match update_type {
        "agent_message_chunk" | "agent_message_delta" => {
            let Some(delta) = acp_content_text(message) else {
                return Vec::new();
            };
            vec![AppEvent::AgentMessageDelta(AgentMessageDeltaEvent {
                session_id,
                message_id: message_id(message),
                delta: delta.to_owned(),
                provider_payload: Some(message.clone()),
            })]
        }
        "agent_message" | "agent_message_complete" | "complete" | "done" => {
            vec![AppEvent::AgentMessageCompleted(MessageCompletedEvent {
                session_id,
                message_id: message_id(message),
                content: acp_content_text(message).map(str::to_owned),
                provider_payload: Some(message.clone()),
            })]
        }
        "plan" | "plan_update" => vec![AppEvent::PlanUpdated(PlanEvent {
            session_id,
            plan_id: string_at(message, &["/params/update/planId", "/params/update/id"])
                .map(str::to_owned),
            title: string_at(message, &["/params/update/title"]).map(str::to_owned),
            content: string_at(message, &["/params/update/content", "/params/update/text"])
                .map(str::to_owned),
            entries: plan_entries(message),
            append: bool_at(
                message,
                &["/params/update/partial", "/params/update/isPartial"],
            )
            .unwrap_or(false),
            provider_payload: Some(message.clone()),
        })],
        "tool_call" => vec![AppEvent::ToolStarted(tool_event(session_id, message))],
        "tool_call_update" => vec![AppEvent::ToolUpdated(tool_event(session_id, message))],
        "error" => vec![AppEvent::Error(AgentErrorEvent {
            session_id: Some(session_id),
            code: string_at(message, &["/params/update/code", "/params/code"]).map(str::to_owned),
            message: string_at(message, &["/params/update/message", "/params/message"])
                .unwrap_or("ACP provider reported an error")
                .to_owned(),
            provider_payload: Some(message.clone()),
        })],
        _ => vec![AppEvent::ProviderEvent(provider_event(
            session_id,
            provider_id,
            message,
            update_type,
        ))],
    }
}

#[allow(dead_code)]
pub mod acp_codec {
    use serde_json::Value;

    #[must_use]
    pub fn method(message: &Value) -> Option<&str> {
        super::jsonrpc_method(message)
    }
}

#[allow(dead_code)]
pub mod acp_reducer {
    use agenter_core::{
        AgentProviderId, NativeRef, SessionId, TurnId, TurnState, TurnStatus, UniversalEventKind,
        UniversalEventSource,
    };
    use serde_json::Value;
    use uuid::Uuid;

    use crate::agents::adapter::{
        legacy_events_to_adapter_events, AdapterEvent, AdapterUniversalEvent,
    };

    #[derive(Clone, Debug)]
    pub struct AcpReducerState {
        session_id: SessionId,
        provider_id: AgentProviderId,
        active_turn_id: Option<TurnId>,
    }

    impl AcpReducerState {
        #[must_use]
        pub fn new(session_id: SessionId, provider_id: AgentProviderId) -> Self {
            Self {
                session_id,
                provider_id,
                active_turn_id: None,
            }
        }

        #[must_use]
        pub fn start_prompt(&mut self, native_prompt_id: &str) -> AdapterEvent {
            let turn_id = self.prompt_turn_id(native_prompt_id);
            self.active_turn_id = Some(turn_id);
            AdapterEvent {
                universal: AdapterUniversalEvent {
                    session_id: Some(self.session_id),
                    turn_id: Some(turn_id),
                    item_id: None,
                    source: UniversalEventSource::Native,
                    native: Some(NativeRef {
                        protocol: "acp-stdio".to_owned(),
                        method: Some("session/prompt".to_owned()),
                        kind: Some(self.provider_id.to_string()),
                        native_id: Some(native_prompt_id.to_owned()),
                        summary: Some("ACP prompt started".to_owned()),
                        hash: None,
                        pointer: None,
                    }),
                    event: UniversalEventKind::TurnStarted {
                        turn: TurnState {
                            turn_id,
                            session_id: self.session_id,
                            status: TurnStatus::Running,
                            started_at: None,
                            completed_at: None,
                            model: None,
                            mode: None,
                        },
                    },
                },
                legacy: None,
            }
        }

        #[must_use]
        pub fn complete_prompt(&mut self, native_prompt_id: &str) -> Option<AdapterEvent> {
            let turn_id = self.active_turn_id.take()?;
            Some(AdapterEvent {
                universal: AdapterUniversalEvent {
                    session_id: Some(self.session_id),
                    turn_id: Some(turn_id),
                    item_id: None,
                    source: UniversalEventSource::Native,
                    native: Some(NativeRef {
                        protocol: "acp-stdio".to_owned(),
                        method: Some("session/prompt".to_owned()),
                        kind: Some(self.provider_id.to_string()),
                        native_id: Some(native_prompt_id.to_owned()),
                        summary: Some("ACP prompt completed".to_owned()),
                        hash: None,
                        pointer: None,
                    }),
                    event: UniversalEventKind::TurnCompleted {
                        turn: TurnState {
                            turn_id,
                            session_id: self.session_id,
                            status: TurnStatus::Completed,
                            started_at: None,
                            completed_at: None,
                            model: None,
                            mode: None,
                        },
                    },
                },
                legacy: None,
            })
        }

        #[must_use]
        pub fn reduce_native_message(&mut self, message: &Value) -> Vec<AdapterEvent> {
            let events = reduce_native_message(self.session_id, self.provider_id.clone(), message);
            if let Some(turn_id) = self.active_turn_id {
                events
                    .into_iter()
                    .map(|event| event.with_turn_id(turn_id))
                    .collect()
            } else {
                events
            }
        }

        fn prompt_turn_id(&self, native_prompt_id: &str) -> TurnId {
            TurnId::from_uuid(Uuid::new_v5(
                &Uuid::NAMESPACE_URL,
                format!(
                    "agenter:acp:turn:{}:{}:{native_prompt_id}",
                    self.provider_id, self.session_id
                )
                .as_bytes(),
            ))
        }
    }

    #[must_use]
    pub fn reduce_native_message(
        session_id: SessionId,
        provider_id: AgentProviderId,
        message: &Value,
    ) -> Vec<AdapterEvent> {
        let method = super::acp_codec::method(message);
        legacy_events_to_adapter_events(
            provider_id.clone(),
            "acp-stdio",
            method,
            super::normalize_acp_message(session_id, provider_id, message),
        )
    }
}

fn normalize_acp_prompt_response(
    session_id: SessionId,
    _provider_id: AgentProviderId,
    message: &Value,
) -> AppEvent {
    AppEvent::AgentMessageCompleted(MessageCompletedEvent {
        session_id,
        message_id: string_at(
            message,
            &[
                "/result/messageId",
                "/result/sessionId",
                "/result/turnId",
                "/id",
            ],
        )
        .unwrap_or("acp-prompt")
        .to_owned(),
        content: acp_content_text(message)
            .or_else(|| string_at(message, &["/result/content", "/result/message"]))
            .map(str::to_owned),
        provider_payload: Some(message.clone()),
    })
}

fn provider_event(
    session_id: SessionId,
    provider_id: AgentProviderId,
    message: &Value,
    category: &str,
) -> ProviderEvent {
    ProviderEvent {
        session_id,
        provider_id,
        event_id: string_at(message, &["/params/update/id", "/id"]).map(str::to_owned),
        method: jsonrpc_method(message)
            .unwrap_or("provider_event")
            .to_owned(),
        category: category.to_owned(),
        title: string_at(
            message,
            &["/params/update/title", "/params/title", "/method"],
        )
        .unwrap_or(category)
        .to_owned(),
        detail: string_at(
            message,
            &["/params/update/detail", "/params/update/message"],
        )
        .map(str::to_owned),
        status: string_at(message, &["/params/update/status", "/params/status"]).map(str::to_owned),
        provider_payload: Some(message.clone()),
    }
}

fn normalize_acp_permission_request(
    session_id: SessionId,
    provider_id: AgentProviderId,
    message: &Value,
) -> Option<(ApprovalId, AppEvent)> {
    if jsonrpc_method(message) != Some("session/request_permission") {
        return None;
    }
    let approval_id = ApprovalId::new();
    let title = string_at(
        message,
        &[
            "/params/toolCall/title",
            "/params/toolCall/name",
            "/params/title",
            "/params/name",
        ],
    )
    .unwrap_or("Approve ACP permission")
    .to_owned();
    let details = string_at(
        message,
        &[
            "/params/toolCall/content/0/text",
            "/params/content/0/text",
            "/params/description",
        ],
    )
    .map(str::to_owned)
    .or_else(|| serde_json::to_string(message.get("params").unwrap_or(&Value::Null)).ok());
    Some((
        approval_id,
        AppEvent::ApprovalRequested(ApprovalRequestEvent {
            session_id,
            approval_id,
            kind: ApprovalKind::ProviderSpecific,
            title,
            details: details.clone(),
            expires_at: None,
            presentation: Some(json!({
                "provider_id": provider_id,
                "options": message.pointer("/params/options").cloned().unwrap_or(Value::Null)
            })),
            resolution_state: None,
            resolving_decision: None,
            status: None,
            turn_id: None,
            item_id: None,
            options: agenter_core::ApprovalOption::canonical_defaults(),
            risk: None,
            subject: details.clone(),
            native_request_id: message.get("id").map(ToString::to_string),
            native_blocking: true,
            policy: None,
            provider_payload: Some(message.clone()),
        }),
    ))
}

pub fn acp_permission_response(message: &Value, decision: ApprovalDecision) -> Value {
    match decision {
        ApprovalDecision::Cancel => json!({"outcome": {"outcome": "cancelled"}}),
        ApprovalDecision::ProviderSpecific { payload } => payload,
        ApprovalDecision::Accept | ApprovalDecision::AcceptForSession => {
            selected_option_response(message, &["allow_once", "allow_always"], "allow_once")
        }
        ApprovalDecision::Decline => {
            selected_option_response(message, &["reject_once", "reject_always"], "reject_once")
        }
    }
}

fn selected_option_response(message: &Value, preferred_kinds: &[&str], fallback: &str) -> Value {
    let selected = message
        .pointer("/params/options")
        .and_then(Value::as_array)
        .and_then(|options| {
            preferred_kinds.iter().find_map(|kind| {
                options
                    .iter()
                    .find(|option| option.get("kind").and_then(Value::as_str) == Some(*kind))
            })
        })
        .or_else(|| {
            message
                .pointer("/params/options")
                .and_then(Value::as_array)
                .and_then(|options| options.last())
        })
        .and_then(|option| option.get("optionId"))
        .and_then(Value::as_str)
        .unwrap_or(fallback);
    json!({"outcome": {"outcome": "selected", "optionId": selected}})
}

#[derive(Clone)]
pub struct AcpWorkspaceFileService {
    workspace_path: PathBuf,
}

impl AcpWorkspaceFileService {
    #[must_use]
    pub fn new(workspace_path: PathBuf) -> Self {
        Self { workspace_path }
    }

    pub fn contained_path(&self, path: &str) -> anyhow::Result<PathBuf> {
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            return Err(anyhow!("ACP path must be absolute: {}", path.display()));
        }
        if !path.starts_with(&self.workspace_path) {
            return Err(anyhow!("ACP path is outside workspace: {}", path.display()));
        }
        Ok(path)
    }

    async fn read_text_file(&self, message: &Value) -> anyhow::Result<String> {
        let path = self.contained_path(
            string_at(message, &["/params/path", "/params/filePath"])
                .ok_or_else(|| anyhow!("ACP fs/read_text_file missing path"))?,
        )?;
        tokio::fs::read_to_string(path)
            .await
            .context("failed to read ACP text file")
    }

    async fn write_text_file(&self, message: &Value) -> anyhow::Result<String> {
        let raw_path = string_at(message, &["/params/path", "/params/filePath"])
            .ok_or_else(|| anyhow!("ACP fs/write_text_file missing path"))?;
        let path = self.contained_path(raw_path)?;
        let content = string_at(message, &["/params/content", "/params/text"]).unwrap_or("");
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&path, content).await?;
        Ok(raw_path.to_owned())
    }
}

#[derive(Clone)]
struct AcpTerminalService {
    workspace_path: PathBuf,
    terminals: Arc<Mutex<HashMap<String, AcpTerminalState>>>,
}

#[derive(Clone, Debug)]
struct AcpTerminalState {
    output: String,
    exit_code: Option<i32>,
    started_at: Instant,
}

impl AcpTerminalService {
    fn new(workspace_path: PathBuf) -> Self {
        Self {
            workspace_path,
            terminals: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn create_terminal(
        &self,
        session_id: SessionId,
        message: &Value,
        event_sender: mpsc::UnboundedSender<AppEvent>,
    ) -> anyhow::Result<Value> {
        let command = string_at(message, &["/params/command", "/params/cmd"])
            .ok_or_else(|| anyhow!("ACP terminal/create missing command"))?
            .to_owned();
        let terminal_id = format!("acp-terminal-{}", Uuid::new_v4());
        self.terminals.lock().await.insert(
            terminal_id.clone(),
            AcpTerminalState {
                output: String::new(),
                exit_code: None,
                started_at: Instant::now(),
            },
        );
        event_sender
            .send(AppEvent::CommandStarted(CommandEvent {
                session_id,
                command_id: terminal_id.clone(),
                command: command.clone(),
                cwd: Some(self.workspace_path.display().to_string()),
                source: Some("acp".to_owned()),
                process_id: None,
                actions: Vec::new(),
                provider_payload: Some(message.clone()),
            }))
            .ok();

        let terminals = self.terminals.clone();
        let terminal_id_for_task = terminal_id.clone();
        let workspace = self.workspace_path.clone();
        tokio::spawn(async move {
            let output = Command::new("sh")
                .arg("-lc")
                .arg(&command)
                .current_dir(&workspace)
                .output()
                .await;
            let (text, exit_code) = match output {
                Ok(output) => {
                    let mut text = String::new();
                    text.push_str(&String::from_utf8_lossy(&output.stdout));
                    text.push_str(&String::from_utf8_lossy(&output.stderr));
                    (text, output.status.code().unwrap_or(1))
                }
                Err(error) => (error.to_string(), 1),
            };
            let duration_ms = {
                let mut guard = terminals.lock().await;
                if let Some(state) = guard.get_mut(&terminal_id_for_task) {
                    state.output = text.clone();
                    state.exit_code = Some(exit_code);
                    Some(state.started_at.elapsed().as_millis() as u64)
                } else {
                    None
                }
            };
            if !text.is_empty() {
                event_sender
                    .send(AppEvent::CommandOutputDelta(CommandOutputEvent {
                        session_id,
                        command_id: terminal_id_for_task.clone(),
                        stream: CommandOutputStream::Stdout,
                        delta: text,
                        provider_payload: None,
                    }))
                    .ok();
            }
            event_sender
                .send(AppEvent::CommandCompleted(CommandCompletedEvent {
                    session_id,
                    command_id: terminal_id_for_task,
                    exit_code: Some(exit_code),
                    duration_ms,
                    success: exit_code == 0,
                    provider_payload: None,
                }))
                .ok();
        });

        Ok(json!({ "terminalId": terminal_id }))
    }

    async fn output(&self, message: &Value) -> anyhow::Result<Value> {
        let terminal_id = string_at(message, &["/params/terminalId"])
            .ok_or_else(|| anyhow!("ACP terminal/output missing terminalId"))?;
        let guard = self.terminals.lock().await;
        let Some(state) = guard.get(terminal_id) else {
            return Ok(json!({
                "output": "",
                "truncated": false,
                "exitStatus": {"exitCode": 1}
            }));
        };
        Ok(json!({
            "output": state.output,
            "truncated": false,
            "exitStatus": state.exit_code.map(|exit_code| json!({"exitCode": exit_code}))
        }))
    }

    async fn wait_for_exit(&self, message: &Value) -> anyhow::Result<Value> {
        for _ in 0..200 {
            if let Some(exit_code) = self
                .terminal_exit_code(string_at(message, &["/params/terminalId"]).unwrap_or(""))
                .await
            {
                return Ok(json!({"exitCode": exit_code}));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Ok(json!({"exitCode": 1}))
    }

    async fn terminal_exit_code(&self, terminal_id: &str) -> Option<i32> {
        self.terminals
            .lock()
            .await
            .get(terminal_id)
            .and_then(|state| state.exit_code)
    }

    async fn kill(&self, _message: &Value) -> anyhow::Result<Value> {
        Ok(json!({}))
    }

    async fn release(&self, message: &Value) -> anyhow::Result<Value> {
        if let Some(terminal_id) = string_at(message, &["/params/terminalId"]) {
            self.terminals.lock().await.remove(terminal_id);
        }
        Ok(json!({}))
    }
}

fn tool_event(session_id: SessionId, message: &Value) -> ToolEvent {
    ToolEvent {
        session_id,
        tool_call_id: string_at(
            message,
            &[
                "/params/update/toolCallId",
                "/params/update/id",
                "/params/update/toolCall/toolCallId",
            ],
        )
        .unwrap_or("acp-tool")
        .to_owned(),
        name: string_at(
            message,
            &[
                "/params/update/name",
                "/params/update/toolCall/name",
                "/params/update/kind",
            ],
        )
        .unwrap_or("tool")
        .to_owned(),
        title: string_at(
            message,
            &["/params/update/title", "/params/update/toolCall/title"],
        )
        .map(str::to_owned),
        input: message.pointer("/params/update/input").cloned(),
        output: message.pointer("/params/update/output").cloned(),
        provider_payload: Some(message.clone()),
    }
}

fn plan_entries(message: &Value) -> Vec<PlanEntry> {
    message
        .pointer("/params/update/entries")
        .or_else(|| message.pointer("/params/update/steps"))
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    Some(PlanEntry {
                        label: string_at(entry, &["/label", "/title", "/content"])?.to_owned(),
                        status: match string_at(entry, &["/status"]).unwrap_or("pending") {
                            "in_progress" | "running" => PlanEntryStatus::InProgress,
                            "completed" | "complete" | "done" => PlanEntryStatus::Completed,
                            "failed" | "error" => PlanEntryStatus::Failed,
                            "cancelled" | "canceled" => PlanEntryStatus::Cancelled,
                            _ => PlanEntryStatus::Pending,
                        },
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn acp_update_type(message: &Value) -> Option<&str> {
    string_at(
        message,
        &[
            "/params/update/sessionUpdate",
            "/params/update/type",
            "/params/sessionUpdate",
        ],
    )
}

fn acp_content_text(message: &Value) -> Option<&str> {
    string_at(
        message,
        &[
            "/params/update/content/text",
            "/params/update/content",
            "/params/content/text",
            "/params/content",
            "/result/content/text",
            "/result/message/text",
        ],
    )
}

fn message_id(message: &Value) -> String {
    string_at(
        message,
        &[
            "/params/update/messageId",
            "/params/update/id",
            "/params/messageId",
            "/params/sessionId",
        ],
    )
    .unwrap_or("acp-message")
    .to_owned()
}

fn acp_session_id(message: &Value) -> Option<&str> {
    string_at(
        message,
        &["/result/sessionId", "/params/sessionId", "/sessionId"],
    )
}

fn jsonrpc_method(message: &Value) -> Option<&str> {
    message.get("method")?.as_str()
}

fn is_response_to(message: &Value, request_id: &Value) -> bool {
    message.get("id") == Some(request_id)
        && (message.get("result").is_some() || message.get("error").is_some())
}

fn string_at<'a>(message: &'a Value, pointers: &[&str]) -> Option<&'a str> {
    pointers
        .iter()
        .find_map(|pointer| message.pointer(pointer).and_then(Value::as_str))
}

fn bool_at(message: &Value, pointers: &[&str]) -> Option<bool> {
    pointers
        .iter()
        .find_map(|pointer| message.pointer(pointer).and_then(Value::as_bool))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use agenter_core::{AgentProviderId, SessionId};
    use serde_json::json;

    use super::*;

    #[test]
    fn acp_profiles_cover_qwen_gemini_and_opencode_commands() {
        let profiles = AcpProviderProfile::all();

        assert_eq!(
            profiles
                .iter()
                .map(|profile| profile.provider_id.as_str())
                .collect::<Vec<_>>(),
            vec!["qwen", "gemini", "opencode"]
        );
        assert_eq!(
            profiles[0].command_line(&PathBuf::from("/work/project")),
            (
                "qwen".to_owned(),
                vec![
                    "--acp".to_owned(),
                    "--approval-mode".to_owned(),
                    "default".to_owned()
                ]
            )
        );
        assert_eq!(
            profiles[1].command_line(&PathBuf::from("/work/project")),
            ("gemini".to_owned(), vec!["--acp".to_owned()])
        );
        assert_eq!(
            profiles[2].command_line(&PathBuf::from("/work/project")),
            (
                "opencode".to_owned(),
                vec![
                    "acp".to_owned(),
                    "--cwd".to_owned(),
                    "/work/project".to_owned()
                ]
            )
        );
    }

    #[test]
    fn initialize_capabilities_map_to_agenter_capabilities() {
        let initialize = json!({
            "result": {
                "agentCapabilities": {
                    "loadSession": true,
                    "sessionCapabilities": {"list": {}, "resume": {}},
                    "promptCapabilities": {"image": true, "embeddedContext": true},
                    "mcpCapabilities": {"http": true, "sse": true}
                }
            }
        });

        let capabilities =
            AcpInitializeCapabilities::from_initialize(&initialize).to_agent_capabilities();

        assert!(capabilities.streaming);
        assert!(capabilities.session_resume);
        assert!(capabilities.session_history);
        assert!(capabilities.approvals);
        assert!(capabilities.file_changes);
        assert!(capabilities.command_execution);
        assert!(capabilities.plan_updates);
        assert!(!capabilities.interrupt);
    }

    #[test]
    fn gemini_initialize_load_session_does_not_imply_list_or_resume() {
        let initialize = json!({
            "result": {
                "agentCapabilities": {
                    "loadSession": true,
                    "promptCapabilities": {
                        "image": true,
                        "audio": true,
                        "embeddedContext": true
                    },
                    "mcpCapabilities": {"http": true, "sse": true}
                }
            }
        });

        let acp_capabilities = AcpInitializeCapabilities::from_initialize(&initialize);
        let agenter_capabilities = acp_capabilities.to_agent_capabilities();

        assert!(acp_capabilities.load_session);
        assert!(!acp_capabilities.session_list);
        assert!(!acp_capabilities.session_resume);
        assert!(!acp_capabilities.session_fork);
        assert!(agenter_capabilities.session_resume);
        assert!(!agenter_capabilities.session_history);
    }

    #[test]
    fn qwen_initialize_preserves_list_and_resume() {
        let initialize = json!({
            "result": {
                "agentCapabilities": {
                    "loadSession": true,
                    "sessionCapabilities": {"list": {}, "resume": {}}
                }
            }
        });

        let acp_capabilities = AcpInitializeCapabilities::from_initialize(&initialize);
        let agenter_capabilities = acp_capabilities.to_agent_capabilities();

        assert!(acp_capabilities.load_session);
        assert!(acp_capabilities.session_list);
        assert!(acp_capabilities.session_resume);
        assert!(agenter_capabilities.session_resume);
        assert!(agenter_capabilities.session_history);
    }

    #[test]
    fn gemini_profile_uses_longer_response_timeout() {
        assert_eq!(
            AcpProviderProfile::gemini().response_timeout(),
            Duration::from_secs(60)
        );
        assert_eq!(
            AcpProviderProfile::qwen().response_timeout(),
            Duration::from_secs(20)
        );
    }

    #[test]
    fn timeout_error_mentions_provider_and_stderr_excerpt() {
        let message = acp_timeout_error(
            &AgentProviderId::from(AgentProviderId::GEMINI),
            "initialize",
            "Error authenticating: listen EPERM: operation not permitted 0.0.0.0",
        );

        assert!(message.contains("provider `gemini`"));
        assert!(message.contains("initialize"));
        assert!(message.contains("recent stderr"));
        assert!(message.contains("listen EPERM"));
        assert!(message.contains("authenticate and trust the workspace locally"));
    }

    #[test]
    fn unknown_acp_session_update_becomes_provider_event() {
        let message = json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": "native-1",
                "update": {
                    "sessionUpdate": "new_provider_thing",
                    "title": "Provider thing"
                }
            }
        });

        let events = normalize_acp_message(
            SessionId::nil(),
            AgentProviderId::from("opencode"),
            &message,
        );

        assert_eq!(events.len(), 1);
        let agenter_core::AppEvent::ProviderEvent(event) = &events[0] else {
            panic!("expected provider event fallback");
        };
        assert_eq!(event.provider_id.as_str(), "opencode");
        assert_eq!(event.method, "session/update");
        assert_eq!(event.category, "new_provider_thing");
        assert_eq!(event.title, "Provider thing");
    }

    #[test]
    fn acp_semantic_reducer_preserves_legacy_projection() {
        let session_id = SessionId::nil();
        let provider_id = AgentProviderId::from(AgentProviderId::QWEN);
        let message = json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": "native-1",
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "id": "chunk-1",
                    "content": "hello"
                }
            }
        });

        let legacy = normalize_acp_message(session_id, provider_id.clone(), &message);
        let semantic = super::acp_reducer::reduce_native_message(session_id, provider_id, &message);

        assert_eq!(semantic.len(), legacy.len());
        assert_eq!(semantic[0].legacy_projection(), Some(&legacy[0]));
        assert_eq!(semantic[0].universal.session_id, Some(session_id));
        let native = semantic[0].universal.native.as_ref().expect("native ref");
        assert_eq!(native.protocol, "acp-stdio");
        assert_eq!(native.method.as_deref(), Some("session/update"));
        assert_eq!(native.kind.as_deref(), Some(AgentProviderId::QWEN));
        assert_eq!(native.native_id.as_deref(), Some("chunk-1"));
        assert_eq!(native.summary.as_deref(), Some("assistant message delta"));
        let agenter_core::UniversalEventKind::ContentDelta {
            block_id,
            kind,
            delta,
        } = &semantic[0].universal.event
        else {
            panic!("expected universal content delta");
        };
        assert_eq!(block_id, "text-chunk-1");
        assert_eq!(kind, &Some(agenter_core::ContentBlockKind::Text));
        assert_eq!(delta, "hello");
    }

    #[test]
    fn acp_stage6_golden_trace_attaches_updates_to_active_prompt_turn() {
        let session_id = SessionId::nil();
        let provider_id = AgentProviderId::from(AgentProviderId::QWEN);
        let trace: Vec<Value> =
            serde_json::from_str(include_str!("../../tests/fixtures/acp_stage6_trace.json"))
                .expect("fixture parses");
        let mut reducer = super::acp_reducer::AcpReducerState::new(session_id, provider_id.clone());
        let turn = reducer.start_prompt("prompt-1");
        let turn_id = turn.universal.turn_id.expect("turn id");
        let semantic = trace
            .iter()
            .flat_map(|message| reducer.reduce_native_message(message))
            .collect::<Vec<_>>();

        let agenter_core::UniversalEventKind::TurnStarted { turn: started } = &turn.universal.event
        else {
            panic!("expected turn started");
        };
        assert_eq!(started.turn_id, turn_id);
        assert!(semantic
            .iter()
            .all(|event| event.universal.turn_id == Some(turn_id)));
        let content = semantic
            .iter()
            .find_map(|event| match &event.universal.event {
                agenter_core::UniversalEventKind::ContentDelta {
                    block_id,
                    kind,
                    delta,
                } => Some((block_id, kind, delta)),
                _ => None,
            })
            .expect("message delta");
        assert_eq!(content.0, "text-msg-1");
        assert_eq!(content.1, &Some(agenter_core::ContentBlockKind::Text));
        assert_eq!(content.2, "hello");
        let tool = semantic
            .iter()
            .find_map(|event| match &event.universal.event {
                agenter_core::UniversalEventKind::ItemCreated { item } => Some(item),
                _ => None,
            })
            .expect("tool item");
        assert_eq!(tool.turn_id, Some(turn_id));
        assert_eq!(
            tool.content[0].kind,
            agenter_core::ContentBlockKind::ToolCall
        );
        assert_eq!(tool.content[0].text.as_deref(), Some("Read file"));
        let plan = semantic
            .iter()
            .find_map(|event| match &event.universal.event {
                agenter_core::UniversalEventKind::PlanUpdated { plan } => Some(plan),
                _ => None,
            })
            .expect("plan");
        assert_eq!(plan.turn_id, Some(turn_id));
        assert_eq!(plan.entries[0].label, "Inspect");
        assert_eq!(plan.entries[1].status, PlanEntryStatus::InProgress);
        assert!(!semantic.iter().any(|event| matches!(
            &event.universal.event,
            agenter_core::UniversalEventKind::ArtifactCreated { .. }
        )));

        let permission = json!({
            "jsonrpc": "2.0",
            "id": "permission-1",
            "method": "session/request_permission",
            "params": {
                "toolCall": {"toolCallId": "tool-1", "name": "write_file"},
                "options": [{"optionId": "allow_once", "kind": "allow_once"}]
            }
        });
        let (_approval_id, legacy) =
            normalize_acp_permission_request(session_id, provider_id.clone(), &permission)
                .expect("permission");
        let approval = crate::agents::adapter::AdapterEvent::from_legacy(
            provider_id,
            "acp-stdio",
            Some("session/request_permission"),
            legacy,
        )
        .with_turn_id(turn_id);
        assert!(matches!(
            approval.universal.event,
            agenter_core::UniversalEventKind::ApprovalRequested { .. }
        ));
        let completed = reducer.complete_prompt("prompt-1").expect("completion");
        assert!(matches!(
            completed.universal.event,
            agenter_core::UniversalEventKind::TurnCompleted { .. }
        ));
        let later = reducer.reduce_native_message(&trace[0]);
        assert!(later
            .iter()
            .all(|event| event.universal.turn_id != Some(turn_id)));
    }

    #[test]
    fn acp_stage10_provider_traces_share_prompt_plan_permission_shape() {
        let trace: Value =
            serde_json::from_str(include_str!("../../tests/fixtures/acp_stage10_trace.json"))
                .expect("fixture parses");
        let cases = trace.as_object().expect("provider trace object");

        for (provider, messages) in cases {
            let provider_id = AgentProviderId::from(provider.as_str());
            let session_id = SessionId::nil();
            let mut reducer =
                super::acp_reducer::AcpReducerState::new(session_id, provider_id.clone());
            let turn = reducer.start_prompt("prompt-1");
            let turn_id = turn.universal.turn_id.expect("turn id");
            let semantic = messages
                .as_array()
                .expect("provider messages")
                .iter()
                .flat_map(|message| reducer.reduce_native_message(message))
                .collect::<Vec<_>>();

            assert!(semantic.iter().all(|event| {
                event.universal.session_id == Some(session_id)
                    && event.universal.turn_id == Some(turn_id)
            }));
            assert!(semantic.iter().any(|event| matches!(
                event.universal.event,
                agenter_core::UniversalEventKind::PlanUpdated { .. }
            )));
            assert!(semantic.iter().any(|event| matches!(
                event.universal.event,
                agenter_core::UniversalEventKind::ItemCreated { .. }
                    | agenter_core::UniversalEventKind::ContentDelta { .. }
            )));

            let permission = messages
                .as_array()
                .expect("provider messages")
                .iter()
                .find(|message| jsonrpc_method(message) == Some("session/request_permission"))
                .expect("permission request");
            let (_approval_id, legacy) =
                normalize_acp_permission_request(session_id, provider_id.clone(), permission)
                    .expect("permission should normalize");
            let approval = crate::agents::adapter::AdapterEvent::from_legacy(
                provider_id,
                "acp-stdio",
                Some("session/request_permission"),
                legacy,
            )
            .with_turn_id(turn_id);
            let agenter_core::UniversalEventKind::ApprovalRequested { approval } =
                approval.universal.event
            else {
                panic!("expected universal approval");
            };
            assert!(
                approval
                    .options
                    .iter()
                    .any(|option| option.option_id == "cancel_turn"),
                "{provider} permission should expose terminal cancel semantics"
            );
        }
    }

    #[test]
    fn acp_prompt_turn_ids_include_session_and_provider_scope() {
        let same_prompt = "prompt-1";
        let mut first = super::acp_reducer::AcpReducerState::new(
            SessionId::nil(),
            AgentProviderId::from(AgentProviderId::QWEN),
        );
        let mut second = super::acp_reducer::AcpReducerState::new(
            SessionId::new(),
            AgentProviderId::from(AgentProviderId::QWEN),
        );
        let mut third = super::acp_reducer::AcpReducerState::new(
            SessionId::nil(),
            AgentProviderId::from("opencode"),
        );

        let first_id = first.start_prompt(same_prompt).universal.turn_id;
        let second_id = second.start_prompt(same_prompt).universal.turn_id;
        let third_id = third.start_prompt(same_prompt).universal.turn_id;

        assert_ne!(first_id, second_id);
        assert_ne!(first_id, third_id);
    }

    #[test]
    fn acp_stage6_unknown_native_update_stays_safe_native_unknown() {
        let session_id = SessionId::nil();
        let provider_id = AgentProviderId::from(AgentProviderId::QWEN);
        let message = json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": "native-1",
                "update": {
                    "sessionUpdate": "future_native_update",
                    "title": "Future native update",
                    "secret": "must-not-be-copied"
                }
            }
        });
        let semantic = super::acp_reducer::reduce_native_message(session_id, provider_id, &message);

        assert_eq!(semantic.len(), 1);
        let agenter_core::UniversalEventKind::NativeUnknown { summary } =
            &semantic[0].universal.event
        else {
            panic!("expected unknown native event");
        };
        assert_eq!(summary.as_deref(), Some("provider event"));
        assert_eq!(
            semantic[0]
                .universal
                .native
                .as_ref()
                .and_then(|native| native.method.as_deref()),
            Some("session/update")
        );
    }

    #[test]
    fn workspace_paths_must_stay_inside_workspace() {
        let service = AcpWorkspaceFileService::new(PathBuf::from("/work/project"));

        assert_eq!(
            service
                .contained_path("/work/project/src/main.rs")
                .expect("contained path"),
            PathBuf::from("/work/project/src/main.rs")
        );
        assert!(service.contained_path("/work/other/src/main.rs").is_err());
        assert!(service.contained_path("src/main.rs").is_err());
    }

    #[test]
    fn permission_decision_selects_provider_option() {
        let request = json!({
            "id": "permission-1",
            "method": "session/request_permission",
            "params": {
                "options": [
                    {"optionId": "allow", "kind": "allow_once"},
                    {"optionId": "reject", "kind": "reject_once"}
                ]
            }
        });

        assert_eq!(
            acp_permission_response(&request, agenter_core::ApprovalDecision::Accept),
            json!({"outcome": {"outcome": "selected", "optionId": "allow"}})
        );
        assert_eq!(
            acp_permission_response(&request, agenter_core::ApprovalDecision::Decline),
            json!({"outcome": {"outcome": "selected", "optionId": "reject"}})
        );
    }
}

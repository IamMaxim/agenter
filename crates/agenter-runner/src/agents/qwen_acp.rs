use std::{collections::HashMap, path::PathBuf, time::Duration};

use agenter_core::{
    AgentErrorEvent, AgentMessageDeltaEvent, AppEvent, ApprovalDecision, ApprovalId, ApprovalKind,
    ApprovalRequestEvent, MessageCompletedEvent, SessionId, SessionStatus,
    SessionStatusChangedEvent,
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

#[derive(Debug)]
pub struct QwenTurnRequest {
    pub session_id: SessionId,
    pub workspace_path: PathBuf,
    pub external_session_id: Option<String>,
    pub prompt: String,
}

#[derive(Debug)]
pub struct PendingQwenApproval {
    pub response: oneshot::Sender<ApprovalDecision>,
}

#[derive(Debug)]
pub struct QwenAcp {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    provider_session_id: Option<String>,
}

impl QwenAcp {
    pub fn spawn(workspace_path: PathBuf) -> anyhow::Result<Self> {
        tracing::info!(workspace = %workspace_path.display(), "spawning qwen acp");
        let mut child = Command::new("qwen")
            .args(["--acp", "--approval-mode", "default"])
            .current_dir(&workspace_path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .context("failed to start `qwen --acp --approval-mode default`")?;

        let stdin = child.stdin.take().context("qwen stdin was not piped")?;
        let stdout = child.stdout.take().context("qwen stdout was not piped")?;
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::warn!(target: "qwen-stderr", "{line}");
                }
            });
        }

        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
            provider_session_id: None,
        })
    }

    pub async fn initialize_and_start_session(
        &mut self,
        request: &QwenTurnRequest,
    ) -> anyhow::Result<()> {
        tracing::debug!(
            session_id = %request.session_id,
            has_external_session_id = request.external_session_id.is_some(),
            "initializing qwen acp"
        );
        self.send_request(
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
        .await?;

        if let Some(session_id) = &request.external_session_id {
            self.provider_session_id = Some(session_id.clone());
            tracing::info!(session_id = %request.session_id, provider_session_id = %session_id, "resuming qwen session");
            self.send_request(
                "session/resume",
                json!({
                    "sessionId": session_id,
                    "cwd": request.workspace_path,
                    "mcpServers": []
                }),
            )
            .await?;
        } else {
            tracing::info!(session_id = %request.session_id, "starting qwen session");
            self.send_request(
                "session/new",
                json!({
                    "cwd": request.workspace_path,
                    "mcpServers": []
                }),
            )
            .await?;
        }

        Ok(())
    }

    pub async fn send_prompt(&mut self, request: &QwenTurnRequest) -> anyhow::Result<Value> {
        let Some(session_id) = &self.provider_session_id else {
            return Err(anyhow!(
                "qwen session id was not observed before prompt start"
            ));
        };
        tracing::info!(
            session_id = %request.session_id,
            provider_session_id = %session_id,
            prompt_len = request.prompt.len(),
            payload_preview = agenter_core::logging::payload_preview(
                &json!({"prompt": &request.prompt}),
                agenter_core::logging::payload_logging_enabled()
            ).as_deref(),
            "sending qwen prompt"
        );
        let request_id = self
            .send_request(
                "session/prompt",
                json!({
                    "sessionId": session_id,
                    "prompt": [{"type": "text", "text": request.prompt}]
                }),
            )
            .await?;
        Ok(request_id)
    }

    pub async fn next_message(&mut self) -> anyhow::Result<Option<Value>> {
        let mut line = String::new();
        if self.stdout.read_line(&mut line).await? == 0 {
            return Ok(None);
        }
        let message = serde_json::from_str::<Value>(line.trim())
            .with_context(|| format!("qwen emitted invalid JSON-RPC line: {line}"))?;
        tracing::debug!(
            method = message.get("method").and_then(serde_json::Value::as_str),
            id = ?message.get("id"),
            payload_preview = agenter_core::logging::payload_preview(
                &message,
                agenter_core::logging::payload_logging_enabled()
            ).as_deref(),
            "received qwen json-rpc message"
        );
        if let Some(session_id) = qwen_session_id(&message) {
            self.provider_session_id = Some(session_id.to_owned());
            tracing::info!(provider_session_id = %session_id, "observed qwen session id");
        }
        Ok(Some(message))
    }

    pub async fn respond(&mut self, id: Value, result: Value) -> anyhow::Result<()> {
        tracing::debug!(id = ?id, "sending qwen json-rpc response");
        write_json(
            &mut self.stdin,
            &json!({
                "id": id,
                "result": result
            }),
        )
        .await
    }

    async fn send_request(&mut self, method: &str, params: Value) -> anyhow::Result<Value> {
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
            "sending qwen json-rpc request"
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
        Ok(json!(id))
    }

    pub async fn shutdown(mut self) -> anyhow::Result<()> {
        tracing::debug!("shutting down qwen acp");
        self.stdin.shutdown().await.ok();
        match timeout(SHUTDOWN_TIMEOUT, self.child.wait()).await {
            Ok(result) => {
                result?;
                tracing::debug!("qwen acp exited");
            }
            Err(_) => {
                self.child.kill().await.ok();
                tracing::warn!("killed qwen acp after shutdown timeout");
            }
        }
        Ok(())
    }
}

pub async fn run_qwen_turn(
    request: QwenTurnRequest,
    event_sender: mpsc::UnboundedSender<AppEvent>,
    pending_approvals: std::sync::Arc<tokio::sync::Mutex<HashMap<ApprovalId, PendingQwenApproval>>>,
) -> anyhow::Result<()> {
    tracing::info!(session_id = %request.session_id, "qwen turn task started");
    let mut server = QwenAcp::spawn(request.workspace_path.clone())?;
    server.initialize_and_start_session(&request).await?;

    while server.provider_session_id.is_none() {
        let Some(message) = server.next_message().await? else {
            return Err(anyhow!("qwen exited before returning a session id"));
        };
        if let Some(response) = inert_client_response(&message) {
            tracing::debug!(id = ?message.get("id"), "answering qwen client request while waiting for session id");
            server.respond(message["id"].clone(), response).await?;
        }
    }

    let prompt_request_id = server.send_prompt(&request).await?;
    while let Some(message) = server.next_message().await? {
        if let Some(response) = inert_client_response(&message) {
            tracing::debug!(id = ?message.get("id"), "answering qwen client request");
            server.respond(message["id"].clone(), response).await?;
            continue;
        }

        if is_response_to(&message, &prompt_request_id) {
            let event = normalize_qwen_prompt_response(request.session_id, &message);
            event_sender.send(event).ok();
            event_sender
                .send(AppEvent::SessionStatusChanged(SessionStatusChangedEvent {
                    session_id: request.session_id,
                    status: SessionStatus::Completed,
                    reason: Some("Qwen prompt completed.".to_owned()),
                }))
                .ok();
            tracing::info!(session_id = %request.session_id, "qwen prompt completed");
            server.shutdown().await?;
            return Ok(());
        }

        if let Some((approval_id, native_request_id, event)) =
            normalize_qwen_permission_request(request.session_id, &message)
        {
            let (sender, receiver) = oneshot::channel();
            pending_approvals
                .lock()
                .await
                .insert(approval_id, PendingQwenApproval { response: sender });
            event_sender
                .send(AppEvent::SessionStatusChanged(SessionStatusChangedEvent {
                    session_id: request.session_id,
                    status: SessionStatus::WaitingForApproval,
                    reason: Some("Qwen is waiting for approval.".to_owned()),
                }))
                .ok();
            tracing::info!(
                session_id = %request.session_id,
                %approval_id,
                native_request_id = ?native_request_id,
                "qwen permission request pending"
            );
            event_sender.send(event).ok();
            if let Ok(decision) = receiver.await {
                server
                    .respond(
                        native_request_id,
                        qwen_permission_response(&message, decision),
                    )
                    .await?;
                event_sender
                    .send(AppEvent::SessionStatusChanged(SessionStatusChangedEvent {
                        session_id: request.session_id,
                        status: SessionStatus::Running,
                        reason: Some("Approval answered.".to_owned()),
                    }))
                    .ok();
            }
            continue;
        }

        for event in normalize_qwen_message(request.session_id, &message) {
            let completed = matches!(event, AppEvent::AgentMessageCompleted(_));
            event_sender.send(event).ok();
            if completed {
                event_sender
                    .send(AppEvent::SessionStatusChanged(SessionStatusChangedEvent {
                        session_id: request.session_id,
                        status: SessionStatus::Completed,
                        reason: Some("Qwen turn completed.".to_owned()),
                    }))
                    .ok();
                tracing::info!(session_id = %request.session_id, "qwen turn completed");
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

pub fn normalize_qwen_message(session_id: SessionId, message: &Value) -> Vec<AppEvent> {
    if jsonrpc_method(message) != Some("session/update") {
        return Vec::new();
    }
    let update_type = string_at(
        message,
        &[
            "/params/update/sessionUpdate",
            "/params/update/type",
            "/params/sessionUpdate",
        ],
    );
    match update_type {
        Some("agent_message_chunk" | "agent_message_delta") => {
            let Some(delta) = string_at(
                message,
                &[
                    "/params/update/content/text",
                    "/params/update/content",
                    "/params/content/text",
                    "/params/content",
                ],
            ) else {
                return Vec::new();
            };
            vec![AppEvent::AgentMessageDelta(AgentMessageDeltaEvent {
                session_id,
                message_id: message_id(message),
                delta: delta.to_owned(),
                provider_payload: Some(message.clone()),
            })]
        }
        Some("agent_message" | "agent_message_complete" | "complete" | "done") => {
            vec![AppEvent::AgentMessageCompleted(MessageCompletedEvent {
                session_id,
                message_id: message_id(message),
                content: string_at(
                    message,
                    &[
                        "/params/update/content/text",
                        "/params/update/content",
                        "/params/content/text",
                        "/params/content",
                    ],
                )
                .map(str::to_owned),
                provider_payload: Some(message.clone()),
            })]
        }
        Some("error") => vec![AppEvent::Error(AgentErrorEvent {
            session_id: Some(session_id),
            code: string_at(message, &["/params/update/code", "/params/code"]).map(str::to_owned),
            message: string_at(message, &["/params/update/message", "/params/message"])
                .unwrap_or("Qwen reported an error")
                .to_owned(),
            provider_payload: Some(message.clone()),
        })],
        _ => Vec::new(),
    }
}

pub fn normalize_qwen_prompt_response(session_id: SessionId, message: &Value) -> AppEvent {
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
        .unwrap_or("qwen-prompt")
        .to_owned(),
        content: string_at(
            message,
            &[
                "/result/content/text",
                "/result/content",
                "/result/message/text",
                "/result/message",
            ],
        )
        .map(str::to_owned),
        provider_payload: Some(message.clone()),
    })
}

pub fn normalize_qwen_permission_request(
    session_id: SessionId,
    message: &Value,
) -> Option<(ApprovalId, Value, AppEvent)> {
    if jsonrpc_method(message) != Some("session/request_permission") {
        return None;
    }
    let native_request_id = message.get("id")?.clone();
    let approval_id = ApprovalId::new();
    let details = string_at(
        message,
        &[
            "/params/toolCall/name",
            "/params/toolCall/title",
            "/params/toolCall/toolCallId",
            "/params/description",
        ],
    )
    .map(str::to_owned)
    .or_else(|| serde_json::to_string(message.get("params").unwrap_or(&Value::Null)).ok());
    let event = AppEvent::ApprovalRequested(ApprovalRequestEvent {
        session_id,
        approval_id,
        kind: ApprovalKind::ProviderSpecific,
        title: "Approve Qwen permission".to_owned(),
        details,
        expires_at: None,
        presentation: None,
        provider_payload: Some(message.clone()),
    });
    Some((approval_id, native_request_id, event))
}

pub fn qwen_permission_response(message: &Value, decision: ApprovalDecision) -> Value {
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

fn inert_client_response(message: &Value) -> Option<Value> {
    match jsonrpc_method(message)? {
        "fs/read_text_file" => Some(json!({"content": ""})),
        "fs/write_text_file" | "terminal/release" | "terminal/kill" => Some(json!({})),
        "terminal/create" => Some(json!({"terminalId": "agenter-runner-terminal-denied"})),
        "terminal/output" => Some(json!({
            "output": "",
            "truncated": false,
            "exitStatus": {"exitCode": 1}
        })),
        "terminal/wait_for_exit" => Some(json!({"exitCode": 1})),
        _ => None,
    }
}

fn jsonrpc_method(message: &Value) -> Option<&str> {
    message.get("method")?.as_str()
}

fn is_response_to(message: &Value, request_id: &Value) -> bool {
    message.get("id") == Some(request_id)
        && (message.get("result").is_some() || message.get("error").is_some())
}

fn qwen_session_id(message: &Value) -> Option<&str> {
    message
        .pointer("/result/sessionId")
        .and_then(Value::as_str)
        .or_else(|| message.pointer("/params/sessionId").and_then(Value::as_str))
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
    .unwrap_or("qwen-message")
    .to_owned()
}

fn string_at<'a>(message: &'a Value, pointers: &[&str]) -> Option<&'a str> {
    pointers
        .iter()
        .find_map(|pointer| message.pointer(pointer).and_then(Value::as_str))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_qwen_message_chunk_fixture() {
        let message = json!({
            "method": "session/update",
            "params": {
                "sessionId": "session-1",
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "content": {"type": "text", "text": "hello"}
                }
            }
        });

        let events = normalize_qwen_message(SessionId::nil(), &message);

        assert_eq!(events.len(), 1);
        let AppEvent::AgentMessageDelta(delta) = &events[0] else {
            panic!("expected qwen delta");
        };
        assert_eq!(delta.delta, "hello");
    }

    #[test]
    fn normalizes_qwen_permission_fixture() {
        let message = json!({
            "id": "permission-1",
            "method": "session/request_permission",
            "params": {
                "sessionId": "session-1",
                "toolCall": {"toolCallId": "tool-1", "name": "shell"},
                "options": [{"optionId": "reject", "kind": "reject_once"}]
            }
        });

        let (_approval_id, native_id, event) =
            normalize_qwen_permission_request(SessionId::nil(), &message)
                .expect("permission should normalize");

        assert_eq!(native_id, json!("permission-1"));
        let AppEvent::ApprovalRequested(request) = event else {
            panic!("expected approval request");
        };
        assert_eq!(request.kind, ApprovalKind::ProviderSpecific);
        assert_eq!(request.details.as_deref(), Some("shell"));
    }

    #[test]
    fn normalizes_qwen_prompt_response_as_completion() {
        let message = json!({
            "id": 3,
            "result": {
                "sessionId": "session-1",
                "stopReason": "end_turn"
            }
        });

        let event = normalize_qwen_prompt_response(SessionId::nil(), &message);

        let AppEvent::AgentMessageCompleted(completed) = event else {
            panic!("expected qwen completion");
        };
        assert_eq!(completed.message_id, "session-1");
        assert!(completed.content.is_none());
    }

    #[test]
    fn maps_qwen_permission_decisions_to_options() {
        let message = json!({
            "params": {
                "options": [
                    {"optionId": "allow", "kind": "allow_once"},
                    {"optionId": "reject", "kind": "reject_once"}
                ]
            }
        });

        assert_eq!(
            qwen_permission_response(&message, ApprovalDecision::Accept),
            json!({"outcome": {"outcome": "selected", "optionId": "allow"}})
        );
        assert_eq!(
            qwen_permission_response(&message, ApprovalDecision::Decline),
            json!({"outcome": {"outcome": "selected", "optionId": "reject"}})
        );
        assert_eq!(
            qwen_permission_response(&message, ApprovalDecision::Cancel),
            json!({"outcome": {"outcome": "cancelled"}})
        );
    }
}

use std::{env, path::PathBuf, time::Duration};

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    time::{timeout, Instant},
};
use tracing::{info, warn};

const DEFAULT_PROMPT: &str = "Protocol spike: reply briefly, then try to run `pwd` and try to create `agenter-codex-approval-probe.txt`. Ask for approval when required.";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(180);
const THREAD_START_TIMEOUT: Duration = Duration::from_secs(15);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::main]
async fn main() -> Result<()> {
    agenter_core::logging::init_tracing("agenter-codex-spike");

    let (workspace, prompt) = parse_args()?;
    let workspace = workspace
        .canonicalize()
        .with_context(|| format!("failed to canonicalize workspace {}", workspace.display()))?;

    info!(workspace = %workspace.display(), "starting codex app-server spike");
    let mut child = spawn_codex(&workspace)?;
    let mut stdin = child
        .stdin
        .take()
        .context("codex app-server stdin was not piped")?;
    let stdout = child
        .stdout
        .take()
        .context("codex app-server stdout was not piped")?;
    let stderr = child
        .stderr
        .take()
        .context("codex app-server stderr was not piped")?;

    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            warn!(target: "provider-stderr", "{line}");
        }
    });

    let run_result = run_spike(&workspace, &prompt, &mut stdin, stdout).await;
    let shutdown_result = shutdown_child(child, stdin).await;
    run_result.and(shutdown_result)?;
    Ok(())
}

async fn run_spike(
    workspace: &PathBuf,
    prompt: &str,
    stdin: &mut ChildStdin,
    stdout: impl tokio::io::AsyncRead + Unpin,
) -> Result<()> {
    let mut lines = BufReader::new(stdout).lines();
    let mut next_id = 1_u64;
    let mut thread_id: Option<String> = None;
    let mut turn_started = false;
    let mut approval_seen = false;
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    let thread_start_deadline = Instant::now() + THREAD_START_TIMEOUT;

    send_request(
        stdin,
        next_jsonrpc_id(&mut next_id),
        "initialize",
        json!({
            "clientInfo": {
                "name": "agenter-codex-spike",
                "title": "Agenter Codex Spike",
                "version": "0.1.0"
            },
            "capabilities": {"experimentalApi": true}
        }),
    )
    .await?;

    let thread_start_id = next_jsonrpc_id(&mut next_id);
    send_request(
        stdin,
        thread_start_id,
        "thread/start",
        json!({
            "cwd": workspace,
            "approvalPolicy": "on-request",
            "approvalsReviewer": "user",
            "sandbox": "read-only",
            "sessionStartSource": "startup"
        }),
    )
    .await?;

    while Instant::now() < deadline {
        let active_deadline = if turn_started {
            deadline
        } else {
            deadline.min(thread_start_deadline)
        };
        let remaining = active_deadline.saturating_duration_since(Instant::now());
        let line = timeout(remaining, lines.next_line())
            .await
            .with_context(|| {
                if turn_started {
                    "timed out waiting for codex app-server output".to_owned()
                } else {
                    "timed out waiting for codex thread/start response with a thread id".to_owned()
                }
            })?
            .context("failed to read codex app-server stdout")?
            .ok_or_else(|| anyhow!("codex app-server exited before approval was observed"))?;

        let message: Value = serde_json::from_str(&line)
            .with_context(|| format!("codex app-server emitted invalid JSON line: {line}"))?;
        if !message_belongs_to_thread(&message, thread_id.as_deref()) {
            continue;
        }
        log_message("codex", &message);

        if thread_id.is_none() {
            if let Some(observed_thread_id) = codex_thread_id(&message) {
                thread_id = Some(observed_thread_id.to_owned());
            }
        }
        if let Some(completion) = spike_turn_completion(&message) {
            completion.map_err(anyhow::Error::msg)?;
            info!(approval_seen, thread_id, "codex app-server spike finished");
            return Ok(());
        }
        if let Some(observed_thread_id) = codex_thread_id(&message) {
            thread_id = Some(observed_thread_id.to_owned());
        }
        if let Some(error) = thread_start_error_summary(&message, thread_start_id) {
            return Err(anyhow!(error));
        }
        if thread_start_response_missing_thread_id(&message, thread_start_id) {
            return Err(anyhow!(
                "codex thread/start response did not include a thread id; rerun with RUST_LOG=codex_app_server_spike=info and inspect the response payload"
            ));
        }

        if is_approval_request(&message) {
            approval_seen = true;
            respond(stdin, &message["id"], codex_approval_response(&message)).await?;
            continue;
        }

        if !turn_started {
            if let Some(thread_id) = thread_id.as_deref() {
                send_request(
                    stdin,
                    next_jsonrpc_id(&mut next_id),
                    "turn/start",
                    json!({
                        "threadId": thread_id,
                        "cwd": workspace,
                        "approvalPolicy": "on-request",
                        "approvalsReviewer": "user",
                        "sandboxPolicy": {"type": "readOnly", "networkAccess": false},
                        "input": [{"type": "text", "text": prompt}]
                    }),
                )
                .await?;
                turn_started = true;
            }
        }
    }

    Err(anyhow!(
        "timed out waiting for codex turn completion; approval_seen={approval_seen}"
    ))
}

fn parse_args() -> Result<(PathBuf, String)> {
    let mut args = env::args().skip(1);
    let workspace = args
        .next()
        .map(PathBuf::from)
        .unwrap_or(env::current_dir().context("failed to read current directory")?);
    let prompt = {
        let parts = args.collect::<Vec<_>>();
        if parts.is_empty() {
            env::var("AGENTER_SPIKE_PROMPT").unwrap_or_else(|_| DEFAULT_PROMPT.to_owned())
        } else {
            parts.join(" ")
        }
    };
    Ok((workspace, prompt))
}

fn spawn_codex(workspace: &PathBuf) -> Result<Child> {
    Command::new("codex")
        .args(["app-server", "--listen", "stdio://"])
        .current_dir(workspace)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("failed to start `codex app-server --listen stdio://`; is codex installed and authenticated?")
}

fn next_jsonrpc_id(next_id: &mut u64) -> u64 {
    let id = *next_id;
    *next_id += 1;
    id
}

async fn send_request(stdin: &mut ChildStdin, id: u64, method: &str, params: Value) -> Result<()> {
    write_json(
        stdin,
        &json!({
            "id": id,
            "method": method,
            "params": params
        }),
    )
    .await?;
    info!(direction = "send", method, id, "json-rpc request");
    Ok(())
}

async fn respond(stdin: &mut ChildStdin, id: &Value, result: Value) -> Result<()> {
    write_json(
        stdin,
        &json!({
            "id": id,
            "result": result
        }),
    )
    .await?;
    info!(direction = "send", id = %id, "json-rpc response");
    Ok(())
}

async fn write_json(stdin: &mut ChildStdin, message: &Value) -> Result<()> {
    let mut encoded = serde_json::to_vec(message).context("failed to encode JSON-RPC message")?;
    encoded.push(b'\n');
    stdin
        .write_all(&encoded)
        .await
        .context("failed to write JSON-RPC message")?;
    stdin.flush().await.context("failed to flush stdin")
}

fn log_message(provider: &str, message: &Value) {
    let payload_preview = agenter_core::logging::payload_preview(
        message,
        agenter_core::logging::payload_logging_enabled(),
    );
    if let Some(method) = jsonrpc_method(message) {
        info!(
            direction = "recv",
            provider,
            method,
            payload_preview = payload_preview.as_deref(),
            "json-rpc method"
        );
    } else if message.get("id").is_some() && message.get("error").is_some() {
        warn!(direction = "recv", provider, id = %message["id"], payload_preview = payload_preview.as_deref(), "json-rpc error response");
    } else if message.get("id").is_some() {
        info!(direction = "recv", provider, id = %message["id"], payload_preview = payload_preview.as_deref(), "json-rpc response");
    }
}

fn jsonrpc_method(message: &Value) -> Option<&str> {
    message.get("method")?.as_str()
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

fn thread_start_response_missing_thread_id(message: &Value, thread_start_id: u64) -> bool {
    message.get("id").and_then(Value::as_u64) == Some(thread_start_id)
        && message.get("result").is_some()
        && codex_thread_id(message).is_none()
}

fn thread_start_error_summary(message: &Value, thread_start_id: u64) -> Option<String> {
    if message.get("id").and_then(Value::as_u64) != Some(thread_start_id) {
        return None;
    }
    let error = message.get("error")?;
    let code = error
        .get("code")
        .map(Value::to_string)
        .unwrap_or_else(|| "unknown".to_owned());
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("unknown provider error");
    Some(format!("codex thread/start failed: {code} {message}"))
}

fn message_belongs_to_thread(message: &Value, target_thread_id: Option<&str>) -> bool {
    let Some(target_thread_id) = target_thread_id else {
        return true;
    };
    let Some(message_thread_id) = message_thread_id(message) else {
        return true;
    };
    message_thread_id == target_thread_id
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

fn spike_turn_completion(message: &Value) -> Option<std::result::Result<(), String>> {
    if jsonrpc_method(message) != Some("turn/completed") {
        return None;
    }
    let error = message.pointer("/params/turn/error");
    if matches!(error, None | Some(Value::Null)) {
        return Some(Ok(()));
    }
    let message = error
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("unknown provider error");
    Some(Err(format!("codex turn failed: {message}")))
}

fn is_approval_request(message: &Value) -> bool {
    matches!(
        jsonrpc_method(message),
        Some(
            "item/commandExecution/requestApproval"
                | "item/fileChange/requestApproval"
                | "item/permissions/requestApproval"
        )
    ) && message.get("id").is_some()
}

fn string_at<'a>(message: &'a Value, pointers: &[&str]) -> Option<&'a str> {
    pointers
        .iter()
        .find_map(|pointer| message.pointer(pointer).and_then(Value::as_str))
}

fn codex_approval_response(message: &Value) -> Value {
    if jsonrpc_method(message) == Some("item/permissions/requestApproval") {
        json!({"permissions": {"fileSystem": null, "network": null}, "scope": "turn"})
    } else {
        json!({"decision": "decline"})
    }
}

async fn shutdown_child(mut child: Child, mut stdin: ChildStdin) -> Result<()> {
    stdin
        .shutdown()
        .await
        .context("failed to close codex app-server stdin")?;
    match timeout(SHUTDOWN_TIMEOUT, child.wait()).await {
        Ok(status) => {
            let status = status.context("failed to wait for codex app-server")?;
            info!(%status, "codex app-server exited");
        }
        Err(_) => {
            warn!("codex app-server did not exit after stdin closed; killing child");
            child
                .start_kill()
                .context("failed to kill codex app-server child")?;
            let _ = child.wait().await;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_jsonrpc_method_names() {
        let message = json!({"id": 10, "method": "thread/start", "params": {}});

        assert_eq!(jsonrpc_method(&message), Some("thread/start"));
    }

    #[test]
    fn extracts_codex_thread_id_from_known_response_shapes() {
        let nested = json!({"result": {"thread": {"id": "thread-nested"}}});
        let flat = json!({"result": {"threadId": "thread-flat"}});
        let notification = json!({"method": "thread/started", "params": {"thread": {"id": "thread-notification"}}});
        let result_id = json!({"result": {"id": "thread-result-id"}});

        assert_eq!(codex_thread_id(&nested), Some("thread-nested"));
        assert_eq!(codex_thread_id(&flat), Some("thread-flat"));
        assert_eq!(codex_thread_id(&notification), Some("thread-notification"));
        assert_eq!(codex_thread_id(&result_id), Some("thread-result-id"));
    }

    #[test]
    fn identifies_thread_start_response_without_thread_id() {
        let message = json!({"id": 2, "result": {"thread": {"status": "failed"}}});

        assert!(thread_start_response_missing_thread_id(&message, 2));
    }

    #[test]
    fn summarizes_thread_start_error_response() {
        let message = json!({
            "id": 2,
            "error": {
                "code": -32603,
                "message": "permission denied"
            }
        });

        assert_eq!(
            thread_start_error_summary(&message, 2),
            Some("codex thread/start failed: -32603 permission denied".to_owned())
        );
    }

    #[test]
    fn treats_successful_turn_completed_as_spike_completion() {
        let message = json!({
            "method": "turn/completed",
            "params": {
                "threadId": "thread-1",
                "turn": {
                    "id": "turn-1",
                    "status": "completed",
                    "error": null
                }
            }
        });

        assert_eq!(spike_turn_completion(&message), Some(Ok(())));
    }

    #[test]
    fn treats_failed_turn_completed_as_spike_error() {
        let message = json!({
            "method": "turn/completed",
            "params": {
                "threadId": "thread-1",
                "turn": {
                    "id": "turn-1",
                    "status": "failed",
                    "error": {"message": "model failed"}
                }
            }
        });

        assert_eq!(
            spike_turn_completion(&message),
            Some(Err("codex turn failed: model failed".to_owned()))
        );
    }

    #[test]
    fn filters_spike_events_to_target_thread() {
        let target = Some("thread-1");
        let matching = json!({
            "method": "item/agentMessage/delta",
            "params": {
                "threadId": "thread-1",
                "delta": "ok"
            }
        });
        let unrelated = json!({
            "method": "item/agentMessage/delta",
            "params": {
                "threadId": "thread-2",
                "delta": "wrong"
            }
        });

        assert!(message_belongs_to_thread(&matching, target));
        assert!(!message_belongs_to_thread(&unrelated, target));
    }

    #[test]
    fn uses_permissions_response_shape_for_codex_permission_approval() {
        let message = json!({"id": 7, "method": "item/permissions/requestApproval"});

        assert_eq!(
            codex_approval_response(&message),
            json!({"permissions": {"fileSystem": null, "network": null}, "scope": "turn"})
        );
    }
}

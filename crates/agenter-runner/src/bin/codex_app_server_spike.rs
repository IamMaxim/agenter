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
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .without_time()
        .init();

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

    let mut lines = BufReader::new(stdout).lines();
    let mut next_id = 1_u64;
    let mut thread_id: Option<String> = None;
    let mut turn_started = false;
    let mut approval_seen = false;
    let deadline = Instant::now() + REQUEST_TIMEOUT;

    send_request(
        &mut stdin,
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

    send_request(
        &mut stdin,
        next_jsonrpc_id(&mut next_id),
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

    while Instant::now() < deadline && !approval_seen {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let line = timeout(remaining, lines.next_line())
            .await
            .context("timed out waiting for codex app-server output")?
            .context("failed to read codex app-server stdout")?
            .ok_or_else(|| anyhow!("codex app-server exited before approval was observed"))?;

        let message: Value = serde_json::from_str(&line)
            .with_context(|| format!("codex app-server emitted invalid JSON line: {line}"))?;
        log_message("codex", &message);

        if let Some(observed_thread_id) = codex_thread_id(&message) {
            thread_id = Some(observed_thread_id.to_owned());
        }

        if is_approval_request(&message) {
            approval_seen = true;
            respond(&mut stdin, &message["id"], json!({"decision": "decline"})).await?;
            continue;
        }

        if !turn_started {
            if let Some(thread_id) = thread_id.as_deref() {
                send_request(
                    &mut stdin,
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

    if !approval_seen {
        warn!("codex spike timed out before observing an approval request");
    }

    shutdown_child(child, stdin).await?;
    info!(approval_seen, thread_id, "codex app-server spike finished");
    Ok(())
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
    if let Some(method) = jsonrpc_method(message) {
        info!(direction = "recv", provider, method, "json-rpc method");
    } else if message.get("id").is_some() && message.get("error").is_some() {
        warn!(direction = "recv", provider, id = %message["id"], "json-rpc error response");
    } else if message.get("id").is_some() {
        info!(direction = "recv", provider, id = %message["id"], "json-rpc response");
    }
}

fn jsonrpc_method(message: &Value) -> Option<&str> {
    message.get("method")?.as_str()
}

fn codex_thread_id(message: &Value) -> Option<&str> {
    message
        .pointer("/result/thread/id")
        .and_then(Value::as_str)
        .or_else(|| message.pointer("/result/threadId").and_then(Value::as_str))
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

        assert_eq!(codex_thread_id(&nested), Some("thread-nested"));
        assert_eq!(codex_thread_id(&flat), Some("thread-flat"));
    }
}

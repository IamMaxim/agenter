use std::{env, path::PathBuf, time::Duration};

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    time::{timeout, Instant},
};
use tracing::{info, warn};

const DEFAULT_PROMPT: &str = "Protocol spike: reply briefly, then try to run `pwd` and try to create `agenter-qwen-approval-probe.txt`. Ask for permission when required.";
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

    info!(workspace = %workspace.display(), "starting qwen ACP spike");
    let mut child = spawn_qwen(&workspace)?;
    let mut stdin = child.stdin.take().context("qwen stdin was not piped")?;
    let stdout = child.stdout.take().context("qwen stdout was not piped")?;
    let stderr = child.stderr.take().context("qwen stderr was not piped")?;

    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            warn!(target: "provider-stderr", "{line}");
        }
    });

    let mut lines = BufReader::new(stdout).lines();
    let mut next_id = 1_u64;
    let mut session_id: Option<String> = None;
    let mut prompt_sent = false;
    let mut permission_seen = false;
    let deadline = Instant::now() + REQUEST_TIMEOUT;

    send_request(
        &mut stdin,
        next_jsonrpc_id(&mut next_id),
        "initialize",
        json!({
            "protocolVersion": 1,
            "clientInfo": {
                "name": "agenter-qwen-acp-spike",
                "title": "Agenter Qwen ACP Spike",
                "version": "0.1.0"
            },
            "clientCapabilities": {
                "fs": {"readTextFile": true, "writeTextFile": true},
                "terminal": true
            }
        }),
    )
    .await?;

    send_request(
        &mut stdin,
        next_jsonrpc_id(&mut next_id),
        "session/new",
        json!({
            "cwd": workspace,
            "mcpServers": []
        }),
    )
    .await?;

    while Instant::now() < deadline && !permission_seen {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let line = timeout(remaining, lines.next_line())
            .await
            .context("timed out waiting for qwen ACP output")?
            .context("failed to read qwen stdout")?
            .ok_or_else(|| anyhow!("qwen --acp exited before permission was observed"))?;

        let message: Value = serde_json::from_str(&line)
            .with_context(|| format!("qwen emitted invalid JSON line: {line}"))?;
        log_message("qwen", &message);

        if let Some(observed_session_id) = qwen_session_id(&message) {
            session_id = Some(observed_session_id.to_owned());
        }

        match jsonrpc_method(&message) {
            Some("session/request_permission") if message.get("id").is_some() => {
                permission_seen = true;
                respond(
                    &mut stdin,
                    &message["id"],
                    qwen_permission_response(&message),
                )
                .await?;
                continue;
            }
            Some("fs/read_text_file") if message.get("id").is_some() => {
                respond(&mut stdin, &message["id"], json!({"content": ""})).await?;
                continue;
            }
            Some("fs/write_text_file") if message.get("id").is_some() => {
                respond(&mut stdin, &message["id"], json!({})).await?;
                continue;
            }
            Some("terminal/create") if message.get("id").is_some() => {
                respond(
                    &mut stdin,
                    &message["id"],
                    json!({"terminalId": "agenter-spike-terminal-denied"}),
                )
                .await?;
                continue;
            }
            Some("terminal/output") if message.get("id").is_some() => {
                respond(
                    &mut stdin,
                    &message["id"],
                    json!({"output": "", "truncated": false, "exitStatus": {"exitCode": 1}}),
                )
                .await?;
                continue;
            }
            Some("terminal/wait_for_exit") if message.get("id").is_some() => {
                respond(&mut stdin, &message["id"], json!({"exitCode": 1})).await?;
                continue;
            }
            Some("terminal/release" | "terminal/kill") if message.get("id").is_some() => {
                respond(&mut stdin, &message["id"], json!({})).await?;
                continue;
            }
            _ => {}
        }

        if !prompt_sent {
            if let Some(session_id) = session_id.as_deref() {
                send_request(
                    &mut stdin,
                    next_jsonrpc_id(&mut next_id),
                    "session/prompt",
                    json!({
                        "sessionId": session_id,
                        "prompt": [{"type": "text", "text": prompt}]
                    }),
                )
                .await?;
                prompt_sent = true;
            }
        }
    }

    if !permission_seen {
        warn!("qwen spike timed out before observing a permission request");
    }

    shutdown_child(child, stdin).await?;
    info!(permission_seen, session_id, "qwen ACP spike finished");
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

fn spawn_qwen(workspace: &PathBuf) -> Result<Child> {
    Command::new("qwen")
        .args(["--acp", "--approval-mode", "default"])
        .current_dir(workspace)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("failed to start `qwen --acp --approval-mode default`; is qwen installed and authenticated?")
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

fn qwen_session_id(message: &Value) -> Option<&str> {
    message.pointer("/result/sessionId").and_then(Value::as_str)
}

fn qwen_permission_response(message: &Value) -> Value {
    let selected = message
        .pointer("/params/options")
        .and_then(Value::as_array)
        .and_then(|options| {
            options
                .iter()
                .find(|option| {
                    matches!(
                        option.get("kind").and_then(Value::as_str),
                        Some("reject_once" | "reject_always")
                    )
                })
                .or_else(|| options.last())
        })
        .and_then(|option| option.get("optionId"))
        .and_then(Value::as_str);

    match selected {
        Some(option_id) => json!({"outcome": {"outcome": "selected", "optionId": option_id}}),
        None => json!({"outcome": {"outcome": "cancelled"}}),
    }
}

async fn shutdown_child(mut child: Child, mut stdin: ChildStdin) -> Result<()> {
    stdin
        .shutdown()
        .await
        .context("failed to close qwen stdin")?;
    match timeout(SHUTDOWN_TIMEOUT, child.wait()).await {
        Ok(status) => {
            let status = status.context("failed to wait for qwen --acp")?;
            info!(%status, "qwen --acp exited");
        }
        Err(_) => {
            warn!("qwen --acp did not exit after stdin closed; killing child");
            child.start_kill().context("failed to kill qwen child")?;
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
        let message = json!({"id": 10, "method": "session/new", "params": {}});

        assert_eq!(jsonrpc_method(&message), Some("session/new"));
    }

    #[test]
    fn extracts_qwen_session_id_from_known_response_shape() {
        let message = json!({"result": {"sessionId": "session-123"}});

        assert_eq!(qwen_session_id(&message), Some("session-123"));
    }
}

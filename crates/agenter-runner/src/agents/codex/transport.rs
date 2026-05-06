#![allow(dead_code)]

use std::{
    collections::VecDeque,
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::ExitStatus,
    sync::Arc,
    time::Duration,
};

use anyhow::{anyhow, Context};
use serde_json::{json, Value};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::Mutex,
    time::{sleep, timeout},
};

use super::codec::{
    CodexClientErrorFrame, CodexClientResponseFrame, CodexCodec, CodexDecodedFrame, RequestId,
};

const CODEX_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const CODEX_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
const CODEX_STDERR_EXCERPT_LINES: usize = 12;

#[derive(Debug, Clone)]
pub struct CodexTransportConfig {
    pub command: String,
    pub args: Vec<String>,
    pub workspace_path: PathBuf,
    pub request_timeout: Duration,
}

impl CodexTransportConfig {
    #[must_use]
    pub fn app_server(workspace_path: impl Into<PathBuf>) -> Self {
        Self {
            command: codex_command(),
            args: vec![
                "app-server".to_owned(),
                "--listen".to_owned(),
                "stdio://".to_owned(),
            ],
            workspace_path: workspace_path.into(),
            request_timeout: CODEX_REQUEST_TIMEOUT,
        }
    }

    #[must_use]
    pub fn command(
        command: impl Into<String>,
        args: impl IntoIterator<Item = impl Into<String>>,
        workspace_path: impl Into<PathBuf>,
    ) -> Self {
        Self {
            command: command.into(),
            args: args.into_iter().map(Into::into).collect(),
            workspace_path: workspace_path.into(),
            request_timeout: CODEX_REQUEST_TIMEOUT,
        }
    }

    #[must_use]
    pub fn with_request_timeout(mut self, request_timeout: Duration) -> Self {
        self.request_timeout = request_timeout;
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum CodexTransportEvent {
    Frame(CodexDecodedFrame),
    ProcessExited {
        status: Option<ExitStatus>,
        stderr_excerpt: String,
    },
}

pub struct CodexTransport {
    config: CodexTransportConfig,
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    codec: CodexCodec,
    queued_events: VecDeque<CodexTransportEvent>,
    recent_stderr: Arc<Mutex<VecDeque<String>>>,
}

impl CodexTransport {
    pub fn spawn(config: CodexTransportConfig) -> anyhow::Result<Self> {
        let command = resolve_command(&config.command);
        if !config.workspace_path.is_dir() {
            anyhow::bail!(
                "failed to start Codex app-server: workspace `{}` does not exist or is not a directory",
                config.workspace_path.display()
            );
        }
        tracing::info!(
            command = %command.display(),
            args = ?config.args,
            workspace = %config.workspace_path.display(),
            "spawning Codex app-server transport"
        );
        let mut child = Command::new(&command)
            .args(&config.args)
            .current_dir(&config.workspace_path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .with_context(|| {
                format!(
                    "failed to start Codex app-server command `{}` in `{}`; set AGENTER_CODEX_BIN to an absolute codex binary path if it is installed through a shell-only manager such as nvm; PATH={}",
                    command.display(),
                    config.workspace_path.display(),
                    env::var("PATH").unwrap_or_default()
                )
            })?;
        let stdin = child
            .stdin
            .take()
            .context("Codex app-server stdin was not piped")?;
        let stdout = child
            .stdout
            .take()
            .context("Codex app-server stdout was not piped")?;
        let recent_stderr = Arc::new(Mutex::new(VecDeque::new()));
        if let Some(stderr) = child.stderr.take() {
            let recent_stderr_for_task = recent_stderr.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::warn!(target: "codex-app-server-stderr", "{line}");
                    let mut guard = recent_stderr_for_task.lock().await;
                    guard.push_back(line);
                    while guard.len() > CODEX_STDERR_EXCERPT_LINES {
                        guard.pop_front();
                    }
                }
            });
        }
        Ok(Self {
            config,
            child,
            stdin,
            stdout: BufReader::new(stdout),
            codec: CodexCodec::new(),
            queued_events: VecDeque::new(),
            recent_stderr,
        })
    }

    pub async fn initialize(
        &mut self,
        experimental_api: bool,
    ) -> anyhow::Result<CodexClientResponseFrame> {
        self.request_response(
            "initialize",
            json!({
                "clientInfo": {
                    "name": "agenter-runner",
                    "title": "Agenter Runner",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "capabilities": {
                    "experimentalApi": experimental_api,
                    "optOutNotificationMethods": null,
                },
            }),
        )
        .await
    }

    pub async fn send_request(
        &mut self,
        method: impl Into<String>,
        params: Value,
    ) -> anyhow::Result<RequestId> {
        let (request_id, payload) = self.codec.encode_request(method, params);
        write_json(&mut self.stdin, &payload).await?;
        Ok(request_id)
    }

    pub async fn respond(&mut self, request_id: RequestId, result: Value) -> anyhow::Result<()> {
        let payload = CodexCodec::encode_response(request_id, result);
        write_json(&mut self.stdin, &payload).await
    }

    pub async fn respond_raw(&mut self, payload: Value) -> anyhow::Result<()> {
        write_json(&mut self.stdin, &payload).await
    }

    pub async fn request_response(
        &mut self,
        method: impl Into<String>,
        params: Value,
    ) -> anyhow::Result<CodexClientResponseFrame> {
        let method = method.into();
        let request_id = self.send_request(method.clone(), params).await?;
        let request_timeout = self.config.request_timeout;
        match timeout(request_timeout, self.wait_for_response(request_id.clone())).await {
            Ok(result) => result,
            Err(_) => Err(anyhow!(
                "timed out waiting for Codex app-server `{method}` response with id `{request_id}`"
            )),
        }
    }

    async fn wait_for_response(
        &mut self,
        request_id: RequestId,
    ) -> anyhow::Result<CodexClientResponseFrame> {
        loop {
            let event = self.read_next_event().await?;
            match event {
                CodexTransportEvent::Frame(CodexDecodedFrame::ClientResponse(response))
                    if response.request_id == request_id =>
                {
                    return Ok(response);
                }
                CodexTransportEvent::Frame(CodexDecodedFrame::ClientError(error))
                    if error.request_id == request_id =>
                {
                    return Err(codex_client_error(error));
                }
                other => self.queued_events.push_back(other),
            }
        }
    }

    pub async fn next_event(&mut self) -> anyhow::Result<CodexTransportEvent> {
        if let Some(event) = self.queued_events.pop_front() {
            return Ok(event);
        }
        self.read_next_event().await
    }

    async fn read_next_event(&mut self) -> anyhow::Result<CodexTransportEvent> {
        let mut line = String::new();
        if self.stdout.read_line(&mut line).await? == 0 {
            sleep(Duration::from_millis(10)).await;
            return Ok(CodexTransportEvent::ProcessExited {
                status: self.child.try_wait()?,
                stderr_excerpt: self.stderr_excerpt().await,
            });
        }
        Ok(CodexTransportEvent::Frame(self.codec.decode_line(&line)))
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

    pub async fn shutdown(mut self) -> anyhow::Result<()> {
        self.stdin.shutdown().await.ok();
        match timeout(CODEX_SHUTDOWN_TIMEOUT, self.child.wait()).await {
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

fn codex_command() -> String {
    env::var("AGENTER_CODEX_BIN")
        .ok()
        .filter(|path| !path.trim().is_empty())
        .unwrap_or_else(|| "codex".to_owned())
}

fn resolve_command(command: &str) -> PathBuf {
    let command_path = PathBuf::from(command);
    if command_path.components().count() > 1 {
        return command_path;
    }
    if command != "codex" {
        return command_path;
    }
    if let Some(path) = find_on_path("codex") {
        return path;
    }
    find_nvm_codex().unwrap_or(command_path)
}

fn find_on_path(command: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|dir| dir.join(command))
        .find(|candidate| is_executable_file(candidate))
}

fn find_nvm_codex() -> Option<PathBuf> {
    let home = env::var_os("HOME").map(PathBuf::from)?;
    let versions = home.join(".nvm").join("versions").join("node");
    let mut candidates = fs::read_dir(versions)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("bin").join("codex"))
        .filter(|path| is_executable_file(path))
        .collect::<Vec<_>>();
    candidates.sort_by_key(|path| version_sort_key(path).unwrap_or_default());
    candidates.pop()
}

fn version_sort_key(path: &Path) -> Option<OsString> {
    path.parent()?.parent()?.file_name().map(OsString::from)
}

fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

fn codex_client_error(error: CodexClientErrorFrame) -> anyhow::Error {
    let method = error.method.as_deref().unwrap_or("<unknown>");
    anyhow!(
        "Codex app-server `{method}` request `{}` failed: {} ({})",
        error.request_id,
        error.error.message,
        error.error.code
    )
}

async fn write_json(stdin: &mut ChildStdin, message: &Value) -> anyhow::Result<()> {
    let mut encoded = serde_json::to_vec(message)?;
    encoded.push(b'\n');
    stdin.write_all(&encoded).await?;
    stdin.flush().await?;
    Ok(())
}

#[must_use]
pub fn app_server_config_for_workspace(workspace_path: &Path) -> CodexTransportConfig {
    CodexTransportConfig::app_server(workspace_path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::codex::codec::{native_ref_for_decoded_frame, CodexDecodedFrame};
    use serde_json::json;

    fn shell_transport(script: &str) -> CodexTransportConfig {
        CodexTransportConfig::command(
            "/bin/sh",
            ["-c", script],
            std::env::current_dir().expect("test process should have a current directory"),
        )
        .with_request_timeout(Duration::from_millis(200))
    }

    #[tokio::test]
    async fn codex_transport_request_response_correlates_by_native_id() {
        let mut transport = CodexTransport::spawn(shell_transport(
            r#"read line
printf '%s\n' '{"id":1,"result":{"models":[]}}'
"#,
        ))
        .expect("fake app-server should spawn");

        let response = transport
            .request_response("model/list", json!({ "onlyAvailable": false }))
            .await
            .expect("response should be correlated");

        assert_eq!(response.request_id.to_string(), "1");
        assert_eq!(response.method.as_deref(), Some("model/list"));
        assert_eq!(response.raw_payload["result"]["models"], json!([]));
        transport.shutdown().await.ok();
    }

    #[tokio::test]
    async fn codex_transport_queues_server_request_and_notification_while_waiting_for_response() {
        let mut transport = CodexTransport::spawn(shell_transport(
            r#"read line
printf '%s\n' '{"id":"server-1","method":"codex/futureRequest","params":{"native":true}}'
printf '%s\n' '{"method":"codex/futureNotification","params":{"visible":true}}'
printf '%s\n' '{"id":1,"result":{"models":[]}}'
"#,
        ))
        .expect("fake app-server should spawn");

        let response = transport
            .request_response("model/list", json!({}))
            .await
            .expect("response should be correlated after queued events");
        assert_eq!(response.method.as_deref(), Some("model/list"));

        let request_event = transport.next_event().await.expect("queued request event");
        let CodexTransportEvent::Frame(CodexDecodedFrame::ServerRequest(request)) = &request_event
        else {
            panic!("expected queued server request");
        };
        assert_eq!(request.method, "codex/futureRequest");
        assert_eq!(request.raw_payload["params"]["native"], true);

        let notification_event = transport
            .next_event()
            .await
            .expect("queued notification event");
        let CodexTransportEvent::Frame(CodexDecodedFrame::ServerNotification(notification)) =
            &notification_event
        else {
            panic!("expected queued server notification");
        };
        assert_eq!(notification.method, "codex/futureNotification");
        assert_eq!(notification.raw_payload["params"]["visible"], true);

        transport.shutdown().await.ok();
    }

    #[tokio::test]
    async fn codex_transport_preserves_malformed_frames() {
        let mut transport = CodexTransport::spawn(shell_transport(
            r#"printf '%s\n' '{ not json'
"#,
        ))
        .expect("fake app-server should spawn");

        let event = transport.next_event().await.expect("malformed event");
        let CodexTransportEvent::Frame(CodexDecodedFrame::Malformed(malformed)) = event else {
            panic!("expected malformed frame");
        };
        assert!(!malformed.error.is_empty());

        transport.shutdown().await.ok();
    }

    #[tokio::test]
    async fn codex_transport_request_timeout_reports_pending_method() {
        let mut transport = CodexTransport::spawn(shell_transport(
            r#"read line
sleep 1
"#,
        ))
        .expect("fake app-server should spawn");

        let error = transport
            .request_response("model/list", json!({}))
            .await
            .expect_err("request should time out");
        assert!(error.to_string().contains("model/list"));

        transport.shutdown().await.ok();
    }

    #[tokio::test]
    async fn codex_transport_process_exit_is_visible() {
        let mut transport = CodexTransport::spawn(shell_transport(
            r#"printf '%s\n' 'stderr line' 1>&2
exit 7
"#,
        ))
        .expect("fake app-server should spawn");

        let event = transport.next_event().await.expect("process exit event");
        let CodexTransportEvent::ProcessExited {
            status,
            stderr_excerpt,
        } = event
        else {
            panic!("expected process exit");
        };
        assert_eq!(status.and_then(|status| status.code()), Some(7));
        assert!(stderr_excerpt.contains("stderr line"));
    }

    #[test]
    fn codex_transport_app_server_uses_configurable_codex_binary() {
        let config = CodexTransportConfig::app_server("/tmp/workspace");
        assert!(!config.command.trim().is_empty());
        assert_eq!(config.args, ["app-server", "--listen", "stdio://"]);
    }

    #[tokio::test]
    async fn codex_transport_frames_can_build_native_refs_with_raw_payload() {
        let mut transport = CodexTransport::spawn(shell_transport(
            r#"printf '%s\n' '{"id":"server-1","method":"codex/futureRequest","params":{"native":true}}'
"#,
        ))
        .expect("fake app-server should spawn");

        let event = transport.next_event().await.expect("server request event");
        let CodexTransportEvent::Frame(frame) = event else {
            panic!("expected native frame");
        };
        let native = native_ref_for_decoded_frame(&frame);
        assert_eq!(native.protocol, "codex/app-server/v2");
        assert_eq!(native.method.as_deref(), Some("codex/futureRequest"));
        assert_eq!(native.kind.as_deref(), Some("server_request"));
        assert_eq!(native.native_id.as_deref(), Some("server-1"));
        assert_eq!(native.raw_payload.unwrap()["params"]["native"], true);

        transport.shutdown().await.ok();
    }
}

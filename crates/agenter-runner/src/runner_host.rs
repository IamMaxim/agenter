use std::{future::Future, path::PathBuf, pin::Pin};

use agenter_protocol::runner::{
    RunnerClientMessage, RunnerCommandEnvelope, RunnerEvent, RunnerEventEnvelope, RunnerHello,
    RunnerOperationKind, RunnerOperationLogLevel, RunnerOperationProgress, RunnerOperationStatus,
    RunnerOperationUpdate, RunnerResponseEnvelope, RunnerResponseOutcome, RunnerServerMessage,
};
use agenter_protocol::{
    chunk_message, reassemble_message, RequestId, RunnerTransportChunkFrame,
    RunnerTransportChunkReassembler, RunnerTransportOutboundFrame,
};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration};
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::agents::adapter::AdapterEventReceiver;
use crate::wal::{self, RunnerWal, RunnerWalRecord};

const DEFAULT_RUNNER_WS_CHUNK_BYTES: usize = 1024 * 1024;
const DEFAULT_RUNNER_WS_MAX_MESSAGE_BYTES: usize = 512 * 1024 * 1024;

pub(crate) type RunnerHostBoxFuture<'a, T> =
    Pin<Box<dyn Future<Output = anyhow::Result<T>> + Send + 'a>>;
pub(crate) type RunnerBackgroundEventSender =
    mpsc::UnboundedSender<(Option<RequestId>, RunnerEvent)>;
pub(crate) type RunnerBackgroundEventReceiver =
    mpsc::UnboundedReceiver<(Option<RequestId>, RunnerEvent)>;

pub(crate) trait RunnerCommandHandler: Send {
    fn handle_command<'a>(
        &'a mut self,
        envelope: Box<RunnerCommandEnvelope>,
        events: RunnerBackgroundEventSender,
    ) -> RunnerHostBoxFuture<'a, RunnerResponseOutcome>;
}

#[derive(Clone, Debug)]
pub(crate) struct RunnerHostConfig {
    pub label: &'static str,
    pub url: String,
    pub workspace_path: PathBuf,
    pub hello_template: RunnerHello,
}

impl RunnerHostConfig {
    pub(crate) fn new(
        label: &'static str,
        url: impl Into<String>,
        workspace_path: PathBuf,
        hello_template: RunnerHello,
    ) -> Self {
        Self {
            label,
            url: url.into(),
            workspace_path,
            hello_template,
        }
    }

    pub(crate) async fn hello_with_wal_state(&self, wal: &RunnerWal) -> RunnerHello {
        let mut hello = self.hello_template.clone();
        let acked = wal.acked_seq().await;
        hello.acked_runner_event_seq = (acked > 0).then_some(acked);
        hello.replay_from_runner_event_seq = wal
            .unacked()
            .await
            .first()
            .map(|record| record.runner_event_seq);
        hello
    }
}

#[derive(Clone)]
pub(crate) struct RunnerOperationReporter {
    request_id: RequestId,
    sender: RunnerBackgroundEventSender,
}

impl RunnerOperationReporter {
    pub(crate) fn new(request_id: RequestId, sender: RunnerBackgroundEventSender) -> Self {
        Self { request_id, sender }
    }

    pub(crate) fn info(
        &self,
        status: RunnerOperationStatus,
        stage_label: &str,
        progress: Option<RunnerOperationProgress>,
        message: Option<String>,
    ) {
        self.send(
            status,
            stage_label,
            progress,
            message,
            RunnerOperationLogLevel::Info,
        );
    }

    pub(crate) fn error(
        &self,
        status: RunnerOperationStatus,
        stage_label: &str,
        message: Option<String>,
    ) {
        self.send(
            status,
            stage_label,
            None,
            message,
            RunnerOperationLogLevel::Error,
        );
    }

    fn send(
        &self,
        status: RunnerOperationStatus,
        stage_label: &str,
        progress: Option<RunnerOperationProgress>,
        message: Option<String>,
        level: RunnerOperationLogLevel,
    ) {
        self.sender
            .send((
                Some(self.request_id.clone()),
                RunnerEvent::OperationUpdated(RunnerOperationUpdate {
                    operation_id: self.request_id.clone(),
                    kind: RunnerOperationKind::SessionRefresh,
                    status,
                    stage_label: stage_label.to_owned(),
                    progress,
                    message,
                    level,
                    ts: None,
                }),
            ))
            .ok();
    }
}

pub(crate) fn background_event_channel(
) -> (RunnerBackgroundEventSender, RunnerBackgroundEventReceiver) {
    mpsc::unbounded_channel()
}

pub(crate) async fn run_runner_host<H>(
    config: RunnerHostConfig,
    mut adapter_events: AdapterEventReceiver,
    background_sender: RunnerBackgroundEventSender,
    mut background_events: RunnerBackgroundEventReceiver,
    mut handler: H,
) -> anyhow::Result<()>
where
    H: RunnerCommandHandler,
{
    let wal = RunnerWal::open(crate::runner_wal_path(
        config.hello_template.runner_id,
        &config.workspace_path,
    ))
    .await?;
    let advertised_workspace = config.hello_template.workspaces.first().cloned();

    loop {
        tracing::info!(
            url = %config.url,
            runner_id = %config.hello_template.runner_id,
            mode = config.label,
            "connecting runner to control plane"
        );
        let (socket, _) = match connect_async(&config.url).await {
            Ok(socket) => socket,
            Err(error) => {
                tracing::warn!(
                    %error,
                    mode = config.label,
                    "runner websocket connect failed; retrying"
                );
                sleep(runner_reconnect_delay()).await;
                continue;
            }
        };
        let (mut sender, mut receiver) = socket.split();
        let mut server_message_reassembler =
            RunnerTransportChunkReassembler::new(runner_ws_max_message_bytes());
        let hello = config.hello_with_wal_state(&wal).await;
        tracing::info!(
            runner_id = %hello.runner_id,
            mode = config.label,
            "sending runner hello"
        );
        if let Err(error) =
            send_runner_message(&mut sender, RunnerClientMessage::Hello(hello)).await
        {
            tracing::warn!(%error, mode = config.label, "failed to send runner hello; reconnecting");
            sleep(runner_reconnect_delay()).await;
            continue;
        }
        if let Err(error) = replay_unacked_wal(&mut sender, &wal).await {
            tracing::warn!(%error, mode = config.label, "failed to replay runner WAL; reconnecting");
            sleep(runner_reconnect_delay()).await;
            continue;
        }

        let transport_result: anyhow::Result<()> = async {
            loop {
                tokio::select! {
                    background = background_events.recv() => {
                        let Some((request_id, event)) = background else {
                            continue;
                        };
                        if let Err(error) = send_wal_event(
                            &mut sender,
                            &wal,
                            request_id,
                            None,
                            event,
                        )
                        .await {
                            tracing::warn!(%error, mode = config.label, "failed to send background event; reconnecting");
                            break;
                        }
                    }
                    event = adapter_events.recv() => {
                        let Some(adapter_event) = event else {
                            continue;
                        };
                        let Some(agent_event) = adapter_event.universal_projection_for_wal() else {
                            continue;
                        };
                        let session_id = agent_event.session_id;
                        if let Err(error) = send_wal_event(
                            &mut sender,
                            &wal,
                            None,
                            Some(session_id),
                            RunnerEvent::AgentEvent(Box::new(agent_event)),
                        )
                        .await {
                            tracing::warn!(%error, mode = config.label, "failed to send agent event; reconnecting");
                            break;
                        }
                    }
                    message = receiver.next() => {
                        let Some(message) = message else {
                            tracing::info!(mode = config.label, "control plane websocket closed for runner");
                            break;
                        };
                        let Message::Text(text) = message? else {
                            continue;
                        };
                        let Some(message) = next_runner_server_message(&mut server_message_reassembler, &text)? else {
                            continue;
                        };
                        let Some(envelope) = handle_runner_server_message(&wal, message).await else {
                            continue;
                        };
                        let request_id = envelope.request_id.clone();
                        let outcome = handler
                            .handle_command(envelope, background_sender.clone())
                            .await
                            .unwrap_or_else(|error| RunnerResponseOutcome::Error {
                                error: crate::runner_error("runner_command_failed", error),
                            });
                        send_runner_message(
                            &mut sender,
                            RunnerClientMessage::Response(RunnerResponseEnvelope {
                                request_id,
                                outcome,
                            }),
                        )
                        .await?;
                    }
                }
            }
            Ok(())
        }
        .await;

        if let Some(ref workspace) = advertised_workspace {
            tracing::debug!(workspace = %workspace.path, mode = config.label, "runner closed");
        }
        if let Err(error) = transport_result {
            tracing::warn!(%error, mode = config.label, "runner transport session failed; reconnecting");
        }
        sleep(runner_reconnect_delay()).await;
    }
}

pub(crate) async fn send_runner_message(
    sender: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    message: RunnerClientMessage,
) -> anyhow::Result<()> {
    let chunk_bytes = runner_ws_chunk_bytes();
    let frames = chunk_message(&message, chunk_bytes)?;
    if frames.len() > 1 {
        let json = serde_json::to_string(&message)?;
        let chunk_start = runner_transport_chunk_start(&frames);
        tracing::warn!(
            direction = "runner_to_control_plane",
            message_type = runner_client_message_type(&message),
            transfer_id = chunk_start.as_ref().map(|start| start.transfer_id.as_str()),
            total_bytes = chunk_start.as_ref().map(|start| start.total_bytes),
            message_bytes = json.len(),
            chunk_bytes,
            frame_count = frames.len(),
            total_chunks = chunk_start.as_ref().map(|start| start.total_chunks),
            "sending chunked runner websocket message"
        );
    } else {
        let RunnerTransportOutboundFrame::Text(json) = &frames[0];
        tracing::debug!(
            message_bytes = json.len(),
            "sending runner websocket message"
        );
    }

    for frame in frames {
        let RunnerTransportOutboundFrame::Text(text) = frame;
        sender.send(Message::Text(text.into())).await?;
    }
    Ok(())
}

pub(crate) async fn send_wal_event(
    sender: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    wal: &RunnerWal,
    request_id: Option<RequestId>,
    session_id: Option<agenter_core::SessionId>,
    event: RunnerEvent,
) -> anyhow::Result<()> {
    if !wal::event_is_replayable(&event) {
        return send_runner_event(sender, request_id, None, event).await;
    }
    let record = wal.append(request_id, session_id, event).await?;
    send_wal_record(sender, wal, &record).await
}

async fn send_runner_event(
    sender: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    request_id: Option<RequestId>,
    acked_runner_event_seq: Option<u64>,
    event: RunnerEvent,
) -> anyhow::Result<()> {
    send_runner_message(
        sender,
        RunnerClientMessage::Event(Box::new(RunnerEventEnvelope {
            request_id,
            runner_event_seq: None,
            acked_runner_event_seq,
            event,
        })),
    )
    .await
}

async fn send_wal_record(
    sender: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    wal: &RunnerWal,
    record: &RunnerWalRecord,
) -> anyhow::Result<()> {
    let acked = wal.acked_seq().await;
    send_runner_message(
        sender,
        RunnerClientMessage::Event(Box::new(RunnerEventEnvelope {
            request_id: record.request_id.clone(),
            runner_event_seq: Some(record.runner_event_seq),
            acked_runner_event_seq: (acked > 0).then_some(acked),
            event: record.event.clone(),
        })),
    )
    .await
}

async fn replay_unacked_wal(
    sender: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    wal: &RunnerWal,
) -> anyhow::Result<()> {
    for record in wal.unacked().await {
        send_wal_record(sender, wal, &record).await?;
    }
    Ok(())
}

fn runner_client_message_type(message: &RunnerClientMessage) -> &'static str {
    match message {
        RunnerClientMessage::Hello(_) => "runner_hello",
        RunnerClientMessage::Heartbeat(_) => "runner_heartbeat",
        RunnerClientMessage::Response(_) => "runner_response",
        RunnerClientMessage::Event(_) => "runner_event",
    }
}

fn runner_transport_chunk_start(
    frames: &[RunnerTransportOutboundFrame],
) -> Option<agenter_protocol::runner_transport::RunnerChunkStart> {
    let RunnerTransportOutboundFrame::Text(text) = frames.first()?;
    let Ok(RunnerTransportChunkFrame::Start(start)) =
        serde_json::from_str::<RunnerTransportChunkFrame>(text)
    else {
        return None;
    };
    Some(start)
}

fn next_runner_server_message(
    reassembler: &mut RunnerTransportChunkReassembler,
    text: &str,
) -> anyhow::Result<Option<RunnerServerMessage>> {
    match reassemble_message(reassembler, text) {
        Ok(message) => Ok(message),
        Err(error) => {
            tracing::warn!(%error, "runner ignored undecodable control-plane message");
            Ok(None)
        }
    }
}

async fn handle_runner_server_message(
    wal: &RunnerWal,
    message: RunnerServerMessage,
) -> Option<Box<RunnerCommandEnvelope>> {
    match message {
        RunnerServerMessage::Command(envelope) => Some(envelope),
        RunnerServerMessage::EventAck(ack) => {
            if let Err(error) = wal.ack(ack.runner_event_seq).await {
                tracing::warn!(
                    runner_event_seq = ack.runner_event_seq,
                    %error,
                    "failed to persist runner event ack"
                );
            }
            None
        }
        RunnerServerMessage::HeartbeatAck(_) => None,
    }
}

fn runner_ws_chunk_bytes() -> usize {
    env_usize(
        "AGENTER_RUNNER_WS_CHUNK_BYTES",
        DEFAULT_RUNNER_WS_CHUNK_BYTES,
    )
}

fn runner_ws_max_message_bytes() -> usize {
    env_usize(
        "AGENTER_RUNNER_WS_MAX_MESSAGE_BYTES",
        DEFAULT_RUNNER_WS_MAX_MESSAGE_BYTES,
    )
}

fn runner_reconnect_delay() -> Duration {
    Duration::from_secs(env_usize("AGENTER_RUNNER_RECONNECT_SECONDS", 2) as u64)
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use agenter_core::{
        AgentProviderId, NativeRef, RunnerId, SessionId, UniversalEventKind, UniversalEventSource,
        WorkspaceId, WorkspaceRef, UNIVERSAL_PROTOCOL_VERSION,
    };
    use agenter_protocol::runner::{
        AgentProviderAdvertisement, AgentUniversalEvent, RunnerCapabilities, RunnerCommandResult,
        RunnerHello, PROTOCOL_VERSION,
    };
    use uuid::Uuid;

    use super::*;

    fn hello() -> RunnerHello {
        let runner_id = RunnerId::from_uuid(Uuid::from_u128(1));
        RunnerHello {
            runner_id,
            protocol_version: PROTOCOL_VERSION.to_owned(),
            token: "token".to_owned(),
            capabilities: RunnerCapabilities {
                agent_providers: vec![AgentProviderAdvertisement {
                    provider_id: AgentProviderId::from("test"),
                    capabilities: agenter_core::AgentCapabilities::default(),
                }],
                transports: vec!["test".to_owned()],
                workspace_discovery: false,
            },
            acked_runner_event_seq: None,
            replay_from_runner_event_seq: None,
            workspaces: vec![WorkspaceRef {
                workspace_id: WorkspaceId::from_uuid(Uuid::from_u128(2)),
                runner_id,
                path: "/workspace".to_owned(),
                display_name: Some("workspace".to_owned()),
            }],
        }
    }

    fn wal_event() -> RunnerEvent {
        RunnerEvent::AgentEvent(Box::new(AgentUniversalEvent {
            protocol_version: UNIVERSAL_PROTOCOL_VERSION.to_owned(),
            session_id: SessionId::new(),
            event_id: None,
            turn_id: None,
            item_id: None,
            ts: None,
            source: UniversalEventSource::Native,
            native: Some(NativeRef {
                protocol: "test".to_owned(),
                method: Some("test/event".to_owned()),
                kind: None,
                native_id: None,
                summary: None,
                hash: None,
                pointer: None,
                raw_payload: None,
            }),
            event: UniversalEventKind::NativeUnknown {
                summary: Some("test".to_owned()),
            },
        }))
    }

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "agenter-runner-host-{name}-{}-{}.jsonl",
            std::process::id(),
            Uuid::new_v4()
        ))
    }

    #[tokio::test]
    async fn host_config_applies_wal_ack_and_replay_cursor_to_hello() {
        let path = temp_path("cursor");
        let wal = RunnerWal::open(&path).await.expect("open wal");
        let first = wal
            .append(None, None, wal_event())
            .await
            .expect("append first");
        let second = wal
            .append(None, None, wal_event())
            .await
            .expect("append second");
        wal.ack(first.runner_event_seq).await.expect("ack first");

        let config = RunnerHostConfig::new(
            "test",
            "ws://control-plane",
            PathBuf::from("/workspace"),
            hello(),
        );

        let hello = config.hello_with_wal_state(&wal).await;

        assert_eq!(hello.acked_runner_event_seq, Some(first.runner_event_seq));
        assert_eq!(
            hello.replay_from_runner_event_seq,
            Some(second.runner_event_seq)
        );

        let _ = tokio::fs::remove_file(&path).await;
        let _ = tokio::fs::remove_file(path.with_extension("jsonl.ack")).await;
    }

    #[test]
    fn command_handler_trait_is_object_safe_for_host_dispatch() {
        struct Handler;

        impl RunnerCommandHandler for Handler {
            fn handle_command<'a>(
                &'a mut self,
                _envelope: Box<RunnerCommandEnvelope>,
                _events: RunnerBackgroundEventSender,
            ) -> RunnerHostBoxFuture<'a, RunnerResponseOutcome> {
                Box::pin(async {
                    Ok(RunnerResponseOutcome::Ok {
                        result: RunnerCommandResult::Accepted,
                    })
                })
            }
        }

        let _handler: Box<dyn RunnerCommandHandler> = Box::new(Handler);
    }
}

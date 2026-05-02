use agenter_core::{AppEvent, SessionId};
use agenter_protocol::runner::{
    AgentEvent, RunnerClientMessage, RunnerCommand, RunnerEvent, RunnerHeartbeat,
    RunnerResponseEnvelope, RunnerServerMessage, PROTOCOL_VERSION,
};
use agenter_protocol::{
    chunk_message, reassemble_message, RunnerTransportChunkFrame, RunnerTransportChunkReassembler,
    RunnerTransportOutboundFrame,
};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::Response,
};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;

use crate::state::AppState;

const DEFAULT_RUNNER_WS_CHUNK_BYTES: usize = 1024 * 1024;
const DEFAULT_RUNNER_WS_MAX_MESSAGE_BYTES: usize = 512 * 1024 * 1024;

#[cfg(test)]
pub fn smoke_session_id() -> SessionId {
    SessionId::from_uuid(uuid::Uuid::from_u128(0x11111111111111111111111111111111))
}

pub async fn handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    tracing::debug!("runner websocket upgrade requested");
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    let Some(first) = receiver.next().await else {
        tracing::warn!("runner websocket closed before hello");
        return;
    };

    let mut runner_message_reassembler =
        RunnerTransportChunkReassembler::new(runner_ws_max_message_bytes());

    let Ok(Message::Text(text)) = first else {
        tracing::warn!("runner websocket first frame was not text");
        return;
    };

    let Ok(RunnerClientMessage::Hello(hello)) = serde_json::from_str::<RunnerClientMessage>(&text)
    else {
        tracing::warn!("runner websocket rejected invalid hello frame");
        return;
    };

    if hello.protocol_version != PROTOCOL_VERSION || !state.is_runner_token_valid(&hello.token) {
        tracing::warn!(
            runner_id = %hello.runner_id,
            protocol_version = %hello.protocol_version,
            expected_protocol_version = PROTOCOL_VERSION,
            token_valid = state.is_runner_token_valid(&hello.token),
            "runner websocket rejected hello"
        );
        return;
    }

    if hello.workspaces.is_empty() {
        tracing::warn!(runner_id = %hello.runner_id, "runner hello rejected without workspaces");
        return;
    };
    if hello.capabilities.agent_providers.is_empty() {
        tracing::warn!(runner_id = %hello.runner_id, "runner hello rejected without providers");
        return;
    };
    tracing::info!(
        runner_id = %hello.runner_id,
        workspace_count = hello.workspaces.len(),
        provider_count = hello.capabilities.agent_providers.len(),
        "runner hello accepted"
    );
    let runner = state
        .register_runner(
            hello.runner_id,
            hello.capabilities.clone(),
            hello.workspaces.clone(),
        )
        .await;
    let (outbound_sender, mut outbound_receiver) = mpsc::unbounded_channel();
    let connection_id = state
        .connect_runner(runner.runner_id, outbound_sender)
        .await;

    loop {
        tokio::select! {
            message = receiver.next() => {
                match message {
                    Some(Ok(Message::Text(text))) => {
                        match classify_runner_client_text(&mut runner_message_reassembler, &text) {
                            Ok(Some(RunnerClientFrame::Event(envelope))) => {
                                match envelope.event {
                                    RunnerEvent::AgentEvent(AgentEvent { session_id, event }) => {
                                    if app_event_session_id(&event) != Some(session_id) {
                                        tracing::warn!(
                                            %session_id,
                                            "runner event envelope session_id did not match embedded event"
                                        );
                                        continue;
                                    }
                                    state.publish_event(session_id, event).await;
                                    }
                                    RunnerEvent::SessionsDiscovered(discovered) => {
                                        let request_id = envelope.request_id.clone();
                                        let import_mode = if request_id.is_some() {
                                            crate::state::SessionImportMode::Forced
                                        } else {
                                            crate::state::SessionImportMode::Automatic
                                        };
                                        let summary = state
                                            .import_discovered_sessions(runner.runner_id, discovered, import_mode)
                                            .await;
                                        if let Some(request_id) = request_id {
                                            state.record_refresh_summary(request_id, summary).await;
                                        }
                                    }
                                    RunnerEvent::HealthChanged(_) | RunnerEvent::Error(_) => {}
                                }
                            }
                                Ok(Some(RunnerClientFrame::Response(response))) => {
                                    tracing::debug!(
                                        runner_id = %runner.runner_id,
                                        request_id = %response.request_id,
                                        "runner command response received"
                                    );
                                    state
                                        .finish_runner_response(
                                            runner.runner_id,
                                            response.request_id,
                                            response.outcome,
                                        )
                                        .await;
                                }
                            Ok(Some(RunnerClientFrame::Heartbeat(heartbeat))) => {
                                tracing::debug!(
                                    runner_id = %runner.runner_id,
                                    sequence = heartbeat.sequence,
                                    "runner heartbeat received"
                                );
                            }
                            Ok(Some(RunnerClientFrame::Hello)) => {
                                tracing::warn!(runner_id = %runner.runner_id, "runner hello received after handshake");
                            }
                            Ok(None) => {}
                            Err(error) => {
                                tracing::warn!(runner_id = %runner.runner_id, %error, "runner websocket ignored undecodable text frame");
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        tracing::info!(runner_id = %runner.runner_id, "runner websocket closed");
                        break;
                    }
                    Some(Err(error)) => {
                        tracing::warn!(runner_id = %runner.runner_id, %error, "runner websocket receive error");
                        break;
                    }
                    Some(Ok(_)) => {}
                }
            }
            outbound = outbound_receiver.recv() => {
                let Some(outbound) = outbound else {
                    break;
                };
                if let Some(approval_id) = approval_answer_id(&outbound.message) {
                    if !state.approval_is_resolving(approval_id).await {
                        tracing::warn!(
                            runner_id = %runner.runner_id,
                            %approval_id,
                            "dropped stale approval answer before runner delivery"
                        );
                        let _ = outbound.delivered.send(Err(crate::state::RunnerSendError::StaleApproval));
                        continue;
                    }
                }
                let result = send_server_message(&mut sender, outbound.message).await;
                let should_break = result.is_err();
                if should_break {
                    tracing::warn!(runner_id = %runner.runner_id, "runner websocket send failed");
                }
                let _ = outbound.delivered.send(result.map_err(|_| crate::state::RunnerSendError::Closed));
                if should_break {
                    break;
                }
            }
        }
    }

    state
        .disconnect_runner(runner.runner_id, connection_id)
        .await;
}

fn approval_answer_id(message: &RunnerServerMessage) -> Option<agenter_core::ApprovalId> {
    let RunnerServerMessage::Command(command) = message else {
        return None;
    };
    let RunnerCommand::AnswerApproval(answer) = &command.command else {
        return None;
    };
    Some(answer.approval_id)
}

async fn send_server_message(
    sender: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    message: RunnerServerMessage,
) -> Result<(), axum::Error> {
    let frames =
        chunk_message(&message, runner_ws_chunk_bytes()).expect("runner server message serializes");
    if frames.len() > 1 {
        let chunk_start = runner_transport_chunk_start(&frames);
        tracing::warn!(
            direction = "control_plane_to_runner",
            message_type = runner_server_message_type(&message),
            transfer_id = chunk_start.as_ref().map(|start| start.transfer_id.as_str()),
            total_bytes = chunk_start.as_ref().map(|start| start.total_bytes),
            frame_count = frames.len(),
            total_chunks = chunk_start.as_ref().map(|start| start.total_chunks),
            "sending chunked runner websocket message"
        );
    }
    for frame in frames {
        let RunnerTransportOutboundFrame::Text(text) = frame;
        sender.send(Message::Text(text.into())).await?;
    }
    Ok(())
}

fn runner_server_message_type(message: &RunnerServerMessage) -> &'static str {
    match message {
        RunnerServerMessage::Command(_) => "runner_command",
        RunnerServerMessage::HeartbeatAck(_) => "runner_heartbeat_ack",
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

#[derive(Debug)]
enum RunnerClientFrame {
    Hello,
    Heartbeat(RunnerHeartbeat),
    Response(RunnerResponseEnvelope),
    Event(agenter_protocol::runner::RunnerEventEnvelope),
}

fn classify_runner_client_text(
    reassembler: &mut RunnerTransportChunkReassembler,
    text: &str,
) -> Result<Option<RunnerClientFrame>, agenter_protocol::runner_transport::RunnerTransportError> {
    let Some(message) = reassemble_message::<RunnerClientMessage>(reassembler, text)? else {
        return Ok(None);
    };
    match message {
        RunnerClientMessage::Hello(_) => Ok(Some(RunnerClientFrame::Hello)),
        RunnerClientMessage::Heartbeat(heartbeat) => {
            Ok(Some(RunnerClientFrame::Heartbeat(heartbeat)))
        }
        RunnerClientMessage::Response(response) => Ok(Some(RunnerClientFrame::Response(response))),
        RunnerClientMessage::Event(event) => Ok(Some(RunnerClientFrame::Event(event))),
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

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn app_event_session_id(event: &AppEvent) -> Option<SessionId> {
    match event {
        AppEvent::SessionStarted(info) => Some(info.session_id),
        AppEvent::SessionStatusChanged(event) => Some(event.session_id),
        AppEvent::UserMessage(event) => Some(event.session_id),
        AppEvent::AgentMessageDelta(event) => Some(event.session_id),
        AppEvent::AgentMessageCompleted(event) => Some(event.session_id),
        AppEvent::PlanUpdated(event) => Some(event.session_id),
        AppEvent::ToolStarted(event)
        | AppEvent::ToolUpdated(event)
        | AppEvent::ToolCompleted(event) => Some(event.session_id),
        AppEvent::CommandStarted(event) => Some(event.session_id),
        AppEvent::CommandOutputDelta(event) => Some(event.session_id),
        AppEvent::CommandCompleted(event) => Some(event.session_id),
        AppEvent::FileChangeProposed(event)
        | AppEvent::FileChangeApplied(event)
        | AppEvent::FileChangeRejected(event) => Some(event.session_id),
        AppEvent::ApprovalRequested(event) => Some(event.session_id),
        AppEvent::ApprovalResolved(event) => Some(event.session_id),
        AppEvent::QuestionRequested(event) => Some(event.session_id),
        AppEvent::QuestionAnswered(event) => Some(event.session_id),
        AppEvent::TurnDiffUpdated(event)
        | AppEvent::ItemReasoning(event)
        | AppEvent::ServerRequestResolved(event)
        | AppEvent::McpToolCallProgress(event)
        | AppEvent::ThreadRealtimeEvent(event)
        | AppEvent::ProviderEvent(event) => Some(event.session_id),
        AppEvent::Error(event) => event.session_id,
    }
}

#[cfg(test)]
mod tests {
    use agenter_protocol::{
        runner::{
            RunnerClientMessage, RunnerCommandResult, RunnerResponseEnvelope, RunnerResponseOutcome,
        },
        RequestId,
    };

    use super::*;

    #[test]
    fn classifies_runner_response_as_valid_runner_frame() {
        let text = serde_json::to_string(&RunnerClientMessage::Response(RunnerResponseEnvelope {
            request_id: RequestId::from("request-1"),
            outcome: RunnerResponseOutcome::Ok {
                result: RunnerCommandResult::Accepted,
            },
        }))
        .expect("serialize runner response");

        assert!(matches!(
            classify_runner_client_text(
                &mut RunnerTransportChunkReassembler::new(runner_ws_max_message_bytes()),
                &text
            ),
            Ok(Some(RunnerClientFrame::Response(_)))
        ));
    }
}

#[cfg(test)]
use agenter_core::SessionId;
use agenter_protocol::runner::{
    RunnerClientMessage, RunnerCommand, RunnerEvent, RunnerEventAck, RunnerEventEnvelope,
    RunnerHeartbeat, RunnerResponseEnvelope, RunnerServerMessage, PROTOCOL_VERSION,
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

#[derive(Debug)]
struct DiscoveryImportCompletion {
    runner_event_seq: Option<u64>,
    accepted: bool,
}

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
    state
        .seed_runner_event_ack(runner.runner_id, hello.acked_runner_event_seq)
        .await;
    let (discovery_import_sender, mut discovery_import_receiver) =
        mpsc::unbounded_channel::<DiscoveryImportCompletion>();

    loop {
        tokio::select! {
            completion = discovery_import_receiver.recv() => {
                let Some(completion) = completion else {
                    continue;
                };
                if completion.accepted {
                    if let Some(seq) = completion.runner_event_seq {
                        state.mark_runner_event_accepted(runner.runner_id, seq).await;
                        if let Err(error) = send_server_message(
                            &mut sender,
                            RunnerServerMessage::EventAck(RunnerEventAck {
                                runner_event_seq: seq,
                            }),
                        )
                        .await
                        {
                            tracing::warn!(
                                runner_id = %runner.runner_id,
                                runner_event_seq = seq,
                                %error,
                                "runner event ack send failed"
                            );
                            break;
                        }
                    }
                }
            }
            message = receiver.next() => {
                match message {
                    Some(Ok(Message::Text(text))) => {
                        match classify_runner_client_text(&mut runner_message_reassembler, &text) {
                            Ok(Some(RunnerClientFrame::Event(envelope))) => {
                                let runner_event_seq = envelope.runner_event_seq;
                                let duplicate = state
                                    .runner_event_already_accepted(
                                        runner.runner_id,
                                        runner_event_seq,
                                    )
                                    .await;
                                let mut accepted = duplicate;
                                if !duplicate {
                                    match envelope.event {
                                        RunnerEvent::AgentEvent(agent_event) => {
                                            match state.accept_runner_agent_event(*agent_event).await {
                                                Ok(_) => {
                                                    accepted = true;
                                                }
                                                Err(error) => {
                                                    tracing::warn!(
                                                        runner_id = %runner.runner_id,
                                                        runner_event_seq = ?runner_event_seq,
                                                        %error,
                                                        "runner app event was not accepted; withholding ack"
                                                    );
                                                }
                                            }
                                        }
                                        RunnerEvent::SessionsDiscovered(discovered) => {
                                            accepted = false;
                                            let state = state.clone();
                                            let completion_sender = discovery_import_sender.clone();
                                            let request_id = envelope.request_id.clone();
                                            let runner_id = runner.runner_id;
                                            tokio::spawn(async move {
                                                let db_backed = state.db_pool().is_some();
                                                let accepted = state
                                                    .process_runner_discovered_sessions(
                                                        runner_id,
                                                        request_id,
                                                        discovered,
                                                    )
                                                    .await;
                                                if db_backed && !accepted {
                                                    tracing::warn!(
                                                        %runner_id,
                                                        runner_event_seq = ?runner_event_seq,
                                                        "processed discovered sessions but withholding ack because import did not complete successfully"
                                                    );
                                                }
                                                completion_sender
                                                    .send(DiscoveryImportCompletion {
                                                        runner_event_seq,
                                                        accepted,
                                                    })
                                                    .ok();
                                            });
                                            if runner_event_seq.is_none() {
                                                accepted = true;
                                            }
                                        }
                                        RunnerEvent::HealthChanged(_) => {
                                            accepted = true;
                                        }
                                        RunnerEvent::OperationUpdated(update) => {
                                            state.record_refresh_operation_update(update).await;
                                            accepted = true;
                                        }
                                        RunnerEvent::Error(error) => {
                                            if let Some(request_id) = envelope.request_id {
                                                state
                                                    .fail_workspace_session_refresh(
                                                        request_id,
                                                        format!("{}: {}", error.code, error.message),
                                                    )
                                                    .await;
                                            }
                                            accepted = true;
                                        }
                                    }
                                }
                                if accepted {
                                if let Some(seq) = runner_event_seq {
                                    state.mark_runner_event_accepted(runner.runner_id, seq).await;
                                    if let Err(error) = send_server_message(
                                        &mut sender,
                                        RunnerServerMessage::EventAck(RunnerEventAck {
                                            runner_event_seq: seq,
                                        }),
                                    )
                                    .await
                                    {
                                        tracing::warn!(
                                            runner_id = %runner.runner_id,
                                            runner_event_seq = seq,
                                            %error,
                                            "runner event ack send failed"
                                        );
                                        break;
                                    }
                                }
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
        RunnerServerMessage::EventAck(_) => "runner_event_ack",
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
    Event(RunnerEventEnvelope),
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
        RunnerClientMessage::Event(event) => Ok(Some(RunnerClientFrame::Event(*event))),
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

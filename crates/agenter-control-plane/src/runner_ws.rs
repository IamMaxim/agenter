use agenter_core::SessionId;
use agenter_protocol::runner::{
    AgentEvent, AgentInput, AgentInputCommand, RunnerClientMessage, RunnerCommand,
    RunnerCommandEnvelope, RunnerEvent, RunnerServerMessage, PROTOCOL_VERSION,
};
use agenter_protocol::RequestId;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::Response,
};
use futures_util::{SinkExt, StreamExt};
use uuid::Uuid;

use crate::state::AppState;

const SMOKE_REQUEST_ID: &str = "smoke-input-1";

pub fn smoke_session_id() -> SessionId {
    SessionId::from_uuid(Uuid::from_u128(0x11111111111111111111111111111111))
}

pub async fn handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    let Some(first) = receiver.next().await else {
        return;
    };

    let Ok(Message::Text(text)) = first else {
        return;
    };

    let Ok(RunnerClientMessage::Hello(hello)) = serde_json::from_str::<RunnerClientMessage>(&text)
    else {
        return;
    };

    if hello.protocol_version != PROTOCOL_VERSION || !state.is_runner_token_valid(&hello.token) {
        return;
    }

    let session_id = smoke_session_id();
    let command = RunnerServerMessage::Command(Box::new(RunnerCommandEnvelope {
        request_id: RequestId::from(SMOKE_REQUEST_ID),
        command: RunnerCommand::AgentSendInput(AgentInputCommand {
            session_id,
            external_session_id: None,
            input: AgentInput::Text {
                text: "hello from control plane".to_owned(),
            },
        }),
    }));

    if send_server_message(&mut sender, command).await.is_err() {
        return;
    }

    while let Some(message) = receiver.next().await {
        match message {
            Ok(Message::Text(text)) => {
                if let Ok(RunnerClientMessage::Event(envelope)) =
                    serde_json::from_str::<RunnerClientMessage>(&text)
                {
                    if let RunnerEvent::AgentEvent(AgentEvent { session_id, event }) =
                        envelope.event
                    {
                        state.publish_event(session_id, event).await;
                    }
                }
            }
            Ok(Message::Close(_)) | Err(_) => return,
            Ok(_) => {}
        }
    }
}

async fn send_server_message(
    sender: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    message: RunnerServerMessage,
) -> Result<(), axum::Error> {
    let json = serde_json::to_string(&message).expect("runner server message serializes");
    sender.send(Message::Text(json.into())).await
}

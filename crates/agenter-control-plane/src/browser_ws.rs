use agenter_protocol::browser::{
    BrowserAck, BrowserClientMessage, BrowserError, BrowserEventEnvelope, BrowserServerMessage,
};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::Response,
};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::broadcast;

use crate::state::AppState;

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

    let Ok(BrowserClientMessage::SubscribeSession(subscription)) =
        serde_json::from_str::<BrowserClientMessage>(&text)
    else {
        let _ = send_server_message(
            &mut sender,
            BrowserServerMessage::Error(BrowserError {
                request_id: None,
                code: "invalid_message".to_owned(),
                message: "expected subscribe_session".to_owned(),
            }),
        )
        .await;
        return;
    };

    if send_server_message(
        &mut sender,
        BrowserServerMessage::Ack(BrowserAck {
            request_id: subscription.request_id.clone(),
        }),
    )
    .await
    .is_err()
    {
        return;
    }

    for event in state.cached_events(subscription.session_id).await {
        if send_server_message(
            &mut sender,
            BrowserServerMessage::Event(BrowserEventEnvelope {
                event_id: None,
                event,
            }),
        )
        .await
        .is_err()
        {
            return;
        }
    }

    let mut events = state.subscribe_session(subscription.session_id).await;
    loop {
        tokio::select! {
            message = receiver.next() => {
                match message {
                    Some(Ok(Message::Text(text))) => {
                        if matches!(
                            serde_json::from_str::<BrowserClientMessage>(&text),
                            Ok(BrowserClientMessage::UnsubscribeSession(_))
                        ) {
                            return;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => return,
                    Some(Ok(_)) => {}
                    Some(Err(_)) => return,
                }
            }
            event = events.recv() => {
                match event {
                    Ok(event) => {
                        if send_server_message(
                            &mut sender,
                            BrowserServerMessage::Event(BrowserEventEnvelope {
                                event_id: None,
                                event,
                            }),
                        )
                        .await
                        .is_err()
                        {
                            return;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        }
    }
}

async fn send_server_message(
    sender: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    message: BrowserServerMessage,
) -> Result<(), axum::Error> {
    let json = serde_json::to_string(&message).expect("browser server message serializes");
    sender.send(Message::Text(json.into())).await
}

use agenter_core::UserId;
use agenter_protocol::browser::{
    BrowserAck, BrowserClientMessage, BrowserError, BrowserServerMessage,
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

pub async fn handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    user_id: UserId,
) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state, user_id))
}

async fn handle_socket(socket: WebSocket, state: AppState, user_id: UserId) {
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

    if !state
        .can_access_session(user_id, subscription.session_id)
        .await
    {
        let _ = send_server_message(
            &mut sender,
            BrowserServerMessage::Error(BrowserError {
                request_id: subscription.request_id,
                code: "forbidden".to_owned(),
                message: "session is not accessible by this user".to_owned(),
            }),
        )
        .await;
        return;
    }

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

    let (cached_events, mut events) = state.subscribe_session(subscription.session_id).await;
    for event in cached_events {
        if send_server_message(&mut sender, BrowserServerMessage::Event(event))
            .await
            .is_err()
        {
            return;
        }
    }

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
                        if send_server_message(&mut sender, BrowserServerMessage::Event(event))
                        .await
                        .is_err()
                        {
                            return;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        let _ = send_server_message(
                            &mut sender,
                            BrowserServerMessage::Error(BrowserError {
                                request_id: None,
                                code: "event_lagged".to_owned(),
                                message: format!(
                                    "browser event stream lagged by {skipped} messages; resubscribe to replay cache"
                                ),
                            }),
                        )
                        .await;
                        return;
                    }
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

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
    tracing::debug!(%user_id, "browser websocket upgrade requested");
    ws.on_upgrade(move |socket| handle_socket(socket, state, user_id))
}

async fn handle_socket(socket: WebSocket, state: AppState, user_id: UserId) {
    let (mut sender, mut receiver) = socket.split();
    let Some(first) = receiver.next().await else {
        tracing::warn!(%user_id, "browser websocket closed before subscription");
        return;
    };

    let Ok(Message::Text(text)) = first else {
        tracing::warn!(%user_id, "browser websocket first frame was not text");
        return;
    };

    let Ok(BrowserClientMessage::SubscribeSession(subscription)) =
        serde_json::from_str::<BrowserClientMessage>(&text)
    else {
        tracing::warn!(%user_id, "browser websocket rejected invalid subscription frame");
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
        tracing::warn!(
            %user_id,
            session_id = %subscription.session_id,
            "browser websocket subscription forbidden"
        );
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
        tracing::warn!(%user_id, session_id = %subscription.session_id, "browser websocket failed to send subscription ack");
        return;
    }

    let universal_subscription = subscription.include_snapshot || subscription.after_seq.is_some();
    let mut session_subscription = state
        .subscribe_session(
            subscription.session_id,
            subscription.after_seq,
            subscription.include_snapshot,
        )
        .await;
    let mut sent_universal_events = std::collections::HashSet::new();
    if let Some(mut snapshot) = session_subscription.snapshot.take() {
        snapshot.request_id = subscription.request_id.clone();
        let replay_incomplete = snapshot.has_more;
        let latest_seq = snapshot.latest_seq;
        for event in &snapshot.events {
            sent_universal_events.insert((event.seq, event.event_id.clone()));
        }
        if send_server_message(&mut sender, BrowserServerMessage::SessionSnapshot(snapshot))
            .await
            .is_err()
        {
            tracing::warn!(%user_id, session_id = %subscription.session_id, "browser websocket failed to send session snapshot");
            return;
        }
        if universal_subscription && replay_incomplete && !subscription.include_snapshot {
            let _ = send_server_message(
                &mut sender,
                BrowserServerMessage::Error(BrowserError {
                    request_id: subscription.request_id.clone(),
                    code: "snapshot_replay_incomplete".to_owned(),
                    message: format!(
                        "universal event replay is incomplete; resubscribe/page from snapshot.latest_seq {latest_seq:?} before consuming live events"
                    ),
                }),
            )
            .await;
            tracing::info!(
                %user_id,
                session_id = %subscription.session_id,
                "browser websocket closed universal subscription after incomplete replay"
            );
            return;
        }
    }
    tracing::info!(
        %user_id,
        session_id = %subscription.session_id,
        "browser websocket subscribed"
    );

    loop {
        tokio::select! {
            message = receiver.next() => {
                match message {
                    Some(Ok(Message::Text(text))) => {
                        if matches!(
                            serde_json::from_str::<BrowserClientMessage>(&text),
                            Ok(BrowserClientMessage::UnsubscribeSession(_))
                        ) {
                            tracing::info!(%user_id, session_id = %subscription.session_id, "browser websocket unsubscribed");
                            return;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        tracing::info!(%user_id, session_id = %subscription.session_id, "browser websocket closed");
                        return;
                    }
                    Some(Ok(_)) => {}
                    Some(Err(error)) => {
                        tracing::warn!(%user_id, session_id = %subscription.session_id, %error, "browser websocket receive error");
                        return;
                    }
                }
            }
            event = session_subscription.receiver.recv() => {
                match event {
                    Ok(event) => {
                        if let Some(universal_event) = event.universal_event {
                            if !sent_universal_events
                                .insert((universal_event.seq, universal_event.event_id.clone()))
                            {
                                continue;
                            }
                            if send_server_message(
                                &mut sender,
                                BrowserServerMessage::UniversalEvent(universal_event),
                            )
                            .await
                            .is_err()
                            {
                                tracing::warn!(%user_id, session_id = %subscription.session_id, "browser websocket send universal event failed");
                                return;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(%user_id, session_id = %subscription.session_id, skipped, "browser websocket lagged");
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
                    Err(broadcast::error::RecvError::Closed) => {
                        tracing::warn!(%user_id, session_id = %subscription.session_id, "browser websocket event channel closed");
                        return;
                    }
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

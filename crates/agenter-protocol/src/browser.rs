use agenter_core::{AppEvent, SessionId};
use serde::{Deserialize, Serialize};

pub use crate::{EventId, RequestId};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BrowserClientMessage {
    SubscribeSession(SubscribeSession),
    UnsubscribeSession(SubscribeSession),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SubscribeSession {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<RequestId>,
    pub session_id: SessionId,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BrowserServerMessage {
    #[serde(rename = "app_event")]
    Event(BrowserEventEnvelope),
    Ack(BrowserAck),
    Error(BrowserError),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct BrowserEventEnvelope {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_id: Option<EventId>,
    pub event: AppEvent,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BrowserAck {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<RequestId>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BrowserError {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<RequestId>,
    pub code: String,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use agenter_core::{AppEvent, SessionId, UserMessageEvent};

    use super::*;

    #[test]
    fn round_trips_browser_session_subscription() {
        let message = BrowserClientMessage::SubscribeSession(SubscribeSession {
            request_id: Some(RequestId::from("sub-1")),
            session_id: SessionId::nil(),
        });

        let json = serde_json::to_value(&message).expect("serialize subscribe");
        let decoded: BrowserClientMessage =
            serde_json::from_value(json.clone()).expect("deserialize subscribe");

        assert_eq!(json["type"], "subscribe_session");
        assert_eq!(json["request_id"], "sub-1");
        assert_eq!(json["session_id"], SessionId::nil().to_string());
        assert_eq!(decoded, message);
    }

    #[test]
    fn round_trips_browser_app_event_envelope() {
        let message = BrowserServerMessage::Event(BrowserEventEnvelope {
            event_id: Some(EventId::from("evt-1")),
            event: AppEvent::UserMessage(UserMessageEvent {
                session_id: SessionId::nil(),
                message_id: Some("msg-1".to_owned()),
                author_user_id: None,
                content: "hello".to_owned(),
            }),
        });

        let json = serde_json::to_value(&message).expect("serialize browser event");
        let decoded: BrowserServerMessage =
            serde_json::from_value(json.clone()).expect("deserialize browser event");

        assert_eq!(json["type"], "app_event");
        assert_eq!(json["event_id"], "evt-1");
        assert_eq!(json["event"]["type"], "user_message");
        assert_eq!(decoded, message);
    }
}

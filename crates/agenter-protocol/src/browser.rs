use agenter_core::{SessionId, SessionSnapshot, UniversalEventEnvelope, UniversalSeq};
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_seq: Option<UniversalSeq>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub include_snapshot: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BrowserServerMessage {
    UniversalEvent(UniversalEventEnvelope),
    SessionSnapshot(BrowserSessionSnapshot),
    Ack(BrowserAck),
    Error(BrowserError),
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct BrowserSessionSnapshot {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<RequestId>,
    pub snapshot: SessionSnapshot,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<UniversalEventEnvelope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_seq: Option<UniversalSeq>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub has_more: bool,
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
    use agenter_core::{
        SessionId, SessionSnapshot, UniversalEventEnvelope, UniversalEventKind,
        UniversalEventSource, UniversalSeq,
    };

    use super::*;

    #[test]
    fn round_trips_browser_session_subscription() {
        let message = BrowserClientMessage::SubscribeSession(SubscribeSession {
            request_id: Some(RequestId::from("sub-1")),
            session_id: SessionId::nil(),
            after_seq: Some(UniversalSeq::new(41)),
            include_snapshot: true,
        });

        let json = serde_json::to_value(&message).expect("serialize subscribe");
        let decoded: BrowserClientMessage =
            serde_json::from_value(json.clone()).expect("deserialize subscribe");

        assert_eq!(json["type"], "subscribe_session");
        assert_eq!(json["request_id"], "sub-1");
        assert_eq!(json["session_id"], SessionId::nil().to_string());
        assert_eq!(json["after_seq"], "41");
        assert_eq!(json["include_snapshot"], true);
        assert_eq!(decoded, message);
    }

    #[test]
    fn decodes_legacy_browser_subscription_without_replay_options() {
        let json = serde_json::json!({
            "type": "subscribe_session",
            "request_id": "sub-legacy",
            "session_id": SessionId::nil()
        });

        let decoded: BrowserClientMessage =
            serde_json::from_value(json).expect("deserialize legacy subscribe");

        match decoded {
            BrowserClientMessage::SubscribeSession(subscription) => {
                assert_eq!(subscription.request_id, Some(RequestId::from("sub-legacy")));
                assert_eq!(subscription.session_id, SessionId::nil());
                assert_eq!(subscription.after_seq, None);
                assert!(!subscription.include_snapshot);
            }
            other => panic!("unexpected message {other:?}"),
        }
    }

    #[test]
    fn round_trips_browser_session_snapshot_message() {
        let event = UniversalEventEnvelope {
            event_id: "evt-1".to_owned(),
            seq: UniversalSeq::new(7),
            session_id: SessionId::nil(),
            turn_id: None,
            item_id: None,
            ts: serde_json::from_value(serde_json::json!("2026-05-03T13:00:00Z"))
                .expect("valid timestamp"),
            source: UniversalEventSource::ControlPlane,
            native: None,
            event: UniversalEventKind::NativeUnknown {
                summary: Some("legacy projection".to_owned()),
            },
        };
        let message = BrowserServerMessage::SessionSnapshot(BrowserSessionSnapshot {
            request_id: Some(RequestId::from("snapshot-1")),
            snapshot: SessionSnapshot {
                session_id: SessionId::nil(),
                latest_seq: Some(UniversalSeq::new(7)),
                ..SessionSnapshot::default()
            },
            events: vec![event],
            latest_seq: Some(UniversalSeq::new(7)),
            has_more: false,
        });

        let json = serde_json::to_value(&message).expect("serialize snapshot");
        let decoded: BrowserServerMessage =
            serde_json::from_value(json.clone()).expect("deserialize snapshot");

        assert_eq!(json["type"], "session_snapshot");
        assert_eq!(json["latest_seq"], "7");
        assert_eq!(json.get("has_more"), None);
        assert_eq!(json["events"][0]["event"]["type"], "native.unknown");
        assert_eq!(decoded, message);
    }

    #[test]
    fn round_trips_live_universal_event_message() {
        let event = UniversalEventEnvelope {
            event_id: "evt-live".to_owned(),
            seq: UniversalSeq::new(9),
            session_id: SessionId::nil(),
            turn_id: None,
            item_id: None,
            ts: serde_json::from_value(serde_json::json!("2026-05-03T13:01:00Z"))
                .expect("valid timestamp"),
            source: UniversalEventSource::ControlPlane,
            native: None,
            event: UniversalEventKind::NativeUnknown {
                summary: Some("live".to_owned()),
            },
        };
        let message = BrowserServerMessage::UniversalEvent(event.clone());

        let json = serde_json::to_value(&message).expect("serialize universal event");
        let decoded: BrowserServerMessage =
            serde_json::from_value(json.clone()).expect("deserialize universal event");

        assert_eq!(json["type"], "universal_event");
        assert_eq!(json["seq"], "9");
        assert_eq!(decoded, message);
    }

    #[test]
    fn browser_session_snapshot_allows_empty_latest_seq() {
        let message = BrowserServerMessage::SessionSnapshot(BrowserSessionSnapshot {
            request_id: Some(RequestId::from("snapshot-empty")),
            snapshot: SessionSnapshot {
                session_id: SessionId::nil(),
                latest_seq: None,
                ..SessionSnapshot::default()
            },
            events: Vec::new(),
            latest_seq: None,
            has_more: false,
        });

        let json = serde_json::to_value(&message).expect("serialize empty snapshot");
        let decoded: BrowserServerMessage =
            serde_json::from_value(json.clone()).expect("deserialize empty snapshot");

        assert_eq!(json["type"], "session_snapshot");
        assert!(json.get("latest_seq").is_none());
        assert_eq!(decoded, message);
    }

    #[test]
    fn session_snapshot_can_mark_truncated_replay() {
        let message = BrowserServerMessage::SessionSnapshot(BrowserSessionSnapshot {
            request_id: None,
            snapshot: SessionSnapshot {
                session_id: SessionId::nil(),
                latest_seq: Some(UniversalSeq::new(100)),
                ..SessionSnapshot::default()
            },
            events: Vec::new(),
            latest_seq: Some(UniversalSeq::new(50)),
            has_more: true,
        });

        let json = serde_json::to_value(&message).expect("serialize truncated snapshot");
        let decoded: BrowserServerMessage =
            serde_json::from_value(json.clone()).expect("deserialize truncated snapshot");

        assert_eq!(json["type"], "session_snapshot");
        assert_eq!(json["latest_seq"], "50");
        assert_eq!(json["has_more"], true);
        assert_eq!(decoded, message);
    }
}

use agenter_core::{
    SessionId, SessionSnapshot, UniversalEventEnvelope, UniversalSeq, UNIVERSAL_PROTOCOL_VERSION,
};
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

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct BrowserSessionSnapshot {
    #[serde(default = "browser_universal_protocol_version")]
    pub protocol_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<RequestId>,
    pub snapshot: SessionSnapshot,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<UniversalEventEnvelope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_seq: Option<UniversalSeq>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replay_from_seq: Option<UniversalSeq>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replay_through_seq: Option<UniversalSeq>,
    pub replay_complete: bool,
}

fn browser_universal_protocol_version() -> String {
    UNIVERSAL_PROTOCOL_VERSION.to_owned()
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
                summary: Some("native event".to_owned()),
            },
        };
        let message = BrowserServerMessage::SessionSnapshot(BrowserSessionSnapshot {
            protocol_version: UNIVERSAL_PROTOCOL_VERSION.to_owned(),
            request_id: Some(RequestId::from("snapshot-1")),
            snapshot: SessionSnapshot {
                session_id: SessionId::nil(),
                latest_seq: Some(UniversalSeq::new(7)),
                ..SessionSnapshot::default()
            },
            events: vec![event],
            snapshot_seq: Some(UniversalSeq::new(7)),
            replay_from_seq: Some(UniversalSeq::new(7)),
            replay_through_seq: Some(UniversalSeq::new(7)),
            replay_complete: true,
        });

        let json = serde_json::to_value(&message).expect("serialize snapshot");
        let decoded: BrowserServerMessage =
            serde_json::from_value(json.clone()).expect("deserialize snapshot");

        assert_eq!(json["type"], "session_snapshot");
        assert_eq!(json["protocol_version"], "uap/2");
        assert_eq!(json["snapshot_seq"], "7");
        assert_eq!(json["replay_from_seq"], "7");
        assert_eq!(json["replay_through_seq"], "7");
        assert_eq!(json["replay_complete"], true);
        assert!(json.get("latest_seq").is_none());
        assert!(json.get("has_more").is_none());
        assert_eq!(json["events"][0]["protocol_version"], "uap/2");
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
        assert_eq!(json["protocol_version"], "uap/2");
        assert_eq!(json["seq"], "9");
        assert_eq!(decoded, message);
    }

    #[test]
    fn browser_session_snapshot_allows_empty_latest_seq() {
        let message = BrowserServerMessage::SessionSnapshot(BrowserSessionSnapshot {
            protocol_version: UNIVERSAL_PROTOCOL_VERSION.to_owned(),
            request_id: Some(RequestId::from("snapshot-empty")),
            snapshot: SessionSnapshot {
                session_id: SessionId::nil(),
                latest_seq: None,
                ..SessionSnapshot::default()
            },
            events: Vec::new(),
            snapshot_seq: None,
            replay_from_seq: None,
            replay_through_seq: None,
            replay_complete: true,
        });

        let json = serde_json::to_value(&message).expect("serialize empty snapshot");
        let decoded: BrowserServerMessage =
            serde_json::from_value(json.clone()).expect("deserialize empty snapshot");

        assert_eq!(json["type"], "session_snapshot");
        assert!(json.get("latest_seq").is_none());
        assert_eq!(json["replay_complete"], true);
        assert_eq!(decoded, message);
    }

    #[test]
    fn session_snapshot_can_mark_incomplete_replay() {
        let message = BrowserServerMessage::SessionSnapshot(BrowserSessionSnapshot {
            protocol_version: UNIVERSAL_PROTOCOL_VERSION.to_owned(),
            request_id: None,
            snapshot: SessionSnapshot {
                session_id: SessionId::nil(),
                latest_seq: Some(UniversalSeq::new(100)),
                ..SessionSnapshot::default()
            },
            events: Vec::new(),
            snapshot_seq: Some(UniversalSeq::new(100)),
            replay_from_seq: None,
            replay_through_seq: Some(UniversalSeq::new(50)),
            replay_complete: false,
        });

        let json = serde_json::to_value(&message).expect("serialize truncated snapshot");
        let decoded: BrowserServerMessage =
            serde_json::from_value(json.clone()).expect("deserialize truncated snapshot");

        assert_eq!(json["type"], "session_snapshot");
        assert_eq!(json["snapshot_seq"], "100");
        assert_eq!(json["replay_through_seq"], "50");
        assert_eq!(json["replay_complete"], false);
        assert!(json.get("latest_seq").is_none());
        assert!(json.get("has_more").is_none());
        assert_eq!(decoded, message);
    }
}

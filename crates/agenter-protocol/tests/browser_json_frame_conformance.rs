use agenter_core::{
    SessionId, SessionSnapshot, UniversalEventEnvelope, UniversalEventKind, UniversalEventSource,
    UniversalSeq, UNIVERSAL_PROTOCOL_VERSION,
};
use agenter_protocol::{BrowserServerMessage, BrowserSessionSnapshot, RequestId};

fn native_unknown(seq: i64, summary: &str) -> UniversalEventEnvelope {
    UniversalEventEnvelope {
        event_id: format!("evt-{seq}"),
        seq: UniversalSeq::new(seq),
        session_id: SessionId::nil(),
        turn_id: None,
        item_id: None,
        ts: serde_json::from_value(serde_json::json!("2026-05-05T12:00:00Z"))
            .expect("valid timestamp"),
        source: UniversalEventSource::Runner,
        native: None,
        event: UniversalEventKind::NativeUnknown {
            summary: Some(summary.to_owned()),
        },
    }
}

#[test]
fn versioned_snapshot_replay_frame_matches_uap_json_contract() {
    let message = BrowserServerMessage::SessionSnapshot(BrowserSessionSnapshot {
        protocol_version: UNIVERSAL_PROTOCOL_VERSION.to_owned(),
        request_id: Some(RequestId::from("snapshot-conformance")),
        snapshot: SessionSnapshot {
            session_id: SessionId::nil(),
            latest_seq: Some(UniversalSeq::new(10)),
            ..SessionSnapshot::default()
        },
        events: vec![
            native_unknown(11, "replayed"),
            native_unknown(12, "replayed live boundary"),
        ],
        snapshot_seq: Some(UniversalSeq::new(10)),
        replay_from_seq: Some(UniversalSeq::new(11)),
        replay_through_seq: Some(UniversalSeq::new(12)),
        replay_complete: true,
    });

    let json = serde_json::to_value(&message).expect("serialize browser snapshot");

    assert_eq!(json["type"], "session_snapshot");
    assert_eq!(json["protocol_version"], "uap/2");
    assert_eq!(json["request_id"], "snapshot-conformance");
    assert_eq!(json["snapshot"]["latest_seq"], "10");
    assert_eq!(json["snapshot_seq"], "10");
    assert_eq!(json["replay_from_seq"], "11");
    assert_eq!(json["replay_through_seq"], "12");
    assert_eq!(json["replay_complete"], true);
    assert!(
        json.get("latest_seq").is_none(),
        "uap/2 snapshots use explicit replay cursors"
    );
    assert!(
        json.get("has_more").is_none(),
        "uap/2 snapshots use replay_complete"
    );
    assert_eq!(json["events"][0]["protocol_version"], "uap/2");
    assert_eq!(json["events"][0]["seq"], "11");
    assert_eq!(json["events"][1]["protocol_version"], "uap/2");
    assert_eq!(json["events"][1]["seq"], "12");

    let decoded: BrowserServerMessage =
        serde_json::from_value(json).expect("deserialize browser snapshot");
    assert_eq!(decoded, message);
}

#[test]
fn versioned_truncated_snapshot_frame_keeps_replay_boundary_explicit() {
    let message = BrowserServerMessage::SessionSnapshot(BrowserSessionSnapshot {
        protocol_version: UNIVERSAL_PROTOCOL_VERSION.to_owned(),
        request_id: None,
        snapshot: SessionSnapshot {
            session_id: SessionId::nil(),
            latest_seq: Some(UniversalSeq::new(20)),
            ..SessionSnapshot::default()
        },
        events: vec![native_unknown(15, "bounded replay page")],
        snapshot_seq: Some(UniversalSeq::new(20)),
        replay_from_seq: Some(UniversalSeq::new(15)),
        replay_through_seq: Some(UniversalSeq::new(15)),
        replay_complete: false,
    });

    let json = serde_json::to_value(&message).expect("serialize truncated snapshot");

    assert_eq!(json["type"], "session_snapshot");
    assert_eq!(json["protocol_version"], "uap/2");
    assert_eq!(json["snapshot"]["latest_seq"], "20");
    assert_eq!(json["snapshot_seq"], "20");
    assert_eq!(json["replay_from_seq"], "15");
    assert_eq!(json["replay_through_seq"], "15");
    assert_eq!(json["replay_complete"], false);
    assert!(
        json.get("latest_seq").is_none(),
        "uap/2 snapshots use explicit replay cursors"
    );
    assert!(
        json.get("has_more").is_none(),
        "uap/2 snapshots use replay_complete"
    );
    assert_eq!(json["events"][0]["protocol_version"], "uap/2");
    assert_eq!(json["events"][0]["seq"], "15");

    let decoded: BrowserServerMessage =
        serde_json::from_value(json).expect("deserialize truncated snapshot");
    assert_eq!(decoded, message);
}

#[test]
fn versioned_live_universal_event_frame_matches_uap_json_contract() {
    let event = native_unknown(30, "live");
    let message = BrowserServerMessage::UniversalEvent(event);

    let json = serde_json::to_value(&message).expect("serialize live universal event");

    assert_eq!(json["type"], "universal_event");
    assert_eq!(json["protocol_version"], "uap/2");
    assert_eq!(json["seq"], "30");
    assert_eq!(json["source"], "runner");
    assert_eq!(json["event"]["type"], "native.unknown");

    let decoded: BrowserServerMessage =
        serde_json::from_value(json).expect("deserialize live universal event");
    assert_eq!(decoded, message);
}

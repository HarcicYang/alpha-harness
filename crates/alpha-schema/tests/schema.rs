use std::str::FromStr;

use alpha_schema::{
    EventEnvelope, LiveAgentEvent, PermissionMode, ReasoningLevel, Seq, SessionEventNotification,
    SessionId, Timestamp,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Sample {
    text: String,
}

#[test]
fn ids_are_unique_and_round_trip() {
    let session = SessionId::new();

    let json = serde_json::to_string(&session).expect("serialize");
    let parsed: SessionId = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed, session);

    let text = session.to_string();
    assert_eq!(SessionId::from_str(&text).expect("parse"), session);
}

#[test]
fn envelopes_round_trip_with_payloads() {
    let envelope = EventEnvelope::at(
        Seq::new(7),
        Timestamp::from_millis(1_700_000_000_000),
        Sample {
            text: "hello".to_owned(),
        },
    );
    let json = serde_json::to_string(&envelope).expect("serialize");
    let parsed: EventEnvelope<Sample> = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed, envelope);
}

#[test]
fn permission_modes_serialize_as_lowercase() {
    assert_eq!(
        serde_json::to_string(&PermissionMode::Auto).expect("serialize"),
        "\"auto\""
    );
    assert_eq!(
        PermissionMode::from_str("reviewed").expect("parse"),
        PermissionMode::Reviewed
    );
    assert!(PermissionMode::from_str("nope").is_err());
    assert_eq!(PermissionMode::default(), PermissionMode::Auto);
}

#[test]
fn reasoning_levels_round_trip_with_stable_names() {
    assert_eq!(
        serde_json::to_string(&ReasoningLevel::XHigh).expect("serialize"),
        "\"xhigh\""
    );
    assert_eq!(
        serde_json::from_str::<ReasoningLevel>("\"max\"").expect("parse"),
        ReasoningLevel::Max
    );
    assert_eq!(ReasoningLevel::default(), ReasoningLevel::Auto);
}

#[test]
fn live_session_events_keep_the_protocol_shape() {
    let session_id = SessionId::new();
    let notification = SessionEventNotification::Live {
        session_id,
        turn_id: Some("turn-1".to_owned()),
        stream_id: Some("turn-1:text".to_owned()),
        step_id: None,
        event: LiveAgentEvent::TextDelta {
            text: "hello".to_owned(),
        },
    };

    let value = serde_json::to_value(&notification).expect("serialize notification");
    assert_eq!(
        value,
        serde_json::json!({
            "sessionId": session_id,
            "kind": "live",
            "turnId": "turn-1",
            "streamId": "turn-1:text",
            "event": {
                "type": "text_delta",
                "text": "hello"
            }
        })
    );

    let decoded: SessionEventNotification =
        serde_json::from_value(value).expect("deserialize notification");
    assert_eq!(decoded, notification);
}

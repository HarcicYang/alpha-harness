use std::str::FromStr;

use alpha_schema::{
    EventEnvelope, MessageId, PermissionMode, Seq, SessionId, Timestamp, ToolCallId,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Sample {
    text: String,
}

#[test]
fn ids_are_unique_and_round_trip() {
    let session = SessionId::new();
    let other = SessionId::new();
    assert_ne!(session, other);
    assert_ne!(MessageId::new(), MessageId::new());
    assert_ne!(ToolCallId::new(), ToolCallId::new());

    let json = serde_json::to_string(&session).expect("serialize");
    let parsed: SessionId = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed, session);

    let text = session.to_string();
    assert_eq!(SessionId::from_str(&text).expect("parse"), session);
}

#[test]
fn sequence_numbers_are_ordered() {
    assert!(Seq::new(1) < Seq::new(2));
    assert_eq!(Seq::new(1).next(), Seq::new(2));
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

//! Durable event envelopes.

use serde::{Deserialize, Serialize};

use crate::time::{Seq, Timestamp};

/// Schema version of the current durable payloads.
pub const CURRENT_SCHEMA: u32 = 6;

/// One durable event in a session log.
///
/// The payload type is owned by the module that defines the event taxonomy,
/// while the envelope stays stable across modules.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventEnvelope<T> {
    /// Monotonically increasing sequence number.
    pub seq: Seq,
    /// Wall-clock record time.
    pub at: Timestamp,
    /// Payload schema version.
    pub schema: u32,
    /// Event payload.
    pub payload: T,
}

impl<T> EventEnvelope<T> {
    /// Wrap a payload with the current schema version and wall-clock time.
    pub fn new(seq: Seq, payload: T) -> Self {
        Self {
            seq,
            at: Timestamp::now(),
            schema: CURRENT_SCHEMA,
            payload,
        }
    }

    /// Wrap a payload with an explicit timestamp.
    pub fn at(seq: Seq, at: Timestamp, payload: T) -> Self {
        Self {
            seq,
            at,
            schema: CURRENT_SCHEMA,
            payload,
        }
    }
}

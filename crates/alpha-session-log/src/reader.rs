//! Read-only access to one event generation.

use std::fs;
use std::path::Path;

use alpha_schema::EventEnvelope;

use crate::CURRENT_EVENT_SCHEMA;
use crate::event::SessionEvent;
use crate::header::SessionHeader;
use crate::state::SessionState;
use crate::store::SessionError;

/// Information about a torn tail that was dropped while reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TailDamage {
    /// Bytes dropped from the end of the file.
    pub dropped_bytes: usize,
}

/// A read-only view of one session generation.
#[derive(Debug)]
pub struct SessionReader {
    header: SessionHeader,
    generation: u32,
    events: Vec<EventEnvelope<SessionEvent>>,
    tail: Option<TailDamage>,
}

impl SessionReader {
    pub(crate) fn new(
        header: SessionHeader,
        generation: u32,
        events: Vec<EventEnvelope<SessionEvent>>,
        tail: Option<TailDamage>,
    ) -> Self {
        Self {
            header,
            generation,
            events,
            tail,
        }
    }

    /// The session header.
    pub fn header(&self) -> &SessionHeader {
        &self.header
    }

    /// The generation number that was read.
    pub fn generation(&self) -> u32 {
        self.generation
    }

    /// The durable events in order.
    pub fn events(&self) -> &[EventEnvelope<SessionEvent>] {
        &self.events
    }

    /// Damaged tail bytes, when the last write was torn.
    pub fn tail_damage(&self) -> Option<TailDamage> {
        self.tail
    }

    /// Fold the events into projected state.
    pub fn state(&self) -> SessionState {
        SessionState::from_events(&self.header, &self.events)
    }
}

pub(crate) struct ReadResult {
    pub(crate) events: Vec<EventEnvelope<SessionEvent>>,
    pub(crate) tail: Option<TailDamage>,
}

pub(crate) fn read_generation(path: &Path) -> Result<ReadResult, SessionError> {
    let bytes = fs::read(path)?;
    let text = String::from_utf8_lossy(&bytes);
    let mut events = Vec::new();
    let mut tail = None;
    let mut line_number = 0usize;
    let mut remaining = text.as_ref();

    while let Some(index) = remaining.find('\n') {
        let (line, rest) = remaining.split_at(index);
        remaining = &rest[1..];
        line_number += 1;
        parse_line(line.trim_end_matches('\r'), line_number, &mut events)?;
    }

    if !remaining.is_empty() {
        line_number += 1;
        match serde_json::from_str::<EventEnvelope<SessionEvent>>(remaining) {
            Ok(envelope) => push_envelope(envelope, line_number, &mut events)?,
            Err(_) => {
                tail = Some(TailDamage {
                    dropped_bytes: remaining.len(),
                });
            }
        }
    }

    Ok(ReadResult { events, tail })
}

fn parse_line(
    line: &str,
    line_number: usize,
    events: &mut Vec<EventEnvelope<SessionEvent>>,
) -> Result<(), SessionError> {
    if line.is_empty() {
        return Ok(());
    }
    let envelope: EventEnvelope<SessionEvent> =
        serde_json::from_str(line).map_err(|error| SessionError::CorruptEvent {
            line: line_number,
            reason: error.to_string(),
        })?;
    push_envelope(envelope, line_number, events)
}

fn push_envelope(
    mut envelope: EventEnvelope<SessionEvent>,
    line_number: usize,
    events: &mut Vec<EventEnvelope<SessionEvent>>,
) -> Result<(), SessionError> {
    if envelope.schema > CURRENT_EVENT_SCHEMA {
        return Err(SessionError::UnsupportedVersion {
            found: envelope.schema,
            supported: CURRENT_EVENT_SCHEMA,
        });
    }
    if envelope.schema < CURRENT_EVENT_SCHEMA {
        if envelope.schema == 0 {
            return Err(SessionError::MigrationRequired {
                found: envelope.schema,
                supported: CURRENT_EVENT_SCHEMA,
            });
        }
        envelope.schema = CURRENT_EVENT_SCHEMA;
    }
    if let Some(previous) = events.last()
        && envelope.seq <= previous.seq
    {
        return Err(SessionError::CorruptEvent {
            line: line_number,
            reason: format!("sequence {} does not follow {}", envelope.seq, previous.seq),
        });
    }
    events.push(envelope);
    Ok(())
}

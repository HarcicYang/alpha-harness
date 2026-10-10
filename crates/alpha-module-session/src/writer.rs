//! Single-writer append path and crash repair planning.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use alpha_schema::{EventEnvelope, Seq};
use fs2::FileExt;

use crate::event::{SessionEvent, ToolStatus, TurnEndReason};
use crate::header::SessionHeader;
use crate::state::SessionState;
use crate::store::SessionError;

/// Exclusive writer lease backed by an OS file lock.
pub(crate) struct Lease {
    _file: File,
}

impl Lease {
    pub(crate) fn acquire(path: &Path) -> Result<Self, SessionError> {
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)?;
        match file.try_lock_exclusive() {
            Ok(()) => {}
            Err(_) => {
                let mut contents = String::new();
                let _ = file.read_to_string(&mut contents);
                let pid = contents.trim().parse().ok();
                return Err(SessionError::WriterBusy { pid });
            }
        }
        file.set_len(0)?;
        write!(file, "{}", std::process::id())?;
        file.sync_all()?;
        Ok(Self { _file: file })
    }
}

/// A session opened for writing.
pub struct SessionWriter {
    header: SessionHeader,
    dir: PathBuf,
    file: File,
    _lease: Lease,
    last_seq: Seq,
    repairs: Vec<EventEnvelope<SessionEvent>>,
}

impl SessionWriter {
    pub(crate) fn new(
        header: SessionHeader,
        dir: PathBuf,
        file: File,
        lease: Lease,
        last_seq: Seq,
        repairs: Vec<EventEnvelope<SessionEvent>>,
    ) -> Self {
        Self {
            header,
            dir,
            file,
            _lease: lease,
            last_seq,
            repairs,
        }
    }

    /// The session header.
    pub fn header(&self) -> &SessionHeader {
        &self.header
    }

    /// The session directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The last allocated sequence number.
    pub fn last_seq(&self) -> Seq {
        self.last_seq
    }

    /// Repair events appended when this writer opened the session.
    pub fn repairs(&self) -> &[EventEnvelope<SessionEvent>] {
        &self.repairs
    }

    /// Append one durable event and return its envelope.
    pub fn append(
        &mut self,
        event: SessionEvent,
    ) -> Result<EventEnvelope<SessionEvent>, SessionError> {
        let envelope = append_event(&mut self.file, self.last_seq.next(), event)?;
        self.last_seq = envelope.seq;
        Ok(envelope)
    }
}

pub(crate) fn open_for_append(path: &Path) -> Result<File, SessionError> {
    Ok(OpenOptions::new().append(true).open(path)?)
}

pub(crate) fn create_generation(path: &Path) -> Result<File, SessionError> {
    Ok(OpenOptions::new().create_new(true).write(true).open(path)?)
}

pub(crate) fn copy_generation(
    target: &mut File,
    events: &[EventEnvelope<SessionEvent>],
) -> Result<(), SessionError> {
    for envelope in events {
        let mut line = serde_json::to_string(envelope)?;
        line.push('\n');
        target.write_all(line.as_bytes())?;
    }
    target.sync_all()?;
    Ok(())
}

pub(crate) fn append_event(
    file: &mut File,
    seq: Seq,
    event: SessionEvent,
) -> Result<EventEnvelope<SessionEvent>, SessionError> {
    let envelope = EventEnvelope::new(seq, event);
    let mut line = serde_json::to_string(&envelope)?;
    line.push('\n');
    file.write_all(line.as_bytes())?;
    Ok(envelope)
}

/// Compute repairs for interrupted work.
pub(crate) fn plan_repairs(
    header: &SessionHeader,
    events: &[EventEnvelope<SessionEvent>],
) -> Vec<SessionEvent> {
    let state = SessionState::from_events(header, events);
    let mut repairs = Vec::new();
    for id in &state.pending_tool_calls {
        repairs.push(SessionEvent::ToolResult {
            id: *id,
            status: ToolStatus::Interrupted,
            content: String::new(),
        });
    }
    if state.turn_open {
        repairs.push(SessionEvent::TurnEnded {
            reason: TurnEndReason::Interrupted,
        });
    }
    repairs
}

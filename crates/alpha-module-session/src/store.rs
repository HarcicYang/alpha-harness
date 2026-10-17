//! Session storage layout and lifecycle.

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use alpha_schema::{Seq, SessionId};
use serde::Serialize;
use thiserror::Error;

use crate::header::{HEADER_SCHEMA, SessionHeader};
use crate::reader::{SessionReader, read_generation};
use crate::state::MessageRecord;
use crate::writer::{self, Lease, SessionWriter};

/// Session lock file name.
pub const LOCK_FILE: &str = "session.lock";
/// Session header file name.
pub const HEADER_FILE: &str = "header.json";

/// Errors from session storage.
#[derive(Debug, Error)]
pub enum SessionError {
    /// Filesystem error.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// Serialization error.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// The session directory does not exist.
    #[error("session `{0}` does not exist")]
    NotFound(SessionId),
    /// The session directory already exists.
    #[error("session `{0}` already exists")]
    AlreadyExists(SessionId),
    /// The session has no event generation.
    #[error("session `{0}` has no event log")]
    MissingEvents(SessionId),
    /// The log was written by a newer implementation.
    #[error("session schema {found} is newer than supported schema {supported}")]
    UnsupportedVersion {
        /// Version found on disk.
        found: u32,
        /// Version supported by this build.
        supported: u32,
    },
    /// The log needs a migration that this build does not provide.
    #[error("session schema {found} requires a migration to {supported}")]
    MigrationRequired {
        /// Version found on disk.
        found: u32,
        /// Version supported by this build.
        supported: u32,
    },
    /// A complete line failed to parse or violated ordering rules.
    #[error("corrupt session event at line {line}: {reason}")]
    CorruptEvent {
        /// One-based line number.
        line: usize,
        /// What was wrong.
        reason: String,
    },
    /// Another process holds the writer lease.
    #[error("session is already open for writing{holder}", holder = holder_suffix(*pid))]
    WriterBusy {
        /// Holder process id when known.
        pid: Option<u32>,
    },
}

fn holder_suffix(pid: Option<u32>) -> String {
    match pid {
        Some(pid) => format!(" (pid {pid})"),
        None => String::new(),
    }
}

/// A store rooted at one directory.
#[derive(Clone, Debug)]
pub struct SessionStore {
    root: PathBuf,
}

/// One session listing enriched with an automatically derived preview.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionListEntry {
    /// Durable session header.
    #[serde(flatten)]
    pub header: SessionHeader,
    /// First user message in the durable log, when one exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_message: Option<String>,
}

impl SessionStore {
    /// Create a store at an explicit root.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The default state directory for sessions.
    pub fn default_root() -> PathBuf {
        if let Some(value) = env::var_os("ALPHA_HARNESS_STATE_DIR") {
            return PathBuf::from(value).join("sessions");
        }
        if let Some(value) = env::var_os("XDG_STATE_HOME") {
            return PathBuf::from(value).join("alpha-harness").join("sessions");
        }
        env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".local")
            .join("state")
            .join("alpha-harness")
            .join("sessions")
    }

    /// The root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The directory for one session.
    pub fn session_dir(&self, id: SessionId) -> PathBuf {
        self.root.join(id.to_string())
    }

    /// Create a session and open it for writing.
    pub fn create(&self, header: SessionHeader) -> Result<SessionWriter, SessionError> {
        let dir = self.session_dir(header.id);
        if dir.exists() {
            return Err(SessionError::AlreadyExists(header.id));
        }
        fs::create_dir_all(&dir)?;
        write_header(&dir, &header)?;
        let events_path = dir.join(generation_name(0));
        fs::File::create(&events_path)?;
        let lease = Lease::acquire(&dir.join(LOCK_FILE))?;
        let file = writer::open_for_append(&events_path)?;
        Ok(SessionWriter::new(
            header,
            dir,
            file,
            lease,
            Seq::new(0),
            Vec::new(),
        ))
    }

    /// Open a session for reading. Performs no writes.
    pub fn open_read(&self, id: SessionId) -> Result<SessionReader, SessionError> {
        let dir = self.session_dir(id);
        let header = read_header(&dir, id)?;
        check_header_version(&header)?;
        let (generation, path) = latest_generation(&dir)?.ok_or(SessionError::MissingEvents(id))?;
        let read = read_generation(&path)?;
        Ok(SessionReader::new(
            header,
            generation,
            read.events,
            read.tail,
        ))
    }

    /// Open a session for writing, repairing interrupted work.
    pub fn open_write(&self, id: SessionId) -> Result<SessionWriter, SessionError> {
        let dir = self.session_dir(id);
        let header = read_header(&dir, id)?;
        check_header_version(&header)?;
        let lease = Lease::acquire(&dir.join(LOCK_FILE))?;
        let (generation, path) = latest_generation(&dir)?.ok_or(SessionError::MissingEvents(id))?;
        let read = read_generation(&path)?;

        let repairs = writer::plan_repairs(&header, &read.events);
        let mut last_seq = read
            .events
            .last()
            .map(|event| event.seq)
            .unwrap_or(Seq::new(0));

        let mut file = if read.tail.is_some() {
            let (_, next_path) = next_free_generation(&dir, generation);
            let mut file = writer::create_generation(&next_path)?;
            writer::copy_generation(&mut file, &read.events)?;
            file
        } else {
            writer::open_for_append(&path)?
        };

        let mut recorded = Vec::new();
        for event in repairs {
            let envelope = writer::append_event(&mut file, last_seq.next(), event)?;
            last_seq = envelope.seq;
            recorded.push(envelope);
        }

        Ok(SessionWriter::new(
            header, dir, file, lease, last_seq, recorded,
        ))
    }

    /// List session headers, newest first.
    pub fn list(&self) -> Result<Vec<SessionHeader>, SessionError> {
        let mut headers = Vec::new();
        if !self.root.exists() {
            return Ok(headers);
        }
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let header_path = entry.path().join(HEADER_FILE);
            if !header_path.exists() {
                continue;
            }
            let text = fs::read_to_string(&header_path)?;
            headers.push(serde_json::from_str(&text)?);
        }
        headers.sort_by(|left: &SessionHeader, right: &SessionHeader| {
            right.created_at.cmp(&left.created_at)
        });
        Ok(headers)
    }

    /// List sessions with the first user message projected from each log.
    pub fn list_entries(&self) -> Result<Vec<SessionListEntry>, SessionError> {
        self.list()?
            .into_iter()
            .map(|header| {
                let first_message = self.open_read(header.id).ok().and_then(|reader| {
                    reader
                        .state()
                        .messages
                        .into_iter()
                        .find_map(|message| match message {
                            MessageRecord::User { text, .. } => Some(text),
                            _ => None,
                        })
                });
                Ok(SessionListEntry {
                    header,
                    first_message,
                })
            })
            .collect()
    }

    /// Delete one stored session after rejecting active writers.
    pub fn delete(&self, id: SessionId) -> Result<(), SessionError> {
        let dir = self.session_dir(id);
        if !dir.exists() {
            return Err(SessionError::NotFound(id));
        }
        let lease = Lease::acquire(&dir.join(LOCK_FILE))?;
        if let Err(error) = fs::remove_dir_all(&dir) {
            drop(lease);
            return fs::remove_dir_all(&dir).map_err(|_| error.into());
        }
        drop(lease);
        Ok(())
    }
}

fn check_header_version(header: &SessionHeader) -> Result<(), SessionError> {
    if header.schema > HEADER_SCHEMA {
        return Err(SessionError::UnsupportedVersion {
            found: header.schema,
            supported: HEADER_SCHEMA,
        });
    }
    if header.schema < HEADER_SCHEMA {
        return Err(SessionError::MigrationRequired {
            found: header.schema,
            supported: HEADER_SCHEMA,
        });
    }
    Ok(())
}

fn write_header(dir: &Path, header: &SessionHeader) -> Result<(), SessionError> {
    let path = dir.join(HEADER_FILE);
    let temp = dir.join(format!("{HEADER_FILE}.tmp"));
    let mut text = serde_json::to_string_pretty(header)?;
    text.push('\n');
    fs::write(&temp, text)?;
    fs::rename(&temp, &path)?;
    Ok(())
}

fn read_header(dir: &Path, id: SessionId) -> Result<SessionHeader, SessionError> {
    if !dir.exists() {
        return Err(SessionError::NotFound(id));
    }
    let text = fs::read_to_string(dir.join(HEADER_FILE))?;
    let header: SessionHeader = serde_json::from_str(&text)?;
    Ok(header)
}

/// File name for one generation.
pub fn generation_name(generation: u32) -> String {
    format!("session.v{generation}.jsonl")
}

fn parse_generation(name: &str) -> Option<u32> {
    let rest = name.strip_prefix("session.v")?;
    let digits = rest.strip_suffix(".jsonl")?;
    digits.parse().ok()
}

fn latest_generation(dir: &Path) -> Result<Option<(u32, PathBuf)>, SessionError> {
    let mut latest: Option<(u32, PathBuf)> = None;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(generation) = parse_generation(name) else {
            continue;
        };
        if latest
            .as_ref()
            .is_none_or(|(current, _)| generation > *current)
        {
            latest = Some((generation, entry.path()));
        }
    }
    Ok(latest)
}

fn next_free_generation(dir: &Path, current: u32) -> (u32, PathBuf) {
    let mut generation = current + 1;
    loop {
        let path = dir.join(generation_name(generation));
        if !path.exists() {
            return (generation, path);
        }
        generation += 1;
    }
}

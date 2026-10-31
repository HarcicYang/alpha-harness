//! Append-only session log, projections, and migrations.
//!
//! Infrastructure crate: the log is part of the fixed core, not a swappable
//! module, so the shell and the modules share the same durable format.
//!
//! The session log is the single source of truth for a session. Events are
//! durable, versioned, and appended by a single writer. Repair and migration
//! publish a new generation beside the untouched source generation.

#![forbid(unsafe_code)]

pub mod event;
pub mod header;
pub mod reader;
pub mod state;
pub mod store;
pub mod writer;

pub use event::{SessionEvent, ToolStatus, TurnEndReason};
pub use header::{HEADER_SCHEMA, SessionHeader, WorkspaceRef};
pub use reader::{SessionReader, TailDamage};
pub use state::{CompactionRecord, MessageRecord, SessionState};
pub use store::{SessionError, SessionListEntry, SessionStore};
pub use writer::{SessionEventObserver, SessionWriter};

/// Event payload schema version written by this build.
pub const CURRENT_EVENT_SCHEMA: u32 = alpha_schema::event::CURRENT_SCHEMA;

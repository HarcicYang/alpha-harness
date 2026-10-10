//! Append-only session log, projections, and migrations.
//!
//! The session log is the single source of truth for a session. Events are
//! durable, versioned, and appended by a single writer. Repair and migration
//! publish a new generation beside the untouched source generation.

#![forbid(unsafe_code)]

/// Stable module key used by the generated composition table.
pub const __ALPHA_MODULE__: &str = "session";

pub mod event;
pub mod header;
pub mod reader;
pub mod state;
pub mod store;
pub mod writer;

pub use event::{SessionEvent, ToolStatus, TurnEndReason};
pub use header::{HEADER_SCHEMA, SessionHeader, WorkspaceRef};
pub use reader::{SessionReader, TailDamage};
pub use state::{MessageRecord, SessionState};
pub use store::{SessionError, SessionStore};
pub use writer::SessionWriter;

/// Event payload schema version written by this build.
pub const CURRENT_EVENT_SCHEMA: u32 = alpha_schema::event::CURRENT_SCHEMA;

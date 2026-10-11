//! Public shapes, durable event payloads, and protocol method types shared by
//! the kernel, modules, and protocol surface.

#![forbid(unsafe_code)]

pub mod content;
pub mod event;
pub mod id;
pub mod permission;
pub mod time;

pub use content::AssistantContentBlock;
pub use event::EventEnvelope;
pub use id::{MessageId, SessionId, ToolCallId, TurnId};
pub use permission::PermissionMode;
pub use time::{Seq, Timestamp};

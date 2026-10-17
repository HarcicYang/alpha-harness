//! Public shapes, durable event payloads, and protocol method types shared by
//! the kernel, modules, and protocol surface.

#![forbid(unsafe_code)]

pub mod content;
pub mod event;
pub mod front;
pub mod id;
pub mod permission;
pub mod provider;
pub mod reasoning;
pub mod session;
pub mod time;

pub use content::AssistantContentBlock;
pub use event::EventEnvelope;
pub use front::{
    LiveAgentEvent, ProviderInstanceResponse, ProviderModelList, SessionAttached, SessionCreated,
    SessionEventNotification, SessionListEntry, SessionResumed, TurnStarted,
};
pub use id::{MessageId, SessionId, ToolCallId, TurnId};
pub use permission::PermissionMode;
pub use provider::{
    AdapterRequirement, AuthMethodDescriptor, CredentialBackendStatus, CredentialStatus,
    ModelDescriptor, ProviderDescriptor, ProviderInstance,
};
pub use reasoning::ReasoningLevel;
pub use session::{SessionEvent, ToolStatus, TurnEndReason, WorkspaceRef};
pub use time::{Seq, Timestamp};

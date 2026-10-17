//! Session header and workspace binding.

use alpha_schema::{PermissionMode, SessionId, Timestamp};
use serde::{Deserialize, Serialize};

use crate::event::SessionEvent;

pub use alpha_schema::WorkspaceRef;

/// Header schema version of the current implementation.
pub const HEADER_SCHEMA: u32 = 1;

/// Session identity and binding, stored beside the event log.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionHeader {
    /// Header schema version.
    pub schema: u32,
    /// Session identity.
    pub id: SessionId,
    /// Creation time.
    pub created_at: Timestamp,
    /// Permission mode at creation.
    pub mode: PermissionMode,
    /// Workspace and cwd binding.
    pub workspace: WorkspaceRef,
    /// Provider instance or provider kind selected at creation.
    #[serde(default)]
    pub provider: String,
    /// Model selected at creation.
    #[serde(default)]
    pub model: String,
}

impl SessionHeader {
    /// Create a header with the current schema version.
    pub fn new(id: SessionId, mode: PermissionMode, workspace: WorkspaceRef) -> Self {
        Self {
            schema: HEADER_SCHEMA,
            id,
            created_at: Timestamp::now(),
            mode,
            workspace,
            provider: String::new(),
            model: String::new(),
        }
    }
}

/// Apply a location event to a workspace binding.
pub fn apply_location_event(binding: &mut WorkspaceRef, event: &SessionEvent) -> bool {
    match event {
        SessionEvent::LocationBound { workspace } | SessionEvent::LocationRebound { workspace } => {
            *binding = workspace.clone();
            true
        }
        _ => false,
    }
}

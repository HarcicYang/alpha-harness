//! Permission modes.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// User-facing permission mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionMode {
    /// Read-only sandbox; mutations are denied without prompting.
    Read,
    /// Default mode: workspace-write sandbox with on-request approvals.
    #[default]
    Auto,
    /// Sandboxed mode with two-stage model review before user prompts.
    Reviewed,
    /// Full access with no sandbox and no prompts.
    Free,
}

impl fmt::Display for PermissionMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Read => "read",
            Self::Auto => "auto",
            Self::Reviewed => "reviewed",
            Self::Free => "free",
        };
        f.write_str(name)
    }
}

impl FromStr for PermissionMode {
    type Err = ParsePermissionModeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "read" => Ok(Self::Read),
            "auto" => Ok(Self::Auto),
            "reviewed" => Ok(Self::Reviewed),
            "free" => Ok(Self::Free),
            other => Err(ParsePermissionModeError(other.to_owned())),
        }
    }
}

/// Error returned when parsing an unknown permission mode.
#[derive(Debug, thiserror::Error)]
#[error("unknown permission mode `{0}`")]
pub struct ParsePermissionModeError(String);

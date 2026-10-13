//! Reasoning effort levels shared by fronts, sessions, and providers.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Requested reasoning or thinking effort.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningLevel {
    /// Use the provider or model default.
    #[default]
    Auto,
    /// Disable reasoning when the provider supports it.
    Off,
    /// Lowest explicit reasoning budget.
    Minimal,
    /// Low reasoning effort.
    Low,
    /// Medium reasoning effort.
    Medium,
    /// High reasoning effort.
    High,
    /// Extra-high reasoning effort where supported.
    #[serde(rename = "xhigh")]
    XHigh,
    /// Maximum reasoning effort where supported.
    Max,
}

impl ReasoningLevel {
    /// Ordered levels used by selectors and cycling controls.
    pub const ALL: [Self; 8] = [
        Self::Auto,
        Self::Off,
        Self::Minimal,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::XHigh,
        Self::Max,
    ];

    /// Stable display and command name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        }
    }

    /// Provider-facing effort value. `None` omits explicit reasoning controls.
    pub const fn api_effort(self) -> Option<&'static str> {
        match self {
            Self::Auto => None,
            Self::Off => Some("none"),
            Self::Minimal => Some("minimal"),
            Self::Low => Some("low"),
            Self::Medium => Some("medium"),
            Self::High => Some("high"),
            Self::XHigh => Some("xhigh"),
            Self::Max => Some("max"),
        }
    }

    /// Token budget for providers that expose a thinking budget.
    pub const fn budget_tokens(self) -> Option<u64> {
        match self {
            Self::Auto | Self::Off => None,
            Self::Minimal => Some(1_024),
            Self::Low => Some(1_536),
            Self::Medium => Some(2_048),
            Self::High => Some(3_072),
            Self::XHigh | Self::Max => Some(3_072),
        }
    }
}

impl fmt::Display for ReasoningLevel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

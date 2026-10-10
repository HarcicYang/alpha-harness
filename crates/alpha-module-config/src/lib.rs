//! Layered TOML configuration with provenance.
//!
//! Layer precedence, lowest to highest:
//! module defaults, user config, project config, environment, CLI.

#![forbid(unsafe_code)]

/// Stable module key used by the generated composition table.
pub const __ALPHA_MODULE__: &str = "config";

pub mod source;
pub mod stack;

pub use source::ConfigSource;
pub use stack::{ConfigError, ConfigStack, ResolvedConfig};

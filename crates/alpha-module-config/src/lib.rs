//! Layered TOML configuration with provenance.
//!
//! Layer precedence, lowest to highest:
//! module defaults, user config, project config, environment, CLI.

#![forbid(unsafe_code)]

pub mod source;
pub mod stack;

pub use source::ConfigSource;
pub use stack::{ConfigError, ConfigStack, ResolvedConfig};

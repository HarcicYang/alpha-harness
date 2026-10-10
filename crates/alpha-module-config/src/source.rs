//! Configuration layers.

/// One source of configuration values.
///
/// The derive order is the precedence order: later variants override earlier
/// ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ConfigSource {
    /// Module-provided defaults.
    Defaults,
    /// User-level configuration file.
    User,
    /// Project-level configuration file.
    Project,
    /// Environment variables.
    Environment,
    /// Command-line flags.
    Cli,
}

impl ConfigSource {
    /// Human-readable label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Defaults => "defaults",
            Self::User => "user",
            Self::Project => "project",
            Self::Environment => "environment",
            Self::Cli => "cli",
        }
    }
}

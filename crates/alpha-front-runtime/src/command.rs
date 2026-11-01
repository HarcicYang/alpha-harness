use alpha_schema::{PermissionMode, ReasoningLevel, SessionId};

/// Stable slash-command metadata shared by frontend completions and help.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandSpec {
    /// Command name without the leading slash.
    pub name: &'static str,
    /// User-facing usage string.
    pub usage: &'static str,
    /// Short description.
    pub description: &'static str,
    /// Whether the command accepts an argument.
    pub takes_argument: bool,
}

/// Slash commands understood by every interactive front.
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        name: "provider",
        usage: "/provider [ID]",
        description: "Open the provider manager or switch provider",
        takes_argument: true,
    },
    CommandSpec {
        name: "model",
        usage: "/model [MODEL] [EFFORT]",
        description: "Open the model picker or switch the route",
        takes_argument: true,
    },
    CommandSpec {
        name: "thinking",
        usage: "/thinking LEVEL",
        description: "Open the effort picker or set effort",
        takes_argument: true,
    },
    CommandSpec {
        name: "effort",
        usage: "/effort LEVEL",
        description: "Set the reasoning effort",
        takes_argument: true,
    },
    CommandSpec {
        name: "mode",
        usage: "/mode MODE",
        description: "Set read, auto, reviewed, or free",
        takes_argument: true,
    },
    CommandSpec {
        name: "confirm",
        usage: "/confirm",
        description: "Run pending steers",
        takes_argument: false,
    },
    CommandSpec {
        name: "discard",
        usage: "/discard",
        description: "Discard pending steers",
        takes_argument: false,
    },
    CommandSpec {
        name: "capabilities",
        usage: "/capabilities TOOLS",
        description: "Set provider-native tools",
        takes_argument: true,
    },
    CommandSpec {
        name: "image",
        usage: "/image PATH",
        description: "Attach an image to the next prompt",
        takes_argument: true,
    },
    CommandSpec {
        name: "new",
        usage: "/new",
        description: "Start a new session draft",
        takes_argument: false,
    },
    CommandSpec {
        name: "resume",
        usage: "/resume ID",
        description: "Resume a session",
        takes_argument: true,
    },
    CommandSpec {
        name: "sessions",
        usage: "/sessions",
        description: "Open the session manager",
        takes_argument: false,
    },
    CommandSpec {
        name: "diagnostics",
        usage: "/diagnostics",
        description: "Show core connection details",
        takes_argument: false,
    },
    CommandSpec {
        name: "help",
        usage: "/help",
        description: "Show help",
        takes_argument: false,
    },
    CommandSpec {
        name: "quit",
        usage: "/quit",
        description: "Exit the active front",
        takes_argument: false,
    },
];

/// Parsed command independent of terminal input handling.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrontCommand {
    /// Open provider management.
    OpenProviders,
    /// Select one provider instance.
    SelectProvider(String),
    /// Open the model picker.
    OpenModels,
    /// Select a model, optionally with an explicit reasoning level.
    SelectModel {
        /// Model id or provider/model target.
        target: String,
        /// Explicit reasoning level.
        reasoning: Option<ReasoningLevel>,
    },
    /// Open the reasoning picker.
    OpenThinking,
    /// Set reasoning for the current route.
    SetThinking(ReasoningLevel),
    /// Set permission mode.
    SetMode(PermissionMode),
    /// Confirm pending steers.
    ConfirmSteers,
    /// Discard pending steers.
    DiscardSteers,
    /// Set provider-native tools.
    SetCapabilities(Vec<String>),
    /// Attach an image to the next prompt.
    AttachImage(String),
    /// Start a new draft.
    NewSession,
    /// Resume one session.
    Resume(SessionId),
    /// Open the session picker.
    OpenSessions,
    /// Show core diagnostics.
    Diagnostics,
    /// Show help.
    Help,
    /// Quit the front.
    Quit,
    /// Empty input.
    Empty,
    /// Unknown command.
    Unknown(String),
    /// Invalid command usage.
    Invalid(String),
}

/// Parse one slash command.
pub fn parse_command(command: &str) -> FrontCommand {
    let (name, argument) = split_command(command);
    match name {
        "provider" if argument.is_empty() => FrontCommand::OpenProviders,
        "provider" => FrontCommand::SelectProvider(argument.to_owned()),
        "model" if argument.is_empty() => FrontCommand::OpenModels,
        "model" => {
            let mut parts = argument.split_whitespace();
            let Some(target) = parts.next() else {
                return FrontCommand::OpenModels;
            };
            let reasoning = match parts.next().map(parse_reasoning_level).transpose() {
                Ok(reasoning) => reasoning,
                Err(error) => return FrontCommand::Invalid(error),
            };
            if parts.next().is_some() {
                return FrontCommand::Invalid("usage: /model <model> [effort]".to_owned());
            }
            FrontCommand::SelectModel {
                target: target.to_owned(),
                reasoning,
            }
        }
        "thinking" | "effort" if argument.is_empty() => FrontCommand::OpenThinking,
        "thinking" | "effort" => match parse_reasoning_level(argument) {
            Ok(level) => FrontCommand::SetThinking(level),
            Err(error) => FrontCommand::Invalid(error),
        },
        "mode" if argument.is_empty() => {
            FrontCommand::Invalid("usage: /mode <read|auto|reviewed|free>".to_owned())
        }
        "mode" => match argument.parse::<PermissionMode>() {
            Ok(mode) => FrontCommand::SetMode(mode),
            Err(error) => FrontCommand::Invalid(error.to_string()),
        },
        "confirm" => FrontCommand::ConfirmSteers,
        "discard" => FrontCommand::DiscardSteers,
        "capabilities" => FrontCommand::SetCapabilities(
            argument
                .split([',', ' '])
                .filter(|tool| !tool.is_empty())
                .map(str::to_owned)
                .collect(),
        ),
        "image" if !argument.is_empty() => FrontCommand::AttachImage(argument.to_owned()),
        "image" => FrontCommand::Invalid("usage: /image <PATH>".to_owned()),
        "new" => FrontCommand::NewSession,
        "resume" if !argument.is_empty() => match argument.parse::<SessionId>() {
            Ok(session_id) => FrontCommand::Resume(session_id),
            Err(error) => FrontCommand::Invalid(error.to_string()),
        },
        "resume" => FrontCommand::Invalid("usage: /resume <SESSION_ID>".to_owned()),
        "sessions" => FrontCommand::OpenSessions,
        "diagnostics" => FrontCommand::Diagnostics,
        "help" => FrontCommand::Help,
        "quit" => FrontCommand::Quit,
        "" => FrontCommand::Empty,
        other => FrontCommand::Unknown(other.to_owned()),
    }
}

fn parse_reasoning_level(value: &str) -> Result<ReasoningLevel, String> {
    serde_json::from_value(serde_json::Value::String(value.to_owned()))
        .map_err(|error| error.to_string())
}

fn split_command(command: &str) -> (&str, &str) {
    let command = command
        .trim()
        .strip_prefix('/')
        .unwrap_or_else(|| command.trim());
    command
        .split_once(char::is_whitespace)
        .map_or((command, ""), |(name, argument)| (name, argument.trim()))
}

#[cfg(test)]
mod tests {
    use alpha_schema::{PermissionMode, ReasoningLevel};

    use super::{FrontCommand, parse_command};

    #[test]
    fn parses_commands_without_frontend_specific_state() {
        assert_eq!(
            parse_command("/mode reviewed"),
            FrontCommand::SetMode(PermissionMode::Reviewed)
        );
        assert_eq!(
            parse_command("/model gpt-4o high"),
            FrontCommand::SelectModel {
                target: "gpt-4o".to_owned(),
                reasoning: Some(ReasoningLevel::High),
            }
        );
        assert_eq!(
            parse_command("/capabilities web_search,code"),
            FrontCommand::SetCapabilities(vec!["web_search".to_owned(), "code".to_owned()])
        );
        assert_eq!(
            parse_command("/image /tmp/example.png"),
            FrontCommand::AttachImage("/tmp/example.png".to_owned())
        );
        assert_eq!(
            parse_command("/model anthropic/claude-3.5-sonnet high"),
            FrontCommand::SelectModel {
                target: "anthropic/claude-3.5-sonnet".to_owned(),
                reasoning: Some(ReasoningLevel::High),
            }
        );
        assert_eq!(
            parse_command("/model 模型/测试-v1"),
            FrontCommand::SelectModel {
                target: "模型/测试-v1".to_owned(),
                reasoning: None,
            }
        );
    }
}

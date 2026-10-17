use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use alpha_front_runtime::{
    FrontEvent, FrontRuntime, FrontRuntimeConfig, SessionOptions, read_terminal_approval,
};
use alpha_front_tui::run_tui;
use alpha_schema::LiveAgentEvent;
use alpha_schema::{PermissionMode, SessionId};
use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "alpha",
    version,
    about = "Lightweight, everything-is-a-module coding agent harness"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run one prompt and exit.
    Run(RunArgs),
    /// Open the interactive chat front.
    Chat,
    /// Open the full-screen terminal interface.
    Tui,
    /// List stored sessions.
    Sessions,
    /// Internal core process entry point.
    #[command(name = "__core", hide = true)]
    Core(CoreArgs),
}

#[derive(Debug, Args)]
struct CoreArgs {
    /// Serve alpha-protocol over stdin/stdout.
    #[arg(long)]
    stdio: bool,
}

#[derive(Debug, Args)]
struct RunArgs {
    /// Permission mode: read, auto, reviewed, or free.
    #[arg(long, default_value = "auto", value_parser = parse_mode)]
    mode: PermissionMode,
    /// Workspace root. Defaults to the current directory.
    #[arg(long)]
    workspace: Option<PathBuf>,
    /// Provider instance or provider kind. Defaults to the configured default.
    #[arg(long)]
    provider: Option<String>,
    /// Model identifier. Defaults to the provider catalog.
    #[arg(long)]
    model: Option<String>,
    /// Resume an existing session instead of starting a new one.
    #[arg(long, value_name = "SESSION_ID")]
    resume: Option<SessionId>,
    /// Prompt text.
    #[arg(num_args = 0..)]
    prompt: Vec<String>,
}

fn parse_mode(value: &str) -> Result<PermissionMode, String> {
    value.parse().map_err(|error| format!("{error}"))
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Run(args) => run_command(args),
        Command::Chat | Command::Tui => {
            let runtime = match FrontRuntime::start(FrontRuntimeConfig::default()) {
                Ok(runtime) => runtime,
                Err(error) => {
                    eprintln!("alpha: {error}");
                    return ExitCode::FAILURE;
                }
            };
            match run_tui(runtime) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("alpha: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        Command::Sessions => list_sessions_command(),
        Command::Core(args) => {
            if !args.stdio {
                eprintln!("alpha __core: --stdio is required");
                return ExitCode::FAILURE;
            }
            let store = alpha_module_session::SessionStore::new(
                alpha_module_session::SessionStore::default_root(),
            );
            match alpha_harness::serve_stdio(store) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("alpha __core: {error}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}

fn list_sessions_command() -> ExitCode {
    let mut runtime = match FrontRuntime::start(FrontRuntimeConfig {
        refresh_providers: false,
        ..FrontRuntimeConfig::default()
    }) {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("alpha sessions: {error}");
            return ExitCode::FAILURE;
        }
    };
    let sessions = match runtime.list_sessions() {
        Ok(sessions) => sessions,
        Err(error) => {
            eprintln!("alpha sessions: {error}");
            return ExitCode::FAILURE;
        }
    };
    for session in sessions {
        println!(
            "{}\t{}\t{}\t{}",
            session.id,
            session.mode,
            session.created_at.as_millis(),
            session.workspace.root
        );
    }
    ExitCode::SUCCESS
}

fn run_command(args: RunArgs) -> ExitCode {
    let prompt = args.prompt.join(" ");
    if prompt.trim().is_empty() && args.resume.is_none() {
        eprintln!("alpha run: provide a prompt or use --resume <SESSION_ID>");
        return ExitCode::FAILURE;
    }
    let workspace = match resolve_workspace(args.workspace) {
        Ok(workspace) => workspace,
        Err(error) => {
            eprintln!("alpha run: {error}");
            return ExitCode::FAILURE;
        }
    };
    let mut session = SessionOptions::new(workspace);
    session.mode = args.mode;
    session.provider = args.provider.unwrap_or_default();
    session.model = args.model.unwrap_or_default();
    let mut runtime = match FrontRuntime::start(FrontRuntimeConfig {
        session,
        ..FrontRuntimeConfig::default()
    }) {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("alpha run: {error}");
            return ExitCode::FAILURE;
        }
    };

    let session_id = if let Some(session_id) = args.resume {
        match runtime.resume_session(session_id) {
            Ok(resumed) => {
                eprintln!(
                    "alpha run: resumed {} ({} messages, repairs={})",
                    session_id, resumed.messages, resumed.repairs
                );
                session_id
            }
            Err(error) => {
                eprintln!("alpha run: {error}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        match runtime.create_session() {
            Ok(created) => created.session_id,
            Err(error) => {
                eprintln!("alpha run: {error}. Configure a provider with `alpha tui` first.");
                return ExitCode::FAILURE;
            }
        }
    };

    if prompt.trim().is_empty() {
        return ExitCode::SUCCESS;
    }
    if let Err(error) = runtime.submit_prompt(&prompt) {
        eprintln!("alpha run: {error}");
        return ExitCode::FAILURE;
    }

    let mut streamed = false;
    loop {
        let events = match runtime.receive_events(Duration::from_millis(100)) {
            Ok(events) => events,
            Err(error) => {
                eprintln!("alpha run: {error}");
                return ExitCode::FAILURE;
            }
        };
        for event in events {
            match event {
                FrontEvent::Live { event } => match event {
                    LiveAgentEvent::TextDelta { text } => {
                        print!("{text}");
                        let _ = std::io::stdout().flush();
                        streamed = true;
                    }
                    LiveAgentEvent::StepStarted { step } => {
                        eprintln!("alpha run: step {step}: calling model...");
                    }
                    LiveAgentEvent::ToolStarted { name, .. } => {
                        eprintln!("alpha run: -> {name}");
                    }
                    LiveAgentEvent::ToolFinished { name, status, .. } => {
                        eprintln!("alpha run: <- {name} {}", tool_status_label(status));
                    }
                    LiveAgentEvent::Error { message } => {
                        eprintln!("alpha run: {message}");
                    }
                    LiveAgentEvent::ThinkingDelta { .. } => {}
                },
                FrontEvent::InteractionRequested { .. } => {
                    let prompt = runtime
                        .chat()
                        .interaction
                        .as_ref()
                        .map(|prompt| prompt.prompt.clone())
                        .unwrap_or_default();
                    let decision = read_terminal_approval(&prompt);
                    if let Err(error) = runtime.respond_interaction(decision) {
                        eprintln!("alpha run: {error}");
                        return ExitCode::FAILURE;
                    }
                }
                FrontEvent::TurnEnded { .. } => {
                    if streamed {
                        println!();
                    } else {
                        let final_text = final_assistant_text(runtime.chat());
                        if !final_text.is_empty() {
                            println!("{final_text}");
                        }
                    }
                    eprintln!("session: {session_id}");
                    return ExitCode::SUCCESS;
                }
                FrontEvent::CoreRestarted { resumed } => {
                    eprintln!(
                        "alpha run: core restarted; session {}",
                        if resumed { "resumed" } else { "not resumed" }
                    );
                    return ExitCode::FAILURE;
                }
                FrontEvent::CoreDisconnected { error } => {
                    eprintln!("alpha run: core disconnected: {error}");
                    return ExitCode::FAILURE;
                }
            }
        }
    }
}

fn resolve_workspace(workspace: Option<PathBuf>) -> Result<PathBuf, String> {
    let workspace = match workspace {
        Some(path) => path,
        None => std::env::current_dir().map_err(|error| error.to_string())?,
    };
    std::fs::canonicalize(&workspace).map_err(|error| {
        format!(
            "workspace `{}` is not accessible: {error}",
            workspace.display()
        )
    })
}

fn final_assistant_text(chat: &alpha_front_runtime::ChatState) -> String {
    chat.transcript
        .iter()
        .rev()
        .find_map(|entry| match entry {
            alpha_front_runtime::TranscriptItem::AssistantMessage { blocks } => Some(
                blocks
                    .iter()
                    .filter_map(alpha_schema::AssistantContentBlock::text)
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            ),
            _ => None,
        })
        .unwrap_or_default()
}

fn tool_status_label(status: alpha_schema::ToolStatus) -> &'static str {
    match status {
        alpha_schema::ToolStatus::Completed => "completed",
        alpha_schema::ToolStatus::Failed => "failed",
        alpha_schema::ToolStatus::Interrupted => "interrupted",
    }
}

use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use alpha_front_client::ConnectionEvent;
use alpha_front_supervisor::{CoreSupervisor, SupervisorConfig};
use alpha_front_tui::run_tui;
use alpha_schema::{PermissionMode, SessionId};
use clap::{Args, Parser, Subcommand};
use serde_json::{Value, json};

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
    /// Permission mode: read, auto, or free.
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
        Command::Chat | Command::Tui => match run_tui() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("alpha: {error}");
                ExitCode::FAILURE
            }
        },
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
    let mut supervisor = match CoreSupervisor::start(SupervisorConfig::default()) {
        Ok(supervisor) => supervisor,
        Err(error) => {
            eprintln!("alpha sessions: {error}");
            return ExitCode::FAILURE;
        }
    };
    let sessions = match supervisor.request("session.list", None) {
        Ok(sessions) => sessions,
        Err(error) => {
            eprintln!("alpha sessions: {error}");
            return ExitCode::FAILURE;
        }
    };
    let Some(sessions) = sessions.as_array() else {
        eprintln!("alpha sessions: invalid session list");
        return ExitCode::FAILURE;
    };
    for header in sessions {
        let id = header.get("id").and_then(Value::as_str).unwrap_or("?");
        let mode = header.get("mode").and_then(Value::as_str).unwrap_or("?");
        let created = header
            .get("created_at")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        let workspace = header
            .get("workspace")
            .and_then(|workspace| workspace.get("root"))
            .and_then(Value::as_str)
            .unwrap_or("?");
        println!("{id}\t{mode}\t{created}\t{workspace}");
    }
    ExitCode::SUCCESS
}

fn run_command(args: RunArgs) -> ExitCode {
    let prompt = args.prompt.join(" ");
    if prompt.trim().is_empty() && args.resume.is_none() {
        eprintln!("alpha run: provide a prompt or use --resume <SESSION_ID>");
        return ExitCode::FAILURE;
    }
    let mut supervisor = match CoreSupervisor::start(SupervisorConfig::default()) {
        Ok(supervisor) => supervisor,
        Err(error) => {
            eprintln!("alpha run: {error}");
            return ExitCode::FAILURE;
        }
    };

    let session_id = if let Some(session_id) = args.resume {
        match supervisor.request(
            "session.resume",
            Some(json!({ "sessionId": session_id })),
        ) {
            Ok(resumed) => {
                eprintln!(
                    "alpha run: resumed {} ({} messages, repairs={})",
                    session_id,
                    resumed
                        .get("messages")
                        .and_then(Value::as_u64)
                        .unwrap_or_default(),
                    resumed
                        .get("repairs")
                        .and_then(Value::as_u64)
                        .unwrap_or_default()
                );
                session_id
            }
            Err(error) => {
                eprintln!("alpha run: {error}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        let workspace = match resolve_workspace(args.workspace) {
            Ok(workspace) => workspace,
            Err(error) => {
                eprintln!("alpha run: {error}");
                return ExitCode::FAILURE;
            }
        };
        match supervisor.request(
            "session.create",
            Some(json!({
                "workspace": workspace,
                "cwd": workspace,
                "mode": args.mode,
                "provider": args.provider.unwrap_or_default(),
                "model": args.model.unwrap_or_default(),
            })),
        ) {
            Ok(created) => {
                let Some(session_id) = created
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .and_then(|id| id.parse::<SessionId>().ok())
                else {
                    eprintln!("alpha run: session.create returned no session id");
                    return ExitCode::FAILURE;
                };
                session_id
            }
            Err(error) => {
                eprintln!(
                    "alpha run: {error}. Configure a provider with `alpha tui` first."
                );
                return ExitCode::FAILURE;
            }
        }
    };

    if prompt.trim().is_empty() {
        return ExitCode::SUCCESS;
    }
    if let Err(error) = supervisor.request(
        "session.submit",
        Some(json!({ "sessionId": session_id, "prompt": prompt })),
    ) {
        eprintln!("alpha run: {error}");
        return ExitCode::FAILURE;
    }

    let mut streamed = false;
    let mut final_text = String::new();
    loop {
        let event = match supervisor.recv_event_timeout(Duration::from_millis(100)) {
            Ok(event) => event,
            Err(error) => {
                eprintln!("alpha run: {error}");
                return ExitCode::FAILURE;
            }
        };
        match event {
            Some(ConnectionEvent::Notification(notification))
                if notification.method == "session.event" =>
            {
                let Some(params) = notification.params else {
                    continue;
                };
                if let Some(text) = handle_run_event(&params, &mut streamed) {
                    final_text = text;
                }
                if turn_finished(&params) {
                    if streamed {
                        println!();
                    } else if !final_text.is_empty() {
                        println!("{final_text}");
                    }
                    eprintln!("session: {session_id}");
                    return ExitCode::SUCCESS;
                }
            }
            Some(ConnectionEvent::Request(request))
                if request.method == "interaction.request" =>
            {
                let decision = read_approval_decision(&request.params);
                if let Err(error) =
                    supervisor.respond(request.id, Ok(json!({ "decision": decision })))
                {
                    eprintln!("alpha run: {error}");
                    return ExitCode::FAILURE;
                }
            }
            Some(ConnectionEvent::Closed(error)) => {
                eprintln!("alpha run: core disconnected: {error}");
                return ExitCode::FAILURE;
            }
            _ => {}
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

fn handle_run_event(params: &Value, streamed: &mut bool) -> Option<String> {
    let kind = params.get("kind").and_then(Value::as_str).unwrap_or("");
    let event = params.get("event").unwrap_or(&Value::Null);
    match kind {
        "live" => {
            match event.get("type").and_then(Value::as_str).unwrap_or("") {
                "text_delta" => {
                    if let Some(text) = event.get("text").and_then(Value::as_str) {
                        print!("{text}");
                        let _ = std::io::stdout().flush();
                        *streamed = true;
                    }
                }
                "step_started" => {
                    if let Some(step) = event.get("step").and_then(Value::as_u64) {
                        eprintln!("alpha run: step {step}: calling model...");
                    }
                }
                "tool_started" => {
                    let name = event.get("name").and_then(Value::as_str).unwrap_or("tool");
                    eprintln!("alpha run: -> {name}");
                }
                "tool_finished" => {
                    let name = event.get("name").and_then(Value::as_str).unwrap_or("tool");
                    let status = event
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("finished");
                    eprintln!("alpha run: <- {name} {status}");
                }
                _ => {}
            }
        }
        "durable" => {
            let payload = event.get("payload").unwrap_or(&Value::Null);
            match payload.get("type").and_then(Value::as_str).unwrap_or("") {
                "assistant_message" => {
                    return payload
                        .get("text")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                }
                "tool_call" => {
                    let name = payload.get("name").and_then(Value::as_str).unwrap_or("tool");
                    eprintln!("alpha run: -> {name}");
                }
                "tool_result" => {
                    let status = payload
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("finished");
                    eprintln!("alpha run: <- {status}");
                }
                _ => {}
            }
        }
        _ => {}
    }
    None
}

fn turn_finished(params: &Value) -> bool {
    params.get("kind").and_then(Value::as_str) == Some("durable")
        && params
            .get("event")
            .and_then(|event| event.get("payload"))
            .and_then(|payload| payload.get("type"))
            .and_then(Value::as_str)
            == Some("turn_ended")
}

fn read_approval_decision(params: &Option<Value>) -> &'static str {
    if !std::io::stdin().is_terminal() {
        return "deny";
    }
    if let Some(params) = params {
        eprintln!(
            "alpha run: permission requested: {}",
            serde_json::to_string(params).unwrap_or_default()
        );
    }
    eprintln!("[a]llow once  [s]ession  [d]eny");
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return "deny";
    }
    match line.trim().to_ascii_lowercase().as_str() {
        "a" | "y" | "yes" => "once",
        "s" | "session" => "session",
        _ => "deny",
    }
}

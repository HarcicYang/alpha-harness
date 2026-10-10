use std::path::PathBuf;
use std::process::ExitCode;

use alpha_harness::{RunOptions, compose_openai_runtime, run_prompt, serve_stdio};
use alpha_module_session::SessionStore;
use alpha_schema::PermissionMode;
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
    /// Serve JSON-RPC 2.0 over NDJSON on stdio.
    Serve,
}

#[derive(Debug, Args)]
struct RunArgs {
    /// Permission mode: read, auto, or free.
    #[arg(long, default_value = "auto", value_parser = parse_mode)]
    mode: PermissionMode,
    /// Workspace root. Defaults to the current directory.
    #[arg(long)]
    workspace: Option<PathBuf>,
    /// Model identifier.
    #[arg(long, default_value = "gpt-4o-mini")]
    model: String,
    /// OpenAI-compatible base URL.
    #[arg(long)]
    base_url: Option<String>,
    /// Prompt text.
    #[arg(required = true, num_args = 1..)]
    prompt: Vec<String>,
}

fn parse_mode(value: &str) -> Result<PermissionMode, String> {
    value.parse().map_err(|error| format!("{error}"))
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Run(args) => run_command(args),
        Command::Serve => {
            let store = SessionStore::new(SessionStore::default_root());
            match serve_stdio(store) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("alpha serve: {error}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}

fn run_command(args: RunArgs) -> ExitCode {
    let workspace = match args.workspace {
        Some(path) => path,
        None => match std::env::current_dir() {
            Ok(path) => path,
            Err(error) => {
                eprintln!("alpha run: cannot resolve the workspace: {error}");
                return ExitCode::FAILURE;
            }
        },
    };
    let workspace = match std::fs::canonicalize(&workspace) {
        Ok(path) => path,
        Err(error) => {
            eprintln!(
                "alpha run: workspace `{}` is not accessible: {error}",
                workspace.display()
            );
            return ExitCode::FAILURE;
        }
    };
    let base_url = args
        .base_url
        .or_else(|| std::env::var("OPENAI_BASE_URL").ok())
        .unwrap_or_else(|| "https://api.openai.com/v1".to_owned());
    let api_key = match std::env::var("OPENAI_API_KEY") {
        Ok(key) => key,
        Err(_) => {
            eprintln!("alpha run: set OPENAI_API_KEY to use the openai-compatible provider");
            return ExitCode::FAILURE;
        }
    };
    let prompt = args.prompt.join(" ");

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("alpha run: cannot start the async runtime: {error}");
            return ExitCode::FAILURE;
        }
    };

    let result = runtime.block_on(async {
        let (_ctx, agent) =
            compose_openai_runtime(&workspace, args.mode, &args.model, &base_url, &api_key)?;
        let store = SessionStore::new(SessionStore::default_root());
        run_prompt(
            RunOptions {
                workspace,
                mode: args.mode,
                model: args.model,
                prompt,
            },
            &agent,
            &store,
        )
        .await
    });

    match result {
        Ok(report) => {
            println!("{}", report.assistant_text);
            eprintln!("session: {}", report.session_id);
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("alpha run: {error}");
            ExitCode::FAILURE
        }
    }
}

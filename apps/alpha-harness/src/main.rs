use std::process::ExitCode;

use alpha_harness::serve_stdio;
use alpha_module_session::SessionStore;
use clap::{Parser, Subcommand};

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
    /// Run one prompt. The agent loop is not implemented yet.
    Run {
        /// Prompt text.
        prompt: Vec<String>,
    },
    /// Serve JSON-RPC 2.0 over NDJSON on stdio.
    Serve,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Run { prompt } => {
            let _ = prompt;
            eprintln!("alpha run: the agent loop is not implemented yet");
            ExitCode::SUCCESS
        }
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

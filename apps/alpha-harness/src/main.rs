use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use alpha_harness::{
    ResumeOptions, RunOptions, TtyApprovalChannel, compose_openai_runtime, list_sessions,
    resume_session_in, run_prompt_in, serve_stdio,
};
use alpha_kernel::Ctx;
use alpha_module_loop::{
    AgentError, AgentStepStarted, AgentTextDelta, AgentToolFinished, AgentToolStarted,
};
use alpha_module_permission::ApprovalChannel;
use alpha_module_session::SessionStore;
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
    /// List stored sessions.
    Sessions,
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
        Command::Sessions => {
            let store = SessionStore::new(SessionStore::default_root());
            match list_sessions(&store) {
                Ok(sessions) => {
                    for summary in sessions {
                        println!(
                            "{}\t{}\t{}\t{}",
                            summary.id, summary.mode, summary.created_at_ms, summary.workspace
                        );
                    }
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("alpha sessions: {error}");
                    ExitCode::FAILURE
                }
            }
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

fn run_command(args: RunArgs) -> ExitCode {
    let base_url = args
        .base_url
        .clone()
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

    let approval: Option<Arc<dyn ApprovalChannel>> = if std::io::stdin().is_terminal() {
        Some(Arc::new(TtyApprovalChannel))
    } else {
        None
    };

    if let Some(session_id) = args.resume {
        let store = SessionStore::new(SessionStore::default_root());
        let header = match store.open_read(session_id) {
            Ok(reader) => reader.header().clone(),
            Err(error) => {
                eprintln!("alpha run: {error}");
                return ExitCode::FAILURE;
            }
        };
        if args.workspace.is_some() {
            eprintln!("alpha run: ignoring --workspace; the session binding wins");
        }
        let workspace = PathBuf::from(&header.workspace.root);
        let mode = header.mode;
        let prompt = (!prompt.trim().is_empty()).then_some(prompt);
        eprintln!(
            "alpha run: resume {} (mode={mode}, model={}, workspace={})",
            session_id,
            args.model,
            workspace.display()
        );
        let result = runtime.block_on(async move {
            let composition = compose_openai_runtime(
                &workspace,
                mode,
                &args.model,
                &base_url,
                &api_key,
                approval,
            )?;
            let ctx = composition
                .ctx
                .child()
                .map_err(|error| alpha_harness::RunError::Service(error.to_string()))?;
            let streamed = install_cli_listeners(&ctx);
            let result = resume_session_in(
                &ctx,
                ResumeOptions { session_id, prompt },
                &composition,
                &store,
            )
            .await;
            ctx.dispose();
            Ok::<_, alpha_harness::RunError>((result, streamed))
        });
        return match result {
            Ok((Ok(report), streamed)) => {
                match &report.assistant_text {
                    Some(text) if !streamed.load(Ordering::Relaxed) => println!("{text}"),
                    Some(_) => println!(),
                    None => eprintln!(
                        "session {} resumed: {} messages, turn_open={}, repairs={}",
                        report.session_id, report.messages, report.turn_open, report.repairs
                    ),
                }
                eprintln!("session: {}", report.session_id);
                ExitCode::SUCCESS
            }
            Ok((Err(error), _)) | Err(error) => {
                eprintln!("alpha run: {error}");
                ExitCode::FAILURE
            }
        };
    }

    if prompt.trim().is_empty() {
        eprintln!("alpha run: provide a prompt or use --resume <SESSION_ID>");
        return ExitCode::FAILURE;
    }

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

    eprintln!(
        "alpha run: mode={}, model={}, workspace={}",
        args.mode,
        args.model,
        workspace.display()
    );
    let result = runtime.block_on(async move {
        let composition = compose_openai_runtime(
            &workspace,
            args.mode,
            &args.model,
            &base_url,
            &api_key,
            approval,
        )?;
        let store = SessionStore::new(SessionStore::default_root());
        let ctx = composition
            .ctx
            .child()
            .map_err(|error| alpha_harness::RunError::Service(error.to_string()))?;
        let streamed = install_cli_listeners(&ctx);
        let result = run_prompt_in(
            &ctx,
            RunOptions {
                workspace,
                mode: args.mode,
                model: args.model,
                prompt,
            },
            &composition,
            &store,
        )
        .await;
        ctx.dispose();
        Ok::<_, alpha_harness::RunError>((result, streamed))
    });

    match result {
        Ok((Ok(report), streamed)) => {
            if streamed.load(Ordering::Relaxed) {
                println!();
            } else {
                println!("{}", report.assistant_text);
            }
            eprintln!("session: {}", report.session_id);
            ExitCode::SUCCESS
        }
        Ok((Err(error), _)) | Err(error) => {
            eprintln!("alpha run: {error}");
            ExitCode::FAILURE
        }
    }
}

fn install_cli_listeners(ctx: &Ctx) -> Arc<AtomicBool> {
    let streamed = Arc::new(AtomicBool::new(false));
    let saw_text = streamed.clone();
    ctx.on_parallel::<AgentTextDelta, _>(move |event| {
        saw_text.store(true, Ordering::Relaxed);
        let text = event.text.clone();
        Box::pin(async move {
            use tokio::io::AsyncWriteExt;
            let mut stdout = tokio::io::stdout();
            let _ = stdout.write_all(text.as_bytes()).await;
            let _ = stdout.flush().await;
        })
    })
    .expect("text listener");

    ctx.on_parallel::<AgentStepStarted, _>(move |event| {
        eprintln!("alpha run: step {}: calling model...", event.step);
        Box::pin(async {})
    })
    .expect("step listener");

    ctx.on_parallel::<AgentToolStarted, _>(move |event| {
        let preview = serde_json::to_string(&event.input).unwrap_or_default();
        eprintln!("alpha run: -> {} {}", event.name, snippet(&preview, 160));
        Box::pin(async {})
    })
    .expect("tool start listener");

    ctx.on_parallel::<AgentToolFinished, _>(move |event| {
        let label = match event.status {
            alpha_module_session::ToolStatus::Completed => "ok",
            alpha_module_session::ToolStatus::Failed => "failed",
            alpha_module_session::ToolStatus::Interrupted => "interrupted",
        };
        let first_line = event.content.lines().next().unwrap_or_default();
        eprintln!(
            "alpha run: <- {} {}: {}",
            event.name,
            label,
            snippet(first_line, 120)
        );
        Box::pin(async {})
    })
    .expect("tool finish listener");

    ctx.on_parallel::<AgentError, _>(move |event| {
        eprintln!("alpha run: {}", event.message);
        Box::pin(async {})
    })
    .expect("error listener");
    streamed
}

fn snippet(text: &str, max_chars: usize) -> String {
    let mut output: String = text.chars().take(max_chars).collect();
    if text.chars().count() > max_chars {
        output.push_str("...");
    }
    output
}

//! Fixed command-line shell.
//!
//! The command list is built from the generated composition, so this file never
//! names a module or a frontend crate.

use std::process::ExitCode;

use alpha_host_api::{FrontEntry, FrontRequest, Host};
use alpha_schema::PermissionMode;
use alpha_session_log::SessionStore;
use clap::{Arg, ArgMatches, Command, value_parser};

const ABOUT: &str = "Lightweight, everything-is-a-module coding agent harness";

/// Run the command line against a composed host.
pub fn run(host: &'static Host) -> ExitCode {
    let matches = command(host).get_matches();
    match matches.subcommand() {
        Some(("sessions", _)) => list_sessions(),
        Some(("__core", sub)) => serve_core(sub),
        Some((name, sub)) => match host.fronts.iter().find(|front| front.command == name) {
            Some(front) => run_front(front, sub),
            None => ExitCode::FAILURE,
        },
        None => ExitCode::SUCCESS,
    }
}

fn command(host: &'static Host) -> Command {
    let mut command = Command::new("alpha")
        .version(env!("CARGO_PKG_VERSION"))
        .about(ABOUT)
        .subcommand_required(true)
        .arg_required_else_help(true);
    for front in host.fronts {
        command = command.subcommand(front_command(front));
    }
    command
        .subcommand(Command::new("sessions").about("List stored sessions"))
        .subcommand(
            Command::new("__core")
                .about("Internal core process entry point")
                .hide(true)
                .arg(Arg::new("stdio").long("stdio").action(clap::ArgAction::SetTrue)),
        )
}

fn front_command(front: &'static FrontEntry) -> Command {
    let command = Command::new(front.command).about(front.about);
    if !front.takes_prompt {
        return command;
    }
    command
        .arg(
            Arg::new("prompt")
                .num_args(0..)
                .help("Prompt text"),
        )
        .arg(
            Arg::new("mode")
                .long("mode")
                .value_parser(value_parser!(PermissionMode))
                .default_value("auto")
                .help("Permission mode: read, auto, reviewed, or free"),
        )
        .arg(
            Arg::new("workspace")
                .long("workspace")
                .value_parser(value_parser!(std::path::PathBuf))
                .help("Workspace root. Defaults to the current directory"),
        )
        .arg(
            Arg::new("provider")
                .long("provider")
                .help("Provider instance or provider kind. Defaults to the configured default"),
        )
        .arg(
            Arg::new("model")
                .long("model")
                .help("Model identifier. Defaults to the provider catalog"),
        )
        .arg(
            Arg::new("resume")
                .long("resume")
                .value_name("SESSION_ID")
                .value_parser(value_parser!(alpha_schema::SessionId))
                .help("Resume an existing session instead of starting a new one"),
        )
}

fn run_front(front: &'static FrontEntry, matches: &ArgMatches) -> ExitCode {
    // Commands that take no prompt do not declare these arguments, so every
    // lookup has to tolerate their absence.
    let workspace = matches
        .try_get_one::<std::path::PathBuf>("workspace")
        .ok()
        .flatten()
        .cloned()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let request = FrontRequest {
        workspace,
        mode: matches
            .try_get_one::<PermissionMode>("mode")
            .ok()
            .flatten()
            .copied()
            .unwrap_or(PermissionMode::Auto),
        provider: matches
            .try_get_one::<String>("provider")
            .ok()
            .flatten()
            .cloned()
            .unwrap_or_default(),
        model: matches
            .try_get_one::<String>("model")
            .ok()
            .flatten()
            .cloned()
            .unwrap_or_default(),
        resume: matches
            .try_get_one::<alpha_schema::SessionId>("resume")
            .ok()
            .flatten()
            .copied(),
        prompt: matches
            .try_get_many::<String>("prompt")
            .ok()
            .flatten()
            .map(|values| values.cloned().collect())
            .unwrap_or_default(),
    };
    match (front.run)(request) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("alpha {}: {error}", front.command);
            ExitCode::FAILURE
        }
    }
}

fn list_sessions() -> ExitCode {
    let store = SessionStore::new(SessionStore::default_root());
    match store.list_entries() {
        Ok(entries) => {
            for entry in entries {
                println!(
                    "{}\t{}\t{}\t{}",
                    entry.header.id,
                    entry.header.mode,
                    entry.header.created_at.as_millis(),
                    entry.header.workspace.root
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

fn serve_core(matches: &ArgMatches) -> ExitCode {
    if !matches.get_flag("stdio") {
        eprintln!("alpha __core: --stdio is required");
        return ExitCode::FAILURE;
    }
    let store = SessionStore::new(SessionStore::default_root());
    match crate::serve_stdio(store) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("alpha __core: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    static RAN: AtomicBool = AtomicBool::new(false);

    fn fake_desktop(_request: FrontRequest) -> Result<(), alpha_host_api::FrontError> {
        RAN.store(true, Ordering::SeqCst);
        Ok(())
    }

    static FRONTS: &[FrontEntry] = &[FrontEntry {
        command: "desktop",
        about: "Open the desktop app",
        takes_prompt: false,
        run: fake_desktop,
    }];

    static HOST: Host = Host {
        modules: &[],
        fronts: FRONTS,
    };

    #[test]
    fn prompt_less_fronts_do_not_declare_run_options() {
        let matches = command(&HOST)
            .try_get_matches_from(["alpha", "desktop"])
            .expect("desktop parses");
        let (name, sub) = matches.subcommand().expect("subcommand");
        assert_eq!(name, "desktop");
        let front = HOST
            .fronts
            .iter()
            .find(|front| front.command == name)
            .expect("front");
        assert_eq!(run_front(front, sub), ExitCode::SUCCESS);
        assert!(RAN.load(Ordering::SeqCst));
    }

    #[test]
    fn run_command_still_accepts_its_options() {
        let matches = command(&HOST)
            .try_get_matches_from([
                "alpha",
                "run",
                "--mode",
                "free",
                "--workspace",
                "/tmp",
                "--resume",
                "019a7a30-0000-7000-8000-000000000000",
                "hello",
            ])
            .ok();
        assert!(matches.is_none(), "unknown subcommand must not parse");
    }
}

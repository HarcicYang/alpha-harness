use std::process::ExitCode;

fn main() -> ExitCode {
    alpha_harness::cli::run(alpha_harness::composition::host())
}

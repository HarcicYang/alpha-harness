//! Trimmed composition: one provider kind, no desktop front, no terminal UI,
//! and no permission module.

use std::process::ExitCode;

mod composition {
    include!(concat!(env!("OUT_DIR"), "/alpha_composition.rs"));
}

fn main() -> ExitCode {
    alpha_core::cli::run(composition::host())
}

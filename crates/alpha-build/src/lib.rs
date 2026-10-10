//! Build-time composition validation and module table generation.
//!
//! Reads the resolved Cargo dependency graph, validates
//! `[package.metadata.alpha]` declarations, and writes an explicit module
//! table into `OUT_DIR`. Never edits user files.

#![forbid(unsafe_code)]

pub mod check;
pub mod generate;
pub mod metadata;

pub use check::{BuildError, ModuleInfo, check};
pub use generate::check_and_generate;

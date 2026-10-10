//! Build-time composition validation and module table generation.
//!
//! Reads the resolved Cargo dependency graph, validates
//! `[package.metadata.alpha]` declarations, and writes an explicit module
//! table into `OUT_DIR`. Never edits user files.
//!
//! Skeleton crate: implementation is pending.

#![forbid(unsafe_code)]

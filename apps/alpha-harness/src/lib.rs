//! Composition root for the `alpha` binary.

include!(concat!(env!("OUT_DIR"), "/alpha_modules.rs"));

pub mod composition {
    include!(concat!(env!("OUT_DIR"), "/alpha_composition.rs"));
}

pub use alpha_core::*;

/// Module keys enabled by this composition.
pub fn module_keys() -> &'static [&'static str] {
    ALPHA_MODULE_KEYS
}

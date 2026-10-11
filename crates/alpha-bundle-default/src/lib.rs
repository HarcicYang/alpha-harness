//! The default M1 module bundle. Dependencies only, no behavior.
//!
//! Re-exports every bundled module crate so generated composition code can
//! reference module descriptors through this crate.

#![forbid(unsafe_code)]

pub use alpha_module_compaction;
pub use alpha_module_config;
pub use alpha_module_credential;
pub use alpha_module_fs;
pub use alpha_module_http;
pub use alpha_module_loop;
pub use alpha_module_permission;
pub use alpha_module_process;
pub use alpha_module_prompt;
pub use alpha_module_provider_openai;
pub use alpha_module_session;
pub use alpha_module_tools;

//! The default M1 module bundle. Dependencies only, no behavior.
//!
//! Re-exports every bundled module crate so generated composition code can
//! reference module descriptors through this crate.

#![forbid(unsafe_code)]

pub use alpha_api_anthropic_messages;
pub use alpha_front_desktop;
pub use alpha_front_runtime;
pub use alpha_front_tui;
pub use alpha_api_openai_chat;
pub use alpha_api_openai_responses;
pub use alpha_module_compaction;
pub use alpha_module_config;
pub use alpha_module_credential;
pub use alpha_module_fs;
pub use alpha_module_http;
pub use alpha_module_loop;
pub use alpha_module_permission;
pub use alpha_module_process;
pub use alpha_module_prompt;
pub use alpha_module_provider_anthropic_messages;
pub use alpha_module_provider_config;
pub use alpha_module_provider_deepseek;
pub use alpha_module_provider_openai;
pub use alpha_module_provider_openai_chat_completions;
pub use alpha_module_provider_openai_responses;
pub use alpha_module_provider_opencode_go;
pub use alpha_module_tools;
pub use alpha_provider_api;

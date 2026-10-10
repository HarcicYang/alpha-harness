//! Component runtime for alpha-harness: context, services, events, effects,
//! scopes, and structured task spawning.
//!
//! The kernel holds mechanisms only. Policies and user-visible behavior live
//! in modules.

#![forbid(unsafe_code)]

pub mod context;
pub mod effect;
pub mod error;

pub use context::{Ctx, ServiceChange};
pub use effect::{Disposer, Scope, TaskHandle};
pub use error::KernelError;

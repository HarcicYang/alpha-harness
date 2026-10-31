//! Agent contract: providers, model routes, registries, and the driver seam.
//!
//! The fixed shell drives a turn through these types only, so the loop
//! implementation is a replaceable dependency.

#![forbid(unsafe_code)]

pub mod driver;
pub mod error;
pub mod provider;

pub use driver::{AgentDriver, ContextInjector};

pub use error::{ContextError, LoopError, TurnOutcome};

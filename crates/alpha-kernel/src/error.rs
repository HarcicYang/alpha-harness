//! Kernel errors.

/// Errors produced by kernel primitives.
#[derive(Debug, thiserror::Error)]
pub enum KernelError {
    /// A registration was attempted on a disposed scope.
    #[error("scope is closed")]
    ScopeClosed,

    /// The same singleton service type was provided twice in one context.
    #[error("service `{type_name}` is already provided in this context")]
    ServiceConflict { type_name: &'static str },

    /// A required service is not available in this context or its ancestors.
    #[error("required service `{type_name}` is not available")]
    MissingService { type_name: &'static str },
}

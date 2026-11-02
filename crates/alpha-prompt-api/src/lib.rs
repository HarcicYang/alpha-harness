//! Prompt assembly contract.
//!
//! A prompt is assembled from independently registered parts. Any crate may
//! contribute a system-prompt section or a non-system context block through the
//! registries in this crate; nothing has to depend on the prompt module itself.
//!
//! The fixed shell asks for a system prompt through [`PromptAssembler`] only, so
//! the prompt module is a replaceable dependency rather than a fixed one.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use alpha_kernel::{Ctx, Disposer};
use alpha_session_log::SessionState;
use thiserror::Error;

/// Prompt assembly contract consumed by the application runtime.
pub trait PromptAssembler: Send + Sync + 'static {
    /// Assemble a deterministic system prompt for one session.
    fn assemble(&self, ctx: &Ctx, state: &SessionState) -> Result<String, PromptError>;
}

/// Errors from prompt assembly.
#[derive(Debug, Error)]
pub enum PromptError {
    /// Two providers use the same stable key.
    #[error("prompt section `{0}` is already registered")]
    DuplicateSection(String),
    /// Kernel registration failed.
    #[error("prompt registration failed: {0}")]
    Kernel(String),
    /// The session cwd is outside its workspace root.
    #[error("cwd `{cwd}` is outside workspace root `{root}`")]
    InvalidWorkspace {
        /// Workspace root.
        root: String,
        /// Session cwd.
        cwd: String,
    },
    /// Reading a project instruction file failed.
    #[error("failed to read project instructions `{path}`: {message}")]
    ProjectInstruction {
        /// Workspace-relative path.
        path: String,
        /// Underlying error.
        message: String,
    },
}

/// Ordering tier for one prompt section.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PromptPriority {
    /// Runtime facts and environment.
    Runtime = 10,
    /// Mandatory framework rules.
    Policy = 20,
    /// Output contract.
    Output = 30,
    /// Tool-use policy.
    Tools = 40,
    /// Project-provided instructions.
    Project = 50,
    /// Optional capability notes.
    Capability = 60,
    /// Role and persona, rendered last.
    Persona = 70,
}

/// One request to render a prompt section or context block.
pub struct PromptRequest<'a> {
    /// Active context.
    pub ctx: &'a Ctx,
    /// Session state before the turn starts.
    pub state: &'a SessionState,
}

/// One composable system-prompt section.
pub trait PromptSectionProvider: Send + Sync + 'static {
    /// Stable section key.
    fn key(&self) -> &'static str;

    /// Ordering tier.
    fn priority(&self) -> PromptPriority;

    /// Render the section. Returning `None` omits it.
    fn render(&self, request: &PromptRequest<'_>) -> Result<Option<String>, PromptError>;
}

/// One dynamic context section rendered into durable user messages.
pub trait ContextSectionProvider: Send + Sync + 'static {
    /// Stable section key. Re-rendering the same key replaces the previous block.
    fn key(&self) -> &'static str;

    /// Ordering tier.
    fn priority(&self) -> PromptPriority;

    /// Render the current section body. Returning `None` omits it.
    fn render(&self, request: &PromptRequest<'_>) -> Result<Option<String>, PromptError>;
}

macro_rules! registry {
    ($registry:ident, $provider:ident, $label:literal) => {
        /// Registry of prompt providers contributed by any crate.
        #[derive(Clone, Default)]
        pub struct $registry {
            providers: Arc<RwLock<BTreeMap<String, Arc<dyn $provider>>>>,
        }

        impl $registry {
            /// Create an empty registry.
            pub fn new() -> Self {
                Self::default()
            }

            /// Register one provider as a reversible effect of `ctx`.
            pub fn register(
                &self,
                ctx: &Ctx,
                provider: Arc<dyn $provider>,
            ) -> Result<Disposer, PromptError> {
                let key = provider.key().to_owned();
                {
                    let mut providers = self.providers.write().expect($label);
                    if providers.contains_key(&key) {
                        return Err(PromptError::DuplicateSection(key));
                    }
                    providers.insert(key.clone(), provider);
                }

                let providers = self.providers.clone();
                let cleanup_key = key.clone();
                match ctx.scope().effect(move || {
                    providers.write().expect($label).remove(&cleanup_key);
                }) {
                    Ok(disposer) => Ok(disposer),
                    Err(error) => {
                        self.providers.write().expect($label).remove(&key);
                        Err(PromptError::Kernel(error.to_string()))
                    }
                }
            }

            /// Every registered provider in render order.
            fn ordered(&self) -> Vec<Arc<dyn $provider>> {
                let mut providers: Vec<Arc<dyn $provider>> = self
                    .providers
                    .read()
                    .expect($label)
                    .values()
                    .cloned()
                    .collect();
                providers.sort_by(|left, right| {
                    left.priority()
                        .cmp(&right.priority())
                        .then_with(|| left.key().cmp(right.key()))
                });
                providers
            }
        }
    };
}

registry!(
    PromptRegistry,
    PromptSectionProvider,
    "prompt lock poisoned"
);
registry!(
    ContextRegistry,
    ContextSectionProvider,
    "context lock poisoned"
);

impl PromptRegistry {
    /// Render every registered section into ordered, non-empty blocks.
    pub fn render(&self, request: &PromptRequest<'_>) -> Result<Vec<String>, PromptError> {
        let mut output = Vec::new();
        for provider in self.ordered() {
            if let Some(section) = provider.render(request)? {
                let section = section.trim();
                if !section.is_empty() {
                    output.push(section.to_owned());
                }
            }
        }
        Ok(output)
    }
}

impl ContextRegistry {
    /// Render every registered block into ordered `(key, body)` pairs.
    pub fn render(
        &self,
        request: &PromptRequest<'_>,
    ) -> Result<Vec<(String, String)>, PromptError> {
        let mut output = Vec::new();
        for provider in self.ordered() {
            if let Some(section) = provider.render(request)? {
                let section = section.trim();
                if !section.is_empty() {
                    output.push((provider.key().to_owned(), section.to_owned()));
                }
            }
        }
        Ok(output)
    }
}

/// The system-prompt registry of this context, created on first use.
///
/// Registration order does not matter: a crate that activates before the prompt
/// module owns the registry the prompt module later reuses.
pub fn section_registry(ctx: &Ctx) -> Result<Arc<PromptRegistry>, PromptError> {
    if let Some(registry) = ctx.get::<PromptRegistry>() {
        return Ok(registry);
    }
    let registry = Arc::new(PromptRegistry::new());
    ctx.provide::<PromptRegistry>(registry.clone())
        .map_err(|error| PromptError::Kernel(error.to_string()))?;
    Ok(registry)
}

/// The injected-context registry of this context, created on first use.
pub fn context_registry(ctx: &Ctx) -> Result<Arc<ContextRegistry>, PromptError> {
    if let Some(registry) = ctx.get::<ContextRegistry>() {
        return Ok(registry);
    }
    let registry = Arc::new(ContextRegistry::new());
    ctx.provide::<ContextRegistry>(registry.clone())
        .map_err(|error| PromptError::Kernel(error.to_string()))?;
    Ok(registry)
}

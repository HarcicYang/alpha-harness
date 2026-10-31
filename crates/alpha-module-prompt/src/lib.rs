//! Composable system-prompt assembly.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use alpha_kernel::{ActivateError, CapabilityScope, CapabilitySpec, Ctx, Disposer, Module};
use alpha_module_loop::{ChatMessage, ContextError, ContextInjector};
use alpha_session_log::{MessageRecord, SessionEvent, SessionState, SessionWriter};
use alpha_schema::MessageId;
use futures::future::BoxFuture;
use serde::Deserialize;

mod builtin;

pub use builtin::{
    MandatoryRulesSection, OutputRulesSection, PermissionModesSection, PersonaSection,
    StopConditionsSection, ToolRulesSection,
};

/// Stable module key used by the generated composition table.
pub const __ALPHA_MODULE__: &str = "prompt";

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

/// One request to render a prompt section.
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

/// Registry of prompt-section providers.
#[derive(Default)]
pub struct PromptRegistry {
    sections: Arc<RwLock<BTreeMap<String, Arc<dyn PromptSectionProvider>>>>,
}

impl PromptRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register one provider as a reversible effect of `ctx`.
    pub fn register(
        &self,
        ctx: &Ctx,
        provider: Arc<dyn PromptSectionProvider>,
    ) -> Result<Disposer, PromptError> {
        let key = provider.key().to_owned();
        {
            let mut sections = self.sections.write().expect("prompt lock poisoned");
            if sections.contains_key(&key) {
                return Err(PromptError::DuplicateSection(key));
            }
            sections.insert(key.clone(), provider);
        }

        let sections = self.sections.clone();
        let cleanup_key = key.clone();
        match ctx.scope().effect(move || {
            sections
                .write()
                .expect("prompt lock poisoned")
                .remove(&cleanup_key);
        }) {
            Ok(disposer) => Ok(disposer),
            Err(error) => {
                self.sections
                    .write()
                    .expect("prompt lock poisoned")
                    .remove(&key);
                Err(PromptError::Kernel(error.to_string()))
            }
        }
    }

    fn render(&self, request: &PromptRequest<'_>) -> Result<Vec<String>, PromptError> {
        let mut providers: Vec<Arc<dyn PromptSectionProvider>> = self
            .sections
            .read()
            .expect("prompt lock poisoned")
            .values()
            .cloned()
            .collect();
        providers.sort_by(|left, right| {
            left.priority()
                .cmp(&right.priority())
                .then_with(|| left.key().cmp(right.key()))
        });

        let mut output = Vec::new();
        for provider in providers {
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

/// Prompt assembly contract consumed by the application runtime.
pub trait PromptAssembler: Send + Sync + 'static {
    /// Assemble a deterministic system prompt for one session.
    fn assemble(&self, ctx: &Ctx, state: &SessionState) -> Result<String, PromptError>;
}

/// One dynamic context section rendered into durable user messages.
pub trait ContextSectionProvider: Send + Sync + 'static {
    /// Stable section key.
    fn key(&self) -> &'static str;

    /// Ordering tier.
    fn priority(&self) -> PromptPriority;

    /// Render the current section body.
    fn render(&self, request: &PromptRequest<'_>) -> Result<Option<String>, PromptError>;
}

/// Registry of dynamic context sections.
#[derive(Default)]
pub struct ContextRegistry {
    sections: Arc<RwLock<BTreeMap<String, Arc<dyn ContextSectionProvider>>>>,
}

impl ContextRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register one provider as a reversible effect of `ctx`.
    pub fn register(
        &self,
        ctx: &Ctx,
        provider: Arc<dyn ContextSectionProvider>,
    ) -> Result<Disposer, PromptError> {
        let key = provider.key().to_owned();
        {
            let mut sections = self.sections.write().expect("context lock poisoned");
            if sections.contains_key(&key) {
                return Err(PromptError::DuplicateSection(key));
            }
            sections.insert(key.clone(), provider);
        }

        let sections = self.sections.clone();
        let cleanup_key = key.clone();
        match ctx.scope().effect(move || {
            sections
                .write()
                .expect("context lock poisoned")
                .remove(&cleanup_key);
        }) {
            Ok(disposer) => Ok(disposer),
            Err(error) => {
                self.sections
                    .write()
                    .expect("context lock poisoned")
                    .remove(&key);
                Err(PromptError::Kernel(error.to_string()))
            }
        }
    }

    fn render(&self, request: &PromptRequest<'_>) -> Result<Vec<(String, String)>, PromptError> {
        let mut providers: Vec<Arc<dyn ContextSectionProvider>> = self
            .sections
            .read()
            .expect("context lock poisoned")
            .values()
            .cloned()
            .collect();
        providers.sort_by(|left, right| {
            left.priority()
                .cmp(&right.priority())
                .then_with(|| left.key().cmp(right.key()))
        });

        let mut output = Vec::new();
        for provider in providers {
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

/// Default injector over a dynamic context registry.
pub struct PromptContextInjector {
    registry: Arc<ContextRegistry>,
}

impl PromptContextInjector {
    /// Create an injector over one registry.
    pub fn new(registry: Arc<ContextRegistry>) -> Self {
        Self { registry }
    }
}

impl ContextInjector for PromptContextInjector {
    fn inject<'a>(
        &'a self,
        ctx: &'a Ctx,
        writer: &'a SessionWriter,
        state: &'a mut SessionState,
        messages: &'a mut Vec<ChatMessage>,
    ) -> BoxFuture<'a, Result<(), ContextError>> {
        Box::pin(async move {
            let request = PromptRequest { ctx, state };
            let sections = self
                .registry
                .render(&request)
                .map_err(|error| ContextError::message(error.to_string()))?;
            let visible = visible_context(state);
            for (key, body) in sections {
                let text = context_message(&key, &body);
                if visible.get(&key) == Some(&text) {
                    continue;
                }
                let id = MessageId::new();
                let event = writer
                    .append(SessionEvent::ContextInjected {
                        id,
                        key,
                        text: text.clone(),
                    })
                    .map_err(|error| ContextError::message(error.to_string()))?;
                state.apply(&event);
                messages.push(ChatMessage::User(text));
            }
            Ok(())
        })
    }
}

fn visible_context(state: &SessionState) -> BTreeMap<String, String> {
    let mut visible = BTreeMap::new();
    for record in &state.messages {
        if state
            .compaction
            .as_ref()
            .is_some_and(|compaction| record.seq() <= compaction.through_seq)
        {
            continue;
        }
        if let MessageRecord::Context { key, text, .. } = record {
            visible.insert(key.clone(), text.clone());
        }
    }
    visible
}

fn context_message(key: &str, body: &str) -> String {
    format!(
        "<alpha_context key=\"{key}\">\n{}\n</alpha_context>",
        body.trim()
    )
}

/// Default prompt assembler over a section registry.
pub struct DefaultPromptAssembler {
    registry: Arc<PromptRegistry>,
}

impl DefaultPromptAssembler {
    /// Create an assembler over one registry.
    pub fn new(registry: Arc<PromptRegistry>) -> Self {
        Self { registry }
    }
}

impl PromptAssembler for DefaultPromptAssembler {
    fn assemble(&self, ctx: &Ctx, state: &SessionState) -> Result<String, PromptError> {
        let request = PromptRequest { ctx, state };
        Ok(self.registry.render(&request)?.join("\n\n"))
    }
}

/// Prompt assembly failures.
#[derive(Debug, thiserror::Error)]
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

/// Configuration for prompt assembly.
#[derive(Clone, Debug, Deserialize)]
pub struct PromptConfig {
    /// Whether `AGENTS.md` files are discovered and included.
    #[serde(default = "default_include_project_instructions")]
    pub include_project_instructions: bool,
    /// Maximum total bytes included from project instruction files.
    #[serde(default = "default_max_project_instruction_bytes")]
    pub max_project_instruction_bytes: usize,
}

impl Default for PromptConfig {
    fn default() -> Self {
        Self {
            include_project_instructions: default_include_project_instructions(),
            max_project_instruction_bytes: default_max_project_instruction_bytes(),
        }
    }
}

fn default_include_project_instructions() -> bool {
    true
}

fn default_max_project_instruction_bytes() -> usize {
    64 * 1024
}

/// Registers the prompt assembler and its built-in sections.
pub struct PromptModule;

impl Module for PromptModule {
    type Requirements = ();
    type Config = PromptConfig;

    const KEY: &'static str = "prompt";
    const CAPABILITIES: &'static [CapabilitySpec] = &[
        CapabilitySpec::singleton("prompt.assembler", CapabilityScope::Location),
        CapabilitySpec::registry("prompt.section", CapabilityScope::Location),
        CapabilitySpec::registry("prompt.context", CapabilityScope::Location),
        CapabilitySpec::singleton("context.injector", CapabilityScope::Location),
    ];

    fn activate(&self, ctx: &Ctx, config: &Self::Config) -> Result<(), ActivateError> {
        let registry = Arc::new(PromptRegistry::new());
        for section in builtin::sections(config) {
            registry
                .register(ctx, section)
                .map_err(|error| ActivateError::new(Self::KEY, error))?;
        }
        ctx.provide::<PromptRegistry>(registry.clone())
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        let assembler = Arc::new(DefaultPromptAssembler::new(registry));
        ctx.provide::<dyn PromptAssembler>(assembler)
            .map_err(|error| ActivateError::new(Self::KEY, error))?;

        let context_registry = Arc::new(ContextRegistry::new());
        for section in builtin::context_sections() {
            context_registry
                .register(ctx, section)
                .map_err(|error| ActivateError::new(Self::KEY, error))?;
        }
        ctx.provide::<ContextRegistry>(context_registry.clone())
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        let injector: Arc<dyn ContextInjector> =
            Arc::new(PromptContextInjector::new(context_registry));
        ctx.provide::<dyn ContextInjector>(injector)
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        Ok(())
    }
}

//! Composable system-prompt assembly.
//!
//! The assembly plumbing lives here; the sections themselves belong to the crates
//! that own the facts they describe. Any crate contributes through
//! [`alpha_prompt_api::section_registry`] or [`alpha_prompt_api::context_registry`].

pub use alpha_prompt_api::{
    ContextRegistry, ContextSectionProvider, PromptAssembler, PromptError, PromptPriority,
    PromptRegistry, PromptRequest, PromptSectionProvider,
};

use std::collections::BTreeMap;
use std::sync::Arc;

use alpha_kernel::{ActivateError, CapabilityScope, CapabilitySpec, Ctx, Module};
use alpha_module_loop::{ChatMessage, ContextError, ContextInjector};
use alpha_schema::MessageId;
use alpha_session_log::{MessageRecord, SessionEvent, SessionState, SessionWriter};
use futures::future::BoxFuture;
use serde::Deserialize;

mod builtin;

pub use builtin::{
    MandatoryRulesSection, OutputRulesSection, PersonaSection, StopConditionsSection,
};

/// Stable module key used by the generated composition table.
pub const __ALPHA_MODULE__: &str = "prompt";

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
        let registry = alpha_prompt_api::section_registry(ctx)
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        for section in builtin::sections(config) {
            registry
                .register(ctx, section)
                .map_err(|error| ActivateError::new(Self::KEY, error))?;
        }
        let assembler = Arc::new(DefaultPromptAssembler::new(registry));
        ctx.provide::<dyn PromptAssembler>(assembler)
            .map_err(|error| ActivateError::new(Self::KEY, error))?;

        let context_registry = alpha_prompt_api::context_registry(ctx)
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        for section in builtin::context_sections() {
            context_registry
                .register(ctx, section)
                .map_err(|error| ActivateError::new(Self::KEY, error))?;
        }
        let injector: Arc<dyn ContextInjector> =
            Arc::new(PromptContextInjector::new(context_registry));
        ctx.provide::<dyn ContextInjector>(injector)
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        Ok(())
    }
}

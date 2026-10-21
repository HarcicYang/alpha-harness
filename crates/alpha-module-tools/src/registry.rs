//! Named tool registry.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use thiserror::Error;

use crate::contract::{Tool, ToolError};

/// Registry errors.
#[derive(Debug, Error)]
pub enum RegistryError {
    /// A tool with this name is already registered.
    #[error("tool `{0}` is already registered")]
    Duplicate(String),
}

/// Named tool registry.
#[derive(Default)]
pub struct ToolRegistry {
    tools: RwLock<BTreeMap<String, Arc<dyn Tool>>>,
}

impl ToolRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a tool under its own name.
    pub fn register(&self, tool: Arc<dyn Tool>) -> Result<(), RegistryError> {
        let name = tool.name().to_owned();
        let mut tools = self.tools.write().expect("tool lock poisoned");
        if tools.contains_key(&name) {
            return Err(RegistryError::Duplicate(name));
        }
        tools.insert(name, tool);
        Ok(())
    }

    /// Remove a tool by name.
    pub fn unregister(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.write().expect("tool lock poisoned").remove(name)
    }

    /// Look up a tool by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools
            .read()
            .expect("tool lock poisoned")
            .get(name)
            .cloned()
    }

    /// Registered tool names, sorted.
    pub fn names(&self) -> Vec<String> {
        self.tools
            .read()
            .expect("tool lock poisoned")
            .keys()
            .cloned()
            .collect()
    }

    /// Prepare every registered tool before schema generation.
    pub async fn prepare_all(&self) -> Result<(), ToolError> {
        let tools = self
            .tools
            .read()
            .expect("tool lock poisoned")
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for tool in tools {
            tool.prepare().await?;
        }
        Ok(())
    }
}

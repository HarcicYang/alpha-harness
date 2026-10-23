use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use alpha_front_supervisor::CoreSupervisor;
use alpha_schema::{
    CredentialStatus, ModelDescriptor, ProviderDescriptor, ProviderInstance, ReasoningLevel,
    default_model_id, effective_models,
};
use serde_json::{Value, json};

use crate::FrontError;

/// UI-independent provider configuration and model catalog state.
#[derive(Clone, Debug, Default)]
pub struct ProviderState {
    /// Registered provider kinds.
    pub kinds: Vec<ProviderDescriptor>,
    /// Persisted provider instances.
    pub instances: Vec<ProviderInstance>,
    /// Dynamically discovered model catalogs.
    pub model_cache: BTreeMap<String, Vec<ModelDescriptor>>,
    /// Credential backend status.
    pub credential_status: Option<CredentialStatus>,
}

impl ProviderState {
    /// Reload provider state from the managed core.
    pub fn refresh(&mut self, supervisor: &mut CoreSupervisor) -> Result<(), FrontError> {
        let kinds = supervisor.request("provider.list_kinds", None)?;
        let instances = supervisor.request("provider.list_instances", None)?;
        let credential_status = supervisor.request("provider.credential_status", None)?;
        self.kinds = parse_response(kinds)?;
        self.instances = parse_response(instances)?;
        self.credential_status = Some(parse_response(credential_status)?);
        Ok(())
    }

    /// Refresh dynamic model catalogs, retaining static metadata on failure.
    pub fn refresh_models(
        &mut self,
        supervisor: &mut CoreSupervisor,
        provider: Option<&str>,
    ) -> Result<(), FrontError> {
        let mut instance_ids = self
            .instances
            .iter()
            .filter(|instance| !instance.disabled)
            .map(|instance| instance.id.clone())
            .collect::<Vec<_>>();
        if let Some(provider) = provider {
            instance_ids.retain(|id| id == provider);
        }
        self.model_cache
            .retain(|id, _| instance_ids.iter().any(|instance_id| instance_id == id));
        let mut failures = Vec::new();
        for id in instance_ids {
            if self.model_cache.contains_key(&id) {
                continue;
            }
            match supervisor.request("provider.list_models", Some(json!({ "id": id }))) {
                Ok(value) => match parse_response::<Vec<ModelDescriptor>>(value) {
                    Ok(models) => {
                        self.model_cache.insert(id, models);
                    }
                    Err(error) => failures.push(format!("{id}: {error}")),
                },
                Err(error) => failures.push(format!("{id}: {error}")),
            }
        }
        if !failures.is_empty() {
            return Err(FrontError::Protocol(format!(
                "model catalog fallback: {}",
                failures.join("; ")
            )));
        }
        Ok(())
    }

    /// Look up one provider kind.
    pub fn kind(&self, kind: &str) -> Option<&ProviderDescriptor> {
        self.kinds.iter().find(|descriptor| descriptor.kind == kind)
    }

    /// Look up one provider instance.
    pub fn instance(&self, id: &str) -> Option<&ProviderInstance> {
        self.instances.iter().find(|instance| instance.id == id)
    }

    /// Create an instance using the defaults from one provider kind.
    pub fn create_instance(
        &mut self,
        supervisor: &mut CoreSupervisor,
        kind_id: &str,
    ) -> Result<String, FrontError> {
        let kind = self
            .kind(kind_id)
            .ok_or_else(|| FrontError::Protocol(format!("unknown provider kind: {kind_id}")))?;
        let id = format!(
            "{}-{}",
            kind.kind,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        );
        let instance = ProviderInstance {
            id: id.clone(),
            kind: kind.kind.clone(),
            config_version: 1,
            config: kind.default_config.clone(),
            auth_ref: None,
            model_overrides: json!({}),
            disabled: false,
        };
        supervisor.request("provider.create_instance", Some(json!(instance)))?;
        self.refresh(supervisor)?;
        Ok(id)
    }

    /// Remove one provider instance.
    pub fn remove_instance(
        &mut self,
        supervisor: &mut CoreSupervisor,
        id: &str,
    ) -> Result<ProviderRemoved, FrontError> {
        let response = supervisor.request("provider.remove_instance", Some(json!({ "id": id })))?;
        let removed = ProviderRemoved {
            id: id.to_owned(),
            credential_removed: response
                .get("credentialRemoved")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            credential_error: response
                .get("credentialError")
                .and_then(Value::as_str)
                .map(str::to_owned),
        };
        self.model_cache.remove(id);
        self.refresh(supervisor)?;
        Ok(removed)
    }

    /// Select the default provider instance.
    pub fn set_default_instance(
        &mut self,
        supervisor: &mut CoreSupervisor,
        id: &str,
    ) -> Result<(), FrontError> {
        supervisor.request("provider.set_default_instance", Some(json!({ "id": id })))?;
        Ok(())
    }

    /// Validate one provider instance.
    pub fn test_instance(
        &self,
        supervisor: &mut CoreSupervisor,
        id: &str,
    ) -> Result<(), FrontError> {
        supervisor.request("provider.test_instance", Some(json!({ "id": id })))?;
        Ok(())
    }

    /// Store a credential for one provider instance.
    pub fn authenticate_instance(
        &mut self,
        supervisor: &mut CoreSupervisor,
        id: &str,
        secret: String,
    ) -> Result<(), FrontError> {
        if secret.is_empty() {
            return Err(FrontError::Protocol(
                "credential input was empty".to_owned(),
            ));
        }
        supervisor.request(
            "provider.authenticate_instance",
            Some(json!({ "id": id, "secret": secret })),
        )?;
        self.refresh(supervisor)
    }

    /// Update and validate one provider instance, rolling back on failure.
    pub fn update_instance(
        &mut self,
        supervisor: &mut CoreSupervisor,
        instance: ProviderInstance,
    ) -> Result<(), FrontError> {
        let original = self
            .instance(&instance.id)
            .cloned()
            .ok_or_else(|| FrontError::Protocol("unknown provider instance".to_owned()))?;
        supervisor.request("provider.update_instance", Some(json!(instance)))?;
        if let Err(error) =
            supervisor.request("provider.test_instance", Some(json!({ "id": instance.id })))
        {
            let _ = supervisor.request("provider.update_instance", Some(json!(original)));
            return Err(error.into());
        }
        self.model_cache.remove(&instance.id);
        self.refresh(supervisor)
    }

    /// Persist the default model for one provider instance.
    pub fn set_default_model(
        &self,
        supervisor: &mut CoreSupervisor,
        provider: &str,
        model: &str,
    ) -> Result<(), FrontError> {
        supervisor.request(
            "provider.set_default_model",
            Some(json!({ "id": provider, "model": model })),
        )?;
        supervisor.request(
            "provider.set_default_instance",
            Some(json!({ "id": provider })),
        )?;
        Ok(())
    }

    /// Build the effective model choices visible to every frontend.
    pub fn model_choices(&self) -> Vec<ModelChoice> {
        let mut choices = Vec::new();
        for instance in self.instances.iter().filter(|instance| !instance.disabled) {
            let Some(kind) = self.kind(&instance.kind) else {
                continue;
            };
            let mut models = self
                .model_cache
                .get(&instance.id)
                .cloned()
                .unwrap_or_else(|| effective_models(instance, &kind.models));
            let default_model = default_model_id(instance, &models);
            models.sort_by_key(|model| Some(model.id.as_str()) != default_model.as_deref());
            for model in models {
                choices.push(ModelChoice {
                    provider: instance.id.clone(),
                    provider_name: kind.name.clone(),
                    model: model.id,
                    model_name: model.name,
                    reasoning_levels: if model.reasoning {
                        model.reasoning_levels
                    } else {
                        vec![ReasoningLevel::Auto]
                    },
                    context_window: model.context_window as usize,
                    max_tokens: model.max_tokens as usize,
                    provider_tools: model.provider_tools,
                });
            }
        }
        choices
    }

    /// Resolve the current provider/model metadata from the effective catalog.
    pub fn model_choice(&self, provider: &str, model: &str) -> Option<ModelChoice> {
        let choices = self.model_choices();
        choices
            .iter()
            .find(|choice| choice.provider == provider && choice.model == model)
            .or_else(|| choices.iter().find(|choice| choice.provider == provider))
            .cloned()
    }
}

/// Result of removing a provider instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderRemoved {
    /// Removed instance id.
    pub id: String,
    /// Whether credential cleanup succeeded.
    pub credential_removed: bool,
    /// Credential cleanup failure detail.
    pub credential_error: Option<String>,
}

/// One selectable provider/model combination.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelChoice {
    /// Provider instance id.
    pub provider: String,
    /// Human-readable provider kind name.
    pub provider_name: String,
    /// Model id.
    pub model: String,
    /// Human-readable model name.
    pub model_name: String,
    /// Selectable reasoning levels.
    pub reasoning_levels: Vec<ReasoningLevel>,
    /// Maximum model context window in tokens.
    pub context_window: usize,
    /// Maximum model output tokens.
    pub max_tokens: usize,
    /// Provider-native tools advertised by the model.
    pub provider_tools: Vec<String>,
}

/// Resolve a model target against the effective catalog.
pub fn resolve_model_selection(
    choices: &[ModelChoice],
    current_provider: &str,
    target: &str,
) -> Option<ModelChoice> {
    let target = target.trim();
    if target.is_empty() {
        return None;
    }
    if let Some(choice) = choices
        .iter()
        .find(|choice| format!("{}/{}", choice.provider, choice.model) == target)
        .or_else(|| {
            choices
                .iter()
                .find(|choice| choice.provider == current_provider && choice.model == target)
        })
        .or_else(|| {
            let mut matches = choices.iter().filter(|choice| choice.model == target);
            let first = matches.next()?;
            matches.next().is_none().then_some(first)
        })
    {
        return Some(choice.clone());
    }

    let explicit = choices.iter().find_map(|choice| {
        target
            .strip_prefix(&format!("{}/", choice.provider))
            .map(|model| (choice.provider.clone(), model.to_owned()))
    });
    let (provider, model) = explicit.unwrap_or_else(|| {
        let provider = if choices
            .iter()
            .any(|choice| choice.provider == current_provider)
        {
            current_provider.to_owned()
        } else {
            choices
                .first()
                .map(|choice| choice.provider.clone())
                .unwrap_or_default()
        };
        (provider, target.to_owned())
    });
    if provider.is_empty() || model.is_empty() {
        return None;
    }
    let provider_name = choices
        .iter()
        .find(|choice| choice.provider == provider)
        .map(|choice| choice.provider_name.clone())
        .unwrap_or_else(|| provider.clone());
    Some(ModelChoice {
        provider,
        provider_name,
        model: model.clone(),
        model_name: model,
        reasoning_levels: ReasoningLevel::ALL.to_vec(),
        context_window: 0,
        max_tokens: 0,
        provider_tools: Vec::new(),
    })
}

/// Supported reasoning levels for one provider/model route.
pub fn supported_reasoning_levels(
    choices: &[ModelChoice],
    provider: &str,
    model: &str,
) -> Vec<ReasoningLevel> {
    choices
        .iter()
        .find(|choice| choice.provider == provider && choice.model == model)
        .map(|choice| choice.reasoning_levels.clone())
        .unwrap_or_else(|| vec![ReasoningLevel::Auto])
}

fn parse_response<T>(value: Value) -> Result<T, FrontError>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_value(value).map_err(|error| FrontError::Protocol(error.to_string()))
}

#[cfg(test)]
mod tests {
    use alpha_schema::{
        AuthMethodDescriptor, ModelDescriptor, ProviderDescriptor, ProviderInstance, ReasoningLevel,
    };
    use serde_json::json;

    use super::{ProviderState, resolve_model_selection};

    fn descriptor() -> ProviderDescriptor {
        ProviderDescriptor {
            kind: "test".to_owned(),
            name: "Test".to_owned(),
            config_schema: json!({}),
            default_config: json!({}),
            auth_methods: vec![AuthMethodDescriptor {
                key: "api_key".to_owned(),
                label: "API key".to_owned(),
                interactive: true,
            }],
            models: vec![ModelDescriptor {
                id: "model".to_owned(),
                name: "Model".to_owned(),
                adapter: "test".to_owned(),
                context_window: 1_000,
                max_tokens: 100,
                reasoning: true,
                reasoning_levels: ReasoningLevel::ALL.to_vec(),
                provider_tools: Vec::new(),
            }],
            adapters: Vec::new(),
        }
    }

    #[test]
    fn arbitrary_model_ids_resolve_to_the_current_provider() {
        let state = ProviderState {
            kinds: vec![descriptor()],
            instances: vec![ProviderInstance {
                id: "test".to_owned(),
                kind: "test".to_owned(),
                config_version: 1,
                config: json!({}),
                auth_ref: None,
                model_overrides: json!({}),
                disabled: false,
            }],
            ..ProviderState::default()
        };
        let selected = resolve_model_selection(&state.model_choices(), "test", "future-model")
            .expect("selection");
        assert_eq!(selected.provider, "test");
        assert_eq!(selected.model, "future-model");
        assert_eq!(selected.reasoning_levels, ReasoningLevel::ALL.to_vec());
    }
}

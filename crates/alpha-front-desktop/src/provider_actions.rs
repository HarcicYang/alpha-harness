use super::*;

impl AlphaDesktop {
    pub(crate) fn open_providers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.bump_overlay_generation();
        self.reset_scroll(ScrollRegion::ProviderKinds);
        self.reset_scroll(ScrollRegion::Providers);
        match self.runtime.refresh_providers() {
            Ok(()) => {
                let providers = self.runtime.providers();
                self.provider_kind = providers
                    .instances
                    .first()
                    .map(|instance| instance.kind.clone())
                    .or_else(|| providers.kinds.first().map(|kind| kind.kind.clone()));
                self.provider_status = None;
                self.providers_open = true;
                let first_instance = self.provider_kind.as_deref().and_then(|kind| {
                    self.runtime
                        .providers()
                        .instances
                        .iter()
                        .find(|instance| instance.kind == kind)
                        .map(|instance| instance.id.clone())
                });
                if let Some(id) = first_instance {
                    self.select_provider_instance(id, window, cx);
                } else {
                    self.provider_selected = None;
                    self.provider_remove_confirm = None;
                }
            }
            Err(error) => self.local_status = Some(StatusMessage::error(error.to_string())),
        }
        cx.notify();
    }

    pub(crate) fn select_provider_kind(
        &mut self,
        kind: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.provider_kind = Some(kind.clone());
        self.provider_status = None;
        let first_instance = self
            .runtime
            .providers()
            .instances
            .iter()
            .find(|instance| instance.kind == kind)
            .map(|instance| instance.id.clone());
        if let Some(id) = first_instance {
            self.select_provider_instance(id, window, cx);
        } else {
            self.provider_selected = None;
            self.provider_remove_confirm = None;
            self.provider_config
                .update(cx, |state, cx| state.set_value("", window, cx));
        }
        cx.notify();
    }

    pub(crate) fn select_provider_instance(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(instance) = self
            .runtime
            .providers()
            .instances
            .iter()
            .find(|instance| instance.id == id)
            .cloned()
        else {
            return;
        };
        self.provider_selected = Some(id);
        self.provider_remove_confirm = None;
        let config = pretty_provider_config(&instance.config);
        self.provider_config
            .update(cx, |state, cx| state.set_value(&config, window, cx));
        self.provider_credential
            .update(cx, |state, cx| state.set_value("", window, cx));
    }

    pub(crate) fn create_provider_instance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(kind) = self.provider_kind.clone() else {
            return;
        };
        match self.runtime.create_provider_instance(&kind) {
            Ok(id) => {
                self.provider_status = Some(format!("created {id}"));
                let _ = self.runtime.refresh_providers();
                self.select_provider_instance(id, window, cx);
            }
            Err(error) => self.provider_status = Some(error.to_string()),
        }
        cx.notify();
    }

    pub(crate) fn authenticate_selected_provider(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(id), secret) = (
            self.provider_selected.clone(),
            self.provider_credential.read(cx).value().trim().to_owned(),
        ) else {
            return;
        };
        match self.runtime.authenticate_provider(&id, secret) {
            Ok(()) => {
                self.provider_status = Some(format!("credential stored for {id}"));
                self.provider_credential
                    .update(cx, |state, cx| state.set_value("", window, cx));
                let _ = self.runtime.refresh_providers();
            }
            Err(error) => self.provider_status = Some(error.to_string()),
        }
        cx.notify();
    }

    pub(crate) fn save_provider_config(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.provider_selected.clone() else {
            return;
        };
        let Some(mut instance) = self
            .runtime
            .providers()
            .instances
            .iter()
            .find(|instance| instance.id == id)
            .cloned()
        else {
            return;
        };
        let text = self.provider_config.read(cx).value();
        match parse_provider_config(&text) {
            Ok(config) => {
                instance.config = config;
                match self.runtime.update_provider(instance) {
                    Ok(()) => self.provider_status = Some(format!("configuration saved for {id}")),
                    Err(error) => self.provider_status = Some(error.to_string()),
                }
            }
            Err(error) => self.provider_status = Some(error),
        }
        cx.notify();
    }

    pub(crate) fn remove_selected_provider(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.provider_selected.clone() else {
            return;
        };
        if self.provider_remove_confirm.as_deref() != Some(id.as_str()) {
            self.provider_remove_confirm = Some(id);
            cx.notify();
            return;
        }
        match self.runtime.remove_provider_instance(&id) {
            Ok(removed) => {
                self.provider_status = Some(if removed.credential_removed {
                    format!("removed {id}")
                } else {
                    format!(
                        "removed {id}; credential cleanup failed: {}",
                        removed
                            .credential_error
                            .unwrap_or_else(|| "unknown error".to_owned())
                    )
                });
                self.provider_selected = None;
                self.provider_remove_confirm = None;
                let _ = self.runtime.refresh_providers();
                let first_instance = self.provider_kind.as_deref().and_then(|kind| {
                    self.runtime
                        .providers()
                        .instances
                        .iter()
                        .find(|instance| instance.kind == kind)
                        .map(|instance| instance.id.clone())
                });
                if let Some(id) = first_instance {
                    self.select_provider_instance(id, window, cx);
                }
            }
            Err(error) => self.provider_status = Some(error.to_string()),
        }
        cx.notify();
    }

    pub(crate) fn test_provider_instance(&mut self, id: String, cx: &mut Context<Self>) {
        match self.runtime.test_provider(&id) {
            Ok(()) => self.provider_status = Some(format!("connection passed: {id}")),
            Err(error) => self.provider_status = Some(error.to_string()),
        }
        cx.notify();
    }

    pub(crate) fn set_default_provider_instance(&mut self, id: String, cx: &mut Context<Self>) {
        match self.runtime.set_default_provider(&id) {
            Ok(()) => self.provider_status = Some(format!("default provider: {id}")),
            Err(error) => self.provider_status = Some(error.to_string()),
        }
        cx.notify();
    }
}

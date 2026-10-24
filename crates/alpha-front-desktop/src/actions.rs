use super::*;
use crate::helpers::*;
use crate::model::*;

impl AlphaDesktop {
    pub(crate) fn new(runtime: FrontRuntime, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Ask alpha to inspect, change, or explain...")
                .auto_grow(1, 6)
                .submit_on_enter(true)
        });
        let composer_subscription = cx.subscribe(&composer, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.submit_requested = true;
            }
            cx.notify();
        });
        let session_search = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Search sessions")
                .auto_grow(1, 1)
        });
        let session_search_subscription = cx.subscribe(&session_search, |_, _, _, cx| {
            cx.notify();
        });
        let provider_credential = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("API key or access token")
                .auto_grow(1, 2)
        });
        let provider_config = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("{}")
                .auto_grow(4, 10)
        });
        let model_choices = runtime.providers().model_choices();
        let mut this = Self {
            runtime,
            composer,
            model_choices,
            composer_menu: None,
            submit_requested: false,
            context_open: false,
            context_tab: ContextTab::Changes,
            change_review_open: false,
            selected_change: 0,
            thinking_expanded: false,
            expanded_tool_group: None,
            sessions_open: false,
            session_search,
            session_selected: None,
            session_delete_confirm: None,
            session_status: None,
            sessions: Vec::new(),
            providers_open: false,
            provider_kind: None,
            provider_selected: None,
            provider_remove_confirm: None,
            provider_status: None,
            provider_credential,
            provider_config,
            transcript_scroll: ScrollHandle::new(),
            jump_to_latest_hidden_until: None,
            more_open: false,
            help_open: false,
            local_status: None,
            _poll_task: None,
            _composer_subscription: composer_subscription,
            _session_search_subscription: session_search_subscription,
        };
        this._poll_task = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
                if this
                    .update_in(cx, |this, window, cx| {
                        if this
                            .jump_to_latest_hidden_until
                            .is_some_and(|deadline| Instant::now() >= deadline)
                        {
                            this.jump_to_latest_hidden_until = None;
                            cx.notify();
                        }
                        if this.submit_requested {
                            this.submit_requested = false;
                            this.submit_composer(window, cx);
                        }
                        let follow_transcript = transcript_is_at_bottom(&this.transcript_scroll);
                        let mut changed = this.runtime.expire_interaction().unwrap_or(false);
                        match this.runtime.try_receive_events() {
                            Ok(events) => changed |= !events.is_empty(),
                            Err(error) => this.local_status = Some(error.to_string()),
                        }
                        if changed {
                            if follow_transcript {
                                this.transcript_scroll.scroll_to_bottom();
                            }
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
        this
    }

    pub(crate) fn open_composer_menu(&mut self, menu: ComposerMenu) {
        self.composer_menu = if self.composer_menu == Some(menu) {
            None
        } else {
            if menu == ComposerMenu::Model
                && let Err(error) = self.runtime.refresh_providers()
            {
                self.local_status = Some(error.to_string());
            }
            let provider = self.runtime.chat().provider.clone();
            if menu == ComposerMenu::Model
                && let Err(error) = self.runtime.refresh_models(Some(&provider))
            {
                self.local_status = Some(error.to_string());
            }
            self.model_choices = self.runtime.providers().model_choices();
            Some(menu)
        };
    }

    pub(crate) fn select_mode(&mut self, mode: PermissionMode, cx: &mut Context<Self>) {
        match self.runtime.set_mode(mode) {
            Ok(()) => {
                self.composer_menu = None;
                self.local_status = None;
            }
            Err(error) => self.local_status = Some(error.to_string()),
        }
        cx.notify();
    }

    pub(crate) fn select_model(&mut self, choice: &ModelChoice, cx: &mut Context<Self>) {
        let reasoning = if choice
            .reasoning_levels
            .contains(&self.runtime.chat().reasoning)
        {
            self.runtime.chat().reasoning
        } else {
            choice
                .reasoning_levels
                .first()
                .copied()
                .unwrap_or(ReasoningLevel::Auto)
        };
        match self.runtime.set_route_and_default(
            choice.provider.clone(),
            choice.model.clone(),
            reasoning,
        ) {
            Ok(()) => {
                self.composer_menu = None;
                self.local_status = None;
            }
            Err(error) => self.local_status = Some(error.to_string()),
        }
        cx.notify();
    }

    pub(crate) fn select_effort(&mut self, reasoning: ReasoningLevel, cx: &mut Context<Self>) {
        let provider = self.runtime.chat().provider.clone();
        let model = self.runtime.chat().model.clone();
        match self.runtime.set_route(provider, model, reasoning) {
            Ok(()) => {
                self.composer_menu = None;
                self.local_status = None;
            }
            Err(error) => self.local_status = Some(error.to_string()),
        }
        cx.notify();
    }

    pub(crate) fn open_sessions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.runtime.list_sessions() {
            Ok(sessions) => {
                let active = self.runtime.chat().session_id;
                self.session_selected = active
                    .filter(|id| sessions.iter().any(|session| session.id == *id))
                    .or_else(|| sessions.first().map(|session| session.id));
                self.sessions = sessions;
                self.session_delete_confirm = None;
                self.session_status = None;
                self.session_search
                    .update(cx, |state, cx| state.set_value("", window, cx));
                self.sessions_open = true;
                self.local_status = None;
            }
            Err(error) => self.local_status = Some(error.to_string()),
        }
        cx.notify();
    }

    pub(crate) fn resume_session(&mut self, session_id: SessionId, cx: &mut Context<Self>) {
        match self.runtime.resume_session(session_id) {
            Ok(_) => {
                self.sessions_open = false;
                self.session_delete_confirm = None;
                self.session_status = None;
                self.change_review_open = false;
                self.selected_change = 0;
                self.context_tab = ContextTab::Changes;
                self.thinking_expanded = false;
                self.expanded_tool_group = None;
                self.jump_to_latest_hidden_until = None;
                self.transcript_scroll.scroll_to_bottom();
                self.local_status = None;
            }
            Err(error) => self.local_status = Some(error.to_string()),
        }
        cx.notify();
    }

    pub(crate) fn new_session(&mut self, cx: &mut Context<Self>) {
        self.runtime.new_session();
        self.sessions_open = false;
        self.session_delete_confirm = None;
        self.session_status = None;
        self.composer_menu = None;
        self.change_review_open = false;
        self.selected_change = 0;
        self.context_tab = ContextTab::Changes;
        self.thinking_expanded = false;
        self.expanded_tool_group = None;
        self.jump_to_latest_hidden_until = None;
        self.transcript_scroll.scroll_to_bottom();
        self.local_status = None;
        cx.notify();
    }

    pub(crate) fn delete_selected_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session_id) = self.session_selected else {
            return;
        };
        if self.session_delete_confirm != Some(session_id) {
            self.session_delete_confirm = Some(session_id);
            self.session_status = Some("delete confirmation required".to_owned());
            cx.notify();
            return;
        }
        match self.runtime.delete_session(session_id) {
            Ok(()) => {
                self.session_status = Some(format!("deleted {}", short_id(session_id)));
                match self.runtime.list_sessions() {
                    Ok(sessions) => {
                        self.sessions = sessions;
                        self.session_selected = self.sessions.first().map(|session| session.id);
                    }
                    Err(error) => self.session_status = Some(error.to_string()),
                }
                self.session_delete_confirm = None;
                self.session_search
                    .update(cx, |state, cx| state.set_value("", window, cx));
            }
            Err(error) => self.session_status = Some(error.to_string()),
        }
        cx.notify();
    }

    pub(crate) fn open_providers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.more_open = false;
        match self.runtime.refresh_providers() {
            Ok(()) => {
                self.provider_kind = self
                    .runtime
                    .providers()
                    .kinds
                    .first()
                    .map(|kind| kind.kind.clone());
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
            Err(error) => self.local_status = Some(error.to_string()),
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

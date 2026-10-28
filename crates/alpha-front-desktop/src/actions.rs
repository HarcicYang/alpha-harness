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
        let composer_subscription =
            cx.subscribe(&composer, |this, composer, event: &InputEvent, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.submit_requested = true;
                }
                if matches!(event, InputEvent::Change) {
                    let value = composer.read(cx).value();
                    let slash_open = value
                        .trim_start()
                        .strip_prefix('/')
                        .is_some_and(|query| !query.contains(char::is_whitespace));
                    if slash_open && !this.slash_palette_open {
                        this.bump_overlay_generation();
                        this.reset_scroll(ScrollRegion::SlashCommands);
                    }
                    this.slash_palette_open = slash_open;
                }
                cx.notify();
            });
        let session_search = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Search sessions")
                .auto_grow(1, 1)
        });
        let session_search_subscription = cx.subscribe(&session_search, |this, search, _, cx| {
            let query = search.read(cx).value().to_lowercase();
            let query = query.trim();
            let selected_visible = this.session_selected.is_some_and(|selected| {
                this.sessions.iter().any(|session| {
                    session.id == selected
                        && (query.is_empty() || session_matches_query(session, query))
                })
            });
            if !selected_visible {
                this.session_selected = this
                    .sessions
                    .iter()
                    .find(|session| query.is_empty() || session_matches_query(session, query))
                    .map(|session| session.id);
            }
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
        let capabilities_text = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("shell, read, edit, web_search")
                .auto_grow(2, 5)
        });
        let capabilities_subscription = cx.subscribe(&capabilities_text, |_, _, _, cx| {
            cx.notify();
        });
        let model_search = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Search models")
                .auto_grow(1, 1)
        });
        let model_search_subscription = cx.subscribe(&model_search, |_, _, _, cx| {
            cx.notify();
        });
        let command_palette_query = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Search commands and actions")
                .auto_grow(1, 1)
                .submit_on_enter(true)
        });
        let command_palette_subscription = cx.subscribe(&command_palette_query, |_, _, _, cx| {
            cx.notify();
        });
        let focus_handle = cx.focus_handle();
        let model_choices = runtime.providers().model_choices();
        let mut this = Self {
            runtime,
            focus_handle,
            composer,
            model_search,
            model_choices,
            composer_menu: None,
            command_palette_query,
            command_palette_open: false,
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
            capabilities_open: false,
            capabilities_text,
            overlay_generation: 0,
            slash_palette_open: false,
            transcript_epoch: 0,
            context_tab_generation: 0,
            model_scroll: ScrollHandle::new(),
            session_scroll: ScrollHandle::new(),
            provider_kind_scroll: ScrollHandle::new(),
            provider_scroll: ScrollHandle::new(),
            context_scroll: ScrollHandle::new(),
            command_scroll: ScrollHandle::new(),
            slash_scroll: ScrollHandle::new(),
            change_file_scroll: ScrollHandle::new(),
            change_diff_scroll: ScrollHandle::new(),
            tool_scroll: ScrollHandle::new(),
            composer_menu_scroll: ScrollHandle::new(),
            transcript_scroll: ScrollHandle::new(),
            scroll_motion: std::array::from_fn(|_| SmoothScroll::default()),
            transcript_items: Arc::new(Vec::new()),
            transcript_dirty: true,
            jump_to_latest_hidden_until: None,
            help_open: false,
            local_status: None,
            _poll_task: None,
            _composer_subscription: composer_subscription,
            _model_search_subscription: model_search_subscription,
            _command_palette_subscription: command_palette_subscription,
            _session_search_subscription: session_search_subscription,
            _capabilities_subscription: capabilities_subscription,
        };
        let composer_focus = this.composer.read(cx).focus_handle(cx);
        window.focus(&composer_focus, cx);
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
                        this.transcript_dirty |= changed;
                        if this.runtime.chat().interaction.is_some() {
                            cx.notify();
                        }
                        if changed {
                            if follow_transcript {
                                this.scroll_to_bottom(ScrollRegion::Transcript);
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

    pub(crate) fn focus_composer(&self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self.composer.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
    }

    pub(crate) fn bump_overlay_generation(&mut self) {
        self.overlay_generation = self.overlay_generation.wrapping_add(1).max(1);
    }

    pub(crate) fn toggle_context(&mut self) {
        if self.context_open {
            self.context_open = false;
            self.change_review_open = false;
        } else {
            self.bump_overlay_generation();
            self.reset_scroll(ScrollRegion::Context);
            self.context_open = true;
        }
    }

    pub(crate) fn open_change_review(&mut self, index: usize) {
        if !self.change_review_open {
            self.bump_overlay_generation();
        }
        self.reset_scroll(ScrollRegion::ChangeFiles);
        self.reset_scroll(ScrollRegion::ChangeDiff);
        self.selected_change = index;
        self.change_review_open = true;
    }

    pub(crate) fn open_composer_menu(
        &mut self,
        menu: ComposerMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.composer_menu = if self.composer_menu == Some(menu) {
            None
        } else {
            self.bump_overlay_generation();
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
            self.reset_scroll(ScrollRegion::ComposerMenu);
            if menu == ComposerMenu::Model {
                self.reset_scroll(ScrollRegion::ModelPicker);
                self.model_search
                    .update(cx, |state, cx| state.set_value("", window, cx));
                let focus = self.model_search.read(cx).focus_handle(cx);
                window.focus(&focus, cx);
            }
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
}

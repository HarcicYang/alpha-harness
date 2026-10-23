//! GPUI desktop frontend for alpha-harness.

#![forbid(unsafe_code)]

use std::time::{Duration, Instant};

use alpha_front_runtime::{
    ActivityState, CommandOutcome, ContextView, FileChange, FileChangeKind, FrontRuntime,
    InteractionDecision, ModelChoice, TranscriptItem, parse_command, supported_reasoning_levels,
};
use alpha_schema::{
    AssistantContentBlock, CredentialBackendStatus, PermissionMode, ReasoningLevel, SessionId,
    SessionListEntry, ToolCallId, ToolStatus,
};
use gpui_kit::base::Selectable as _;
use gpui_kit::component::{
    Theme, ThemeMode,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    input::{InputEvent, Textarea, TextareaState},
    scroll::ScrollableElement as _,
    text::markdown,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

const CANVAS: u32 = 0x08090a;
const SURFACE: u32 = 0x101215;
const SURFACE_RAISED: u32 = 0x14171b;
const BORDER: u32 = 0x24282e;
const TEXT: u32 = 0xf0f2f4;
const TEXT_SECONDARY: u32 = 0xa7adb5;
const MUTED: u32 = 0x707780;
const ACCENT: u32 = 0x5cc8e6;

/// Desktop frontend startup failure.
#[derive(Debug, thiserror::Error)]
pub enum DesktopError {
    /// The runtime could not enter the desktop event loop.
    #[error("{0}")]
    Startup(String),
}

struct AlphaDesktop {
    runtime: FrontRuntime,
    composer: Entity<TextareaState>,
    model_choices: Vec<ModelChoice>,
    composer_menu: Option<ComposerMenu>,
    submit_requested: bool,
    context_open: bool,
    context_tab: ContextTab,
    change_review_open: bool,
    selected_change: usize,
    thinking_expanded: bool,
    expanded_tool_group: Option<String>,
    sessions_open: bool,
    session_search: Entity<TextareaState>,
    session_selected: Option<SessionId>,
    session_delete_confirm: Option<SessionId>,
    session_status: Option<String>,
    sessions: Vec<SessionListEntry>,
    providers_open: bool,
    provider_kind: Option<String>,
    provider_selected: Option<String>,
    provider_remove_confirm: Option<String>,
    provider_status: Option<String>,
    provider_credential: Entity<TextareaState>,
    provider_config: Entity<TextareaState>,
    transcript_scroll: ScrollHandle,
    jump_to_latest_hidden_until: Option<Instant>,
    more_open: bool,
    help_open: bool,
    local_status: Option<String>,
    _poll_task: Option<Task<()>>,
    _composer_subscription: Subscription,
    _session_search_subscription: Subscription,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ComposerMenu {
    Mode,
    Model,
    Effort,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ContextTab {
    Changes,
    Activity,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum DesktopTranscriptItem {
    User(String),
    Assistant(String),
    Thinking(String),
    Tools { id: String, tools: Vec<ToolSummary> },
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ToolSummary {
    id: ToolCallId,
    name: String,
    status: Option<ToolStatus>,
    content: Option<String>,
}

impl AlphaDesktop {
    fn new(runtime: FrontRuntime, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Ask alpha to inspect, change, or explain...")
                .auto_grow(1, 6)
                .submit_on_enter(true)
        });
        let composer_subscription = cx.subscribe(&composer, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.submit_requested = true;
                cx.notify();
            }
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

    fn open_composer_menu(&mut self, menu: ComposerMenu) {
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

    fn select_mode(&mut self, mode: PermissionMode, cx: &mut Context<Self>) {
        match self.runtime.set_mode(mode) {
            Ok(()) => {
                self.composer_menu = None;
                self.local_status = None;
            }
            Err(error) => self.local_status = Some(error.to_string()),
        }
        cx.notify();
    }

    fn select_model(&mut self, choice: &ModelChoice, cx: &mut Context<Self>) {
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

    fn select_effort(&mut self, reasoning: ReasoningLevel, cx: &mut Context<Self>) {
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

    fn open_sessions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    fn resume_session(&mut self, session_id: SessionId, cx: &mut Context<Self>) {
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

    fn new_session(&mut self, cx: &mut Context<Self>) {
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

    fn delete_selected_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    fn open_providers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    fn select_provider_kind(&mut self, kind: String, window: &mut Window, cx: &mut Context<Self>) {
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

    fn select_provider_instance(
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

    fn create_provider_instance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    fn authenticate_selected_provider(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    fn save_provider_config(&mut self, cx: &mut Context<Self>) {
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

    fn remove_selected_provider(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    fn test_provider_instance(&mut self, id: String, cx: &mut Context<Self>) {
        match self.runtime.test_provider(&id) {
            Ok(()) => self.provider_status = Some(format!("connection passed: {id}")),
            Err(error) => self.provider_status = Some(error.to_string()),
        }
        cx.notify();
    }

    fn set_default_provider_instance(&mut self, id: String, cx: &mut Context<Self>) {
        match self.runtime.set_default_provider(&id) {
            Ok(()) => self.provider_status = Some(format!("default provider: {id}")),
            Err(error) => self.provider_status = Some(error.to_string()),
        }
        cx.notify();
    }

    fn render_more_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        if !self.more_open {
            return div().into_any_element();
        }
        div()
            .absolute()
            .top(px(50.))
            .right(px(16.))
            .w(px(224.))
            .rounded(px(20.))
            .border_1()
            .border_color(rgb(BORDER))
            .bg(rgb(SURFACE_RAISED))
            .p(px(5.))
            .flex()
            .flex_col()
            .gap_1()
            .child(
                Button::new("more-providers")
                    .label("Provider settings")
                    .ghost()
                    .w_full()
                    .rounded(px(15.))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_providers(window, cx);
                    })),
            )
            .child(
                Button::new("more-new-session")
                    .label("New session")
                    .ghost()
                    .w_full()
                    .rounded(px(15.))
                    .on_click(cx.listener(|this, _, _, cx| this.new_session(cx))),
            )
            .child(
                Button::new("more-diagnostics")
                    .label("Core diagnostics")
                    .ghost()
                    .w_full()
                    .rounded(px(15.))
                    .on_click(cx.listener(|this, _, _, cx| {
                        let info = this.runtime.core_info();
                        this.local_status = Some(format!(
                            "core pid={} protocol={} instance={}",
                            info.pid, info.protocol, info.core_instance_id
                        ));
                        this.more_open = false;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("more-help")
                    .label("Keyboard shortcuts")
                    .ghost()
                    .w_full()
                    .rounded(px(15.))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.more_open = false;
                        this.help_open = true;
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    fn render_help(&self, cx: &mut Context<Self>) -> AnyElement {
        if !self.help_open {
            return div().into_any_element();
        }
        div()
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .child(div().absolute().inset_0().bg(rgba(0x00000099)))
            .child(
                div()
                    .relative()
                    .mt(px(90.))
                    .w(px(560.))
                    .rounded(px(24.))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(SURFACE_RAISED))
                    .shadow_lg()
                    .p(px(18.))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Keyboard shortcuts"),
                            )
                            .child(
                                Button::new("close-help")
                                    .label("×")
                                    .ghost()
                                    .rounded(px(999.))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.help_open = false;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(help_row("Send message", "Enter"))
                    .child(help_row("Insert newline", "Shift + Enter"))
                    .child(help_row("Model route", "Composer model pill"))
                    .child(help_row("Reasoning effort", "Composer effort pill"))
                    .child(help_row("Permission mode", "Composer mode pill"))
                    .child(help_row("Sessions", "Sessions button"))
                    .child(help_row("Commands", "/command in composer")),
            )
            .into_any_element()
    }

    fn render_context_panel(&self, context: &ContextView, cx: &mut Context<Self>) -> AnyElement {
        if !self.context_open {
            return div().into_any_element();
        }

        let percent_value = if context.context_window == 0 {
            0.0
        } else {
            (context.input_tokens as f64 / context.context_window as f64 * 100.0).min(100.0)
        };
        let percent_label = if percent_value > 0.0 && percent_value < 1.0 {
            "<1%".to_owned()
        } else {
            format!("{percent_value:.0}%")
        };
        let meter_width = if percent_value == 0.0 {
            0.0
        } else {
            (304.0 * percent_value as f32 / 100.0).max(6.0)
        };
        let changes_count = context.changes.len();
        let body = match self.context_tab {
            ContextTab::Changes => {
                let rows =
                    if context.changes.is_empty() {
                        vec![
                            div()
                                .py(px(26.))
                                .text_center()
                                .text_size(px(10.))
                                .text_color(rgb(MUTED))
                                .child("No file changes in this session")
                                .into_any_element(),
                        ]
                    } else {
                        context
                            .changes
                            .iter()
                            .enumerate()
                            .take(6)
                            .map(|(index, change)| {
                                let (name, directory) = path_parts(&change.path);
                                Button::new(format!("context-change:{index}"))
                                    .child(
                                        div()
                                            .w_full()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .size(px(24.))
                                                    .flex_none()
                                                    .rounded_full()
                                                    .bg(rgb(0x102e37))
                                                    .text_center()
                                                    .text_size(px(9.))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(rgb(ACCENT))
                                                    .child(match change.kind {
                                                        FileChangeKind::Added => "A",
                                                        FileChangeKind::Modified => "M",
                                                    }),
                                            )
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w(px(0.))
                                                    .flex()
                                                    .flex_col()
                                                    .items_start()
                                                    .child(
                                                        div()
                                                            .max_w_full()
                                                            .truncate()
                                                            .text_size(px(10.))
                                                            .font_weight(FontWeight::MEDIUM)
                                                            .text_color(rgb(TEXT))
                                                            .child(name),
                                                    )
                                                    .child(
                                                        div()
                                                            .max_w_full()
                                                            .truncate()
                                                            .text_size(px(8.))
                                                            .text_color(rgb(MUTED))
                                                            .child(directory),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .gap_1()
                                                    .font_family("monospace")
                                                    .text_size(px(8.))
                                                    .child(
                                                        div().text_color(rgb(0x72d69a)).child(
                                                            format!("+{}", change.added_lines),
                                                        ),
                                                    )
                                                    .child(div().text_color(rgb(0xff7888)).child(
                                                        format!("-{}", change.removed_lines),
                                                    )),
                                            ),
                                    )
                                    .ghost()
                                    .w_full()
                                    .rounded(px(16.))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.selected_change = index;
                                        this.change_review_open = true;
                                        cx.notify();
                                    }))
                                    .into_any_element()
                            })
                            .collect()
                    };
                let added = context
                    .changes
                    .iter()
                    .map(|change| change.added_lines)
                    .sum::<usize>();
                let removed = context
                    .changes
                    .iter()
                    .map(|change| change.removed_lines)
                    .sum::<usize>();
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(9.))
                                    .text_color(rgb(MUTED))
                                    .child("Working tree"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_1()
                                    .font_family("monospace")
                                    .text_size(px(9.))
                                    .child(
                                        div().text_color(rgb(0x72d69a)).child(format!("+{added}")),
                                    )
                                    .child(
                                        div()
                                            .text_color(rgb(0xff7888))
                                            .child(format!("-{removed}")),
                                    ),
                            ),
                    )
                    .children(rows)
                    .when(!context.changes.is_empty(), |this| {
                        this.child(
                            Button::new("review-all-changes")
                                .label("Review all changes")
                                .secondary()
                                .w_full()
                                .rounded(px(999.))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.selected_change = 0;
                                    this.change_review_open = true;
                                    cx.notify();
                                })),
                        )
                    })
                    .into_any_element()
            }
            ContextTab::Activity => {
                let rows = if context.activity.is_empty() {
                    vec![
                        div()
                            .py(px(26.))
                            .text_center()
                            .text_size(px(10.))
                            .text_color(rgb(MUTED))
                            .child("No activity yet")
                            .into_any_element(),
                    ]
                } else {
                    context
                        .activity
                        .iter()
                        .rev()
                        .take(8)
                        .map(|activity| {
                            let color = match activity.state {
                                ActivityState::Running => ACCENT,
                                ActivityState::Completed => 0x72d69a,
                                ActivityState::Failed => 0xff7888,
                            };
                            div()
                                .flex()
                                .items_start()
                                .gap_2()
                                .py(px(4.))
                                .child(
                                    div()
                                        .mt(px(5.))
                                        .size(px(6.))
                                        .flex_none()
                                        .rounded_full()
                                        .bg(rgb(color)),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.))
                                        .flex()
                                        .flex_col()
                                        .child(
                                            div()
                                                .text_size(px(10.))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(rgb(TEXT_SECONDARY))
                                                .child(activity.title.clone()),
                                        )
                                        .child(
                                            div()
                                                .max_w_full()
                                                .truncate()
                                                .text_size(px(9.))
                                                .text_color(rgb(MUTED))
                                                .child(activity.detail.clone()),
                                        ),
                                )
                                .into_any_element()
                        })
                        .collect()
                };
                div().flex().flex_col().children(rows).into_any_element()
            }
        };

        let capabilities = context_capabilities(context);
        let capability_chips = if capabilities.is_empty() {
            div()
                .text_size(px(9.))
                .text_color(rgb(MUTED))
                .child("Reported after the first model request")
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_wrap()
                .gap_1()
                .children(capabilities.into_iter().map(|capability| {
                    div()
                        .rounded(px(999.))
                        .border_1()
                        .border_color(rgb(BORDER))
                        .bg(rgb(SURFACE))
                        .px(px(8.))
                        .py(px(3.))
                        .font_family("monospace")
                        .text_size(px(8.))
                        .text_color(rgb(TEXT_SECONDARY))
                        .child(capability)
                        .into_any_element()
                }))
                .into_any_element()
        };

        div()
            .absolute()
            .top(px(58.))
            .right(px(18.))
            .w(px(336.))
            .h(px(560.))
            .rounded(px(28.))
            .border_1()
            .border_color(rgb(BORDER))
            .bg(rgb(SURFACE_RAISED))
            .shadow_lg()
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(58.))
                    .flex_none()
                    .px(px(18.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Context"),
                            )
                            .child(
                                div()
                                    .text_size(px(9.))
                                    .text_color(rgb(MUTED))
                                    .child("Session working set"),
                            ),
                    )
                    .child(
                        Button::new("close-context")
                            .label("×")
                            .ghost()
                            .rounded(px(999.))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.context_open = false;
                                this.change_review_open = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .px(px(16.))
                    .pb(px(12.))
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(9.))
                                    .text_color(rgb(MUTED))
                                    .child("Context window"),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(ACCENT))
                                    .child(percent_label),
                            ),
                    )
                    .child(
                        div()
                            .h(px(5.))
                            .w_full()
                            .rounded(px(999.))
                            .bg(rgb(SURFACE))
                            .overflow_hidden()
                            .child(
                                div()
                                    .h_full()
                                    .w(px(meter_width))
                                    .rounded(px(999.))
                                    .bg(rgb(ACCENT)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .text_size(px(8.))
                            .text_color(rgb(MUTED))
                            .child(format!(
                                "{} input",
                                format_token_count(context.input_tokens)
                            ))
                            .child(format!(
                                "{} output",
                                format_token_count(context.output_tokens)
                            ))
                            .child(if context.context_window == 0 {
                                "limit unknown".to_owned()
                            } else {
                                format!("{} limit", format_token_count(context.context_window))
                            }),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .px(px(16.))
                    .py(px(10.))
                    .flex()
                    .gap_1()
                    .child(
                        Button::new("context-tab-changes")
                            .label(if changes_count == 0 {
                                "Changes".to_owned()
                            } else {
                                format!("Changes  {changes_count}")
                            })
                            .selected(self.context_tab == ContextTab::Changes)
                            .ghost()
                            .rounded(px(999.))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.context_tab = ContextTab::Changes;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("context-tab-activity")
                            .label("Activity")
                            .selected(self.context_tab == ContextTab::Activity)
                            .ghost()
                            .rounded(px(999.))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.context_tab = ContextTab::Activity;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .id("context-body")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scrollbar()
                    .px(px(16.))
                    .pb(px(12.))
                    .child(body),
            )
            .child(
                div()
                    .flex_none()
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .px(px(16.))
                    .py(px(12.))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(9.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(MUTED))
                            .child("Capabilities"),
                    )
                    .child(capability_chips),
            )
            .into_any_element()
    }

    fn render_change_review(&self, context: &ContextView, cx: &mut Context<Self>) -> AnyElement {
        if !self.change_review_open || context.changes.is_empty() {
            return div().into_any_element();
        }
        let selected = self.selected_change.min(context.changes.len() - 1);
        let change = &context.changes[selected];
        let rows = context
            .changes
            .iter()
            .enumerate()
            .map(|(index, change)| {
                let (name, directory) = path_parts(&change.path);
                Button::new(format!("review-file:{index}"))
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .size(px(24.))
                                    .flex_none()
                                    .rounded_full()
                                    .bg(rgb(0x102e37))
                                    .text_center()
                                    .text_size(px(9.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(ACCENT))
                                    .child(match change.kind {
                                        FileChangeKind::Added => "A",
                                        FileChangeKind::Modified => "M",
                                    }),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .flex()
                                    .flex_col()
                                    .items_start()
                                    .child(
                                        div()
                                            .max_w_full()
                                            .truncate()
                                            .text_size(px(10.))
                                            .text_color(rgb(TEXT))
                                            .child(name),
                                    )
                                    .child(
                                        div()
                                            .max_w_full()
                                            .truncate()
                                            .text_size(px(8.))
                                            .text_color(rgb(MUTED))
                                            .child(directory),
                                    ),
                            ),
                    )
                    .selected(index == selected)
                    .ghost()
                    .w_full()
                    .rounded(px(16.))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected_change = index;
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let (name, directory) = path_parts(&change.path);

        div()
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .child(div().absolute().inset_0().bg(rgba(0x000000bb)))
            .child(
                div()
                    .relative()
                    .mt(px(56.))
                    .w(px(860.))
                    .h(px(620.))
                    .rounded(px(28.))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(SURFACE_RAISED))
                    .shadow_lg()
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .h(px(60.))
                            .flex_none()
                            .px(px(18.))
                            .flex()
                            .items_center()
                            .justify_between()
                            .border_b_1()
                            .border_color(rgb(BORDER))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .text_size(px(14.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child("Workspace changes"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(9.))
                                            .text_color(rgb(MUTED))
                                            .child("Files inferred from completed session tools"),
                                    ),
                            )
                            .child(
                                Button::new("close-change-review")
                                    .label("×")
                                    .ghost()
                                    .rounded(px(999.))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.change_review_open = false;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h(px(0.))
                            .flex()
                            .child(
                                div()
                                    .w(px(238.))
                                    .flex_none()
                                    .border_r_1()
                                    .border_color(rgb(BORDER))
                                    .bg(rgb(SURFACE))
                                    .p(px(7.))
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .children(rows),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .flex_none()
                                            .px(px(18.))
                                            .py(px(14.))
                                            .border_b_1()
                                            .border_color(rgb(BORDER))
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_col()
                                                    .child(
                                                        div()
                                                            .text_size(px(13.))
                                                            .font_weight(FontWeight::SEMIBOLD)
                                                            .child(name),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_size(px(8.))
                                                            .text_color(rgb(MUTED))
                                                            .child(directory),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .gap_2()
                                                    .font_family("monospace")
                                                    .text_size(px(9.))
                                                    .child(
                                                        div().text_color(rgb(0x72d69a)).child(
                                                            format!("+{}", change.added_lines),
                                                        ),
                                                    )
                                                    .child(div().text_color(rgb(0xff7888)).child(
                                                        format!("-{}", change.removed_lines),
                                                    )),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .id("change-review-diff")
                                            .flex_1()
                                            .min_h(px(0.))
                                            .overflow_y_scrollbar()
                                            .p(px(14.))
                                            .child(render_change_diff(change)),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .h(px(54.))
                            .flex_none()
                            .px(px(16.))
                            .flex()
                            .items_center()
                            .justify_between()
                            .border_t_1()
                            .border_color(rgb(BORDER))
                            .child(
                                div()
                                    .text_size(px(9.))
                                    .text_color(rgb(MUTED))
                                    .child(format!(
                                        "{} files changed · +{} -{}",
                                        context.changes.len(),
                                        context
                                            .changes
                                            .iter()
                                            .map(|change| change.added_lines)
                                            .sum::<usize>(),
                                        context
                                            .changes
                                            .iter()
                                            .map(|change| change.removed_lines)
                                            .sum::<usize>(),
                                    )),
                            )
                            .child(
                                Button::new("done-change-review")
                                    .label("Done")
                                    .custom(accent_button_variant(cx))
                                    .rounded(px(999.))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.change_review_open = false;
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_providers(&self, cx: &mut Context<Self>) -> AnyElement {
        if !self.providers_open {
            return div().into_any_element();
        }
        let providers = self.runtime.providers();
        let selected_kind = self.provider_kind.as_deref();
        let kind = selected_kind.and_then(|kind| providers.kind(kind));
        let selected_instance = self
            .provider_selected
            .as_deref()
            .and_then(|id| providers.instance(id));
        let instances = selected_kind.map_or_else(Vec::new, |kind| {
            providers
                .instances
                .iter()
                .filter(|instance| instance.kind == kind)
                .cloned()
                .collect::<Vec<_>>()
        });

        let kinds = if providers.kinds.is_empty() {
            vec![
                div()
                    .p(px(12.))
                    .text_size(px(11.))
                    .text_color(rgb(MUTED))
                    .child("No provider kinds registered")
                    .into_any_element(),
            ]
        } else {
            providers
                .kinds
                .iter()
                .map(|kind| {
                    let selected = selected_kind == Some(kind.kind.as_str());
                    let kind_id = kind.kind.clone();
                    Button::new(format!("provider-kind:{}", kind.kind))
                        .child(
                            div()
                                .w_full()
                                .flex()
                                .flex_col()
                                .items_start()
                                .child(
                                    div()
                                        .text_size(px(10.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(rgb(TEXT))
                                        .child(kind.name.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(px(8.))
                                        .text_color(rgb(MUTED))
                                        .child(kind.kind.clone()),
                                ),
                        )
                        .selected(selected)
                        .ghost()
                        .w_full()
                        .rounded(px(16.))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.select_provider_kind(kind_id.clone(), window, cx);
                        }))
                        .into_any_element()
                })
                .collect()
        };

        let instance_rows = if instances.is_empty() {
            vec![
                div()
                    .py(px(18.))
                    .text_center()
                    .text_size(px(10.))
                    .text_color(rgb(MUTED))
                    .child("No configured instances")
                    .into_any_element(),
            ]
        } else {
            instances
                .into_iter()
                .map(|instance| {
                    let id = instance.id.clone();
                    let selected = self.provider_selected.as_deref() == Some(id.as_str());
                    let has_credential = instance.auth_ref.is_some();
                    Button::new(format!("provider-instance:{id}"))
                        .child(
                            div()
                                .w_full()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .size(px(24.))
                                        .flex_none()
                                        .rounded_full()
                                        .bg(if instance.disabled {
                                            rgb(0x2a3038)
                                        } else {
                                            rgb(0x12333c)
                                        })
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_size(px(9.))
                                        .text_color(if instance.disabled {
                                            rgb(MUTED)
                                        } else {
                                            rgb(ACCENT)
                                        })
                                        .child(if instance.disabled { "!" } else { "✓" }),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.))
                                        .flex()
                                        .flex_col()
                                        .items_start()
                                        .child(
                                            div()
                                                .max_w_full()
                                                .truncate()
                                                .font_family("monospace")
                                                .text_size(px(9.))
                                                .text_color(rgb(TEXT))
                                                .child(id.clone()),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(8.))
                                                .text_color(if has_credential {
                                                    rgb(0x72d69a)
                                                } else {
                                                    rgb(MUTED)
                                                })
                                                .child(if has_credential {
                                                    "credential ready"
                                                } else {
                                                    "credential required"
                                                }),
                                        ),
                                ),
                        )
                        .selected(selected)
                        .ghost()
                        .w_full()
                        .rounded(px(16.))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.select_provider_instance(id.clone(), window, cx);
                            cx.notify();
                        }))
                        .into_any_element()
                })
                .collect()
        };

        let credential_backend = providers
            .credential_status
            .as_ref()
            .map(|status| {
                format!(
                    "{} backend: {}",
                    status.backend,
                    match status.status {
                        CredentialBackendStatus::Available => "available",
                        CredentialBackendStatus::Unavailable => "unavailable",
                    }
                )
            })
            .unwrap_or_else(|| "credential backend unknown".to_owned());

        let details = if let Some(instance) = selected_instance {
            let selected_id = instance.id.clone();
            let remove_pending =
                self.provider_remove_confirm.as_deref() == Some(selected_id.as_str());
            div()
                .flex()
                .flex_col()
                .gap_4()
                .child(
                    div()
                        .flex()
                        .items_start()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .font_family("monospace")
                                        .text_size(px(14.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(selected_id.clone()),
                                )
                                .child(div().text_size(px(9.)).text_color(rgb(MUTED)).child(
                                    format!(
                                        "{} · config v{} · {}",
                                        instance.kind, instance.config_version, credential_backend,
                                    ),
                                )),
                        )
                        .child(
                            div()
                                .rounded(px(999.))
                                .border_1()
                                .border_color(rgb(BORDER))
                                .bg(rgb(SURFACE))
                                .px(px(10.))
                                .py(px(5.))
                                .text_size(px(8.))
                                .text_color(if instance.disabled {
                                    rgb(0xff7888)
                                } else {
                                    rgb(0x72d69a)
                                })
                                .child(if instance.disabled {
                                    "Disabled"
                                } else {
                                    "Ready"
                                }),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .text_size(px(9.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(MUTED))
                                .child("Credential"),
                        )
                        .child(
                            div()
                                .rounded(px(18.))
                                .border_1()
                                .border_color(rgb(BORDER))
                                .bg(rgb(SURFACE))
                                .p(px(6.))
                                .child(
                                    Textarea::new(&self.provider_credential)
                                        .appearance(false)
                                        .bordered(false)
                                        .h(px(42.))
                                        .aria_label("Provider credential"),
                                ),
                        )
                        .child(
                            div().flex().justify_end().child(
                                Button::new("save-provider-credential")
                                    .label("Store credential")
                                    .secondary()
                                    .rounded(px(999.))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.authenticate_selected_provider(window, cx);
                                    })),
                            ),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_size(px(9.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(rgb(MUTED))
                                        .child("Configuration JSON"),
                                )
                                .child(
                                    Button::new("save-provider-config")
                                        .label("Save config")
                                        .secondary()
                                        .rounded(px(999.))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.save_provider_config(cx);
                                        })),
                                ),
                        )
                        .child(
                            div()
                                .rounded(px(20.))
                                .border_1()
                                .border_color(rgb(BORDER))
                                .bg(rgb(SURFACE))
                                .p(px(8.))
                                .child(
                                    Textarea::new(&self.provider_config)
                                        .appearance(false)
                                        .bordered(false)
                                        .h(px(170.))
                                        .aria_label("Provider configuration JSON"),
                                ),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Button::new("test-selected-provider")
                                .label("Test connection")
                                .ghost()
                                .rounded(px(999.))
                                .on_click(cx.listener({
                                    let id = selected_id.clone();
                                    move |this, _, _, cx| {
                                        this.test_provider_instance(id.clone(), cx);
                                    }
                                })),
                        )
                        .child(
                            Button::new("default-selected-provider")
                                .label("Set default")
                                .ghost()
                                .rounded(px(999.))
                                .on_click(cx.listener({
                                    let id = selected_id.clone();
                                    move |this, _, _, cx| {
                                        this.set_default_provider_instance(id.clone(), cx);
                                    }
                                })),
                        )
                        .child(div().flex_1())
                        .child(
                            Button::new("remove-selected-provider")
                                .label(if remove_pending {
                                    "Confirm remove"
                                } else {
                                    "Remove"
                                })
                                .ghost()
                                .rounded(px(999.))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.remove_selected_provider(window, cx);
                                })),
                        )
                        .when(remove_pending, |this| {
                            this.child(
                                Button::new("cancel-remove-provider")
                                    .label("Cancel")
                                    .secondary()
                                    .rounded(px(999.))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.provider_remove_confirm = None;
                                        cx.notify();
                                    })),
                            )
                        }),
                )
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .py(px(60.))
                .child(
                    div()
                        .size(px(42.))
                        .rounded_full()
                        .bg(rgb(0x102e37))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(16.))
                        .text_color(rgb(ACCENT))
                        .child("+"),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .font_weight(FontWeight::MEDIUM)
                        .child("No provider instance selected"),
                )
                .child(
                    div()
                        .text_size(px(9.))
                        .text_color(rgb(MUTED))
                        .child("Create an instance or select one from the list."),
                )
                .into_any_element()
        };

        div()
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .child(div().absolute().inset_0().bg(rgba(0x00000099)))
            .child(
                div()
                    .relative()
                    .mt(px(48.))
                    .w(px(900.))
                    .h(px(640.))
                    .rounded(px(26.))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(SURFACE_RAISED))
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .child(
                        div()
                            .h(px(58.))
                            .flex_none()
                            .px(px(16.))
                            .flex()
                            .items_center()
                            .justify_between()
                            .border_b_1()
                            .border_color(rgb(BORDER))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .text_size(px(14.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child("Providers"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(9.))
                                            .text_color(rgb(MUTED))
                                            .child(
                                                "Credentials, configuration, and model routes",
                                            ),
                                    ),
                            )
                            .child(
                                Button::new("close-providers")
                                    .label("×")
                                    .ghost()
                                    .rounded(px(999.))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.providers_open = false;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h(px(0.))
                            .flex()
                            .child(
                                div()
                                    .w(px(230.))
                                    .flex_none()
                                    .border_r_1()
                                    .border_color(rgb(BORDER))
                                    .bg(rgb(SURFACE))
                                    .p(px(7.))
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .children(kinds),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .flex_none()
                                            .px(px(18.))
                                            .pt(px(16.))
                                            .pb(px(10.))
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .child(
                                                div()
                                                    .text_size(px(15.))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .child(
                                                        kind.map(|kind| kind.name.clone())
                                                            .unwrap_or_else(|| {
                                                                "Provider".to_owned()
                                                            }),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .gap_1()
                                                    .child(
                                                        Button::new("refresh-providers")
                                                            .label("Refresh")
                                                            .ghost()
                                                            .rounded(px(999.))
                                                            .on_click(cx.listener(
                                                                |this, _, _, cx| {
                                                                    match this
                                                                        .runtime
                                                                        .refresh_providers()
                                                                    {
                                                                        Ok(()) => this
                                                                            .provider_status =
                                                                            Some(
                                                                                "providers refreshed"
                                                                                    .to_owned(),
                                                                            ),
                                                                        Err(error) => {
                                                                            this.provider_status =
                                                                                Some(
                                                                                    error
                                                                                        .to_string(),
                                                                                )
                                                                        }
                                                                    }
                                                                    cx.notify();
                                                                },
                                                            )),
                                                    )
                                                    .child(
                                                        Button::new("create-provider")
                                                            .custom(accent_button_variant(cx))
                                                            .label("New instance")
                                                            .rounded(px(999.))
                                                            .on_click(cx.listener(
                                                                |this, _, window, cx| {
                                                                    this.create_provider_instance(
                                                                        window, cx,
                                                                    );
                                                                },
                                                            )),
                                                    ),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .id("provider-instance-list")
                                            .h(px(126.))
                                            .flex_none()
                                            .overflow_y_scrollbar()
                                            .px(px(14.))
                                            .pb(px(8.))
                                            .flex()
                                            .flex_col()
                                            .gap_1()
                                            .children(instance_rows),
                                    )
                                    .child(
                                        div().h(px(1.)).flex_none().bg(rgb(BORDER)),
                                    )
                                    .child(
                                        div()
                                            .id("provider-details")
                                            .flex_1()
                                            .min_h(px(0.))
                                            .overflow_y_scrollbar()
                                            .p(px(18.))
                                            .child(details),
                                    )
                                    .child(
                                        div()
                                            .h(px(42.))
                                            .flex_none()
                                            .border_t_1()
                                            .border_color(rgb(BORDER))
                                            .px(px(16.))
                                            .flex()
                                            .items_center()
                                            .child(
                                                div()
                                                    .truncate()
                                                    .text_size(px(9.))
                                                    .text_color(if self.provider_status.is_some() {
                                                        rgb(ACCENT)
                                                    } else {
                                                        rgb(MUTED)
                                                    })
                                                    .child(
                                                        self.provider_status.clone().unwrap_or_else(
                                                            || {
                                                                "Select an instance to edit its route."
                                                                    .to_owned()
                                                            },
                                                        ),
                                                    ),
                                            ),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_sessions(&self, cx: &mut Context<Self>) -> AnyElement {
        if !self.sessions_open {
            return div().into_any_element();
        }
        let query = self.session_search.read(cx).value().to_lowercase();
        let filtered = self
            .sessions
            .iter()
            .filter(|session| {
                if query.trim().is_empty() {
                    return true;
                }
                session_matches_query(session, query.trim())
            })
            .collect::<Vec<_>>();
        let selected = filtered
            .iter()
            .copied()
            .find(|session| Some(session.id) == self.session_selected)
            .or_else(|| filtered.first().copied());
        let active_session = self.runtime.chat().session_id;

        let rows = if filtered.is_empty() {
            vec![
                div()
                    .py(px(40.))
                    .text_center()
                    .text_size(px(10.))
                    .text_color(rgb(MUTED))
                    .child(if self.sessions.is_empty() {
                        "No stored sessions"
                    } else {
                        "No matching sessions"
                    })
                    .into_any_element(),
            ]
        } else {
            filtered
                .into_iter()
                .map(|session| {
                    let route = if session.provider.is_empty() && session.model.is_empty() {
                        "default".to_owned()
                    } else {
                        format!("{}/{}", session.provider, session.model)
                    };
                    let preview = session
                        .first_message
                        .as_deref()
                        .map(|text| text.split_whitespace().collect::<Vec<_>>().join(" "))
                        .filter(|text| !text.is_empty())
                        .unwrap_or_else(|| "(empty session)".to_owned());
                    let session_id = session.id;
                    let is_selected = Some(session.id) == selected.map(|session| session.id);
                    let is_active = active_session == Some(session.id);
                    Button::new(format!("session:{}", session.id))
                        .child(
                            div()
                                .w_full()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .size(px(30.))
                                        .flex_none()
                                        .rounded_full()
                                        .bg(if is_selected {
                                            rgb(0x102e37)
                                        } else {
                                            rgb(SURFACE_RAISED)
                                        })
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_size(px(10.))
                                        .text_color(rgb(ACCENT))
                                        .child(">"),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.))
                                        .flex()
                                        .flex_col()
                                        .items_start()
                                        .gap_1()
                                        .child(
                                            div()
                                                .w_full()
                                                .flex()
                                                .items_center()
                                                .gap_1()
                                                .child(
                                                    div()
                                                        .font_family("monospace")
                                                        .text_size(px(10.))
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .text_color(rgb(TEXT))
                                                        .child(short_id(session.id)),
                                                )
                                                .child(
                                                    div()
                                                        .rounded(px(999.))
                                                        .bg(rgb(SURFACE_RAISED))
                                                        .px(px(7.))
                                                        .py(px(2.))
                                                        .text_size(px(8.))
                                                        .text_color(rgb(ACCENT))
                                                        .child(session.mode.to_string()),
                                                )
                                                .when(is_active, |this| {
                                                    this.child(
                                                        div()
                                                            .rounded(px(999.))
                                                            .bg(rgb(0x12333c))
                                                            .px(px(7.))
                                                            .py(px(2.))
                                                            .text_size(px(8.))
                                                            .text_color(rgb(0x72d69a))
                                                            .child("CURRENT"),
                                                    )
                                                }),
                                        )
                                        .child(
                                            div()
                                                .w_full()
                                                .truncate()
                                                .text_size(px(10.))
                                                .text_color(rgb(TEXT_SECONDARY))
                                                .child(preview),
                                        )
                                        .child(
                                            div()
                                                .max_w_full()
                                                .truncate()
                                                .text_size(px(8.))
                                                .font_family("monospace")
                                                .text_color(rgb(MUTED))
                                                .child(route),
                                        ),
                                ),
                        )
                        .selected(is_selected)
                        .ghost()
                        .bg(rgb(SURFACE))
                        .border_1()
                        .border_color(if is_selected {
                            rgb(0x36515a)
                        } else {
                            rgb(BORDER)
                        })
                        .w_full()
                        .h(px(74.))
                        .p(px(8.))
                        .rounded(px(18.))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.session_selected = Some(session_id);
                            this.session_delete_confirm = None;
                            this.session_status = None;
                            cx.notify();
                        }))
                        .into_any_element()
                })
                .collect()
        };

        let preview = if let Some(session) = selected {
            let route = if session.provider.is_empty() && session.model.is_empty() {
                "default".to_owned()
            } else {
                format!("{}/{}", session.provider, session.model)
            };
            let preview_text = session
                .first_message
                .as_deref()
                .map(|text| text.split_whitespace().collect::<Vec<_>>().join(" "))
                .filter(|text| !text.is_empty())
                .unwrap_or_else(|| "(empty session)".to_owned());
            let session_id = session.id;
            let remove_pending = self.session_delete_confirm == Some(session.id);
            let is_active = active_session == Some(session.id);
            div()
                .size_full()
                .flex()
                .flex_col()
                .gap_4()
                .child(
                    div()
                        .flex()
                        .items_start()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .text_size(px(16.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(short_id(session.id)),
                                )
                                .child(
                                    div()
                                        .font_family("monospace")
                                        .text_size(px(8.))
                                        .text_color(rgb(MUTED))
                                        .child(session.id.to_string()),
                                ),
                        )
                        .when(is_active, |this| {
                            this.child(
                                div()
                                    .rounded(px(999.))
                                    .bg(rgb(0x12333c))
                                    .px(px(9.))
                                    .py(px(4.))
                                    .text_size(px(8.))
                                    .text_color(rgb(0x72d69a))
                                    .child("CURRENT SESSION"),
                            )
                        }),
                )
                .child(
                    div()
                        .rounded(px(20.))
                        .bg(rgb(SURFACE))
                        .p(px(14.))
                        .text_size(px(11.))
                        .line_height(px(19.))
                        .text_color(rgb(TEXT_SECONDARY))
                        .child(preview_text),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(session_meta_row(
                            "Workspace",
                            session.workspace.root.clone(),
                        ))
                        .child(session_meta_row("CWD", session.workspace.cwd.clone()))
                        .child(session_meta_row("Route", route))
                        .child(session_meta_row("Mode", session.mode.to_string())),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Button::new("resume-selected-session")
                                .custom(accent_button_variant(cx))
                                .label(if is_active {
                                    "Continue session"
                                } else {
                                    "Resume session"
                                })
                                .rounded(px(999.))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.resume_session(session_id, cx);
                                })),
                        )
                        .child(div().flex_1())
                        .child(
                            Button::new("delete-selected-session")
                                .label(if remove_pending {
                                    "Confirm delete"
                                } else {
                                    "Delete"
                                })
                                .ghost()
                                .rounded(px(999.))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.delete_selected_session(window, cx);
                                })),
                        )
                        .when(remove_pending, |this| {
                            this.child(
                                Button::new("cancel-delete-session")
                                    .label("Cancel")
                                    .secondary()
                                    .rounded(px(999.))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.session_delete_confirm = None;
                                        this.session_status = None;
                                        cx.notify();
                                    })),
                            )
                        }),
                )
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .child(
                    div()
                        .size(px(42.))
                        .rounded_full()
                        .bg(rgb(SURFACE))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(16.))
                        .text_color(rgb(ACCENT))
                        .child("+"),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .font_weight(FontWeight::MEDIUM)
                        .child("No session selected"),
                )
                .child(
                    div()
                        .text_size(px(9.))
                        .text_color(rgb(MUTED))
                        .child("Select a stored session or start a new draft."),
                )
                .into_any_element()
        };

        div()
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .child(div().absolute().inset_0().bg(rgba(0x00000099)))
            .child(
                div()
                    .relative()
                    .mt(px(48.))
                    .w(px(900.))
                    .h(px(610.))
                    .rounded(px(28.))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(SURFACE_RAISED))
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .child(
                        div()
                            .h(px(60.))
                            .flex_none()
                            .px(px(18.))
                            .flex()
                            .items_center()
                            .justify_between()
                            .border_b_1()
                            .border_color(rgb(BORDER))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .text_size(px(14.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child("Sessions"),
                                    )
                                    .child(
                                        div().text_size(px(9.)).text_color(rgb(MUTED)).child(
                                            format!("{} stored sessions", self.sessions.len()),
                                        ),
                                    ),
                            )
                            .child(
                                Button::new("close-sessions")
                                    .label("×")
                                    .ghost()
                                    .rounded(px(999.))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.sessions_open = false;
                                        this.session_delete_confirm = None;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h(px(0.))
                            .flex()
                            .child(
                                div()
                                    .w(px(430.))
                                    .flex_none()
                                    .border_r_1()
                                    .border_color(rgb(BORDER))
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .flex_none()
                                            .p(px(10.))
                                            .border_b_1()
                                            .border_color(rgb(BORDER))
                                            .child(
                                                div()
                                                    .rounded(px(18.))
                                                    .border_1()
                                                    .border_color(rgb(BORDER))
                                                    .bg(rgb(SURFACE))
                                                    .p(px(5.))
                                                    .child(
                                                        Textarea::new(&self.session_search)
                                                            .appearance(false)
                                                            .bordered(false)
                                                            .h(px(38.))
                                                            .aria_label("Search sessions"),
                                                    ),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .id("session-list")
                                            .flex_1()
                                            .min_h(px(0.))
                                            .overflow_y_scrollbar()
                                            .p(px(8.))
                                            .flex()
                                            .flex_col()
                                            .gap_1()
                                            .children(rows),
                                    )
                                    .child(
                                        div()
                                            .h(px(50.))
                                            .flex_none()
                                            .px(px(12.))
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .border_t_1()
                                            .border_color(rgb(BORDER))
                                            .child(
                                                div()
                                                    .truncate()
                                                    .text_size(px(9.))
                                                    .text_color(if self.session_status.is_some() {
                                                        rgb(ACCENT)
                                                    } else {
                                                        rgb(MUTED)
                                                    })
                                                    .child(
                                                        self.session_status.clone().unwrap_or_else(
                                                            || {
                                                                "Search by message, route, or ID"
                                                                    .to_owned()
                                                            },
                                                        ),
                                                    ),
                                            )
                                            .child(
                                                Button::new("new-session")
                                                    .secondary()
                                                    .label("New draft")
                                                    .rounded(px(999.))
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.new_session(cx);
                                                    })),
                                            ),
                                    ),
                            )
                            .child(div().flex_1().min_w(px(0.)).p(px(20.)).child(preview)),
                    ),
            )
            .into_any_element()
    }

    fn render_composer_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(menu) = self.composer_menu else {
            return div().into_any_element();
        };
        let chat = self.runtime.chat();
        let panel = div()
            .when(menu == ComposerMenu::Model, |this| this.w(px(480.)))
            .when(menu != ComposerMenu::Model, |this| this.w(px(320.)))
            .max_h(px(430.))
            .overflow_y_scrollbar()
            .rounded(px(22.))
            .border_1()
            .border_color(rgb(BORDER))
            .bg(rgb(SURFACE_RAISED))
            .p(px(6.))
            .flex()
            .flex_col()
            .gap_1();

        match menu {
            ComposerMenu::Mode => panel
                .child(menu_label("Permission mode"))
                .children(
                    [
                        PermissionMode::Read,
                        PermissionMode::Auto,
                        PermissionMode::Reviewed,
                        PermissionMode::Free,
                    ]
                    .into_iter()
                    .map(|mode| {
                        let selected = chat.mode == mode;
                        let label = match mode {
                            PermissionMode::Read => "Read",
                            PermissionMode::Auto => "Auto",
                            PermissionMode::Reviewed => "Reviewed",
                            PermissionMode::Free => "Free",
                        };
                        Button::new(format!("mode:{label}"))
                            .label(label)
                            .selected(selected)
                            .ghost()
                            .w_full()
                            .rounded(px(16.))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.select_mode(mode, cx);
                            }))
                    }),
                )
                .into_any_element(),
            ComposerMenu::Model => panel
                .child(menu_label("Model route"))
                .children(self.model_choices.iter().map(|choice| {
                    let selected = chat.provider == choice.provider && chat.model == choice.model;
                    let levels = choice
                        .reasoning_levels
                        .iter()
                        .map(|level| level.as_str())
                        .collect::<Vec<_>>()
                        .join(" / ");
                    Button::new(format!("model:{}:{}", choice.provider, choice.model))
                        .child(
                            div()
                                .w_full()
                                .h_full()
                                .flex()
                                .flex_col()
                                .items_start()
                                .justify_center()
                                .gap_1()
                                .child(
                                    div()
                                        .w_full()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .gap_2()
                                        .child(
                                            div()
                                                .min_w(px(0.))
                                                .truncate()
                                                .text_size(px(11.))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(rgb(TEXT))
                                                .child(format!(
                                                    "{}  ·  {}",
                                                    choice.provider_name, choice.model_name
                                                )),
                                        )
                                        .child(
                                            div()
                                                .flex_none()
                                                .text_size(px(8.))
                                                .text_color(rgb(if selected {
                                                    0x72d69a
                                                } else {
                                                    MUTED
                                                }))
                                                .child(if selected { "CURRENT" } else { "" }),
                                        ),
                                )
                                .child(
                                    div()
                                        .w_full()
                                        .truncate()
                                        .text_size(px(9.))
                                        .font_family("monospace")
                                        .text_color(rgb(MUTED))
                                        .child(format!(
                                            "{} · {} ctx · {} out · {}",
                                            choice.model,
                                            if choice.context_window == 0 {
                                                "unknown".to_owned()
                                            } else {
                                                format_token_count(choice.context_window)
                                            },
                                            if choice.max_tokens == 0 {
                                                "unknown".to_owned()
                                            } else {
                                                format_token_count(choice.max_tokens)
                                            },
                                            if levels.is_empty() {
                                                "auto".to_owned()
                                            } else {
                                                levels
                                            },
                                        )),
                                ),
                        )
                        .selected(selected)
                        .ghost()
                        .w_full()
                        .h(px(58.))
                        .p(px(6.))
                        .rounded(px(16.))
                        .on_click(cx.listener({
                            let choice = choice.clone();
                            move |this, _, _, cx| this.select_model(&choice, cx)
                        }))
                }))
                .into_any_element(),
            ComposerMenu::Effort => {
                let levels =
                    supported_reasoning_levels(&self.model_choices, &chat.provider, &chat.model);
                panel
                    .child(menu_label("Reasoning effort"))
                    .children(levels.into_iter().map(|level| {
                        let selected = chat.reasoning == level;
                        Button::new(format!("effort:{}", level.as_str()))
                            .label(level.as_str())
                            .selected(selected)
                            .ghost()
                            .w_full()
                            .rounded(px(16.))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.select_effort(level, cx);
                            }))
                    }))
                    .into_any_element()
            }
        }
    }

    fn submit_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.composer.read(cx).value().trim().to_owned();
        if value.is_empty() {
            if self.runtime.chat().running_turn.is_some()
                && let Err(error) = self.runtime.cancel()
            {
                self.local_status = Some(error.to_string());
            }
            cx.notify();
            return;
        }

        let mut outcome = None;
        let result = if value.starts_with('/') {
            self.runtime
                .execute_command(parse_command(&value))
                .map(|command| {
                    outcome = Some(command);
                })
        } else {
            self.runtime.submit_prompt(&value)
        };

        match result {
            Ok(()) => {
                self.local_status = None;
                self.transcript_scroll.scroll_to_bottom();
                match outcome {
                    Some(CommandOutcome::OpenProviders) => self.open_providers(window, cx),
                    Some(CommandOutcome::OpenModels) => {
                        self.open_composer_menu(ComposerMenu::Model);
                    }
                    Some(CommandOutcome::OpenThinking) => {
                        self.open_composer_menu(ComposerMenu::Effort);
                    }
                    Some(CommandOutcome::OpenSessions) => self.open_sessions(window, cx),
                    Some(CommandOutcome::Help) => self.help_open = true,
                    Some(CommandOutcome::Quit) => cx.quit(),
                    _ => {}
                }
                self.composer
                    .update(cx, |state, cx| state.set_value("", window, cx));
            }
            Err(error) => self.local_status = Some(error.to_string()),
        }
        cx.notify();
    }

    fn render_transcript(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let items = desktop_transcript_items(&self.runtime.chat().transcript)
            .into_iter()
            .map(|item| self.render_desktop_transcript_item(item, cx))
            .collect::<Vec<_>>();

        if items.is_empty() {
            return div()
                .flex()
                .flex_col()
                .items_center()
                .gap_3()
                .pt(px(120.))
                .child(
                    div()
                        .size(px(52.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .bg(rgb(SURFACE_RAISED))
                        .border_1()
                        .border_color(rgb(BORDER))
                        .text_size(px(24.))
                        .text_color(rgb(ACCENT))
                        .child("α"),
                )
                .child(
                    div()
                        .text_size(px(20.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(TEXT))
                        .child("Start with a task"),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(rgb(MUTED))
                        .child("Describe a change or ask alpha to investigate the workspace."),
                )
                .into_any_element();
        }

        div()
            .flex()
            .flex_col()
            .gap_5()
            .children(items)
            .into_any_element()
    }

    fn render_desktop_transcript_item(
        &self,
        item: DesktopTranscriptItem,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match item {
            DesktopTranscriptItem::User(text) => div()
                .w_full()
                .rounded(px(24.))
                .bg(rgb(0x15181c))
                .px(px(18.))
                .py(px(15.))
                .text_size(px(14.))
                .line_height(px(23.))
                .text_color(rgb(TEXT))
                .child(text)
                .into_any_element(),
            DesktopTranscriptItem::Assistant(text) => div()
                .w_full()
                .child(
                    markdown(text)
                        .text_size(px(14.))
                        .line_height(px(24.))
                        .text_color(rgb(TEXT)),
                )
                .into_any_element(),
            DesktopTranscriptItem::Thinking(text) => {
                let expanded = self.thinking_expanded;
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        Button::new("toggle-thinking")
                            .child(
                                div()
                                    .w_full()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .size(px(22.))
                                            .flex_none()
                                            .rounded_full()
                                            .bg(rgb(0x102e37))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .text_size(px(10.))
                                            .text_color(rgb(ACCENT))
                                            .child("*"),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .text_size(px(10.))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(rgb(TEXT_SECONDARY))
                                            .child("Thought"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(9.))
                                            .text_color(rgb(MUTED))
                                            .child(if expanded { "Hide" } else { "Show" }),
                                    ),
                            )
                            .ghost()
                            .rounded(px(16.))
                            .on_click(cx.listener(|this, _, _, cx| {
                                let follow = transcript_is_at_bottom(&this.transcript_scroll);
                                this.thinking_expanded = !this.thinking_expanded;
                                if follow {
                                    this.transcript_scroll.scroll_to_bottom();
                                }
                                cx.notify();
                            })),
                    )
                    .when(expanded, |this| {
                        this.child(
                            div()
                                .rounded(px(20.))
                                .bg(rgb(SURFACE))
                                .p(px(14.))
                                .text_size(px(12.))
                                .line_height(px(21.))
                                .italic()
                                .text_color(rgb(MUTED))
                                .child(text),
                        )
                    })
                    .into_any_element()
            }
            DesktopTranscriptItem::Error(text) => div()
                .w_full()
                .text_size(px(13.))
                .line_height(px(22.))
                .text_color(rgb(0xff7888))
                .child(text)
                .into_any_element(),
            DesktopTranscriptItem::Tools { id, tools } => self.render_tool_group(&id, &tools, cx),
        }
    }

    fn render_tool_group(
        &self,
        id: &str,
        tools: &[ToolSummary],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(latest) = tools.last() else {
            return div().into_any_element();
        };
        let expanded = self.expanded_tool_group.as_deref() == Some(id);
        let detail_rows = tools
            .iter()
            .map(|tool| {
                let status = tool_status_label(tool.status);
                let color = tool_status_color(tool.status);
                div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .py(px(7.))
                    .child(
                        div()
                            .mt(px(4.))
                            .size(px(7.))
                            .flex_none()
                            .rounded_full()
                            .bg(rgb(color)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_2()
                                    .child(
                                        div()
                                            .min_w(px(0.))
                                            .truncate()
                                            .text_size(px(11.))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(rgb(TEXT_SECONDARY))
                                            .child(tool.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .flex_none()
                                            .text_size(px(9.))
                                            .text_color(rgb(color))
                                            .child(status),
                                    ),
                            )
                            .when_some(tool.content.as_deref(), |this, content| {
                                this.child(
                                    div()
                                        .mt(px(2.))
                                        .max_h(px(240.))
                                        .overflow_y_scrollbar()
                                        .text_size(px(10.))
                                        .line_height(px(17.))
                                        .text_color(rgb(MUTED))
                                        .child(tool_detail_content(content)),
                                )
                            }),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let group_id = id.to_owned();

        div()
            .w_full()
            .rounded(px(22.))
            .border_1()
            .border_color(rgb(BORDER))
            .bg(rgb(SURFACE))
            .p(px(4.))
            .flex()
            .flex_col()
            .child(
                Button::new(format!("tool-group:{id}"))
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(9.))
                                    .font_family("monospace")
                                    .text_color(rgb(MUTED))
                                    .child(if tools.len() == 1 {
                                        "1 tool call".to_owned()
                                    } else {
                                        format!("{} tool calls", tools.len())
                                    }),
                            )
                            .child(div().text_size(px(9.)).text_color(rgb(ACCENT)).child(
                                if expanded {
                                    "Hide details"
                                } else {
                                    "View details"
                                },
                            )),
                    )
                    .ghost()
                    .w_full()
                    .rounded(px(16.))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let follow = transcript_is_at_bottom(&this.transcript_scroll);
                        this.expanded_tool_group =
                            if this.expanded_tool_group.as_deref() == Some(group_id.as_str()) {
                                None
                            } else {
                                Some(group_id.clone())
                            };
                        if follow {
                            this.transcript_scroll.scroll_to_bottom();
                        }
                        cx.notify();
                    })),
            )
            .when(!expanded, |this| {
                this.child(
                    div()
                        .rounded(px(18.))
                        .px(px(11.))
                        .py(px(9.))
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .min_w(px(0.))
                                .truncate()
                                .text_size(px(12.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(TEXT_SECONDARY))
                                .child(latest.name.clone()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(10.))
                                .text_color(rgb(tool_status_color(latest.status)))
                                .child(tool_status_label(latest.status)),
                        ),
                )
                .when_some(latest.content.as_deref(), |this, detail| {
                    this.child(
                        div()
                            .px(px(11.))
                            .pb(px(9.))
                            .truncate()
                            .text_size(px(11.))
                            .text_color(rgb(MUTED))
                            .child(compact_tool_content(detail)),
                    )
                })
            })
            .when(expanded, |this| {
                this.child(
                    div()
                        .rounded(px(18.))
                        .bg(rgb(SURFACE_RAISED))
                        .px(px(10.))
                        .py(px(4.))
                        .flex()
                        .flex_col()
                        .children(detail_rows),
                )
            })
            .into_any_element()
    }
}

fn desktop_transcript_items(items: &[TranscriptItem]) -> Vec<DesktopTranscriptItem> {
    let mut rendered = Vec::new();
    let mut tools = Vec::new();

    for item in items {
        match item {
            TranscriptItem::ToolCall {
                id, name, input, ..
            } => tools.push(ToolSummary {
                id: *id,
                name: if input.is_null() {
                    name.clone()
                } else {
                    format!("{name} {}", compact_json(input))
                },
                status: None,
                content: None,
            }),
            TranscriptItem::ToolResult {
                id,
                name,
                status,
                content,
            } => {
                if let Some(tool) = tools.iter_mut().rev().find(|tool| tool.id == *id) {
                    tool.name = name.clone();
                    tool.status = Some(*status);
                    tool.content = Some(content.clone());
                } else {
                    tools.push(ToolSummary {
                        id: *id,
                        name: name.clone(),
                        status: Some(*status),
                        content: Some(content.clone()),
                    });
                }
            }
            other => {
                flush_tools(&mut rendered, &mut tools);
                match other {
                    TranscriptItem::User { text } => {
                        rendered.push(DesktopTranscriptItem::User(text.clone()));
                    }
                    TranscriptItem::QueuedSteer { .. } => continue,
                    TranscriptItem::AssistantDelta { text } => {
                        rendered.push(DesktopTranscriptItem::Assistant(text.clone()));
                    }
                    TranscriptItem::ThinkingDelta { text } => {
                        rendered.push(DesktopTranscriptItem::Thinking(text.clone()));
                    }
                    TranscriptItem::AssistantMessage { blocks } => {
                        let thinking = blocks
                            .iter()
                            .filter_map(AssistantContentBlock::thinking)
                            .map(|(text, _)| text)
                            .collect::<Vec<_>>()
                            .join("\n");
                        if !thinking.is_empty() {
                            rendered.push(DesktopTranscriptItem::Thinking(thinking));
                        }
                        let text = blocks
                            .iter()
                            .filter_map(AssistantContentBlock::text)
                            .collect::<Vec<_>>()
                            .join("\n");
                        if !text.is_empty() {
                            rendered.push(DesktopTranscriptItem::Assistant(text));
                        }
                    }
                    TranscriptItem::Error { message } => {
                        rendered.push(DesktopTranscriptItem::Error(message.clone()));
                    }
                    TranscriptItem::ToolCall { .. } | TranscriptItem::ToolResult { .. } => {}
                }
            }
        }
    }
    flush_tools(&mut rendered, &mut tools);
    rendered
}

fn flush_tools(items: &mut Vec<DesktopTranscriptItem>, tools: &mut Vec<ToolSummary>) {
    if !tools.is_empty() {
        let tools = std::mem::take(tools);
        let id = tools
            .first()
            .map(|tool| tool.id.to_string())
            .unwrap_or_default();
        items.push(DesktopTranscriptItem::Tools { id, tools });
    }
}

fn compact_json(value: &serde_json::Value) -> String {
    let value = serde_json::to_string(value).unwrap_or_else(|_| value.to_string());
    if value.chars().count() <= 80 {
        value
    } else {
        format!("{}...", value.chars().take(77).collect::<String>())
    }
}

fn transcript_is_at_bottom(scroll: &ScrollHandle) -> bool {
    let offset = -scroll.offset().y;
    let maximum = scroll.max_offset().y;
    offset >= maximum - px(4.) && offset <= maximum + px(4.)
}

fn tool_status_label(status: Option<ToolStatus>) -> &'static str {
    status.map_or("running", |status| match status {
        ToolStatus::Completed => "completed",
        ToolStatus::Failed => "failed",
        ToolStatus::Interrupted => "interrupted",
    })
}

fn tool_status_color(status: Option<ToolStatus>) -> u32 {
    status.map_or(ACCENT, |status| match status {
        ToolStatus::Completed => 0x72d69a,
        ToolStatus::Failed => 0xff7888,
        ToolStatus::Interrupted => 0xf1c56b,
    })
}

fn compact_tool_content(content: &str) -> String {
    let compact = content.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= 360 {
        compact
    } else {
        format!("{}...", compact.chars().take(357).collect::<String>())
    }
}

fn tool_detail_content(content: &str) -> String {
    let content = content.trim();
    if content.chars().count() <= 1_200 {
        content.to_owned()
    } else {
        format!("{}...", content.chars().take(1_197).collect::<String>())
    }
}

impl Render for AlphaDesktop {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (model, effort, mode, running, status, interaction, queued_steers, has_transcript) = {
            let chat = self.runtime.chat();
            let model = if chat.model.is_empty() {
                "unset".to_owned()
            } else {
                chat.model.clone()
            };
            let queued_steers = chat
                .transcript
                .iter()
                .filter_map(|item| match item {
                    TranscriptItem::QueuedSteer { text, .. } => Some(text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            (
                model,
                chat.reasoning.to_string(),
                chat.mode.to_string(),
                chat.running_turn.is_some(),
                self.local_status.clone().or_else(|| chat.status.clone()),
                chat.interaction.clone(),
                queued_steers,
                !chat.transcript.is_empty(),
            )
        };
        let context = self.runtime.context();
        let transcript_at_bottom = transcript_is_at_bottom(&self.transcript_scroll);
        let jump_to_latest_hidden = self
            .jump_to_latest_hidden_until
            .is_some_and(|deadline| Instant::now() < deadline);

        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(CANVAS))
            .text_color(rgb(TEXT))
            .child(
                div()
                    .h(px(56.))
                    .flex_none()
                    .px(px(22.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        Button::new("workspace")
                            .label("alpha-harness")
                            .ghost()
                            .rounded(px(999.)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                Button::new("sessions")
                                    .label("Sessions")
                                    .ghost()
                                    .rounded(px(999.))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_sessions(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("context")
                                    .label("Context")
                                    .selected(self.context_open)
                                    .ghost()
                                    .rounded(px(999.))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.context_open = !this.context_open;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("commands")
                                    .label("More")
                                    .ghost()
                                    .rounded(px(999.))
                                    .selected(self.more_open)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.more_open = !this.more_open;
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .when(!queued_steers.is_empty(), |this| {
                this.child(
                    div()
                        .flex_none()
                        .px(px(20.))
                        .when(self.context_open, |this| this.pr(px(354.)))
                        .child(
                            div()
                                .w(px(780.))
                                .max_w_full()
                                .mx_auto()
                                .mb(px(8.))
                                .rounded(px(18.))
                                .border_1()
                                .border_color(rgb(0x5cc8e655))
                                .bg(rgb(0x0e2a31))
                                .px(px(12.))
                                .py(px(8.))
                                .flex()
                                .items_center()
                                .gap_3()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.))
                                        .flex()
                                        .flex_col()
                                        .child(
                                            div()
                                                .text_size(px(9.))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(ACCENT))
                                                .child("Queued steer"),
                                        )
                                        .child(
                                            div()
                                                .truncate()
                                                .text_size(px(10.))
                                                .text_color(rgb(TEXT_SECONDARY))
                                                .child(queued_steers.join("  ·  ")),
                                        ),
                                )
                                .child(
                                    Button::new("discard-steers")
                                        .label("Discard")
                                        .ghost()
                                        .rounded(px(999.))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            let _ = this.runtime.confirm_steers(false);
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    Button::new("run-steers")
                                        .label("Run next")
                                        .secondary()
                                        .rounded(px(999.))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            let _ = this.runtime.confirm_steers(true);
                                            cx.notify();
                                        })),
                                ),
                        ),
                )
            })
            .child(
                div()
                    .id("transcript")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .track_scroll(&self.transcript_scroll)
                    .when(self.context_open, |this| this.pr(px(354.)))
                    .child(
                        div()
                            .w(px(780.))
                            .max_w_full()
                            .mx_auto()
                            .px(px(24.))
                            .py(px(18.))
                            .child(self.render_transcript(cx))
                            .when_some(interaction, |this, interaction| {
                                let prompt = serde_json::to_string_pretty(&interaction.prompt)
                                    .unwrap_or_else(|_| interaction.prompt.to_string());
                                this.child(
                                    div()
                                        .mt_5()
                                        .rounded(px(24.))
                                        .border_1()
                                        .border_color(rgb(0x5cc8e6))
                                        .bg(rgb(SURFACE))
                                        .p(px(16.))
                                        .flex()
                                        .flex_col()
                                        .gap_3()
                                        .child(
                                            div()
                                                .text_size(px(13.))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .child("Permission requested"),
                                        )
                                        .child(
                                            div()
                                                .rounded(px(16.))
                                                .bg(rgb(SURFACE_RAISED))
                                                .p(px(10.))
                                                .font_family("monospace")
                                                .text_size(px(11.))
                                                .text_color(rgb(TEXT_SECONDARY))
                                                .child(prompt),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .justify_end()
                                                .gap_2()
                                                .child(
                                                    Button::new("deny")
                                                        .label("Deny")
                                                        .ghost()
                                                        .rounded(px(999.))
                                                        .on_click(cx.listener(|this, _, _, cx| {
                                                            let _ =
                                                                this.runtime.respond_interaction(
                                                                    InteractionDecision::Deny,
                                                                );
                                                            cx.notify();
                                                        })),
                                                )
                                                .child(
                                                    Button::new("allow-session")
                                                        .label("Allow session")
                                                        .secondary()
                                                        .rounded(px(999.))
                                                        .on_click(cx.listener(|this, _, _, cx| {
                                                            let _ =
                                                                this.runtime.respond_interaction(
                                                                    InteractionDecision::Session,
                                                                );
                                                            cx.notify();
                                                        })),
                                                )
                                                .child(
                                                    Button::new("allow-once")
                                                        .label("Allow once")
                                                        .custom(accent_button_variant(cx))
                                                        .rounded(px(999.))
                                                        .on_click(cx.listener(|this, _, _, cx| {
                                                            let _ =
                                                                this.runtime.respond_interaction(
                                                                    InteractionDecision::Once,
                                                                );
                                                            cx.notify();
                                                        })),
                                                ),
                                        ),
                                )
                            }),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .px(px(20.))
                    .pb(px(22.))
                    .when(self.context_open, |this| this.pr(px(354.)))
                    .child(
                        div()
                            .w(px(780.))
                            .max_w_full()
                            .mx_auto()
                            .rounded(px(28.))
                            .border_1()
                            .border_color(rgb(BORDER))
                            .bg(rgb(SURFACE))
                            .p(px(8.))
                            .child(
                                Textarea::new(&self.composer)
                                    .appearance(false)
                                    .bordered(false)
                                    .h(px(54.))
                                    .aria_label("Message"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_2()
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .child(
                                                Button::new("mode")
                                                    .label(mode)
                                                    .ghost()
                                                    .rounded(px(999.))
                                                    .selected(
                                                        self.composer_menu
                                                            == Some(ComposerMenu::Mode),
                                                    )
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.open_composer_menu(ComposerMenu::Mode);
                                                        cx.notify();
                                                    })),
                                            )
                                            .child(
                                                Button::new("model")
                                                    .label(model)
                                                    .ghost()
                                                    .rounded(px(999.))
                                                    .selected(
                                                        self.composer_menu
                                                            == Some(ComposerMenu::Model),
                                                    )
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.open_composer_menu(
                                                            ComposerMenu::Model,
                                                        );
                                                        cx.notify();
                                                    })),
                                            )
                                            .child(
                                                Button::new("effort")
                                                    .label(effort)
                                                    .ghost()
                                                    .rounded(px(999.))
                                                    .selected(
                                                        self.composer_menu
                                                            == Some(ComposerMenu::Effort),
                                                    )
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.open_composer_menu(
                                                            ComposerMenu::Effort,
                                                        );
                                                        cx.notify();
                                                    })),
                                            ),
                                    )
                                    .child(
                                        Button::new("send")
                                            .label(if running { "■" } else { "↑" })
                                            .custom(accent_button_variant(cx))
                                            .rounded(px(999.))
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.submit_composer(window, cx);
                                            })),
                                    ),
                            ),
                    ),
            )
            .when_some(status, |this, status| {
                this.child(
                    div()
                        .absolute()
                        .bottom(px(6.))
                        .left(px(0.))
                        .right(px(0.))
                        .text_center()
                        .text_size(px(10.))
                        .text_color(rgb(MUTED))
                        .child(status),
                )
            })
            .when(
                has_transcript && !transcript_at_bottom && !jump_to_latest_hidden,
                |this| {
                    this.child(
                        div()
                            .absolute()
                            .bottom(px(126.))
                            .right(px(if self.context_open { 370. } else { 18. }))
                            .child(
                                Button::new("jump-to-latest")
                                    .label("Jump to latest")
                                    .secondary()
                                    .rounded(px(999.))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.jump_to_latest_hidden_until =
                                            Some(Instant::now() + Duration::from_millis(250));
                                        this.transcript_scroll.scroll_to_bottom();
                                        cx.notify();
                                    })),
                            ),
                    )
                },
            )
            .when(self.composer_menu.is_some(), |this| {
                this.child(
                    div()
                        .absolute()
                        .bottom(px(126.))
                        .left(px(0.))
                        .right(px(0.))
                        .when(self.context_open, |this| this.pr(px(354.)))
                        .flex()
                        .justify_center()
                        .child(self.render_composer_menu(cx)),
                )
            })
            .child(self.render_more_menu(cx))
            .child(self.render_sessions(cx))
            .child(self.render_providers(cx))
            .child(self.render_help(cx))
            .child(self.render_context_panel(&context, cx))
            .child(self.render_change_review(&context, cx))
    }
}

fn short_id(id: SessionId) -> String {
    let id = id.to_string();
    id.chars().take(8).collect()
}

fn session_matches_query(session: &SessionListEntry, query: &str) -> bool {
    let route = format!("{}/{}", session.provider, session.model);
    let preview = session.first_message.as_deref().unwrap_or_default();
    format!(
        "{} {} {} {} {}",
        session.id, session.mode, route, session.workspace.root, preview
    )
    .to_lowercase()
    .contains(&query.to_lowercase())
}

fn session_meta_row(label: &str, value: String) -> AnyElement {
    div()
        .flex()
        .items_start()
        .justify_between()
        .gap_3()
        .border_b_1()
        .border_color(rgb(BORDER))
        .py(px(6.))
        .child(
            div()
                .flex_none()
                .text_size(px(9.))
                .text_color(rgb(MUTED))
                .child(label.to_owned()),
        )
        .child(
            div()
                .min_w(px(0.))
                .truncate()
                .text_align(TextAlign::Right)
                .font_family("monospace")
                .text_size(px(9.))
                .text_color(rgb(TEXT_SECONDARY))
                .child(value),
        )
        .into_any_element()
}

fn path_parts(path: &str) -> (String, String) {
    let path = path.trim_start_matches("./");
    match path.rsplit_once('/') {
        Some((directory, name)) => (
            name.to_owned(),
            if directory.is_empty() {
                "Workspace root".to_owned()
            } else {
                directory.to_owned()
            },
        ),
        None => (path.to_owned(), "Workspace root".to_owned()),
    }
}

fn pretty_provider_config(config: &serde_json::Value) -> String {
    serde_json::to_string_pretty(config).unwrap_or_else(|_| config.to_string())
}

fn parse_provider_config(text: &str) -> Result<serde_json::Value, String> {
    let config = serde_json::from_str::<serde_json::Value>(text.trim())
        .map_err(|error| format!("invalid JSON: {error}"))?;
    if !config.is_object() {
        return Err("configuration must be a JSON object".to_owned());
    }
    Ok(config)
}

fn format_token_count(tokens: usize) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}m", tokens as f32 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}k", tokens as f32 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

fn context_capabilities(context: &ContextView) -> Vec<String> {
    let mut capabilities = Vec::new();
    for capability in context
        .local_tools
        .iter()
        .chain(context.provider_tools.iter())
    {
        if !capabilities.contains(capability) {
            capabilities.push(capability.clone());
        }
    }
    capabilities
}

fn render_change_diff(change: &FileChange) -> AnyElement {
    let mut lines = Vec::new();
    let mut remaining = 80usize;
    if let Some(original) = &change.original {
        for line in original.lines().take(remaining) {
            lines.push(diff_line("-", line, 0xff9aaa, 0x281417));
            remaining = remaining.saturating_sub(1);
        }
    }
    if let Some(updated) = &change.updated {
        for line in updated.lines().take(remaining) {
            lines.push(diff_line("+", line, 0x9ce8b7, 0x10231a));
        }
    }
    if lines.is_empty() {
        lines.push(
            div()
                .text_size(px(10.))
                .text_color(rgb(MUTED))
                .child("No preview available")
                .into_any_element(),
        );
    }
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(1.))
        .children(lines)
        .into_any_element()
}

fn diff_line(marker: &str, line: &str, color: u32, background: u32) -> AnyElement {
    div()
        .w_full()
        .rounded(px(6.))
        .bg(rgb(background))
        .px(px(8.))
        .py(px(3.))
        .font_family("monospace")
        .text_size(px(9.))
        .line_height(px(15.))
        .text_color(rgb(color))
        .child(format!("{marker} {line}"))
        .into_any_element()
}

fn menu_label(label: &str) -> AnyElement {
    div()
        .px(px(9.))
        .py(px(7.))
        .text_size(px(9.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(MUTED))
        .child(label.to_owned())
        .into_any_element()
}

fn help_row(label: &str, shortcut: &str) -> AnyElement {
    div()
        .h(px(30.))
        .flex()
        .items_center()
        .justify_between()
        .border_b_1()
        .border_color(rgb(BORDER))
        .text_size(px(10.))
        .text_color(rgb(TEXT_SECONDARY))
        .child(label.to_owned())
        .child(
            div()
                .rounded(px(999.))
                .bg(rgb(SURFACE))
                .px(px(8.))
                .py(px(3.))
                .font_family("monospace")
                .text_size(px(9.))
                .text_color(rgb(MUTED))
                .child(shortcut.to_owned()),
        )
        .into_any_element()
}

fn accent_button_variant(cx: &App) -> ButtonCustomVariant {
    ButtonCustomVariant::new(cx)
        .color(rgb(ACCENT).into())
        .foreground(rgb(CANVAS).into())
        .hover(rgb(0x84dcf3).into())
        .active(rgb(0x84dcf3).into())
}

/// Run the desktop application until its window closes.
pub fn run_desktop(runtime: FrontRuntime) -> Result<(), DesktopError> {
    let mut runtime = Some(runtime);
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            Theme::change(ThemeMode::Dark, None, cx);
            let runtime = runtime
                .take()
                .ok_or_else(|| DesktopError::Startup("desktop runtime consumed".to_owned()))
                .expect("desktop runtime is initialized once");
            gpui_kit::open_window(WindowOptions::default(), cx, move |window, cx| {
                cx.new(|cx| AlphaDesktop::new(runtime, window, cx))
            })
            .expect("desktop window opens");
        });
    Ok(())
}

#[cfg(test)]
mod tests {
    use alpha_front_runtime::{ContextView, TranscriptItem};
    use alpha_schema::{
        PermissionMode, SessionId, SessionListEntry, Timestamp, ToolCallId, ToolStatus,
        WorkspaceRef,
    };

    use super::{
        DesktopTranscriptItem, context_capabilities, desktop_transcript_items, format_token_count,
        parse_provider_config, path_parts, pretty_provider_config, session_matches_query,
    };

    #[test]
    fn consecutive_tool_items_render_as_one_group_with_the_latest_tool() {
        let first = ToolCallId::new();
        let second = ToolCallId::new();
        let items = vec![
            TranscriptItem::ToolCall {
                id: first,
                name: "read".to_owned(),
                input: serde_json::json!({ "path": "README.md" }),
            },
            TranscriptItem::ToolResult {
                id: first,
                name: "read".to_owned(),
                status: ToolStatus::Completed,
                content: "README".to_owned(),
            },
            TranscriptItem::ToolCall {
                id: second,
                name: "grep".to_owned(),
                input: serde_json::json!({ "pattern": "desktop" }),
            },
            TranscriptItem::ToolResult {
                id: second,
                name: "grep".to_owned(),
                status: ToolStatus::Completed,
                content: "4 matches".to_owned(),
            },
        ];

        let rendered = desktop_transcript_items(&items);
        assert_eq!(rendered.len(), 1);
        let DesktopTranscriptItem::Tools { id, tools } = &rendered[0] else {
            panic!("consecutive tools must render as a tool group");
        };
        assert_eq!(id, &first.to_string());
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[1].name, "grep");
        assert_eq!(tools[1].status, Some(ToolStatus::Completed));
        assert_eq!(tools[1].content.as_deref(), Some("4 matches"));
    }

    #[test]
    fn durable_assistant_thinking_is_preserved_before_the_response() {
        let rendered = desktop_transcript_items(&[TranscriptItem::AssistantMessage {
            blocks: vec![
                alpha_schema::AssistantContentBlock::Thinking {
                    text: "inspect the workspace".to_owned(),
                    signature: None,
                },
                alpha_schema::AssistantContentBlock::Text {
                    text: "The workspace is ready.".to_owned(),
                },
            ],
        }]);
        assert_eq!(
            rendered,
            vec![
                DesktopTranscriptItem::Thinking("inspect the workspace".to_owned()),
                DesktopTranscriptItem::Assistant("The workspace is ready.".to_owned()),
            ]
        );
    }

    #[test]
    fn context_helpers_format_paths_tokens_and_capabilities() {
        assert_eq!(
            path_parts("crates/alpha-front-desktop/src/lib.rs"),
            (
                "lib.rs".to_owned(),
                "crates/alpha-front-desktop/src".to_owned()
            )
        );
        assert_eq!(
            path_parts("README.md"),
            ("README.md".to_owned(), "Workspace root".to_owned())
        );
        assert_eq!(format_token_count(987), "987");
        assert_eq!(format_token_count(12_400), "12.4k");
        assert_eq!(format_token_count(1_250_000), "1.2m");

        let context = ContextView {
            local_tools: vec!["read".to_owned(), "edit".to_owned()],
            provider_tools: vec!["edit".to_owned(), "web_search".to_owned()],
            ..ContextView::default()
        };
        assert_eq!(
            context_capabilities(&context),
            [
                "read".to_owned(),
                "edit".to_owned(),
                "web_search".to_owned()
            ]
        );
    }

    #[test]
    fn provider_config_round_trips_and_rejects_non_objects() {
        let config = serde_json::json!({
            "base_url": "https://example.test",
            "headers": { "x-test": "1" }
        });
        let formatted = pretty_provider_config(&config);
        assert_eq!(parse_provider_config(&formatted).unwrap(), config);
        assert_eq!(
            parse_provider_config("[]").unwrap_err(),
            "configuration must be a JSON object"
        );
        assert!(
            parse_provider_config("{")
                .unwrap_err()
                .starts_with("invalid JSON:")
        );
    }

    #[test]
    fn session_search_matches_preview_route_and_workspace() {
        let session = SessionListEntry {
            id: SessionId::new(),
            created_at: Timestamp::from_millis(1),
            mode: PermissionMode::Auto,
            workspace: WorkspaceRef {
                root: "/workspace/alpha".to_owned(),
                cwd: "/workspace/alpha".to_owned(),
            },
            provider: "deepseek".to_owned(),
            model: "chat".to_owned(),
            first_message: Some("Fix the desktop session manager".to_owned()),
        };
        assert!(session_matches_query(&session, "DESKTOP"));
        assert!(session_matches_query(&session, "deepseek/chat"));
        assert!(session_matches_query(&session, "workspace/alpha"));
        assert!(!session_matches_query(&session, "unrelated"));
    }
}

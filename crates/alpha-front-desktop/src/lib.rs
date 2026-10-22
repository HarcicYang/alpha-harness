//! GPUI desktop frontend for alpha-harness.

#![forbid(unsafe_code)]

use std::time::Duration;

use alpha_front_runtime::{
    CommandOutcome, FrontRuntime, InteractionDecision, ModelChoice, TranscriptItem, parse_command,
    supported_reasoning_levels,
};
use alpha_schema::{
    AssistantContentBlock, PermissionMode, ReasoningLevel, SessionId, SessionListEntry, ToolStatus,
};
use gpui_kit::base::Selectable as _;
use gpui_kit::component::{
    Theme, ThemeMode,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    input::{InputEvent, Textarea, TextareaState},
    scroll::ScrollableElement as _,
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
    sessions_open: bool,
    sessions: Vec<SessionListEntry>,
    local_status: Option<String>,
    _poll_task: Option<Task<()>>,
    _composer_subscription: Subscription,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ComposerMenu {
    Mode,
    Model,
    Effort,
}

#[derive(Clone, Debug)]
enum DesktopTranscriptItem {
    User(String),
    Assistant(String),
    Thinking(String),
    Tools(Vec<ToolSummary>),
    Error(String),
}

#[derive(Clone, Debug)]
struct ToolSummary {
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
        let model_choices = runtime.providers().model_choices();
        let mut this = Self {
            runtime,
            composer,
            model_choices,
            composer_menu: None,
            submit_requested: false,
            context_open: false,
            sessions_open: false,
            sessions: Vec::new(),
            local_status: None,
            _poll_task: None,
            _composer_subscription: composer_subscription,
        };
        this._poll_task = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
                if this
                    .update_in(cx, |this, window, cx| {
                        if this.submit_requested {
                            this.submit_requested = false;
                            this.submit_composer(window, cx);
                        }
                        let mut changed = this.runtime.expire_interaction().unwrap_or(false);
                        match this.runtime.try_receive_events() {
                            Ok(events) => changed |= !events.is_empty(),
                            Err(error) => this.local_status = Some(error.to_string()),
                        }
                        if changed {
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

    fn open_sessions(&mut self, cx: &mut Context<Self>) {
        match self.runtime.list_sessions() {
            Ok(sessions) => {
                self.sessions = sessions;
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
                self.local_status = None;
            }
            Err(error) => self.local_status = Some(error.to_string()),
        }
        cx.notify();
    }

    fn new_session(&mut self, cx: &mut Context<Self>) {
        self.runtime.new_session();
        self.sessions_open = false;
        self.composer_menu = None;
        self.local_status = None;
        cx.notify();
    }

    fn render_sessions(&self, cx: &mut Context<Self>) -> AnyElement {
        if !self.sessions_open {
            return div().into_any_element();
        }
        let rows = if self.sessions.is_empty() {
            vec![
                div()
                    .py(px(40.))
                    .text_center()
                    .text_size(px(12.))
                    .text_color(rgb(MUTED))
                    .child("No stored sessions")
                    .into_any_element(),
            ]
        } else {
            self.sessions
                .iter()
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
                    Button::new(format!("session:{}", session.id))
                        .child(
                            div()
                                .w_full()
                                .flex()
                                .flex_col()
                                .items_start()
                                .gap_1()
                                .child(
                                    div()
                                        .w_full()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap_2()
                                                .child(
                                                    div()
                                                        .text_size(px(11.))
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
                                                        .text_size(px(9.))
                                                        .text_color(rgb(ACCENT))
                                                        .child(session.mode.to_string()),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(9.))
                                                .font_family("monospace")
                                                .text_color(rgb(MUTED))
                                                .child(route),
                                        ),
                                )
                                .child(
                                    div()
                                        .w_full()
                                        .truncate()
                                        .text_size(px(11.))
                                        .text_color(rgb(TEXT_SECONDARY))
                                        .child(preview),
                                ),
                        )
                        .bg(rgb(SURFACE))
                        .border_1()
                        .border_color(rgb(BORDER))
                        .w_full()
                        .rounded(px(18.))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.resume_session(session_id, cx);
                        }))
                        .into_any_element()
                })
                .collect()
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
                    .mt(px(70.))
                    .w(px(680.))
                    .h(px(520.))
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
                                    .text_size(px(14.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Sessions"),
                            )
                            .child(
                                Button::new("close-sessions")
                                    .label("×")
                                    .ghost()
                                    .rounded(px(999.))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.sessions_open = false;
                                        cx.notify();
                                    })),
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
                            .h(px(54.))
                            .flex_none()
                            .px(px(12.))
                            .flex()
                            .items_center()
                            .justify_between()
                            .border_t_1()
                            .border_color(rgb(BORDER))
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(rgb(MUTED))
                                    .child("Select a session to resume"),
                            )
                            .child(
                                Button::new("new-session")
                                    .custom(accent_button_variant(cx))
                                    .label("New session")
                                    .rounded(px(999.))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.new_session(cx);
                                    })),
                            ),
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
            .w(px(320.))
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
                    Button::new(format!("model:{}:{}", choice.provider, choice.model))
                        .label(format!(
                            "{}  ·  {}",
                            choice.provider_name, choice.model_name
                        ))
                        .selected(selected)
                        .ghost()
                        .w_full()
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

        let result = if value.starts_with('/') {
            self.runtime
                .execute_command(parse_command(&value))
                .map(|outcome| {
                    if matches!(outcome, CommandOutcome::OpenSessions) {
                        self.local_status = Some("session manager is not wired yet".to_owned());
                    }
                })
        } else {
            self.runtime.submit_prompt(&value)
        };

        match result {
            Ok(()) => {
                self.local_status = None;
                self.composer
                    .update(cx, |state, cx| state.set_value("", window, cx));
            }
            Err(error) => self.local_status = Some(error.to_string()),
        }
        cx.notify();
    }

    fn render_transcript(&self) -> AnyElement {
        let items = desktop_transcript_items(&self.runtime.chat().transcript)
            .into_iter()
            .map(render_desktop_transcript_item)
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
}

fn desktop_transcript_items(items: &[TranscriptItem]) -> Vec<DesktopTranscriptItem> {
    let mut rendered = Vec::new();
    let mut tools = Vec::new();

    for item in items {
        match item {
            TranscriptItem::ToolCall { name, input, .. } => tools.push(ToolSummary {
                name: if input.is_null() {
                    name.clone()
                } else {
                    format!("{name} {}", compact_json(input))
                },
                status: None,
                content: None,
            }),
            TranscriptItem::ToolResult {
                name,
                status,
                content,
                ..
            } => {
                if let Some(tool) = tools.iter_mut().rev().find(|tool| {
                    tool.status.is_none()
                        && (tool.name == *name || tool.name.starts_with(&format!("{name} ")))
                }) {
                    tool.name = name.clone();
                    tool.status = Some(*status);
                    tool.content = Some(content.clone());
                } else {
                    tools.push(ToolSummary {
                        name: name.clone(),
                        status: Some(*status),
                        content: Some(content.clone()),
                    });
                }
            }
            other => {
                flush_tools(&mut rendered, &mut tools);
                match other {
                    TranscriptItem::User { text } | TranscriptItem::QueuedSteer { text, .. } => {
                        rendered.push(DesktopTranscriptItem::User(text.clone()));
                    }
                    TranscriptItem::AssistantDelta { text } => {
                        rendered.push(DesktopTranscriptItem::Assistant(text.clone()));
                    }
                    TranscriptItem::ThinkingDelta { text } => {
                        rendered.push(DesktopTranscriptItem::Thinking(text.clone()));
                    }
                    TranscriptItem::AssistantMessage { blocks } => {
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
        items.push(DesktopTranscriptItem::Tools(std::mem::take(tools)));
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

fn render_desktop_transcript_item(item: DesktopTranscriptItem) -> AnyElement {
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
            .text_size(px(14.))
            .line_height(px(24.))
            .text_color(rgb(TEXT))
            .child(text)
            .into_any_element(),
        DesktopTranscriptItem::Thinking(text) => div()
            .w_full()
            .text_size(px(13.))
            .line_height(px(22.))
            .italic()
            .text_color(rgb(MUTED))
            .child(text)
            .into_any_element(),
        DesktopTranscriptItem::Error(text) => div()
            .w_full()
            .text_size(px(13.))
            .line_height(px(22.))
            .text_color(rgb(0xff7888))
            .child(text)
            .into_any_element(),
        DesktopTranscriptItem::Tools(tools) => render_tool_group(&tools),
    }
}

fn render_tool_group(tools: &[ToolSummary]) -> AnyElement {
    let Some(latest) = tools.last() else {
        return div().into_any_element();
    };
    let status = latest.status.map_or("running", |status| match status {
        ToolStatus::Completed => "completed",
        ToolStatus::Failed => "failed",
        ToolStatus::Interrupted => "interrupted",
    });

    div()
        .w_full()
        .rounded(px(22.))
        .border_1()
        .border_color(rgb(BORDER))
        .bg(rgb(SURFACE))
        .p(px(4.))
        .flex()
        .flex_col()
        .when(tools.len() > 1, |this| {
            this.child(
                div()
                    .px(px(9.))
                    .py(px(7.))
                    .text_size(px(9.))
                    .font_family("monospace")
                    .text_color(rgb(MUTED))
                    .child(format!("{} tool calls", tools.len())),
            )
        })
        .child(
            div()
                .rounded(px(18.))
                .px(px(11.))
                .py(px(9.))
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(TEXT_SECONDARY))
                        .child(latest.name.clone()),
                )
                .child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(MUTED))
                        .child(status),
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
                    .child(detail.to_owned()),
            )
        })
        .into_any_element()
}

impl Render for AlphaDesktop {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chat = self.runtime.chat();
        let model = if chat.model.is_empty() {
            "unset".to_owned()
        } else {
            chat.model.clone()
        };
        let effort = chat.reasoning.to_string();
        let mode = chat.mode.to_string();
        let running = chat.running_turn.is_some();
        let status = self.local_status.as_deref().or(chat.status.as_deref());
        let interaction = chat.interaction.clone();

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
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.open_sessions(cx);
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
                                    .rounded(px(999.)),
                            ),
                    ),
            )
            .child(
                div()
                    .id("transcript")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .child(
                        div()
                            .w(px(780.))
                            .max_w_full()
                            .mx_auto()
                            .px(px(24.))
                            .py(px(18.))
                            .child(self.render_transcript())
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
                div().flex_none().px(px(20.)).pb(px(22.)).child(
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
                                                    self.composer_menu == Some(ComposerMenu::Mode),
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
                                                    self.composer_menu == Some(ComposerMenu::Model),
                                                )
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.open_composer_menu(ComposerMenu::Model);
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
                                                    this.open_composer_menu(ComposerMenu::Effort);
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
                        .child(status.to_owned()),
                )
            })
            .when(self.composer_menu.is_some(), |this| {
                this.child(
                    div()
                        .absolute()
                        .bottom(px(126.))
                        .left(px(0.))
                        .right(px(0.))
                        .flex()
                        .justify_center()
                        .child(self.render_composer_menu(cx)),
                )
            })
            .child(self.render_sessions(cx))
            .when(self.context_open, |this| {
                this.child(
                    div()
                        .absolute()
                        .top(px(58.))
                        .right(px(18.))
                        .w(px(336.))
                        .rounded(px(22.))
                        .border_1()
                        .border_color(rgb(BORDER))
                        .bg(rgb(SURFACE_RAISED))
                        .p(px(16.))
                        .child(
                            div()
                                .text_size(px(12.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("Context"),
                        )
                        .child(
                            div()
                                .mt_2()
                                .text_size(px(11.))
                                .line_height(px(18.))
                                .text_color(rgb(TEXT_SECONDARY))
                                .child(format!(
                                    "Provider: {}\nModel: {}\nEffort: {}\nMode: {}",
                                    if chat.provider.is_empty() {
                                        "default"
                                    } else {
                                        &chat.provider
                                    },
                                    chat.model,
                                    chat.reasoning,
                                    chat.mode,
                                )),
                        ),
                )
            })
    }
}

fn short_id(id: SessionId) -> String {
    let id = id.to_string();
    id.chars().take(8).collect()
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
    use alpha_front_runtime::TranscriptItem;
    use alpha_schema::{ToolCallId, ToolStatus};

    use super::{DesktopTranscriptItem, desktop_transcript_items};

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
        let DesktopTranscriptItem::Tools(tools) = &rendered[0] else {
            panic!("consecutive tools must render as a tool group");
        };
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[1].name, "grep");
        assert_eq!(tools[1].status, Some(ToolStatus::Completed));
        assert_eq!(tools[1].content.as_deref(), Some("4 matches"));
    }
}

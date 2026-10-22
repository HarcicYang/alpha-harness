//! GPUI desktop frontend for alpha-harness.

#![forbid(unsafe_code)]

use std::time::Duration;

use alpha_front_runtime::{
    CommandOutcome, FrontRuntime, InteractionDecision, TranscriptItem, parse_command,
};
use alpha_schema::AssistantContentBlock;
use gpui_kit::base::Selectable as _;
use gpui_kit::component::{
    Theme, ThemeMode,
    button::{Button, ButtonVariants as _},
    input::{Textarea, TextareaState},
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
    context_open: bool,
    local_status: Option<String>,
    _poll_task: Option<Task<()>>,
}

impl AlphaDesktop {
    fn new(runtime: FrontRuntime, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Ask alpha to inspect, change, or explain...")
                .auto_grow(1, 6)
        });
        let mut this = Self {
            runtime,
            composer,
            context_open: false,
            local_status: None,
            _poll_task: None,
        };
        this._poll_task = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
                if this
                    .update_in(cx, |this, _, cx| {
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
        let items = self
            .runtime
            .chat()
            .transcript
            .iter()
            .filter_map(render_transcript_item)
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

fn render_transcript_item(item: &TranscriptItem) -> Option<AnyElement> {
    match item {
        TranscriptItem::User { text } | TranscriptItem::QueuedSteer { text, .. } => Some(
            div()
                .w_full()
                .rounded(px(24.))
                .bg(rgb(0x15181c))
                .px(px(18.))
                .py(px(15.))
                .text_size(px(14.))
                .line_height(px(23.))
                .text_color(rgb(TEXT))
                .child(text.clone())
                .into_any_element(),
        ),
        TranscriptItem::AssistantDelta { text }
        | TranscriptItem::ThinkingDelta { text }
        | TranscriptItem::Error { message: text } => Some(
            div()
                .w_full()
                .text_size(px(14.))
                .line_height(px(24.))
                .text_color(rgb(TEXT))
                .child(text.clone())
                .into_any_element(),
        ),
        TranscriptItem::AssistantMessage { blocks } => {
            let text = blocks
                .iter()
                .filter_map(AssistantContentBlock::text)
                .collect::<Vec<_>>()
                .join("\n");
            (!text.is_empty()).then(|| {
                div()
                    .w_full()
                    .text_size(px(14.))
                    .line_height(px(24.))
                    .text_color(rgb(TEXT))
                    .child(text)
                    .into_any_element()
            })
        }
        TranscriptItem::ToolCall { name, .. } => Some(tool_row(name, "running", None)),
        TranscriptItem::ToolResult {
            name,
            status,
            content,
            ..
        } => Some(tool_row(
            name,
            match status {
                alpha_schema::ToolStatus::Completed => "completed",
                alpha_schema::ToolStatus::Failed => "failed",
                alpha_schema::ToolStatus::Interrupted => "interrupted",
            },
            Some(content),
        )),
    }
}

fn tool_row(name: &str, status: &str, detail: Option<&str>) -> AnyElement {
    div()
        .w_full()
        .rounded(px(20.))
        .border_1()
        .border_color(rgb(BORDER))
        .bg(rgb(SURFACE))
        .px(px(14.))
        .py(px(11.))
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(TEXT_SECONDARY))
                        .child(name.to_owned()),
                )
                .child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(MUTED))
                        .child(status.to_owned()),
                ),
        )
        .when_some(detail, |this, detail| {
            this.child(
                div()
                    .mt_1()
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
                                    .rounded(px(999.)),
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
                                                        .primary()
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
                        .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                            if event.keystroke.key == "enter" && !event.keystroke.modifiers.shift {
                                window.prevent_default();
                                this.submit_composer(window, cx);
                            }
                        }))
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
                                                .rounded(px(999.)),
                                        )
                                        .child(
                                            Button::new("model")
                                                .label(model)
                                                .ghost()
                                                .rounded(px(999.)),
                                        )
                                        .child(
                                            Button::new("effort")
                                                .label(effort)
                                                .ghost()
                                                .rounded(px(999.)),
                                        ),
                                )
                                .child(
                                    Button::new("send")
                                        .label(if running { "■" } else { "↑" })
                                        .primary()
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

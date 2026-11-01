use super::*;

mod tools;

impl AlphaDesktop {
    pub(crate) fn render_transcript(
        &mut self,
        viewport_height: f32,
        viewport_width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let transcript_changed = self.transcript_dirty;
        if transcript_changed {
            let previous_count = self.transcript_items.len();
            let items = Arc::new(desktop_transcript_items(&self.runtime.chat().transcript));
            self.sync_tool_outputs(&items);
            self.transcript_animation_floor = if self.transcript_items.is_empty() {
                0
            } else {
                previous_count.min(items.len())
            };
            self.transcript_items = items;
            self.transcript_dirty = false;
        }
        let items = self.transcript_items.clone();
        if items.is_empty() {
            if transcript_changed {
                let old_count = self.transcript_list.item_count();
                if old_count > 0 {
                    self.transcript_list.splice(0..old_count, 0);
                }
            }
            return self.render_empty_transcript(viewport_height, cx);
        }

        if transcript_changed {
            let old_count = self.transcript_list.item_count();
            let new_count = items.len();
            if old_count != new_count {
                if new_count > old_count {
                    self.transcript_list
                        .splice(old_count..old_count, new_count - old_count);
                    if old_count > 0 {
                        self.transcript_list
                            .remeasure_items(old_count - 1..old_count);
                    }
                } else {
                    self.transcript_list.splice(0..old_count, new_count);
                }
            } else {
                self.transcript_list
                    .remeasure_items(new_count.saturating_sub(1)..new_count);
            }
        }

        let namespace = format!("conversation-{}", self.transcript_epoch);
        let animation_floor = self.transcript_animation_floor;
        if transcript_changed {
            self.transcript_animation_floor = items.len();
        }
        let weak = cx.weak_entity();
        list(self.transcript_list.clone(), move |index, _, cx| {
            let Some(item) = items.get(index).cloned() else {
                return div().into_any_element();
            };
            weak.update(cx, |this, cx| {
                let rendered = this.render_desktop_transcript_item(
                    &namespace,
                    index,
                    &item,
                    viewport_width,
                    cx,
                );
                div()
                    .w_full()
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .w(px(CONTENT_WIDTH))
                            .max_w_full()
                            .px(px(24.))
                            .pt(px(11.))
                            .pb(px(11.))
                            .child(if index >= animation_floor {
                                enter_from_below(
                                    div().child(rendered),
                                    format!("transcript:{namespace}:{index}"),
                                )
                            } else {
                                rendered
                            }),
                    )
                    .into_any_element()
            })
            .unwrap_or_else(|_| div().into_any_element())
        })
        .size_full()
        .min_h(px(0.))
        .pt(px(3.))
        .pr(px(SCROLLBAR_GUTTER))
        .into_any_element()
    }

    fn sync_tool_outputs(&mut self, items: &[DesktopTranscriptItem]) {
        let tool_ids = items
            .iter()
            .flat_map(|item| match item {
                DesktopTranscriptItem::Tools { tools, .. } => {
                    tools.iter().map(|tool| tool.id).collect::<Vec<_>>()
                }
                _ => Vec::new(),
            })
            .collect::<BTreeSet<_>>();
        self.tool_outputs
            .retain(|tool_id, _| tool_ids.contains(tool_id));
        for tool_id in tool_ids {
            self.tool_outputs.entry(tool_id).or_default();
        }
    }

    fn render_empty_transcript(&self, viewport_height: f32, cx: &mut Context<Self>) -> AnyElement {
        let _ = cx;

        div()
            .w_full()
            .h(px((viewport_height - 210.).max(280.)))
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .child(
                div()
                    .relative()
                    .size(px(260.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        empty_alpha_layer(184., ACCENT)
                            .absolute()
                            .top(px(-2.))
                            .left(px(2.))
                            .with_animation(
                                "empty-alpha-glow",
                                Animation::new(Duration::from_millis(900)),
                                |alpha, progress| alpha.opacity(0.08 + progress * 0.05),
                            ),
                    )
                    .child(empty_alpha_layer(176., 0x526b7b).with_animation(
                        "empty-alpha-depth",
                        Animation::new(Duration::from_millis(900)),
                        |alpha, progress| alpha.opacity(0.88 + progress * 0.1),
                    ))
                    .child(
                        empty_alpha_layer(174., 0x9bb2bf)
                            .absolute()
                            .top(px(-1.))
                            .left(px(-1.))
                            .opacity(0.14),
                    ),
            )
            .into_any_element()
    }

    fn render_desktop_transcript_item(
        &self,
        namespace: &str,
        index: usize,
        item: &DesktopTranscriptItem,
        viewport_width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match item {
            DesktopTranscriptItem::User(text) => div()
                .w_full()
                .rounded(px(RADIUS_MD))
                .border_1()
                .border_color(rgba(BORDER_SUBTLE))
                .bg(rgb(USER_BAND))
                .px(px(17.))
                .pt(px(14.))
                .pb(px(15.))
                .flex()
                .flex_col()
                .child(
                    div()
                        .mb(px(7.))
                        .text_size(px(9.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(MUTED))
                        .child("You"),
                )
                .child(
                    div()
                        .w_full()
                        .min_w(px(0.))
                        .text_size(px(13.5))
                        .line_height(px(22.))
                        .text_color(rgb(TEXT))
                        .child(text.clone()),
                )
                .into_any_element(),
            DesktopTranscriptItem::Assistant(text) => {
                self.render_assistant_text(namespace, index, text, false)
            }
            DesktopTranscriptItem::StreamingAssistant(text) => {
                self.render_assistant_text(namespace, index, text, true)
            }
            DesktopTranscriptItem::Thinking(text) => {
                self.render_thinking(index, text.clone(), false, viewport_width, cx)
            }
            DesktopTranscriptItem::StreamingThinking(text) => {
                self.render_thinking(index, text.clone(), true, viewport_width, cx)
            }
            DesktopTranscriptItem::Error(text) => div()
                .w_full()
                .rounded(px(18.))
                .border_1()
                .border_color(rgba(0xc9808b4d))
                .bg(rgba(DANGER_SOFT))
                .px(px(13.))
                .py(px(11.))
                .flex()
                .items_start()
                .gap(px(9.))
                .child(icon_badge(IconName::CircleX, DANGER, DANGER_SOFT, 24.))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .text_size(px(11.5))
                        .line_height(px(19.))
                        .text_color(rgb(DANGER))
                        .child(text.clone()),
                )
                .into_any_element(),
            DesktopTranscriptItem::Tools { id, tools } => self.render_tool_group(id, tools, cx),
        }
    }

    fn render_assistant_text(
        &self,
        namespace: &str,
        index: usize,
        text: &SharedString,
        streaming: bool,
    ) -> AnyElement {
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                TextView::markdown(
                    format!("assistant-markdown:{namespace}:{index}"),
                    text.clone(),
                )
                .w_full()
                .min_w(px(0.))
                .text_size(px(13.5))
                .line_height(px(23.))
                .text_color(rgb(TEXT)),
            )
            .when(streaming, |this| {
                this.child(stream_cursor(format!("stream-cursor:{namespace}:{index}")))
            })
            .into_any_element()
    }

    fn render_thinking(
        &self,
        index: usize,
        text: SharedString,
        streaming: bool,
        viewport_width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let expanded = self.expanded_thoughts.contains(&index);
        let summary_width = (viewport_width.min(CONTENT_WIDTH) - 96.).max(120.);
        let summary_chars = (summary_width / 7.).floor().max(24.) as usize;
        let summary = thought_summary(&text, summary_chars);
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                Button::new(format!("toggle-thinking:{index}"))
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .items_center()
                            .justify_start()
                            .gap(px(7.))
                            .child(icon_badge(IconName::Sparkles, ACCENT, ACCENT_SOFT, 18.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .truncate()
                                    .text_size(px(10.5))
                                    .text_color(rgb(MUTED))
                                    .child(format!("Thought  {summary}")),
                            )
                            .child(disclosure_chevron(
                                format!("thought-chevron:{index}"),
                                expanded,
                                11.,
                            )),
                    )
                    .ghost()
                    .w_full()
                    .h(px(26.))
                    .px(px(7.))
                    .rounded_full()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.expanded_thoughts.remove(&index) {
                            this.expanded_thoughts.insert(index);
                        }
                        this.bump_overlay_generation();
                        cx.notify();
                    })),
            )
            .when(expanded, |this| {
                this.child(popover_enter(
                    div()
                        .ml(px(3.))
                        .border_l_2()
                        .border_color(rgba(BORDER_STRONG))
                        .pl(px(13.))
                        .py(px(2.))
                        .text_size(px(12.))
                        .line_height(px(20.))
                        .italic()
                        .text_color(rgb(TEXT_SECONDARY))
                        .child(text),
                    format!("thought-expansion:{index}:{}", self.overlay_generation),
                ))
            })
            .when(streaming, |this| {
                this.child(stream_cursor(format!("thinking-cursor:{index}")))
            })
            .into_any_element()
    }
}

fn disclosure_chevron(id: String, expanded: bool, size: f32) -> AnyElement {
    let animation_id = format!("{id}:{expanded}");
    Icon::new(IconName::ChevronRight)
        .size(px(size))
        .text_color(rgb(FAINT))
        .with_animation(
            animation_id,
            Animation::new(Duration::from_millis(180)),
            move |icon, progress| {
                let start = if expanded {
                    0.
                } else {
                    std::f32::consts::FRAC_PI_2
                };
                let end = if expanded {
                    std::f32::consts::FRAC_PI_2
                } else {
                    0.
                };
                icon.rotate(radians(start + (end - start) * progress))
            },
        )
        .into_any_element()
}

fn thought_summary(text: &str, max_chars: usize) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let count = compact.chars().count();
    if count <= max_chars {
        return compact;
    }
    let keep = max_chars.saturating_sub(3);
    let tail = compact.chars().skip(count - keep).collect::<String>();
    format!("...{tail}")
}

fn empty_alpha_layer(size: f32, color: u32) -> Div {
    div()
        .absolute()
        .inset_0()
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .font_family("Georgia")
        .text_size(px(size))
        .line_height(px(size))
        .font_weight(FontWeight::THIN)
        .italic()
        .text_center()
        .text_color(rgb(color))
        .child("α")
}

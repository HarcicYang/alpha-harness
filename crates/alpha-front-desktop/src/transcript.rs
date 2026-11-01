use super::*;

impl AlphaDesktop {
    pub(crate) fn render_transcript(
        &mut self,
        viewport_height: f32,
        viewport_width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.transcript_dirty {
            let items = Arc::new(desktop_transcript_items(&self.runtime.chat().transcript));
            self.sync_tool_outputs(&items);
            self.transcript_items = items;
            self.transcript_dirty = false;
        }
        let items = self.transcript_items.clone();
        if items.is_empty() {
            return self.render_empty_transcript(viewport_height, cx);
        }

        let namespace = format!("conversation-{}", self.transcript_epoch);
        let children = items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let rendered = self.render_desktop_transcript_item(
                    &namespace,
                    index,
                    item,
                    viewport_width,
                    cx,
                );
                enter_from_below(
                    div().child(rendered),
                    format!("transcript:{namespace}:{index}"),
                )
            })
            .collect::<Vec<_>>();

        div()
            .flex()
            .flex_col()
            .gap(px(22.))
            .children(children)
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
                                Animation::new(Duration::from_millis(5200)).repeat(),
                                |alpha, progress| {
                                    let wave = (progress * std::f32::consts::TAU).sin() * 0.5 + 0.5;
                                    alpha.opacity(0.08 + wave * 0.05)
                                },
                            ),
                    )
                    .child(empty_alpha_layer(176., 0x526b7b).with_animation(
                        "empty-alpha-depth",
                        Animation::new(Duration::from_millis(5200)).repeat(),
                        |alpha, progress| {
                            let wave = (progress * std::f32::consts::TAU).sin() * 0.5 + 0.5;
                            alpha.opacity(0.88 + wave * 0.1)
                        },
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
        let summary_chars = (summary_width / 10.).floor().max(18.) as usize;
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
                            .flex()
                            .items_center()
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
                    .h(px(26.))
                    .px(px(7.))
                    .rounded_full()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let follow = transcript_is_at_bottom(&this.transcript_scroll);
                        if !this.expanded_thoughts.remove(&index) {
                            this.expanded_thoughts.insert(index);
                        }
                        this.bump_overlay_generation();
                        if follow {
                            this.scroll_to_bottom(ScrollRegion::Transcript);
                        }
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

    fn render_tool_group(
        &self,
        id: &str,
        tools: &[ToolSummary],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if tools.is_empty() {
            return div().into_any_element();
        }
        if tools.len() == 1 {
            return self.render_tool_row(&tools[0], ToolRowStyle::Standalone, cx);
        }

        let expanded = self.expanded_tool_groups.contains(id);
        let group_id = id.to_owned();
        let (latest, history) = tools
            .split_last()
            .expect("multi-tool groups contain a latest tool");
        let tool_ids = tools.iter().map(|tool| tool.id).collect::<Vec<_>>();
        let history_rows = history
            .iter()
            .map(|tool| self.render_tool_row(tool, ToolRowStyle::History, cx))
            .collect::<Vec<_>>();

        div()
            .w_full()
            .rounded(px(22.))
            .border_1()
            .border_color(rgba(BORDER_SUBTLE))
            .bg(rgb(SURFACE))
            .p(px(4.))
            .flex()
            .flex_col()
            .child(
                Button::new(format!("tool-group:{id}"))
                    .child(
                        div()
                            .w_full()
                            .h(px(30.))
                            .px(px(8.))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                Icon::new(IconName::Layers)
                                    .size(px(12.))
                                    .text_color(rgb(ACCENT)),
                            )
                            .child(
                                div()
                                    .text_size(px(8.5))
                                    .font_family("monospace")
                                    .text_color(rgb(TEXT_SECONDARY))
                                    .child(if tools.len() == 1 {
                                        "1 tool call".to_owned()
                                    } else {
                                        format!("{} tool calls", tools.len())
                                    }),
                            )
                            .child(div().flex_1())
                            .child(disclosure_chevron(
                                format!("tool-group-chevron:{id}"),
                                expanded,
                                12.,
                            )),
                    )
                    .ghost()
                    .w_full()
                    .rounded(px(16.))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_tool_group(&group_id, &tool_ids, cx);
                    })),
            )
            .when(expanded, |this| {
                this.child(popover_enter(
                    div()
                        .mx(px(3.))
                        .my(px(3.))
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .children(history_rows),
                    format!("tool-expansion:{id}:{}", self.overlay_generation),
                ))
            })
            .child(self.render_tool_row(latest, ToolRowStyle::Latest, cx))
            .into_any_element()
    }

    fn toggle_tool_group(
        &mut self,
        group_id: &str,
        tool_ids: &[ToolCallId],
        cx: &mut Context<Self>,
    ) {
        let follow = transcript_is_at_bottom(&self.transcript_scroll);
        if !self.expanded_tool_groups.remove(group_id) {
            self.bump_overlay_generation();
            self.expanded_tool_groups.insert(group_id.to_owned());
        }
        for tool_id in tool_ids {
            self.reset_tool_scroll(*tool_id);
        }
        if follow {
            self.scroll_to_bottom(ScrollRegion::Transcript);
        }
        cx.notify();
    }

    fn render_tool_row(
        &self,
        tool: &ToolSummary,
        style: ToolRowStyle,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = self
            .tool_outputs
            .get(&tool.id)
            .expect("tool output state is synchronized before rendering");
        let expanded = state.expanded;
        let status = tool_status_label(tool.status);
        let color = tool_status_color(tool.status);
        let tool_id = tool.id;
        let (background, border, radius, min_height) = match style {
            ToolRowStyle::Standalone => (rgb(SURFACE), rgba(BORDER_SUBTLE), 18., 54.),
            ToolRowStyle::History => (rgb(SURFACE_INSET), rgba(0x00000000), 16., 52.),
            ToolRowStyle::Latest => (rgba(0x00000000), rgba(0x00000000), 16., 54.),
        };
        let (expansion_x, expansion_top, expansion_bottom) = match style {
            ToolRowStyle::Latest => (4., 4., 2.),
            ToolRowStyle::Standalone | ToolRowStyle::History => (10., 5., 1.),
        };
        let tool_kind = tool.name.split_whitespace().next().unwrap_or("tool");

        div()
            .w_full()
            .rounded(px(radius))
            .border_1()
            .border_color(border)
            .bg(background)
            .child(
                Button::new(format!("tool-call:{tool_id}"))
                    .child(
                        div()
                            .w_full()
                            .min_h(px(min_height))
                            .px(px(10.))
                            .py(px(8.))
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(icon_badge(
                                tool_status_icon(tool.status),
                                color,
                                ACCENT_SOFT,
                                24.,
                            ))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .truncate()
                                            .text_size(px(11.5))
                                            .text_color(rgb(TEXT_SECONDARY))
                                            .child(tool.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .mt(px(2.))
                                            .truncate()
                                            .text_size(px(9.5))
                                            .text_color(rgb(MUTED))
                                            .child(tool.content.as_deref().map_or_else(
                                                || {
                                                    tool.input
                                                        .as_ref()
                                                        .map(compact_json)
                                                        .unwrap_or_default()
                                                },
                                                compact_tool_content,
                                            )),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(8.5))
                                    .text_color(rgb(color))
                                    .child(status),
                            )
                            .child(disclosure_chevron(
                                format!("tool-chevron:{tool_id}"),
                                expanded,
                                11.,
                            )),
                    )
                    .ghost()
                    .w_full()
                    .h(px(min_height))
                    .rounded(px(radius))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let follow = transcript_is_at_bottom(&this.transcript_scroll);
                        this.bump_overlay_generation();
                        let state = this
                            .tool_outputs
                            .get_mut(&tool_id)
                            .expect("tool output state is synchronized before interaction");
                        state.expanded = !state.expanded;
                        this.reset_tool_scroll(tool_id);
                        if follow {
                            this.scroll_to_bottom(ScrollRegion::Transcript);
                        }
                        cx.notify();
                    })),
            )
            .when(expanded, |this| {
                this.child(popover_enter(
                    div()
                        .mx(px(expansion_x))
                        .mb(px(expansion_bottom))
                        .mt(px(expansion_top))
                        .overflow_hidden()
                        .rounded(px(20.))
                        .border_1()
                        .border_color(rgba(BORDER_SUBTLE))
                        .bg(rgb(SURFACE_INSET))
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .px(px(13.))
                                .pt(px(10.))
                                .flex()
                                .gap(px(5.))
                                .child(tool_meta_chip(tool_kind))
                                .child(tool_meta_chip(status)),
                        )
                        .child(
                            div()
                                .id(format!("tool-scroll-shell:{tool_id}"))
                                .relative()
                                .mt(px(8.))
                                .max_h(px(160.))
                                .tool_scroll_shell(tool_id, cx)
                                .child(
                                    div()
                                        .id(format!("tool-scroll:{tool_id}"))
                                        .w_full()
                                        .max_h(px(160.))
                                        .overflow_y_scroll()
                                        .track_scroll(&state.scroll)
                                        .px(px(13.))
                                        .pb(px(12.))
                                        .pr(px(SCROLLBAR_GUTTER))
                                        .font_family("monospace")
                                        .text_size(px(9.))
                                        .line_height(px(15.75))
                                        .text_color(rgb(TEXT_SECONDARY))
                                        .child(tool_detail_content(tool)),
                                )
                                .child(scrollbar_rail(format!("tool:{tool_id}"), &state.scroll)),
                        ),
                    format!("tool-detail:{tool_id}:{}", self.overlay_generation),
                ))
            })
            .into_any_element()
    }
}

#[derive(Clone, Copy)]
enum ToolRowStyle {
    Standalone,
    History,
    Latest,
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

fn tool_meta_chip(label: &str) -> Div {
    div()
        .rounded_full()
        .border_1()
        .border_color(rgba(BORDER_SUBTLE))
        .bg(rgb(SURFACE))
        .px(px(8.))
        .py(px(3.))
        .font_family("monospace")
        .text_size(px(8.))
        .text_color(rgb(MUTED))
        .child(label.to_owned())
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

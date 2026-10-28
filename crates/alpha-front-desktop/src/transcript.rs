use super::*;

impl AlphaDesktop {
    pub(crate) fn render_transcript(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if self.transcript_dirty {
            self.transcript_items =
                Arc::new(desktop_transcript_items(&self.runtime.chat().transcript));
            self.transcript_dirty = false;
        }
        let items = self.transcript_items.clone();
        if items.is_empty() {
            return self.render_empty_transcript(cx);
        }

        let namespace = format!("conversation-{}", self.transcript_epoch);
        let children = items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let rendered = self.render_desktop_transcript_item(&namespace, index, item, cx);
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

    fn render_empty_transcript(&self, cx: &mut Context<Self>) -> AnyElement {
        let suggestions = [
            (
                IconName::Layers,
                "Map the desktop adapter onto the shared front runtime",
            ),
            (
                IconName::Search,
                "Review the workspace for boundary violations",
            ),
            (IconName::Terminal, "Run workspace checks and fix failures"),
        ];

        div()
            .w_full()
            .pt(px(62.))
            .flex()
            .flex_col()
            .items_center()
            .child(
                div()
                    .size(px(48.))
                    .rounded_full()
                    .border_1()
                    .border_color(rgba(ACCENT_BORDER))
                    .bg(rgba(ACCENT_SOFT))
                    .flex()
                    .items_center()
                    .justify_center()
                    .font_family("Georgia")
                    .text_size(px(27.))
                    .text_color(rgb(ACCENT))
                    .child("α"),
            )
            .child(
                div()
                    .mt(px(16.))
                    .text_size(px(18.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(TEXT))
                    .child("Start with a task"),
            )
            .child(
                div()
                    .mt(px(6.))
                    .text_size(px(10.5))
                    .text_color(rgb(MUTED))
                    .child(
                        "Describe a change, ask for an investigation, or reference a file in the workspace.",
                    ),
            )
            .child(
                div()
                    .mt(px(18.))
                    .w_full()
                    .max_w(px(430.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .children(suggestions.into_iter().map(|(icon, prompt)| {
                        Button::new(format!("prompt-suggestion:{prompt}"))
                            .child(
                                div()
                                    .w_full()
                                    .min_h(px(40.))
                                    .px(px(10.))
                                    .flex()
                                    .items_center()
                                    .gap(px(9.))
                                    .child(
                                        Icon::new(icon)
                                            .size(px(14.))
                                            .text_color(rgb(ACCENT)),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .text_size(px(9.5))
                                            .text_color(rgb(TEXT_SECONDARY))
                                            .child(prompt),
                                    ),
                            )
                            .ghost()
                            .w_full()
                            .h(px(40.))
                            .rounded(px(18.))
                            .border_1()
                            .border_color(rgba(BORDER_SUBTLE))
                            .bg(rgb(SURFACE))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.composer.update(cx, |state, cx| {
                                    state.set_value(prompt, window, cx);
                                });
                                let focus = this.composer.read(cx).focus_handle(cx);
                                window.focus(&focus, cx);
                                cx.notify();
                            }))
                    })),
            )
            .into_any_element()
    }

    fn render_desktop_transcript_item(
        &self,
        namespace: &str,
        index: usize,
        item: &DesktopTranscriptItem,
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
                self.render_thinking(index, text.clone(), false, cx)
            }
            DesktopTranscriptItem::StreamingThinking(text) => {
                self.render_thinking(index, text.clone(), true, cx)
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
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let expanded = self.thinking_expanded;
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                Button::new("toggle-thinking")
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(7.))
                            .child(icon_badge(IconName::Sparkles, ACCENT, ACCENT_SOFT, 18.))
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(rgb(MUTED))
                                    .child("Thought"),
                            )
                            .child(
                                Icon::new(if expanded {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                })
                                .size(px(11.))
                                .text_color(rgb(FAINT)),
                            ),
                    )
                    .ghost()
                    .h(px(26.))
                    .px(px(7.))
                    .rounded_full()
                    .on_click(cx.listener(|this, _, _, cx| {
                        let follow = transcript_is_at_bottom(&this.transcript_scroll);
                        this.thinking_expanded = !this.thinking_expanded;
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
        let Some(latest) = tools.last() else {
            return div().into_any_element();
        };
        let expanded = self.expanded_tool_group.as_deref() == Some(id);
        let group_id = id.to_owned();
        let history_rows = tools
            .iter()
            .map(|tool| {
                let status = tool_status_label(tool.status);
                let color = tool_status_color(tool.status);
                div()
                    .rounded(px(16.))
                    .bg(rgb(SURFACE_INSET))
                    .px(px(10.))
                    .py(px(8.))
                    .flex()
                    .items_start()
                    .gap(px(9.))
                    .child(icon_badge(
                        tool_status_icon(tool.status),
                        color,
                        ACCENT_SOFT,
                        22.,
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
                                    .text_size(px(10.5))
                                    .text_color(rgb(TEXT_SECONDARY))
                                    .child(tool.name.clone()),
                            )
                            .when_some(tool.content.as_deref(), |this, content| {
                                this.child(
                                    div()
                                        .id(format!("tool-scroll-shell:{}", tool.id))
                                        .relative()
                                        .mt(px(2.))
                                        .max_h(px(160.))
                                        .smooth_scroll_shell(ScrollRegion::ToolOutput, cx)
                                        .child(
                                            div()
                                                .id(format!("tool-scroll:{}", tool.id))
                                                .w_full()
                                                .max_h(px(160.))
                                                .overflow_y_scroll()
                                                .track_scroll(&self.tool_scroll)
                                                .pr(px(SCROLLBAR_GUTTER))
                                                .font_family("monospace")
                                                .text_size(px(8.5))
                                                .line_height(px(15.))
                                                .text_color(rgb(MUTED))
                                                .child(tool_detail_content(content)),
                                        )
                                        .child(scrollbar_rail(
                                            format!("tool:{}", tool.id),
                                            &self.tool_scroll,
                                        )),
                                )
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(8.5))
                            .text_color(rgb(color))
                            .child(status),
                    )
                    .into_any_element()
            })
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
                            .child(
                                Icon::new(if expanded {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                })
                                .size(px(12.))
                                .text_color(rgb(FAINT)),
                            ),
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
                                this.bump_overlay_generation();
                                this.reset_scroll(ScrollRegion::ToolOutput);
                                Some(group_id.clone())
                            };
                        if follow {
                            this.scroll_to_bottom(ScrollRegion::Transcript);
                        }
                        cx.notify();
                    })),
            )
            .when(expanded, |this| {
                this.child(popover_enter(
                    div()
                        .p(px(4.))
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .children(history_rows),
                    format!("tool-expansion:{id}:{}", self.overlay_generation),
                ))
            })
            .when(!expanded, |this| {
                this.child(enter_from_below(
                    div()
                        .rounded(px(16.))
                        .px(px(10.))
                        .py(px(8.))
                        .flex()
                        .items_center()
                        .gap(px(9.))
                        .child(icon_badge(
                            tool_status_icon(latest.status),
                            tool_status_color(latest.status),
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
                                        .child(latest.name.clone()),
                                )
                                .when_some(latest.content.as_deref(), |this, content| {
                                    this.child(
                                        div()
                                            .truncate()
                                            .text_size(px(9.5))
                                            .text_color(rgb(MUTED))
                                            .child(compact_tool_content(content)),
                                    )
                                }),
                        )
                        .child(
                            Icon::new(IconName::ChevronRight)
                                .size(px(12.))
                                .text_color(rgb(FAINT)),
                        ),
                    format!("tool-latest:{id}:{}", latest.id),
                ))
            })
            .into_any_element()
    }
}

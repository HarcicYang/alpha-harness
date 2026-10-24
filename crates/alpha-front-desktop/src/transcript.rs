use super::*;

impl AlphaDesktop {
    pub(crate) fn render_transcript(&mut self, cx: &mut Context<Self>) -> AnyElement {
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

    pub(crate) fn render_desktop_transcript_item(
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

    pub(crate) fn render_tool_group(
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

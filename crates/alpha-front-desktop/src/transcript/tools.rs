use super::*;

impl AlphaDesktop {
    pub(super) fn render_tool_group(
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
                                    .child(format!("{} tool calls", tools.len())),
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
        if !self.expanded_tool_groups.remove(group_id) {
            self.bump_overlay_generation();
            self.expanded_tool_groups.insert(group_id.to_owned());
        }
        for tool_id in tool_ids {
            self.reset_tool_scroll(*tool_id);
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
                        this.bump_overlay_generation();
                        let state = this
                            .tool_outputs
                            .get_mut(&tool_id)
                            .expect("tool output state is synchronized before interaction");
                        state.expanded = !state.expanded;
                        this.reset_tool_scroll(tool_id);
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

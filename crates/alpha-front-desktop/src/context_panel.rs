use super::*;

impl AlphaDesktop {
    pub(crate) fn render_context_panel(
        &self,
        context: &ContextView,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
}

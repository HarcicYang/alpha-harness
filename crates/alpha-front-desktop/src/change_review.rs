use super::*;
use crate::helpers::*;

impl AlphaDesktop {
    pub(crate) fn render_change_review(
        &self,
        context: &ContextView,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
                                    .bg(rgba(ACCENT_SOFT))
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
            .p(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .child(div().absolute().inset_0().bg(rgba(SCRIM)))
            .child(
                div()
                    .relative()
                    .w(px(860.))
                    .max_w_full()
                    .h(px(620.))
                    .max_h_full()
                    .rounded(px(28.))
                    .border_1()
                    .border_color(rgba(BORDER))
                    .bg(rgb(SURFACE))
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
                            .border_color(rgba(BORDER_SUBTLE))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(10.))
                                    .child(icon_badge(
                                        IconName::FilePen,
                                        ACCENT,
                                        ACCENT_SOFT,
                                        32.,
                                    ))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .child(
                                                div()
                                                    .text_size(px(13.))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .child("Workspace changes"),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(9.5))
                                                    .text_color(rgb(MUTED))
                                                    .child(
                                                        "Files inferred from completed session tools",
                                                    ),
                                            ),
                                    ),
                            )
                            .child(
                                icon_button("close-change-review", IconName::X, "Close")
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
                                                        div().text_color(rgb(ACCENT)).child(
                                                            format!("+{}", change.added_lines),
                                                        ),
                                                    )
                                                    .child(div().text_color(rgb(MUTED)).child(
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
                                    .accent()
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
}

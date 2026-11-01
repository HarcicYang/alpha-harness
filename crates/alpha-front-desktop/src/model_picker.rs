use super::*;

impl AlphaDesktop {
    pub(crate) fn render_model_picker(
        &self,
        viewport_height: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let query = self.model_search.read(cx).value().to_lowercase();
        let query = query.trim();
        let chat = self.runtime.chat();
        let choices = self
            .model_choices
            .iter()
            .filter(|choice| {
                query.is_empty()
                    || choice.model_name.to_lowercase().contains(query)
                    || choice.model.to_lowercase().contains(query)
                    || choice.provider_name.to_lowercase().contains(query)
                    || choice.provider.to_lowercase().contains(query)
            })
            .cloned()
            .collect::<Vec<_>>();
        let mut rows = Vec::new();
        let mut previous_provider: Option<String> = None;
        for choice in choices {
            if previous_provider.as_deref() != Some(choice.provider_name.as_str()) {
                previous_provider = Some(choice.provider_name.clone());
                rows.push(
                    div()
                        .h(px(28.))
                        .px(px(8.))
                        .flex()
                        .items_center()
                        .justify_between()
                        .text_size(px(9.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(MUTED))
                        .child(choice.provider_name.clone())
                        .child(
                            div()
                                .font_family("monospace")
                                .text_size(px(8.))
                                .text_color(rgb(FAINT))
                                .child(
                                    self.model_choices
                                        .iter()
                                        .filter(|item| item.provider_name == choice.provider_name)
                                        .count()
                                        .to_string(),
                                ),
                        )
                        .into_any_element(),
                );
            }
            let selected = chat.provider == choice.provider && chat.model == choice.model;
            let initials = provider_initials(&choice.provider_name);
            let current = if selected { " CURRENT" } else { "" };
            let metadata = format!(
                "{} context · {}",
                if choice.context_window == 0 {
                    "Unknown".to_owned()
                } else {
                    format_token_count(choice.context_window)
                },
                choice
                    .reasoning_levels
                    .last()
                    .map(|level| display_value(level.to_string()))
                    .unwrap_or_else(|| "Auto".to_owned())
            );
            let choice_for_click = choice.clone();
            rows.push(
                Button::new(format!("model:{}:{}", choice.provider, choice.model))
                    .child(
                        div()
                            .w_full()
                            .min_h(px(58.))
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(provider_avatar(&initials, selected))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .flex()
                                    .flex_col()
                                    .items_start()
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(6.))
                                            .child(
                                                div()
                                                    .text_size(px(10.5))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(rgb(if selected {
                                                        TEXT
                                                    } else {
                                                        TEXT_SECONDARY
                                                    }))
                                                    .child(choice.model_name),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(7.))
                                                    .font_family("monospace")
                                                    .text_color(rgb(if selected {
                                                        ACCENT
                                                    } else {
                                                        FAINT
                                                    }))
                                                    .child(current),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(8.5))
                                            .text_color(rgb(MUTED))
                                            .child(metadata),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(8.))
                                    .font_family("monospace")
                                    .text_color(rgb(FAINT))
                                    .child(initials),
                            ),
                    )
                    .ghost()
                    .selected(selected)
                    .w_full()
                    .h(px(58.))
                    .rounded(px(18.))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_model(&choice_for_click, cx);
                        this.focus_composer(window, cx);
                    }))
                    .into_any_element(),
            );
        }
        if rows.is_empty() {
            rows.push(
                div()
                    .py(px(36.))
                    .text_center()
                    .text_size(px(10.))
                    .text_color(rgb(MUTED))
                    .child("No matching models")
                    .into_any_element(),
            );
        }

        let dialog = div()
            .relative()
            .w(px(680.))
            .max_w_full()
            .h(px(714.))
            .max_h(px((viewport_height - 56.).max(360.)))
            .rounded(px(28.))
            .border_1()
            .border_color(rgba(BORDER))
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
                    .border_b_1()
                    .border_color(rgba(BORDER_SUBTLE))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(icon_badge(IconName::Sparkles, ACCENT, ACCENT_SOFT, 34.))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child("Models"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(9.5))
                                            .text_color(rgb(MUTED))
                                            .child("Select a route and reasoning effort"),
                                    ),
                            ),
                    )
                    .child(icon_button("close-models", IconName::X, "Close").on_click(
                        cx.listener(|this, _, window, cx| {
                            this.composer_menu = None;
                            this.focus_composer(window, cx);
                            cx.notify();
                        }),
                    )),
            )
            .child(
                div()
                    .h(px(54.))
                    .flex_none()
                    .px(px(16.))
                    .flex()
                    .items_center()
                    .gap(px(9.))
                    .border_b_1()
                    .border_color(rgba(BORDER_SUBTLE))
                    .child(
                        Icon::new(IconName::Search)
                            .size(px(14.))
                            .text_color(rgb(MUTED)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .rounded_full()
                            .border_1()
                            .border_color(rgba(BORDER))
                            .bg(rgb(SURFACE))
                            .px(px(10.))
                            .child(
                                Textarea::new(&self.model_search)
                                    .appearance(false)
                                    .bordered(false)
                                    .h(px(38.))
                                    .aria_label("Search models"),
                            ),
                    ),
            )
            .child(
                div()
                    .id("model-list-shell")
                    .relative()
                    .flex_1()
                    .min_h(px(0.))
                    .smooth_scroll_shell(ScrollRegion::ModelPicker, cx)
                    .child(
                        div()
                            .id("model-list")
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.model_scroll)
                            .p(px(9.))
                            .pr(px(SCROLLBAR_GUTTER))
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .children(rows),
                    )
                    .child(scrollbar_rail(
                        "model-picker".to_owned(),
                        &self.model_scroll,
                    )),
            )
            .child(
                div()
                    .h(px(52.))
                    .flex_none()
                    .px(px(16.))
                    .border_t_1()
                    .border_color(rgba(BORDER_SUBTLE))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(8.5))
                            .text_color(rgb(FAINT))
                            .child("Selecting a model applies it immediately"),
                    )
                    .child(
                        Button::new("cancel-models")
                            .label("Cancel")
                            .ghost()
                            .rounded_full()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.composer_menu = None;
                                this.focus_composer(window, cx);
                                cx.notify();
                            })),
                    ),
            );

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .p(px(28.))
            .child(backdrop_enter(
                div().absolute().inset_0().bg(rgba(SCRIM)).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        this.composer_menu = None;
                        this.focus_composer(window, cx);
                        cx.notify();
                    }),
                ),
                format!("models-backdrop:{}", self.overlay_generation),
            ))
            .child(modal_enter(
                dialog,
                format!("models-dialog:{}", self.overlay_generation),
            ))
            .into_any_element()
    }
}

fn provider_initials(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|part| part.chars().next())
        .take(3)
        .collect::<String>()
        .to_uppercase()
}

fn provider_avatar(initials: &str, selected: bool) -> AnyElement {
    div()
        .size(px(32.))
        .flex_none()
        .rounded_full()
        .border_1()
        .border_color(rgba(if selected { ACCENT_BORDER } else { BORDER }))
        .bg(if selected {
            rgba(ACCENT_SOFT)
        } else {
            rgb(SURFACE_INSET)
        })
        .flex()
        .items_center()
        .justify_center()
        .font_family("monospace")
        .text_size(px(8.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(if selected { ACCENT } else { TEXT_SECONDARY }))
        .child(initials.to_owned())
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::provider_initials;

    #[test]
    fn provider_initials_are_stable_for_multiword_names() {
        assert_eq!(provider_initials("OpenCode Go"), "OG");
        assert_eq!(provider_initials("OpenAI"), "O");
    }
}

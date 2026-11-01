use super::*;

impl AlphaDesktop {
    pub(crate) fn open_capabilities(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.bump_overlay_generation();
        let tools = self.runtime.chat().provider_tools.join(", ");
        self.capabilities_text.update(cx, |state, cx| {
            state.set_value(&tools, window, cx);
        });
        self.capabilities_open = true;
        self.command_palette_open = false;
        let focus = self.capabilities_text.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    pub(crate) fn render_capabilities(&self, cx: &mut Context<Self>) -> AnyElement {
        if !self.capabilities_open {
            return div().into_any_element();
        }

        let route = format!(
            "{} · {}",
            if self.runtime.chat().provider.is_empty() {
                "default"
            } else {
                &self.runtime.chat().provider
            },
            if self.runtime.chat().model.is_empty() {
                "default model"
            } else {
                &self.runtime.chat().model
            }
        );
        let dialog = div()
            .relative()
            .w(px(560.))
            .max_w_full()
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
                    .h(px(68.))
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
                            .child(icon_badge(
                                IconName::SlidersHorizontal,
                                ACCENT,
                                ACCENT_SOFT,
                                34.,
                            ))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child("Provider capabilities"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(9.5))
                                            .text_color(rgb(MUTED))
                                            .child(route),
                                    ),
                            ),
                    )
                    .child(
                        icon_button("close-capabilities", IconName::X, "Close")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.capabilities_open = false;
                                this.focus_composer(window, cx);
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .p(px(18.))
                    .flex()
                    .flex_col()
                    .gap(px(9.))
                    .child(
                        div()
                            .text_size(px(9.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(TEXT_SECONDARY))
                            .child("Provider-native tools"),
                    )
                    .child(
                        div()
                            .rounded(px(18.))
                            .border_1()
                            .border_color(rgba(BORDER))
                            .bg(rgb(SURFACE_INSET))
                            .p(px(8.))
                            .child(
                                Textarea::new(&self.capabilities_text)
                                    .appearance(false)
                                    .bordered(false)
                                    .h(px(72.))
                                    .aria_label("Provider-native tools"),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(8.5))
                            .line_height(px(14.))
                            .text_color(rgb(MUTED))
                            .child(
                                "Separate tool names with commas or spaces. An empty list disables provider-native tools.",
                            ),
                    ),
            )
            .child(
                div()
                    .h(px(52.))
                    .flex_none()
                    .px(px(14.))
                    .border_t_1()
                    .border_color(rgba(BORDER_SUBTLE))
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(6.))
                    .child(
                        Button::new("cancel-capabilities")
                            .label("Cancel")
                            .ghost()
                            .h(px(32.))
                            .rounded_full()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.capabilities_open = false;
                                this.focus_composer(window, cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("save-capabilities")
                            .label("Save capabilities")
                            .accent()
                            .h(px(32.))
                            .rounded_full()
                            .on_click(cx.listener(|this, _, window, cx| {
                                let tools = parse_capabilities_text(
                                    &this.capabilities_text.read(cx).value(),
                                );
                                match this.runtime.set_capabilities(tools) {
                                    Ok(()) => {
                                        this.local_status = Some(StatusMessage::success(
                                            "provider capabilities updated",
                                        ));
                                        this.capabilities_open = false;
                                        this.focus_composer(window, cx);
                                    }
                                    Err(error) => {
                                        this.local_status =
                                            Some(StatusMessage::error(error.to_string()));
                                    }
                                }
                                cx.notify();
                            })),
                    ),
            );

        div()
            .absolute()
            .inset_0()
            .p(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .child(backdrop_enter(
                div().absolute().inset_0().bg(rgba(SCRIM)).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        this.capabilities_open = false;
                        this.focus_composer(window, cx);
                        cx.notify();
                    }),
                ),
                format!("capabilities-backdrop:{}", self.overlay_generation),
            ))
            .child(modal_enter(
                dialog,
                format!("capabilities-dialog:{}", self.overlay_generation),
            ))
            .into_any_element()
    }
}

pub(crate) fn parse_capabilities_text(text: &str) -> Vec<String> {
    let mut tools = Vec::new();
    for tool in text
        .split([',', ' ', '\n', '\t'])
        .map(str::trim)
        .filter(|tool| !tool.is_empty())
    {
        if !tools.iter().any(|existing| existing == tool) {
            tools.push(tool.to_owned());
        }
    }
    tools
}

#[cfg(test)]
mod tests {
    use super::parse_capabilities_text;

    #[test]
    fn capabilities_parse_separators_and_remove_duplicates() {
        assert_eq!(
            parse_capabilities_text("web_search, code\nread web_search"),
            ["web_search", "code", "read"]
        );
        assert!(parse_capabilities_text(" , \n ").is_empty());
    }
}

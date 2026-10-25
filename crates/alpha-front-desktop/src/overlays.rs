use super::*;

impl AlphaDesktop {
    pub(crate) fn render_help(&self, cx: &mut Context<Self>) -> AnyElement {
        if !self.help_open {
            return div().into_any_element();
        }

        let dialog =
            div()
                .relative()
                .w(px(620.))
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
                                .child(icon_badge(IconName::Keyboard, ACCENT, ACCENT_SOFT, 34.))
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .child(
                                            div()
                                                .text_size(px(13.))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .child("Keyboard shortcuts"),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(9.5))
                                                .text_color(rgb(MUTED))
                                                .child("Desktop navigation and composer controls"),
                                        ),
                                ),
                        )
                        .child(icon_button("close-help", IconName::X, "Close").on_click(
                            cx.listener(|this, _, window, cx| {
                                this.help_open = false;
                                this.focus_composer(window, cx);
                                cx.notify();
                            }),
                        )),
                )
                .child(
                    div()
                        .grid()
                        .grid_cols(2)
                        .child(
                            div()
                                .p(px(18.))
                                .border_r_1()
                                .border_color(rgba(BORDER_SUBTLE))
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .mb(px(8.))
                                        .text_size(px(10.5))
                                        .font_weight(FontWeight::MEDIUM)
                                        .child("Navigation"),
                                )
                                .child(help_row("New session", "Ctrl N"))
                                .child(help_row("Sessions", "Ctrl R"))
                                .child(help_row("Command palette", "Ctrl K"))
                                .child(help_row("Provider settings", "Ctrl P")),
                        )
                        .child(
                            div()
                                .p(px(18.))
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .mb(px(8.))
                                        .text_size(px(10.5))
                                        .font_weight(FontWeight::MEDIUM)
                                        .child("Composer"),
                                )
                                .child(help_row("Send message", "Enter"))
                                .child(help_row("Insert newline", "Shift Enter"))
                                .child(help_row("Model route", "Ctrl L"))
                                .child(help_row("Reasoning effort", "Ctrl T")),
                        ),
                )
                .child(
                    div()
                        .h(px(46.))
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
                                .child("Shortcuts use Ctrl on Linux and Windows, Cmd on macOS"),
                        )
                        .child(
                            Button::new("done-help")
                                .accent()
                                .label("Done")
                                .h(px(32.))
                                .rounded_full()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.help_open = false;
                                    this.focus_composer(window, cx);
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
            .child(div().absolute().inset_0().bg(rgba(SCRIM)).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.help_open = false;
                    this.focus_composer(window, cx);
                    cx.notify();
                }),
            ))
            .child(modal_enter(dialog, "help-dialog"))
            .into_any_element()
    }
}

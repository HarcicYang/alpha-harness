use super::*;

impl AlphaDesktop {
    pub(crate) fn render_more_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        if !self.more_open {
            return div().into_any_element();
        }
        div()
            .absolute()
            .top(px(50.))
            .right(px(16.))
            .w(px(224.))
            .rounded(px(20.))
            .border_1()
            .border_color(rgb(BORDER))
            .bg(rgb(SURFACE_RAISED))
            .p(px(5.))
            .flex()
            .flex_col()
            .gap_1()
            .child(
                Button::new("more-providers")
                    .label("Provider settings")
                    .ghost()
                    .w_full()
                    .rounded(px(15.))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_providers(window, cx);
                    })),
            )
            .child(
                Button::new("more-new-session")
                    .label("New session")
                    .ghost()
                    .w_full()
                    .rounded(px(15.))
                    .on_click(cx.listener(|this, _, _, cx| this.new_session(cx))),
            )
            .child(
                Button::new("more-diagnostics")
                    .label("Core diagnostics")
                    .ghost()
                    .w_full()
                    .rounded(px(15.))
                    .on_click(cx.listener(|this, _, _, cx| {
                        let info = this.runtime.core_info();
                        this.local_status = Some(format!(
                            "core pid={} protocol={} instance={}",
                            info.pid, info.protocol, info.core_instance_id
                        ));
                        this.more_open = false;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("more-help")
                    .label("Keyboard shortcuts")
                    .ghost()
                    .w_full()
                    .rounded(px(15.))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.more_open = false;
                        this.help_open = true;
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    pub(crate) fn render_help(&self, cx: &mut Context<Self>) -> AnyElement {
        if !self.help_open {
            return div().into_any_element();
        }
        div()
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .child(div().absolute().inset_0().bg(rgba(0x00000099)))
            .child(
                div()
                    .relative()
                    .mt(px(90.))
                    .w(px(560.))
                    .rounded(px(24.))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(SURFACE_RAISED))
                    .shadow_lg()
                    .p(px(18.))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Keyboard shortcuts"),
                            )
                            .child(
                                Button::new("close-help")
                                    .label("×")
                                    .ghost()
                                    .rounded(px(999.))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.help_open = false;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(help_row("Send message", "Enter"))
                    .child(help_row("Insert newline", "Shift + Enter"))
                    .child(help_row("Model route", "Composer model pill"))
                    .child(help_row("Reasoning effort", "Composer effort pill"))
                    .child(help_row("Permission mode", "Composer mode pill"))
                    .child(help_row("Sessions", "Sessions button"))
                    .child(help_row("Commands", "/command in composer")),
            )
            .into_any_element()
    }
}

use super::*;

impl AlphaDesktop {
    pub(crate) fn render_command_palette(
        &self,
        commands: &[CommandSpec],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rows = commands
            .iter()
            .map(|command| {
                let insert = if command.takes_argument {
                    format!("/{} ", command.name)
                } else {
                    format!("/{}", command.name)
                };
                Button::new(format!("slash-command:{}", command.name))
                    .child(
                        div()
                            .w_full()
                            .min_h(px(36.))
                            .flex()
                            .items_center()
                            .gap(px(9.))
                            .child(icon_badge(
                                command_icon(command.name),
                                TEXT_SECONDARY,
                                SURFACE_ACTIVE,
                                22.,
                            ))
                            .child(
                                div()
                                    .w(px(154.))
                                    .flex_none()
                                    .font_family("monospace")
                                    .text_size(px(10.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(TEXT))
                                    .child(command.usage),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .truncate()
                                    .text_size(px(9.5))
                                    .text_color(rgb(MUTED))
                                    .child(command.description),
                            ),
                    )
                    .ghost()
                    .w_full()
                    .h(px(36.))
                    .rounded(px(14.))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.composer.update(cx, |state, cx| {
                            state.set_value(&insert, window, cx);
                        });
                        let focus = this.composer.read(cx).focus_handle(cx);
                        window.focus(&focus, cx);
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect::<Vec<_>>();

        div()
            .absolute()
            .bottom(px(142.))
            .left(px(20.))
            .right(px(20.))
            .flex()
            .justify_center()
            .child(popover_enter(
                div()
                    .id("slash-palette-shell")
                    .relative()
                    .w(px(650.))
                    .max_w_full()
                    .h(px(360.))
                    .max_h_full()
                    .rounded(px(22.))
                    .border_1()
                    .border_color(rgba(BORDER))
                    .bg(rgb(SURFACE_RAISED))
                    .shadow_lg()
                    .overflow_hidden()
                    .occlude()
                    .smooth_scroll_shell(ScrollRegion::SlashCommands, cx)
                    .child(
                        div()
                            .id("slash-palette-scroll")
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.slash_scroll)
                            .p(px(5.))
                            .pr(px(SCROLLBAR_GUTTER))
                            .flex()
                            .flex_col()
                            .gap(px(1.))
                            .child(
                                div()
                                    .px(px(8.))
                                    .py(px(7.))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_size(px(9.))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(rgb(MUTED))
                                            .child("Commands"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(8.))
                                            .text_color(rgb(FAINT))
                                            .child("Tab to complete"),
                                    ),
                            )
                            .children(rows),
                    )
                    .child(scrollbar_rail(
                        "slash-commands".to_owned(),
                        &self.slash_scroll,
                    )),
                format!("slash-command-palette:{}", self.overlay_generation),
            ))
            .into_any_element()
    }
}

fn command_icon(name: &str) -> IconName {
    match name {
        "provider" => IconName::Server,
        "model" => IconName::Sparkles,
        "thinking" | "effort" => IconName::Gauge,
        "mode" => IconName::ShieldCheck,
        "confirm" => IconName::Check,
        "discard" => IconName::X,
        "capabilities" => IconName::SlidersHorizontal,
        "new" => IconName::MessageSquarePlus,
        "resume" | "sessions" => IconName::Clock,
        "diagnostics" => IconName::Cpu,
        "help" => IconName::Keyboard,
        "quit" => IconName::CircleX,
        _ => IconName::SquarePen,
    }
}

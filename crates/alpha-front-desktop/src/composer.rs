use super::*;

impl AlphaDesktop {
    pub(crate) fn render_composer_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(menu) = self.composer_menu else {
            return div().into_any_element();
        };
        let chat = self.runtime.chat();
        let panel = div()
            .when(menu == ComposerMenu::Model, |this| this.w(px(480.)))
            .when(menu != ComposerMenu::Model, |this| this.w(px(320.)))
            .max_h(px(430.))
            .overflow_y_scrollbar()
            .rounded(px(22.))
            .border_1()
            .border_color(rgb(BORDER))
            .bg(rgb(SURFACE_RAISED))
            .p(px(6.))
            .flex()
            .flex_col()
            .gap_1();

        match menu {
            ComposerMenu::Mode => panel
                .child(menu_label("Permission mode"))
                .children(
                    [
                        PermissionMode::Read,
                        PermissionMode::Auto,
                        PermissionMode::Reviewed,
                        PermissionMode::Free,
                    ]
                    .into_iter()
                    .map(|mode| {
                        let selected = chat.mode == mode;
                        let label = match mode {
                            PermissionMode::Read => "Read",
                            PermissionMode::Auto => "Auto",
                            PermissionMode::Reviewed => "Reviewed",
                            PermissionMode::Free => "Free",
                        };
                        Button::new(format!("mode:{label}"))
                            .label(label)
                            .selected(selected)
                            .ghost()
                            .w_full()
                            .rounded(px(16.))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.select_mode(mode, cx);
                            }))
                    }),
                )
                .into_any_element(),
            ComposerMenu::Model => panel
                .child(menu_label("Model route"))
                .children(self.model_choices.iter().map(|choice| {
                    let selected = chat.provider == choice.provider && chat.model == choice.model;
                    let levels = choice
                        .reasoning_levels
                        .iter()
                        .map(|level| level.as_str())
                        .collect::<Vec<_>>()
                        .join(" / ");
                    Button::new(format!("model:{}:{}", choice.provider, choice.model))
                        .child(
                            div()
                                .w_full()
                                .h_full()
                                .flex()
                                .flex_col()
                                .items_start()
                                .justify_center()
                                .gap_1()
                                .child(
                                    div()
                                        .w_full()
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
                                                .text_color(rgb(TEXT))
                                                .child(format!(
                                                    "{}  ·  {}",
                                                    choice.provider_name, choice.model_name
                                                )),
                                        )
                                        .child(
                                            div()
                                                .flex_none()
                                                .text_size(px(8.))
                                                .text_color(rgb(if selected {
                                                    0x72d69a
                                                } else {
                                                    MUTED
                                                }))
                                                .child(if selected { "CURRENT" } else { "" }),
                                        ),
                                )
                                .child(
                                    div()
                                        .w_full()
                                        .truncate()
                                        .text_size(px(9.))
                                        .font_family("monospace")
                                        .text_color(rgb(MUTED))
                                        .child(format!(
                                            "{} · {} ctx · {} out · {}",
                                            choice.model,
                                            if choice.context_window == 0 {
                                                "unknown".to_owned()
                                            } else {
                                                format_token_count(choice.context_window)
                                            },
                                            if choice.max_tokens == 0 {
                                                "unknown".to_owned()
                                            } else {
                                                format_token_count(choice.max_tokens)
                                            },
                                            if levels.is_empty() {
                                                "auto".to_owned()
                                            } else {
                                                levels
                                            },
                                        )),
                                ),
                        )
                        .selected(selected)
                        .ghost()
                        .w_full()
                        .h(px(58.))
                        .p(px(6.))
                        .rounded(px(16.))
                        .on_click(cx.listener({
                            let choice = choice.clone();
                            move |this, _, _, cx| this.select_model(&choice, cx)
                        }))
                }))
                .into_any_element(),
            ComposerMenu::Effort => {
                let levels =
                    supported_reasoning_levels(&self.model_choices, &chat.provider, &chat.model);
                panel
                    .child(menu_label("Reasoning effort"))
                    .children(levels.into_iter().map(|level| {
                        let selected = chat.reasoning == level;
                        Button::new(format!("effort:{}", level.as_str()))
                            .label(level.as_str())
                            .selected(selected)
                            .ghost()
                            .w_full()
                            .rounded(px(16.))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.select_effort(level, cx);
                            }))
                    }))
                    .into_any_element()
            }
        }
    }

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
                Button::new(format!("command-palette:{}", command.name))
                    .child(
                        div()
                            .w_full()
                            .min_h(px(40.))
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .w(px(184.))
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
                                    .text_size(px(10.))
                                    .text_color(rgb(MUTED))
                                    .child(command.description),
                            ),
                    )
                    .ghost()
                    .w_full()
                    .rounded(px(16.))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let value = insert.clone();
                        this.composer.update(cx, |state, cx| {
                            state.set_value(&value, window, cx);
                        });
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect::<Vec<_>>();

        div()
            .absolute()
            .bottom(px(126.))
            .left(px(0.))
            .right(px(0.))
            .when(self.context_open, |this| this.pr(px(354.)))
            .flex()
            .justify_center()
            .child(
                div()
                    .w(px(620.))
                    .max_w_full()
                    .max_h(px(360.))
                    .overflow_y_scrollbar()
                    .rounded(px(24.))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(SURFACE_RAISED))
                    .shadow_lg()
                    .p(px(6.))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .px(px(8.))
                            .py(px(6.))
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
                                    .text_color(rgb(MUTED))
                                    .child("Click to insert"),
                            ),
                    )
                    .children(rows),
            )
            .into_any_element()
    }

    pub(crate) fn submit_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.composer.read(cx).value().trim().to_owned();
        if value.is_empty() {
            if self.runtime.chat().running_turn.is_some()
                && let Err(error) = self.runtime.cancel()
            {
                self.local_status = Some(error.to_string());
            }
            cx.notify();
            return;
        }

        let mut outcome = None;
        let result = if value.starts_with('/') {
            self.runtime
                .execute_command(parse_command(&value))
                .map(|command| {
                    outcome = Some(command);
                })
        } else {
            self.runtime.submit_prompt(&value)
        };

        match result {
            Ok(()) => {
                self.local_status = None;
                self.transcript_scroll.scroll_to_bottom();
                match outcome {
                    Some(CommandOutcome::OpenProviders) => self.open_providers(window, cx),
                    Some(CommandOutcome::OpenModels) => {
                        self.open_composer_menu(ComposerMenu::Model);
                    }
                    Some(CommandOutcome::OpenThinking) => {
                        self.open_composer_menu(ComposerMenu::Effort);
                    }
                    Some(CommandOutcome::OpenSessions) => self.open_sessions(window, cx),
                    Some(CommandOutcome::Help) => self.help_open = true,
                    Some(CommandOutcome::Quit) => cx.quit(),
                    _ => {}
                }
                self.composer
                    .update(cx, |state, cx| state.set_value("", window, cx));
            }
            Err(error) => self.local_status = Some(error.to_string()),
        }
        cx.notify();
    }
}

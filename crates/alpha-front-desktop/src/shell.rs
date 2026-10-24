use super::*;

impl Render for AlphaDesktop {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (
            workspace,
            model,
            effort,
            mode,
            running,
            status,
            interaction,
            queued_steers,
            has_transcript,
        ) = {
            let chat = self.runtime.chat();
            let model = if chat.model.is_empty() {
                "unset".to_owned()
            } else {
                chat.model.clone()
            };
            let queued_steers = chat
                .transcript
                .iter()
                .filter_map(|item| match item {
                    TranscriptItem::QueuedSteer { text, .. } => Some(text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            (
                workspace_name(&chat.workspace),
                model,
                chat.reasoning.to_string(),
                chat.mode.to_string(),
                chat.running_turn.is_some(),
                self.local_status.clone().or_else(|| chat.status.clone()),
                chat.interaction.clone(),
                queued_steers,
                !chat.transcript.is_empty(),
            )
        };
        let context = self.runtime.context();
        let transcript_at_bottom = transcript_is_at_bottom(&self.transcript_scroll);
        let jump_to_latest_hidden = self
            .jump_to_latest_hidden_until
            .is_some_and(|deadline| Instant::now() < deadline);
        let composer_value = self.composer.read(cx).value().trim_start().to_owned();
        let command_query = composer_value
            .strip_prefix('/')
            .filter(|value| !value.contains(char::is_whitespace))
            .map(str::to_owned);
        let slash_commands = command_query
            .as_deref()
            .map(command_suggestions)
            .unwrap_or_default();
        let show_command_palette = !slash_commands.is_empty()
            && self.composer_menu.is_none()
            && !self.sessions_open
            && !self.providers_open
            && !self.help_open
            && interaction.is_none();

        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(CANVAS))
            .text_color(rgb(TEXT))
            .child(
                div()
                    .h(px(56.))
                    .flex_none()
                    .px(px(22.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        Button::new("workspace")
                            .label(workspace)
                            .ghost()
                            .rounded(px(999.)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                Button::new("sessions")
                                    .label("Sessions")
                                    .ghost()
                                    .rounded(px(999.))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_sessions(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("context")
                                    .label("Context")
                                    .selected(self.context_open)
                                    .ghost()
                                    .rounded(px(999.))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.context_open = !this.context_open;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("commands")
                                    .label("More")
                                    .ghost()
                                    .rounded(px(999.))
                                    .selected(self.more_open)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.more_open = !this.more_open;
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .when(!queued_steers.is_empty(), |this| {
                this.child(
                    div()
                        .flex_none()
                        .px(px(20.))
                        .when(self.context_open, |this| this.pr(px(354.)))
                        .child(
                            div()
                                .w(px(780.))
                                .max_w_full()
                                .mx_auto()
                                .mb(px(8.))
                                .rounded(px(18.))
                                .border_1()
                                .border_color(rgb(0x5cc8e655))
                                .bg(rgb(0x0e2a31))
                                .px(px(12.))
                                .py(px(8.))
                                .flex()
                                .items_center()
                                .gap_3()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.))
                                        .flex()
                                        .flex_col()
                                        .child(
                                            div()
                                                .text_size(px(9.))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(ACCENT))
                                                .child("Queued steer"),
                                        )
                                        .child(
                                            div()
                                                .truncate()
                                                .text_size(px(10.))
                                                .text_color(rgb(TEXT_SECONDARY))
                                                .child(queued_steers.join("  ·  ")),
                                        ),
                                )
                                .child(
                                    Button::new("discard-steers")
                                        .label("Discard")
                                        .ghost()
                                        .rounded(px(999.))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            let _ = this.runtime.confirm_steers(false);
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    Button::new("run-steers")
                                        .label("Run next")
                                        .secondary()
                                        .rounded(px(999.))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            let _ = this.runtime.confirm_steers(true);
                                            cx.notify();
                                        })),
                                ),
                        ),
                )
            })
            .child(
                div()
                    .id("transcript")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .track_scroll(&self.transcript_scroll)
                    .when(self.context_open, |this| this.pr(px(354.)))
                    .child(
                        div()
                            .w(px(780.))
                            .max_w_full()
                            .mx_auto()
                            .px(px(24.))
                            .py(px(18.))
                            .child(self.render_transcript(cx))
                            .when_some(interaction, |this, interaction| {
                                let prompt = serde_json::to_string_pretty(&interaction.prompt)
                                    .unwrap_or_else(|_| interaction.prompt.to_string());
                                this.child(
                                    div()
                                        .mt_5()
                                        .rounded(px(24.))
                                        .border_1()
                                        .border_color(rgb(0x5cc8e6))
                                        .bg(rgb(SURFACE))
                                        .p(px(16.))
                                        .flex()
                                        .flex_col()
                                        .gap_3()
                                        .child(
                                            div()
                                                .text_size(px(13.))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .child("Permission requested"),
                                        )
                                        .child(
                                            div()
                                                .rounded(px(16.))
                                                .bg(rgb(SURFACE_RAISED))
                                                .p(px(10.))
                                                .font_family("monospace")
                                                .text_size(px(11.))
                                                .text_color(rgb(TEXT_SECONDARY))
                                                .child(prompt),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .justify_end()
                                                .gap_2()
                                                .child(
                                                    Button::new("deny")
                                                        .label("Deny")
                                                        .ghost()
                                                        .rounded(px(999.))
                                                        .on_click(cx.listener(|this, _, _, cx| {
                                                            let _ =
                                                                this.runtime.respond_interaction(
                                                                    InteractionDecision::Deny,
                                                                );
                                                            cx.notify();
                                                        })),
                                                )
                                                .child(
                                                    Button::new("allow-session")
                                                        .label("Allow session")
                                                        .secondary()
                                                        .rounded(px(999.))
                                                        .on_click(cx.listener(|this, _, _, cx| {
                                                            let _ =
                                                                this.runtime.respond_interaction(
                                                                    InteractionDecision::Session,
                                                                );
                                                            cx.notify();
                                                        })),
                                                )
                                                .child(
                                                    Button::new("allow-once")
                                                        .label("Allow once")
                                                        .custom(accent_button_variant(cx))
                                                        .rounded(px(999.))
                                                        .on_click(cx.listener(|this, _, _, cx| {
                                                            let _ =
                                                                this.runtime.respond_interaction(
                                                                    InteractionDecision::Once,
                                                                );
                                                            cx.notify();
                                                        })),
                                                ),
                                        ),
                                )
                            }),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .px(px(20.))
                    .pb(px(22.))
                    .when(self.context_open, |this| this.pr(px(354.)))
                    .child(
                        div()
                            .w(px(780.))
                            .max_w_full()
                            .mx_auto()
                            .rounded(px(28.))
                            .border_1()
                            .border_color(rgb(BORDER))
                            .bg(rgb(SURFACE))
                            .p(px(8.))
                            .child(
                                Textarea::new(&self.composer)
                                    .appearance(false)
                                    .bordered(false)
                                    .h(px(54.))
                                    .aria_label("Message"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_2()
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .child(
                                                div()
                                                    .h(px(28.))
                                                    .rounded(px(999.))
                                                    .border_1()
                                                    .border_color(rgb(BORDER))
                                                    .px(px(8.))
                                                    .flex()
                                                    .items_center()
                                                    .gap_1()
                                                    .when(running, |this| {
                                                        this.child(
                                                            Spinner::new()
                                                                .xsmall()
                                                                .color(rgb(ACCENT).into()),
                                                        )
                                                    })
                                                    .when(!running, |this| {
                                                        this.child(
                                                            div()
                                                                .size(px(6.))
                                                                .rounded_full()
                                                                .bg(rgb(0x72d69a)),
                                                        )
                                                    })
                                                    .child(
                                                        div()
                                                            .text_size(px(8.))
                                                            .font_weight(FontWeight::MEDIUM)
                                                            .text_color(if running {
                                                                rgb(ACCENT)
                                                            } else {
                                                                rgb(MUTED)
                                                            })
                                                            .child(if running {
                                                                "Working"
                                                            } else {
                                                                "Ready"
                                                            }),
                                                    ),
                                            )
                                            .child(
                                                Button::new("mode")
                                                    .label(mode)
                                                    .ghost()
                                                    .rounded(px(999.))
                                                    .selected(
                                                        self.composer_menu
                                                            == Some(ComposerMenu::Mode),
                                                    )
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.open_composer_menu(ComposerMenu::Mode);
                                                        cx.notify();
                                                    })),
                                            )
                                            .child(
                                                Button::new("model")
                                                    .label(model)
                                                    .ghost()
                                                    .rounded(px(999.))
                                                    .selected(
                                                        self.composer_menu
                                                            == Some(ComposerMenu::Model),
                                                    )
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.open_composer_menu(
                                                            ComposerMenu::Model,
                                                        );
                                                        cx.notify();
                                                    })),
                                            )
                                            .child(
                                                Button::new("effort")
                                                    .label(effort)
                                                    .ghost()
                                                    .rounded(px(999.))
                                                    .selected(
                                                        self.composer_menu
                                                            == Some(ComposerMenu::Effort),
                                                    )
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.open_composer_menu(
                                                            ComposerMenu::Effort,
                                                        );
                                                        cx.notify();
                                                    })),
                                            ),
                                    )
                                    .child(
                                        Button::new("send")
                                            .label(if running { "■" } else { "↑" })
                                            .custom(accent_button_variant(cx))
                                            .rounded(px(999.))
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.submit_composer(window, cx);
                                            })),
                                    ),
                            ),
                    ),
            )
            .when_some(status, |this, status| {
                this.child(
                    div()
                        .absolute()
                        .bottom(px(6.))
                        .left(px(0.))
                        .right(px(0.))
                        .text_center()
                        .text_size(px(10.))
                        .text_color(rgb(MUTED))
                        .child(status),
                )
            })
            .when(
                has_transcript && !transcript_at_bottom && !jump_to_latest_hidden,
                |this| {
                    this.child(
                        div()
                            .absolute()
                            .bottom(px(126.))
                            .right(px(if self.context_open { 370. } else { 18. }))
                            .child(
                                Button::new("jump-to-latest")
                                    .label("Jump to latest")
                                    .secondary()
                                    .rounded(px(999.))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.jump_to_latest_hidden_until =
                                            Some(Instant::now() + Duration::from_millis(250));
                                        this.transcript_scroll.scroll_to_bottom();
                                        cx.notify();
                                    })),
                            ),
                    )
                },
            )
            .when(show_command_palette, |this| {
                this.child(self.render_command_palette(&slash_commands, cx))
            })
            .when(self.composer_menu.is_some(), |this| {
                this.child(
                    div()
                        .absolute()
                        .bottom(px(126.))
                        .left(px(0.))
                        .right(px(0.))
                        .when(self.context_open, |this| this.pr(px(354.)))
                        .flex()
                        .justify_center()
                        .child(self.render_composer_menu(cx)),
                )
            })
            .child(self.render_more_menu(cx))
            .child(self.render_sessions(cx))
            .child(self.render_providers(cx))
            .child(self.render_help(cx))
            .child(self.render_context_panel(&context, cx))
            .child(self.render_change_review(&context, cx))
    }
}

use super::*;

impl AlphaDesktop {
    fn render_ambient_header(&self, workspace: String, cx: &mut Context<Self>) -> AnyElement {
        div()
            .h(px(HEADER_HEIGHT))
            .flex_none()
            .px(px(20.))
            .flex()
            .items_center()
            .justify_between()
            .child(
                Button::new("workspace")
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(7.))
                            .child(
                                Icon::new(IconName::Folder)
                                    .size(px(14.))
                                    .text_color(rgb(MUTED)),
                            )
                            .child(
                                div()
                                    .max_w(px(180.))
                                    .truncate()
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(workspace),
                            )
                            .child(
                                Icon::new(IconName::ChevronDown)
                                    .size(px(11.))
                                    .text_color(rgb(MUTED)),
                            ),
                    )
                    .ghost()
                    .h(px(32.))
                    .px(px(10.))
                    .rounded_full()
                    .border_1()
                    .border_color(rgba(BORDER_SUBTLE))
                    .bg(rgb(SURFACE))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_workspace_menu(window, cx);
                    })),
            )
            .child(
                div()
                    .p(px(3.))
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .rounded_full()
                    .border_1()
                    .border_color(rgba(BORDER_SUBTLE))
                    .bg(rgb(SURFACE))
                    .child(
                        icon_button("sessions", IconName::Clock, "Sessions").on_click(cx.listener(
                            |this, _, window, cx| {
                                this.open_sessions(window, cx);
                            },
                        )),
                    )
                    .child(
                        icon_button("context", IconName::Layers, "Context")
                            .selected(self.context_open)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.toggle_context();
                                cx.notify();
                            })),
                    )
                    .child(
                        icon_button("commands", IconName::Ellipsis, "Commands").on_click(
                            cx.listener(|this, _, window, cx| {
                                this.open_command_palette(window, cx);
                            }),
                        ),
                    ),
            )
            .into_any_element()
    }

    fn render_queued_steer(&self, queued_steers: &[String], cx: &mut Context<Self>) -> AnyElement {
        if queued_steers.is_empty() {
            return div().into_any_element();
        }

        let summary = queued_steers.join("  ·  ");
        enter_from_below(
            div()
                .w(px(CONTENT_WIDTH))
                .max_w_full()
                .mx_auto()
                .px(px(20.))
                .pb(px(7.))
                .child(
                    div()
                        .rounded(px(20.))
                        .border_1()
                        .border_color(rgba(ACCENT_BORDER))
                        .bg(rgba(ACCENT_SOFT))
                        .px(px(9.))
                        .py(px(8.))
                        .flex()
                        .items_center()
                        .gap(px(9.))
                        .child(icon_badge(
                            IconName::MessageSquarePlus,
                            ACCENT,
                            ACCENT_SOFT,
                            26.,
                        ))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .text_size(px(9.5))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(TEXT_SECONDARY))
                                        .child("Queued steer"),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(9.5))
                                        .text_color(rgb(MUTED))
                                        .child(summary.clone()),
                                ),
                        )
                        .child(
                            Button::new("discard-steers")
                                .label("Discard")
                                .ghost()
                                .h(px(28.))
                                .rounded_full()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    match this.runtime.confirm_steers(false) {
                                        Ok(()) => {
                                            this.local_status = Some("steer discarded".into())
                                        }
                                        Err(error) => this.local_status = Some(error.to_string()),
                                    }
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("run-steers")
                                .label("Run next")
                                .secondary()
                                .h(px(28.))
                                .rounded_full()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    match this.runtime.confirm_steers(true) {
                                        Ok(()) => {
                                            this.local_status = Some("steer queued to run".into())
                                        }
                                        Err(error) => this.local_status = Some(error.to_string()),
                                    }
                                    cx.notify();
                                })),
                        ),
                ),
            format!("queued-steer:{summary}"),
        )
    }

    fn render_status_toast(&self, status: &str) -> AnyElement {
        enter_from_below(
            div()
                .absolute()
                .right(px(18.))
                .bottom(px(132.))
                .w(px(310.))
                .rounded(px(20.))
                .border_1()
                .border_color(rgba(BORDER))
                .bg(rgb(SURFACE_RAISED))
                .shadow_lg()
                .px(px(10.))
                .py(px(9.))
                .flex()
                .items_center()
                .gap(px(9.))
                .child(icon_badge(IconName::CircleCheck, ACCENT, ACCENT_SOFT, 26.))
                .child(
                    div()
                        .min_w(px(0.))
                        .flex_1()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_size(px(9.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(TEXT))
                                .child("Updated"),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(px(8.5))
                                .text_color(rgb(MUTED))
                                .child(status.to_owned()),
                        ),
                ),
            format!("status-toast:{status}"),
        )
    }
}

impl Render for AlphaDesktop {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport_width: f32 = window.viewport_size().width.into();
        let viewport_height: f32 = window.viewport_size().height.into();
        let (workspace, status, interaction, queued_steers, has_transcript) = {
            let chat = self.runtime.chat();
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
                self.local_status.clone(),
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
            && !self.capabilities_open
            && !self.help_open
            && !self.workspace_menu_open
            && interaction.is_none();

        self.prepare_scroll_motion(ScrollRegion::Transcript, window, cx);
        if show_command_palette {
            self.prepare_scroll_motion(ScrollRegion::SlashCommands, window, cx);
        }
        if self.command_palette_open {
            self.prepare_scroll_motion(ScrollRegion::Commands, window, cx);
        }
        if self.workspace_menu_open {
            self.prepare_scroll_motion(ScrollRegion::WorkspaceList, window, cx);
            self.prepare_scroll_motion(ScrollRegion::WorkspaceSessions, window, cx);
        }
        if self.composer_menu == Some(ComposerMenu::Model) {
            self.prepare_scroll_motion(ScrollRegion::ModelPicker, window, cx);
        }
        if matches!(
            self.composer_menu,
            Some(ComposerMenu::Mode | ComposerMenu::Effort)
        ) {
            self.prepare_scroll_motion(ScrollRegion::ComposerMenu, window, cx);
        }
        if self.sessions_open {
            self.prepare_scroll_motion(ScrollRegion::Sessions, window, cx);
        }
        if self.providers_open {
            self.prepare_scroll_motion(ScrollRegion::ProviderKinds, window, cx);
            self.prepare_scroll_motion(ScrollRegion::Providers, window, cx);
        }
        if self.context_open {
            self.prepare_scroll_motion(ScrollRegion::Context, window, cx);
        }
        if self.change_review_open {
            self.prepare_scroll_motion(ScrollRegion::ChangeFiles, window, cx);
            self.prepare_scroll_motion(ScrollRegion::ChangeDiff, window, cx);
        }
        self.prepare_tool_scroll_motion(window, cx);

        div()
            .id("alpha-desktop")
            .key_context("AlphaDesktop")
            .track_focus(&self.focus_handle)
            .capture_key_down(cx.listener(Self::handle_desktop_key))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(rgb(CANVAS))
            .text_color(rgb(TEXT))
            .child(self.render_ambient_header(workspace, cx))
            .child(
                div()
                    .id("transcript-shell")
                    .relative()
                    .flex_1()
                    .min_h(px(0.))
                    .smooth_scroll_shell(ScrollRegion::Transcript, cx)
                    .child(
                        div()
                            .id("transcript")
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.transcript_scroll)
                            .pr(px(SCROLLBAR_GUTTER))
                            .child(
                                div()
                                    .w(px(CONTENT_WIDTH))
                                    .max_w_full()
                                    .mx_auto()
                                    .px(px(24.))
                                    .pt(px(14.))
                                    .pb(px(28.))
                                    .child(self.render_transcript(
                                        viewport_height,
                                        viewport_width,
                                        cx,
                                    ))
                                    .when_some(interaction, |this, interaction| {
                                        let animation_id =
                                            format!("permission:{:?}", interaction.id);
                                        this.child(enter_from_below(
                                            div().child(
                                                self.render_permission_card(interaction, cx),
                                            ),
                                            animation_id,
                                        ))
                                    }),
                            ),
                    )
                    .child(scrollbar_rail(
                        "transcript".to_owned(),
                        &self.transcript_scroll,
                    )),
            )
            .when(!queued_steers.is_empty(), |this| {
                this.child(self.render_queued_steer(&queued_steers, cx))
            })
            .child(self.render_composer(cx))
            .when_some(status, |this, status| {
                this.child(self.render_status_toast(&status))
            })
            .when(
                has_transcript && !transcript_at_bottom && !jump_to_latest_hidden,
                |this| {
                    this.child(
                        div().absolute().bottom(px(146.)).right(px(18.)).child(
                            Button::new("jump-to-latest")
                                .label("Jump to latest")
                                .secondary()
                                .rounded_full()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.jump_to_latest_hidden_until =
                                        Some(Instant::now() + Duration::from_millis(250));
                                    this.scroll_to_bottom(ScrollRegion::Transcript);
                                    cx.notify();
                                })),
                        ),
                    )
                },
            )
            .when(show_command_palette, |this| {
                this.child(self.render_command_palette(&slash_commands, cx))
            })
            .when(self.composer_menu == Some(ComposerMenu::Model), |this| {
                this.child(self.render_model_picker(viewport_height, cx))
            })
            .when(
                matches!(
                    self.composer_menu,
                    Some(ComposerMenu::Mode | ComposerMenu::Effort)
                ),
                |this| this.child(self.render_composer_popover(viewport_width, cx)),
            )
            .when(self.command_palette_open, |this| {
                this.child(self.render_global_command_palette(viewport_height, cx))
            })
            .child(self.render_capabilities(cx))
            .child(self.render_sessions(cx))
            .child(self.render_providers(cx))
            .child(self.render_help(cx))
            .when(self.context_open, |this| {
                this.child(backdrop_enter(
                    div().absolute().inset_0().on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.context_open = false;
                            this.change_review_open = false;
                            this.focus_composer(window, cx);
                            cx.notify();
                        }),
                    ),
                    format!("context-backdrop:{}", self.overlay_generation),
                ))
            })
            .child(self.render_context_panel(&context, cx))
            .child(self.render_change_review(&context, cx))
            .child(self.render_workspace_menu(cx))
    }
}

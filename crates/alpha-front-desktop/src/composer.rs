use super::*;

impl AlphaDesktop {
    fn render_composer_pill(
        &self,
        id: &'static str,
        icon: IconName,
        label: String,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> Button {
        let _ = self;
        Button::new(id)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .child(if id == "mode" {
                        pulse_dot()
                    } else {
                        Icon::new(icon)
                            .size(px(12.))
                            .text_color(rgb(ACCENT))
                            .into_any_element()
                    })
                    .child(label),
            )
            .ghost()
            .selected(selected)
            .h(px(27.))
            .px(px(9.))
            .rounded_full()
            .border_1()
            .border_color(rgba(BORDER_SUBTLE))
            .bg(rgb(SURFACE_RAISED))
            .text_size(px(9.5))
            .on_click(cx.listener(move |this, _, window, cx| {
                let menu = match id {
                    "mode" => ComposerMenu::Mode,
                    "model" => ComposerMenu::Model,
                    "effort" => ComposerMenu::Effort,
                    _ => return,
                };
                this.open_composer_menu(menu, window, cx);
                cx.notify();
            }))
    }

    pub(crate) fn render_composer(&self, cx: &mut Context<Self>) -> AnyElement {
        let chat = self.runtime.chat();
        let model = chat
            .provider
            .is_empty()
            .then_some(chat.model.as_str())
            .filter(|model| !model.is_empty())
            .map(str::to_owned)
            .or_else(|| {
                self.runtime
                    .providers()
                    .model_choice(&chat.provider, &chat.model)
                    .map(|choice| choice.model_name)
            })
            .unwrap_or_else(|| {
                if chat.model.is_empty() {
                    "Model".to_owned()
                } else {
                    chat.model.clone()
                }
            });
        let mode = display_value(chat.mode.to_string());
        let effort = display_value(chat.reasoning.to_string());
        let running = chat.running_turn.is_some();
        let composer_value = self.composer.read(cx).value();
        let cancel_action = running && composer_value.trim().is_empty();
        let status = if running {
            "Working"
        } else if chat.status.as_deref() == Some("new session") {
            "New session draft"
        } else {
            "Ready"
        };

        div()
            .flex_none()
            .px(px(20.))
            .pb(px(22.))
            .child(
                div()
                    .relative()
                    .w(px(CONTENT_WIDTH))
                    .max_w_full()
                    .mx_auto()
                    .rounded(px(RADIUS_MD))
                    .border_1()
                    .border_color(rgba(BORDER))
                    .bg(rgb(SURFACE))
                    .p(px(8.))
                    .child(
                        Textarea::new(&self.composer)
                            .appearance(false)
                            .bordered(false)
                            .h(px(50.))
                            .aria_label("Message"),
                    )
                    .child(
                        div()
                            .h(px(34.))
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(10.))
                            .child(
                                div()
                                    .min_w(px(0.))
                                    .flex_1()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .child(
                                        Button::new("attach")
                                            .icon(Icon::new(IconName::Plus))
                                            .ghost()
                                            .compact()
                                            .size(px(28.))
                                            .rounded_full()
                                            .tooltip("Attach file")
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.attach_files(window, cx);
                                            })),
                                    )
                                    .child(self.render_composer_pill(
                                        "mode",
                                        IconName::CircleDot,
                                        mode,
                                        self.composer_menu == Some(ComposerMenu::Mode),
                                        cx,
                                    ))
                                    .child(self.render_composer_pill(
                                        "model",
                                        IconName::Sparkles,
                                        model,
                                        self.composer_menu == Some(ComposerMenu::Model),
                                        cx,
                                    ))
                                    .child(self.render_composer_pill(
                                        "effort",
                                        IconName::Gauge,
                                        effort,
                                        self.composer_menu == Some(ComposerMenu::Effort),
                                        cx,
                                    )),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .when(running, |this| {
                                        this.child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(5.))
                                                .child(streaming_dots())
                                                .text_size(px(9.))
                                                .text_color(rgb(FAINT))
                                                .child(status),
                                        )
                                    })
                                    .child(
                                        Button::new("send")
                                            .icon(Icon::new(if cancel_action {
                                                IconName::CircleStop
                                            } else {
                                                IconName::ArrowUp
                                            }))
                                            .when(cancel_action, |button| {
                                                button
                                                    .ghost()
                                                    .bg(rgb(SURFACE_ACTIVE))
                                                    .text_color(rgb(TEXT))
                                            })
                                            .when(!cancel_action, |button| button.accent())
                                            .size(px(36.))
                                            .rounded_full()
                                            .tooltip(if cancel_action {
                                                "Stop turn"
                                            } else {
                                                "Send message"
                                            })
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.submit_composer(window, cx);
                                            })),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }

    pub(crate) fn render_composer_popover(
        &self,
        viewport_width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(menu @ (ComposerMenu::Mode | ComposerMenu::Effort)) = self.composer_menu else {
            return div().into_any_element();
        };
        let content_left = ((viewport_width - CONTENT_WIDTH) / 2.).max(0.);
        let left = match menu {
            ComposerMenu::Mode => (content_left - 180.).max(12.),
            ComposerMenu::Effort => (content_left - 25.).max(12.),
            ComposerMenu::Model => return div().into_any_element(),
        };
        let width = if menu == ComposerMenu::Mode {
            272.
        } else {
            250.
        };
        let shell = div()
            .id(format!("composer-menu-shell:{menu:?}"))
            .relative()
            .w(px(width))
            .max_h(px(370.))
            .rounded(px(22.))
            .border_1()
            .border_color(rgba(BORDER))
            .bg(rgb(SURFACE_RAISED))
            .shadow_lg()
            .overflow_hidden()
            .occlude()
            .smooth_scroll_shell(ScrollRegion::ComposerMenu, cx);
        let panel = div()
            .id(format!("composer-menu-scroll:{menu:?}"))
            .w_full()
            .max_h(px(370.))
            .overflow_y_scroll()
            .track_scroll(&self.composer_menu_scroll)
            .p(px(5.))
            .pr(px(SCROLLBAR_GUTTER))
            .flex()
            .flex_col()
            .gap(px(1.));
        let panel = panel.child(menu_label(if menu == ComposerMenu::Mode {
            "Permission mode"
        } else {
            "Reasoning effort"
        }));

        let panel = match menu {
            ComposerMenu::Mode => panel.children(
                [
                    PermissionMode::Read,
                    PermissionMode::Auto,
                    PermissionMode::Reviewed,
                    PermissionMode::Free,
                ]
                .into_iter()
                .map(|mode| {
                    let selected = self.runtime.chat().mode == mode;
                    let icon = match mode {
                        PermissionMode::Read => IconName::BookOpen,
                        PermissionMode::Auto => IconName::CircleDot,
                        PermissionMode::Reviewed => IconName::ShieldCheck,
                        PermissionMode::Free => IconName::LockOpen,
                    };
                    let description = match mode {
                        PermissionMode::Read => "Read-only workspace sandbox",
                        PermissionMode::Auto => "Approvals only when required",
                        PermissionMode::Reviewed => "Model review before requesting access",
                        PermissionMode::Free => "Full access without prompts",
                    };
                    menu_option(
                        format!("mode:{}", mode),
                        icon,
                        display_value(mode.to_string()),
                        description,
                        selected,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_mode(mode, cx);
                    }))
                    .into_any_element()
                }),
            ),
            ComposerMenu::Effort => {
                let chat = self.runtime.chat();
                let levels =
                    supported_reasoning_levels(&self.model_choices, &chat.provider, &chat.model);
                panel.children(levels.into_iter().map(|level| {
                    let selected = chat.reasoning == level;
                    menu_option(
                        format!("effort:{}", level.as_str()),
                        IconName::Gauge,
                        display_value(level.to_string()),
                        reasoning_description(level),
                        selected,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_effort(level, cx);
                    }))
                    .into_any_element()
                }))
            }
            ComposerMenu::Model => unreachable!(),
        };
        let panel = shell.child(panel).child(scrollbar_rail(
            format!("composer-menu:{menu:?}"),
            &self.composer_menu_scroll,
        ));

        div()
            .absolute()
            .bottom(px(68.))
            .left(px(left))
            .child(popover_enter(
                panel,
                format!("composer-menu:{menu:?}:{}", self.overlay_generation),
            ))
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
                self.transcript_dirty = true;
                self.local_status = None;
                self.scroll_to_bottom(ScrollRegion::Transcript);
                match outcome {
                    Some(CommandOutcome::OpenProviders) => self.open_providers(window, cx),
                    Some(CommandOutcome::OpenModels) => {
                        self.open_composer_menu(ComposerMenu::Model, window, cx);
                    }
                    Some(CommandOutcome::OpenThinking) => {
                        self.open_composer_menu(ComposerMenu::Effort, window, cx);
                    }
                    Some(CommandOutcome::OpenSessions) => self.open_sessions(window, cx),
                    Some(CommandOutcome::Help) => self.open_help(),
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

    fn attach_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach files".into()),
        });
        cx.spawn_in(window, async move |this, cx| match receiver.await {
            Ok(Ok(Some(paths))) if !paths.is_empty() => {
                let _ = this.update_in(cx, |this, window, cx| {
                    let mut value = this.composer.read(cx).value().trim_end().to_owned();
                    for path in paths {
                        if !value.is_empty() {
                            value.push(' ');
                        }
                        value.push('@');
                        value.push_str(&path.display().to_string());
                    }
                    this.composer.update(cx, |state, cx| {
                        state.set_value(&value, window, cx);
                    });
                    this.focus_composer(window, cx);
                    cx.notify();
                });
            }
            Ok(Err(error)) => {
                let _ = this.update_in(cx, |this, _, cx| {
                    this.local_status = Some(format!("file picker failed: {error}"));
                    cx.notify();
                });
            }
            _ => {}
        })
        .detach();
    }
}

fn menu_option(
    id: String,
    icon: IconName,
    label: String,
    description: &'static str,
    selected: bool,
) -> Button {
    Button::new(id)
        .child(
            div()
                .w_full()
                .min_h(px(38.))
                .flex()
                .items_center()
                .gap(px(9.))
                .child(icon_badge(
                    icon,
                    if selected { ACCENT } else { MUTED },
                    if selected { ACCENT_SOFT } else { SURFACE_INSET },
                    24.,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .items_start()
                        .child(
                            div()
                                .text_size(px(10.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(if selected { TEXT } else { TEXT_SECONDARY }))
                                .child(label),
                        )
                        .child(
                            div()
                                .text_size(px(8.5))
                                .text_color(rgb(MUTED))
                                .child(description),
                        ),
                )
                .when(selected, |this| {
                    this.child(
                        Icon::new(IconName::Check)
                            .size(px(13.))
                            .text_color(rgb(ACCENT)),
                    )
                }),
        )
        .ghost()
        .selected(selected)
        .w_full()
        .h(px(38.))
        .rounded(px(16.))
}

fn reasoning_description(level: ReasoningLevel) -> &'static str {
    match level {
        ReasoningLevel::Auto => "Use the provider default",
        ReasoningLevel::Off => "Disable reasoning when supported",
        ReasoningLevel::Minimal => "Quickest reasoning",
        ReasoningLevel::Low => "Light reasoning",
        ReasoningLevel::Medium => "Balanced reasoning",
        ReasoningLevel::High => "Deep reasoning",
        ReasoningLevel::XHigh => "Extra-deep reasoning",
        ReasoningLevel::Max => "Maximum reasoning",
    }
}

use super::*;
use crate::composer::is_image_path;

impl AlphaDesktop {
    pub(crate) fn open_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.bump_overlay_generation();
        self.reset_scroll(ScrollRegion::Commands);
        self.command_palette_open = true;
        self.composer_menu = None;
        self.sessions_open = false;
        self.providers_open = false;
        self.capabilities_open = false;
        self.help_open = false;
        self.context_open = false;
        self.command_palette_query.update(cx, |state, cx| {
            state.set_value("", window, cx);
        });
        let focus = self.command_palette_query.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    fn run_desktop_command(
        &mut self,
        command: DesktopCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.command_palette_open = false;
        match command {
            DesktopCommand::NewSession => {
                self.new_session(cx);
                self.focus_composer(window, cx);
            }
            DesktopCommand::ResumeSession => self.open_sessions(window, cx),
            DesktopCommand::Model => self.open_composer_menu(ComposerMenu::Model, window, cx),
            DesktopCommand::Effort => {
                self.focus_composer(window, cx);
                self.open_composer_menu(ComposerMenu::Effort, window, cx);
            }
            DesktopCommand::Mode => {
                self.focus_composer(window, cx);
                self.open_composer_menu(ComposerMenu::Mode, window, cx);
            }
            DesktopCommand::Providers => self.open_providers(window, cx),
            DesktopCommand::Capabilities => self.open_capabilities(window, cx),
            DesktopCommand::ReviewChanges => {
                self.bump_overlay_generation();
                self.reset_scroll(ScrollRegion::Context);
                self.context_open = true;
                self.context_tab = ContextTab::Changes;
                if self.runtime.context().changes.is_empty() {
                    self.change_review_open = false;
                } else {
                    self.open_change_review(0);
                }
                self.focus_composer(window, cx);
            }
            DesktopCommand::Diagnostics => {
                let info = self.runtime.core_info();
                self.local_status = Some(format!(
                    "core pid={} · protocol={} · instance={}",
                    info.pid, info.protocol, info.core_instance_id
                ));
                self.focus_composer(window, cx);
            }
            DesktopCommand::Help => {
                self.open_help();
                self.focus_composer(window, cx);
            }
        }
        cx.notify();
    }

    pub(crate) fn handle_desktop_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.to_ascii_lowercase();
        if key == "escape" {
            let had_overlay = self.command_palette_open
                || self.composer_menu.is_some()
                || self.workspace_menu_open
                || self.sessions_open
                || self.providers_open
                || self.capabilities_open
                || self.help_open
                || self.context_open;
            if had_overlay {
                self.command_palette_open = false;
                self.composer_menu = None;
                self.workspace_menu_open = false;
                self.sessions_open = false;
                self.providers_open = false;
                self.capabilities_open = false;
                self.help_open = false;
                self.context_open = false;
                self.change_review_open = false;
                let focus = self.composer.read(cx).focus_handle(cx);
                window.focus(&focus, cx);
                cx.stop_propagation();
                cx.notify();
            }
            return;
        }

        if self.command_palette_open && matches!(key.as_str(), "enter" | "return") {
            let query = self.command_palette_query.read(cx).value();
            if let Some(command) = filtered_commands(&query).first().copied() {
                self.run_desktop_command(command.command, window, cx);
            }
            cx.stop_propagation();
            return;
        }

        if key == "tab" && !self.command_palette_open {
            let value = self.composer.read(cx).value();
            if let Some(query) = value
                .strip_prefix('/')
                .filter(|query| !query.contains(char::is_whitespace))
                && let Some(command) = command_suggestions(query).first()
            {
                let completed = if command.takes_argument {
                    format!("/{} ", command.name)
                } else {
                    format!("/{}", command.name)
                };
                self.composer.update(cx, |state, cx| {
                    state.set_value(&completed, window, cx);
                });
                cx.stop_propagation();
                cx.notify();
                return;
            }
        }

        if event.keystroke.modifiers.secondary() {
            let handled = match key.as_str() {
                "v" => {
                    let Some(item) = cx.read_from_clipboard() else {
                        return;
                    };
                    let mut attached = 0usize;
                    for entry in item.entries() {
                        match entry {
                            ClipboardEntry::Image(image) => {
                                self.runtime.attach_image_bytes(
                                    format!("clipboard-{attached}.{}", image.format().extension()),
                                    image.format().mime_type(),
                                    image.bytes().to_vec(),
                                );
                                attached = attached.saturating_add(1);
                            }
                            ClipboardEntry::ExternalPaths(paths) => {
                                for path in &paths.0 {
                                    if is_image_path(path)
                                        && self.runtime.attach_image(path).is_ok()
                                    {
                                        attached = attached.saturating_add(1);
                                    }
                                }
                            }
                            ClipboardEntry::String(_) => {}
                        }
                    }
                    if attached > 0 {
                        self.local_status = Some(format!(
                            "attached {attached} image{}",
                            if attached == 1 { "" } else { "s" }
                        ));
                        self.focus_composer(window, cx);
                        true
                    } else {
                        false
                    }
                }
                "n" => {
                    self.new_session(cx);
                    self.focus_composer(window, cx);
                    true
                }
                "r" => {
                    self.open_sessions(window, cx);
                    true
                }
                "l" => {
                    self.open_composer_menu(ComposerMenu::Model, window, cx);
                    true
                }
                "t" => {
                    self.focus_composer(window, cx);
                    self.open_composer_menu(ComposerMenu::Effort, window, cx);
                    true
                }
                "p" => {
                    self.open_providers(window, cx);
                    true
                }
                "k" => {
                    self.open_command_palette(window, cx);
                    true
                }
                _ => false,
            };
            if handled {
                cx.stop_propagation();
                cx.notify();
            }
            return;
        }

        if key == "?" {
            self.open_help();
            cx.stop_propagation();
            cx.notify();
        }
    }

    pub(crate) fn render_global_command_palette(
        &self,
        viewport_height: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let query = self.command_palette_query.read(cx).value().to_lowercase();
        let query = query.trim();
        let commands = filtered_commands(query);

        let mut result_rows = Vec::new();
        let mut previous_group = None;
        for (index, command) in commands.into_iter().enumerate() {
            let selected = index == 0;
            if previous_group != Some(command.group) {
                previous_group = Some(command.group);
                result_rows.push(
                    div()
                        .px(px(9.))
                        .pt(px(9.))
                        .pb(px(5.))
                        .text_size(px(9.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(MUTED))
                        .child(command.group)
                        .into_any_element(),
                );
            }
            result_rows.push(
                Button::new(format!("command:{:?}", command.command))
                    .child(
                        div()
                            .w_full()
                            .min_h(px(43.))
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(icon_badge(
                                command_icon_for_desktop(command.command),
                                if selected { ACCENT } else { TEXT_SECONDARY },
                                if selected {
                                    ACCENT_SOFT
                                } else {
                                    SURFACE_ACTIVE
                                },
                                27.,
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
                                            .text_color(rgb(if selected {
                                                TEXT
                                            } else {
                                                TEXT_SECONDARY
                                            }))
                                            .child(command.title),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(9.))
                                            .text_color(rgb(MUTED))
                                            .child(command.description),
                                    ),
                            )
                            .when_some(command.shortcut, |this, shortcut| {
                                this.child(
                                    div()
                                        .h(px(19.))
                                        .rounded(px(RADIUS_XS))
                                        .border_1()
                                        .border_color(rgba(BORDER))
                                        .bg(rgb(SURFACE_INSET))
                                        .px(px(5.))
                                        .flex()
                                        .items_center()
                                        .font_family("monospace")
                                        .text_size(px(8.))
                                        .text_color(rgb(MUTED))
                                        .child(shortcut),
                                )
                            }),
                    )
                    .ghost()
                    .selected(selected)
                    .w_full()
                    .h(px(43.))
                    .rounded(px(18.))
                    .border_1()
                    .border_color(if selected {
                        rgba(ACCENT_BORDER)
                    } else {
                        rgba(0x00000000)
                    })
                    .bg(if selected {
                        rgba(ACCENT_SOFT)
                    } else {
                        rgba(0x00000000)
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.run_desktop_command(command.command, window, cx);
                    }))
                    .into_any_element(),
            );
        }
        if result_rows.is_empty() {
            result_rows.push(
                div()
                    .py(px(36.))
                    .text_center()
                    .text_size(px(10.))
                    .text_color(rgb(MUTED))
                    .child("No matching commands")
                    .into_any_element(),
            );
        }

        let dialog = div()
            .relative()
            .w(px(650.))
            .max_w_full()
            .h(px(660.))
            .max_h(px((viewport_height - 124.).max(320.)))
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
                    .h(px(54.))
                    .flex_none()
                    .px(px(16.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .border_b_1()
                    .border_color(rgba(BORDER_SUBTLE))
                    .child(
                        Icon::new(IconName::Search)
                            .size(px(14.))
                            .text_color(rgb(MUTED)),
                    )
                    .child(
                        div().flex_1().child(
                            Textarea::new(&self.command_palette_query)
                                .appearance(false)
                                .bordered(false)
                                .h(px(42.))
                                .aria_label("Search commands"),
                        ),
                    )
                    .child(
                        div()
                            .h(px(20.))
                            .rounded(px(RADIUS_XS))
                            .border_1()
                            .border_color(rgba(BORDER))
                            .bg(rgb(SURFACE_INSET))
                            .px(px(6.))
                            .flex()
                            .items_center()
                            .font_family("monospace")
                            .text_size(px(8.))
                            .text_color(rgb(MUTED))
                            .child("Esc"),
                    ),
            )
            .child(
                div()
                    .id("command-results-shell")
                    .relative()
                    .flex_1()
                    .min_h(px(0.))
                    .smooth_scroll_shell(ScrollRegion::Commands, cx)
                    .child(
                        div()
                            .id("command-results")
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.command_scroll)
                            .p(px(7.))
                            .pr(px(SCROLLBAR_GUTTER))
                            .flex()
                            .flex_col()
                            .child(div().children(result_rows)),
                    )
                    .child(scrollbar_rail("commands".to_owned(), &self.command_scroll)),
            )
            .child(
                div()
                    .h(px(48.))
                    .flex_none()
                    .px(px(14.))
                    .border_t_1()
                    .border_color(rgba(BORDER_SUBTLE))
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_size(px(8.5))
                    .text_color(rgb(FAINT))
                    .child("Arrow keys navigate · Enter runs")
                    .child("alpha command palette"),
            );

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_start()
            .justify_center()
            .pt(px(96.))
            .px(px(28.))
            .child(backdrop_enter(
                div().absolute().inset_0().bg(rgba(SCRIM)).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        this.command_palette_open = false;
                        this.focus_composer(window, cx);
                        cx.notify();
                    }),
                ),
                format!("command-backdrop:{}", self.overlay_generation),
            ))
            .child(modal_enter(
                dialog,
                format!("command-dialog:{}", self.overlay_generation),
            ))
            .into_any_element()
    }
}

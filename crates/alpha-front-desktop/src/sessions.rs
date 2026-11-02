use super::*;

impl AlphaDesktop {
    pub(crate) fn render_sessions(&self, cx: &mut Context<Self>) -> AnyElement {
        if !self.sessions_open {
            return div().into_any_element();
        }

        let query = self.session_search.read(cx).value().to_lowercase();
        let filtered = self
            .sessions
            .iter()
            .filter(|session| {
                query.trim().is_empty() || session_matches_query(session, query.trim())
            })
            .collect::<Vec<_>>();
        let selected = filtered
            .iter()
            .copied()
            .find(|session| Some(session.id) == self.session_selected)
            .or_else(|| filtered.first().copied());
        let active_session = self.runtime.chat().session_id;

        let rows = if filtered.is_empty() {
            vec![
                div()
                    .py(px(48.))
                    .text_center()
                    .text_size(px(10.))
                    .text_color(rgb(MUTED))
                    .child(if self.sessions.is_empty() {
                        "No stored sessions"
                    } else {
                        "No matching sessions"
                    })
                    .into_any_element(),
            ]
        } else {
            filtered
                .into_iter()
                .map(|session| {
                    let session_id = session.id;
                    let is_selected = Some(session.id) == selected.map(|selected| selected.id);
                    let is_active = active_session == Some(session.id);
                    let title = session_title(session);
                    let preview = session_preview(session);
                    let route = session_route(session);
                    Button::new(format!("session:{}", session.id))
                        .child(
                            div()
                                .size_full()
                                .flex()
                                .items_start()
                                .gap(px(9.))
                                .child(icon_badge(
                                    IconName::MessageSquare,
                                    if is_selected { ACCENT } else { MUTED },
                                    if is_selected { ACCENT_SOFT } else { SURFACE },
                                    30.,
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
                                                .w_full()
                                                .flex()
                                                .items_center()
                                                .gap(px(6.))
                                                .child(
                                                    div()
                                                        .min_w(px(0.))
                                                        .flex_1()
                                                        .truncate()
                                                        .text_size(px(10.5))
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .text_color(rgb(if is_selected {
                                                            TEXT
                                                        } else {
                                                            TEXT_SECONDARY
                                                        }))
                                                        .child(title),
                                                )
                                                .when(is_active, |this| {
                                                    this.child(
                                                        div()
                                                            .rounded_full()
                                                            .bg(rgba(ACCENT_SOFT))
                                                            .px(px(6.))
                                                            .py(px(1.))
                                                            .text_size(px(7.))
                                                            .font_family("monospace")
                                                            .text_color(rgb(ACCENT))
                                                            .child("CURRENT"),
                                                    )
                                                }),
                                        )
                                        .child(
                                            div()
                                                .mt(px(2.))
                                                .w_full()
                                                .truncate()
                                                .text_size(px(8.5))
                                                .text_color(rgb(MUTED))
                                                .child(preview),
                                        )
                                        .child(
                                            div()
                                                .mt(px(5.))
                                                .flex()
                                                .gap(px(9.))
                                                .text_size(px(7.5))
                                                .text_color(rgb(FAINT))
                                                .child(display_value(session.mode.to_string()))
                                                .child(route),
                                        ),
                                ),
                        )
                        .selected(is_selected)
                        .ghost()
                        .w_full()
                        .h(px(62.))
                        .p(px(7.))
                        .rounded(px(18.))
                        .border_1()
                        .border_color(if is_selected {
                            rgba(ACCENT_BORDER)
                        } else {
                            rgba(0x00000000)
                        })
                        .bg(if is_selected {
                            rgba(ACCENT_SOFT)
                        } else {
                            rgba(0x00000000)
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.session_selected = Some(session_id);
                            this.session_delete_confirm = None;
                            this.session_status = None;
                            cx.notify();
                        }))
                        .into_any_element()
                })
                .collect()
        };

        let preview = if let Some(session) = selected {
            let session_id = session.id;
            let is_active = active_session == Some(session.id);
            let title = session_title(session);
            let preview_text = session_preview(session);
            let route = session_route(session);
            let status = self.session_status.clone().unwrap_or_else(|| {
                if is_active {
                    "Current session · ready to continue".to_owned()
                } else {
                    "Idle · restore from the durable log".to_owned()
                }
            });

            div()
                .size_full()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_start()
                        .justify_between()
                        .gap(px(8.))
                        .child(
                            div()
                                .text_size(px(12.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .line_height(px(17.))
                                .child(title),
                        )
                        .when(is_active, |this| {
                            this.child(
                                div()
                                    .rounded_full()
                                    .bg(rgba(ACCENT_SOFT))
                                    .px(px(6.))
                                    .py(px(2.))
                                    .text_size(px(7.))
                                    .font_family("monospace")
                                    .text_color(rgb(ACCENT))
                                    .child("CURRENT"),
                            )
                        }),
                )
                .child(
                    div()
                        .mt(px(8.))
                        .text_size(px(9.5))
                        .line_height(px(15.))
                        .text_color(rgb(MUTED))
                        .child(preview_text),
                )
                .child(
                    div()
                        .mt(px(18.))
                        .flex()
                        .flex_col()
                        .gap(px(10.))
                        .child(preview_meta("Workspace", &session.workspace.root))
                        .child(preview_meta("Route", &route))
                        .child(preview_meta(
                            "Permission mode",
                            &display_value(session.mode.to_string()),
                        ))
                        .child(preview_meta("Session ID", &short_id(session.id))),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(
                            Button::new("resume-selected-session")
                                .accent()
                                .label(if is_active {
                                    "Continue session"
                                } else {
                                    "Resume session"
                                })
                                .w_full()
                                .h(px(34.))
                                .rounded_full()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.resume_session(session_id, cx);
                                    this.focus_composer(window, cx);
                                })),
                        )
                        .child(
                            Button::new("start-draft-from-sessions")
                                .label("Start a new draft")
                                .secondary()
                                .w_full()
                                .h(px(34.))
                                .rounded_full()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.new_session(cx);
                                    this.focus_composer(window, cx);
                                })),
                        )
                        .child(
                            div()
                                .mt(px(2.))
                                .truncate()
                                .text_size(px(8.5))
                                .text_color(if self.local_status.is_some() {
                                    rgb(ACCENT)
                                } else {
                                    rgb(FAINT)
                                })
                                .child(status),
                        ),
                )
                .into_any_element()
        } else {
            div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(7.))
                .child(icon_badge(IconName::Clock, MUTED, SURFACE, 36.))
                .child(
                    div()
                        .text_size(px(11.))
                        .font_weight(FontWeight::MEDIUM)
                        .child("No session selected"),
                )
                .child(
                    div()
                        .text_size(px(9.))
                        .text_color(rgb(MUTED))
                        .child("Select a stored session or start a new draft."),
                )
                .into_any_element()
        };

        let dialog = div()
            .relative()
            .w(px(980.))
            .max_w_full()
            .h(px(610.))
            .max_h_full()
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
                    .h(px(78.))
                    .flex_none()
                    .px(px(20.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(rgba(BORDER_SUBTLE))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(11.))
                            .child(icon_badge(IconName::Clock, ACCENT, ACCENT_SOFT, 34.))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child("Sessions"),
                                    )
                                    .child(
                                        div().text_size(px(9.5)).text_color(rgb(MUTED)).child(
                                            format!("{} stored sessions", self.sessions.len()),
                                        ),
                                    ),
                            ),
                    )
                    .child(
                        icon_button("close-sessions", IconName::X, "Close").on_click(cx.listener(
                            |this, _, window, cx| {
                                this.sessions_open = false;
                                this.session_delete_confirm = None;
                                this.focus_composer(window, cx);
                                cx.notify();
                            },
                        )),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .border_r_1()
                            .border_color(rgba(BORDER_SUBTLE))
                            .child(
                                div()
                                    .h(px(54.))
                                    .flex_none()
                                    .px(px(12.))
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .border_b_1()
                                    .border_color(rgba(BORDER_SUBTLE))
                                    .child(
                                        div()
                                            .flex_1()
                                            .h(px(32.))
                                            .rounded_full()
                                            .border_1()
                                            .border_color(rgba(BORDER))
                                            .bg(rgb(SURFACE_INSET))
                                            .px(px(8.))
                                            .flex()
                                            .items_center()
                                            .gap(px(7.))
                                            .child(
                                                Icon::new(IconName::Search)
                                                    .size(px(13.))
                                                    .text_color(rgb(MUTED)),
                                            )
                                            .child(
                                                div().flex_1().child(
                                                    Textarea::new(&self.session_search)
                                                        .appearance(false)
                                                        .bordered(false)
                                                        .h(px(30.))
                                                        .aria_label("Search sessions"),
                                                ),
                                            ),
                                    )
                                    .child(
                                        icon_button(
                                            "refresh-sessions",
                                            IconName::RefreshCw,
                                            "Refresh sessions",
                                        )
                                        .on_click(
                                            cx.listener(|this, _, _, cx| {
                                                match this.runtime.list_sessions() {
                                                    Ok(sessions) => {
                                                        this.sessions = sessions;
                                                        this.session_status =
                                                            Some("sessions refreshed".to_owned());
                                                    }
                                                    Err(error) => {
                                                        this.session_status =
                                                            Some(error.to_string())
                                                    }
                                                }
                                                cx.notify();
                                            }),
                                        ),
                                    ),
                            )
                            .child(
                                div()
                                    .id("session-list-shell")
                                    .relative()
                                    .flex_1()
                                    .min_h(px(0.))
                                    .smooth_scroll_shell(ScrollRegion::Sessions, cx)
                                    .child(
                                        div()
                                            .id("session-list")
                                            .size_full()
                                            .overflow_y_scroll()
                                            .track_scroll(&self.session_scroll)
                                            .p(px(7.))
                                            .pr(px(SCROLLBAR_GUTTER))
                                            .flex()
                                            .flex_col()
                                            .gap(px(2.))
                                            .children(rows),
                                    )
                                    .child(scrollbar_rail(
                                        "sessions".to_owned(),
                                        &self.session_scroll,
                                    ))
                                    .children(scroll_fades(
                                        "sessions",
                                        rgb(SURFACE),
                                        panel_scroll_edges(&self.session_scroll),
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .w(px(230.))
                            .flex_none()
                            .bg(rgb(SURFACE_INSET))
                            .p(px(16.))
                            .child(preview),
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
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                Button::new("delete-selected-session")
                                    .label(if self.session_delete_confirm.is_some() {
                                        "Confirm delete"
                                    } else {
                                        "Delete"
                                    })
                                    .when(self.session_delete_confirm.is_some(), |button| {
                                        button.destructive()
                                    })
                                    .when(self.session_delete_confirm.is_none(), |button| {
                                        button.ghost()
                                    })
                                    .h(px(32.))
                                    .rounded_full()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.delete_selected_session(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("new-session")
                                    .label("New session")
                                    .secondary()
                                    .h(px(32.))
                                    .rounded_full()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.new_session(cx);
                                        this.focus_composer(window, cx);
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .text_size(px(8.5))
                                    .text_color(rgb(FAINT))
                                    .child("Enter  Resume"),
                            )
                            .child(
                                Button::new("resume-footer-session")
                                    .accent()
                                    .label("Resume session")
                                    .h(px(32.))
                                    .rounded_full()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        if let Some(session_id) = this.session_selected {
                                            this.resume_session(session_id, cx);
                                            this.focus_composer(window, cx);
                                        }
                                    })),
                            ),
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
                        this.sessions_open = false;
                        this.session_delete_confirm = None;
                        this.focus_composer(window, cx);
                        cx.notify();
                    }),
                ),
                format!("sessions-backdrop:{}", self.overlay_generation),
            ))
            .child(modal_enter(
                dialog,
                format!("sessions-dialog:{}", self.overlay_generation),
            ))
            .into_any_element()
    }
}

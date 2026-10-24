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
                if query.trim().is_empty() {
                    return true;
                }
                session_matches_query(session, query.trim())
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
                    .py(px(40.))
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
                    let route = if session.provider.is_empty() && session.model.is_empty() {
                        "default".to_owned()
                    } else {
                        format!("{}/{}", session.provider, session.model)
                    };
                    let preview = session
                        .first_message
                        .as_deref()
                        .map(|text| text.split_whitespace().collect::<Vec<_>>().join(" "))
                        .filter(|text| !text.is_empty())
                        .unwrap_or_else(|| "(empty session)".to_owned());
                    let session_id = session.id;
                    let is_selected = Some(session.id) == selected.map(|session| session.id);
                    let is_active = active_session == Some(session.id);
                    Button::new(format!("session:{}", session.id))
                        .child(
                            div()
                                .w_full()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .size(px(30.))
                                        .flex_none()
                                        .rounded_full()
                                        .bg(if is_selected {
                                            rgb(0x102e37)
                                        } else {
                                            rgb(SURFACE_RAISED)
                                        })
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_size(px(10.))
                                        .text_color(rgb(ACCENT))
                                        .child(">"),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.))
                                        .flex()
                                        .flex_col()
                                        .items_start()
                                        .gap_1()
                                        .child(
                                            div()
                                                .w_full()
                                                .flex()
                                                .items_center()
                                                .gap_1()
                                                .child(
                                                    div()
                                                        .font_family("monospace")
                                                        .text_size(px(10.))
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .text_color(rgb(TEXT))
                                                        .child(short_id(session.id)),
                                                )
                                                .child(
                                                    div()
                                                        .rounded(px(999.))
                                                        .bg(rgb(SURFACE_RAISED))
                                                        .px(px(7.))
                                                        .py(px(2.))
                                                        .text_size(px(8.))
                                                        .text_color(rgb(ACCENT))
                                                        .child(session.mode.to_string()),
                                                )
                                                .when(is_active, |this| {
                                                    this.child(
                                                        div()
                                                            .rounded(px(999.))
                                                            .bg(rgb(0x12333c))
                                                            .px(px(7.))
                                                            .py(px(2.))
                                                            .text_size(px(8.))
                                                            .text_color(rgb(0x72d69a))
                                                            .child("CURRENT"),
                                                    )
                                                }),
                                        )
                                        .child(
                                            div()
                                                .w_full()
                                                .truncate()
                                                .text_size(px(10.))
                                                .text_color(rgb(TEXT_SECONDARY))
                                                .child(preview),
                                        )
                                        .child(
                                            div()
                                                .max_w_full()
                                                .truncate()
                                                .text_size(px(8.))
                                                .font_family("monospace")
                                                .text_color(rgb(MUTED))
                                                .child(route),
                                        ),
                                ),
                        )
                        .selected(is_selected)
                        .ghost()
                        .bg(rgb(SURFACE))
                        .border_1()
                        .border_color(if is_selected {
                            rgb(0x36515a)
                        } else {
                            rgb(BORDER)
                        })
                        .w_full()
                        .h(px(74.))
                        .p(px(8.))
                        .rounded(px(18.))
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
            let route = if session.provider.is_empty() && session.model.is_empty() {
                "default".to_owned()
            } else {
                format!("{}/{}", session.provider, session.model)
            };
            let preview_text = session
                .first_message
                .as_deref()
                .map(|text| text.split_whitespace().collect::<Vec<_>>().join(" "))
                .filter(|text| !text.is_empty())
                .unwrap_or_else(|| "(empty session)".to_owned());
            let session_id = session.id;
            let remove_pending = self.session_delete_confirm == Some(session.id);
            let is_active = active_session == Some(session.id);
            div()
                .size_full()
                .flex()
                .flex_col()
                .gap_4()
                .child(
                    div()
                        .flex()
                        .items_start()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .text_size(px(16.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(short_id(session.id)),
                                )
                                .child(
                                    div()
                                        .font_family("monospace")
                                        .text_size(px(8.))
                                        .text_color(rgb(MUTED))
                                        .child(session.id.to_string()),
                                ),
                        )
                        .when(is_active, |this| {
                            this.child(
                                div()
                                    .rounded(px(999.))
                                    .bg(rgb(0x12333c))
                                    .px(px(9.))
                                    .py(px(4.))
                                    .text_size(px(8.))
                                    .text_color(rgb(0x72d69a))
                                    .child("CURRENT SESSION"),
                            )
                        }),
                )
                .child(
                    div()
                        .rounded(px(20.))
                        .bg(rgb(SURFACE))
                        .p(px(14.))
                        .text_size(px(11.))
                        .line_height(px(19.))
                        .text_color(rgb(TEXT_SECONDARY))
                        .child(preview_text),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(session_meta_row(
                            "Workspace",
                            session.workspace.root.clone(),
                        ))
                        .child(session_meta_row("CWD", session.workspace.cwd.clone()))
                        .child(session_meta_row("Route", route))
                        .child(session_meta_row("Mode", session.mode.to_string())),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Button::new("resume-selected-session")
                                .custom(accent_button_variant(cx))
                                .label(if is_active {
                                    "Continue session"
                                } else {
                                    "Resume session"
                                })
                                .rounded(px(999.))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.resume_session(session_id, cx);
                                })),
                        )
                        .child(div().flex_1())
                        .child(
                            Button::new("delete-selected-session")
                                .label(if remove_pending {
                                    "Confirm delete"
                                } else {
                                    "Delete"
                                })
                                .ghost()
                                .rounded(px(999.))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.delete_selected_session(window, cx);
                                })),
                        )
                        .when(remove_pending, |this| {
                            this.child(
                                Button::new("cancel-delete-session")
                                    .label("Cancel")
                                    .secondary()
                                    .rounded(px(999.))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.session_delete_confirm = None;
                                        this.session_status = None;
                                        cx.notify();
                                    })),
                            )
                        }),
                )
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .child(
                    div()
                        .size(px(42.))
                        .rounded_full()
                        .bg(rgb(SURFACE))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(16.))
                        .text_color(rgb(ACCENT))
                        .child("+"),
                )
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

        div()
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .child(div().absolute().inset_0().bg(rgba(0x00000099)))
            .child(
                div()
                    .relative()
                    .mt(px(48.))
                    .w(px(900.))
                    .h(px(610.))
                    .rounded(px(28.))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(SURFACE_RAISED))
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .child(
                        div()
                            .h(px(60.))
                            .flex_none()
                            .px(px(18.))
                            .flex()
                            .items_center()
                            .justify_between()
                            .border_b_1()
                            .border_color(rgb(BORDER))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .text_size(px(14.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child("Sessions"),
                                    )
                                    .child(
                                        div().text_size(px(9.)).text_color(rgb(MUTED)).child(
                                            format!("{} stored sessions", self.sessions.len()),
                                        ),
                                    ),
                            )
                            .child(
                                Button::new("close-sessions")
                                    .label("×")
                                    .ghost()
                                    .rounded(px(999.))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.sessions_open = false;
                                        this.session_delete_confirm = None;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h(px(0.))
                            .flex()
                            .child(
                                div()
                                    .w(px(430.))
                                    .flex_none()
                                    .border_r_1()
                                    .border_color(rgb(BORDER))
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .flex_none()
                                            .p(px(10.))
                                            .border_b_1()
                                            .border_color(rgb(BORDER))
                                            .child(
                                                div()
                                                    .rounded(px(18.))
                                                    .border_1()
                                                    .border_color(rgb(BORDER))
                                                    .bg(rgb(SURFACE))
                                                    .p(px(5.))
                                                    .child(
                                                        Textarea::new(&self.session_search)
                                                            .appearance(false)
                                                            .bordered(false)
                                                            .h(px(38.))
                                                            .aria_label("Search sessions"),
                                                    ),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .id("session-list")
                                            .flex_1()
                                            .min_h(px(0.))
                                            .overflow_y_scrollbar()
                                            .p(px(8.))
                                            .flex()
                                            .flex_col()
                                            .gap_1()
                                            .children(rows),
                                    )
                                    .child(
                                        div()
                                            .h(px(50.))
                                            .flex_none()
                                            .px(px(12.))
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .border_t_1()
                                            .border_color(rgb(BORDER))
                                            .child(
                                                div()
                                                    .truncate()
                                                    .text_size(px(9.))
                                                    .text_color(if self.session_status.is_some() {
                                                        rgb(ACCENT)
                                                    } else {
                                                        rgb(MUTED)
                                                    })
                                                    .child(
                                                        self.session_status.clone().unwrap_or_else(
                                                            || {
                                                                "Search by message, route, or ID"
                                                                    .to_owned()
                                                            },
                                                        ),
                                                    ),
                                            )
                                            .child(
                                                Button::new("new-session")
                                                    .secondary()
                                                    .label("New draft")
                                                    .rounded(px(999.))
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.new_session(cx);
                                                    })),
                                            ),
                                    ),
                            )
                            .child(div().flex_1().min_w(px(0.)).p(px(20.)).child(preview)),
                    ),
            )
            .into_any_element()
    }
}

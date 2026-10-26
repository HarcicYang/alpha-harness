use super::*;

impl AlphaDesktop {
    pub(crate) fn render_providers(&self, cx: &mut Context<Self>) -> AnyElement {
        if !self.providers_open {
            return div().into_any_element();
        }

        let providers = self.runtime.providers();
        let kinds = providers.kinds.clone();
        let instances = providers.instances.clone();
        let credential_backend = providers
            .credential_status
            .as_ref()
            .map(|status| {
                format!(
                    "{} · {}",
                    status.backend,
                    match status.status {
                        CredentialBackendStatus::Available => "available",
                        CredentialBackendStatus::Unavailable => "unavailable",
                    }
                )
            })
            .unwrap_or_else(|| "credential backend unknown".to_owned());
        let selected_kind = self.provider_kind.as_deref();
        let kind = selected_kind
            .and_then(|kind| kinds.iter().find(|descriptor| descriptor.kind == kind))
            .cloned();
        let selected_instance = self
            .provider_selected
            .as_deref()
            .and_then(|id| instances.iter().find(|instance| instance.id == id))
            .cloned();
        let kind_instances = selected_kind.map_or_else(Vec::new, |kind| {
            instances
                .iter()
                .filter(|instance| instance.kind == kind)
                .cloned()
                .collect::<Vec<_>>()
        });

        let (kind_rows, instance_rows) =
            self.render_provider_rows(&kinds, &instances, &kind_instances, selected_kind, cx);

        let provider_content = self.render_provider_content(
            selected_instance.as_ref(),
            kind.as_ref(),
            &kind_instances,
            instance_rows,
            &credential_backend,
            cx,
        );

        let dialog = div()
            .relative()
            .w(px(980.))
            .max_w_full()
            .h(px(690.))
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
                    .border_b_1()
                    .border_color(rgba(BORDER_SUBTLE))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(11.))
                            .child(icon_badge(IconName::Server, ACCENT, ACCENT_SOFT, 34.))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child("Providers"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(9.5))
                                            .text_color(rgb(MUTED))
                                            .child("Manage instances, credentials, and routes"),
                                    ),
                            ),
                    )
                    .child(
                        icon_button("close-providers", IconName::X, "Close").on_click(cx.listener(
                            |this, _, window, cx| {
                                this.providers_open = false;
                                this.provider_remove_confirm = None;
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
                            .w(px(210.))
                            .flex_none()
                            .border_r_1()
                            .border_color(rgba(BORDER_SUBTLE))
                            .bg(rgb(SURFACE_INSET))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .h(px(43.))
                                    .px(px(12.))
                                    .border_b_1()
                                    .border_color(rgba(BORDER_SUBTLE))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .text_size(px(9.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(MUTED))
                                    .child("Provider kinds")
                                    .child(
                                        icon_button(
                                            "refresh-providers",
                                            IconName::RefreshCw,
                                            "Refresh providers",
                                        )
                                        .on_click(
                                            cx.listener(|this, _, _, cx| {
                                                match this.runtime.refresh_providers() {
                                                    Ok(()) => {
                                                        this.model_choices = this
                                                            .runtime
                                                            .providers()
                                                            .model_choices();
                                                        this.provider_status =
                                                            Some("providers refreshed".to_owned());
                                                    }
                                                    Err(error) => {
                                                        this.provider_status =
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
                                    .id("provider-kind-list-shell")
                                    .relative()
                                    .flex_1()
                                    .min_h(px(0.))
                                    .smooth_scroll_shell(ScrollRegion::ProviderKinds, cx)
                                    .child(
                                        div()
                                            .id("provider-kind-list")
                                            .size_full()
                                            .overflow_y_scroll()
                                            .track_scroll(&self.provider_kind_scroll)
                                            .vertical_scrollbar(&self.provider_kind_scroll)
                                            .p(px(7.))
                                            .flex()
                                            .flex_col()
                                            .gap(px(2.))
                                            .children(kind_rows),
                                    ),
                            )
                            .child(
                                div()
                                    .p(px(9.))
                                    .border_t_1()
                                    .border_color(rgba(BORDER_SUBTLE))
                                    .child(
                                        Button::new("create-provider")
                                            .label("+  Add instance")
                                            .secondary()
                                            .w_full()
                                            .h(px(34.))
                                            .rounded_full()
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.create_provider_instance(window, cx);
                                            })),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .id("provider-content-shell")
                            .relative()
                            .flex_1()
                            .min_w(px(0.))
                            .min_h(px(0.))
                            .smooth_scroll_shell(ScrollRegion::Providers, cx)
                            .child(
                                div()
                                    .id("provider-content")
                                    .size_full()
                                    .overflow_y_scroll()
                                    .track_scroll(&self.provider_scroll)
                                    .vertical_scrollbar(&self.provider_scroll)
                                    .child(provider_content),
                            ),
                    ),
            )
            .child(
                div()
                    .h(px(52.))
                    .flex_none()
                    .px(px(16.))
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
                                Icon::new(IconName::ShieldCheck)
                                    .size(px(12.))
                                    .text_color(rgb(MUTED)),
                            )
                            .text_size(px(8.5))
                            .text_color(rgb(FAINT))
                            .child(self.provider_status.clone().unwrap_or_else(|| {
                                "Credentials are write-only and stored in the platform keyring"
                                    .to_owned()
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(7.))
                            .child(
                                Button::new("test-provider-footer")
                                    .label("Test connection")
                                    .ghost()
                                    .disabled(self.provider_selected.is_none())
                                    .h(px(32.))
                                    .rounded_full()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some(id) = this.provider_selected.clone() {
                                            this.test_provider_instance(id, cx);
                                        }
                                    })),
                            )
                            .child(
                                Button::new("done-providers")
                                    .accent()
                                    .label("Done")
                                    .h(px(32.))
                                    .rounded_full()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.providers_open = false;
                                        this.provider_remove_confirm = None;
                                        this.focus_composer(window, cx);
                                        cx.notify();
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
                        this.providers_open = false;
                        this.provider_remove_confirm = None;
                        this.focus_composer(window, cx);
                        cx.notify();
                    }),
                ),
                format!("providers-backdrop:{}", self.overlay_generation),
            ))
            .child(modal_enter(
                dialog,
                format!("providers-dialog:{}", self.overlay_generation),
            ))
            .into_any_element()
    }
}

pub(crate) fn provider_logo(name: &str, color: u32, background: u32) -> AnyElement {
    let initials = name
        .split_whitespace()
        .filter_map(|word| word.chars().next())
        .take(3)
        .collect::<String>()
        .to_uppercase();
    let background: Hsla = if background > 0x00ff_ffff {
        rgba(background).into()
    } else {
        rgb(background).into()
    };
    div()
        .size(px(30.))
        .flex_none()
        .rounded_full()
        .border_1()
        .border_color(rgba(BORDER))
        .bg(background)
        .flex()
        .items_center()
        .justify_center()
        .font_family("monospace")
        .text_size(px(8.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(color))
        .child(initials)
        .into_any_element()
}

pub(crate) fn section_heading(title: &'static str, detail: impl Into<String>) -> AnyElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .text_size(px(10.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(TEXT_SECONDARY))
                .child(title),
        )
        .child(
            div()
                .text_size(px(8.5))
                .text_color(rgb(MUTED))
                .child(detail.into()),
        )
        .into_any_element()
}

pub(crate) fn field_label(label: &'static str) -> AnyElement {
    div()
        .text_size(px(8.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(MUTED))
        .child(label)
        .into_any_element()
}

pub(crate) fn detail_stat(label: &'static str, value: impl Into<String>) -> AnyElement {
    div()
        .rounded(px(16.))
        .border_1()
        .border_color(rgba(BORDER_SUBTLE))
        .bg(rgb(SURFACE_INSET))
        .px(px(10.))
        .py(px(8.))
        .flex()
        .flex_col()
        .child(div().text_size(px(8.)).text_color(rgb(MUTED)).child(label))
        .child(
            div()
                .mt(px(3.))
                .truncate()
                .font_family("monospace")
                .text_size(px(8.5))
                .text_color(rgb(TEXT_SECONDARY))
                .child(value.into()),
        )
        .into_any_element()
}

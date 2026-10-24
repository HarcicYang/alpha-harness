use super::*;

impl AlphaDesktop {
    pub(crate) fn render_providers(&self, cx: &mut Context<Self>) -> AnyElement {
        if !self.providers_open {
            return div().into_any_element();
        }
        let providers = self.runtime.providers();
        let selected_kind = self.provider_kind.as_deref();
        let kind = selected_kind.and_then(|kind| providers.kind(kind));
        let selected_instance = self
            .provider_selected
            .as_deref()
            .and_then(|id| providers.instance(id));
        let instances = selected_kind.map_or_else(Vec::new, |kind| {
            providers
                .instances
                .iter()
                .filter(|instance| instance.kind == kind)
                .cloned()
                .collect::<Vec<_>>()
        });

        let kinds = if providers.kinds.is_empty() {
            vec![
                div()
                    .p(px(12.))
                    .text_size(px(11.))
                    .text_color(rgb(MUTED))
                    .child("No provider kinds registered")
                    .into_any_element(),
            ]
        } else {
            providers
                .kinds
                .iter()
                .map(|kind| {
                    let selected = selected_kind == Some(kind.kind.as_str());
                    let kind_id = kind.kind.clone();
                    Button::new(format!("provider-kind:{}", kind.kind))
                        .child(
                            div()
                                .w_full()
                                .flex()
                                .flex_col()
                                .items_start()
                                .child(
                                    div()
                                        .text_size(px(10.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(rgb(TEXT))
                                        .child(kind.name.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(px(8.))
                                        .text_color(rgb(MUTED))
                                        .child(kind.kind.clone()),
                                ),
                        )
                        .selected(selected)
                        .ghost()
                        .w_full()
                        .rounded(px(16.))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.select_provider_kind(kind_id.clone(), window, cx);
                        }))
                        .into_any_element()
                })
                .collect()
        };

        let instance_rows = if instances.is_empty() {
            vec![
                div()
                    .py(px(18.))
                    .text_center()
                    .text_size(px(10.))
                    .text_color(rgb(MUTED))
                    .child("No configured instances")
                    .into_any_element(),
            ]
        } else {
            instances
                .into_iter()
                .map(|instance| {
                    let id = instance.id.clone();
                    let selected = self.provider_selected.as_deref() == Some(id.as_str());
                    let has_credential = instance.auth_ref.is_some();
                    Button::new(format!("provider-instance:{id}"))
                        .child(
                            div()
                                .w_full()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .size(px(24.))
                                        .flex_none()
                                        .rounded_full()
                                        .bg(if instance.disabled {
                                            rgb(0x2a3038)
                                        } else {
                                            rgb(0x12333c)
                                        })
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_size(px(9.))
                                        .text_color(if instance.disabled {
                                            rgb(MUTED)
                                        } else {
                                            rgb(ACCENT)
                                        })
                                        .child(if instance.disabled { "!" } else { "✓" }),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.))
                                        .flex()
                                        .flex_col()
                                        .items_start()
                                        .child(
                                            div()
                                                .max_w_full()
                                                .truncate()
                                                .font_family("monospace")
                                                .text_size(px(9.))
                                                .text_color(rgb(TEXT))
                                                .child(id.clone()),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(8.))
                                                .text_color(if has_credential {
                                                    rgb(0x72d69a)
                                                } else {
                                                    rgb(MUTED)
                                                })
                                                .child(if has_credential {
                                                    "credential ready"
                                                } else {
                                                    "credential required"
                                                }),
                                        ),
                                ),
                        )
                        .selected(selected)
                        .ghost()
                        .w_full()
                        .rounded(px(16.))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.select_provider_instance(id.clone(), window, cx);
                            cx.notify();
                        }))
                        .into_any_element()
                })
                .collect()
        };

        let credential_backend = providers
            .credential_status
            .as_ref()
            .map(|status| {
                format!(
                    "{} backend: {}",
                    status.backend,
                    match status.status {
                        CredentialBackendStatus::Available => "available",
                        CredentialBackendStatus::Unavailable => "unavailable",
                    }
                )
            })
            .unwrap_or_else(|| "credential backend unknown".to_owned());

        let details = if let Some(instance) = selected_instance {
            let selected_id = instance.id.clone();
            let remove_pending =
                self.provider_remove_confirm.as_deref() == Some(selected_id.as_str());
            div()
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
                                        .font_family("monospace")
                                        .text_size(px(14.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(selected_id.clone()),
                                )
                                .child(div().text_size(px(9.)).text_color(rgb(MUTED)).child(
                                    format!(
                                        "{} · config v{} · {}",
                                        instance.kind, instance.config_version, credential_backend,
                                    ),
                                )),
                        )
                        .child(
                            div()
                                .rounded(px(999.))
                                .border_1()
                                .border_color(rgb(BORDER))
                                .bg(rgb(SURFACE))
                                .px(px(10.))
                                .py(px(5.))
                                .text_size(px(8.))
                                .text_color(if instance.disabled {
                                    rgb(0xff7888)
                                } else {
                                    rgb(0x72d69a)
                                })
                                .child(if instance.disabled {
                                    "Disabled"
                                } else {
                                    "Ready"
                                }),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .text_size(px(9.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(MUTED))
                                .child("Credential"),
                        )
                        .child(
                            div()
                                .rounded(px(18.))
                                .border_1()
                                .border_color(rgb(BORDER))
                                .bg(rgb(SURFACE))
                                .p(px(6.))
                                .child(
                                    Textarea::new(&self.provider_credential)
                                        .appearance(false)
                                        .bordered(false)
                                        .h(px(42.))
                                        .aria_label("Provider credential"),
                                ),
                        )
                        .child(
                            div().flex().justify_end().child(
                                Button::new("save-provider-credential")
                                    .label("Store credential")
                                    .secondary()
                                    .rounded(px(999.))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.authenticate_selected_provider(window, cx);
                                    })),
                            ),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_size(px(9.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(rgb(MUTED))
                                        .child("Configuration JSON"),
                                )
                                .child(
                                    Button::new("save-provider-config")
                                        .label("Save config")
                                        .secondary()
                                        .rounded(px(999.))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.save_provider_config(cx);
                                        })),
                                ),
                        )
                        .child(
                            div()
                                .rounded(px(20.))
                                .border_1()
                                .border_color(rgb(BORDER))
                                .bg(rgb(SURFACE))
                                .p(px(8.))
                                .child(
                                    Textarea::new(&self.provider_config)
                                        .appearance(false)
                                        .bordered(false)
                                        .h(px(170.))
                                        .aria_label("Provider configuration JSON"),
                                ),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Button::new("test-selected-provider")
                                .label("Test connection")
                                .ghost()
                                .rounded(px(999.))
                                .on_click(cx.listener({
                                    let id = selected_id.clone();
                                    move |this, _, _, cx| {
                                        this.test_provider_instance(id.clone(), cx);
                                    }
                                })),
                        )
                        .child(
                            Button::new("default-selected-provider")
                                .label("Set default")
                                .ghost()
                                .rounded(px(999.))
                                .on_click(cx.listener({
                                    let id = selected_id.clone();
                                    move |this, _, _, cx| {
                                        this.set_default_provider_instance(id.clone(), cx);
                                    }
                                })),
                        )
                        .child(div().flex_1())
                        .child(
                            Button::new("remove-selected-provider")
                                .label(if remove_pending {
                                    "Confirm remove"
                                } else {
                                    "Remove"
                                })
                                .ghost()
                                .rounded(px(999.))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.remove_selected_provider(window, cx);
                                })),
                        )
                        .when(remove_pending, |this| {
                            this.child(
                                Button::new("cancel-remove-provider")
                                    .label("Cancel")
                                    .secondary()
                                    .rounded(px(999.))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.provider_remove_confirm = None;
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
                .py(px(60.))
                .child(
                    div()
                        .size(px(42.))
                        .rounded_full()
                        .bg(rgb(0x102e37))
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
                        .child("No provider instance selected"),
                )
                .child(
                    div()
                        .text_size(px(9.))
                        .text_color(rgb(MUTED))
                        .child("Create an instance or select one from the list."),
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
                    .h(px(640.))
                    .rounded(px(26.))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(SURFACE_RAISED))
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .child(
                        div()
                            .h(px(58.))
                            .flex_none()
                            .px(px(16.))
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
                                            .child("Providers"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(9.))
                                            .text_color(rgb(MUTED))
                                            .child(
                                                "Credentials, configuration, and model routes",
                                            ),
                                    ),
                            )
                            .child(
                                Button::new("close-providers")
                                    .label("×")
                                    .ghost()
                                    .rounded(px(999.))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.providers_open = false;
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
                                    .w(px(230.))
                                    .flex_none()
                                    .border_r_1()
                                    .border_color(rgb(BORDER))
                                    .bg(rgb(SURFACE))
                                    .p(px(7.))
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .children(kinds),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .flex_none()
                                            .px(px(18.))
                                            .pt(px(16.))
                                            .pb(px(10.))
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .child(
                                                div()
                                                    .text_size(px(15.))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .child(
                                                        kind.map(|kind| kind.name.clone())
                                                            .unwrap_or_else(|| {
                                                                "Provider".to_owned()
                                                            }),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .gap_1()
                                                    .child(
                                                        Button::new("refresh-providers")
                                                            .label("Refresh")
                                                            .ghost()
                                                            .rounded(px(999.))
                                                            .on_click(cx.listener(
                                                                |this, _, _, cx| {
                                                                    match this
                                                                        .runtime
                                                                        .refresh_providers()
                                                                    {
                                                                        Ok(()) => this
                                                                            .provider_status =
                                                                            Some(
                                                                                "providers refreshed"
                                                                                    .to_owned(),
                                                                            ),
                                                                        Err(error) => {
                                                                            this.provider_status =
                                                                                Some(
                                                                                    error
                                                                                        .to_string(),
                                                                                )
                                                                        }
                                                                    }
                                                                    cx.notify();
                                                                },
                                                            )),
                                                    )
                                                    .child(
                                                        Button::new("create-provider")
                                                            .custom(accent_button_variant(cx))
                                                            .label("New instance")
                                                            .rounded(px(999.))
                                                            .on_click(cx.listener(
                                                                |this, _, window, cx| {
                                                                    this.create_provider_instance(
                                                                        window, cx,
                                                                    );
                                                                },
                                                            )),
                                                    ),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .id("provider-instance-list")
                                            .h(px(126.))
                                            .flex_none()
                                            .overflow_y_scrollbar()
                                            .px(px(14.))
                                            .pb(px(8.))
                                            .flex()
                                            .flex_col()
                                            .gap_1()
                                            .children(instance_rows),
                                    )
                                    .child(
                                        div().h(px(1.)).flex_none().bg(rgb(BORDER)),
                                    )
                                    .child(
                                        div()
                                            .id("provider-details")
                                            .flex_1()
                                            .min_h(px(0.))
                                            .overflow_y_scrollbar()
                                            .p(px(18.))
                                            .child(details),
                                    )
                                    .child(
                                        div()
                                            .h(px(42.))
                                            .flex_none()
                                            .border_t_1()
                                            .border_color(rgb(BORDER))
                                            .px(px(16.))
                                            .flex()
                                            .items_center()
                                            .child(
                                                div()
                                                    .truncate()
                                                    .text_size(px(9.))
                                                    .text_color(if self.provider_status.is_some() {
                                                        rgb(ACCENT)
                                                    } else {
                                                        rgb(MUTED)
                                                    })
                                                    .child(
                                                        self.provider_status.clone().unwrap_or_else(
                                                            || {
                                                                "Select an instance to edit its route."
                                                                    .to_owned()
                                                            },
                                                        ),
                                                    ),
                                            ),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }
}

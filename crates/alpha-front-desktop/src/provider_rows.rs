use super::*;
use crate::providers::provider_logo;
use alpha_schema::{ProviderDescriptor, ProviderInstance};

impl AlphaDesktop {
    pub(crate) fn render_provider_rows(
        &self,
        kinds: &[ProviderDescriptor],
        instances: &[ProviderInstance],
        kind_instances: &[ProviderInstance],
        selected_kind: Option<&str>,
        cx: &mut Context<Self>,
    ) -> (Vec<AnyElement>, Vec<AnyElement>) {
        let kind_rows = if kinds.is_empty() {
            vec![
                div()
                    .p(px(12.))
                    .text_size(px(10.))
                    .text_color(rgb(MUTED))
                    .child("No provider kinds registered")
                    .into_any_element(),
            ]
        } else {
            kinds
                .iter()
                .map(|kind| {
                    let selected = selected_kind == Some(kind.kind.as_str());
                    let kind_id = kind.kind.clone();
                    let count = instances
                        .iter()
                        .filter(|instance| instance.kind == kind.kind)
                        .count();
                    Button::new(format!("provider-kind:{}", kind.kind))
                        .child(
                            div()
                                .w_full()
                                .min_h(px(46.))
                                .flex()
                                .items_center()
                                .gap(px(9.))
                                .child(provider_logo(
                                    &kind.name,
                                    if selected { ACCENT } else { TEXT_SECONDARY },
                                    if selected { ACCENT_SOFT } else { SURFACE },
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
                                                .text_size(px(10.))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(rgb(if selected {
                                                    TEXT
                                                } else {
                                                    TEXT_SECONDARY
                                                }))
                                                .child(kind.name.clone()),
                                        )
                                        .child(
                                            div().text_size(px(8.)).text_color(rgb(MUTED)).child(
                                                format!(
                                                    "{count} {}",
                                                    if count == 1 {
                                                        "instance"
                                                    } else {
                                                        "instances"
                                                    }
                                                ),
                                            ),
                                        ),
                                )
                                .child(
                                    div()
                                        .min_w(px(17.))
                                        .h(px(17.))
                                        .rounded_full()
                                        .bg(rgb(SURFACE_ACTIVE))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .font_family("monospace")
                                        .text_size(px(7.5))
                                        .text_color(rgb(MUTED))
                                        .child(count.to_string()),
                                ),
                        )
                        .selected(selected)
                        .ghost()
                        .w_full()
                        .h(px(46.))
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
                            this.select_provider_kind(kind_id.clone(), window, cx);
                        }))
                        .into_any_element()
                })
                .collect()
        };

        let instance_rows = if kind_instances.is_empty() {
            vec![
                div()
                    .rounded(px(18.))
                    .border_1()
                    .border_color(rgba(BORDER_SUBTLE))
                    .bg(rgb(SURFACE_INSET))
                    .p(px(14.))
                    .text_size(px(9.5))
                    .text_color(rgb(MUTED))
                    .child("No instances configured for this provider")
                    .into_any_element(),
            ]
        } else {
            kind_instances
                .iter()
                .map(|instance| {
                    let id = instance.id.clone();
                    let selected = self.provider_selected.as_deref() == Some(id.as_str());
                    let has_credential = instance.auth_ref.is_some();
                    let test_id = id.clone();
                    let select_id = id.clone();
                    let credential_id = id.clone();
                    let edit_id = id.clone();
                    div()
                        .id(format!("provider-instance:{id}"))
                        .min_h(px(48.))
                        .rounded(px(18.))
                        .border_1()
                        .border_color(if selected {
                            rgba(ACCENT_BORDER)
                        } else {
                            rgba(BORDER)
                        })
                        .bg(if selected {
                            rgba(ACCENT_SOFT)
                        } else {
                            rgb(SURFACE_INSET)
                        })
                        .px(px(10.))
                        .py(px(7.))
                        .flex()
                        .items_center()
                        .gap(px(9.))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.select_provider_instance(select_id.clone(), window, cx);
                            cx.notify();
                        }))
                        .child(icon_badge(
                            if instance.disabled {
                                IconName::CircleX
                            } else {
                                IconName::CircleCheck
                            },
                            if instance.disabled { DANGER } else { ACCENT },
                            if instance.disabled {
                                DANGER_SOFT
                            } else {
                                ACCENT_SOFT
                            },
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
                                        .flex()
                                        .items_center()
                                        .gap(px(5.))
                                        .child(
                                            div()
                                                .truncate()
                                                .font_family("monospace")
                                                .text_size(px(9.5))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(rgb(TEXT_SECONDARY))
                                                .child(instance.id.clone()),
                                        )
                                        .when(instance.auth_ref.is_some(), |this| {
                                            this.child(
                                                div()
                                                    .rounded_full()
                                                    .bg(rgba(ACCENT_SOFT))
                                                    .px(px(5.))
                                                    .py(px(1.))
                                                    .text_size(px(6.5))
                                                    .font_family("monospace")
                                                    .text_color(rgb(ACCENT))
                                                    .child("KEY"),
                                            )
                                        }),
                                )
                                .child(
                                    div()
                                        .mt(px(2.))
                                        .truncate()
                                        .text_size(px(8.5))
                                        .text_color(rgb(if has_credential {
                                            MUTED
                                        } else {
                                            WARNING
                                        }))
                                        .child(if has_credential {
                                            "Responses API · credential stored in platform keyring"
                                                .to_owned()
                                        } else {
                                            "Credential required before this route can run"
                                                .to_owned()
                                        }),
                                ),
                        )
                        .child(
                            icon_button(
                                format!("set-credential:{credential_id}"),
                                IconName::KeyRound,
                                "Set credential",
                            )
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.select_provider_instance(
                                        credential_id.clone(),
                                        window,
                                        cx,
                                    );
                                    cx.notify();
                                },
                            )),
                        )
                        .child(
                            icon_button(
                                format!("test-provider:{id}"),
                                IconName::RefreshCw,
                                "Test connection",
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.test_provider_instance(test_id.clone(), cx);
                                },
                            )),
                        )
                        .child(
                            icon_button(
                                format!("edit-provider:{edit_id}"),
                                IconName::SlidersHorizontal,
                                "Edit configuration",
                            )
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.select_provider_instance(edit_id.clone(), window, cx);
                                    cx.notify();
                                },
                            )),
                        )
                        .into_any_element()
                })
                .collect()
        };
        (kind_rows, instance_rows)
    }
}

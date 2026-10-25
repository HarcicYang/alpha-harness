use super::*;
use crate::providers::{detail_stat, field_label, section_heading};
use alpha_schema::{ProviderDescriptor, ProviderInstance};

impl AlphaDesktop {
    pub(crate) fn render_provider_content(
        &self,
        selected_instance: Option<&ProviderInstance>,
        kind: Option<&ProviderDescriptor>,
        kind_instances: &[ProviderInstance],
        instance_rows: Vec<AnyElement>,
        credential_backend: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let (Some(selected), Some(kind)) = (selected_instance.as_ref(), kind.as_ref()) {
            let adapter = kind
                .adapters
                .iter()
                .find(|adapter| adapter.required)
                .or_else(|| kind.adapters.first())
                .map(|adapter| adapter.key.clone())
                .unwrap_or_else(|| "native".to_owned());
            let auth = kind
                .auth_methods
                .first()
                .map(|method| method.label.clone())
                .unwrap_or_else(|| "None".to_owned());
            let selected_id = selected.id.clone();
            let remove_pending =
                self.provider_remove_confirm.as_deref() == Some(selected_id.as_str());
            let models = self
                .model_choices
                .iter()
                .filter(|choice| choice.provider == selected_id)
                .collect::<Vec<_>>();
            let model_rows = if models.is_empty() {
                vec![
                    div()
                        .py(px(14.))
                        .text_size(px(9.))
                        .text_color(rgb(MUTED))
                        .child("No model metadata available")
                        .into_any_element(),
                ]
            } else {
                models
                    .into_iter()
                    .map(|model| {
                        div()
                            .min_h(px(34.))
                            .px(px(8.))
                            .rounded(px(RADIUS_XS))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .flex_1()
                                    .truncate()
                                    .font_family("monospace")
                                    .text_size(px(8.5))
                                    .text_color(rgb(TEXT_SECONDARY))
                                    .child(model.model_name.clone()),
                            )
                            .child(
                                div()
                                    .w(px(90.))
                                    .font_family("monospace")
                                    .text_size(px(8.5))
                                    .text_color(rgb(MUTED))
                                    .child(if model.context_window == 0 {
                                        "unknown".to_owned()
                                    } else {
                                        format!("{}k tokens", model.context_window / 1_000)
                                    }),
                            )
                            .child(
                                div()
                                    .w(px(70.))
                                    .font_family("monospace")
                                    .text_size(px(8.5))
                                    .text_color(rgb(MUTED))
                                    .child(if model.reasoning_levels.len() > 1 {
                                        "controlled"
                                    } else {
                                        "default"
                                    }),
                            )
                            .child(
                                div()
                                    .w(px(118.))
                                    .truncate()
                                    .font_family("monospace")
                                    .text_size(px(8.5))
                                    .text_color(rgb(MUTED))
                                    .child(model.model.clone()),
                            )
                            .into_any_element()
                    })
                    .collect()
            };

            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .px(px(22.))
                        .py(px(18.))
                        .border_b_1()
                        .border_color(rgba(BORDER_SUBTLE))
                        .flex()
                        .items_start()
                        .justify_between()
                        .child(
                            div()
                                .min_w(px(0.))
                                .child(
                                    div()
                                        .text_size(px(19.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(kind.name.clone()),
                                )
                                .child(
                                    div()
                                        .mt(px(4.))
                                        .max_w(px(520.))
                                        .text_size(px(10.5))
                                        .line_height(px(17.))
                                        .text_color(rgb(MUTED))
                                        .child(format!(
                                            "{} routes use the {} adapter with stored credentials and provider-managed model metadata.",
                                            kind.name, adapter
                                        )),
                                ),
                        )
                        .child(
                            div()
                                .flex_none()
                                .rounded_full()
                                .border_1()
                                .border_color(rgba(ACCENT_BORDER))
                                .bg(rgba(ACCENT_SOFT))
                                .px(px(8.))
                                .py(px(4.))
                                .flex()
                                .items_center()
                                .gap(px(5.))
                                .child(
                                    div()
                                        .size(px(5.))
                                        .rounded_full()
                                        .bg(rgb(ACCENT)),
                                )
                                .text_size(px(8.))
                                .text_color(rgb(ACCENT))
                                .child("Adapter available"),
                        ),
                )
                .child(
                    div()
                        .px(px(22.))
                        .py(px(16.))
                        .border_b_1()
                        .border_color(rgba(BORDER_SUBTLE))
                        .flex()
                        .flex_col()
                        .gap(px(9.))
                        .child(section_heading(
                            "Instances",
                            format!("{} configured", kind_instances.len()),
                        ))
                        .children(instance_rows),
                )
                .child(
                    div()
                        .px(px(22.))
                        .py(px(16.))
                        .border_b_1()
                        .border_color(rgba(BORDER_SUBTLE))
                        .flex()
                        .flex_col()
                        .gap(px(10.))
                        .child(section_heading("Selected instance", if selected.disabled {
                            "Disabled"
                        } else if selected.auth_ref.is_some() {
                            "Ready"
                        } else {
                            "Credential required"
                        }))
                        .child(
                            div()
                                .grid()
                                .grid_cols(3)
                                .gap(px(8.))
                                .child(detail_stat("Adapter", adapter))
                                .child(detail_stat("Authentication", auth))
                                .child(detail_stat(
                                    "Credential",
                                    if selected.auth_ref.is_some() {
                                        "stored in platform keyring"
                                    } else {
                                        "not configured"
                                    },
                                )),
                        )
                        .child(
                            div()
                                .grid()
                                .grid_cols(2)
                                .gap(px(12.))
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(6.))
                                        .child(field_label("Credential"))
                                        .child(
                                            div()
                                                .rounded(px(18.))
                                                .border_1()
                                                .border_color(rgba(BORDER))
                                                .bg(rgb(SURFACE))
                                                .px(px(6.))
                                                .child(
                                                    Textarea::new(&self.provider_credential)
                                                        .appearance(false)
                                                        .bordered(false)
                                                        .h(px(38.))
                                                        .aria_label("Provider credential"),
                                                ),
                                        )
                                        .child(
                                            div().flex().justify_end().child(
                                                Button::new("save-provider-credential")
                                                    .label("Store credential")
                                                    .secondary()
                                                    .h(px(30.))
                                                    .rounded_full()
                                                    .on_click(cx.listener(
                                                        |this, _, window, cx| {
                                                            this.authenticate_selected_provider(
                                                                window, cx,
                                                            );
                                                        },
                                                    )),
                                            ),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(6.))
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .justify_between()
                                                .child(field_label("Configuration JSON"))
                                                .child(
                                                    Button::new("save-provider-config")
                                                        .label("Save")
                                                        .ghost()
                                                        .h(px(26.))
                                                        .rounded_full()
                                                        .on_click(cx.listener(
                                                            |this, _, _, cx| {
                                                                this.save_provider_config(cx);
                                                            },
                                                        )),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .rounded(px(18.))
                                                .border_1()
                                                .border_color(rgba(BORDER))
                                                .bg(rgb(SURFACE))
                                                .p(px(6.))
                                                .child(
                                                    Textarea::new(&self.provider_config)
                                                        .appearance(false)
                                                        .bordered(false)
                                                        .h(px(90.))
                                                        .aria_label("Provider configuration JSON"),
                                                ),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(5.))
                                .child(
                                    Button::new("test-selected-provider")
                                        .label("Test connection")
                                        .ghost()
                                        .h(px(30.))
                                        .rounded_full()
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
                                        .h(px(30.))
                                        .rounded_full()
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
                                        .when(remove_pending, |button| button.destructive())
                                        .when(!remove_pending, |button| button.ghost())
                                        .h(px(30.))
                                        .rounded_full()
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.remove_selected_provider(window, cx);
                                        })),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(8.5))
                                .text_color(rgb(MUTED))
                                .child(format!("Credential backend · {credential_backend}")),
                        ),
                )
                .child(
                    div()
                        .px(px(22.))
                        .py(px(16.))
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child(section_heading(
                            "Model catalog",
                            format!("{} models · live metadata", model_rows.len()),
                        ))
                        .child(
                            div()
                                .min_h(px(28.))
                                .px(px(8.))
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .text_size(px(8.))
                                .text_color(rgb(FAINT))
                                .child(div().flex_1().child("Model"))
                                .child(div().w(px(90.)).child("Context"))
                                .child(div().w(px(70.)).child("Effort"))
                                .child(div().w(px(118.)).child("Route")),
                        )
                        .children(model_rows),
                )
                .into_any_element()
        } else {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(8.))
                .child(icon_badge(IconName::Server, MUTED, SURFACE_INSET, 38.))
                .child(
                    div()
                        .text_size(px(11.))
                        .font_weight(FontWeight::MEDIUM)
                        .child("Select or create an instance"),
                )
                .child(
                    div()
                        .text_size(px(9.))
                        .text_color(rgb(MUTED))
                        .child("Credentials never leave the managed core."),
                )
                .into_any_element()
        }
    }
}

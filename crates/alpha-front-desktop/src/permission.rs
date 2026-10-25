use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
struct PermissionPromptView {
    action: String,
    resources: Vec<String>,
    reason: String,
    protected: bool,
    sandboxed: bool,
}

#[derive(Clone, Copy)]
enum PermissionButtonStyle {
    Ghost,
    Secondary,
    Primary,
}

impl AlphaDesktop {
    pub(crate) fn render_permission_card(
        &self,
        interaction: InteractionPrompt,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let prompt = permission_prompt_view(&interaction.prompt);
        let command = prompt.resources.join("\n");
        let reason = prompt.reason.clone();
        let detail = format!(
            "{} mode · {}{}",
            display_mode(self.runtime.chat().mode),
            if prompt.protected {
                "protected action"
            } else {
                "waiting for your decision"
            },
            if prompt.sandboxed {
                " · sandboxed"
            } else {
                ""
            }
        );

        div()
            .mt(px(20.))
            .w_full()
            .rounded(px(RADIUS_MD))
            .border_1()
            .border_color(rgba(ACCENT_BORDER))
            .bg(rgb(SURFACE))
            .p(px(16.))
            .flex()
            .flex_col()
            .gap(px(11.))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(10.))
                    .child(icon_badge(
                        IconName::ShieldCheck,
                        WARNING,
                        WARNING_SOFT,
                        28.,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(TEXT))
                                    .child(permission_title(&prompt.action)),
                            )
                            .child(
                                div()
                                    .text_size(px(9.5))
                                    .text_color(rgb(MUTED))
                                    .child(detail),
                            )
                            .when(!reason.is_empty(), |this| {
                                this.child(
                                    div()
                                        .mt(px(2.))
                                        .text_size(px(9.))
                                        .text_color(rgb(FAINT))
                                        .child(reason),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .text_size(px(9.))
                            .font_family("monospace")
                            .text_color(rgb(MUTED))
                            .child(
                                Icon::new(IconName::Clock)
                                    .size(px(11.))
                                    .text_color(rgb(MUTED)),
                            )
                            .child(format_deadline(interaction.deadline)),
                    ),
            )
            .when(!command.is_empty(), |this| {
                this.child(
                    div()
                        .rounded(px(RADIUS_XS))
                        .border_1()
                        .border_color(rgba(BORDER_SUBTLE))
                        .bg(rgb(SURFACE_INSET))
                        .px(px(10.))
                        .py(px(7.))
                        .flex()
                        .items_start()
                        .gap(px(8.))
                        .child(
                            div()
                                .font_family("monospace")
                                .text_size(px(11.))
                                .text_color(rgb(ACCENT))
                                .child("$"),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .font_family("monospace")
                                .text_size(px(10.5))
                                .line_height(px(17.))
                                .text_color(rgb(TEXT_SECONDARY))
                                .child(command),
                        ),
                )
            })
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(6.))
                    .child(permission_button(
                        "deny",
                        "Deny",
                        PermissionButtonStyle::Ghost,
                        InteractionDecision::Deny,
                        cx,
                    ))
                    .child(permission_button(
                        "allow-session",
                        "Allow for session",
                        PermissionButtonStyle::Secondary,
                        InteractionDecision::Session,
                        cx,
                    ))
                    .child(permission_button(
                        "allow-once",
                        "Allow once",
                        PermissionButtonStyle::Primary,
                        InteractionDecision::Once,
                        cx,
                    )),
            )
            .into_any_element()
    }
}

fn permission_button(
    id: &'static str,
    label: &'static str,
    style: PermissionButtonStyle,
    decision: InteractionDecision,
    cx: &mut Context<AlphaDesktop>,
) -> Button {
    let button = Button::new(id).label(label).h(px(33.)).rounded_full();
    let button = match style {
        PermissionButtonStyle::Ghost => button.ghost(),
        PermissionButtonStyle::Secondary => button.secondary(),
        PermissionButtonStyle::Primary => button.accent(),
    };
    button.on_click(cx.listener(move |this, _, _, cx| {
        match this.runtime.respond_interaction(decision) {
            Ok(()) => {
                this.local_status = Some(match decision {
                    InteractionDecision::Deny => "permission denied".to_owned(),
                    InteractionDecision::Session => "allowed for this session".to_owned(),
                    InteractionDecision::Once => "allowed once".to_owned(),
                    InteractionDecision::Timeout => "permission timed out".to_owned(),
                });
            }
            Err(error) => this.local_status = Some(error.to_string()),
        }
        cx.notify();
    }))
}

fn permission_prompt_view(prompt: &serde_json::Value) -> PermissionPromptView {
    let request = prompt.get("request").unwrap_or(prompt);
    PermissionPromptView {
        action: request
            .get("action")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("permission")
            .to_owned(),
        resources: request
            .get("resources")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
            .map(str::to_owned)
            .collect(),
        reason: prompt
            .get("reason")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        protected: prompt
            .get("protected")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        sandboxed: request
            .get("sandboxed")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
    }
}

fn permission_title(action: &str) -> String {
    match action {
        "shell.exec" => "Workspace command requested".to_owned(),
        "fs.write" | "fs.edit" | "fs.create" => "Workspace write requested".to_owned(),
        "net.request" => "Network access requested".to_owned(),
        action => format!("Permission requested · {action}"),
    }
}

fn display_mode(mode: PermissionMode) -> String {
    display_value(mode.to_string())
}

fn format_deadline(deadline: Instant) -> String {
    let remaining = deadline.saturating_duration_since(Instant::now()).as_secs();
    format!("{}:{:02}", remaining / 60, remaining % 60)
}

#[cfg(test)]
mod tests {
    use super::{permission_prompt_view, permission_title};

    #[test]
    fn permission_prompt_projects_the_shared_payload() {
        let prompt = serde_json::json!({
            "request": {
                "action": "shell.exec",
                "resources": ["cargo test -p alpha-front-runtime"],
                "sandboxed": true
            },
            "reason": "run the frontend tests",
            "protected": false
        });

        let projected = permission_prompt_view(&prompt);
        assert_eq!(projected.action, "shell.exec");
        assert_eq!(projected.resources, ["cargo test -p alpha-front-runtime"]);
        assert_eq!(projected.reason, "run the frontend tests");
        assert!(projected.sandboxed);
        assert!(!projected.protected);
        assert_eq!(
            permission_title(&projected.action),
            "Workspace command requested"
        );
    }
}

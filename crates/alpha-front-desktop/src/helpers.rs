use super::*;

pub(crate) fn short_id(id: SessionId) -> String {
    let id = id.to_string();
    id.chars().take(8).collect()
}

pub(crate) fn session_matches_query(session: &SessionListEntry, query: &str) -> bool {
    let route = format!("{}/{}", session.provider, session.model);
    let preview = session.first_message.as_deref().unwrap_or_default();
    format!(
        "{} {} {} {} {}",
        session.id, session.mode, route, session.workspace.root, preview
    )
    .to_lowercase()
    .contains(&query.to_lowercase())
}

pub(crate) fn session_meta_row(label: &str, value: String) -> AnyElement {
    div()
        .flex()
        .items_start()
        .justify_between()
        .gap_3()
        .border_b_1()
        .border_color(rgb(BORDER))
        .py(px(6.))
        .child(
            div()
                .flex_none()
                .text_size(px(9.))
                .text_color(rgb(MUTED))
                .child(label.to_owned()),
        )
        .child(
            div()
                .min_w(px(0.))
                .truncate()
                .text_align(TextAlign::Right)
                .font_family("monospace")
                .text_size(px(9.))
                .text_color(rgb(TEXT_SECONDARY))
                .child(value),
        )
        .into_any_element()
}

pub(crate) fn workspace_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("workspace")
        .to_owned()
}

pub(crate) fn command_suggestions(query: &str) -> Vec<CommandSpec> {
    let query = query.to_ascii_lowercase();
    COMMANDS
        .iter()
        .copied()
        .filter(|command| command.name.starts_with(&query))
        .collect()
}

pub(crate) fn path_parts(path: &str) -> (String, String) {
    let path = path.trim_start_matches("./");
    match path.rsplit_once('/') {
        Some((directory, name)) => (
            name.to_owned(),
            if directory.is_empty() {
                "Workspace root".to_owned()
            } else {
                directory.to_owned()
            },
        ),
        None => (path.to_owned(), "Workspace root".to_owned()),
    }
}

pub(crate) fn pretty_provider_config(config: &serde_json::Value) -> String {
    serde_json::to_string_pretty(config).unwrap_or_else(|_| config.to_string())
}

pub(crate) fn parse_provider_config(text: &str) -> Result<serde_json::Value, String> {
    let config = serde_json::from_str::<serde_json::Value>(text.trim())
        .map_err(|error| format!("invalid JSON: {error}"))?;
    if !config.is_object() {
        return Err("configuration must be a JSON object".to_owned());
    }
    Ok(config)
}

pub(crate) fn format_token_count(tokens: usize) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}m", tokens as f32 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}k", tokens as f32 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

pub(crate) fn context_capabilities(context: &ContextView) -> Vec<String> {
    let mut capabilities = Vec::new();
    for capability in context
        .local_tools
        .iter()
        .chain(context.provider_tools.iter())
    {
        if !capabilities.contains(capability) {
            capabilities.push(capability.clone());
        }
    }
    capabilities
}

pub(crate) fn render_change_diff(change: &FileChange) -> AnyElement {
    let mut lines = Vec::new();
    let mut remaining = 80usize;
    if let Some(original) = &change.original {
        for line in original.lines().take(remaining) {
            lines.push(diff_line("-", line, 0xff9aaa, 0x281417));
            remaining = remaining.saturating_sub(1);
        }
    }
    if let Some(updated) = &change.updated {
        for line in updated.lines().take(remaining) {
            lines.push(diff_line("+", line, 0x9ce8b7, 0x10231a));
        }
    }
    if lines.is_empty() {
        lines.push(
            div()
                .text_size(px(10.))
                .text_color(rgb(MUTED))
                .child("No preview available")
                .into_any_element(),
        );
    }
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(1.))
        .children(lines)
        .into_any_element()
}

pub(crate) fn diff_line(marker: &str, line: &str, color: u32, background: u32) -> AnyElement {
    div()
        .w_full()
        .rounded(px(6.))
        .bg(rgb(background))
        .px(px(8.))
        .py(px(3.))
        .font_family("monospace")
        .text_size(px(9.))
        .line_height(px(15.))
        .text_color(rgb(color))
        .child(format!("{marker} {line}"))
        .into_any_element()
}

pub(crate) fn menu_label(label: &str) -> AnyElement {
    div()
        .px(px(9.))
        .py(px(7.))
        .text_size(px(9.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(MUTED))
        .child(label.to_owned())
        .into_any_element()
}

pub(crate) fn help_row(label: &str, shortcut: &str) -> AnyElement {
    div()
        .h(px(30.))
        .flex()
        .items_center()
        .justify_between()
        .border_b_1()
        .border_color(rgb(BORDER))
        .text_size(px(10.))
        .text_color(rgb(TEXT_SECONDARY))
        .child(label.to_owned())
        .child(
            div()
                .rounded(px(999.))
                .bg(rgb(SURFACE))
                .px(px(8.))
                .py(px(3.))
                .font_family("monospace")
                .text_size(px(9.))
                .text_color(rgb(MUTED))
                .child(shortcut.to_owned()),
        )
        .into_any_element()
}

pub(crate) fn accent_button_variant(cx: &App) -> ButtonCustomVariant {
    ButtonCustomVariant::new(cx)
        .color(rgb(ACCENT).into())
        .foreground(rgb(CANVAS).into())
        .hover(rgb(0x84dcf3).into())
        .active(rgb(0x84dcf3).into())
}

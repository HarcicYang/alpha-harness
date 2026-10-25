use super::*;

pub(crate) fn session_title(session: &SessionListEntry) -> String {
    let preview = session_preview(session);
    let words = preview.split_whitespace().take(9).collect::<Vec<_>>();
    if words.is_empty() {
        format!("Session {}", short_id(session.id))
    } else {
        words.join(" ")
    }
}

pub(crate) fn session_preview(session: &SessionListEntry) -> String {
    session
        .first_message
        .as_deref()
        .map(|text| text.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "(empty session)".to_owned())
}

pub(crate) fn session_route(session: &SessionListEntry) -> String {
    if session.provider.is_empty() && session.model.is_empty() {
        "default".to_owned()
    } else {
        format!("{} · {}", session.provider, session.model)
    }
}

pub(crate) fn preview_meta(label: &'static str, value: &str) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .child(div().text_size(px(8.)).text_color(rgb(FAINT)).child(label))
        .child(
            div()
                .mt(px(2.))
                .max_w_full()
                .truncate()
                .font_family("monospace")
                .text_size(px(8.5))
                .text_color(rgb(TEXT_SECONDARY))
                .child(value.to_owned()),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use alpha_schema::{PermissionMode, SessionId, SessionListEntry, Timestamp, WorkspaceRef};

    use super::{session_preview, session_route, session_title};

    fn session() -> SessionListEntry {
        SessionListEntry {
            id: SessionId::new(),
            created_at: Timestamp::from_millis(1),
            mode: PermissionMode::Auto,
            workspace: WorkspaceRef {
                root: "/workspace/alpha".to_owned(),
                cwd: "/workspace/alpha/crates/front".to_owned(),
            },
            provider: "openai".to_owned(),
            model: "gpt-5".to_owned(),
            first_message: Some(
                "Build the desktop shell against the shared front runtime without duplication"
                    .to_owned(),
            ),
        }
    }

    #[test]
    fn session_preview_helpers_stay_deterministic() {
        let session = session();
        assert_eq!(
            session_title(&session),
            "Build the desktop shell against the shared front runtime"
        );
        assert_eq!(
            session_preview(&session),
            "Build the desktop shell against the shared front runtime without duplication"
        );
        assert_eq!(session_route(&session), "openai · gpt-5");
    }
}

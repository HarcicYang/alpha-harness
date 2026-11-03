use super::*;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Longest title prefix in display columns, not characters. CJK glyphs occupy
/// two columns, so counting characters would cut a Chinese title much later
/// than a Latin one.
const TITLE_COLUMNS: usize = 72;

pub(crate) fn session_title(session: &SessionListEntry) -> String {
    let preview = session_preview(session);
    if preview.is_empty() {
        format!("Session {}", short_id(session.id))
    } else {
        truncate_display(&preview, TITLE_COLUMNS)
    }
}

/// Truncate at a display-column boundary and mark the cut with an ellipsis.
///
/// Spaced scripts get a word boundary when the final word is short. Scripts
/// without spaces, such as Chinese, are cut exactly at the column budget.
pub(crate) fn truncate_display(text: &str, max_columns: usize) -> String {
    if UnicodeWidthStr::width(text) <= max_columns {
        return text.to_owned();
    }
    let budget = max_columns.saturating_sub(1);
    let mut used = 0;
    let mut end = 0;
    for (index, character) in text.char_indices() {
        let width = UnicodeWidthChar::width(character).unwrap_or(0);
        if used + width > budget {
            break;
        }
        used += width;
        end = index + character.len_utf8();
    }
    let mut prefix = &text[..end];
    if let Some(space) = prefix.rfind(char::is_whitespace) {
        let tail = &prefix[space..];
        if UnicodeWidthStr::width(tail) <= 8 {
            prefix = &prefix[..space];
        }
    }
    format!("{}…", prefix.trim_end())
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

    use super::{session_preview, session_route, session_title, truncate_display};
    use unicode_width::UnicodeWidthStr;

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
            "Build the desktop shell against the shared front runtime without…"
        );
        assert_eq!(
            session_preview(&session),
            "Build the desktop shell against the shared front runtime without duplication"
        );
        assert_eq!(session_route(&session), "openai · gpt-5");
    }

    #[test]
    fn display_truncation_handles_spaced_and_unspaced_scripts() {
        assert_eq!(truncate_display("short", 10), "short");

        let latin = truncate_display(
            "Build the desktop shell against the shared front runtime without duplication",
            40,
        );
        assert!(latin.ends_with('…'));
        assert!(UnicodeWidthStr::width(latin.as_str()) <= 40);

        let chinese = truncate_display("我们的前两个session被打断毁掉了，我们来规划“安全打断”", 24);
        assert!(chinese.ends_with('…'));
        assert!(UnicodeWidthStr::width(chinese.as_str()) <= 24);
        assert!(chinese.starts_with("我们的前两个"));
    }
}

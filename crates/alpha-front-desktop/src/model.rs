use super::*;

pub(crate) fn desktop_transcript_items(items: &[TranscriptItem]) -> Vec<DesktopTranscriptItem> {
    let mut rendered = Vec::new();
    let mut tools = Vec::new();

    for item in items {
        match item {
            TranscriptItem::ToolCall {
                id, name, input, ..
            } => tools.push(ToolSummary {
                id: *id,
                name: name.clone(),
                input: (!input.is_null()).then(|| input.clone()),
                status: None,
                content: None,
            }),
            TranscriptItem::ToolResult {
                id,
                name,
                status,
                content,
            } => {
                if let Some(tool) = tools.iter_mut().rev().find(|tool| tool.id == *id) {
                    tool.status = Some(*status);
                    tool.content = Some(content.clone());
                } else {
                    tools.push(ToolSummary {
                        id: *id,
                        name: name.clone(),
                        input: None,
                        status: Some(*status),
                        content: Some(content.clone()),
                    });
                }
            }
            TranscriptItem::ToolOutput { id, text } => {
                if let Some(tool) = tools.iter_mut().rev().find(|tool| tool.id == *id) {
                    tool.content = Some(text.clone());
                }
            }
            other => match other {
                TranscriptItem::User { text } => {
                    if text.is_empty() {
                        continue;
                    }
                    flush_tools(&mut rendered, &mut tools);
                    rendered.push(DesktopTranscriptItem::User(text.clone().into()));
                }
                TranscriptItem::QueuedSteer { .. } => continue,
                TranscriptItem::AssistantDelta { text } => {
                    if text.is_empty() {
                        continue;
                    }
                    flush_tools(&mut rendered, &mut tools);
                    rendered.push(DesktopTranscriptItem::StreamingAssistant(
                        text.clone().into(),
                    ));
                }
                TranscriptItem::ThinkingDelta { text } => {
                    if text.is_empty() {
                        continue;
                    }
                    flush_tools(&mut rendered, &mut tools);
                    rendered.push(DesktopTranscriptItem::StreamingThinking(
                        text.clone().into(),
                    ));
                }
                TranscriptItem::AssistantMessage { blocks } => {
                    let thinking = blocks
                        .iter()
                        .filter_map(AssistantContentBlock::thinking)
                        .map(|(text, _)| text)
                        .collect::<Vec<_>>()
                        .join("\n");
                    let text = blocks
                        .iter()
                        .filter_map(AssistantContentBlock::text)
                        .collect::<Vec<_>>()
                        .join("\n");
                    if thinking.is_empty() && text.is_empty() {
                        continue;
                    }
                    flush_tools(&mut rendered, &mut tools);
                    if !thinking.is_empty() {
                        rendered.push(DesktopTranscriptItem::Thinking(thinking.into()));
                    }
                    if !text.is_empty() {
                        rendered.push(DesktopTranscriptItem::Assistant(text.into()));
                    }
                }
                TranscriptItem::Error { message } => {
                    if message.is_empty() {
                        continue;
                    }
                    flush_tools(&mut rendered, &mut tools);
                    rendered.push(DesktopTranscriptItem::Error(message.clone().into()));
                }
                TranscriptItem::ToolCall { .. } | TranscriptItem::ToolResult { .. } => {}
                TranscriptItem::ToolOutput { .. } => {}
            },
        }
    }
    flush_tools(&mut rendered, &mut tools);
    rendered
}

pub(crate) fn flush_tools(items: &mut Vec<DesktopTranscriptItem>, tools: &mut Vec<ToolSummary>) {
    if !tools.is_empty() {
        let tools = std::mem::take(tools);
        let id = tools
            .first()
            .map(|tool| tool.id.to_string())
            .unwrap_or_default();
        items.push(DesktopTranscriptItem::Tools { id, tools });
    }
}

pub(crate) fn compact_json(value: &serde_json::Value) -> String {
    let value = serde_json::to_string(value).unwrap_or_else(|_| value.to_string());
    if value.chars().count() <= 80 {
        value
    } else {
        format!("{}...", value.chars().take(77).collect::<String>())
    }
}

pub(crate) fn transcript_is_at_bottom(scroll: &ScrollHandle) -> bool {
    let offset = -scroll.offset().y;
    let maximum = scroll.max_offset().y;
    offset >= maximum - px(4.) && offset <= maximum + px(4.)
}

pub(crate) fn tool_status_label(status: Option<ToolStatus>) -> &'static str {
    status.map_or("running", |status| match status {
        ToolStatus::Completed => "completed",
        ToolStatus::Failed => "failed",
        ToolStatus::Interrupted => "interrupted",
    })
}

pub(crate) fn tool_status_color(status: Option<ToolStatus>) -> u32 {
    status.map_or(ACCENT, |status| match status {
        ToolStatus::Completed => ACCENT,
        ToolStatus::Failed => DANGER,
        ToolStatus::Interrupted => MUTED,
    })
}

pub(crate) fn tool_status_icon(status: Option<ToolStatus>) -> IconName {
    match status {
        Some(ToolStatus::Completed) => IconName::Check,
        Some(ToolStatus::Failed) => IconName::X,
        Some(ToolStatus::Interrupted) => IconName::CircleStop,
        None => IconName::LoaderCircle,
    }
}

pub(crate) fn compact_tool_content(content: &str) -> String {
    let compact = content.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= 360 {
        compact
    } else {
        format!("{}...", compact.chars().take(357).collect::<String>())
    }
}

pub(crate) fn tool_detail_content(tool: &ToolSummary) -> String {
    let mut sections = Vec::new();
    if let Some(input) = &tool.input {
        let input = serde_json::to_string_pretty(input).unwrap_or_else(|_| input.to_string());
        sections.push(format!("Input\n{input}"));
    }
    if let Some(content) = tool.content.as_deref() {
        let content = content.trim();
        if !content.is_empty() {
            sections.push(format!("Output\n{content}"));
        }
    }
    sections.join("\n\n")
}

//! Full-screen terminal frontend for alpha.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::io::{self, stdout};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use alpha_front_client::ConnectionEvent;
use alpha_front_supervisor::{CoreSupervisor, SupervisorConfig, SupervisorError};
use alpha_protocol::{Id, Request};
use alpha_provider_api::{
    ModelDescriptor, ProviderDescriptor, ProviderInstance, ReasoningLevel, effective_models,
};
use alpha_schema::PermissionMode;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers,
    KeyboardEnhancementFlags, MouseEventKind, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    supports_keyboard_enhancement,
};
use pulldown_cmark::{Event as MarkdownEvent, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Borders, Clear, List, ListItem, ListState as RatatuiListState, Paragraph, Wrap,
};
use serde_json::{Value, json};
use unicode_width::UnicodeWidthChar;

const COLOR_BRAND: Color = Color::Rgb(90, 221, 225);
const COLOR_ACCENT: Color = Color::Rgb(97, 192, 224);
const COLOR_BRIGHT: Color = Color::Rgb(184, 255, 254);
const COLOR_TEXT: Color = Color::Rgb(215, 255, 255);
const COLOR_MUTED: Color = Color::Rgb(65, 128, 176);
const COLOR_SUCCESS: Color = Color::Rgb(93, 227, 144);
const COLOR_WARNING: Color = Color::Rgb(82, 171, 237);
const COLOR_ERROR: Color = Color::Rgb(255, 48, 70);

const CORE_EVENT_BATCH_LIMIT: usize = 256;
const FRAME_INTERVAL: Duration = Duration::from_millis(16);
const INPUT_POLL_INTERVAL: Duration = Duration::from_millis(16);

/// TUI runtime failure.
#[derive(Debug, thiserror::Error)]
pub enum TuiError {
    /// Core process failed.
    #[error(transparent)]
    Supervisor(#[from] SupervisorError),
    /// Terminal IO failed.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// A core response was malformed.
    #[error("invalid core response: {0}")]
    Protocol(String),
}

/// Secret input state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthPrompt {
    /// Provider instance receiving the credential.
    pub instance_id: String,
    /// Secret input buffer.
    pub input: String,
}

/// Provider state rendered by the first TUI screen.
#[derive(Clone, Debug, Default)]
pub struct ProviderView {
    /// Registered provider kinds.
    pub kinds: Vec<ProviderDescriptor>,
    /// Persisted provider instances.
    pub instances: Vec<ProviderInstance>,
    /// Dynamically discovered model catalogs.
    pub model_cache: BTreeMap<String, Vec<ModelDescriptor>>,
    /// Selected provider kind index.
    pub selected_kind: usize,
    /// Selected instance index within the current kind.
    pub selected_instance: usize,
    /// Active secret input.
    pub auth_prompt: Option<AuthPrompt>,
    /// Active provider config editor.
    pub config_editor: Option<ConfigEditor>,
    /// Instance waiting for delete confirmation.
    pub confirm_remove: Option<String>,
    /// Credential backend status.
    pub credential_status: Option<CredentialStatusView>,
    /// Status or error line.
    pub status: Option<String>,
    /// When the current status line was first observed.
    status_changed_at: Option<Instant>,
    /// Last status text observed by the UI loop.
    last_status: Option<String>,
}

/// Credential backend state rendered by the provider screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialStatusView {
    /// Backend name.
    pub backend: String,
    /// Whether the backend can be used.
    pub available: bool,
    /// Optional diagnostic detail.
    pub detail: Option<String>,
}

/// Opaque provider config editor state.
#[derive(Clone, Debug)]
pub struct ConfigEditor {
    /// Instance being edited.
    pub instance: ProviderInstance,
    /// Editable JSON text.
    pub text: String,
    /// Validation or update error.
    pub error: Option<String>,
}

/// One visible line in the chat transcript.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TranscriptEntry {
    /// User message.
    User(String),
    /// Assistant message.
    Assistant(String),
    /// Assistant reasoning or thinking content.
    Thinking(String),
    /// Tool or system progress.
    Progress(String),
    /// Error or warning.
    Error(String),
}

/// One pending interaction rendered by the modal host.
#[derive(Clone, Debug)]
pub struct InteractionPrompt {
    /// Protocol request identity used for the response.
    pub id: Id,
    /// Interaction kind.
    pub kind: String,
    /// Short title.
    pub title: String,
    /// Body text.
    pub body: String,
    /// Deadline for the modal.
    pub deadline: Instant,
}

/// Chat state for the primary TUI screen.
#[derive(Clone, Debug, Default)]
pub struct ChatView {
    /// Active session identity.
    pub session_id: Option<String>,
    /// Visible transcript.
    pub transcript: Vec<TranscriptEntry>,
    /// Index of the in-progress assistant line.
    pub streaming_assistant: Option<usize>,
    /// Index of the in-progress thinking line.
    pub streaming_thinking: Option<usize>,
    /// Composer input.
    pub composer: Composer,
    /// Selected slash-command completion.
    completion_selected: usize,
    /// Whether the user dismissed the current completion list.
    completion_dismissed: bool,
    /// Status or error line.
    pub status: Option<String>,
    /// Current running turn id.
    pub running_turn: Option<String>,
    /// Pending approval or other interaction.
    pub interaction: Option<InteractionPrompt>,
    /// Provider instance or kind used for new sessions.
    pub provider: String,
    /// Model used for new sessions.
    pub model: String,
    /// Reasoning effort used for new sessions.
    pub reasoning: ReasoningLevel,
    /// Effective permission mode.
    pub mode: PermissionMode,
    /// Tool names keyed by durable call identity.
    tool_names: BTreeMap<String, String>,
    /// Distance from the transcript bottom after manual scrolling.
    transcript_scroll_back: u16,
    /// When the current status line was first observed.
    status_changed_at: Option<Instant>,
    /// Last status text observed by the UI loop.
    last_status: Option<String>,
    /// Monotonic revision per transcript entry.
    transcript_entry_revisions: Vec<u64>,
    /// Next transcript entry revision.
    next_transcript_entry_revision: u64,
    /// Whether the cached transcript rendering is stale.
    transcript_cache_dirty: bool,
    /// Cached rendered transcript for the current width.
    transcript_cache: Option<TranscriptCache>,
}

/// Multiline composer with a character-boundary cursor.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Composer {
    text: String,
    cursor: usize,
}

impl Composer {
    fn text(&self) -> &str {
        &self.text
    }

    fn cursor(&self) -> usize {
        self.cursor
    }

    fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    fn insert_char(&mut self, character: char) {
        self.text.insert(self.cursor, character);
        self.cursor += character.len_utf8();
    }

    fn insert_newline(&mut self) {
        self.insert_char('\n');
    }

    fn backspace(&mut self) {
        let previous = self.previous_boundary();
        if previous == self.cursor {
            return;
        }
        self.text.replace_range(previous..self.cursor, "");
        self.cursor = previous;
    }

    fn delete(&mut self) {
        let next = self.next_boundary();
        if next == self.cursor {
            return;
        }
        self.text.replace_range(self.cursor..next, "");
    }

    fn move_left(&mut self) {
        self.cursor = self.previous_boundary();
    }

    fn move_right(&mut self) {
        self.cursor = self.next_boundary();
    }

    fn move_up(&mut self) {
        let line_start = self.line_start();
        if line_start == 0 {
            return;
        }
        let previous_end = line_start - 1;
        let previous_start = self.text[..previous_end]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        self.cursor = char_column_offset(
            &self.text,
            previous_start,
            previous_end,
            self.text[line_start..self.cursor].chars().count(),
        );
    }

    fn move_down(&mut self) {
        let line_end = self.line_end();
        if line_end == self.text.len() {
            return;
        }
        let next_start = line_end + 1;
        let next_end = self.text[next_start..]
            .find('\n')
            .map_or(self.text.len(), |index| next_start + index);
        let line_start = self.line_start();
        self.cursor = char_column_offset(
            &self.text,
            next_start,
            next_end,
            self.text[line_start..self.cursor].chars().count(),
        );
    }

    fn move_home(&mut self) {
        self.cursor = self.line_start();
    }

    fn move_end(&mut self) {
        self.cursor = self.line_end();
    }

    fn delete_to_line_start(&mut self) {
        let start = self.line_start();
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    fn delete_to_line_end(&mut self) {
        let end = self.line_end();
        self.text.replace_range(self.cursor..end, "");
    }

    fn replace_range(&mut self, start: usize, end: usize, replacement: &str) {
        if start > end
            || end > self.text.len()
            || !self.text.is_char_boundary(start)
            || !self.text.is_char_boundary(end)
        {
            return;
        }
        self.text.replace_range(start..end, replacement);
        self.cursor = start + replacement.len();
    }

    fn previous_boundary(&self) -> usize {
        self.text[..self.cursor]
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index)
    }

    fn next_boundary(&self) -> usize {
        self.text[self.cursor..]
            .chars()
            .next()
            .map_or(self.cursor, |character| self.cursor + character.len_utf8())
    }

    fn line_start(&self) -> usize {
        self.text[..self.cursor]
            .rfind('\n')
            .map_or(0, |index| index + 1)
    }

    fn line_end(&self) -> usize {
        self.text[self.cursor..]
            .find('\n')
            .map_or(self.text.len(), |index| self.cursor + index)
    }
}

fn char_column_offset(text: &str, start: usize, end: usize, column: usize) -> usize {
    text[start..end]
        .char_indices()
        .nth(column)
        .map_or(end, |(index, _)| start + index)
}

struct ComposerVisual {
    lines: Vec<Line<'static>>,
    cursor_row: usize,
    cursor_col: usize,
}

fn composer_visual(composer: &Composer, inner_width: u16) -> ComposerVisual {
    let width = usize::from(inner_width.max(1));
    let mut rows = vec!["> ".to_owned()];
    let mut used = 2usize;
    let mut cursor = None;
    if composer.cursor == 0 {
        cursor = Some((0, used));
    }
    for (index, character) in composer.text.char_indices() {
        if index == composer.cursor {
            cursor = Some((rows.len() - 1, used));
        }
        if character == '\n' {
            rows.push("  ".to_owned());
            used = 2;
        } else {
            let character_width = UnicodeWidthChar::width(character).unwrap_or_default();
            if used > 2 && used.saturating_add(character_width) > width {
                rows.push("  ".to_owned());
                used = 2;
            }
            if let Some(row) = rows.last_mut() {
                row.push(character);
            }
            used = used.saturating_add(character_width);
        }
        if index + character.len_utf8() == composer.cursor {
            cursor = Some((rows.len() - 1, used));
        }
    }
    if composer.cursor == composer.text.len() {
        cursor = Some((rows.len() - 1, used));
    }
    let lines = rows
        .into_iter()
        .map(|row| {
            let (prefix, content) = row.split_at(2.min(row.len()));
            Line::from(vec![
                Span::styled(
                    prefix.to_owned(),
                    Style::default()
                        .fg(COLOR_BRAND)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(content.to_owned(), Style::default().fg(COLOR_TEXT)),
            ])
        })
        .collect();
    let (cursor_row, cursor_col) = cursor.unwrap_or((0, 2));
    ComposerVisual {
        lines,
        cursor_row,
        cursor_col,
    }
}

fn composer_height(composer: &Composer, width: u16) -> u16 {
    let inner_width = width.saturating_sub(2);
    let rows = composer_visual(composer, inner_width).lines.len();
    u16::try_from(rows)
        .unwrap_or(u16::MAX)
        .clamp(1, 6)
        .saturating_add(2)
}

#[derive(Clone, Debug)]
struct TranscriptCache {
    width: u16,
    entries: Vec<TranscriptCacheEntry>,
    lines: Vec<Line<'static>>,
    line_count: usize,
}

#[derive(Clone, Copy, Debug)]
struct TranscriptCacheEntry {
    revision: u64,
    line_start: usize,
    line_len: usize,
    line_count: usize,
}

impl ChatView {
    fn invalidate_transcript(&mut self) {
        self.transcript_cache_dirty = true;
    }

    fn ensure_transcript_entry_revisions(&mut self) {
        while self.transcript_entry_revisions.len() < self.transcript.len() {
            let revision = self.next_transcript_entry_revision();
            self.transcript_entry_revisions.push(revision);
        }
        self.transcript_entry_revisions
            .truncate(self.transcript.len());
    }

    fn next_transcript_entry_revision(&mut self) -> u64 {
        self.next_transcript_entry_revision = self.next_transcript_entry_revision.wrapping_add(1);
        self.next_transcript_entry_revision
    }

    fn push_transcript_entry(&mut self, entry: TranscriptEntry) {
        self.ensure_transcript_entry_revisions();
        let revision = self.next_transcript_entry_revision();
        self.transcript.push(entry);
        self.transcript_entry_revisions.push(revision);
        self.invalidate_transcript();
    }

    fn extend_transcript_entries(&mut self, entries: impl IntoIterator<Item = TranscriptEntry>) {
        for entry in entries {
            self.push_transcript_entry(entry);
        }
    }

    fn mark_transcript_entry(&mut self, index: usize) {
        self.ensure_transcript_entry_revisions();
        if index < self.transcript_entry_revisions.len() {
            self.transcript_entry_revisions[index] = self.next_transcript_entry_revision();
            self.invalidate_transcript();
        }
    }

    fn ensure_session(&mut self, supervisor: &mut CoreSupervisor) -> Result<(), TuiError> {
        if self.session_id.is_some() {
            return Ok(());
        }
        let workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let created = supervisor
            .request(
                "session.create",
                Some(json!({
                    "workspace": workspace,
                    "cwd": workspace,
                    "mode": "auto",
                    "provider": self.provider,
                    "model": self.model,
                    "reasoning": self.reasoning,
                })),
            )
            .map_err(|error| TuiError::Protocol(error.to_string()))?;
        self.session_id = created
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if self.session_id.is_none() {
            return Err(TuiError::Protocol(
                "session.create returned no session id".to_owned(),
            ));
        }
        if let Some(provider) = created.get("provider").and_then(Value::as_str) {
            self.provider = provider.to_owned();
        }
        if let Some(model) = created.get("model").and_then(Value::as_str) {
            self.model = model.to_owned();
        }
        if let Some(reasoning) = created
            .get("reasoning")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok())
        {
            self.reasoning = reasoning;
        }
        if let Some(mode) = created
            .get("mode")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok())
        {
            self.mode = mode;
        }
        self.status = Some("session ready".to_owned());
        Ok(())
    }

    fn new_session(&mut self, supervisor: &mut CoreSupervisor) -> Result<(), TuiError> {
        self.session_id = None;
        self.transcript.clear();
        self.transcript_entry_revisions.clear();
        self.invalidate_transcript();
        self.streaming_assistant = None;
        self.streaming_thinking = None;
        self.running_turn = None;
        self.interaction = None;
        self.tool_names.clear();
        self.transcript_scroll_back = 0;
        self.reset_completion();
        self.ensure_session(supervisor)
    }

    fn resume_session(
        &mut self,
        supervisor: &mut CoreSupervisor,
        session_id: &str,
    ) -> Result<(), TuiError> {
        let session_id = session_id
            .parse::<alpha_schema::SessionId>()
            .map_err(|error| TuiError::Protocol(error.to_string()))?;
        let resumed = supervisor
            .request("session.resume", Some(json!({ "sessionId": session_id })))
            .map_err(|error| TuiError::Protocol(error.to_string()))?;
        self.session_id = Some(session_id.to_string());
        if let Some(provider) = resumed.get("provider").and_then(Value::as_str) {
            self.provider = provider.to_owned();
        }
        if let Some(model) = resumed.get("model").and_then(Value::as_str) {
            self.model = model.to_owned();
        }
        if let Some(reasoning) = resumed
            .get("reasoning")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok())
        {
            self.reasoning = reasoning;
        }
        if let Some(mode) = resumed
            .get("mode")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok())
        {
            self.mode = mode;
        }
        self.transcript.clear();
        self.transcript_entry_revisions.clear();
        self.invalidate_transcript();
        self.streaming_assistant = None;
        self.streaming_thinking = None;
        self.running_turn = None;
        self.tool_names.clear();
        self.transcript_scroll_back = 0;
        self.reset_completion();
        supervisor
            .request(
                "session.attach",
                Some(json!({ "sessionId": session_id, "afterSeq": 0 })),
            )
            .map_err(|error| TuiError::Protocol(error.to_string()))?;
        self.status = Some(format!(
            "resumed {session_id} ({} messages)",
            resumed
                .get("messages")
                .and_then(Value::as_u64)
                .unwrap_or_default()
        ));
        Ok(())
    }

    fn set_mode(
        &mut self,
        supervisor: &mut CoreSupervisor,
        mode: PermissionMode,
    ) -> Result<(), TuiError> {
        let Some(session_id) = &self.session_id else {
            return Err(TuiError::Protocol("no active session".to_owned()));
        };
        supervisor
            .request(
                "session.set_mode",
                Some(json!({ "sessionId": session_id, "mode": mode })),
            )
            .map_err(|error| TuiError::Protocol(error.to_string()))?;
        self.mode = mode;
        self.status = Some(format!("mode: {mode}"));
        Ok(())
    }

    fn set_route(
        &mut self,
        supervisor: &mut CoreSupervisor,
        provider: String,
        model: String,
        reasoning: ReasoningLevel,
    ) -> Result<(), TuiError> {
        if provider.trim().is_empty() || model.trim().is_empty() {
            return Err(TuiError::Protocol(
                "provider and model must not be empty".to_owned(),
            ));
        }
        if let Some(session_id) = &self.session_id {
            supervisor
                .request(
                    "session.set_route",
                    Some(json!({
                        "sessionId": session_id,
                        "provider": provider,
                        "model": model,
                        "reasoning": reasoning,
                    })),
                )
                .map_err(|error| TuiError::Protocol(error.to_string()))?;
        }
        self.provider = provider;
        self.model = model;
        self.reasoning = reasoning;
        self.status = Some(format!(
            "route: {}/{}  effort:{}",
            self.provider, self.model, self.reasoning
        ));
        Ok(())
    }

    fn confirm_steers(
        &mut self,
        supervisor: &mut CoreSupervisor,
        accept: bool,
    ) -> Result<(), TuiError> {
        let Some(session_id) = &self.session_id else {
            return Err(TuiError::Protocol("no active session".to_owned()));
        };
        supervisor
            .request(
                "session.confirm_steers",
                Some(json!({ "sessionId": session_id, "accept": accept })),
            )
            .map_err(|error| TuiError::Protocol(error.to_string()))?;
        self.status = Some(if accept {
            "pending steers confirmed".to_owned()
        } else {
            "pending steers discarded".to_owned()
        });
        Ok(())
    }

    fn set_capabilities(
        &mut self,
        supervisor: &mut CoreSupervisor,
        provider_tools: Vec<String>,
    ) -> Result<(), TuiError> {
        let Some(session_id) = &self.session_id else {
            return Err(TuiError::Protocol("no active session".to_owned()));
        };
        supervisor
            .request(
                "session.set_capabilities",
                Some(json!({
                    "sessionId": session_id,
                    "providerTools": provider_tools,
                })),
            )
            .map_err(|error| TuiError::Protocol(error.to_string()))?;
        self.status = Some("provider capabilities updated".to_owned());
        Ok(())
    }

    fn submit(&mut self, supervisor: &mut CoreSupervisor) -> Result<(), TuiError> {
        let prompt = self.composer.text().trim().to_owned();
        if prompt.is_empty() {
            return Ok(());
        }
        self.ensure_session(supervisor)?;
        let session_id = self
            .session_id
            .clone()
            .ok_or_else(|| TuiError::Protocol("no active session".to_owned()))?;
        if self.running_turn.is_some() {
            supervisor
                .request(
                    "session.steer",
                    Some(json!({ "sessionId": session_id, "text": prompt })),
                )
                .map_err(|error| TuiError::Protocol(error.to_string()))?;
            self.status = Some("steer queued".to_owned());
        } else {
            let response = supervisor
                .request(
                    "session.submit",
                    Some(json!({ "sessionId": session_id, "prompt": prompt })),
                )
                .map_err(|error| TuiError::Protocol(error.to_string()))?;
            self.running_turn = response
                .get("turnId")
                .and_then(Value::as_str)
                .map(str::to_owned);
            self.status = Some("turn started".to_owned());
        }
        self.composer.clear();
        self.reset_completion();
        self.transcript_scroll_back = 0;
        Ok(())
    }

    fn reset_completion(&mut self) {
        self.completion_selected = 0;
        self.completion_dismissed = false;
    }

    fn scroll_transcript_up(&mut self, lines: u16) {
        self.transcript_scroll_back = self.transcript_scroll_back.saturating_add(lines);
    }

    fn scroll_transcript_down(&mut self, lines: u16) {
        self.transcript_scroll_back = self.transcript_scroll_back.saturating_sub(lines);
    }

    fn cancel(&mut self, supervisor: &mut CoreSupervisor) -> Result<(), TuiError> {
        let (Some(session_id), Some(turn_id)) = (&self.session_id, &self.running_turn) else {
            return Ok(());
        };
        supervisor
            .request(
                "session.cancel",
                Some(json!({ "sessionId": session_id, "turnId": turn_id })),
            )
            .map_err(|error| TuiError::Protocol(error.to_string()))?;
        self.status = Some("cancel requested".to_owned());
        Ok(())
    }

    fn handle_connection_event(&mut self, event: ConnectionEvent) {
        match event {
            ConnectionEvent::Notification(notification)
                if notification.method == "session.event" =>
            {
                if let Some(params) = notification.params {
                    self.handle_session_event(&params);
                }
            }
            ConnectionEvent::Request(request) if request.method == "interaction.request" => {
                self.handle_interaction_request(&request);
            }
            ConnectionEvent::Closed(error) => {
                self.status = Some(format!("core disconnected: {error}"));
            }
            _ => {}
        }
    }

    fn handle_session_event(&mut self, params: &Value) {
        if let Some(session_id) = params.get("sessionId").and_then(Value::as_str) {
            if self.session_id.as_deref() != Some(session_id) {
                return;
            }
        }
        let kind = params.get("kind").and_then(Value::as_str).unwrap_or("");
        let event = params.get("event").unwrap_or(&Value::Null);
        match kind {
            "durable" => self.handle_durable_event(event.get("payload").unwrap_or(&Value::Null)),
            "live" => self.handle_live_event(event),
            _ => {}
        }
    }

    fn handle_durable_event(&mut self, payload: &Value) {
        match payload.get("type").and_then(Value::as_str).unwrap_or("") {
            "user_message" => {
                if let Some(text) = payload.get("text").and_then(Value::as_str) {
                    self.push_transcript_entry(TranscriptEntry::User(text.to_owned()));
                    self.streaming_assistant = None;
                    self.streaming_thinking = None;
                }
            }
            "assistant_message" => {
                let entries = assistant_payload_entries(payload);
                self.clear_live_assistant();
                self.extend_transcript_entries(entries);
            }
            "tool_call" => {
                let id = payload
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let name = payload
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("tool");
                let input = payload.get("input").unwrap_or(&Value::Null);
                if !id.is_empty() {
                    self.tool_names.insert(id, name.to_owned());
                }
                self.push_transcript_entry(TranscriptEntry::Progress(tool_call_summary(
                    name, input,
                )));
            }
            "tool_result" => {
                let id = payload
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let status = payload
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("finished");
                let name = self
                    .tool_names
                    .remove(id)
                    .unwrap_or_else(|| "tool".to_owned());
                let content = payload
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                self.push_transcript_entry(TranscriptEntry::Progress(tool_result_summary(
                    &name, status, content,
                )));
            }
            "turn_ended" => {
                self.running_turn = None;
                self.status = Some("turn finished".to_owned());
            }
            "mode_changed" => {
                if let Ok(mode) =
                    serde_json::from_value(payload.get("mode").cloned().unwrap_or(Value::Null))
                {
                    self.mode = mode;
                    self.status = Some(format!("mode: {mode}"));
                }
            }
            "route_changed" => {
                if let Some(provider) = payload.get("provider").and_then(Value::as_str) {
                    self.provider = provider.to_owned();
                }
                if let Some(model) = payload.get("model").and_then(Value::as_str) {
                    self.model = model.to_owned();
                }
                if let Ok(reasoning) = serde_json::from_value(
                    payload
                        .get("reasoning")
                        .cloned()
                        .unwrap_or(Value::String("auto".to_owned())),
                ) {
                    self.reasoning = reasoning;
                }
                self.status = Some(format!(
                    "route: {}/{}  effort:{}",
                    self.provider, self.model, self.reasoning
                ));
            }
            "steer_queued" => self.status = Some("steer queued".to_owned()),
            "steer_injected" => self.status = Some("steer injected".to_owned()),
            _ => {}
        }
    }

    fn handle_live_event(&mut self, event: &Value) {
        match event.get("type").and_then(Value::as_str).unwrap_or("") {
            "text_delta" => {
                let delta = event
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if let Some(index) = self.streaming_assistant {
                    if let Some(TranscriptEntry::Assistant(text)) = self.transcript.get_mut(index) {
                        text.push_str(delta);
                        self.mark_transcript_entry(index);
                    }
                } else {
                    self.push_transcript_entry(TranscriptEntry::Assistant(delta.to_owned()));
                    self.streaming_assistant = Some(self.transcript.len() - 1);
                }
            }
            "thinking_delta" => {
                let delta = event
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if let Some(index) = self.streaming_thinking {
                    if let Some(TranscriptEntry::Thinking(text)) = self.transcript.get_mut(index) {
                        text.push_str(delta);
                        self.mark_transcript_entry(index);
                    }
                } else {
                    self.push_transcript_entry(TranscriptEntry::Thinking(delta.to_owned()));
                    self.streaming_thinking = Some(self.transcript.len() - 1);
                }
            }
            "step_started" => {
                if let Some(step) = event.get("step").and_then(Value::as_u64) {
                    self.status = Some(format!("model step {step}"));
                }
            }
            "tool_started" => {
                let name = event.get("name").and_then(Value::as_str).unwrap_or("tool");
                let input = event.get("input").unwrap_or(&Value::Null);
                self.status = Some(format!("running {}", tool_call_summary(name, input)));
            }
            "tool_finished" => {
                let name = event.get("name").and_then(Value::as_str).unwrap_or("tool");
                let status = event
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("finished");
                let content = event
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                self.status = Some(tool_result_summary(name, status, content));
            }
            "error" => {
                let message = event
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("model request failed");
                self.clear_live_assistant();
                self.push_transcript_entry(TranscriptEntry::Error(message.to_owned()));
                self.status = Some(message.to_owned());
                self.running_turn = None;
                self.transcript_scroll_back = 0;
            }
            _ => {}
        }
    }

    fn clear_live_assistant(&mut self) {
        let mut indices = [self.streaming_thinking, self.streaming_assistant]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        indices.sort_unstable();
        indices.dedup();
        for index in indices.into_iter().rev() {
            if index < self.transcript.len()
                && matches!(
                    &self.transcript[index],
                    TranscriptEntry::Assistant(_) | TranscriptEntry::Thinking(_)
                )
            {
                self.transcript.remove(index);
                if index < self.transcript_entry_revisions.len() {
                    self.transcript_entry_revisions.remove(index);
                }
                self.invalidate_transcript();
            }
        }
        self.streaming_thinking = None;
        self.streaming_assistant = None;
    }

    fn tick_status(&mut self) -> bool {
        if self.status != self.last_status {
            self.last_status = self.status.clone();
            self.status_changed_at = Some(Instant::now());
        }
        if self
            .status_changed_at
            .is_some_and(|changed| changed.elapsed() >= Duration::from_secs(6))
        {
            self.status = None;
            self.last_status = None;
            self.status_changed_at = None;
            return true;
        }
        false
    }

    fn handle_interaction_request(&mut self, request: &Request) {
        let params = request.params.clone().unwrap_or(Value::Null);
        let kind = params
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("interaction")
            .to_owned();
        let title = if kind == "permission" {
            "Permission requested".to_owned()
        } else {
            kind.clone()
        };
        let body = serde_json::to_string_pretty(&params).unwrap_or_else(|_| params.to_string());
        self.interaction = Some(InteractionPrompt {
            id: request.id.clone(),
            kind,
            title,
            body,
            deadline: Instant::now() + Duration::from_secs(300),
        });
    }

    fn respond_interaction(
        &mut self,
        supervisor: &CoreSupervisor,
        decision: &str,
    ) -> Result<(), TuiError> {
        let Some(prompt) = self.interaction.take() else {
            return Ok(());
        };
        supervisor
            .respond(prompt.id, Ok(json!({ "decision": decision })))
            .map_err(|error| TuiError::Protocol(error.to_string()))?;
        self.status = Some(format!("interaction: {decision}"));
        Ok(())
    }

    fn expire_interaction(&mut self, supervisor: &CoreSupervisor) -> bool {
        let expired = self
            .interaction
            .as_ref()
            .is_some_and(|prompt| Instant::now() >= prompt.deadline);
        if expired {
            let _ = self.respond_interaction(supervisor, "timeout");
        }
        expired
    }
}

fn assistant_payload_entries(payload: &Value) -> Vec<TranscriptEntry> {
    let mut entries = Vec::new();
    if let Some(blocks) = payload.get("blocks").and_then(Value::as_array) {
        for block in blocks {
            match block.get("type").and_then(Value::as_str) {
                Some("thinking") => {
                    if let Some(text) = block.get("text").and_then(Value::as_str)
                        && !text.is_empty()
                    {
                        entries.push(TranscriptEntry::Thinking(text.to_owned()));
                    }
                }
                Some("text") => {
                    if let Some(text) = block.get("text").and_then(Value::as_str)
                        && !text.is_empty()
                    {
                        entries.push(TranscriptEntry::Assistant(text.to_owned()));
                    }
                }
                _ => {}
            }
        }
    }
    if entries.is_empty()
        && let Some(text) = payload.get("text").and_then(Value::as_str)
        && !text.is_empty()
    {
        entries.push(TranscriptEntry::Assistant(text.to_owned()));
    }
    entries
}

#[derive(Clone, Copy, Default)]
struct MarkdownFormat {
    bold: bool,
    italic: bool,
    strike: bool,
    code: bool,
    link: bool,
    heading: Option<HeadingLevel>,
    quote: bool,
}

impl MarkdownFormat {
    fn style(self) -> Style {
        let mut style = Style::default();
        if let Some(level) = self.heading {
            style = style.fg(match level {
                HeadingLevel::H1 => COLOR_BRIGHT,
                HeadingLevel::H2 => COLOR_BRAND,
                HeadingLevel::H3 => COLOR_TEXT,
                HeadingLevel::H4 | HeadingLevel::H5 | HeadingLevel::H6 => COLOR_MUTED,
            });
            style = style.add_modifier(Modifier::BOLD);
        }
        if self.quote {
            style = style.fg(COLOR_MUTED).add_modifier(Modifier::ITALIC);
        }
        if self.code {
            style = style.fg(COLOR_SUCCESS);
        }
        if self.link {
            style = style.fg(COLOR_WARNING).add_modifier(Modifier::UNDERLINED);
        }
        if self.bold {
            style = style.add_modifier(Modifier::BOLD);
        }
        if self.italic {
            style = style.add_modifier(Modifier::ITALIC);
        }
        if self.strike {
            style = style.add_modifier(Modifier::CROSSED_OUT);
        }
        style
    }
}

#[derive(Clone, Copy)]
struct ListState {
    ordered: bool,
    next: u64,
}

#[derive(Default)]
struct MarkdownTable {
    alignments: Vec<pulldown_cmark::Alignment>,
    header: Vec<Vec<Span<'static>>>,
    rows: Vec<Vec<Vec<Span<'static>>>>,
    current_row: Vec<Vec<Span<'static>>>,
    current_cell: Vec<Span<'static>>,
    in_header: bool,
    in_cell: bool,
}

#[derive(Default)]
struct MarkdownRenderer {
    lines: Vec<Line<'static>>,
    current: Vec<Span<'static>>,
    format: MarkdownFormat,
    quote_depth: usize,
    lists: Vec<ListState>,
    item_prefixes: Vec<String>,
    link_targets: Vec<String>,
    in_code_block: bool,
    table: Option<MarkdownTable>,
}

impl MarkdownRenderer {
    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {}
            Tag::Heading { level, .. } => {
                self.flush_line();
                self.format.heading = Some(level);
            }
            Tag::BlockQuote(_) => {
                self.quote_depth = self.quote_depth.saturating_add(1);
                self.format.quote = true;
            }
            Tag::CodeBlock(_) => {
                self.flush_line();
                self.in_code_block = true;
                self.format.code = true;
            }
            Tag::Table(alignments) => {
                self.flush_line();
                self.table = Some(MarkdownTable {
                    alignments,
                    ..MarkdownTable::default()
                });
            }
            Tag::TableHead => {
                if let Some(table) = &mut self.table {
                    table.in_header = true;
                }
            }
            Tag::TableRow => {
                if let Some(table) = &mut self.table {
                    table.current_row.clear();
                }
            }
            Tag::TableCell => {
                if let Some(table) = &mut self.table {
                    table.current_cell.clear();
                    table.in_cell = true;
                }
            }
            Tag::List(start) => {
                self.lists.push(ListState {
                    ordered: start.is_some(),
                    next: start.unwrap_or(1),
                });
            }
            Tag::Item => {
                self.flush_line();
                let marker = if let Some(list) = self.lists.last_mut() {
                    if list.ordered {
                        let marker = format!("{}. ", list.next);
                        list.next = list.next.saturating_add(1);
                        marker
                    } else {
                        "- ".to_owned()
                    }
                } else {
                    "- ".to_owned()
                };
                self.item_prefixes.push(marker);
            }
            Tag::Emphasis => self.format.italic = true,
            Tag::Strong => self.format.bold = true,
            Tag::Strikethrough => self.format.strike = true,
            Tag::Link { dest_url, .. } => {
                self.link_targets.push(dest_url.to_string());
                self.format.link = true;
            }
            Tag::Image { .. } => self.format.italic = true,
            Tag::HtmlBlock | Tag::FootnoteDefinition(_) => {}
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.blank_line(),
            TagEnd::Heading(_) => {
                self.format.heading = None;
                self.blank_line();
            }
            TagEnd::BlockQuote(_) => {
                self.quote_depth = self.quote_depth.saturating_sub(1);
                self.format.quote = self.quote_depth > 0;
                self.blank_line();
            }
            TagEnd::CodeBlock => {
                self.in_code_block = false;
                self.format.code = false;
                self.blank_line();
            }
            TagEnd::TableCell => {
                if let Some(table) = &mut self.table {
                    table.in_cell = false;
                    let cell = std::mem::take(&mut table.current_cell);
                    if table.in_header {
                        table.header.push(cell);
                    } else {
                        table.current_row.push(cell);
                    }
                }
            }
            TagEnd::TableRow => {
                if let Some(table) = &mut self.table {
                    let row = std::mem::take(&mut table.current_row);
                    if table.in_header && table.header.is_empty() {
                        table.header = row;
                    } else if !row.is_empty() {
                        table.rows.push(row);
                    }
                }
            }
            TagEnd::TableHead => {
                if let Some(table) = &mut self.table {
                    table.in_header = false;
                    if table.header.is_empty() && !table.current_row.is_empty() {
                        table.header = std::mem::take(&mut table.current_row);
                    }
                }
            }
            TagEnd::Table => {
                if let Some(table) = self.table.take() {
                    self.lines.extend(render_markdown_table(table));
                    self.blank_line();
                }
            }
            TagEnd::List(_) => {
                self.lists.pop();
                self.blank_line();
            }
            TagEnd::Item => {
                self.flush_line();
                self.item_prefixes.pop();
            }
            TagEnd::Emphasis => self.format.italic = false,
            TagEnd::Strong => self.format.bold = false,
            TagEnd::Strikethrough => self.format.strike = false,
            TagEnd::Link => {
                self.format.link = false;
                if let Some(url) = self.link_targets.pop()
                    && !url.is_empty()
                {
                    self.append_styled(&format!(" ({url})"), Style::default().fg(COLOR_MUTED));
                }
            }
            TagEnd::Image => self.format.italic = false,
            TagEnd::HtmlBlock => self.blank_line(),
            _ => {}
        }
    }

    fn text(&mut self, text: &str) {
        let style = self.format.style();
        self.append_styled(text, style);
    }

    fn inline_code(&mut self, text: &str) {
        let previous = self.format.code;
        self.format.code = true;
        self.text(text);
        self.format.code = previous;
    }

    fn append_styled(&mut self, text: &str, style: Style) {
        if let Some(table) = &mut self.table
            && table.in_cell
        {
            for (index, part) in text.split('\n').enumerate() {
                if index > 0 && !part.is_empty() {
                    table.current_cell.push(Span::styled(" ".to_owned(), style));
                }
                if !part.is_empty() {
                    table
                        .current_cell
                        .push(Span::styled(part.to_owned(), style));
                }
            }
            return;
        }
        let mut first = true;
        for part in text.split('\n') {
            if !first {
                self.flush_line();
            }
            if !part.is_empty() {
                self.ensure_prefix();
                self.current.push(Span::styled(part.to_owned(), style));
            }
            first = false;
        }
    }

    fn ensure_prefix(&mut self) {
        if !self.current.is_empty() {
            return;
        }
        let mut prefix = self.line_prefix();
        if self.in_code_block {
            prefix.push_str("  ");
        }
        if !prefix.is_empty() {
            self.current
                .push(Span::styled(prefix, Style::default().fg(COLOR_MUTED)));
        }
    }

    fn line_prefix(&self) -> String {
        let mut prefix = "> ".repeat(self.quote_depth);
        for (index, marker) in self.item_prefixes.iter().enumerate() {
            if index > 0 {
                prefix.push_str("  ");
            }
            prefix.push_str(marker);
        }
        prefix
    }

    fn flush_line(&mut self) {
        if !self.current.is_empty() {
            self.lines
                .push(Line::from(std::mem::take(&mut self.current)));
        }
    }

    fn in_table_cell(&self) -> bool {
        self.table.as_ref().is_some_and(|table| table.in_cell)
    }

    fn blank_line(&mut self) {
        self.flush_line();
        if self.lines.is_empty() || self.lines.last().is_some_and(|line| line.spans.is_empty()) {
            return;
        }
        self.lines.push(Line::default());
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        if let Some(table) = self.table.take() {
            self.lines.extend(render_markdown_table(table));
        }
        self.flush_line();
        while self.lines.last().is_some_and(|line| line.spans.is_empty()) {
            self.lines.pop();
        }
        if self.lines.is_empty() {
            self.lines.push(Line::default());
        }
        self.lines
    }
}

fn render_markdown_table(mut table: MarkdownTable) -> Vec<Line<'static>> {
    let columns = table
        .alignments
        .len()
        .max(table.header.len())
        .max(table.rows.iter().map(Vec::len).max().unwrap_or_default());
    if columns == 0 {
        return Vec::new();
    }
    table.header.resize(columns, Vec::new());
    for row in &mut table.rows {
        row.resize(columns, Vec::new());
    }

    let mut widths = vec![1; columns];
    for (index, cell) in table.header.iter().enumerate() {
        widths[index] = widths[index].max(table_cell_width(cell));
    }
    for row in &table.rows {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(table_cell_width(cell));
        }
    }

    let mut lines = Vec::with_capacity(table.rows.len() + 3);
    lines.push(render_table_row(
        &table.header,
        &widths,
        &table.alignments,
        true,
    ));
    lines.push(render_table_separator(&widths));
    for row in &table.rows {
        lines.push(render_table_row(row, &widths, &table.alignments, false));
    }
    lines
}

fn table_cell_width(cell: &[Span<'static>]) -> usize {
    Line::from(cell.to_vec()).width().max(1)
}

fn render_table_row(
    cells: &[Vec<Span<'static>>],
    widths: &[usize],
    alignments: &[pulldown_cmark::Alignment],
    header: bool,
) -> Line<'static> {
    let border_style = Style::default().fg(COLOR_MUTED);
    let mut spans = vec![Span::styled("|", border_style)];
    for (index, cell) in cells.iter().enumerate() {
        let width = widths[index];
        let content_width = table_cell_width(cell);
        let padding = width.saturating_sub(content_width);
        let (left, right) = match alignments
            .get(index)
            .copied()
            .unwrap_or(pulldown_cmark::Alignment::None)
        {
            pulldown_cmark::Alignment::None | pulldown_cmark::Alignment::Left => (0, padding),
            pulldown_cmark::Alignment::Center => {
                let left = padding / 2;
                (left, padding - left)
            }
            pulldown_cmark::Alignment::Right => (padding, 0),
        };
        spans.push(Span::styled(" ", border_style));
        if left > 0 {
            spans.push(Span::raw(" ".repeat(left)));
        }
        for span in cell {
            let mut span = span.clone();
            if header {
                span.style = span.style.add_modifier(Modifier::BOLD);
            }
            spans.push(span);
        }
        if right > 0 {
            spans.push(Span::raw(" ".repeat(right)));
        }
        spans.push(Span::styled(" ", border_style));
        spans.push(Span::styled("|", border_style));
    }
    Line::from(spans)
}

fn render_table_separator(widths: &[usize]) -> Line<'static> {
    let border_style = Style::default().fg(COLOR_MUTED);
    let mut spans = Vec::with_capacity(widths.len() * 2 + 1);
    for width in widths {
        spans.push(Span::styled("|", border_style));
        spans.push(Span::styled("-".repeat(width + 2), border_style));
    }
    spans.push(Span::styled("|", border_style));
    Line::from(spans)
}

fn render_markdown(text: &str) -> Vec<Line<'static>> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    let mut renderer = MarkdownRenderer::default();
    for event in Parser::new_ext(text, options) {
        match event {
            MarkdownEvent::Start(tag) => renderer.start(tag),
            MarkdownEvent::End(tag) => renderer.end(tag),
            MarkdownEvent::Text(text) => renderer.text(&text),
            MarkdownEvent::Code(text) => renderer.inline_code(&text),
            MarkdownEvent::SoftBreak => renderer.text(" "),
            MarkdownEvent::HardBreak => {
                if renderer.in_table_cell() {
                    renderer.text(" ");
                } else {
                    renderer.flush_line();
                }
            }
            MarkdownEvent::Rule => {
                renderer.flush_line();
                renderer
                    .current
                    .push(Span::styled("---", Style::default().fg(COLOR_MUTED)));
                renderer.blank_line();
            }
            MarkdownEvent::InlineMath(text) | MarkdownEvent::DisplayMath(text) => {
                renderer.text(&text)
            }
            MarkdownEvent::FootnoteReference(name) => renderer.text(&format!("[{name}]")),
            MarkdownEvent::TaskListMarker(marker) => {
                renderer.text(if marker { "[x] " } else { "[ ] " })
            }
            MarkdownEvent::Html(_) | MarkdownEvent::InlineHtml(_) => {}
        }
    }
    renderer.finish()
}

fn assistant_transcript_lines(text: &str) -> Vec<Line<'static>> {
    let mut lines = render_markdown(text);
    if let Some(first) = lines.first_mut() {
        let mut spans = vec![Span::styled(
            "alpha: ",
            Style::default()
                .fg(COLOR_BRAND)
                .add_modifier(Modifier::BOLD),
        )];
        spans.append(&mut first.spans);
        *first = Line::from(spans);
    }
    lines
}

fn tool_call_summary(name: &str, input: &Value) -> String {
    let detail = match name {
        "shell" => input.get("command").and_then(Value::as_str),
        "read" | "write" | "edit" => input.get("path").and_then(Value::as_str),
        "grep" => input.get("pattern").and_then(Value::as_str),
        _ => None,
    }
    .map(str::to_owned)
    .or_else(|| {
        (!input.is_null())
            .then(|| serde_json::to_string(input).unwrap_or_default())
            .filter(|value| !value.is_empty())
    })
    .unwrap_or_default();
    let detail = detail.replace(['\n', '\r'], " ");
    if detail.trim().is_empty() {
        name.to_owned()
    } else {
        format!("{name}: {}", truncate_summary(detail.trim(), 120))
    }
}

fn tool_result_summary(name: &str, status: &str, content: &str) -> String {
    let line_count = content.lines().count();
    let first = content
        .lines()
        .find(|line| !line.trim().is_empty())
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| truncate_summary(line, 120))
        .unwrap_or_else(|| "(no output)".to_owned());
    if line_count > 1 {
        format!("{name}: {status} - {first} ({line_count} lines)")
    } else {
        format!("{name}: {status} - {first}")
    }
}

fn truncate_summary(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let mut output = String::new();
    for _ in 0..max_chars {
        let Some(character) = chars.next() else {
            return output;
        };
        output.push(character);
    }
    if chars.next().is_some() {
        output.push_str("...");
    }
    output
}

#[derive(Clone, Copy)]
struct CommandSpec {
    name: &'static str,
    usage: &'static str,
    description: &'static str,
    takes_argument: bool,
}

const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        name: "provider",
        usage: "/provider [ID]",
        description: "Open the provider manager or switch provider",
        takes_argument: true,
    },
    CommandSpec {
        name: "model",
        usage: "/model [MODEL] [EFFORT]",
        description: "Open the model picker or switch the route",
        takes_argument: true,
    },
    CommandSpec {
        name: "thinking",
        usage: "/thinking LEVEL",
        description: "Open the effort picker or set effort",
        takes_argument: true,
    },
    CommandSpec {
        name: "effort",
        usage: "/effort LEVEL",
        description: "Set the reasoning effort",
        takes_argument: true,
    },
    CommandSpec {
        name: "mode",
        usage: "/mode MODE",
        description: "Set read, auto, reviewed, or free",
        takes_argument: true,
    },
    CommandSpec {
        name: "confirm",
        usage: "/confirm",
        description: "Run pending steers",
        takes_argument: false,
    },
    CommandSpec {
        name: "discard",
        usage: "/discard",
        description: "Discard pending steers",
        takes_argument: false,
    },
    CommandSpec {
        name: "capabilities",
        usage: "/capabilities TOOLS",
        description: "Set provider-native tools",
        takes_argument: true,
    },
    CommandSpec {
        name: "new",
        usage: "/new",
        description: "Create a new session",
        takes_argument: false,
    },
    CommandSpec {
        name: "resume",
        usage: "/resume ID",
        description: "Resume a session",
        takes_argument: true,
    },
    CommandSpec {
        name: "sessions",
        usage: "/sessions",
        description: "Open the session manager",
        takes_argument: false,
    },
    CommandSpec {
        name: "diagnostics",
        usage: "/diagnostics",
        description: "Show core connection details",
        takes_argument: false,
    },
    CommandSpec {
        name: "help",
        usage: "/help",
        description: "Show help",
        takes_argument: false,
    },
    CommandSpec {
        name: "quit",
        usage: "/quit",
        description: "Exit the TUI",
        takes_argument: false,
    },
];

#[derive(Clone, Debug, PartialEq, Eq)]
struct CommandCompletion {
    start: usize,
    end: usize,
    label: String,
    insert: String,
    description: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Screen {
    Chat,
    Providers,
}

fn model_choices(view: &ProviderView) -> Vec<ModelChoice> {
    let mut choices = Vec::new();
    for instance in view.instances.iter().filter(|instance| !instance.disabled) {
        let Some(kind) = view.kinds.iter().find(|kind| kind.kind == instance.kind) else {
            continue;
        };
        let mut models = view
            .model_cache
            .get(&instance.id)
            .cloned()
            .unwrap_or_else(|| effective_models(instance, &kind.models));
        let default_model = instance
            .model_overrides
            .get("default")
            .and_then(Value::as_str);
        models.sort_by_key(|model| Some(model.id.as_str()) != default_model);
        for model in models {
            choices.push(ModelChoice {
                provider: instance.id.clone(),
                provider_name: kind.name.clone(),
                model: model.id,
                model_name: model.name,
                reasoning_levels: if model.reasoning {
                    model.reasoning_levels
                } else {
                    vec![ReasoningLevel::Auto]
                },
            });
        }
    }
    choices
}

fn current_model_choice<'a>(
    choices: &'a [ModelChoice],
    provider: &str,
    model: &str,
) -> Option<&'a ModelChoice> {
    choices
        .iter()
        .find(|choice| choice.provider == provider && choice.model == model)
}

fn supported_reasoning_levels(
    choices: &[ModelChoice],
    provider: &str,
    model: &str,
) -> Vec<ReasoningLevel> {
    current_model_choice(choices, provider, model)
        .map(|choice| choice.reasoning_levels.clone())
        .unwrap_or_else(|| vec![ReasoningLevel::Auto])
}

fn resolve_model_selection(
    choices: &[ModelChoice],
    current_provider: &str,
    target: &str,
) -> Option<ModelChoice> {
    let target = target.trim();
    if target.is_empty() {
        return None;
    }
    if let Some(choice) = choices
        .iter()
        .find(|choice| format!("{}/{}", choice.provider, choice.model) == target)
        .or_else(|| {
            choices
                .iter()
                .find(|choice| choice.provider == current_provider && choice.model == target)
        })
        .or_else(|| {
            let mut matches = choices.iter().filter(|choice| choice.model == target);
            let first = matches.next()?;
            matches.next().is_none().then_some(first)
        })
    {
        return Some(choice.clone());
    }

    let explicit = choices.iter().find_map(|choice| {
        target
            .strip_prefix(&format!("{}/", choice.provider))
            .map(|model| (choice.provider.clone(), model.to_owned()))
    });
    let (provider, model) = explicit.unwrap_or_else(|| {
        let provider = if choices
            .iter()
            .any(|choice| choice.provider == current_provider)
        {
            current_provider.to_owned()
        } else {
            choices
                .first()
                .map(|choice| choice.provider.clone())
                .unwrap_or_default()
        };
        (provider, target.to_owned())
    });
    if provider.is_empty() || model.is_empty() {
        return None;
    }
    let provider_name = choices
        .iter()
        .find(|choice| choice.provider == provider)
        .map(|choice| choice.provider_name.clone())
        .unwrap_or_else(|| provider.clone());
    Some(ModelChoice {
        provider,
        provider_name,
        model: model.clone(),
        model_name: model,
        reasoning_levels: ReasoningLevel::ALL.to_vec(),
    })
}

fn parse_reasoning_level(value: &str) -> Result<ReasoningLevel, TuiError> {
    serde_json::from_value(Value::String(value.to_owned()))
        .map_err(|error| TuiError::Protocol(error.to_string()))
}

fn set_default_model(
    supervisor: &mut CoreSupervisor,
    provider: &str,
    model: &str,
) -> Result<(), TuiError> {
    supervisor
        .request(
            "provider.set_default_model",
            Some(json!({ "id": provider, "model": model })),
        )
        .map_err(|error| TuiError::Protocol(error.to_string()))?;
    supervisor
        .request(
            "provider.set_default_instance",
            Some(json!({ "id": provider })),
        )
        .map_err(|error| TuiError::Protocol(error.to_string()))?;
    Ok(())
}

struct App {
    chat: ChatView,
    providers: ProviderView,
    screen: Screen,
    session_picker: Option<SessionPicker>,
    model_picker: Option<ModelPicker>,
    thinking_picker: Option<ThinkingPicker>,
    help: bool,
    quit: bool,
}

struct SessionPicker {
    sessions: Vec<SessionEntry>,
    selected: usize,
    confirm_delete: Option<SessionEntry>,
    status: Option<String>,
}

#[derive(Clone, Debug)]
struct SessionEntry {
    id: String,
    mode: String,
    workspace: String,
    provider: String,
    model: String,
    first_message: Option<String>,
}

impl SessionEntry {
    fn from_value(value: &Value) -> Option<Self> {
        Some(Self {
            id: value.get("id")?.as_str()?.to_owned(),
            mode: value
                .get("mode")
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_owned(),
            workspace: value
                .get("workspace")
                .and_then(|workspace| workspace.get("root"))
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_owned(),
            provider: value
                .get("provider")
                .and_then(Value::as_str)
                .unwrap_or("default")
                .to_owned(),
            model: value
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or("unset")
                .to_owned(),
            first_message: value
                .get("firstMessage")
                .and_then(Value::as_str)
                .map(str::to_owned),
        })
    }
}

fn load_sessions(supervisor: &mut CoreSupervisor) -> Result<Vec<SessionEntry>, TuiError> {
    let sessions = supervisor
        .request("session.list", None)
        .map_err(|error| TuiError::Protocol(error.to_string()))?;
    Ok(sessions
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(SessionEntry::from_value)
        .collect())
}

fn open_session_picker(app: &mut App, supervisor: &mut CoreSupervisor) -> Result<(), TuiError> {
    let sessions = load_sessions(supervisor)?;
    app.session_picker = Some(SessionPicker {
        sessions,
        selected: 0,
        confirm_delete: None,
        status: None,
    });
    Ok(())
}

#[derive(Clone, Debug)]
struct ModelChoice {
    provider: String,
    provider_name: String,
    model: String,
    model_name: String,
    reasoning_levels: Vec<ReasoningLevel>,
}

struct ModelPicker {
    choices: Vec<ModelChoice>,
    selected: usize,
}

struct ThinkingPicker {
    levels: Vec<ReasoningLevel>,
    selected: usize,
}

impl ProviderView {
    /// Reload provider state from the managed core.
    pub fn refresh(&mut self, supervisor: &mut CoreSupervisor) -> Result<(), TuiError> {
        let kinds = supervisor
            .request("provider.list_kinds", None)
            .map_err(|error| TuiError::Protocol(error.to_string()))?;
        let instances = supervisor
            .request("provider.list_instances", None)
            .map_err(|error| TuiError::Protocol(error.to_string()))?;
        let credential_status = supervisor
            .request("provider.credential_status", None)
            .map_err(|error| TuiError::Protocol(error.to_string()))?;
        self.kinds =
            serde_json::from_value(kinds).map_err(|error| TuiError::Protocol(error.to_string()))?;
        self.instances = serde_json::from_value(instances)
            .map_err(|error| TuiError::Protocol(error.to_string()))?;
        self.credential_status = Some(parse_credential_status(&credential_status));
        self.selected_kind = self.selected_kind.min(self.kinds.len().saturating_sub(1));
        self.clamp_instance_selection();
        Ok(())
    }

    /// Refresh dynamic model catalogs, retaining static metadata on failure.
    pub fn refresh_models(
        &mut self,
        supervisor: &mut CoreSupervisor,
        provider: Option<&str>,
    ) -> Result<(), TuiError> {
        let mut instance_ids = self
            .instances
            .iter()
            .filter(|instance| !instance.disabled)
            .map(|instance| instance.id.clone())
            .collect::<Vec<_>>();
        if let Some(provider) = provider {
            instance_ids.retain(|id| id == provider);
        }
        self.model_cache
            .retain(|id, _| instance_ids.iter().any(|instance_id| instance_id == id));
        let mut failures = Vec::new();
        for id in instance_ids {
            if self.model_cache.contains_key(&id) {
                continue;
            }
            match supervisor.request("provider.list_models", Some(json!({ "id": id }))) {
                Ok(value) => match serde_json::from_value::<Vec<ModelDescriptor>>(value) {
                    Ok(models) => {
                        self.model_cache.insert(id, models);
                    }
                    Err(error) => failures.push(format!("{id}: {error}")),
                },
                Err(error) => failures.push(format!("{id}: {error}")),
            }
        }
        if !failures.is_empty() {
            self.status = Some(format!("model catalog fallback: {}", failures.join("; ")));
        }
        Ok(())
    }

    fn selected_kind(&self) -> Option<&ProviderDescriptor> {
        self.kinds.get(self.selected_kind)
    }

    fn instances_for_selected_kind(&self) -> Vec<&ProviderInstance> {
        let Some(kind) = self.selected_kind() else {
            return Vec::new();
        };
        self.instances
            .iter()
            .filter(|instance| instance.kind == kind.kind)
            .collect()
    }

    fn selected_instance(&self) -> Option<&ProviderInstance> {
        self.instances_for_selected_kind()
            .get(self.selected_instance)
            .copied()
    }

    fn clamp_instance_selection(&mut self) {
        self.selected_instance = self
            .selected_instance
            .min(self.instances_for_selected_kind().len().saturating_sub(1));
    }

    fn move_kind_selection(&mut self, delta: isize) {
        if self.kinds.is_empty() {
            return;
        }
        let current = self.selected_kind as isize + delta;
        self.selected_kind = current.clamp(0, self.kinds.len() as isize - 1) as usize;
        self.selected_instance = 0;
    }

    fn move_instance_selection(&mut self, delta: isize) {
        let count = self.instances_for_selected_kind().len();
        if count == 0 {
            return;
        }
        let current = self.selected_instance as isize + delta;
        self.selected_instance = current.clamp(0, count as isize - 1) as usize;
    }

    fn create_instance(&mut self, supervisor: &mut CoreSupervisor) -> Result<(), TuiError> {
        let kind = self
            .selected_kind()
            .ok_or_else(|| TuiError::Protocol("no provider kind selected".to_owned()))?;
        let id = format!(
            "{}-{}",
            kind.kind,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        );
        let instance = ProviderInstance {
            id: id.clone(),
            kind: kind.kind.clone(),
            config_version: 1,
            config: kind.default_config.clone(),
            auth_ref: None,
            model_overrides: json!({}),
            disabled: false,
        };
        supervisor
            .request("provider.create_instance", Some(json!(instance)))
            .map_err(|error| TuiError::Protocol(error.to_string()))?;
        self.refresh(supervisor)?;
        if let Some(index) = self
            .instances_for_selected_kind()
            .iter()
            .position(|instance| instance.id == id)
        {
            self.selected_instance = index;
        }
        self.status = Some(format!("created {id}"));
        Ok(())
    }

    fn begin_remove_instance(&mut self) -> Result<(), TuiError> {
        let id = self
            .selected_instance()
            .map(|instance| instance.id.clone())
            .ok_or_else(|| TuiError::Protocol("no provider instance selected".to_owned()))?;
        self.confirm_remove = Some(id);
        Ok(())
    }

    fn confirm_remove_instance(&mut self, supervisor: &mut CoreSupervisor) -> Result<(), TuiError> {
        let Some(id) = self.confirm_remove.take() else {
            return Ok(());
        };
        let response = supervisor
            .request("provider.remove_instance", Some(json!({ "id": id })))
            .map_err(|error| TuiError::Protocol(error.to_string()))?;
        self.refresh(supervisor)?;
        if response
            .get("credentialRemoved")
            .and_then(serde_json::Value::as_bool)
            == Some(false)
        {
            let detail = response
                .get("credentialError")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("credential cleanup failed");
            self.status = Some(format!("removed {id}; {detail}"));
        } else {
            self.status = Some(format!("removed {id}"));
        }
        Ok(())
    }

    fn set_default_instance(&mut self, supervisor: &mut CoreSupervisor) -> Result<(), TuiError> {
        let Some(instance) = self.selected_instance() else {
            return Err(TuiError::Protocol(
                "no provider instance selected".to_owned(),
            ));
        };
        let id = instance.id.clone();
        supervisor
            .request("provider.set_default_instance", Some(json!({ "id": id })))
            .map_err(|error| TuiError::Protocol(error.to_string()))?;
        self.status = Some(format!("default provider: {id}"));
        Ok(())
    }

    fn test_instance(&mut self, supervisor: &mut CoreSupervisor) -> Result<(), TuiError> {
        let Some(instance) = self.selected_instance() else {
            return Err(TuiError::Protocol(
                "no provider instance selected".to_owned(),
            ));
        };
        let id = instance.id.clone();
        supervisor
            .request("provider.test_instance", Some(json!({ "id": id })))
            .map_err(|error| TuiError::Protocol(error.to_string()))?;
        self.status = Some(format!("provider test passed: {id}"));
        Ok(())
    }

    fn begin_auth(&mut self) -> Result<(), TuiError> {
        let Some(instance) = self.selected_instance() else {
            return Err(TuiError::Protocol(
                "no provider instance selected".to_owned(),
            ));
        };
        self.auth_prompt = Some(AuthPrompt {
            instance_id: instance.id.clone(),
            input: String::new(),
        });
        Ok(())
    }

    fn submit_auth(&mut self, supervisor: &mut CoreSupervisor) -> Result<(), TuiError> {
        let prompt = self
            .auth_prompt
            .take()
            .ok_or_else(|| TuiError::Protocol("no auth prompt active".to_owned()))?;
        if prompt.input.is_empty() {
            self.status = Some("credential input was empty".to_owned());
            return Ok(());
        }
        supervisor
            .request(
                "provider.authenticate_instance",
                Some(json!({
                    "id": prompt.instance_id,
                    "secret": prompt.input,
                })),
            )
            .map_err(|error| TuiError::Protocol(error.to_string()))?;
        self.status = Some(format!("credential stored for {}", prompt.instance_id));
        self.refresh(supervisor)?;
        Ok(())
    }

    fn begin_config_edit(&mut self) -> Result<(), TuiError> {
        let instance = self
            .selected_instance()
            .cloned()
            .ok_or_else(|| TuiError::Protocol("no provider instance selected".to_owned()))?;
        let text = serde_json::to_string_pretty(&instance.config)
            .map_err(|error| TuiError::Protocol(error.to_string()))?;
        self.config_editor = Some(ConfigEditor {
            instance,
            text,
            error: None,
        });
        Ok(())
    }

    fn submit_config_edit(&mut self, supervisor: &mut CoreSupervisor) -> Result<(), TuiError> {
        let Some(mut editor) = self.config_editor.take() else {
            return Ok(());
        };
        let config: Value = match serde_json::from_str(&editor.text) {
            Ok(config) => config,
            Err(error) => {
                editor.error = Some(error.to_string());
                self.config_editor = Some(editor);
                return Ok(());
            }
        };
        let original = editor.instance.clone();
        editor.instance.config = config;
        supervisor
            .request("provider.update_instance", Some(json!(editor.instance)))
            .map_err(|error| TuiError::Protocol(error.to_string()))?;
        if let Err(error) = supervisor.request(
            "provider.test_instance",
            Some(json!({ "id": editor.instance.id })),
        ) {
            let _ = supervisor.request("provider.update_instance", Some(json!(original)));
            editor.error = Some(error.to_string());
            self.config_editor = Some(editor);
            return Ok(());
        }
        self.model_cache.remove(&editor.instance.id);
        self.refresh(supervisor)?;
        self.status = Some(format!("updated {}", editor.instance.id));
        Ok(())
    }

    fn credential_hint(&self) -> Option<String> {
        let status = self.credential_status.as_ref()?;
        if status.available {
            None
        } else {
            Some(match status.detail.as_deref() {
                Some(detail) if !detail.is_empty() => {
                    format!("keyring_unavailable: {} ({detail})", status.backend)
                }
                _ => format!("keyring_unavailable: {}", status.backend),
            })
        }
    }

    fn tick_status(&mut self) -> bool {
        if self.status != self.last_status {
            self.last_status = self.status.clone();
            self.status_changed_at = Some(Instant::now());
        }
        if self
            .status_changed_at
            .is_some_and(|changed| changed.elapsed() >= Duration::from_secs(6))
        {
            self.status = None;
            self.last_status = None;
            self.status_changed_at = None;
            return true;
        }
        false
    }
}

fn parse_credential_status(value: &serde_json::Value) -> CredentialStatusView {
    CredentialStatusView {
        backend: value
            .get("backend")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
            .to_owned(),
        available: value
            .get("status")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|status| status == "available"),
        detail: value
            .get("detail")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
    }
}

/// Run the interactive TUI.
pub fn run_tui() -> Result<(), TuiError> {
    let mut supervisor = CoreSupervisor::start(SupervisorConfig::default())?;
    let mut app = App {
        chat: ChatView::default(),
        providers: ProviderView::default(),
        screen: Screen::Providers,
        session_picker: None,
        model_picker: None,
        thinking_picker: None,
        help: false,
        quit: false,
    };
    app.providers.refresh(&mut supervisor)?;
    if !app.providers.instances.is_empty() {
        app.screen = Screen::Chat;
        if let Err(error) = app.chat.ensure_session(&mut supervisor) {
            app.chat.status = Some(error.to_string());
            app.screen = Screen::Providers;
        }
    }

    let mut guard = TerminalGuard::enter()?;
    let result = run_loop(&mut guard.terminal, &mut app, &mut supervisor);
    let session_id = app.chat.session_id.clone();
    drop(guard);
    if let Some(session_id) = session_id {
        println!("session: {session_id}");
        println!("resume: /resume {session_id}");
    }
    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    supervisor: &mut CoreSupervisor,
) -> Result<(), TuiError> {
    let mut redraw = true;
    let mut next_frame = Instant::now();
    loop {
        redraw |= app.chat.expire_interaction(supervisor);
        redraw |= app.chat.tick_status();
        redraw |= app.providers.tick_status();

        let mut core_events = 0;
        while core_events < CORE_EVENT_BATCH_LIMIT {
            let Some(connection_event) = supervisor.try_recv_event()? else {
                break;
            };
            core_events += 1;
            redraw = true;
            match connection_event {
                ConnectionEvent::Closed(error) => {
                    app.chat.status = Some(format!("core disconnected: {error}"));
                    if supervisor.restart().is_ok() {
                        let session_id = app.chat.session_id.clone();
                        if let Some(session_id) = session_id {
                            let _ = app.chat.resume_session(supervisor, &session_id);
                        }
                    }
                }
                event => app.chat.handle_connection_event(event),
            }
        }

        let now = Instant::now();
        if redraw && now >= next_frame {
            terminal.draw(|frame| draw_app(frame, app))?;
            redraw = false;
            next_frame = Instant::now() + FRAME_INTERVAL;
        }

        let wait = if core_events == CORE_EVENT_BATCH_LIMIT {
            Duration::ZERO
        } else if redraw {
            next_frame
                .saturating_duration_since(Instant::now())
                .min(INPUT_POLL_INTERVAL)
        } else {
            INPUT_POLL_INTERVAL
        };
        if event::poll(wait)? {
            match event::read()? {
                Event::Key(key)
                    if matches!(
                        key.kind,
                        crossterm::event::KeyEventKind::Press
                            | crossterm::event::KeyEventKind::Repeat
                    ) =>
                {
                    redraw = true;
                    if key.modifiers.contains(KeyModifiers::CONTROL)
                        && matches!(key.code, KeyCode::Char('c'))
                    {
                        return Ok(());
                    }
                    if app.help {
                        if matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q')) {
                            app.help = false;
                        }
                        continue;
                    }
                    if app.session_picker.is_some() {
                        handle_session_picker_key(app, supervisor, key.code)?;
                        continue;
                    }
                    if app.model_picker.is_some() {
                        handle_model_picker_key(app, supervisor, key.code)?;
                        continue;
                    }
                    if app.thinking_picker.is_some() {
                        handle_thinking_picker_key(app, supervisor, key.code)?;
                        continue;
                    }
                    if app.screen == Screen::Providers {
                        handle_provider_key(app, supervisor, key.code, key.modifiers)?;
                    } else {
                        handle_chat_key(app, supervisor, key.code, key.modifiers)?;
                    }
                    if app.quit {
                        return Ok(());
                    }
                }
                Event::Resize(_, _) => redraw = true,
                Event::Mouse(mouse) => {
                    handle_mouse_event(app, mouse.kind);
                    redraw = true;
                }
                _ => {}
            }
        }
    }
}

fn handle_provider_key(
    app: &mut App,
    supervisor: &mut CoreSupervisor,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> Result<(), TuiError> {
    if app.providers.config_editor.is_some() {
        match code {
            KeyCode::Esc => app.providers.config_editor = None,
            KeyCode::Char('s') if modifiers.contains(KeyModifiers::CONTROL) => {
                if let Err(error) = app.providers.submit_config_edit(supervisor) {
                    if let Some(editor) = &mut app.providers.config_editor {
                        editor.error = Some(error.to_string());
                    }
                }
            }
            KeyCode::Enter => {
                if let Some(editor) = &mut app.providers.config_editor {
                    editor.text.push('\n');
                }
            }
            KeyCode::Tab => {
                if let Some(editor) = &mut app.providers.config_editor {
                    editor.text.push_str("  ");
                }
            }
            KeyCode::Backspace => {
                if let Some(editor) = &mut app.providers.config_editor {
                    editor.text.pop();
                }
            }
            KeyCode::Char(character) if !modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(editor) = &mut app.providers.config_editor {
                    editor.text.push(character);
                }
            }
            _ => {}
        }
        return Ok(());
    }
    if app.providers.auth_prompt.is_some() {
        match code {
            KeyCode::Enter => {
                if let Err(error) = app.providers.submit_auth(supervisor) {
                    app.providers.status = Some(error.to_string());
                }
            }
            KeyCode::Esc => app.providers.auth_prompt = None,
            KeyCode::Backspace => {
                if let Some(prompt) = &mut app.providers.auth_prompt {
                    prompt.input.pop();
                }
            }
            KeyCode::Char(character) if !modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(prompt) = &mut app.providers.auth_prompt {
                    prompt.input.push(character);
                }
            }
            _ => {}
        }
        return Ok(());
    }
    if app.providers.confirm_remove.is_some() {
        match code {
            KeyCode::Char('y') => {
                if let Err(error) = app.providers.confirm_remove_instance(supervisor) {
                    app.providers.status = Some(error.to_string());
                }
            }
            KeyCode::Char('n') | KeyCode::Esc => app.providers.confirm_remove = None,
            _ => {}
        }
        return Ok(());
    }
    match code {
        KeyCode::Esc | KeyCode::Char('q') => {
            app.screen = Screen::Chat;
            if let Err(error) = app.chat.ensure_session(supervisor) {
                app.chat.status = Some(error.to_string());
            }
        }
        KeyCode::Char('r') => {
            app.providers.status = None;
            app.providers.model_cache.clear();
            if let Err(error) = app.providers.refresh(supervisor) {
                app.providers.status = Some(error.to_string());
            }
        }
        KeyCode::Up => app.providers.move_kind_selection(-1),
        KeyCode::Down => app.providers.move_kind_selection(1),
        KeyCode::Left => app.providers.move_instance_selection(-1),
        KeyCode::Right => app.providers.move_instance_selection(1),
        KeyCode::Char('n') => {
            if let Err(error) = app.providers.create_instance(supervisor) {
                app.providers.status = Some(error.to_string());
            }
        }
        KeyCode::Char('x') => {
            if let Err(error) = app.providers.begin_remove_instance() {
                app.providers.status = Some(error.to_string());
            }
        }
        KeyCode::Char('s') => {
            if let Err(error) = app.providers.set_default_instance(supervisor) {
                app.providers.status = Some(error.to_string());
            }
        }
        KeyCode::Char('t') => {
            if let Err(error) = app.providers.test_instance(supervisor) {
                app.providers.status = Some(error.to_string());
            }
        }
        KeyCode::Char('c') => {
            if let Err(error) = app.providers.begin_auth() {
                app.providers.status = Some(error.to_string());
            }
        }
        KeyCode::Char('e') => {
            if let Err(error) = app.providers.begin_config_edit() {
                app.providers.status = Some(error.to_string());
            }
        }
        _ => {}
    }
    Ok(())
}

fn handle_chat_key(
    app: &mut App,
    supervisor: &mut CoreSupervisor,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> Result<(), TuiError> {
    if app.chat.interaction.is_some() {
        match code {
            KeyCode::Char('y') => app
                .chat
                .respond_interaction(supervisor, "once")
                .map(|_| ())?,
            KeyCode::Char('s') => app
                .chat
                .respond_interaction(supervisor, "session")
                .map(|_| ())?,
            KeyCode::Char('n') | KeyCode::Esc => app
                .chat
                .respond_interaction(supervisor, "deny")
                .map(|_| ())?,
            _ => {}
        }
        return Ok(());
    }
    let completions = command_completions(app);
    let completion_visible = !completions.is_empty() && !app.chat.completion_dismissed;
    match code {
        KeyCode::Tab if completion_visible => {
            accept_completion(app);
        }
        KeyCode::BackTab if completion_visible => {
            select_completion(app, -1);
        }
        KeyCode::BackTab => {
            if let Err(error) = cycle_thinking_level(app, supervisor) {
                app.chat.status = Some(error.to_string());
            }
        }
        KeyCode::Char('l') if modifiers.contains(KeyModifiers::CONTROL) => {
            if let Err(error) = open_model_picker(app, supervisor) {
                app.chat.status = Some(error.to_string());
            }
        }
        KeyCode::Char('t') if modifiers.contains(KeyModifiers::CONTROL) => {
            if let Err(error) = open_thinking_picker(app, supervisor) {
                app.chat.status = Some(error.to_string());
            }
        }
        KeyCode::Char('p') if modifiers.contains(KeyModifiers::CONTROL) => {
            app.screen = Screen::Providers;
            app.providers.status = None;
            if let Err(error) = app.providers.refresh(supervisor) {
                app.providers.status = Some(error.to_string());
            }
        }
        KeyCode::Char('r') if modifiers.contains(KeyModifiers::CONTROL) => {
            if let Err(error) = open_session_picker(app, supervisor) {
                app.chat.status = Some(error.to_string());
            }
        }
        KeyCode::Enter if modifiers.contains(KeyModifiers::CONTROL) => {
            app.chat.composer.insert_newline();
            app.chat.reset_completion();
        }
        KeyCode::Char('j') if modifiers.contains(KeyModifiers::CONTROL) => {
            app.chat.composer.insert_newline();
            app.chat.reset_completion();
        }
        KeyCode::Enter => {
            if let Err(error) = submit_composer(app, supervisor) {
                app.chat.status = Some(error.to_string());
            }
        }
        KeyCode::Esc => {
            if completion_visible {
                app.chat.completion_dismissed = true;
            } else if let Err(error) = app.chat.cancel(supervisor) {
                app.chat.status = Some(error.to_string());
            }
        }
        KeyCode::PageUp => app.chat.scroll_transcript_up(10),
        KeyCode::PageDown => app.chat.scroll_transcript_down(10),
        KeyCode::Home if modifiers.contains(KeyModifiers::CONTROL) => {
            app.chat.scroll_transcript_up(u16::MAX);
        }
        KeyCode::End if modifiers.contains(KeyModifiers::CONTROL) => {
            app.chat.scroll_transcript_down(u16::MAX);
        }
        KeyCode::Home | KeyCode::Char('a') if modifiers.contains(KeyModifiers::CONTROL) => {
            app.chat.composer.move_home();
        }
        KeyCode::End | KeyCode::Char('e') if modifiers.contains(KeyModifiers::CONTROL) => {
            app.chat.composer.move_end();
        }
        KeyCode::Home => app.chat.composer.move_home(),
        KeyCode::End => app.chat.composer.move_end(),
        KeyCode::Left => app.chat.composer.move_left(),
        KeyCode::Right => app.chat.composer.move_right(),
        KeyCode::Up if completion_visible => {
            select_completion(app, -1);
        }
        KeyCode::Down if completion_visible => {
            select_completion(app, 1);
        }
        KeyCode::Up => app.chat.composer.move_up(),
        KeyCode::Down => app.chat.composer.move_down(),
        KeyCode::Delete => {
            app.chat.composer.delete();
        }
        KeyCode::Backspace => app.chat.composer.backspace(),
        KeyCode::Char('u') if modifiers.contains(KeyModifiers::CONTROL) => {
            app.chat.composer.delete_to_line_start();
        }
        KeyCode::Char('k') if modifiers.contains(KeyModifiers::CONTROL) => {
            app.chat.composer.delete_to_line_end();
        }
        KeyCode::Char(character) if !modifiers.contains(KeyModifiers::CONTROL) => {
            app.chat.composer.insert_char(character);
            app.chat.reset_completion();
        }
        _ => {}
    }
    Ok(())
}

fn submit_composer(app: &mut App, supervisor: &mut CoreSupervisor) -> Result<(), TuiError> {
    if completion_accepts_enter(app) {
        accept_completion(app);
        return Ok(());
    }
    let input = app.chat.composer.text().trim().to_owned();
    if input.is_empty() {
        return Ok(());
    }
    if input.starts_with('/') {
        app.chat.composer.clear();
        app.chat.reset_completion();
        execute_command(app, supervisor, &input)
    } else {
        app.chat.submit(supervisor)
    }
}

fn completion_accepts_enter(app: &App) -> bool {
    if app.chat.completion_dismissed {
        return false;
    }
    let completions = command_completions(app);
    let Some((start, end, completion)) = selected_completion(&completions, app) else {
        return false;
    };
    &app.chat.composer.text()[start..end] != completion.insert.trim_end()
}

fn accept_completion(app: &mut App) {
    let completions = command_completions(app);
    let Some((start, end, completion)) = selected_completion_owned(&completions, app) else {
        return;
    };
    app.chat
        .composer
        .replace_range(start, end, &completion.insert);
    app.chat.reset_completion();
}

fn select_completion(app: &mut App, delta: isize) {
    let completions = command_completions(app);
    if completions.is_empty() {
        return;
    }
    let current = app.chat.completion_selected.min(completions.len() - 1);
    app.chat.completion_selected = if delta < 0 {
        current.saturating_sub(1)
    } else {
        current.saturating_add(1).min(completions.len() - 1)
    };
}

fn selected_completion<'a>(
    completions: &'a [CommandCompletion],
    app: &App,
) -> Option<(usize, usize, &'a CommandCompletion)> {
    let completion = completions.get(
        app.chat
            .completion_selected
            .min(completions.len().saturating_sub(1)),
    )?;
    Some((completion.start, completion.end, completion))
}

fn selected_completion_owned(
    completions: &[CommandCompletion],
    app: &App,
) -> Option<(usize, usize, CommandCompletion)> {
    let (start, end, completion) = selected_completion(completions, app)?;
    Some((start, end, completion.clone()))
}

fn visible_completions(app: &mut App) -> Vec<CommandCompletion> {
    if app.screen != Screen::Chat
        || app.help
        || app.session_picker.is_some()
        || app.model_picker.is_some()
        || app.thinking_picker.is_some()
        || app.chat.interaction.is_some()
        || app.chat.completion_dismissed
    {
        return Vec::new();
    }
    let completions = command_completions(app);
    if !completions.is_empty() {
        app.chat.completion_selected = app
            .chat
            .completion_selected
            .min(completions.len().saturating_sub(1));
    }
    completions
}

fn command_completions(app: &App) -> Vec<CommandCompletion> {
    let text = app.chat.composer.text();
    let cursor = app.chat.composer.cursor();
    let Some(command_start) = text
        .char_indices()
        .find(|(_, character)| !character.is_whitespace())
        .map(|(index, _)| index)
    else {
        return Vec::new();
    };
    if !text[command_start..].starts_with('/') || cursor < command_start {
        return Vec::new();
    }
    let command_end = text[command_start..]
        .find(char::is_whitespace)
        .map_or(text.len(), |index| command_start + index);
    if cursor <= command_end {
        let (start, end) = completion_token(text, cursor);
        let prefix = &text[start..cursor];
        if !prefix.starts_with('/') {
            return Vec::new();
        }
        let prefix = &prefix[1..];
        return COMMANDS
            .iter()
            .filter(|command| command.name.starts_with(prefix))
            .map(|command| {
                let insert = if command.takes_argument {
                    format!("/{} ", command.name)
                } else {
                    format!("/{}", command.name)
                };
                CommandCompletion {
                    start,
                    end,
                    label: command.usage.to_owned(),
                    insert,
                    description: command.description.to_owned(),
                }
            })
            .collect();
    }

    let command = text[command_start..command_end]
        .strip_prefix('/')
        .unwrap_or(&text[command_start..command_end]);
    let (start, end) = completion_token(text, cursor);
    let prefix = &text[start..cursor];
    let mut values = Vec::new();
    match command {
        "mode" => {
            values.extend(
                ["read", "auto", "reviewed", "free"]
                    .into_iter()
                    .map(|value| {
                        (
                            value.to_owned(),
                            value.to_owned(),
                            format!("permission mode {value}"),
                        )
                    }),
            );
        }
        "thinking" | "effort" => {
            values.extend(ReasoningLevel::ALL.into_iter().map(|level| {
                (
                    level.as_str().to_owned(),
                    level.as_str().to_owned(),
                    reasoning_description(level).to_owned(),
                )
            }));
        }
        "provider" => {
            values.extend(app.providers.instances.iter().map(|instance| {
                (
                    instance.id.clone(),
                    instance.id.clone(),
                    format!("provider kind {}", instance.kind),
                )
            }));
        }
        "model" => {
            let argument_count = text[command_end..cursor].split_whitespace().count();
            if argument_count >= 2 {
                values.extend(ReasoningLevel::ALL.into_iter().map(|level| {
                    (
                        level.as_str().to_owned(),
                        level.as_str().to_owned(),
                        reasoning_description(level).to_owned(),
                    )
                }));
            } else {
                values.extend(model_choices(&app.providers).into_iter().map(|choice| {
                    let label = format!("{}/{}", choice.provider, choice.model);
                    (label.clone(), label, choice.model_name)
                }));
                if values.is_empty() && !app.chat.model.is_empty() {
                    values.push((
                        app.chat.model.clone(),
                        app.chat.model.clone(),
                        app.chat.model.clone(),
                    ));
                }
            }
        }
        "capabilities" => {
            let mut tools = BTreeMap::new();
            for kind in &app.providers.kinds {
                for model in &kind.models {
                    for tool in &model.provider_tools {
                        tools.insert(tool.clone(), kind.name.clone());
                    }
                }
            }
            values.extend(
                tools
                    .into_iter()
                    .map(|(tool, provider)| (tool.clone(), tool, provider)),
            );
        }
        _ => {}
    }
    let prefix = prefix.to_ascii_lowercase();
    values
        .into_iter()
        .filter(|(value, _, _)| value.to_ascii_lowercase().starts_with(&prefix))
        .map(|(label, insert, description)| CommandCompletion {
            start,
            end,
            label,
            insert,
            description,
        })
        .collect()
}

fn completion_token(text: &str, cursor: usize) -> (usize, usize) {
    let start = text[..cursor]
        .rfind(char::is_whitespace)
        .map_or(0, |index| index + 1);
    let end = text[cursor..]
        .find(char::is_whitespace)
        .map_or(text.len(), |index| cursor + index);
    (start, end)
}

fn handle_mouse_event(app: &mut App, kind: MouseEventKind) {
    if app.screen != Screen::Chat
        || app.help
        || app.session_picker.is_some()
        || app.model_picker.is_some()
        || app.thinking_picker.is_some()
        || app.chat.interaction.is_some()
    {
        return;
    }
    match kind {
        MouseEventKind::ScrollUp => app.chat.scroll_transcript_up(3),
        MouseEventKind::ScrollDown => app.chat.scroll_transcript_down(3),
        _ => {}
    }
}

fn execute_command(
    app: &mut App,
    supervisor: &mut CoreSupervisor,
    command: &str,
) -> Result<(), TuiError> {
    let (name, argument) = split_command(command);
    match name {
        "provider" if argument.is_empty() => {
            app.screen = Screen::Providers;
            app.providers.refresh(supervisor)?;
        }
        "provider" => {
            app.providers.refresh(supervisor)?;
            app.providers.refresh_models(supervisor, Some(argument))?;
            let choices = model_choices(&app.providers);
            let Some(choice) = choices.iter().find(|choice| choice.provider == argument) else {
                return Err(TuiError::Protocol(format!(
                    "unknown provider instance: {argument}"
                )));
            };
            let reasoning = if choice.reasoning_levels.contains(&app.chat.reasoning) {
                app.chat.reasoning
            } else {
                choice
                    .reasoning_levels
                    .first()
                    .copied()
                    .unwrap_or(ReasoningLevel::Auto)
            };
            app.chat.set_route(
                supervisor,
                choice.provider.clone(),
                choice.model.clone(),
                reasoning,
            )?;
            set_default_model(supervisor, &choice.provider, &choice.model)?;
        }
        "model" if argument.is_empty() => {
            open_model_picker(app, supervisor)?;
        }
        "model" => {
            let mut parts = argument.split_whitespace();
            let target = parts.next().unwrap_or_default();
            let requested_reasoning = parts
                .next()
                .map(parse_reasoning_level)
                .transpose()?
                .unwrap_or(app.chat.reasoning);
            if parts.next().is_some() {
                return Err(TuiError::Protocol(
                    "usage: /model <model> [effort]".to_owned(),
                ));
            }
            app.providers.refresh(supervisor)?;
            app.providers
                .refresh_models(supervisor, Some(&app.chat.provider))?;
            let choices = model_choices(&app.providers);
            let selected = resolve_model_selection(&choices, &app.chat.provider, target)
                .ok_or_else(|| TuiError::Protocol(format!("unknown model: {target}")))?;
            let reasoning = if selected.reasoning_levels.contains(&requested_reasoning) {
                requested_reasoning
            } else {
                selected
                    .reasoning_levels
                    .first()
                    .copied()
                    .unwrap_or(ReasoningLevel::Auto)
            };
            app.chat.set_route(
                supervisor,
                selected.provider.clone(),
                selected.model.clone(),
                reasoning,
            )?;
            set_default_model(supervisor, &selected.provider, &selected.model)?;
        }
        "thinking" | "effort" if argument.is_empty() => {
            open_thinking_picker(app, supervisor)?;
        }
        "thinking" | "effort" => {
            let level = parse_reasoning_level(argument)?;
            app.providers.refresh(supervisor)?;
            app.providers
                .refresh_models(supervisor, Some(&app.chat.provider))?;
            let choices = model_choices(&app.providers);
            let levels = supported_reasoning_levels(&choices, &app.chat.provider, &app.chat.model);
            if !levels.contains(&level) {
                return Err(TuiError::Protocol(format!(
                    "reasoning effort `{level}` is not available for this model"
                )));
            }
            app.chat.set_route(
                supervisor,
                app.chat.provider.clone(),
                app.chat.model.clone(),
                level,
            )?;
        }
        "mode" if argument.is_empty() => {
            return Err(TuiError::Protocol(
                "usage: /mode <read|auto|reviewed|free>".to_owned(),
            ));
        }
        "mode" => {
            let mode = argument
                .parse::<PermissionMode>()
                .map_err(|error| TuiError::Protocol(error.to_string()))?;
            app.chat.set_mode(supervisor, mode)?;
        }
        "confirm" => app.chat.confirm_steers(supervisor, true)?,
        "discard" => app.chat.confirm_steers(supervisor, false)?,
        "capabilities" => {
            let provider_tools = argument
                .split([',', ' '])
                .filter(|tool| !tool.is_empty())
                .map(str::to_owned)
                .collect();
            app.chat.set_capabilities(supervisor, provider_tools)?;
        }
        "new" => {
            app.chat.new_session(supervisor)?;
            app.screen = Screen::Chat;
        }
        "resume" if !argument.is_empty() => {
            app.chat.resume_session(supervisor, argument)?;
            app.screen = Screen::Chat;
        }
        "resume" => {
            return Err(TuiError::Protocol("usage: /resume <SESSION_ID>".to_owned()));
        }
        "sessions" => {
            open_session_picker(app, supervisor)?;
        }
        "diagnostics" => {
            let info = supervisor.info();
            app.chat.status = Some(format!(
                "core pid={} protocol={} instance={}",
                info.pid, info.protocol, info.core_instance_id
            ));
        }
        "help" => app.help = true,
        "quit" => app.quit = true,
        "" => {}
        other => app.chat.status = Some(format!("unknown command: {other}")),
    }
    Ok(())
}

fn split_command(command: &str) -> (&str, &str) {
    let command = command
        .trim()
        .strip_prefix('/')
        .unwrap_or_else(|| command.trim());
    command
        .split_once(char::is_whitespace)
        .map_or((command, ""), |(name, argument)| (name, argument.trim()))
}

fn handle_session_picker_key(
    app: &mut App,
    supervisor: &mut CoreSupervisor,
    code: KeyCode,
) -> Result<(), TuiError> {
    let Some(picker) = &mut app.session_picker else {
        return Ok(());
    };
    if picker.confirm_delete.is_some() {
        match code {
            KeyCode::Char('y') => {
                let Some(session) = picker.confirm_delete.take() else {
                    return Ok(());
                };
                if let Err(error) = delete_session(app, supervisor, &session.id) {
                    if let Some(picker) = &mut app.session_picker {
                        picker.status = Some(error.to_string());
                    }
                } else {
                    let sessions = load_sessions(supervisor)?;
                    if let Some(picker) = &mut app.session_picker {
                        picker.sessions = sessions;
                        picker.selected =
                            picker.selected.min(picker.sessions.len().saturating_sub(1));
                        picker.status = Some(format!("deleted {}", short_session_id(&session.id)));
                    }
                }
            }
            KeyCode::Char('n') | KeyCode::Esc => picker.confirm_delete = None,
            _ => {}
        }
        return Ok(());
    }
    match code {
        KeyCode::Esc => app.session_picker = None,
        KeyCode::Up | KeyCode::Char('k') => {
            picker.selected = picker.selected.saturating_sub(1);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            picker.selected = picker
                .selected
                .saturating_add(1)
                .min(picker.sessions.len().saturating_sub(1));
        }
        KeyCode::PageUp => picker.selected = picker.selected.saturating_sub(10),
        KeyCode::PageDown => {
            picker.selected = picker
                .selected
                .saturating_add(10)
                .min(picker.sessions.len().saturating_sub(1));
        }
        KeyCode::Enter => {
            let selected = picker.sessions.get(picker.selected).cloned();
            if let Some(session) = selected {
                app.chat.resume_session(supervisor, &session.id)?;
                app.screen = Screen::Chat;
            }
            app.session_picker = None;
        }
        KeyCode::Char('n') => {
            app.chat.new_session(supervisor)?;
            app.screen = Screen::Chat;
            app.session_picker = None;
        }
        KeyCode::Char('d') => {
            picker.confirm_delete = picker.sessions.get(picker.selected).cloned();
        }
        KeyCode::Char('r') => {
            picker.status = None;
            picker.sessions = load_sessions(supervisor)?;
            picker.selected = picker.selected.min(picker.sessions.len().saturating_sub(1));
        }
        _ => {}
    }
    Ok(())
}

fn delete_session(
    app: &mut App,
    supervisor: &mut CoreSupervisor,
    session_id: &str,
) -> Result<(), TuiError> {
    supervisor
        .request("session.delete", Some(json!({ "sessionId": session_id })))
        .map_err(|error| TuiError::Protocol(error.to_string()))?;
    if app.chat.session_id.as_deref() == Some(session_id) {
        app.chat.new_session(supervisor)?;
    }
    Ok(())
}

fn open_model_picker(app: &mut App, supervisor: &mut CoreSupervisor) -> Result<(), TuiError> {
    app.providers.refresh(supervisor)?;
    app.providers
        .refresh_models(supervisor, Some(&app.chat.provider))?;
    let mut choices = model_choices(&app.providers);
    if !app.chat.provider.is_empty()
        && !app.chat.model.is_empty()
        && !choices
            .iter()
            .any(|choice| choice.provider == app.chat.provider && choice.model == app.chat.model)
    {
        let provider_name = app
            .providers
            .instances
            .iter()
            .find(|instance| instance.id == app.chat.provider)
            .and_then(|instance| {
                app.providers
                    .kinds
                    .iter()
                    .find(|kind| kind.kind == instance.kind)
            })
            .map(|kind| kind.name.clone())
            .unwrap_or_else(|| app.chat.provider.clone());
        choices.push(ModelChoice {
            provider: app.chat.provider.clone(),
            provider_name,
            model: app.chat.model.clone(),
            model_name: app.chat.model.clone(),
            reasoning_levels: ReasoningLevel::ALL.to_vec(),
        });
    }
    if choices.is_empty() {
        app.chat.status = Some("no models configured".to_owned());
        return Ok(());
    }
    let selected = choices
        .iter()
        .position(|choice| choice.provider == app.chat.provider && choice.model == app.chat.model)
        .unwrap_or(0);
    app.model_picker = Some(ModelPicker { choices, selected });
    Ok(())
}

fn handle_model_picker_key(
    app: &mut App,
    supervisor: &mut CoreSupervisor,
    code: KeyCode,
) -> Result<(), TuiError> {
    let Some(picker) = &mut app.model_picker else {
        return Ok(());
    };
    match code {
        KeyCode::Esc => app.model_picker = None,
        KeyCode::Up => picker.selected = picker.selected.saturating_sub(1),
        KeyCode::Down => {
            picker.selected = picker
                .selected
                .saturating_add(1)
                .min(picker.choices.len().saturating_sub(1));
        }
        KeyCode::PageUp => picker.selected = picker.selected.saturating_sub(10),
        KeyCode::PageDown => {
            picker.selected = picker
                .selected
                .saturating_add(10)
                .min(picker.choices.len().saturating_sub(1));
        }
        KeyCode::Enter => {
            let Some(choice) = picker.choices.get(picker.selected).cloned() else {
                return Ok(());
            };
            let reasoning = if choice.reasoning_levels.contains(&app.chat.reasoning) {
                app.chat.reasoning
            } else {
                choice
                    .reasoning_levels
                    .first()
                    .copied()
                    .unwrap_or(ReasoningLevel::Auto)
            };
            app.chat
                .set_route(supervisor, choice.provider, choice.model, reasoning)?;
            set_default_model(supervisor, &app.chat.provider, &app.chat.model)?;
            app.model_picker = None;
        }
        _ => {}
    }
    Ok(())
}

fn open_thinking_picker(app: &mut App, supervisor: &mut CoreSupervisor) -> Result<(), TuiError> {
    app.providers.refresh(supervisor)?;
    app.providers
        .refresh_models(supervisor, Some(&app.chat.provider))?;
    let choices = model_choices(&app.providers);
    let levels = supported_reasoning_levels(&choices, &app.chat.provider, &app.chat.model);
    if levels.len() <= 1 {
        app.chat.status = Some("current model has no reasoning-effort controls".to_owned());
        return Ok(());
    }
    let selected = levels
        .iter()
        .position(|level| *level == app.chat.reasoning)
        .unwrap_or(0);
    app.thinking_picker = Some(ThinkingPicker { levels, selected });
    Ok(())
}

fn handle_thinking_picker_key(
    app: &mut App,
    supervisor: &mut CoreSupervisor,
    code: KeyCode,
) -> Result<(), TuiError> {
    let Some(picker) = &mut app.thinking_picker else {
        return Ok(());
    };
    match code {
        KeyCode::Esc => app.thinking_picker = None,
        KeyCode::Up => picker.selected = picker.selected.saturating_sub(1),
        KeyCode::Down => {
            picker.selected = picker
                .selected
                .saturating_add(1)
                .min(picker.levels.len().saturating_sub(1));
        }
        KeyCode::Enter => {
            let Some(level) = picker.levels.get(picker.selected).copied() else {
                return Ok(());
            };
            if app.chat.provider.is_empty() || app.chat.model.is_empty() {
                app.chat.status = Some("select a model first".to_owned());
                app.thinking_picker = None;
                return Ok(());
            }
            app.chat.set_route(
                supervisor,
                app.chat.provider.clone(),
                app.chat.model.clone(),
                level,
            )?;
            app.thinking_picker = None;
        }
        _ => {}
    }
    Ok(())
}

fn cycle_thinking_level(app: &mut App, supervisor: &mut CoreSupervisor) -> Result<(), TuiError> {
    if app.chat.provider.is_empty() || app.chat.model.is_empty() {
        app.chat.status = Some("select a model first".to_owned());
        return Ok(());
    }
    app.providers.refresh(supervisor)?;
    app.providers
        .refresh_models(supervisor, Some(&app.chat.provider))?;
    let choices = model_choices(&app.providers);
    let levels = supported_reasoning_levels(&choices, &app.chat.provider, &app.chat.model);
    if levels.len() <= 1 {
        app.chat.status = Some("current model has no reasoning-effort controls".to_owned());
        return Ok(());
    }
    let current = levels
        .iter()
        .position(|level| *level == app.chat.reasoning)
        .unwrap_or(0);
    let next = levels[(current + 1) % levels.len()];
    app.chat.set_route(
        supervisor,
        app.chat.provider.clone(),
        app.chat.model.clone(),
        next,
    )
}

fn draw_app(frame: &mut ratatui::Frame<'_>, app: &mut App) {
    let completions = visible_completions(app);
    let show_composer_cursor = app.screen == Screen::Chat
        && !app.help
        && app.session_picker.is_none()
        && app.model_picker.is_none()
        && app.thinking_picker.is_none()
        && app.chat.interaction.is_none();
    match app.screen {
        Screen::Chat => {
            draw_chat_screen(frame, &mut app.chat, &completions, show_composer_cursor);
        }
        Screen::Providers => draw_provider_screen(frame, &app.providers),
    }
    if let Some(picker) = &app.session_picker {
        draw_session_picker(frame, picker);
    }
    if let Some(picker) = &app.model_picker {
        draw_model_picker(frame, picker, &app.chat);
    }
    if let Some(picker) = &app.thinking_picker {
        draw_thinking_picker(frame, picker);
    }
    if app.help {
        draw_help(frame);
    }
    if let Some(prompt) = &app.chat.interaction {
        draw_interaction_prompt(frame, prompt);
    }
}

fn draw_chat_screen(
    frame: &mut ratatui::Frame<'_>,
    view: &mut ChatView,
    completions: &[CommandCompletion],
    show_cursor: bool,
) {
    let area = frame.area();
    let completion_height = if completions.is_empty() {
        0
    } else {
        u16::try_from(completions.len().min(5))
            .unwrap_or(5)
            .saturating_add(2)
    };
    let composer_height = composer_height(&view.composer, area.width);
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(5),
            Constraint::Length(completion_height),
            Constraint::Length(composer_height),
            Constraint::Length(1),
        ])
        .split(area);
    let session = view.session_id.as_deref().unwrap_or("new session");
    let header = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(layout[0]);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "alpha",
                Style::default()
                    .fg(COLOR_BRAND)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  {}", short_session_id(session)),
                Style::default().fg(COLOR_MUTED),
            ),
        ])),
        header[0],
    );
    let provider = if view.provider.is_empty() {
        "default"
    } else {
        view.provider.as_str()
    };
    let model = if view.model.is_empty() {
        "unset"
    } else {
        view.model.as_str()
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!("{provider}/{model}"),
                Style::default().fg(COLOR_TEXT),
            ),
            Span::styled("  effort:", Style::default().fg(COLOR_MUTED)),
            Span::styled(
                view.reasoning.to_string(),
                Style::default().fg(COLOR_SUCCESS),
            ),
            Span::styled("  mode:", Style::default().fg(COLOR_MUTED)),
            Span::styled(view.mode.to_string(), Style::default().fg(COLOR_WARNING)),
        ]))
        .alignment(Alignment::Right),
        header[1],
    );

    let width = layout[1].width;
    ensure_transcript_cache(view, width);
    let cache = view
        .transcript_cache
        .as_ref()
        .expect("transcript cache must be populated");
    let transcript_title = if view.transcript_scroll_back > 0 {
        "Transcript (scrolled)"
    } else {
        "Transcript"
    };
    let max_scroll = cache
        .line_count
        .saturating_sub(usize::from(layout[1].height.saturating_sub(2)));
    let scroll = max_scroll
        .saturating_sub(usize::from(view.transcript_scroll_back))
        .min(u16::MAX as usize);
    let (line_start, entry_scroll) = transcript_viewport(cache, scroll);
    let transcript_widget = Paragraph::new(Text::from(cache.lines[line_start..].to_vec()))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(COLOR_MUTED))
                .title(Span::styled(
                    transcript_title,
                    Style::default().fg(COLOR_BRAND),
                )),
        )
        .style(Style::default().fg(COLOR_TEXT))
        .wrap(Wrap { trim: false });
    frame.render_widget(
        transcript_widget.scroll((entry_scroll.min(u16::MAX as usize) as u16, 0)),
        layout[1],
    );
    if !completions.is_empty() {
        draw_command_completions(frame, layout[2], completions, view.completion_selected);
    }
    let composer_area = layout[3];
    let visual = composer_visual(&view.composer, composer_area.width.saturating_sub(2));
    let visible_rows = usize::from(composer_area.height.saturating_sub(2)).max(1);
    let scroll = visual
        .cursor_row
        .saturating_sub(visible_rows.saturating_sub(1));
    frame.render_widget(
        Paragraph::new(Text::from(visual.lines))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(COLOR_MUTED))
                    .title(Span::styled("Composer", Style::default().fg(COLOR_BRAND))),
            )
            .scroll((scroll.min(u16::MAX as usize) as u16, 0)),
        composer_area,
    );
    if show_cursor {
        let cursor_x = composer_area
            .x
            .saturating_add(1)
            .saturating_add(visual.cursor_col.min(u16::MAX as usize) as u16)
            .min(composer_area.right().saturating_sub(2));
        let cursor_y = composer_area
            .y
            .saturating_add(1)
            .saturating_add(
                visual
                    .cursor_row
                    .saturating_sub(scroll)
                    .min(u16::MAX as usize) as u16,
            )
            .min(composer_area.bottom().saturating_sub(2));
        frame.set_cursor_position((cursor_x, cursor_y));
    }
    let status = view
        .status
        .as_deref()
        .unwrap_or("Enter send  Tab complete  Ctrl-R sessions  / commands");
    frame.render_widget(
        Paragraph::new(status).style(if view.status.is_some() {
            Style::default().fg(COLOR_WARNING)
        } else {
            Style::default().fg(COLOR_MUTED)
        }),
        layout[4],
    );
}

fn short_session_id(session: &str) -> &str {
    if session.len() <= 12 {
        session
    } else {
        &session[..8]
    }
}

fn transcript_viewport(cache: &TranscriptCache, scroll: usize) -> (usize, usize) {
    if cache.entries.is_empty() {
        return (0, 0);
    }
    let mut consumed = 0usize;
    for entry in &cache.entries {
        let next = consumed.saturating_add(entry.line_count);
        if scroll < next {
            return (entry.line_start, scroll.saturating_sub(consumed));
        }
        consumed = next;
    }
    (cache.lines.len(), 0)
}

fn ensure_transcript_cache(view: &mut ChatView, width: u16) {
    let stale = view.transcript_cache.as_ref().is_none_or(|cache| {
        cache.width != width
            || cache.entries.len() != view.transcript.len()
            || view.transcript_cache_dirty
    });
    if stale {
        rebuild_transcript_cache(view, width);
    }
}

fn rebuild_transcript_cache(view: &mut ChatView, width: u16) {
    view.ensure_transcript_entry_revisions();
    let (previous_width, previous_entries, mut previous_lines) = match view.transcript_cache.take()
    {
        Some(cache) => (
            Some(cache.width),
            cache.entries,
            Some(cache.lines.into_iter()),
        ),
        None => (None, Vec::new(), None),
    };
    let width_changed = previous_width != Some(width);
    let mut entries = Vec::with_capacity(view.transcript.len());
    let mut lines = Vec::new();
    let mut line_count = 0usize;

    for (index, entry) in view.transcript.iter().enumerate() {
        let revision = view.transcript_entry_revisions[index];
        let previous = previous_entries.get(index);
        let previous_len = previous.map_or(0, |cache_entry| cache_entry.line_len);
        let reusable = previous.filter(|cache_entry| cache_entry.revision == revision);
        let (entry_lines, entry_line_count) = if let Some(previous) = reusable {
            let entry_lines = previous_lines
                .as_mut()
                .map(|lines| lines.by_ref().take(previous_len).collect::<Vec<_>>())
                .unwrap_or_default();
            let entry_line_count = if width_changed {
                wrapped_line_count(&entry_lines, width)
            } else {
                previous.line_count
            };
            (entry_lines, entry_line_count)
        } else {
            if let Some(lines) = previous_lines.as_mut() {
                lines.by_ref().take(previous_len).for_each(drop);
            }
            let entry_lines = build_transcript_entry_lines(entry);
            let entry_line_count = wrapped_line_count(&entry_lines, width);
            (entry_lines, entry_line_count)
        };
        let line_start = lines.len();
        entries.push(TranscriptCacheEntry {
            revision,
            line_start,
            line_len: entry_lines.len(),
            line_count: entry_line_count,
        });
        line_count = line_count.saturating_add(entry_line_count);
        lines.extend(entry_lines);
    }

    if view.transcript.is_empty() {
        lines = vec![Line::styled(
            "No messages yet",
            Style::default().fg(COLOR_MUTED),
        )];
        line_count = wrapped_line_count(&lines, width);
    }

    view.transcript_cache = Some(TranscriptCache {
        width,
        entries,
        lines,
        line_count,
    });
    view.transcript_cache_dirty = false;
}

fn wrapped_line_count(lines: &[Line<'_>], width: u16) -> usize {
    Paragraph::new(Text::from(lines.to_vec()))
        .wrap(Wrap { trim: false })
        .line_count(width.saturating_sub(2))
}

fn build_transcript_entry_lines(entry: &TranscriptEntry) -> Vec<Line<'static>> {
    match entry {
        TranscriptEntry::User(text) => vec![Line::from(vec![
            Span::styled(
                "you: ",
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(text.clone(), Style::default().fg(COLOR_TEXT)),
        ])],
        TranscriptEntry::Assistant(text) => assistant_transcript_lines(text),
        TranscriptEntry::Thinking(text) => thinking_transcript_lines(text),
        TranscriptEntry::Progress(text) => vec![Line::from(vec![
            Span::styled("... ", Style::default().fg(COLOR_WARNING)),
            Span::styled(text.clone(), Style::default().fg(COLOR_MUTED)),
        ])],
        TranscriptEntry::Error(text) => vec![Line::from(vec![
            Span::styled(
                "error: ",
                Style::default()
                    .fg(COLOR_ERROR)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(text.clone(), Style::default().fg(COLOR_TEXT)),
        ])],
    }
}

fn thinking_transcript_lines(text: &str) -> Vec<Line<'static>> {
    let style = Style::default()
        .fg(COLOR_MUTED)
        .add_modifier(Modifier::ITALIC);
    let mut lines = Vec::new();
    for (index, part) in text.lines().enumerate() {
        let prefix = if index == 0 { "effort: " } else { "        " };
        lines.push(Line::from(vec![
            Span::styled(
                prefix,
                Style::default()
                    .fg(COLOR_BRAND)
                    .add_modifier(Modifier::ITALIC),
            ),
            Span::styled(part.to_owned(), style),
        ]));
    }
    if lines.is_empty() {
        lines.push(Line::styled("effort:", Style::default().fg(COLOR_BRAND)));
    }
    lines
}

fn draw_session_picker(frame: &mut ratatui::Frame<'_>, picker: &SessionPicker) {
    let area = centered_rect(90, 22, frame.area());
    frame.render_widget(Clear, area);
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(4), Constraint::Length(2)])
        .split(area);
    let items: Vec<ListItem<'_>> = if picker.sessions.is_empty() {
        vec![ListItem::new("No stored sessions")]
    } else {
        picker
            .sessions
            .iter()
            .map(|session| {
                let route = if session.provider.is_empty() && session.model.is_empty() {
                    "default".to_owned()
                } else {
                    format!("{}/{}", session.provider, session.model)
                };
                let preview = session
                    .first_message
                    .as_deref()
                    .map(|text| {
                        truncate_summary(&text.split_whitespace().collect::<Vec<_>>().join(" "), 96)
                    })
                    .filter(|text| !text.is_empty())
                    .unwrap_or_else(|| "(empty session)".to_owned());
                ListItem::new(Text::from(vec![
                    Line::from(vec![
                        Span::styled(
                            format!("{}  ", short_session_id(&session.id)),
                            Style::default()
                                .fg(COLOR_BRIGHT)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!("{}  {}  {}", session.mode, route, session.workspace),
                            Style::default().fg(COLOR_MUTED),
                        ),
                    ]),
                    Line::from(vec![
                        Span::styled("  ", Style::default().fg(COLOR_MUTED)),
                        Span::styled(preview, Style::default().fg(COLOR_TEXT)),
                    ]),
                ]))
            })
            .collect()
    };
    let mut state = RatatuiListState::default().with_selected(Some(picker.selected));
    frame.render_stateful_widget(
        List::new(items)
            .highlight_style(
                Style::default()
                    .fg(COLOR_BRAND)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("> ")
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(COLOR_MUTED))
                    .title(Span::styled(
                        format!("Sessions ({})", picker.sessions.len()),
                        Style::default().fg(COLOR_BRAND),
                    )),
            ),
        layout[0],
        &mut state,
    );
    let footer = picker
        .status
        .as_deref()
        .unwrap_or("Enter switch   n new   d delete   r refresh   Up/Down move   Esc back");
    frame.render_widget(
        Paragraph::new(footer)
            .style(if picker.status.is_some() {
                Style::default().fg(COLOR_ERROR)
            } else {
                Style::default().fg(COLOR_MUTED)
            })
            .wrap(Wrap { trim: false }),
        layout[1],
    );
    if let Some(session) = &picker.confirm_delete {
        draw_session_delete_confirmation(frame, session);
    }
}

fn draw_model_picker(frame: &mut ratatui::Frame<'_>, picker: &ModelPicker, chat: &ChatView) {
    let area = centered_rect(78, 20, frame.area());
    frame.render_widget(Clear, area);
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(area);
    let capacity = usize::from(layout[0].height.saturating_sub(2)).max(1);
    let start = picker
        .selected
        .saturating_sub(capacity / 2)
        .min(picker.choices.len().saturating_sub(capacity));
    let visible = picker
        .choices
        .iter()
        .enumerate()
        .skip(start)
        .take(capacity)
        .map(|(index, choice)| {
            let marker = if index == picker.selected { ">" } else { " " };
            let current = choice.provider == chat.provider && choice.model == chat.model;
            let marker_style = if index == picker.selected {
                Style::default()
                    .fg(COLOR_BRAND)
                    .add_modifier(Modifier::BOLD)
            } else if current {
                Style::default().fg(COLOR_SUCCESS)
            } else {
                Style::default().fg(COLOR_MUTED)
            };
            let model_style = if current {
                Style::default().fg(COLOR_TEXT).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(COLOR_TEXT)
            };
            let levels = choice
                .reasoning_levels
                .iter()
                .map(|level| level.as_str())
                .collect::<Vec<_>>()
                .join("/");
            ListItem::new(Line::from(vec![
                Span::styled(format!("{marker} "), marker_style),
                Span::styled(
                    format!("[{}] ", choice.provider_name),
                    Style::default().fg(COLOR_ACCENT),
                ),
                Span::styled(choice.model_name.clone(), model_style),
                Span::styled(
                    format!("  {}  effort:{}", choice.model, levels),
                    Style::default().fg(COLOR_MUTED),
                ),
            ]))
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(visible).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(COLOR_MUTED))
                .title(Span::styled(
                    format!("Models ({}/{})", start + 1, picker.choices.len()),
                    Style::default().fg(COLOR_BRAND),
                )),
        ),
        layout[0],
    );
    frame.render_widget(
        Paragraph::new("Enter apply   Up/Down move   Esc cancel")
            .style(Style::default().fg(COLOR_MUTED)),
        layout[1],
    );
}

fn draw_session_delete_confirmation(frame: &mut ratatui::Frame<'_>, session: &SessionEntry) {
    let area = centered_rect(62, 8, frame.area());
    frame.render_widget(Clear, area);
    let preview = session
        .first_message
        .as_deref()
        .map(|text| truncate_summary(&text.split_whitespace().collect::<Vec<_>>().join(" "), 80))
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "(empty session)".to_owned());
    let text = Text::from(vec![
        Line::from(format!("Delete session {}?", short_session_id(&session.id))),
        Line::from(""),
        Line::from(preview),
        Line::from(""),
        Line::from("y confirm   n cancel"),
    ]);
    frame.render_widget(
        Paragraph::new(text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(COLOR_ERROR))
                    .title(Span::styled("Confirm", Style::default().fg(COLOR_ERROR))),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_thinking_picker(frame: &mut ratatui::Frame<'_>, picker: &ThinkingPicker) {
    let area = centered_rect(58, 14, frame.area());
    frame.render_widget(Clear, area);
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(area);
    let capacity = usize::from(layout[0].height.saturating_sub(2)).max(1);
    let start = picker
        .selected
        .saturating_sub(capacity / 2)
        .min(picker.levels.len().saturating_sub(capacity));
    let items = picker
        .levels
        .iter()
        .enumerate()
        .skip(start)
        .take(capacity)
        .map(|(index, level)| {
            let marker = if index == picker.selected { ">" } else { " " };
            let style = if index == picker.selected {
                Style::default()
                    .fg(COLOR_BRAND)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(COLOR_TEXT)
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{marker} "), style),
                Span::styled(format!("{level:<8}"), style),
                Span::styled(
                    reasoning_description(*level),
                    Style::default().fg(COLOR_MUTED),
                ),
            ]))
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(COLOR_MUTED))
                .title(Span::styled(
                    "Reasoning effort",
                    Style::default().fg(COLOR_BRAND),
                )),
        ),
        layout[0],
    );
    frame.render_widget(
        Paragraph::new("Enter apply   Up/Down move   Esc cancel")
            .style(Style::default().fg(COLOR_MUTED)),
        layout[1],
    );
}

fn reasoning_description(level: ReasoningLevel) -> &'static str {
    match level {
        ReasoningLevel::Auto => "provider default",
        ReasoningLevel::Off => "disable reasoning when supported",
        ReasoningLevel::Minimal => "quickest reasoning",
        ReasoningLevel::Low => "light reasoning",
        ReasoningLevel::Medium => "balanced reasoning",
        ReasoningLevel::High => "deep reasoning",
        ReasoningLevel::XHigh => "extra-deep reasoning",
        ReasoningLevel::Max => "maximum reasoning",
    }
}

fn draw_help(frame: &mut ratatui::Frame<'_>) {
    let area = centered_rect(70, 20, frame.area());
    frame.render_widget(Clear, area);
    let text = Text::from(vec![
        Line::from("Commands"),
        Line::from(""),
        Line::from("/provider [ID]   open provider manager or select provider"),
        Line::from("/model [MODEL] [LEVEL]  open the picker or switch the route"),
        Line::from("/effort LEVEL    set auto, off, minimal, low, medium, high, xhigh, or max"),
        Line::from("/thinking LEVEL  alias for /effort"),
        Line::from("/mode MODE       set read, auto, reviewed, or free"),
        Line::from("/confirm         run pending steers"),
        Line::from("/discard         discard pending steers"),
        Line::from("/capabilities T  set provider tools, for example web_search"),
        Line::from("/new             create a new session"),
        Line::from("/resume ID       resume a session"),
        Line::from("/sessions        open the session picker"),
        Line::from("/diagnostics     show core connection details"),
        Line::from("/help            show this help"),
        Line::from("/quit            exit"),
        Line::from(""),
        Line::from("Ctrl-L           open model picker"),
        Line::from("Ctrl-T           open effort picker"),
        Line::from("Ctrl-R           open session manager"),
        Line::from("Ctrl-Enter       insert newline"),
        Line::from("Shift-Tab        cycle reasoning effort"),
        Line::from("Wheel/PgUp/PgDn  scroll transcript"),
        Line::from(""),
        Line::from("Esc deny/cancel   Ctrl-P providers   Ctrl-C quit"),
    ]);
    frame.render_widget(
        Paragraph::new(text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(COLOR_MUTED))
                    .title(Span::styled("Help", Style::default().fg(COLOR_BRAND))),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_command_completions(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    completions: &[CommandCompletion],
    selected: usize,
) {
    if area.is_empty() {
        return;
    }
    let items = completions
        .iter()
        .map(|completion| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{:<28}", completion.label),
                    Style::default().fg(COLOR_TEXT),
                ),
                Span::styled(
                    completion.description.clone(),
                    Style::default().fg(COLOR_MUTED),
                ),
            ]))
        })
        .collect::<Vec<_>>();
    let capacity = usize::from(area.height.saturating_sub(2)).max(1);
    let start = selected
        .saturating_sub(capacity / 2)
        .min(completions.len().saturating_sub(capacity));
    let visible = items
        .into_iter()
        .skip(start)
        .take(capacity)
        .collect::<Vec<_>>();
    let mut state = RatatuiListState::default().with_selected(Some(selected.saturating_sub(start)));
    frame.render_stateful_widget(
        List::new(visible)
            .highlight_style(
                Style::default()
                    .fg(COLOR_BRAND)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("> ")
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(COLOR_MUTED))
                    .title(Span::styled("Commands", Style::default().fg(COLOR_BRAND))),
            ),
        area,
        &mut state,
    );
}

fn draw_interaction_prompt(frame: &mut ratatui::Frame<'_>, prompt: &InteractionPrompt) {
    let area = centered_rect(70, 12, frame.area());
    frame.render_widget(Clear, area);
    let text = Text::from(vec![
        Line::from(prompt.title.clone()),
        Line::from(""),
        Line::from(prompt.body.clone()),
        Line::from(""),
        Line::from("y allow once   s allow session   n deny"),
    ]);
    frame.render_widget(
        Paragraph::new(text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(prompt.kind.clone()),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_provider_screen(frame: &mut ratatui::Frame<'_>, view: &ProviderView) {
    let area = frame.area();
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(5),
            Constraint::Length(2),
        ])
        .split(area);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "alpha",
                Style::default()
                    .fg(COLOR_BRAND)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  providers"),
        ])),
        layout[0],
    );

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
        .split(layout[1]);

    let kinds: Vec<ListItem<'_>> = if view.kinds.is_empty() {
        vec![ListItem::new("No provider kinds registered")]
    } else {
        view.kinds
            .iter()
            .enumerate()
            .map(|(index, kind)| {
                let marker = if index == view.selected_kind {
                    ">"
                } else {
                    " "
                };
                ListItem::new(format!("{marker} {}  ({})", kind.name, kind.kind))
            })
            .collect()
    };
    frame.render_widget(
        List::new(kinds).block(Block::default().borders(Borders::ALL).title("Kinds")),
        body[0],
    );

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(6)])
        .split(body[1]);
    frame.render_widget(
        Paragraph::new(kind_details(view.selected_kind()))
            .block(Block::default().borders(Borders::ALL).title("Details"))
            .wrap(Wrap { trim: false }),
        right[0],
    );

    let instances = view.instances_for_selected_kind();
    let items: Vec<ListItem<'_>> = if instances.is_empty() {
        vec![ListItem::new("No instances")]
    } else {
        instances
            .iter()
            .enumerate()
            .map(|(index, instance)| {
                let marker = if index == view.selected_instance {
                    ">"
                } else {
                    " "
                };
                let disabled = if instance.disabled { " [disabled]" } else { "" };
                ListItem::new(format!(
                    "{marker} {}  ({}){disabled}",
                    instance.id, instance.kind
                ))
            })
            .collect()
    };
    frame.render_widget(
        List::new(items).block(Block::default().borders(Borders::ALL).title("Instances")),
        right[1],
    );

    let status = view
        .status
        .as_deref()
        .unwrap_or(if view.instances.is_empty() {
            "No providers configured. Press n to create one, or q to return."
        } else {
            "n new  e config  c credential  t test  s default  x remove  r refresh  q back"
        });
    let status_style = if view.status.is_some() || view.credential_hint().is_some() {
        Style::default().fg(COLOR_ERROR)
    } else {
        Style::default().fg(COLOR_MUTED)
    };
    frame.render_widget(Paragraph::new(status).style(status_style), layout[2]);

    if let Some(hint) = view.credential_hint() {
        let hint_area = Rect {
            x: layout[2].x,
            y: layout[2].y.saturating_add(1),
            width: layout[2].width,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(hint).style(Style::default().fg(COLOR_ERROR)),
            hint_area,
        );
    }

    if let Some(prompt) = &view.auth_prompt {
        draw_auth_prompt(frame, prompt);
    }
    if let Some(editor) = &view.config_editor {
        draw_config_editor(frame, editor);
    }
    if let Some(id) = &view.confirm_remove {
        draw_remove_confirmation(frame, id);
    }
}

fn draw_auth_prompt(frame: &mut ratatui::Frame<'_>, prompt: &AuthPrompt) {
    let area = centered_rect(60, 5, frame.area());
    frame.render_widget(Clear, area);
    let masked = "*".repeat(prompt.input.chars().count());
    let text = Text::from(vec![
        Line::from(format!("Provider: {}", prompt.instance_id)),
        Line::from(masked),
        Line::from("Enter store   Esc cancel"),
    ]);
    frame.render_widget(
        Paragraph::new(text)
            .block(Block::default().borders(Borders::ALL).title("API key"))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_config_editor(frame: &mut ratatui::Frame<'_>, editor: &ConfigEditor) {
    let area = centered_rect(80, 22, frame.area());
    frame.render_widget(Clear, area);
    let mut lines = vec![
        Line::from(format!("Provider: {}", editor.instance.id)),
        Line::from(""),
    ];
    for line in editor.text.lines() {
        lines.push(Line::from(line.to_owned()));
    }
    if let Some(error) = &editor.error {
        lines.push(Line::from(""));
        lines.push(Line::from(format!("error: {error}")));
    }
    lines.push(Line::from(""));
    lines.push(Line::from("Ctrl-S save   Esc cancel"));
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(Block::default().borders(Borders::ALL).title("Config JSON"))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_remove_confirmation(frame: &mut ratatui::Frame<'_>, instance_id: &str) {
    let area = centered_rect(60, 7, frame.area());
    frame.render_widget(Clear, area);
    let text = Text::from(vec![
        Line::from(format!("Remove provider instance `{instance_id}`?")),
        Line::from(""),
        Line::from("y confirm   n cancel"),
    ]);
    frame.render_widget(
        Paragraph::new(text)
            .block(Block::default().borders(Borders::ALL).title("Confirm"))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn centered_rect(percent_x: u16, height: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(height),
            Constraint::Fill(1),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Fill(1),
            Constraint::Percentage(percent_x),
            Constraint::Fill(1),
        ])
        .split(vertical[1])[1]
}

fn kind_details(kind: Option<&ProviderDescriptor>) -> Text<'static> {
    let Some(kind) = kind else {
        return Text::from("No provider selected");
    };
    let mut lines = vec![
        Line::from(kind.name.clone()),
        Line::from(format!("kind: {}", kind.kind)),
        Line::from(""),
        Line::from("Adapters:"),
    ];
    for adapter in &kind.adapters {
        let suffix = if adapter.required { " (required)" } else { "" };
        lines.push(Line::from(format!("  {}{}", adapter.key, suffix)));
    }
    lines.push(Line::from(""));
    lines.push(Line::from("Models:"));
    for model in &kind.models {
        lines.push(Line::from(format!(
            "  {}  [{}]  {}k ctx",
            model.id,
            model.adapter,
            model.context_window / 1000
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from("Auth:"));
    for auth in &kind.auth_methods {
        lines.push(Line::from(format!("  {}  {}", auth.key, auth.label)));
    }
    lines.push(Line::from(""));
    lines.push(Line::from("Config fields:"));
    if let Some(properties) = kind
        .config_schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
    {
        for key in properties.keys() {
            lines.push(Line::from(format!("  {key}")));
        }
    }
    Text::from(lines)
}

struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    keyboard_enhancement: bool,
}

impl TerminalGuard {
    fn enter() -> Result<Self, TuiError> {
        enable_raw_mode()?;
        let mut output = stdout();
        let keyboard_enhancement = matches!(supports_keyboard_enhancement(), Ok(true));
        if keyboard_enhancement {
            execute!(
                output,
                PushKeyboardEnhancementFlags(
                    KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                        | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
                        | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
                        | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                )
            )?;
        }
        execute!(output, EnterAlternateScreen, EnableMouseCapture)?;
        let terminal = Terminal::new(CrosstermBackend::new(output))?;
        Ok(Self {
            terminal,
            keyboard_enhancement,
        })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        if self.keyboard_enhancement {
            let _ = execute!(self.terminal.backend_mut(), PopKeyboardEnhancementFlags);
        }
        let _ = execute!(
            self.terminal.backend_mut(),
            DisableMouseCapture,
            LeaveAlternateScreen
        );
        let _ = self.terminal.show_cursor();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alpha_provider_api::{AdapterRequirement, AuthMethodDescriptor, ModelDescriptor};

    fn composer(text: &str) -> Composer {
        let mut composer = Composer::default();
        for character in text.chars() {
            composer.insert_char(character);
        }
        composer
    }

    fn app_with_composer(text: &str) -> App {
        App {
            chat: ChatView {
                composer: composer(text),
                ..ChatView::default()
            },
            providers: ProviderView::default(),
            screen: Screen::Chat,
            session_picker: None,
            model_picker: None,
            thinking_picker: None,
            help: false,
            quit: false,
        }
    }

    fn descriptor() -> ProviderDescriptor {
        ProviderDescriptor {
            kind: "test".to_owned(),
            name: "Test".to_owned(),
            config_schema: serde_json::json!({}),
            default_config: serde_json::json!({}),
            auth_methods: vec![AuthMethodDescriptor {
                key: "api_key".to_owned(),
                label: "API key".to_owned(),
                interactive: true,
            }],
            models: vec![ModelDescriptor {
                id: "model".to_owned(),
                name: "Model".to_owned(),
                adapter: "test".to_owned(),
                context_window: 1_000,
                max_tokens: 100,
                reasoning: false,
                reasoning_levels: vec![ReasoningLevel::Auto],
                provider_tools: Vec::new(),
            }],
            adapters: vec![AdapterRequirement {
                key: "test".to_owned(),
                required: true,
            }],
        }
    }

    #[test]
    fn selection_stays_in_bounds() {
        let mut view = ProviderView {
            kinds: vec![descriptor(), descriptor()],
            ..ProviderView::default()
        };
        view.move_kind_selection(-1);
        assert_eq!(view.selected_kind, 0);
        view.move_kind_selection(1);
        assert_eq!(view.selected_kind, 1);
        view.move_kind_selection(1);
        assert_eq!(view.selected_kind, 1);
    }

    #[test]
    fn arbitrary_model_ids_resolve_to_the_current_provider() {
        let view = ProviderView {
            kinds: vec![descriptor()],
            instances: vec![ProviderInstance {
                id: "test".to_owned(),
                kind: "test".to_owned(),
                config_version: 1,
                config: serde_json::json!({}),
                auth_ref: None,
                model_overrides: serde_json::json!({}),
                disabled: false,
            }],
            ..ProviderView::default()
        };
        let choices = model_choices(&view);
        let selected =
            resolve_model_selection(&choices, "test", "deepseek-v4.1-flash").expect("selection");
        assert_eq!(selected.provider, "test");
        assert_eq!(selected.model, "deepseek-v4.1-flash");
        assert_eq!(selected.reasoning_levels, ReasoningLevel::ALL.to_vec());
    }

    #[test]
    fn details_include_adapter_and_model() {
        let details = kind_details(Some(&descriptor()));
        let rendered = details
            .lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(rendered.contains("test (required)"));
        assert!(rendered.contains("model"));
    }

    #[test]
    fn parses_unavailable_credential_status() {
        let status = parse_credential_status(&serde_json::json!({
            "backend": "secret-service",
            "status": "unavailable",
            "detail": "dbus unavailable"
        }));
        assert_eq!(status.backend, "secret-service");
        assert!(!status.available);
        assert_eq!(status.detail.as_deref(), Some("dbus unavailable"));
    }

    #[test]
    fn chat_streams_and_replaces_assistant_text() {
        let mut view = ChatView {
            session_id: Some("session-a".to_owned()),
            ..ChatView::default()
        };
        view.handle_session_event(&serde_json::json!({
            "sessionId": "session-a",
            "kind": "live",
            "event": { "type": "text_delta", "text": "hel" }
        }));
        view.handle_session_event(&serde_json::json!({
            "sessionId": "session-a",
            "kind": "live",
            "event": { "type": "text_delta", "text": "lo" }
        }));
        assert_eq!(
            view.transcript,
            vec![TranscriptEntry::Assistant("hello".to_owned())]
        );
        view.handle_session_event(&serde_json::json!({
            "sessionId": "session-a",
            "kind": "durable",
            "event": {
                "payload": {
                    "type": "assistant_message",
                    "text": "hello"
                }
            }
        }));
        assert_eq!(
            view.transcript,
            vec![TranscriptEntry::Assistant("hello".to_owned())]
        );
    }

    #[test]
    fn transcript_cache_reuses_unchanged_entries() {
        let mut view = ChatView {
            transcript: vec![
                TranscriptEntry::Assistant("stable".to_owned()),
                TranscriptEntry::Assistant("live".to_owned()),
            ],
            ..ChatView::default()
        };
        ensure_transcript_cache(&mut view, 40);
        let stable_before = view.transcript_cache.as_ref().expect("cache").lines[0].spans[1]
            .content
            .as_ptr();

        if let Some(TranscriptEntry::Assistant(text)) = view.transcript.get_mut(1) {
            text.push_str(" more");
        }
        view.mark_transcript_entry(1);
        ensure_transcript_cache(&mut view, 40);

        let cache = view.transcript_cache.as_ref().expect("cache");
        assert_eq!(cache.lines[0].spans[1].content.as_ptr(), stable_before);
        let rendered = cache
            .lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(rendered.contains("live more"));
    }

    #[test]
    fn chat_streams_thinking_separately_from_visible_text() {
        let mut view = ChatView {
            session_id: Some("session-a".to_owned()),
            ..ChatView::default()
        };
        view.handle_session_event(&serde_json::json!({
            "sessionId": "session-a",
            "kind": "live",
            "event": { "type": "thinking_delta", "text": "plan" }
        }));
        view.handle_session_event(&serde_json::json!({
            "sessionId": "session-a",
            "kind": "live",
            "event": { "type": "text_delta", "text": "answer" }
        }));
        assert_eq!(
            view.transcript,
            vec![
                TranscriptEntry::Thinking("plan".to_owned()),
                TranscriptEntry::Assistant("answer".to_owned()),
            ]
        );

        view.handle_session_event(&serde_json::json!({
            "sessionId": "session-a",
            "kind": "durable",
            "event": {
                "payload": {
                    "type": "assistant_message",
                    "blocks": [
                        { "type": "thinking", "text": "plan", "signature": "sig" },
                        { "type": "text", "text": "answer" }
                    ]
                }
            }
        }));
        assert_eq!(
            view.transcript,
            vec![
                TranscriptEntry::Thinking("plan".to_owned()),
                TranscriptEntry::Assistant("answer".to_owned()),
            ]
        );
        assert!(view.streaming_thinking.is_none());
        assert!(view.streaming_assistant.is_none());
    }

    #[test]
    fn chat_ignores_events_from_other_sessions() {
        let mut view = ChatView {
            session_id: Some("session-a".to_owned()),
            ..ChatView::default()
        };
        view.handle_session_event(&serde_json::json!({
            "sessionId": "session-b",
            "kind": "live",
            "event": { "type": "text_delta", "text": "wrong" }
        }));
        assert!(view.transcript.is_empty());
    }

    #[test]
    fn route_event_updates_the_active_route() {
        let mut view = ChatView {
            session_id: Some("session-a".to_owned()),
            ..ChatView::default()
        };
        view.handle_session_event(&serde_json::json!({
            "sessionId": "session-a",
            "kind": "durable",
            "event": {
                "payload": {
                    "type": "route_changed",
                    "provider": "provider-b",
                    "model": "model-b",
                    "reasoning": "high"
                }
            }
        }));
        assert_eq!(view.provider, "provider-b");
        assert_eq!(view.model, "model-b");
        assert_eq!(view.reasoning, ReasoningLevel::High);
    }

    #[test]
    fn chat_header_draws_route_and_shortcuts() {
        let mut view = ChatView {
            provider: "provider-a".to_owned(),
            model: "model-a".to_owned(),
            reasoning: ReasoningLevel::High,
            mode: PermissionMode::Auto,
            ..ChatView::default()
        };
        let backend = ratatui::backend::TestBackend::new(100, 20);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| draw_chat_screen(frame, &mut view, &[], true))
            .expect("draw chat");
        assert!(terminal_contains(&terminal, "provider-a/model-a"));
        assert!(terminal_contains(&terminal, "effort:high"));
        assert!(terminal_contains(&terminal, "Ctrl-R sessions"));
    }

    #[test]
    fn reasoning_transcript_uses_the_effort_label() {
        let lines = thinking_transcript_lines("plan\ndetail");
        let rendered = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        assert_eq!(rendered, vec!["effort: plan", "        detail"]);
    }

    #[test]
    fn chat_renders_tool_call_and_result_summaries() {
        let mut view = ChatView {
            session_id: Some("session-a".to_owned()),
            ..ChatView::default()
        };
        view.handle_session_event(&serde_json::json!({
            "sessionId": "session-a",
            "kind": "durable",
            "event": {
                "payload": {
                    "type": "tool_call",
                    "id": "call-1",
                    "name": "shell",
                    "input": { "command": "ls -la" }
                }
            }
        }));
        view.handle_session_event(&serde_json::json!({
            "sessionId": "session-a",
            "kind": "durable",
            "event": {
                "payload": {
                    "type": "tool_result",
                    "id": "call-1",
                    "status": "completed",
                    "content": "src/lib.rs\nCargo.toml"
                }
            }
        }));
        assert_eq!(
            view.transcript,
            vec![
                TranscriptEntry::Progress("shell: ls -la".to_owned()),
                TranscriptEntry::Progress("shell: completed - src/lib.rs (2 lines)".to_owned()),
            ]
        );
    }

    #[test]
    fn markdown_output_preserves_blocks_and_inline_styles() {
        let lines = assistant_transcript_lines(
            "# Title\n\nHello **world** and `code`.\n\n- one\n- two\n\n```rust\nfn main() {}\n```",
        );
        let rendered = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        assert!(rendered.iter().any(|line| line == "alpha: Title"));
        assert!(rendered.iter().any(|line| line == "Hello world and code."));
        assert!(rendered.iter().any(|line| line == "- one"));
        assert!(rendered.iter().any(|line| line == "- two"));
        assert!(rendered.iter().any(|line| line.contains("fn main() {}")));

        let bold = lines
            .iter()
            .flat_map(|line| &line.spans)
            .find(|span| span.content == "world")
            .expect("bold span");
        assert!(bold.style.add_modifier.contains(Modifier::BOLD));
        let code = lines
            .iter()
            .flat_map(|line| &line.spans)
            .find(|span| span.content == "code")
            .expect("code span");
        assert_eq!(code.style.fg, Some(COLOR_SUCCESS));
    }

    #[test]
    fn markdown_output_renders_tables_with_alignment() {
        let lines =
            render_markdown("| Name | Count |\n|:-----|------:|\n| alpha | 1 |\n| beta | 22 |");
        let rendered = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            rendered,
            vec![
                "| Name  | Count |",
                "|-------|-------|",
                "| alpha |     1 |",
                "| beta  |    22 |",
            ]
        );
        let header = lines[0]
            .spans
            .iter()
            .find(|span| span.content == "Name")
            .expect("table header cell");
        assert!(header.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn assistant_payload_preserves_block_order() {
        let payload = serde_json::json!({
            "blocks": [
                { "type": "thinking", "text": "reasoning" },
                { "type": "text", "text": "first" },
                { "type": "text", "text": "second" }
            ]
        });
        assert_eq!(
            assistant_payload_entries(&payload),
            vec![
                TranscriptEntry::Thinking("reasoning".to_owned()),
                TranscriptEntry::Assistant("first".to_owned()),
                TranscriptEntry::Assistant("second".to_owned()),
            ]
        );
    }

    #[test]
    fn transcript_scroll_reaches_the_last_entry() {
        let mut view = ChatView {
            transcript: (0..80)
                .map(|index| TranscriptEntry::Assistant(format!("line {index}")))
                .collect(),
            ..ChatView::default()
        };
        let backend = ratatui::backend::TestBackend::new(40, 12);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| draw_chat_screen(frame, &mut view, &[], true))
            .expect("draw transcript");
        assert!(terminal_contains(&terminal, "line 79"));

        view.scroll_transcript_up(u16::MAX);
        terminal
            .draw(|frame| draw_chat_screen(frame, &mut view, &[], true))
            .expect("draw transcript top");
        assert!(terminal_contains(&terminal, "line 0"));

        view.scroll_transcript_down(u16::MAX);
        terminal
            .draw(|frame| draw_chat_screen(frame, &mut view, &[], true))
            .expect("draw transcript bottom");
        assert!(terminal_contains(&terminal, "line 79"));
    }

    fn terminal_contains(terminal: &Terminal<ratatui::backend::TestBackend>, needle: &str) -> bool {
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
            .contains(needle)
    }

    #[test]
    fn transcript_scroll_returns_to_the_bottom() {
        let mut view = ChatView::default();
        view.scroll_transcript_up(10);
        assert_eq!(view.transcript_scroll_back, 10);
        view.scroll_transcript_down(3);
        assert_eq!(view.transcript_scroll_back, 7);
        view.scroll_transcript_down(100);
        assert_eq!(view.transcript_scroll_back, 0);
    }

    #[test]
    fn config_editor_starts_from_the_selected_instance() {
        let mut view = ProviderView {
            kinds: vec![descriptor()],
            instances: vec![ProviderInstance {
                id: "test".to_owned(),
                kind: "test".to_owned(),
                config_version: 1,
                config: serde_json::json!({ "base_url": "https://example.test" }),
                auth_ref: None,
                model_overrides: serde_json::json!({}),
                disabled: false,
            }],
            ..ProviderView::default()
        };
        view.begin_config_edit().expect("begin edit");
        let editor = view.config_editor.expect("editor");
        assert!(editor.text.contains("https://example.test"));
    }

    #[test]
    fn session_picker_renders_a_first_message_preview() {
        let picker = SessionPicker {
            sessions: vec![SessionEntry {
                id: "01900000-0000-7000-8000-000000000000".to_owned(),
                mode: "auto".to_owned(),
                workspace: "/tmp/workspace".to_owned(),
                provider: "openai".to_owned(),
                model: "gpt".to_owned(),
                first_message: Some("fix the failing parser test".to_owned()),
            }],
            selected: 0,
            confirm_delete: None,
            status: None,
        };
        let backend = ratatui::backend::TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| draw_session_picker(frame, &picker))
            .expect("draw session picker");
        assert!(terminal_contains(&terminal, "fix the failing parser test"));
        assert!(terminal_contains(&terminal, "Enter switch"));
    }

    #[test]
    fn composer_edits_at_the_cursor_and_preserves_unicode_boundaries() {
        let mut composer = composer("a界b");
        composer.move_left();
        assert_eq!(composer.cursor(), "a界".len());
        composer.backspace();
        assert_eq!(composer.text(), "ab");
        composer.insert_char('x');
        assert_eq!(composer.text(), "axb");
        composer.move_home();
        composer.delete();
        assert_eq!(composer.text(), "xb");
    }

    #[test]
    fn composer_moves_between_lines_and_inserts_at_the_target() {
        let mut composer = composer("one\ntwo");
        composer.move_up();
        composer.insert_char('!');
        assert_eq!(composer.text(), "one!\ntwo");
        composer.move_down();
        composer.move_end();
        composer.insert_char('?');
        assert_eq!(composer.text(), "one!\ntwo?");
        composer.insert_newline();
        assert_eq!(composer.text(), "one!\ntwo?\n");
    }

    #[test]
    fn slash_command_completion_filters_and_accepts() {
        let mut app = app_with_composer("/mo");
        let completions = command_completions(&app);
        assert_eq!(completions.len(), 2);
        assert!(
            completions
                .iter()
                .any(|completion| completion.label == "/model [MODEL] [EFFORT]")
        );
        assert!(completion_accepts_enter(&app));

        accept_completion(&mut app);
        assert_eq!(app.chat.composer.text(), "/model ");
        assert!(!completion_accepts_enter(&app));
    }

    #[test]
    fn command_completions_include_argument_values() {
        let app = app_with_composer("/mode ");
        let completions = command_completions(&app);
        assert_eq!(
            completions
                .iter()
                .map(|completion| completion.insert.as_str())
                .collect::<Vec<_>>(),
            vec!["read", "auto", "reviewed", "free"]
        );
    }

    #[test]
    fn command_parser_accepts_the_composer_slash_prefix() {
        assert_eq!(split_command("/mode"), ("mode", ""));
        assert_eq!(split_command("/mode reviewed"), ("mode", "reviewed"));
        assert_eq!(
            split_command("/resume session-id"),
            ("resume", "session-id")
        );
    }

    #[test]
    fn multiline_composer_renders_all_lines() {
        let mut app = app_with_composer("first line\nsecond line");
        let backend = ratatui::backend::TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| {
                draw_chat_screen(frame, &mut app.chat, &[], true);
            })
            .expect("draw composer");
        assert!(terminal_contains(&terminal, "first line"));
        assert!(terminal_contains(&terminal, "second line"));
        assert!(terminal_contains(&terminal, "Composer"));
        assert!(!terminal_contains(&terminal, "Ctrl-Enter newline"));
    }
}

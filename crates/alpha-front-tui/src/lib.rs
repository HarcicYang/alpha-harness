//! Full-screen terminal frontend for alpha.

#![forbid(unsafe_code)]

use std::io::{self, stdout};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use alpha_front_client::ConnectionEvent;
use alpha_front_supervisor::{CoreSupervisor, SupervisorConfig, SupervisorError};
use alpha_protocol::{Id, Request};
use alpha_provider_api::{ProviderDescriptor, ProviderInstance};
use alpha_schema::PermissionMode;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use serde_json::{Value, json};

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
    /// Selected provider kind index.
    pub selected_kind: usize,
    /// Selected instance index within the current kind.
    pub selected_instance: usize,
    /// Active secret input.
    pub auth_prompt: Option<AuthPrompt>,
    /// Credential backend status.
    pub credential_status: Option<CredentialStatusView>,
    /// Status or error line.
    pub status: Option<String>,
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

/// One visible line in the chat transcript.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TranscriptEntry {
    /// User message.
    User(String),
    /// Assistant message.
    Assistant(String),
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
    /// Composer input.
    pub composer: String,
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
}

impl ChatView {
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
        self.status = Some("session ready".to_owned());
        Ok(())
    }

    fn new_session(&mut self, supervisor: &mut CoreSupervisor) -> Result<(), TuiError> {
        self.session_id = None;
        self.transcript.clear();
        self.streaming_assistant = None;
        self.running_turn = None;
        self.interaction = None;
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
        self.transcript.clear();
        self.streaming_assistant = None;
        self.running_turn = None;
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
        self.status = Some(format!("mode: {mode}"));
        Ok(())
    }

    fn submit(&mut self, supervisor: &mut CoreSupervisor) -> Result<(), TuiError> {
        let prompt = self.composer.trim().to_owned();
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
        Ok(())
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
                    self.transcript.push(TranscriptEntry::User(text.to_owned()));
                    self.streaming_assistant = None;
                }
            }
            "assistant_message" => {
                let text = payload
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                if let Some(index) = self.streaming_assistant.take() {
                    if let Some(entry) = self.transcript.get_mut(index) {
                        *entry = TranscriptEntry::Assistant(text);
                        return;
                    }
                }
                self.transcript.push(TranscriptEntry::Assistant(text));
            }
            "tool_call" => {
                let name = payload
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("tool");
                self.transcript
                    .push(TranscriptEntry::Progress(format!("running {name}")));
            }
            "tool_result" => {
                let status = payload
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("finished");
                self.transcript
                    .push(TranscriptEntry::Progress(format!("tool {status}")));
            }
            "turn_ended" => {
                self.running_turn = None;
                self.status = Some("turn finished".to_owned());
            }
            "mode_changed" => {
                if let Some(mode) = payload.get("mode").and_then(Value::as_str) {
                    self.status = Some(format!("mode: {mode}"));
                }
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
                    }
                } else {
                    self.transcript
                        .push(TranscriptEntry::Assistant(delta.to_owned()));
                    self.streaming_assistant = Some(self.transcript.len() - 1);
                }
            }
            "step_started" => {
                if let Some(step) = event.get("step").and_then(Value::as_u64) {
                    self.status = Some(format!("model step {step}"));
                }
            }
            "tool_started" => {
                let name = event.get("name").and_then(Value::as_str).unwrap_or("tool");
                self.status = Some(format!("running {name}"));
            }
            "tool_finished" => {
                let name = event.get("name").and_then(Value::as_str).unwrap_or("tool");
                self.status = Some(format!("finished {name}"));
            }
            _ => {}
        }
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

    fn expire_interaction(&mut self, supervisor: &CoreSupervisor) {
        let expired = self
            .interaction
            .as_ref()
            .is_some_and(|prompt| Instant::now() >= prompt.deadline);
        if expired {
            let _ = self.respond_interaction(supervisor, "timeout");
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Screen {
    Chat,
    Providers,
}

struct App {
    chat: ChatView,
    providers: ProviderView,
    screen: Screen,
    palette: Option<String>,
    session_picker: Option<SessionPicker>,
    help: bool,
    quit: bool,
}

struct SessionPicker {
    sessions: Vec<Value>,
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

    fn remove_instance(&mut self, supervisor: &mut CoreSupervisor) -> Result<(), TuiError> {
        let Some(instance) = self.selected_instance() else {
            return Err(TuiError::Protocol(
                "no provider instance selected".to_owned(),
            ));
        };
        let id = instance.id.clone();
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
        palette: None,
        session_picker: None,
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
    drop(guard);
    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    supervisor: &mut CoreSupervisor,
) -> Result<(), TuiError> {
    loop {
        terminal.draw(|frame| draw_app(frame, app))?;
        app.chat.expire_interaction(supervisor);
        if let Some(connection_event) = supervisor.recv_event_timeout(Duration::from_millis(10))? {
            app.chat.handle_connection_event(connection_event);
        }
        if event::poll(Duration::from_millis(0))? {
            match event::read()? {
                Event::Key(key) if key.kind == crossterm::event::KeyEventKind::Press => {
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
                    if app.palette.is_some() {
                        handle_palette_key(app, supervisor, key.code, key.modifiers)?;
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
                Event::Resize(_, _) => {}
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
    match code {
        KeyCode::Esc | KeyCode::Char('q') => {
            app.screen = Screen::Chat;
            if let Err(error) = app.chat.ensure_session(supervisor) {
                app.chat.status = Some(error.to_string());
            }
        }
        KeyCode::Char('r') => {
            app.providers.status = None;
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
            if let Err(error) = app.providers.remove_instance(supervisor) {
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
    match code {
        KeyCode::Char('p') if modifiers.contains(KeyModifiers::CONTROL) => {
            app.screen = Screen::Providers;
            app.providers.status = None;
            if let Err(error) = app.providers.refresh(supervisor) {
                app.providers.status = Some(error.to_string());
            }
        }
        KeyCode::Enter => {
            if let Err(error) = app.chat.submit(supervisor) {
                app.chat.status = Some(error.to_string());
            }
        }
        KeyCode::Esc => {
            if let Err(error) = app.chat.cancel(supervisor) {
                app.chat.status = Some(error.to_string());
            }
        }
        KeyCode::Backspace => {
            app.chat.composer.pop();
        }
        KeyCode::Char(':') => {
            app.palette = Some(String::new());
        }
        KeyCode::Char(character) if !modifiers.contains(KeyModifiers::CONTROL) => {
            app.chat.composer.push(character);
        }
        _ => {}
    }
    Ok(())
}

fn handle_palette_key(
    app: &mut App,
    supervisor: &mut CoreSupervisor,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> Result<(), TuiError> {
    match code {
        KeyCode::Esc => app.palette = None,
        KeyCode::Enter => {
            let command = app.palette.take().unwrap_or_default();
            execute_palette_command(app, supervisor, &command)?;
        }
        KeyCode::Backspace => {
            if let Some(palette) = &mut app.palette {
                palette.pop();
            }
        }
        KeyCode::Char(character) if !modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(palette) = &mut app.palette {
                palette.push(character);
            }
        }
        _ => {}
    }
    Ok(())
}

fn execute_palette_command(
    app: &mut App,
    supervisor: &mut CoreSupervisor,
    command: &str,
) -> Result<(), TuiError> {
    let command = command.trim();
    let (name, argument) = command
        .split_once(char::is_whitespace)
        .map_or((command, ""), |(name, argument)| (name, argument.trim()));
    match name {
        "provider" if argument.is_empty() => {
            app.screen = Screen::Providers;
            app.providers.refresh(supervisor)?;
        }
        "provider" => {
            app.chat.provider = argument.to_owned();
            app.chat.status = Some(format!("provider: {argument}"));
        }
        "model" if !argument.is_empty() => {
            app.chat.model = argument.to_owned();
            app.chat.status = Some(format!("model: {argument}"));
        }
        "mode" => {
            let mode = argument
                .parse::<PermissionMode>()
                .map_err(|error| TuiError::Protocol(error.to_string()))?;
            app.chat.set_mode(supervisor, mode)?;
        }
        "new" => {
            app.chat.new_session(supervisor)?;
            app.screen = Screen::Chat;
        }
        "resume" if !argument.is_empty() => {
            app.chat.resume_session(supervisor, argument)?;
            app.screen = Screen::Chat;
        }
        "sessions" => {
            let sessions = supervisor
                .request("session.list", None)
                .map_err(|error| TuiError::Protocol(error.to_string()))?;
            let sessions = sessions.as_array().cloned().unwrap_or_default();
            app.session_picker = Some(SessionPicker {
                sessions,
                selected: 0,
            });
        }
        "help" => app.help = true,
        "quit" => app.quit = true,
        "" => {}
        other => app.chat.status = Some(format!("unknown command: {other}")),
    }
    Ok(())
}

fn handle_session_picker_key(
    app: &mut App,
    supervisor: &mut CoreSupervisor,
    code: KeyCode,
) -> Result<(), TuiError> {
    let Some(picker) = &mut app.session_picker else {
        return Ok(());
    };
    match code {
        KeyCode::Esc => app.session_picker = None,
        KeyCode::Up => picker.selected = picker.selected.saturating_sub(1),
        KeyCode::Down => {
            picker.selected = picker
                .selected
                .saturating_add(1)
                .min(picker.sessions.len().saturating_sub(1));
        }
        KeyCode::Enter => {
            let selected = picker.sessions.get(picker.selected).cloned();
            if let Some(session) = selected {
                if let Some(id) = session.get("id").and_then(Value::as_str) {
                    app.chat.resume_session(supervisor, id)?;
                    app.screen = Screen::Chat;
                }
            }
            app.session_picker = None;
        }
        _ => {}
    }
    Ok(())
}

fn draw_app(frame: &mut ratatui::Frame<'_>, app: &App) {
    match app.screen {
        Screen::Chat => draw_chat_screen(frame, &app.chat),
        Screen::Providers => draw_provider_screen(frame, &app.providers),
    }
    if let Some(picker) = &app.session_picker {
        draw_session_picker(frame, picker);
    }
    if app.help {
        draw_help(frame);
    }
    if let Some(palette) = &app.palette {
        draw_palette(frame, palette);
    }
    if let Some(prompt) = &app.chat.interaction {
        draw_interaction_prompt(frame, prompt);
    }
}

fn draw_chat_screen(frame: &mut ratatui::Frame<'_>, view: &ChatView) {
    let area = frame.area();
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(5),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(area);
    let session = view.session_id.as_deref().unwrap_or("new session");
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "alpha",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("  {session}")),
        ])),
        layout[0],
    );

    let transcript = if view.transcript.is_empty() {
        Text::from("No messages yet")
    } else {
        Text::from(
            view.transcript
                .iter()
                .flat_map(|entry| match entry {
                    TranscriptEntry::User(text) => {
                        vec![Line::from(format!("you: {text}"))]
                    }
                    TranscriptEntry::Assistant(text) => {
                        vec![Line::from(format!("alpha: {text}"))]
                    }
                    TranscriptEntry::Progress(text) => {
                        vec![Line::from(format!("... {text}"))]
                    }
                    TranscriptEntry::Error(text) => {
                        vec![Line::from(format!("error: {text}"))]
                    }
                })
                .collect::<Vec<_>>(),
        )
    };
    frame.render_widget(
        Paragraph::new(transcript)
            .block(Block::default().borders(Borders::ALL).title("Transcript"))
            .wrap(Wrap { trim: false }),
        layout[1],
    );
    frame.render_widget(
        Paragraph::new(format!("> {}", view.composer))
            .block(Block::default().borders(Borders::ALL).title("Composer"))
            .wrap(Wrap { trim: false }),
        layout[2],
    );
    let status = view
        .status
        .as_deref()
        .unwrap_or("Enter submit  Esc cancel  Ctrl-P providers  Ctrl-C quit");
    frame.render_widget(
        Paragraph::new(status).style(Style::default().fg(Color::DarkGray)),
        layout[3],
    );
}

fn draw_session_picker(frame: &mut ratatui::Frame<'_>, picker: &SessionPicker) {
    let area = centered_rect(80, 18, frame.area());
    frame.render_widget(Clear, area);
    let items: Vec<ListItem<'_>> = if picker.sessions.is_empty() {
        vec![ListItem::new("No stored sessions")]
    } else {
        picker
            .sessions
            .iter()
            .enumerate()
            .map(|(index, session)| {
                let marker = if index == picker.selected { ">" } else { " " };
                let id = session.get("id").and_then(Value::as_str).unwrap_or("?");
                let workspace = session
                    .get("workspace")
                    .and_then(|workspace| workspace.get("root"))
                    .and_then(Value::as_str)
                    .unwrap_or("?");
                ListItem::new(format!("{marker} {id}  {workspace}"))
            })
            .collect()
    };
    frame.render_widget(
        List::new(items).block(Block::default().borders(Borders::ALL).title("Sessions")),
        area,
    );
}

fn draw_help(frame: &mut ratatui::Frame<'_>) {
    let area = centered_rect(70, 16, frame.area());
    frame.render_widget(Clear, area);
    let text = Text::from(vec![
        Line::from("Commands"),
        Line::from(""),
        Line::from(":provider [ID]   open provider manager or select provider"),
        Line::from(":model MODEL     select model for new sessions"),
        Line::from(":mode MODE       set read, auto, or free"),
        Line::from(":new             create a new session"),
        Line::from(":resume ID       resume a session"),
        Line::from(":sessions        open the session picker"),
        Line::from(":help            show this help"),
        Line::from(":quit            exit"),
        Line::from(""),
        Line::from("Esc deny/cancel   Ctrl-C quit"),
    ]);
    frame.render_widget(
        Paragraph::new(text)
            .block(Block::default().borders(Borders::ALL).title("Help"))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_palette(frame: &mut ratatui::Frame<'_>, palette: &str) {
    let area = centered_rect(70, 3, frame.area());
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(format!(":{palette}"))
            .block(Block::default().borders(Borders::ALL).title("Command")),
        area,
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
                    .fg(Color::Cyan)
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
                ListItem::new(format!("{marker} {}  ({})", instance.id, instance.kind))
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
        .unwrap_or("n new  x remove  c credential  t test  s default  r refresh  q quit");
    let status_style = if view.status.is_some() || view.credential_hint().is_some() {
        Style::default().fg(Color::Red)
    } else {
        Style::default().fg(Color::DarkGray)
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
            Paragraph::new(hint).style(Style::default().fg(Color::Red)),
            hint_area,
        );
    }

    if let Some(prompt) = &view.auth_prompt {
        draw_auth_prompt(frame, prompt);
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
    Text::from(lines)
}

struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
}

impl TerminalGuard {
    fn enter() -> Result<Self, TuiError> {
        enable_raw_mode()?;
        let mut output = stdout();
        execute!(output, EnterAlternateScreen)?;
        let terminal = Terminal::new(CrosstermBackend::new(output))?;
        Ok(Self { terminal })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alpha_provider_api::{AdapterRequirement, AuthMethodDescriptor, ModelDescriptor};

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
        let mut view = ChatView::default();
        view.handle_session_event(&serde_json::json!({
            "kind": "live",
            "event": { "type": "text_delta", "text": "hel" }
        }));
        view.handle_session_event(&serde_json::json!({
            "kind": "live",
            "event": { "type": "text_delta", "text": "lo" }
        }));
        assert_eq!(
            view.transcript,
            vec![TranscriptEntry::Assistant("hello".to_owned())]
        );
        view.handle_session_event(&serde_json::json!({
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
}

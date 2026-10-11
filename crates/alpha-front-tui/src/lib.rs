//! Full-screen terminal frontend for alpha.

#![forbid(unsafe_code)]

use std::io::{self, stdout};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use alpha_front_supervisor::{CoreSupervisor, SupervisorConfig, SupervisorError};
use alpha_provider_api::{ProviderDescriptor, ProviderInstance};
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
use serde_json::json;

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
    let mut view = ProviderView::default();
    view.refresh(&mut supervisor)?;

    let mut guard = TerminalGuard::enter()?;
    let result = run_loop(&mut guard.terminal, &mut view, &mut supervisor);
    drop(guard);
    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    view: &mut ProviderView,
    supervisor: &mut CoreSupervisor,
) -> Result<(), TuiError> {
    loop {
        terminal.draw(|frame| draw_provider_screen(frame, view))?;
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) if key.kind == crossterm::event::KeyEventKind::Press => {
                    if view.auth_prompt.is_some() {
                        match key.code {
                            KeyCode::Enter => {
                                if let Err(error) = view.submit_auth(supervisor) {
                                    view.status = Some(error.to_string());
                                }
                            }
                            KeyCode::Esc => view.auth_prompt = None,
                            KeyCode::Backspace => {
                                if let Some(prompt) = &mut view.auth_prompt {
                                    prompt.input.pop();
                                }
                            }
                            KeyCode::Char(character)
                                if !key.modifiers.contains(KeyModifiers::CONTROL) =>
                            {
                                if let Some(prompt) = &mut view.auth_prompt {
                                    prompt.input.push(character);
                                }
                            }
                            _ => {}
                        }
                        continue;
                    }

                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                        KeyCode::Char('r') => {
                            view.status = None;
                            if let Err(error) = view.refresh(supervisor) {
                                view.status = Some(error.to_string());
                            }
                        }
                        KeyCode::Up => view.move_kind_selection(-1),
                        KeyCode::Down => view.move_kind_selection(1),
                        KeyCode::Left => view.move_instance_selection(-1),
                        KeyCode::Right => view.move_instance_selection(1),
                        KeyCode::Char('n') => {
                            if let Err(error) = view.create_instance(supervisor) {
                                view.status = Some(error.to_string());
                            }
                        }
                        KeyCode::Char('x') => {
                            if let Err(error) = view.remove_instance(supervisor) {
                                view.status = Some(error.to_string());
                            }
                        }
                        KeyCode::Char('s') => {
                            if let Err(error) = view.set_default_instance(supervisor) {
                                view.status = Some(error.to_string());
                            }
                        }
                        KeyCode::Char('t') => {
                            if let Err(error) = view.test_instance(supervisor) {
                                view.status = Some(error.to_string());
                            }
                        }
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            return Ok(());
                        }
                        KeyCode::Char('c') => {
                            if let Err(error) = view.begin_auth() {
                                view.status = Some(error.to_string());
                            }
                        }
                        _ => {}
                    }
                }
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    }
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
}

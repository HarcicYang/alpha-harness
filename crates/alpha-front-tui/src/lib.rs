//! Full-screen terminal frontend for alpha.

#![forbid(unsafe_code)]

use std::io::{self, stdout};
use std::time::Duration;

use alpha_front_supervisor::{CoreSupervisor, SupervisorConfig, SupervisorError};
use alpha_provider_api::{ProviderDescriptor, ProviderInstance};
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};

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

/// Provider state rendered by the first TUI screen.
#[derive(Clone, Debug, Default)]
pub struct ProviderView {
    /// Registered provider kinds.
    pub kinds: Vec<ProviderDescriptor>,
    /// Persisted provider instances.
    pub instances: Vec<ProviderInstance>,
    /// Selected provider kind index.
    pub selected_kind: usize,
    /// Status or error line.
    pub status: Option<String>,
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
        self.kinds =
            serde_json::from_value(kinds).map_err(|error| TuiError::Protocol(error.to_string()))?;
        self.instances = serde_json::from_value(instances)
            .map_err(|error| TuiError::Protocol(error.to_string()))?;
        self.selected_kind = self.selected_kind.min(self.kinds.len().saturating_sub(1));
        Ok(())
    }

    fn selected(&self) -> Option<&ProviderDescriptor> {
        self.kinds.get(self.selected_kind)
    }

    fn move_selection(&mut self, delta: isize) {
        if self.kinds.is_empty() {
            return;
        }
        let current = self.selected_kind as isize + delta;
        self.selected_kind = current.clamp(0, self.kinds.len() as isize - 1) as usize;
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
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                        KeyCode::Char('r') => {
                            view.status = None;
                            if let Err(error) = view.refresh(supervisor) {
                                view.status = Some(error.to_string());
                            }
                        }
                        KeyCode::Up => view.move_selection(-1),
                        KeyCode::Down => view.move_selection(1),
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            return Ok(());
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
    frame.render_widget(
        Paragraph::new(kind_details(view.selected()))
            .block(Block::default().borders(Borders::ALL).title("Details"))
            .wrap(Wrap { trim: false }),
        body[1],
    );

    let status = view
        .status
        .as_deref()
        .unwrap_or("r refresh   ↑/↓ select   q quit");
    frame.render_widget(
        Paragraph::new(status).style(Style::default().fg(if view.status.is_some() {
            Color::Red
        } else {
            Color::DarkGray
        })),
        layout[2],
    );
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
        view.move_selection(-1);
        assert_eq!(view.selected_kind, 0);
        view.move_selection(1);
        assert_eq!(view.selected_kind, 1);
        view.move_selection(1);
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
}

//! UI-independent runtime shared by terminal, desktop, and web frontends.

#![forbid(unsafe_code)]

mod chat;
mod command;
mod provider;

use std::io::IsTerminal;
use std::path::PathBuf;
use std::time::Duration;

use alpha_front_client::{ConnectionEvent, CoreInfo};
use alpha_front_supervisor::{CoreSupervisor, SupervisorConfig, SupervisorError};
use alpha_schema::{
    InteractionRequest, PermissionMode, ReasoningLevel, SessionAttached, SessionCreated,
    SessionEventNotification, SessionId, SessionListEntry, SessionResumed, TurnStarted,
};
use serde_json::{Value, json};

pub use chat::{
    ChatState, ChatUpdate, InteractionDecision, InteractionPrompt, ModelRouteState, TranscriptItem,
};
pub use command::{COMMANDS, CommandSpec, FrontCommand, parse_command};
pub use provider::{
    ModelChoice, ProviderRemoved, ProviderState, resolve_model_selection,
    supported_reasoning_levels,
};

const EVENT_BATCH_LIMIT: usize = 256;

/// Error from a UI-independent frontend operation.
#[derive(Debug, thiserror::Error)]
pub enum FrontError {
    /// Core process failure.
    #[error(transparent)]
    Supervisor(#[from] SupervisorError),
    /// Invalid or malformed core response.
    #[error("invalid core response: {0}")]
    Protocol(String),
}

/// Defaults used when creating a session.
#[derive(Clone, Debug)]
pub struct SessionOptions {
    /// Workspace root.
    pub workspace: PathBuf,
    /// Initial permission mode.
    pub mode: PermissionMode,
    /// Provider instance or provider kind.
    pub provider: String,
    /// Model identifier.
    pub model: String,
    /// Reasoning level.
    pub reasoning: ReasoningLevel,
    /// Provider-native tool override.
    pub provider_tools: Option<Vec<String>>,
}

impl SessionOptions {
    /// Create options for one workspace.
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
            mode: PermissionMode::Auto,
            provider: String::new(),
            model: String::new(),
            reasoning: ReasoningLevel::Auto,
            provider_tools: None,
        }
    }
}

/// Runtime startup configuration.
#[derive(Clone, Debug)]
pub struct FrontRuntimeConfig {
    /// Core process supervisor configuration.
    pub supervisor: SupervisorConfig,
    /// Session defaults consumed by the next draft.
    pub session: SessionOptions,
    /// Whether to load provider state before returning.
    pub refresh_providers: bool,
}

impl Default for FrontRuntimeConfig {
    fn default() -> Self {
        let workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self {
            supervisor: SupervisorConfig::default(),
            session: SessionOptions::new(workspace),
            refresh_providers: true,
        }
    }
}

/// Runtime event surfaced to a frontend.
#[derive(Clone, Debug, PartialEq)]
pub enum FrontEvent {
    /// One live agent event.
    Live {
        /// Live payload.
        event: alpha_schema::LiveAgentEvent,
    },
    /// A turn completed.
    TurnEnded {
        /// Why the turn ended.
        reason: alpha_schema::TurnEndReason,
    },
    /// An interaction became pending.
    InteractionRequested {
        /// Interaction kind.
        kind: String,
    },
    /// The core restarted.
    CoreRestarted {
        /// Whether the active session was resumed.
        resumed: bool,
    },
    /// The core disconnected and could not be restarted.
    CoreDisconnected {
        /// Failure detail.
        error: String,
    },
}

/// UI-independent state and protocol controller shared by all frontends.
pub struct FrontRuntime {
    supervisor: CoreSupervisor,
    chat: ChatState,
    providers: ProviderState,
    session_options: SessionOptions,
}

impl FrontRuntime {
    /// Start a managed core and initialize shared front state.
    pub fn start(config: FrontRuntimeConfig) -> Result<Self, FrontError> {
        let mut supervisor = CoreSupervisor::start(config.supervisor)?;
        let providers = if config.refresh_providers {
            let mut providers = ProviderState::default();
            providers.refresh(&mut supervisor)?;
            providers
        } else {
            ProviderState::default()
        };
        let chat = ChatState {
            mode: config.session.mode,
            route: ModelRouteState {
                provider: config.session.provider.clone(),
                model: config.session.model.clone(),
                reasoning: config.session.reasoning,
            },
            ..ChatState::default()
        };
        Ok(Self {
            supervisor,
            chat,
            providers,
            session_options: config.session,
        })
    }

    /// Build a runtime over an existing supervisor.
    pub fn with_supervisor(
        mut supervisor: CoreSupervisor,
        session: SessionOptions,
        providers: ProviderState,
    ) -> Result<Self, FrontError> {
        let mut providers = providers;
        if providers.kinds.is_empty() {
            providers.refresh(&mut supervisor)?;
        }
        let chat = ChatState {
            mode: session.mode,
            route: ModelRouteState {
                provider: session.provider.clone(),
                model: session.model.clone(),
                reasoning: session.reasoning,
            },
            ..ChatState::default()
        };
        Ok(Self {
            supervisor,
            chat,
            providers,
            session_options: session,
        })
    }

    /// Shared chat state.
    pub fn chat(&self) -> &ChatState {
        &self.chat
    }

    /// Shared provider state.
    pub fn providers(&self) -> &ProviderState {
        &self.providers
    }

    /// Session defaults.
    pub fn session_options(&self) -> &SessionOptions {
        &self.session_options
    }

    /// Mutable session defaults.
    pub fn session_options_mut(&mut self) -> &mut SessionOptions {
        &mut self.session_options
    }

    /// Core handshake information.
    pub fn core_info(&self) -> &CoreInfo {
        self.supervisor.info()
    }

    /// Reload provider state.
    pub fn refresh_providers(&mut self) -> Result<(), FrontError> {
        self.providers.refresh(&mut self.supervisor)
    }

    /// Refresh dynamic models.
    pub fn refresh_models(&mut self, provider: Option<&str>) -> Result<(), FrontError> {
        self.providers
            .refresh_models(&mut self.supervisor, provider)
    }

    /// Create a session using the current defaults.
    pub fn create_session(&mut self) -> Result<SessionCreated, FrontError> {
        let workspace = self.session_options.workspace.clone();
        let created = self.create_session_with(
            workspace.clone(),
            workspace,
            self.session_options.mode,
            self.session_options.provider.clone(),
            self.session_options.model.clone(),
            self.session_options.reasoning,
        )?;
        self.chat.session_id = Some(created.session_id);
        self.chat.route = ModelRouteState {
            provider: created.provider.clone(),
            model: created.model.clone(),
            reasoning: created.reasoning,
        };
        self.chat.mode = created.mode;
        self.chat.pending_provider_tools = None;
        self.chat.status = Some("session ready".to_owned());
        if let Some(provider_tools) = self.session_options.provider_tools.clone() {
            self.set_capabilities(provider_tools)?;
        }
        Ok(created)
    }

    /// Create a session with explicit parameters.
    pub fn create_session_with(
        &mut self,
        workspace: PathBuf,
        cwd: PathBuf,
        mode: PermissionMode,
        provider: String,
        model: String,
        reasoning: ReasoningLevel,
    ) -> Result<SessionCreated, FrontError> {
        let response = self.supervisor.request(
            "session.create",
            Some(json!({
                "workspace": workspace,
                "cwd": cwd,
                "mode": mode,
                "provider": provider,
                "model": model,
                "reasoning": reasoning,
            })),
        )?;
        parse_response(response)
    }

    /// Resume and attach to an existing session.
    pub fn resume_session(&mut self, session_id: SessionId) -> Result<SessionResumed, FrontError> {
        let resumed: SessionResumed = parse_response(
            self.supervisor
                .request("session.resume", Some(json!({ "sessionId": session_id })))?,
        )?;
        let attached: SessionAttached = parse_response(self.supervisor.request(
            "session.attach",
            Some(json!({ "sessionId": session_id, "afterSeq": 0 })),
        )?)?;
        self.chat.session_id = Some(session_id);
        self.chat.transcript.clear();
        self.chat.streaming_assistant = None;
        self.chat.streaming_thinking = None;
        self.chat.running_turn = None;
        self.chat.interaction = None;
        self.chat.pending_provider_tools = None;
        self.chat.last_seq = None;
        self.chat.route = ModelRouteState {
            provider: resumed.provider.clone(),
            model: resumed.model.clone(),
            reasoning: resumed.reasoning,
        };
        self.chat.mode = resumed.mode;
        self.chat.status = Some(format!(
            "resumed {session_id} ({} messages, {} replayed)",
            resumed.messages, attached.replayed
        ));
        Ok(resumed)
    }

    /// Start a new unpersisted draft.
    pub fn new_session(&mut self) {
        self.chat.new_session();
    }

    /// Submit text, creating the session on first use.
    pub fn submit_prompt(&mut self, prompt: &str) -> Result<(), FrontError> {
        let prompt = prompt.trim();
        if prompt.is_empty() {
            return Ok(());
        }
        if self.chat.session_id.is_none() {
            self.create_session()?;
        }
        let session_id = self
            .chat
            .session_id
            .ok_or_else(|| FrontError::Protocol("no active session".to_owned()))?;
        if self.chat.running_turn.is_some() {
            self.supervisor.request(
                "session.steer",
                Some(json!({ "sessionId": session_id, "text": prompt })),
            )?;
            self.chat.status = Some("steer queued".to_owned());
        } else {
            let response = self.supervisor.request(
                "session.submit",
                Some(json!({ "sessionId": session_id, "prompt": prompt })),
            )?;
            let started: TurnStarted = parse_response(response)?;
            self.chat.running_turn = Some(started.turn_id);
            self.chat.status = Some("turn started".to_owned());
        }
        Ok(())
    }

    /// Cancel the active turn.
    pub fn cancel(&mut self) -> Result<(), FrontError> {
        let (Some(session_id), Some(turn_id)) = (self.chat.session_id, self.chat.running_turn)
        else {
            return Ok(());
        };
        self.supervisor.request(
            "session.cancel",
            Some(json!({ "sessionId": session_id, "turnId": turn_id })),
        )?;
        self.chat.status = Some("cancel requested".to_owned());
        Ok(())
    }

    /// Set the effective permission mode.
    pub fn set_mode(&mut self, mode: PermissionMode) -> Result<(), FrontError> {
        if let Some(session_id) = self.chat.session_id {
            self.supervisor.request(
                "session.set_mode",
                Some(json!({ "sessionId": session_id, "mode": mode })),
            )?;
        }
        self.chat.mode = mode;
        self.session_options.mode = mode;
        self.chat.status = Some(format!("mode: {mode}"));
        Ok(())
    }

    /// Set the session route.
    pub fn set_route(
        &mut self,
        provider: String,
        model: String,
        reasoning: ReasoningLevel,
    ) -> Result<(), FrontError> {
        if provider.trim().is_empty() || model.trim().is_empty() {
            return Err(FrontError::Protocol(
                "provider and model must not be empty".to_owned(),
            ));
        }
        if let Some(session_id) = self.chat.session_id {
            self.supervisor.request(
                "session.set_route",
                Some(json!({
                    "sessionId": session_id,
                    "provider": provider,
                    "model": model,
                    "reasoning": reasoning,
                })),
            )?;
        }
        self.chat.route = ModelRouteState {
            provider: provider.clone(),
            model: model.clone(),
            reasoning,
        };
        self.session_options.provider = provider;
        self.session_options.model = model;
        self.session_options.reasoning = reasoning;
        self.chat.status = Some(format!(
            "route: {}/{}  effort:{}",
            self.chat.route.provider, self.chat.route.model, self.chat.route.reasoning
        ));
        Ok(())
    }

    /// Confirm or discard pending steers.
    pub fn confirm_steers(&mut self, accept: bool) -> Result<(), FrontError> {
        let session_id = self
            .chat
            .session_id
            .ok_or_else(|| FrontError::Protocol("no active session".to_owned()))?;
        self.supervisor.request(
            "session.confirm_steers",
            Some(json!({ "sessionId": session_id, "accept": accept })),
        )?;
        self.chat.status = Some(if accept {
            "pending steers confirmed".to_owned()
        } else {
            "pending steers discarded".to_owned()
        });
        Ok(())
    }

    /// Set provider-native tools.
    pub fn set_capabilities(&mut self, provider_tools: Vec<String>) -> Result<(), FrontError> {
        if let Some(session_id) = self.chat.session_id {
            self.supervisor.request(
                "session.set_capabilities",
                Some(json!({
                    "sessionId": session_id,
                    "providerTools": provider_tools,
                })),
            )?;
        }
        self.chat.pending_provider_tools = Some(provider_tools);
        self.session_options.provider_tools = self.chat.pending_provider_tools.clone();
        self.chat.status = Some("provider capabilities updated".to_owned());
        Ok(())
    }

    /// List stored sessions.
    pub fn list_sessions(&mut self) -> Result<Vec<SessionListEntry>, FrontError> {
        parse_response(self.supervisor.request("session.list", None)?)
    }

    /// Delete one stored session.
    pub fn delete_session(&mut self, session_id: SessionId) -> Result<(), FrontError> {
        self.supervisor
            .request("session.delete", Some(json!({ "sessionId": session_id })))?;
        if self.chat.session_id == Some(session_id) {
            self.new_session();
        }
        Ok(())
    }

    /// Respond to the pending interaction.
    pub fn respond_interaction(&mut self, decision: InteractionDecision) -> Result<(), FrontError> {
        let Some(prompt) = self.chat.finish_interaction() else {
            return Ok(());
        };
        self.supervisor
            .respond(prompt.id, Ok(json!({ "decision": decision.as_str() })))?;
        self.chat.status = Some(format!("interaction: {}", decision.as_str()));
        Ok(())
    }

    /// Expire the current interaction, if necessary.
    pub fn expire_interaction(&mut self) -> Result<bool, FrontError> {
        if !self.chat.interaction_expired() {
            return Ok(false);
        }
        self.respond_interaction(InteractionDecision::Timeout)?;
        Ok(true)
    }

    /// Execute one parsed command that does not require a frontend picker.
    pub fn execute_command(&mut self, command: FrontCommand) -> Result<CommandOutcome, FrontError> {
        match command {
            FrontCommand::OpenProviders => Ok(CommandOutcome::OpenProviders),
            FrontCommand::OpenModels => {
                self.refresh_providers()?;
                let provider = self.chat.route.provider.clone();
                self.refresh_models(Some(&provider))?;
                Ok(CommandOutcome::OpenModels)
            }
            FrontCommand::OpenThinking => Ok(CommandOutcome::OpenThinking),
            FrontCommand::OpenSessions => Ok(CommandOutcome::OpenSessions),
            FrontCommand::Help => Ok(CommandOutcome::Help),
            FrontCommand::Quit => Ok(CommandOutcome::Quit),
            FrontCommand::SelectProvider(provider) => {
                self.refresh_providers()?;
                self.refresh_models(Some(&provider))?;
                let choices = self.providers.model_choices();
                let choice = choices
                    .iter()
                    .find(|choice| choice.provider == provider)
                    .cloned()
                    .ok_or_else(|| {
                        FrontError::Protocol(format!("unknown provider instance: {provider}"))
                    })?;
                let reasoning = if choice.reasoning_levels.contains(&self.chat.route.reasoning) {
                    self.chat.route.reasoning
                } else {
                    choice
                        .reasoning_levels
                        .first()
                        .copied()
                        .unwrap_or(ReasoningLevel::Auto)
                };
                self.set_route(choice.provider.clone(), choice.model.clone(), reasoning)?;
                self.providers.set_default_model(
                    &mut self.supervisor,
                    &choice.provider,
                    &choice.model,
                )?;
                Ok(CommandOutcome::Completed)
            }
            FrontCommand::SelectModel { target, reasoning } => {
                self.refresh_providers()?;
                let current_provider = self.chat.route.provider.clone();
                self.refresh_models(Some(&current_provider))?;
                let choices = self.providers.model_choices();
                let selected =
                    resolve_model_selection(&choices, &self.chat.route.provider, &target)
                        .ok_or_else(|| FrontError::Protocol(format!("unknown model: {target}")))?;
                let requested = reasoning.unwrap_or(self.chat.route.reasoning);
                let reasoning = if selected.reasoning_levels.contains(&requested) {
                    requested
                } else {
                    selected
                        .reasoning_levels
                        .first()
                        .copied()
                        .unwrap_or(ReasoningLevel::Auto)
                };
                self.set_route(selected.provider.clone(), selected.model.clone(), reasoning)?;
                self.providers.set_default_model(
                    &mut self.supervisor,
                    &selected.provider,
                    &selected.model,
                )?;
                Ok(CommandOutcome::Completed)
            }
            FrontCommand::SetThinking(level) => {
                self.refresh_providers()?;
                let provider = self.chat.route.provider.clone();
                self.refresh_models(Some(&provider))?;
                let levels = supported_reasoning_levels(
                    &self.providers.model_choices(),
                    &self.chat.route.provider,
                    &self.chat.route.model,
                );
                if !levels.contains(&level) {
                    return Err(FrontError::Protocol(format!(
                        "reasoning effort `{level}` is not available for this model"
                    )));
                }
                self.set_route(
                    self.chat.route.provider.clone(),
                    self.chat.route.model.clone(),
                    level,
                )?;
                Ok(CommandOutcome::Completed)
            }
            FrontCommand::SetMode(mode) => {
                self.set_mode(mode)?;
                Ok(CommandOutcome::Completed)
            }
            FrontCommand::ConfirmSteers => {
                self.confirm_steers(true)?;
                Ok(CommandOutcome::Completed)
            }
            FrontCommand::DiscardSteers => {
                self.confirm_steers(false)?;
                Ok(CommandOutcome::Completed)
            }
            FrontCommand::SetCapabilities(tools) => {
                self.set_capabilities(tools)?;
                Ok(CommandOutcome::Completed)
            }
            FrontCommand::NewSession => {
                self.new_session();
                Ok(CommandOutcome::Completed)
            }
            FrontCommand::Resume(session_id) => {
                self.resume_session(session_id)?;
                Ok(CommandOutcome::Completed)
            }
            FrontCommand::Diagnostics => {
                let info = self.core_info();
                self.chat.status = Some(format!(
                    "core pid={} protocol={} instance={}",
                    info.pid, info.protocol, info.core_instance_id
                ));
                Ok(CommandOutcome::Completed)
            }
            FrontCommand::Empty => Ok(CommandOutcome::Completed),
            FrontCommand::Unknown(other) => {
                self.chat.status = Some(format!("unknown command: {other}"));
                Ok(CommandOutcome::Completed)
            }
            FrontCommand::Invalid(message) => Err(FrontError::Protocol(message)),
        }
    }

    /// Receive and reduce up to one bounded batch of core events.
    pub fn receive_events(&mut self, timeout: Duration) -> Result<Vec<FrontEvent>, FrontError> {
        let mut out = Vec::new();
        let first = self.supervisor.recv_event_timeout(timeout)?;
        if let Some(event) = first {
            self.handle_connection_event(event, &mut out)?;
        } else {
            return Ok(out);
        }
        while out.len() < EVENT_BATCH_LIMIT {
            let Some(event) = self.supervisor.try_recv_event()? else {
                break;
            };
            self.handle_connection_event(event, &mut out)?;
        }
        Ok(out)
    }

    /// Take every immediately available core event.
    pub fn try_receive_events(&mut self) -> Result<Vec<FrontEvent>, FrontError> {
        self.receive_events(Duration::ZERO)
    }

    /// Shut down the managed core.
    pub fn shutdown(self) -> Result<(), FrontError> {
        self.supervisor.shutdown()?;
        Ok(())
    }

    fn handle_connection_event(
        &mut self,
        event: ConnectionEvent,
        out: &mut Vec<FrontEvent>,
    ) -> Result<(), FrontError> {
        match event {
            ConnectionEvent::Notification(notification)
                if notification.method == "session.event" =>
            {
                let Some(params) = notification.params else {
                    return Ok(());
                };
                let notification: SessionEventNotification = parse_response(params)?;
                out.extend(
                    self.chat
                        .apply(notification)
                        .into_iter()
                        .map(map_chat_update),
                );
            }
            ConnectionEvent::Request(request) if request.method == "interaction.request" => {
                let Some(params) = request.params else {
                    return Ok(());
                };
                let params: InteractionRequest = parse_response(params)?;
                let update = self.chat.set_interaction(
                    request.id,
                    params.session_id,
                    params.kind,
                    params.prompt,
                );
                out.push(map_chat_update(update));
            }
            ConnectionEvent::Closed(error) => {
                self.chat.status = Some(format!("core disconnected: {error}"));
                match self.supervisor.restart() {
                    Ok(()) => {
                        let resumed = if let Some(session_id) = self.chat.session_id {
                            self.resume_session(session_id).is_ok()
                        } else {
                            false
                        };
                        out.push(FrontEvent::CoreRestarted { resumed });
                    }
                    Err(restart) => {
                        out.push(FrontEvent::CoreDisconnected {
                            error: restart.to_string(),
                        });
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}

/// Result of executing one parsed front command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandOutcome {
    /// Command completed without opening a picker.
    Completed,
    /// Frontend should open provider management.
    OpenProviders,
    /// Frontend should open the model picker.
    OpenModels,
    /// Frontend should open the reasoning picker.
    OpenThinking,
    /// Frontend should open the session picker.
    OpenSessions,
    /// Frontend should show help.
    Help,
    /// Frontend should exit.
    Quit,
}

/// Whether a terminal front can prompt for approval interactively.
pub fn terminal_approval_available() -> bool {
    std::io::stdin().is_terminal()
}

/// Answer one server-initiated interaction using the standard terminal prompt.
pub fn read_terminal_approval(params: &Value) -> InteractionDecision {
    if !std::io::stdin().is_terminal() {
        return InteractionDecision::Deny;
    }
    eprintln!(
        "alpha run: permission requested: {}",
        serde_json::to_string(params).unwrap_or_default()
    );
    eprintln!("[a]llow once  [s]ession  [d]eny");
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return InteractionDecision::Deny;
    }
    match line.trim().to_ascii_lowercase().as_str() {
        "a" | "y" | "yes" => InteractionDecision::Once,
        "s" | "session" => InteractionDecision::Session,
        _ => InteractionDecision::Deny,
    }
}

fn map_chat_update(update: ChatUpdate) -> FrontEvent {
    match update {
        ChatUpdate::Live(event) => FrontEvent::Live { event },
        ChatUpdate::TurnEnded { reason } => FrontEvent::TurnEnded { reason },
        ChatUpdate::InteractionRequested { kind } => FrontEvent::InteractionRequested { kind },
    }
}

fn parse_response<T>(value: Value) -> Result<T, FrontError>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_value(value).map_err(|error| FrontError::Protocol(error.to_string()))
}

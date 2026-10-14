//! Session actors and live event delivery.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use alpha_kernel::Ctx;
use alpha_module_loop::{
    AgentDriver, LoopError, ModelRoute, ModelRouteOverride, ProviderToolsOverride, SteerMessage,
    SteerQueue,
};
use alpha_module_permission::PermissionService;
use alpha_module_session::{
    SessionError, SessionEvent, SessionHeader, SessionState, SessionStore, SessionWriter,
    TurnEndReason, WorkspaceRef,
};
use alpha_protocol::{OutboundTransport, RpcError};
use alpha_schema::{MessageId, PermissionMode, ReasoningLevel, SessionId, TurnId};
use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// Stable module key used by the generated composition table.
pub const __ALPHA_MODULE__: &str = "agent.host";

/// Failure while creating or driving a session actor.
#[derive(Debug, Error)]
pub enum AgentHostError {
    /// Session storage failed.
    #[error(transparent)]
    Session(#[from] SessionError),
    /// The requested session does not exist.
    #[error("session `{0}` does not exist")]
    SessionNotFound(SessionId),
    /// The session already has a running turn.
    #[error("session `{0}` is busy")]
    SessionBusy(SessionId),
    /// The session actor stopped unexpectedly.
    #[error("session `{0}` actor stopped")]
    ActorStopped(SessionId),
    /// Runtime construction failed.
    #[error("session runtime failed: {0}")]
    Runtime(String),
    /// The request was invalid.
    #[error("invalid session request: {0}")]
    Invalid(String),
}

/// One session runtime built by the application composition.
pub struct SessionRuntime {
    /// Session-scoped context.
    pub ctx: Ctx,
    /// Agent driver for one turn.
    pub agent: Arc<dyn AgentDriver>,
    /// Session-level provider-native tool override.
    pub provider_tools_override: Arc<ProviderToolsOverride>,
    /// Session model and reasoning route.
    pub model_route: Arc<ModelRouteOverride>,
}

/// Inputs used to build one session runtime.
#[derive(Clone)]
pub struct SessionRuntimeRequest {
    /// Session identity.
    pub session_id: SessionId,
    /// Workspace binding.
    pub workspace: WorkspaceRef,
    /// Permission mode.
    pub mode: PermissionMode,
    /// Provider instance or provider kind.
    pub provider: String,
    /// Model identifier.
    pub model: String,
    /// Requested reasoning effort.
    pub reasoning: ReasoningLevel,
    /// Recovered session capability override.
    pub provider_tools_override: Option<Vec<String>>,
    /// Queue consumed by the default loop at step boundaries.
    pub steer_queue: Arc<dyn SteerQueue>,
}

/// Builds session-scoped runtimes from the application composition.
pub trait SessionRuntimeFactory: Send + Sync + 'static {
    /// Build one runtime.
    fn build(&self, request: SessionRuntimeRequest) -> Result<SessionRuntime, AgentHostError>;
}

/// Request to create one session.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionRequest {
    /// Workspace root.
    pub workspace: PathBuf,
    /// Fixed cwd inside the workspace.
    pub cwd: Option<PathBuf>,
    /// Permission mode.
    pub mode: PermissionMode,
    /// Provider instance or provider kind.
    pub provider: String,
    /// Model identifier.
    pub model: String,
    /// Requested reasoning effort.
    #[serde(default)]
    pub reasoning: ReasoningLevel,
}

/// Result of creating a session.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCreated {
    /// Session identity.
    pub session_id: SessionId,
    /// Effective permission mode.
    pub mode: PermissionMode,
    /// Workspace root.
    pub workspace: String,
    /// Effective provider instance id or provider kind.
    pub provider: String,
    /// Effective model identifier.
    pub model: String,
    /// Effective reasoning effort.
    pub reasoning: ReasoningLevel,
}

/// Result of attaching to a session.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionAttached {
    /// Number of durable events replayed.
    pub replayed: usize,
}

/// Result of resuming a session.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionResumed {
    /// Session identity.
    pub session_id: SessionId,
    /// Effective mode.
    pub mode: PermissionMode,
    /// Workspace root.
    pub workspace: String,
    /// Number of messages in the projection.
    pub messages: usize,
    /// Whether a turn was open before repair.
    pub turn_open: bool,
    /// Number of repair events appended.
    pub repairs: usize,
    /// Effective provider instance id or provider kind.
    pub provider: String,
    /// Effective model identifier.
    pub model: String,
    /// Effective reasoning effort.
    pub reasoning: ReasoningLevel,
}

struct ActorSteerQueue {
    queue: Mutex<VecDeque<SteerMessage>>,
}

const MAX_PENDING_STEERS: usize = 16;

impl ActorSteerQueue {
    fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
        }
    }

    fn push(&self, message: SteerMessage) -> Result<(), AgentHostError> {
        let mut queue = self.queue.lock().expect("steer queue lock poisoned");
        if queue.len() >= MAX_PENDING_STEERS {
            return Err(AgentHostError::Invalid("steer queue is full".to_owned()));
        }
        queue.push_back(message);
        Ok(())
    }

    fn clear(&self) {
        self.queue
            .lock()
            .expect("steer queue lock poisoned")
            .clear();
    }
}

impl SteerQueue for ActorSteerQueue {
    fn drain(&self) -> Vec<SteerMessage> {
        self.queue
            .lock()
            .expect("steer queue lock poisoned")
            .drain(..)
            .collect()
    }
}

enum SessionCommand {
    Submit {
        turn_id: TurnId,
        prompt: String,
    },
    Cancel {
        turn_id: TurnId,
        reply: oneshot::Sender<Result<(), AgentHostError>>,
    },
    Steer {
        text: String,
        reply: oneshot::Sender<Result<(), AgentHostError>>,
    },
    SetMode {
        mode: PermissionMode,
        reply: oneshot::Sender<Result<(), AgentHostError>>,
    },
    ConfirmSteers {
        accept: bool,
        reply: oneshot::Sender<Result<(), AgentHostError>>,
    },
    SetCapabilities {
        provider_tools: Vec<String>,
        reply: oneshot::Sender<Result<(), AgentHostError>>,
    },
    SetRoute {
        route: ModelRoute,
        reply: oneshot::Sender<Result<(), AgentHostError>>,
    },
}

struct TurnCompletion {
    result: Result<(), LoopError>,
}

struct SessionActor {
    tx: mpsc::UnboundedSender<SessionCommand>,
    running: Arc<AtomicBool>,
    state: Arc<Mutex<SessionState>>,
}

/// Global session actor registry.
pub struct AgentHostService {
    store: SessionStore,
    transport: Arc<dyn OutboundTransport>,
    sink: Arc<SessionEventSink>,
    factory: Arc<dyn SessionRuntimeFactory>,
    actors: Arc<Mutex<HashMap<SessionId, Arc<SessionActor>>>>,
    idle_timeout: Duration,
}

impl AgentHostService {
    /// Create a host over one session store and protocol transport.
    pub fn new(
        store: SessionStore,
        transport: Arc<dyn OutboundTransport>,
        factory: Arc<dyn SessionRuntimeFactory>,
    ) -> Self {
        Self {
            store,
            sink: Arc::new(SessionEventSink::new(transport.clone())),
            transport,
            factory,
            actors: Arc::new(Mutex::new(HashMap::new())),
            idle_timeout: Duration::from_secs(30 * 60),
        }
    }

    /// Override the idle actor eviction timeout.
    pub fn with_idle_timeout(mut self, idle_timeout: Duration) -> Self {
        self.idle_timeout = idle_timeout;
        self
    }

    /// Number of currently resident session actors.
    pub fn session_count(&self) -> usize {
        self.actors.lock().expect("agent host lock poisoned").len()
    }

    /// Create a new session and start its actor.
    pub fn create_session(
        &self,
        request: CreateSessionRequest,
    ) -> Result<SessionCreated, AgentHostError> {
        let workspace = WorkspaceRef {
            root: request.workspace.display().to_string(),
            cwd: request
                .cwd
                .unwrap_or_else(|| request.workspace.clone())
                .display()
                .to_string(),
        };
        let mut header = SessionHeader::new(SessionId::new(), request.mode, workspace.clone());
        header.provider = request.provider.clone();
        header.model = request.model.clone();
        let session_id = header.id;
        let mode = header.mode;
        let writer = self.store.create(header.clone())?;
        let steer_queue = Arc::new(ActorSteerQueue::new());
        let runtime = self.factory.build(SessionRuntimeRequest {
            session_id: header.id,
            workspace: workspace.clone(),
            mode: request.mode,
            provider: request.provider.clone(),
            model: request.model.clone(),
            reasoning: request.reasoning,
            provider_tools_override: None,
            steer_queue: steer_queue.clone(),
        })?;
        let route = runtime.model_route.get();
        self.spawn_actor(header, writer, runtime, steer_queue, Vec::new())?;
        Ok(SessionCreated {
            session_id,
            mode,
            workspace: workspace.root,
            provider: route.provider,
            model: route.model,
            reasoning: route.reasoning,
        })
    }

    /// Resume an existing session, repairing interrupted work on write-open.
    pub fn resume_session(&self, session_id: SessionId) -> Result<SessionResumed, AgentHostError> {
        if let Some(actor) = self.actor(session_id) {
            let state = actor.state.lock().expect("session state lock poisoned");
            return Ok(SessionResumed {
                session_id,
                mode: state.mode,
                workspace: state.workspace.root.clone(),
                messages: state.messages.len(),
                turn_open: state.turn_open,
                repairs: 0,
                provider: state.provider.clone(),
                model: state.model.clone(),
                reasoning: state.reasoning,
            });
        }
        let reader = self
            .store
            .open_read(session_id)
            .map_err(|error| match error {
                SessionError::NotFound(_) => AgentHostError::SessionNotFound(session_id),
                other => AgentHostError::Session(other),
            })?;
        let header = reader.header().clone();
        let state = reader.state();
        let recovered_steers = pending_steers(reader.events());
        drop(reader);
        let writer = match self.store.open_write(session_id) {
            Ok(writer) => writer,
            Err(SessionError::WriterBusy { .. }) => {
                return Err(AgentHostError::SessionBusy(session_id));
            }
            Err(error) => return Err(error.into()),
        };
        let repairs = writer.repairs().len();
        let steer_queue = Arc::new(ActorSteerQueue::new());
        let runtime = self.factory.build(SessionRuntimeRequest {
            session_id,
            workspace: header.workspace.clone(),
            mode: state.mode,
            provider: state.provider.clone(),
            model: state.model.clone(),
            reasoning: state.reasoning,
            provider_tools_override: state.provider_tools_override.clone(),
            steer_queue: steer_queue.clone(),
        })?;
        self.spawn_actor_with_state(
            header,
            writer,
            runtime,
            state.clone(),
            steer_queue,
            recovered_steers,
        )?;
        Ok(SessionResumed {
            session_id,
            mode: state.mode,
            workspace: state.workspace.root,
            messages: state.messages.len(),
            turn_open: state.turn_open,
            repairs,
            provider: state.provider,
            model: state.model,
            reasoning: state.reasoning,
        })
    }

    /// Submit one prompt and return its turn id immediately.
    pub fn submit(&self, session_id: SessionId, prompt: String) -> Result<TurnId, AgentHostError> {
        if prompt.trim().is_empty() {
            return Err(AgentHostError::Invalid("prompt is empty".to_owned()));
        }
        let actor = self
            .actor(session_id)
            .ok_or(AgentHostError::SessionNotFound(session_id))?;
        actor
            .running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| AgentHostError::SessionBusy(session_id))?;
        let turn_id = TurnId::new();
        actor
            .tx
            .send(SessionCommand::Submit { turn_id, prompt })
            .map_err(|_| {
                actor.running.store(false, Ordering::SeqCst);
                AgentHostError::ActorStopped(session_id)
            })?;
        Ok(turn_id)
    }

    /// Cancel one running turn.
    pub async fn cancel(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
    ) -> Result<(), AgentHostError> {
        let actor = self
            .actor(session_id)
            .ok_or(AgentHostError::SessionNotFound(session_id))?;
        let (reply, receiver) = oneshot::channel();
        actor
            .tx
            .send(SessionCommand::Cancel { turn_id, reply })
            .map_err(|_| AgentHostError::ActorStopped(session_id))?;
        receiver
            .await
            .map_err(|_| AgentHostError::ActorStopped(session_id))?
    }

    /// Queue a steer message for the current or next turn.
    pub async fn steer(&self, session_id: SessionId, text: String) -> Result<(), AgentHostError> {
        if text.trim().is_empty() {
            return Err(AgentHostError::Invalid("steer is empty".to_owned()));
        }
        let actor = self
            .actor(session_id)
            .ok_or(AgentHostError::SessionNotFound(session_id))?;
        let (reply, receiver) = oneshot::channel();
        actor
            .tx
            .send(SessionCommand::Steer { text, reply })
            .map_err(|_| AgentHostError::ActorStopped(session_id))?;
        receiver
            .await
            .map_err(|_| AgentHostError::ActorStopped(session_id))?
    }

    /// Change the effective mode and record it durably.
    pub async fn set_mode(
        &self,
        session_id: SessionId,
        mode: PermissionMode,
    ) -> Result<(), AgentHostError> {
        let actor = self
            .actor(session_id)
            .ok_or(AgentHostError::SessionNotFound(session_id))?;
        let (reply, receiver) = oneshot::channel();
        actor
            .tx
            .send(SessionCommand::SetMode { mode, reply })
            .map_err(|_| AgentHostError::ActorStopped(session_id))?;
        receiver
            .await
            .map_err(|_| AgentHostError::ActorStopped(session_id))?
    }

    /// Change the provider, model, and reasoning route for future model steps.
    pub async fn set_route(
        &self,
        session_id: SessionId,
        route: ModelRoute,
    ) -> Result<(), AgentHostError> {
        let actor = self
            .actor(session_id)
            .ok_or(AgentHostError::SessionNotFound(session_id))?;
        let (reply, receiver) = oneshot::channel();
        actor
            .tx
            .send(SessionCommand::SetRoute { route, reply })
            .map_err(|_| AgentHostError::ActorStopped(session_id))?;
        receiver
            .await
            .map_err(|_| AgentHostError::ActorStopped(session_id))?
    }

    /// Confirm or discard steers waiting outside an active turn.
    pub async fn confirm_steers(
        &self,
        session_id: SessionId,
        accept: bool,
    ) -> Result<(), AgentHostError> {
        let actor = self
            .actor(session_id)
            .ok_or(AgentHostError::SessionNotFound(session_id))?;
        let (reply, receiver) = oneshot::channel();
        actor
            .tx
            .send(SessionCommand::ConfirmSteers { accept, reply })
            .map_err(|_| AgentHostError::ActorStopped(session_id))?;
        receiver
            .await
            .map_err(|_| AgentHostError::ActorStopped(session_id))?
    }

    /// Override provider-native tools for this session.
    pub async fn set_capabilities(
        &self,
        session_id: SessionId,
        provider_tools: Vec<String>,
    ) -> Result<(), AgentHostError> {
        let actor = self
            .actor(session_id)
            .ok_or(AgentHostError::SessionNotFound(session_id))?;
        let (reply, receiver) = oneshot::channel();
        actor
            .tx
            .send(SessionCommand::SetCapabilities {
                provider_tools,
                reply,
            })
            .map_err(|_| AgentHostError::ActorStopped(session_id))?;
        receiver
            .await
            .map_err(|_| AgentHostError::ActorStopped(session_id))?
    }

    /// Replay durable events after a sequence and rely on the writer observer
    /// for subsequent live delivery.
    pub fn attach(
        &self,
        session_id: SessionId,
        after_seq: u64,
    ) -> Result<SessionAttached, AgentHostError> {
        if self.actor(session_id).is_none() {
            return Err(AgentHostError::SessionNotFound(session_id));
        }
        self.sink.begin_attach(session_id);
        let replay = (|| -> Result<(u64, usize), AgentHostError> {
            let reader = self.store.open_read(session_id)?;
            let mut last_seq = after_seq;
            let mut replayed = 0usize;
            for envelope in reader
                .events()
                .iter()
                .filter(|envelope| envelope.seq.get() > after_seq)
            {
                self.transport
                    .notify(
                        "session.event".to_owned(),
                        json!({
                            "sessionId": session_id,
                            "kind": "durable",
                            "event": envelope,
                        }),
                    )
                    .map_err(|error| AgentHostError::Runtime(error.to_string()))?;
                last_seq = envelope.seq.get();
                replayed += 1;
            }
            Ok((last_seq, replayed))
        })();
        let pending = self.sink.finish_attach(session_id);
        let (last_seq, replayed) = replay?;
        for params in pending {
            if params["kind"] == "durable" {
                let seq = params["event"]["seq"].as_u64().unwrap_or_default();
                if seq <= last_seq {
                    continue;
                }
            }
            self.transport
                .notify("session.event".to_owned(), params)
                .map_err(|error| AgentHostError::Runtime(error.to_string()))?;
        }
        Ok(SessionAttached { replayed })
    }

    /// Detach a front. The current protocol has one front per core, so this is
    /// intentionally a no-op that keeps future events flowing to the transport.
    pub fn detach(&self, session_id: SessionId) -> Result<(), AgentHostError> {
        self.actor(session_id)
            .map(|_| ())
            .ok_or(AgentHostError::SessionNotFound(session_id))
    }

    fn actor(&self, session_id: SessionId) -> Option<Arc<SessionActor>> {
        self.actors
            .lock()
            .expect("agent host lock poisoned")
            .get(&session_id)
            .cloned()
    }

    fn spawn_actor(
        &self,
        header: SessionHeader,
        writer: SessionWriter,
        runtime: SessionRuntime,
        steer_queue: Arc<ActorSteerQueue>,
        recovered_steers: Vec<SteerMessage>,
    ) -> Result<(), AgentHostError> {
        let state = SessionState::new(&header);
        self.spawn_actor_with_state(
            header,
            writer,
            runtime,
            state,
            steer_queue,
            recovered_steers,
        )
    }

    fn spawn_actor_with_state(
        &self,
        header: SessionHeader,
        writer: SessionWriter,
        runtime: SessionRuntime,
        state: SessionState,
        steer_queue: Arc<ActorSteerQueue>,
        recovered_steers: Vec<SteerMessage>,
    ) -> Result<(), AgentHostError> {
        let session_id = header.id;
        let state = Arc::new(Mutex::new(state));
        let current_turn = Arc::new(Mutex::new(None::<TurnId>));
        install_live_observers(
            &runtime.ctx,
            session_id,
            self.sink.clone(),
            current_turn.clone(),
        )?;
        let writer = Arc::new(writer);
        install_writer_observer(&writer, session_id, self.sink.clone(), state.clone());
        let (tx, rx) = mpsc::unbounded_channel();
        let running = Arc::new(AtomicBool::new(false));
        let actor = Arc::new(SessionActor {
            tx,
            running: running.clone(),
            state: state.clone(),
        });
        self.actors
            .lock()
            .expect("agent host lock poisoned")
            .insert(session_id, actor);
        let spawn_ctx = runtime.ctx.clone();
        spawn_ctx
            .scope()
            .spawn(actor_loop(
                session_id,
                runtime,
                writer,
                state,
                current_turn,
                steer_queue,
                running,
                rx,
                Arc::downgrade(&self.actors),
                self.idle_timeout,
                recovered_steers,
            ))
            .map_err(|error| AgentHostError::Runtime(error.to_string()))?;
        Ok(())
    }
}

struct SessionEventSink {
    transport: Arc<dyn OutboundTransport>,
    attaching: Mutex<HashSet<SessionId>>,
    pending: Mutex<HashMap<SessionId, Vec<serde_json::Value>>>,
}

impl SessionEventSink {
    fn new(transport: Arc<dyn OutboundTransport>) -> Self {
        Self {
            transport,
            attaching: Mutex::new(HashSet::new()),
            pending: Mutex::new(HashMap::new()),
        }
    }

    fn begin_attach(&self, session_id: SessionId) {
        self.attaching
            .lock()
            .expect("attach lock poisoned")
            .insert(session_id);
        self.pending
            .lock()
            .expect("attach pending lock poisoned")
            .insert(session_id, Vec::new());
    }

    fn finish_attach(&self, session_id: SessionId) -> Vec<serde_json::Value> {
        self.attaching
            .lock()
            .expect("attach lock poisoned")
            .remove(&session_id);
        self.pending
            .lock()
            .expect("attach pending lock poisoned")
            .remove(&session_id)
            .unwrap_or_default()
    }

    fn publish(&self, session_id: SessionId, params: serde_json::Value) -> Result<(), RpcError> {
        if self
            .attaching
            .lock()
            .expect("attach lock poisoned")
            .contains(&session_id)
        {
            self.pending
                .lock()
                .expect("attach pending lock poisoned")
                .entry(session_id)
                .or_default()
                .push(params);
            return Ok(());
        }
        self.transport.notify("session.event".to_owned(), params)
    }
}

fn install_writer_observer(
    writer: &SessionWriter,
    session_id: SessionId,
    sink: Arc<SessionEventSink>,
    state: Arc<Mutex<SessionState>>,
) {
    writer.set_observer(Arc::new(move |envelope| {
        state
            .lock()
            .expect("session state lock poisoned")
            .apply(envelope);
        let _ = sink.publish(
            session_id,
            json!({
                "sessionId": session_id,
                "kind": "durable",
                "event": envelope,
            }),
        );
    }));
}

fn pending_steers(events: &[alpha_schema::EventEnvelope<SessionEvent>]) -> Vec<SteerMessage> {
    let mut queued = BTreeMap::new();
    let mut settled = BTreeSet::new();
    for envelope in events {
        match &envelope.payload {
            SessionEvent::SteerQueued {
                steer_id,
                turn_id,
                text,
            } => {
                queued.insert(*steer_id, (*turn_id, text.clone()));
            }
            SessionEvent::SteerInjected { steer_id, .. }
            | SessionEvent::SteerDiscarded { steer_id } => {
                settled.insert(*steer_id);
            }
            _ => {}
        }
    }
    queued
        .into_iter()
        .filter(|(steer_id, _)| !settled.contains(steer_id))
        .map(|(steer_id, (turn_id, text))| SteerMessage {
            steer_id,
            turn_id,
            text,
        })
        .collect()
}

fn install_live_observers(
    ctx: &Ctx,
    session_id: SessionId,
    sink: Arc<SessionEventSink>,
    current_turn: Arc<Mutex<Option<TurnId>>>,
) -> Result<(), AgentHostError> {
    let text_sink = sink.clone();
    let text_turn = current_turn.clone();
    ctx.on_parallel::<alpha_module_loop::AgentTextDelta, _>(move |event| {
        let sink = text_sink.clone();
        let turn = text_turn.clone();
        Box::pin(async move {
            let turn_id = turn
                .lock()
                .expect("turn lock poisoned")
                .map(|id| id.to_string())
                .unwrap_or_default();
            let _ = sink.publish(
                session_id,
                json!({
                    "sessionId": session_id,
                    "kind": "live",
                    "turnId": turn_id,
                    "streamId": format!("{turn_id}:text"),
                    "event": { "type": "text_delta", "text": event.text },
                }),
            );
        })
    })
    .map_err(|error| AgentHostError::Runtime(error.to_string()))?;

    let step_sink = sink.clone();
    let step_turn = current_turn.clone();
    ctx.on_parallel::<alpha_module_loop::AgentStepStarted, _>(move |event| {
        let sink = step_sink.clone();
        let turn = step_turn.clone();
        Box::pin(async move {
            let turn_id = turn
                .lock()
                .expect("turn lock poisoned")
                .map(|id| id.to_string())
                .unwrap_or_default();
            let _ = sink.publish(
                session_id,
                json!({
                    "sessionId": session_id,
                    "kind": "live",
                    "turnId": turn_id,
                    "stepId": event.step,
                    "streamId": format!("{turn_id}:step-{}", event.step),
                    "event": { "type": "step_started", "step": event.step },
                }),
            );
        })
    })
    .map_err(|error| AgentHostError::Runtime(error.to_string()))?;

    let tool_sink = sink.clone();
    let tool_turn = current_turn.clone();
    ctx.on_parallel::<alpha_module_loop::AgentToolStarted, _>(move |event| {
        let sink = tool_sink.clone();
        let turn = tool_turn.clone();
        Box::pin(async move {
            let turn_id = turn
                .lock()
                .expect("turn lock poisoned")
                .map(|id| id.to_string())
                .unwrap_or_default();
            let _ = sink.publish(
                session_id,
                json!({
                    "sessionId": session_id,
                    "kind": "live",
                    "turnId": turn_id,
                    "streamId": format!("{turn_id}:tool"),
                    "event": {
                        "type": "tool_started",
                        "name": event.name,
                        "input": event.input,
                    },
                }),
            );
        })
    })
    .map_err(|error| AgentHostError::Runtime(error.to_string()))?;

    let finished_sink = sink.clone();
    let finished_turn = current_turn.clone();
    ctx.on_parallel::<alpha_module_loop::AgentToolFinished, _>(move |event| {
        let sink = finished_sink.clone();
        let turn = finished_turn.clone();
        Box::pin(async move {
            let turn_id = turn
                .lock()
                .expect("turn lock poisoned")
                .map(|id| id.to_string())
                .unwrap_or_default();
            let _ = sink.publish(
                session_id,
                json!({
                    "sessionId": session_id,
                    "kind": "live",
                    "turnId": turn_id,
                    "streamId": format!("{turn_id}:tool"),
                    "event": {
                        "type": "tool_finished",
                        "name": event.name,
                        "status": event.status,
                        "content": event.content,
                    },
                }),
            );
        })
    })
    .map_err(|error| AgentHostError::Runtime(error.to_string()))?;

    let error_sink = sink;
    ctx.on_parallel::<alpha_module_loop::AgentError, _>(move |event| {
        let sink = error_sink.clone();
        Box::pin(async move {
            let _ = sink.publish(
                session_id,
                json!({
                    "sessionId": session_id,
                    "kind": "live",
                    "event": {
                        "type": "error",
                        "message": event.message,
                    },
                }),
            );
        })
    })
    .map_err(|error| AgentHostError::Runtime(error.to_string()))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn actor_loop(
    session_id: SessionId,
    runtime: SessionRuntime,
    writer: Arc<SessionWriter>,
    state: Arc<Mutex<SessionState>>,
    current_turn: Arc<Mutex<Option<TurnId>>>,
    steer_queue: Arc<ActorSteerQueue>,
    running: Arc<AtomicBool>,
    mut commands: mpsc::UnboundedReceiver<SessionCommand>,
    actors: Weak<Mutex<HashMap<SessionId, Arc<SessionActor>>>>,
    idle_timeout: Duration,
    mut recovered_steers: Vec<SteerMessage>,
) {
    let (completion_tx, mut completions) = mpsc::unbounded_channel();
    let mut cancel: Option<CancellationToken> = None;
    let mut active_turn: Option<TurnId> = None;
    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else {
                    break;
                };
                match command {
                    SessionCommand::Submit { turn_id, prompt } => {
                        start_turn(
                            &runtime,
                            &writer,
                            &state,
                            &current_turn,
                            &completion_tx,
                            turn_id,
                            prompt,
                            &mut cancel,
                            &mut active_turn,
                        );
                    }
                    SessionCommand::Cancel { turn_id, reply } => {
                        let result = if active_turn == Some(turn_id) {
                            if let Some(token) = &cancel {
                                token.cancel();
                                Ok(())
                            } else {
                                Err(AgentHostError::Invalid("turn is not cancellable".to_owned()))
                            }
                        } else {
                            Err(AgentHostError::Invalid("turn is not running".to_owned()))
                        };
                        let _ = reply.send(result);
                    }
                    SessionCommand::Steer { text, reply } => {
                        let turn_id = active_turn.unwrap_or_default();
                        let steer_id = MessageId::new();
                        let result = writer
                            .append(SessionEvent::SteerQueued {
                                steer_id,
                                turn_id,
                                text: text.clone(),
                            })
                            .map(|_| ())
                            .map_err(AgentHostError::Session)
                            .and_then(|_| {
                                steer_queue.push(SteerMessage {
                                    steer_id,
                                    turn_id,
                                    text,
                                })
                            });
                        let _ = reply.send(result);
                    }
                    SessionCommand::SetMode { mode, reply } => {
                        let result = match runtime.ctx.require::<dyn PermissionService>() {
                            Ok(permission) => writer
                                .append(SessionEvent::ModeChanged { mode })
                                .map_err(AgentHostError::Session)
                                .map(|envelope| {
                                    permission.set_mode(mode);
                                    state
                                        .lock()
                                        .expect("session state lock poisoned")
                                        .apply(&envelope);
                                }),
                            Err(error) => Err(AgentHostError::Runtime(error.to_string())),
                        };
                        let _ = reply.send(result);
                    }
                    SessionCommand::ConfirmSteers { accept, reply } => {
                        if active_turn.is_some() {
                            let _ = reply.send(Err(AgentHostError::Invalid(
                                "cannot confirm steers during an active turn".to_owned(),
                            )));
                            continue;
                        }
                        if !accept {
                            for steer in recovered_steers.drain(..) {
                                let _ = writer.append(SessionEvent::SteerDiscarded {
                                    steer_id: steer.steer_id,
                                });
                            }
                            for steer in steer_queue.drain() {
                                let _ = writer.append(SessionEvent::SteerDiscarded {
                                    steer_id: steer.steer_id,
                                });
                            }
                            steer_queue.clear();
                            let _ = reply.send(Ok(()));
                            continue;
                        }
                        let queued = if recovered_steers.is_empty() {
                            steer_queue.drain()
                        } else {
                            std::mem::take(&mut recovered_steers)
                        };
                        if queued.is_empty() {
                            let _ = reply.send(Ok(()));
                            continue;
                        }
                        for steer in &queued {
                            let _ = writer.append(SessionEvent::SteerDiscarded {
                                steer_id: steer.steer_id,
                            });
                        }
                        let text = queued
                            .into_iter()
                            .map(|message| message.text)
                            .collect::<Vec<_>>()
                            .join("\n");
                        let turn_id = TurnId::new();
                        running.store(true, Ordering::SeqCst);
                        start_turn(
                            &runtime,
                            &writer,
                            &state,
                            &current_turn,
                            &completion_tx,
                            turn_id,
                            text,
                            &mut cancel,
                            &mut active_turn,
                        );
                        let _ = reply.send(Ok(()));
                    }
                    SessionCommand::SetCapabilities {
                        provider_tools,
                        reply,
                    } => {
                        let override_value = (!provider_tools.is_empty()).then_some(provider_tools.clone());
                        runtime.provider_tools_override.set(override_value);
                        let result = writer
                            .append(SessionEvent::CapabilitiesSet { provider_tools })
                            .map(|_| ())
                            .map_err(AgentHostError::Session);
                        let _ = reply.send(result);
                    }
                    SessionCommand::SetRoute { route, reply } => {
                        let result = writer
                            .append(SessionEvent::RouteChanged {
                                provider: route.provider.clone(),
                                model: route.model.clone(),
                                reasoning: route.reasoning,
                            })
                            .map_err(AgentHostError::Session)
                            .map(|envelope| {
                                state
                                    .lock()
                                    .expect("session state lock poisoned")
                                    .apply(&envelope);
                                runtime.model_route.set(route);
                            });
                        let _ = reply.send(result);
                    }
                }
            }
            completion = completions.recv() => {
                let Some(completion) = completion else {
                    break;
                };
                running.store(false, Ordering::SeqCst);
                *current_turn.lock().expect("turn lock poisoned") = None;
                cancel = None;
                active_turn = None;
                let _ = completion.result;
                let queued = steer_queue.drain();
                if !queued.is_empty() {
                    for steer in &queued {
                        let _ = writer.append(SessionEvent::SteerDiscarded {
                            steer_id: steer.steer_id,
                        });
                    }
                    let text = queued
                        .into_iter()
                        .map(|message| message.text)
                        .collect::<Vec<_>>()
                        .join("\n");
                    let turn_id = TurnId::new();
                    running.store(true, Ordering::SeqCst);
                    start_turn(
                        &runtime,
                        &writer,
                        &state,
                        &current_turn,
                        &completion_tx,
                        turn_id,
                        text,
                        &mut cancel,
                        &mut active_turn,
                    );
                }
            }
            _ = tokio::time::sleep(idle_timeout), if active_turn.is_none() => {
                break;
            }
        }
    }
    if let Some(actors) = actors.upgrade() {
        actors
            .lock()
            .expect("agent host lock poisoned")
            .remove(&session_id);
    }
}

#[allow(clippy::too_many_arguments)]
fn start_turn(
    runtime: &SessionRuntime,
    writer: &Arc<SessionWriter>,
    state: &Arc<Mutex<SessionState>>,
    current_turn: &Arc<Mutex<Option<TurnId>>>,
    completion_tx: &mpsc::UnboundedSender<TurnCompletion>,
    turn_id: TurnId,
    prompt: String,
    cancel: &mut Option<CancellationToken>,
    active_turn: &mut Option<TurnId>,
) {
    let turn_writer = writer.clone();
    let state_snapshot = state.lock().expect("session state lock poisoned").clone();
    let ctx = runtime.ctx.clone();
    let agent = runtime.agent.clone();
    let completion_tx = completion_tx.clone();
    let token = CancellationToken::new();
    *cancel = Some(token.clone());
    *active_turn = Some(turn_id);
    *current_turn.lock().expect("turn lock poisoned") = Some(turn_id);
    let session_ctx = ctx.clone();
    let _ = session_ctx.scope().spawn(async move {
        let result = tokio::select! {
            result = agent.run_turn(&ctx, &turn_writer, &state_snapshot, &prompt) => {
                result.map(|_| ())
            }
            _ = token.cancelled() => {
                let _ = turn_writer.append(SessionEvent::TurnEnded {
                    reason: TurnEndReason::Interrupted,
                });
                Err(LoopError::Tool("turn cancelled".to_owned()))
            }
        };
        let _ = completion_tx.send(TurnCompletion { result });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::future::BoxFuture;

    #[derive(Default)]
    struct RecordingTransport {
        events: Mutex<Vec<serde_json::Value>>,
    }

    impl OutboundTransport for RecordingTransport {
        fn notify(&self, method: String, params: serde_json::Value) -> Result<(), RpcError> {
            assert_eq!(method, "session.event");
            self.events
                .lock()
                .expect("event lock poisoned")
                .push(params);
            Ok(())
        }

        fn request(
            &self,
            _method: String,
            _params: serde_json::Value,
        ) -> BoxFuture<'static, Result<serde_json::Value, RpcError>> {
            Box::pin(async { Ok(serde_json::Value::Null) })
        }
    }

    #[test]
    fn attach_buffers_live_events_until_release() {
        let transport = Arc::new(RecordingTransport::default());
        let sink = SessionEventSink::new(transport.clone());
        let session_id = SessionId::new();
        sink.begin_attach(session_id);
        sink.publish(
            session_id,
            json!({ "kind": "live", "event": { "type": "one" } }),
        )
        .expect("buffer event");
        assert!(
            transport
                .events
                .lock()
                .expect("event lock poisoned")
                .is_empty()
        );
        let pending = sink.finish_attach(session_id);
        assert_eq!(pending.len(), 1);
        sink.publish(
            session_id,
            json!({ "kind": "live", "event": { "type": "two" } }),
        )
        .expect("publish event");
        assert_eq!(
            transport.events.lock().expect("event lock poisoned").len(),
            1
        );
    }
}

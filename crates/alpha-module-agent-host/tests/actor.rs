use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use alpha_kernel::{Ctx, Module};
use alpha_module_agent_host::{
    AgentHostError, AgentHostService, CreateSessionRequest, SessionRuntime, SessionRuntimeFactory,
    SessionRuntimeRequest,
};
use alpha_module_loop::{
    AgentDriver, LoopError, ModelRoute, ModelRouteOverride, ProviderToolsOverride, TurnOutcome,
};
use alpha_module_permission::{PermissionConfig, PermissionModule, PermissionService};
use alpha_protocol::{OutboundTransport, RpcError};
use alpha_schema::{AssistantContentBlock, MessageId, PermissionMode, ReasoningLevel};
use alpha_session_log::{SessionEvent, SessionState, SessionStore, SessionWriter, TurnEndReason};
use futures::future::BoxFuture;
use serde_json::{Value, json};
use tokio::sync::Notify;

struct EchoDriver;

impl AgentDriver for EchoDriver {
    fn run_turn<'a>(
        &'a self,
        ctx: &'a Ctx,
        writer: &'a SessionWriter,
        _state: &'a SessionState,
        prompt: &'a str,
    ) -> BoxFuture<'a, Result<TurnOutcome, LoopError>> {
        Box::pin(async move {
            writer.append(SessionEvent::TurnStarted)?;
            writer.append(SessionEvent::UserMessage {
                id: MessageId::new(),
                text: prompt.to_owned(),
                images: Vec::new(),
            })?;
            let id = MessageId::new();
            let tools = match ctx
                .get::<ProviderToolsOverride>()
                .and_then(|override_value| override_value.get())
            {
                Some(provider_tools) if !provider_tools.is_empty() => provider_tools.join(","),
                _ => String::new(),
            };
            let route = ctx
                .get::<ModelRouteOverride>()
                .map(|override_value| override_value.get())
                .unwrap_or_default();
            let route_suffix = if route.provider.is_empty() || route.model.is_empty() {
                String::new()
            } else {
                format!(":{}:{}:{}", route.provider, route.model, route.reasoning)
            };
            let mode_suffix = ctx
                .get::<dyn PermissionService>()
                .map(|permission| permission.mode())
                .filter(|mode| *mode != PermissionMode::Auto)
                .map(|mode| format!(":mode={mode}"))
                .unwrap_or_default();
            let tools_suffix = if tools.is_empty() {
                String::new()
            } else {
                format!(":{tools}")
            };
            let text = format!("echo:{prompt}{route_suffix}{mode_suffix}{tools_suffix}");
            writer.append(SessionEvent::AssistantMessage {
                id,
                blocks: vec![AssistantContentBlock::Text { text: text.clone() }],
                text: String::new(),
            })?;
            writer.append(SessionEvent::TurnEnded {
                reason: TurnEndReason::Completed,
            })?;
            Ok(TurnOutcome {
                assistant_message: id,
                assistant_text: text,
            })
        })
    }
}

struct EchoFactory;

impl SessionRuntimeFactory for EchoFactory {
    fn build(
        &self,
        request: SessionRuntimeRequest,
    ) -> Result<SessionRuntime, alpha_module_agent_host::AgentHostError> {
        let ctx = Ctx::root();
        PermissionModule
            .activate(
                &ctx,
                &PermissionConfig {
                    mode: request.mode,
                    ..PermissionConfig::default()
                },
            )
            .expect("permission module");
        let model_route = Arc::new(ModelRouteOverride::default());
        ctx.provide::<ModelRouteOverride>(model_route.clone())
            .expect("model route override");
        let provider_tools_override = Arc::new(ProviderToolsOverride::default());
        provider_tools_override.set(request.provider_tools_override);
        ctx.provide::<ProviderToolsOverride>(provider_tools_override.clone())
            .expect("provider tools override");
        Ok(SessionRuntime {
            ctx,
            agent: Arc::new(EchoDriver),
            provider_tools_override,
            model_route,
        })
    }
}

struct RouteEchoFactory;

impl SessionRuntimeFactory for RouteEchoFactory {
    fn build(
        &self,
        request: SessionRuntimeRequest,
    ) -> Result<SessionRuntime, alpha_module_agent_host::AgentHostError> {
        let ctx = Ctx::root();
        PermissionModule
            .activate(
                &ctx,
                &PermissionConfig {
                    mode: request.mode,
                    ..PermissionConfig::default()
                },
            )
            .expect("permission module");
        let model_route = Arc::new(ModelRouteOverride::new(ModelRoute::new(
            request.provider,
            request.model,
            request.reasoning,
        )));
        ctx.provide::<ModelRouteOverride>(model_route.clone())
            .expect("model route override");
        let provider_tools_override = Arc::new(ProviderToolsOverride::default());
        provider_tools_override.set(request.provider_tools_override);
        ctx.provide::<ProviderToolsOverride>(provider_tools_override.clone())
            .expect("provider tools override");
        Ok(SessionRuntime {
            ctx,
            agent: Arc::new(EchoDriver),
            provider_tools_override,
            model_route,
        })
    }
}

struct BlockingDriver {
    started: Arc<Notify>,
    release: Arc<Notify>,
    runs: AtomicUsize,
}

impl AgentDriver for BlockingDriver {
    fn run_turn<'a>(
        &'a self,
        _ctx: &'a Ctx,
        writer: &'a SessionWriter,
        _state: &'a SessionState,
        prompt: &'a str,
    ) -> BoxFuture<'a, Result<TurnOutcome, LoopError>> {
        Box::pin(async move {
            if self.runs.fetch_add(1, Ordering::SeqCst) == 0 {
                self.started.notify_one();
                self.release.notified().await;
            }
            writer.append(SessionEvent::TurnStarted)?;
            writer.append(SessionEvent::UserMessage {
                id: MessageId::new(),
                text: prompt.to_owned(),
                images: Vec::new(),
            })?;
            let id = MessageId::new();
            let text = format!("echo:{prompt}");
            writer.append(SessionEvent::AssistantMessage {
                id,
                blocks: vec![AssistantContentBlock::Text { text: text.clone() }],
                text: String::new(),
            })?;
            writer.append(SessionEvent::TurnEnded {
                reason: TurnEndReason::Completed,
            })?;
            Ok(TurnOutcome {
                assistant_message: id,
                assistant_text: text,
            })
        })
    }
}

struct BlockingFactory {
    started: Arc<Notify>,
    release: Arc<Notify>,
}

impl SessionRuntimeFactory for BlockingFactory {
    fn build(&self, _request: SessionRuntimeRequest) -> Result<SessionRuntime, AgentHostError> {
        Ok(SessionRuntime {
            ctx: Ctx::root(),
            agent: Arc::new(BlockingDriver {
                started: self.started.clone(),
                release: self.release.clone(),
                runs: AtomicUsize::new(0),
            }),
            provider_tools_override: Arc::new(ProviderToolsOverride::default()),
            model_route: Arc::new(ModelRouteOverride::default()),
        })
    }
}

/// A create request for the fixture session in `dir`.
fn session_request(dir: &std::path::Path, reasoning: ReasoningLevel) -> CreateSessionRequest {
    CreateSessionRequest {
        workspace: dir.to_path_buf(),
        cwd: None,
        mode: PermissionMode::Auto,
        provider: "test".to_owned(),
        model: "test".to_owned(),
        reasoning,
    }
}

/// Wait until at least `min_count` durable notifications match `predicate`.
async fn wait_for_durable(
    transport: &RecordingTransport,
    min_count: usize,
    predicate: impl Fn(&Value) -> bool,
) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let seen = transport
            .notifications
            .lock()
            .expect("notification lock poisoned")
            .iter()
            .filter(|(method, params)| {
                method == "session.event" && params["kind"] == "durable" && predicate(params)
            })
            .count();
        if seen >= min_count {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// Wait until the host dropped every live actor.
async fn wait_for_idle(host: &AgentHostService) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while host.session_count() > 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[derive(Default)]
struct RecordingTransport {
    notifications: Mutex<Vec<(String, Value)>>,
}

impl OutboundTransport for RecordingTransport {
    fn notify(&self, method: String, params: Value) -> Result<(), RpcError> {
        self.notifications
            .lock()
            .expect("notification lock poisoned")
            .push((method, params));
        Ok(())
    }

    fn request(
        &self,
        _method: String,
        _params: Value,
    ) -> BoxFuture<'static, Result<Value, RpcError>> {
        Box::pin(async { Ok(json!({ "decision": "once" })) })
    }
}

#[test]
fn actor_streams_durable_events_and_completes_a_turn() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = SessionStore::new(dir.path());
        let transport = Arc::new(RecordingTransport::default());
        let host = AgentHostService::new(store.clone(), transport.clone(), Arc::new(EchoFactory))
            .with_idle_timeout(Duration::from_millis(20));
        let created = host
            .create_session(session_request(dir.path(), Default::default()))
            .expect("create");
        host.submit(created.session_id, "hello".to_owned())
            .expect("submit");

        assert!(
            wait_for_durable(&transport, 1, |params| params["kind"] == "durable"
                && params["event"]["payload"]["type"] == "turn_ended")
            .await,
            "durable event did not arrive",
        );

        {
            let notifications = transport
                .notifications
                .lock()
                .expect("notification lock poisoned");
            assert!(notifications.iter().any(|(method, params)| {
                method == "session.event"
                    && params["kind"] == "durable"
                    && params["event"]["payload"]["type"] == "assistant_message"
                    && params["event"]["payload"]["blocks"][0]["text"] == "echo:hello"
            }));
        }

        let reader = store.open_read(created.session_id).expect("read");
        assert!(
            reader
                .events()
                .iter()
                .any(|envelope| envelope.payload.assistant_text().as_deref() == Some("echo:hello"))
        );
        drop(reader);
        wait_for_idle(&host).await;
        assert_eq!(host.session_count(), 0);
        let resumed = host
            .resume_session(created.session_id)
            .expect("resume after eviction");
        assert_eq!(resumed.messages, 2);
    });
}

#[test]
fn initial_reasoning_survives_actor_eviction() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = SessionStore::new(dir.path());
        let transport = Arc::new(RecordingTransport::default());
        let host = AgentHostService::new(store.clone(), transport, Arc::new(RouteEchoFactory))
            .with_idle_timeout(Duration::from_millis(20));
        let created = host
            .create_session(session_request(dir.path(), ReasoningLevel::Max))
            .expect("create");
        assert_eq!(created.reasoning, ReasoningLevel::Max);

        let reader = store.open_read(created.session_id).expect("read");
        assert!(reader.events().iter().any(|event| {
            matches!(
                &event.payload,
                SessionEvent::RouteChanged {
                    provider,
                    model,
                    reasoning,
                } if provider == "test" && model == "test" && *reasoning == ReasoningLevel::Max
            )
        }));
        drop(reader);

        wait_for_idle(&host).await;
        assert_eq!(host.session_count(), 0);
        let resumed = host
            .resume_session(created.session_id)
            .expect("resume after eviction");
        assert_eq!(resumed.reasoning, ReasoningLevel::Max);
    });
}

#[test]
fn a_second_host_reports_session_busy() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = SessionStore::new(dir.path());
        let first = AgentHostService::new(
            store.clone(),
            Arc::new(RecordingTransport::default()),
            Arc::new(EchoFactory),
        );
        let created = first
            .create_session(session_request(dir.path(), Default::default()))
            .expect("create");
        let second = AgentHostService::new(
            store,
            Arc::new(RecordingTransport::default()),
            Arc::new(EchoFactory),
        );
        assert!(matches!(
            second.resume_session(created.session_id),
            Err(AgentHostError::SessionBusy(_))
        ));
    });
}

#[test]
fn route_changes_update_the_live_session_projection() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let transport = Arc::new(RecordingTransport::default());
        let host = AgentHostService::new(
            SessionStore::new(dir.path()),
            transport.clone(),
            Arc::new(EchoFactory),
        );
        let created = host
            .create_session(session_request(dir.path(), ReasoningLevel::Auto))
            .expect("create");
        host.set_route(
            created.session_id,
            ModelRoute::new("provider-b", "model-b", ReasoningLevel::Max),
        )
        .await
        .expect("set route");

        let resumed = host
            .resume_session(created.session_id)
            .expect("resume live session");
        assert_eq!(resumed.provider, "provider-b");
        assert_eq!(resumed.model, "model-b");
        assert_eq!(resumed.reasoning, ReasoningLevel::Max);

        host.submit(created.session_id, "hello".to_owned())
            .expect("submit");
        assert!(
            wait_for_durable(&transport, 1, |params| params["kind"] == "durable"
                && params["event"]["payload"]["type"] == "assistant_message"
                && params["event"]["payload"]["blocks"][0]["text"]
                    == "echo:hello:provider-b:model-b:max")
            .await,
            "durable event did not arrive",
        );
    });
}

#[test]
fn mode_changes_survive_actor_resume() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let transport = Arc::new(RecordingTransport::default());
        let host = AgentHostService::new(
            SessionStore::new(dir.path()),
            transport.clone(),
            Arc::new(EchoFactory),
        )
        .with_idle_timeout(Duration::from_millis(20));
        let created = host
            .create_session(session_request(dir.path(), ReasoningLevel::Auto))
            .expect("create");
        host.set_mode(created.session_id, PermissionMode::Free)
            .await
            .expect("set mode");
        assert_eq!(
            host.resume_session(created.session_id)
                .expect("resume before eviction")
                .mode,
            PermissionMode::Free
        );

        wait_for_idle(&host).await;
        assert_eq!(host.session_count(), 0);
        let resumed = host
            .resume_session(created.session_id)
            .expect("resume after eviction");
        assert_eq!(resumed.mode, PermissionMode::Free);

        host.submit(created.session_id, "hello".to_owned())
            .expect("submit");
        assert!(
            wait_for_durable(&transport, 1, |params| params["kind"] == "durable"
                && params["event"]["payload"]["type"] == "assistant_message"
                && params["event"]["payload"]["blocks"][0]["text"]
                    == "echo:hello:mode=free")
            .await,
            "durable event did not arrive",
        );
    });
}

#[test]
fn pending_steers_require_confirmation() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = SessionStore::new(dir.path());
        let transport = Arc::new(RecordingTransport::default());
        let host = AgentHostService::new(store, transport.clone(), Arc::new(EchoFactory));
        let created = host
            .create_session(session_request(dir.path(), Default::default()))
            .expect("create");
        host.steer(created.session_id, "confirmed".to_owned(), Vec::new())
            .await
            .expect("queue steer");
        let discarded = host
            .confirm_steers(created.session_id, false)
            .await
            .expect("discard steer");
        assert_eq!(discarded.len(), 1);
        assert_eq!(discarded[0].text, "confirmed");
        host.steer(created.session_id, "confirmed".to_owned(), Vec::new())
            .await
            .expect("queue steer");
        host.confirm_steers(created.session_id, true)
            .await
            .expect("confirm steer");
        assert!(
            wait_for_durable(&transport, 1, |params| params["kind"] == "durable"
                && params["event"]["payload"]["type"] == "turn_ended")
            .await,
            "durable event did not arrive",
        );
        let notifications = transport
            .notifications
            .lock()
            .expect("notification lock poisoned");
        assert!(notifications.iter().any(|(method, params)| {
            method == "session.event"
                && params["kind"] == "durable"
                && params["event"]["payload"]["type"] == "assistant_message"
                && params["event"]["payload"]["blocks"][0]["text"] == "echo:confirmed"
        }));
    });
}

#[test]
fn pending_steers_survive_actor_restart() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = SessionStore::new(dir.path());
        let first = AgentHostService::new(
            store.clone(),
            Arc::new(RecordingTransport::default()),
            Arc::new(EchoFactory),
        )
        .with_idle_timeout(Duration::from_millis(20));
        let created = first
            .create_session(session_request(dir.path(), Default::default()))
            .expect("create");
        first
            .steer(created.session_id, "pending".to_owned(), Vec::new())
            .await
            .expect("queue steer");
        first
            .set_capabilities(created.session_id, vec!["web_search".to_owned()])
            .await
            .expect("set capabilities");
        wait_for_idle(&first).await;
        assert_eq!(first.session_count(), 0);
        drop(first);

        let transport = Arc::new(RecordingTransport::default());
        let second = AgentHostService::new(store, transport.clone(), Arc::new(EchoFactory));
        second
            .resume_session(created.session_id)
            .expect("resume session");
        second
            .confirm_steers(created.session_id, true)
            .await
            .expect("confirm recovered steer");
        assert!(
            wait_for_durable(&transport, 1, |params| params["kind"] == "durable"
                && params["event"]["payload"]["type"] == "turn_ended")
            .await,
            "durable event did not arrive",
        );
        let notifications = transport
            .notifications
            .lock()
            .expect("notification lock poisoned");
        assert!(notifications.iter().any(|(method, params)| {
            method == "session.event"
                && params["kind"] == "durable"
                && params["event"]["payload"]["type"] == "assistant_message"
                && params["event"]["payload"]["blocks"][0]["text"] == "echo:pending:web_search"
        }));
    });
}

#[test]
fn running_steer_starts_the_next_turn_after_completion() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = SessionStore::new(dir.path());
        let transport = Arc::new(RecordingTransport::default());
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let host = AgentHostService::new(
            store,
            transport.clone(),
            Arc::new(BlockingFactory {
                started: started.clone(),
                release: release.clone(),
            }),
        );
        let created = host
            .create_session(session_request(dir.path(), Default::default()))
            .expect("create");
        host.submit(created.session_id, "first".to_owned())
            .expect("submit");
        started.notified().await;
        host.steer(created.session_id, "steer".to_owned(), Vec::new())
            .await
            .expect("steer");
        release.notify_one();
        assert!(
            wait_for_durable(&transport, 2, |params| params["event"]["payload"]["type"]
                == "turn_ended")
            .await,
            "the steered turn did not run",
        );
        let notifications = transport
            .notifications
            .lock()
            .expect("notification lock poisoned");
        assert!(notifications.iter().any(|(method, params)| {
            method == "session.event"
                && params["kind"] == "durable"
                && params["event"]["payload"]["type"] == "assistant_message"
                && params["event"]["payload"]["blocks"][0]["text"] == "echo:steer"
        }));
    });
}

#[test]
fn active_steer_can_be_discarded_before_injection() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let transport = Arc::new(RecordingTransport::default());
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let host = AgentHostService::new(
            SessionStore::new(dir.path()),
            transport.clone(),
            Arc::new(BlockingFactory {
                started: started.clone(),
                release: release.clone(),
            }),
        );
        let created = host
            .create_session(session_request(dir.path(), Default::default()))
            .expect("create");
        host.submit(created.session_id, "first".to_owned())
            .expect("submit");
        started.notified().await;
        host.steer(created.session_id, "undo me".to_owned(), Vec::new())
            .await
            .expect("steer");
        let discarded = host
            .confirm_steers(created.session_id, false)
            .await
            .expect("discard active steer");
        assert_eq!(discarded.len(), 1);
        assert_eq!(discarded[0].text, "undo me");
        release.notify_one();
        assert!(
            wait_for_durable(&transport, 1, |params| params["kind"] == "durable"
                && params["event"]["payload"]["type"] == "turn_ended")
            .await,
            "durable event did not arrive",
        );
        let notifications = transport
            .notifications
            .lock()
            .expect("notification lock poisoned");
        assert!(notifications.iter().any(|(method, params)| {
            method == "session.event"
                && params["kind"] == "durable"
                && params["event"]["payload"]["type"] == "steer_discarded"
                && params["event"]["payload"]["steer_id"] == discarded[0].steer_id.to_string()
        }));
        assert!(notifications.iter().all(|(method, params)| {
            method != "session.event"
                || params["event"]["payload"]["type"] != "assistant_message"
                || params["event"]["payload"]["blocks"][0]["text"] != "echo:undo me"
        }));
    });
}

#[test]
fn cancel_stops_the_turn_without_replaying_it() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = SessionStore::new(dir.path());
        let transport = Arc::new(RecordingTransport::default());
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let host = AgentHostService::new(
            store,
            transport.clone(),
            Arc::new(BlockingFactory {
                started: started.clone(),
                release: release.clone(),
            }),
        );
        let created = host
            .create_session(session_request(dir.path(), Default::default()))
            .expect("create");
        let turn_id = host
            .submit(created.session_id, "cancel me".to_owned())
            .expect("submit");
        started.notified().await;
        host.cancel(created.session_id, turn_id)
            .await
            .expect("cancel");
        assert!(
            wait_for_durable(&transport, 1, |params| params["kind"] == "durable"
                && params["event"]["payload"]["type"] == "turn_ended"
                && params["event"]["payload"]["reason"] == "interrupted")
            .await,
            "durable event did not arrive",
        );
        let notifications = transport
            .notifications
            .lock()
            .expect("notification lock poisoned");
        assert!(notifications.iter().any(|(method, params)| {
            method == "session.event"
                && params["kind"] == "durable"
                && params["event"]["payload"]["type"] == "turn_ended"
                && params["event"]["payload"]["reason"] == "interrupted"
        }));
        assert!(!notifications.iter().any(|(_, params)| {
            params["kind"] == "durable" && params["event"]["payload"]["type"] == "assistant_message"
        }));
        drop(release);
    });
}

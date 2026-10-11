use std::sync::{Arc, Mutex};
use std::time::Duration;

use alpha_kernel::Ctx;
use alpha_module_agent_host::{
    AgentHostError, AgentHostService, CreateSessionRequest, SessionRuntime, SessionRuntimeFactory,
    SessionRuntimeRequest,
};
use alpha_module_loop::{AgentDriver, LoopError, ProviderToolsOverride, TurnOutcome};
use alpha_module_session::{
    SessionEvent, SessionState, SessionStore, SessionWriter, TurnEndReason,
};
use alpha_protocol::{OutboundTransport, RpcError};
use alpha_schema::{AssistantContentBlock, MessageId, PermissionMode};
use futures::future::BoxFuture;
use serde_json::{Value, json};

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
            })?;
            let id = MessageId::new();
            let text = match ctx
                .get::<ProviderToolsOverride>()
                .and_then(|override_value| override_value.get())
            {
                Some(provider_tools) if !provider_tools.is_empty() => {
                    format!("echo:{prompt}:{}", provider_tools.join(","))
                }
                _ => format!("echo:{prompt}"),
            };
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
        let provider_tools_override = Arc::new(ProviderToolsOverride::default());
        provider_tools_override.set(request.provider_tools_override);
        ctx.provide::<ProviderToolsOverride>(provider_tools_override.clone())
            .expect("provider tools override");
        Ok(SessionRuntime {
            ctx,
            agent: Arc::new(EchoDriver),
            provider_tools_override,
        })
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
            .create_session(CreateSessionRequest {
                workspace: dir.path().to_path_buf(),
                cwd: None,
                mode: PermissionMode::Auto,
                provider: "test".to_owned(),
                model: "test".to_owned(),
            })
            .expect("create");
        host.submit(created.session_id, "hello".to_owned())
            .expect("submit");

        for _ in 0..50 {
            let done = transport
                .notifications
                .lock()
                .expect("notification lock poisoned")
                .iter()
                .any(|(method, params)| {
                    method == "session.event"
                        && params["kind"] == "durable"
                        && params["event"]["payload"]["type"] == "turn_ended"
                });
            if done {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

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
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(host.session_count(), 0);
        let resumed = host
            .resume_session(created.session_id)
            .expect("resume after eviction");
        assert_eq!(resumed.messages, 2);
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
            .create_session(CreateSessionRequest {
                workspace: dir.path().to_path_buf(),
                cwd: None,
                mode: PermissionMode::Auto,
                provider: "test".to_owned(),
                model: "test".to_owned(),
            })
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
            .create_session(CreateSessionRequest {
                workspace: dir.path().to_path_buf(),
                cwd: None,
                mode: PermissionMode::Auto,
                provider: "test".to_owned(),
                model: "test".to_owned(),
            })
            .expect("create");
        host.steer(created.session_id, "confirmed".to_owned())
            .await
            .expect("queue steer");
        assert!(matches!(
            host.confirm_steers(created.session_id, false).await,
            Ok(())
        ));
        host.steer(created.session_id, "confirmed".to_owned())
            .await
            .expect("queue steer");
        host.confirm_steers(created.session_id, true)
            .await
            .expect("confirm steer");
        for _ in 0..50 {
            let done = transport
                .notifications
                .lock()
                .expect("notification lock poisoned")
                .iter()
                .any(|(method, params)| {
                    method == "session.event"
                        && params["kind"] == "durable"
                        && params["event"]["payload"]["type"] == "turn_ended"
                });
            if done {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
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
            .create_session(CreateSessionRequest {
                workspace: dir.path().to_path_buf(),
                cwd: None,
                mode: PermissionMode::Auto,
                provider: "test".to_owned(),
                model: "test".to_owned(),
            })
            .expect("create");
        first
            .steer(created.session_id, "pending".to_owned())
            .await
            .expect("queue steer");
        first
            .set_capabilities(created.session_id, vec!["web_search".to_owned()])
            .await
            .expect("set capabilities");
        tokio::time::sleep(Duration::from_millis(50)).await;
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
        for _ in 0..50 {
            let done = transport
                .notifications
                .lock()
                .expect("notification lock poisoned")
                .iter()
                .any(|(method, params)| {
                    method == "session.event"
                        && params["kind"] == "durable"
                        && params["event"]["payload"]["type"] == "turn_ended"
                });
            if done {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
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

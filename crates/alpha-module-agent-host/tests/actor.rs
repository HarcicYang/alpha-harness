use std::sync::{Arc, Mutex};
use std::time::Duration;

use alpha_kernel::Ctx;
use alpha_module_agent_host::{
    AgentHostService, CreateSessionRequest, SessionRuntime, SessionRuntimeFactory,
    SessionRuntimeRequest,
};
use alpha_module_loop::{AgentDriver, LoopError, TurnOutcome};
use alpha_module_session::{
    SessionEvent, SessionState, SessionStore, SessionWriter, TurnEndReason,
};
use alpha_protocol::{OutboundTransport, RpcError};
use alpha_schema::{MessageId, PermissionMode};
use futures::future::BoxFuture;
use serde_json::{Value, json};

struct EchoDriver;

impl AgentDriver for EchoDriver {
    fn run_turn<'a>(
        &'a self,
        _ctx: &'a Ctx,
        writer: &'a mut SessionWriter,
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
            let text = format!("echo:{prompt}");
            writer.append(SessionEvent::AssistantMessage {
                id,
                text: text.clone(),
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
        _request: SessionRuntimeRequest,
    ) -> Result<SessionRuntime, alpha_module_agent_host::AgentHostError> {
        Ok(SessionRuntime {
            ctx: Ctx::root(),
            agent: Arc::new(EchoDriver),
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
        let host = AgentHostService::new(store.clone(), transport.clone(), Arc::new(EchoFactory));
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

        let notifications = transport
            .notifications
            .lock()
            .expect("notification lock poisoned");
        assert!(notifications.iter().any(|(method, params)| {
            method == "session.event"
                && params["kind"] == "durable"
                && params["event"]["payload"]["type"] == "assistant_message"
                && params["event"]["payload"]["text"] == "echo:hello"
        }));
        drop(notifications);

        let reader = store.open_read(created.session_id).expect("read");
        assert!(reader.events().iter().any(|envelope| {
            matches!(
                &envelope.payload,
                SessionEvent::AssistantMessage { text, .. } if text == "echo:hello"
            )
        }));
    });
}

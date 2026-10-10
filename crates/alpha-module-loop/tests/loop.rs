use std::sync::{Arc, Mutex};

use alpha_kernel::{Ctx, Module};
use alpha_module_loop::{
    AgentLoop, ChatMessage, CompletionRequest, CompletionResponse, LoopConfig, LoopModule,
    ModelProvider, ProviderError, ProviderRegistry,
};
use alpha_module_session::{
    SessionEvent, SessionHeader, SessionState, SessionStore, TurnEndReason, WorkspaceRef,
};
use alpha_schema::{PermissionMode, SessionId};
use futures::future::BoxFuture;

#[derive(Default)]
struct EchoProvider {
    requests: Mutex<Vec<CompletionRequest>>,
}

impl ModelProvider for EchoProvider {
    fn complete(
        &self,
        request: CompletionRequest,
    ) -> BoxFuture<'_, Result<CompletionResponse, ProviderError>> {
        let last_user = request
            .messages
            .iter()
            .rev()
            .find_map(|message| match message {
                ChatMessage::User(text) => Some(text.clone()),
                ChatMessage::Assistant(_) => None,
            })
            .unwrap_or_default();
        self.requests.lock().expect("request lock").push(request);
        Box::pin(async move {
            Ok(CompletionResponse {
                text: format!("echo: {last_user}"),
            })
        })
    }
}

struct FailingProvider;

impl ModelProvider for FailingProvider {
    fn complete(
        &self,
        _request: CompletionRequest,
    ) -> BoxFuture<'_, Result<CompletionResponse, ProviderError>> {
        Box::pin(async move { Err(ProviderError::Request("boom".to_owned())) })
    }
}

fn store_and_header() -> (tempfile::TempDir, SessionStore, SessionHeader) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SessionStore::new(dir.path());
    let header = SessionHeader::new(
        SessionId::new(),
        PermissionMode::Auto,
        WorkspaceRef {
            root: "/tmp/workspace".to_owned(),
            cwd: "/tmp/workspace".to_owned(),
        },
    );
    (dir, store, header)
}

#[tokio::test]
async fn loop_records_completed_turns_and_replays_history() {
    let (_dir, store, header) = store_and_header();
    let id = header.id;
    let provider = Arc::new(EchoProvider::default());
    let agent = AgentLoop::new(provider.clone(), "echo-model");

    let mut writer = store.create(header.clone()).expect("create");
    let state = SessionState::new(&header);
    let first = agent
        .run_turn(&mut writer, &state, "hello")
        .await
        .expect("first turn");
    assert_eq!(first.assistant_text, "echo: hello");
    drop(writer);

    let reader = store.open_read(id).expect("open read");
    let state = reader.state();
    assert_eq!(state.messages.len(), 2);
    assert!(!state.turn_open);
    drop(reader);

    let mut writer = store.open_write(id).expect("open write");
    let second = agent
        .run_turn(&mut writer, &state, "again")
        .await
        .expect("second turn");
    assert_eq!(second.assistant_text, "echo: again");
    drop(writer);

    let requests = provider.requests.lock().expect("request lock");
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1].messages,
        vec![
            ChatMessage::User("hello".to_owned()),
            ChatMessage::Assistant("echo: hello".to_owned()),
            ChatMessage::User("again".to_owned()),
        ]
    );
}

#[tokio::test]
async fn provider_failure_closes_the_turn() {
    let (_dir, store, header) = store_and_header();
    let id = header.id;
    let agent = AgentLoop::new(Arc::new(FailingProvider), "failing-model");

    let mut writer = store.create(header.clone()).expect("create");
    let state = SessionState::new(&header);
    let error = agent
        .run_turn(&mut writer, &state, "hello")
        .await
        .expect_err("provider failure");
    assert!(error.to_string().contains("boom"));
    drop(writer);

    let reader = store.open_read(id).expect("open read");
    let events: Vec<&SessionEvent> = reader
        .events()
        .iter()
        .map(|envelope| &envelope.payload)
        .collect();
    assert!(matches!(events[0], SessionEvent::TurnStarted));
    assert!(matches!(events[1], SessionEvent::UserMessage { .. }));
    assert!(matches!(
        events[2],
        SessionEvent::TurnEnded {
            reason: TurnEndReason::Failed
        }
    ));
    assert!(!reader.state().turn_open);
}

#[test]
fn loop_module_registers_the_agent_loop() {
    let ctx = Ctx::root();
    let registry = Arc::new(ProviderRegistry::new());
    registry
        .register("echo", Arc::new(EchoProvider::default()))
        .expect("register");
    ctx.provide::<ProviderRegistry>(registry).expect("provide");

    LoopModule
        .activate(
            &ctx,
            &LoopConfig {
                provider: "echo".to_owned(),
                model: "echo-model".to_owned(),
            },
        )
        .expect("activate");
    assert_eq!(
        ctx.require::<AgentLoop>().expect("loop").model(),
        "echo-model"
    );
}

#[test]
fn loop_module_rejects_missing_providers() {
    let ctx = Ctx::root();
    let error = LoopModule
        .activate(
            &ctx,
            &LoopConfig {
                provider: "missing".to_owned(),
                model: "echo-model".to_owned(),
            },
        )
        .expect_err("missing provider");
    assert!(error.to_string().contains("missing"));
}

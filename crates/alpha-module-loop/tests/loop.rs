use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use alpha_kernel::{Ctx, Module};
use alpha_module_loop::{
    AgentLoop, ChatMessage, CompletionRequest, CompletionResponse, LoopConfig, LoopModule,
    ModelProvider, ProviderError, ProviderRegistry, ToolCall,
};
use alpha_module_session::{
    SessionEvent, SessionHeader, SessionState, SessionStore, ToolStatus, TurnEndReason,
    WorkspaceRef,
};
use alpha_module_tools::{Tool, ToolContext, ToolError, ToolRegistry, ToolResponse};
use alpha_schema::{PermissionMode, SessionId};
use futures::future::BoxFuture;
use serde_json::{Value, json};

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
                ChatMessage::Assistant(_) | ChatMessage::ToolResult { .. } => None,
            })
            .unwrap_or_default();
        self.requests.lock().expect("request lock").push(request);
        Box::pin(async move {
            Ok(CompletionResponse {
                text: format!("echo: {last_user}"),
                tool_calls: Vec::new(),
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

struct ScriptedProvider {
    responses: Mutex<VecDeque<CompletionResponse>>,
    requests: Mutex<Vec<CompletionRequest>>,
}

impl ScriptedProvider {
    fn new(responses: Vec<CompletionResponse>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().collect()),
            requests: Mutex::new(Vec::new()),
        }
    }
}

impl ModelProvider for ScriptedProvider {
    fn complete(
        &self,
        request: CompletionRequest,
    ) -> BoxFuture<'_, Result<CompletionResponse, ProviderError>> {
        self.requests.lock().expect("request lock").push(request);
        let response = self
            .responses
            .lock()
            .expect("response lock")
            .pop_front()
            .expect("scripted response");
        Box::pin(async move { Ok(response) })
    }
}

struct NoteTool {
    seen: Arc<Mutex<Vec<String>>>,
}

impl Tool for NoteTool {
    fn name(&self) -> &'static str {
        "write_note"
    }

    fn description(&self) -> String {
        "Record a note".to_owned()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "value": { "type": "string" } },
            "required": ["value"]
        })
    }

    fn execute(
        &self,
        input: Value,
        _context: ToolContext,
    ) -> BoxFuture<'_, Result<ToolResponse, ToolError>> {
        let value = input
            .get("value")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let seen = self.seen.clone();
        Box::pin(async move {
            seen.lock().expect("seen lock").push(value.clone());
            Ok(ToolResponse::text(format!("note: {value}")))
        })
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

#[tokio::test]
async fn loop_executes_tool_calls_and_records_durable_events() {
    let (_dir, store, header) = store_and_header();
    let provider = Arc::new(ScriptedProvider::new(vec![
        CompletionResponse {
            text: String::new(),
            tool_calls: vec![ToolCall {
                id: "call-1".to_owned(),
                name: "write_note".to_owned(),
                arguments: json!({ "value": "hi" }),
            }],
        },
        CompletionResponse {
            text: "done".to_owned(),
            tool_calls: Vec::new(),
        },
    ]));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let registry = Arc::new(ToolRegistry::new());
    registry
        .register(Arc::new(NoteTool { seen: seen.clone() }))
        .expect("register");
    let agent = AgentLoop::new(provider.clone(), "scripted").with_tools(registry);

    let mut writer = store.create(header.clone()).expect("create");
    let state = SessionState::new(&header);
    let outcome = agent
        .run_turn(&mut writer, &state, "write a note")
        .await
        .expect("turn");
    assert_eq!(outcome.assistant_text, "done");
    assert_eq!(*seen.lock().expect("seen lock"), vec!["hi".to_owned()]);
    drop(writer);

    let reader = store.open_read(header.id).expect("read");
    let kinds: Vec<&str> = reader
        .events()
        .iter()
        .map(|envelope| match &envelope.payload {
            SessionEvent::TurnStarted => "turn_started",
            SessionEvent::UserMessage { .. } => "user",
            SessionEvent::ToolCall { .. } => "tool_call",
            SessionEvent::ToolResult {
                status: ToolStatus::Completed,
                ..
            } => "tool_result",
            SessionEvent::AssistantMessage { .. } => "assistant",
            SessionEvent::TurnEnded { .. } => "turn_end",
            _ => "other",
        })
        .collect();
    assert_eq!(
        kinds,
        [
            "turn_started",
            "user",
            "tool_call",
            "tool_result",
            "assistant",
            "turn_end"
        ]
    );

    let requests = provider.requests.lock().expect("request lock");
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].tools.len(), 1);
    assert_eq!(requests[0].tools[0].name, "write_note");
    assert!(requests[1].messages.iter().any(|message| matches!(
        message,
        ChatMessage::ToolResult { call_id, content }
            if call_id == "call-1" && content == "note: hi"
    )));
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

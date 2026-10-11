use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use alpha_kernel::{Ctx, Module};
use alpha_module_loop::{
    AgentLoop, AgentStepStarted, AgentToolFinished, AgentToolStarted, ChatMessage, CompletionChunk,
    CompletionRequest, CompletionResponse, ContextError, ContextPreparer, LoopConfig, LoopModule,
    ModelProvider, ProviderError, ProviderRegistry, ProviderToolsOverride, ToolCall, ToolCallDelta,
    TurnOutcome,
};
use alpha_module_session::{
    SessionEvent, SessionHeader, SessionState, SessionStore, SessionWriter, ToolStatus,
    TurnEndReason, WorkspaceRef,
};
use alpha_module_tools::{Tool, ToolContext, ToolError, ToolRegistry, ToolResponse};
use alpha_schema::{MessageId, PermissionMode, Seq, SessionId};
use futures::StreamExt;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
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
                ChatMessage::System(_)
                | ChatMessage::Assistant(_)
                | ChatMessage::AssistantToolCalls { .. }
                | ChatMessage::ToolResult { .. } => None,
            })
            .unwrap_or_default();
        self.requests.lock().expect("request lock").push(request);
        Box::pin(async move {
            Ok(CompletionResponse {
                text: format!("echo: {last_user}"),
                tool_calls: Vec::new(),
                blocks: Vec::new(),
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

struct StreamingProvider {
    streams: Mutex<VecDeque<Vec<Result<CompletionChunk, ProviderError>>>>,
    requests: Mutex<Vec<CompletionRequest>>,
}

impl StreamingProvider {
    fn new(streams: Vec<Vec<Result<CompletionChunk, ProviderError>>>) -> Self {
        Self {
            streams: Mutex::new(streams.into_iter().collect()),
            requests: Mutex::new(Vec::new()),
        }
    }
}

impl ModelProvider for StreamingProvider {
    fn complete(
        &self,
        _request: CompletionRequest,
    ) -> BoxFuture<'_, Result<CompletionResponse, ProviderError>> {
        Box::pin(async move {
            Err(ProviderError::Request(
                "streaming provider does not support complete".to_owned(),
            ))
        })
    }

    fn stream(
        self: Arc<Self>,
        request: CompletionRequest,
    ) -> BoxStream<'static, Result<CompletionChunk, ProviderError>> {
        self.requests.lock().expect("request lock").push(request);
        let chunks = self
            .streams
            .lock()
            .expect("stream lock")
            .pop_front()
            .expect("scripted stream");
        Box::pin(futures::stream::iter(chunks))
    }
}

struct NoteTool {
    seen: Arc<Mutex<Vec<String>>>,
}

struct SummaryPreparer;

impl ContextPreparer for SummaryPreparer {
    fn prepare<'a>(
        &'a self,
        writer: &'a alpha_module_session::SessionWriter,
        state: &'a mut SessionState,
        _provider: &'a dyn ModelProvider,
        _model: &'a str,
        _prompt: &'a str,
    ) -> BoxFuture<'a, Result<(), ContextError>> {
        Box::pin(async move {
            let envelope = writer
                .append(SessionEvent::ContextCompacted {
                    summary: "older history".to_owned(),
                    from_seq: Seq::new(1),
                    through_seq: Seq::new(1),
                })
                .expect("append compaction");
            state.apply(&envelope);
            Ok(())
        })
    }
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

async fn run_turn(
    agent: &AgentLoop,
    writer: &SessionWriter,
    state: &SessionState,
    prompt: &str,
) -> Result<TurnOutcome, alpha_module_loop::LoopError> {
    let ctx = Ctx::root();
    agent.run_turn(&ctx, writer, state, prompt).await
}

#[tokio::test]
async fn loop_records_completed_turns_and_replays_history() {
    let (_dir, store, header) = store_and_header();
    let id = header.id;
    let provider = Arc::new(EchoProvider::default());
    let agent = AgentLoop::new(provider.clone(), "echo-model");

    let writer = store.create(header.clone()).expect("create");
    let state = SessionState::new(&header);
    let first = run_turn(&agent, &writer, &state, "hello")
        .await
        .expect("first turn");
    assert_eq!(first.assistant_text, "echo: hello");
    drop(writer);

    let reader = store.open_read(id).expect("open read");
    let state = reader.state();
    assert_eq!(state.messages.len(), 2);
    assert!(!state.turn_open);
    drop(reader);

    let writer = store.open_write(id).expect("open write");
    let second = run_turn(&agent, &writer, &state, "again")
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
async fn stream_deltas_are_merged_into_one_durable_message() {
    let (_dir, store, header) = store_and_header();
    let provider = Arc::new(StreamingProvider::new(vec![vec![
        Ok(CompletionChunk::Text("hel".to_owned())),
        Ok(CompletionChunk::Text("lo".to_owned())),
    ]]));
    let agent = AgentLoop::new(provider, "streamed");

    let writer = store.create(header.clone()).expect("create");
    let state = SessionState::new(&header);
    let outcome = run_turn(&agent, &writer, &state, "hi").await.expect("turn");
    assert_eq!(outcome.assistant_text, "hello");
    drop(writer);

    let reader = store.open_read(header.id).expect("read");
    let assistant: Vec<String> = reader
        .events()
        .iter()
        .filter_map(|event| event.payload.assistant_text())
        .collect();
    assert_eq!(assistant, vec!["hello".to_owned()]);
}

#[tokio::test]
async fn stream_failure_preserves_the_received_text_prefix() {
    let (_dir, store, header) = store_and_header();
    let provider = Arc::new(StreamingProvider::new(vec![vec![
        Ok(CompletionChunk::Text("partial".to_owned())),
        Err(ProviderError::Request("stream broke".to_owned())),
    ]]));
    let agent = AgentLoop::new(provider, "streamed");

    let writer = store.create(header.clone()).expect("create");
    let state = SessionState::new(&header);
    let error = run_turn(&agent, &writer, &state, "hi")
        .await
        .expect_err("stream failure");
    assert!(error.to_string().contains("stream broke"));
    drop(writer);

    let reader = store.open_read(header.id).expect("read");
    assert!(
        reader
            .events()
            .iter()
            .any(|event| event.payload.assistant_text().as_deref() == Some("partial"))
    );
    assert!(reader.events().iter().any(|event| matches!(
        &event.payload,
        SessionEvent::TurnEnded {
            reason: TurnEndReason::Failed
        }
    )));
}

#[tokio::test]
async fn streamed_tool_call_fragments_are_assembled() {
    let (_dir, store, header) = store_and_header();
    let provider = Arc::new(StreamingProvider::new(vec![
        vec![
            Ok(CompletionChunk::Text("working".to_owned())),
            Ok(CompletionChunk::ToolCall(ToolCallDelta {
                index: 0,
                id: Some("call-stream".to_owned()),
                name: Some("write_note".to_owned()),
                arguments: "{\"value\":\"".to_owned(),
            })),
            Ok(CompletionChunk::ToolCall(ToolCallDelta {
                index: 0,
                id: None,
                name: None,
                arguments: "hi\"}".to_owned(),
            })),
        ],
        vec![Ok(CompletionChunk::Text("done".to_owned()))],
    ]));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let registry = Arc::new(ToolRegistry::new());
    registry
        .register(Arc::new(NoteTool { seen: seen.clone() }))
        .expect("register");
    let agent = AgentLoop::new(provider, "streamed").with_tools(registry);

    let writer = store.create(header.clone()).expect("create");
    let state = SessionState::new(&header);
    let outcome = run_turn(&agent, &writer, &state, "write")
        .await
        .expect("turn");
    assert_eq!(outcome.assistant_text, "done");
    assert_eq!(*seen.lock().expect("seen lock"), vec!["hi".to_owned()]);
}

#[tokio::test]
async fn complete_only_providers_fall_back_to_a_single_chunk_stream() {
    let provider = Arc::new(EchoProvider::default());
    let chunks: Vec<CompletionChunk> = provider
        .stream(CompletionRequest {
            model: "echo".to_owned(),
            messages: vec![ChatMessage::User("hi".to_owned())],
            tools: Vec::new(),
            provider_tools: Vec::new(),
        })
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(Result::unwrap)
        .collect();
    assert_eq!(chunks, vec![CompletionChunk::Text("echo: hi".to_owned())]);
}

#[tokio::test]
async fn session_provider_tool_override_reaches_the_request() {
    let (_dir, store, header) = store_and_header();
    let provider = Arc::new(EchoProvider::default());
    let agent = AgentLoop::new(provider.clone(), "echo");
    let ctx = Ctx::root();
    let override_value = Arc::new(ProviderToolsOverride::default());
    override_value.set(Some(vec!["web_search".to_owned()]));
    ctx.provide::<ProviderToolsOverride>(override_value)
        .expect("provider tools override");
    let writer = store.create(header.clone()).expect("create");
    let state = SessionState::new(&header);
    agent
        .run_turn(&ctx, &writer, &state, "hello")
        .await
        .expect("turn");
    let requests = provider.requests.lock().expect("request lock");
    assert_eq!(requests[0].provider_tools, vec!["web_search".to_owned()]);
}

#[tokio::test]
async fn system_prompt_is_injected_before_history() {
    let (_dir, store, header) = store_and_header();
    let provider = Arc::new(EchoProvider::default());
    let agent = AgentLoop::new(provider.clone(), "echo-model");

    let writer = store.create(header.clone()).expect("create");
    let prompt = writer
        .append(SessionEvent::SystemPromptSet {
            text: "system rules".to_owned(),
        })
        .expect("append prompt");
    let state = SessionState::from_events(&header, &[prompt]);

    run_turn(&agent, &writer, &state, "hello")
        .await
        .expect("turn");

    let requests = provider.requests.lock().expect("request lock");
    assert_eq!(
        requests[0].messages[0],
        ChatMessage::System("system rules".to_owned())
    );
}

#[tokio::test]
async fn context_preparer_updates_the_request_history() {
    let (_dir, store, header) = store_and_header();
    let provider = Arc::new(EchoProvider::default());
    let agent = AgentLoop::new(provider.clone(), "echo-model")
        .with_context_preparer(Arc::new(SummaryPreparer));

    let writer = store.create(header.clone()).expect("create");
    let events = vec![
        writer
            .append(SessionEvent::UserMessage {
                id: MessageId::new(),
                text: "old user".to_owned(),
            })
            .expect("old user"),
        writer
            .append(SessionEvent::AssistantMessage {
                id: MessageId::new(),
                blocks: Vec::new(),
                text: "old assistant".to_owned(),
            })
            .expect("old assistant"),
    ];
    let state = SessionState::from_events(&header, &events);

    run_turn(&agent, &writer, &state, "hello")
        .await
        .expect("turn");

    let requests = provider.requests.lock().expect("request lock");
    assert_eq!(
        requests[0].messages[0],
        ChatMessage::System("# Compacted History Summary\n\nolder history".to_owned())
    );
    assert!(
        requests[0].messages.iter().any(
            |message| matches!(message, ChatMessage::Assistant(text) if text == "old assistant")
        )
    );
    assert!(
        !requests[0]
            .messages
            .iter()
            .any(|message| matches!(message, ChatMessage::User(text) if text == "old user"))
    );
}

#[tokio::test]
async fn provider_failure_closes_the_turn() {
    let (_dir, store, header) = store_and_header();
    let id = header.id;
    let agent = AgentLoop::new(Arc::new(FailingProvider), "failing-model");

    let writer = store.create(header.clone()).expect("create");
    let state = SessionState::new(&header);
    let error = run_turn(&agent, &writer, &state, "hello")
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
        events[3],
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
            blocks: Vec::new(),
        },
        CompletionResponse {
            text: "done".to_owned(),
            tool_calls: Vec::new(),
            blocks: Vec::new(),
        },
    ]));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let registry = Arc::new(ToolRegistry::new());
    registry
        .register(Arc::new(NoteTool { seen: seen.clone() }))
        .expect("register");
    let agent = AgentLoop::new(provider.clone(), "scripted").with_tools(registry);

    let writer = store.create(header.clone()).expect("create");
    let state = SessionState::new(&header);
    let outcome = run_turn(&agent, &writer, &state, "write a note")
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
            SessionEvent::RequestHeader { .. } => "request_header",
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
            "request_header",
            "tool_call",
            "tool_result",
            "request_header",
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

#[tokio::test]
async fn step_limit_wraps_up_without_tools() {
    let (_dir, store, header) = store_and_header();
    let provider = Arc::new(ScriptedProvider::new(vec![
        CompletionResponse {
            text: String::new(),
            tool_calls: vec![ToolCall {
                id: "call-1".to_owned(),
                name: "write_note".to_owned(),
                arguments: json!({ "value": "x" }),
            }],
            blocks: Vec::new(),
        },
        CompletionResponse {
            text: "wrapped up".to_owned(),
            tool_calls: Vec::new(),
            blocks: Vec::new(),
        },
    ]));
    let registry = Arc::new(ToolRegistry::new());
    registry
        .register(Arc::new(NoteTool {
            seen: Arc::new(Mutex::new(Vec::new())),
        }))
        .expect("register");
    let agent = AgentLoop::new(provider.clone(), "scripted")
        .with_tools(registry)
        .with_max_steps(1);

    let writer = store.create(header.clone()).expect("create");
    let state = SessionState::new(&header);
    let outcome = run_turn(&agent, &writer, &state, "do it")
        .await
        .expect("wrap up");
    assert_eq!(outcome.assistant_text, "wrapped up");

    let requests = provider.requests.lock().expect("request lock");
    assert_eq!(requests.len(), 2);
    assert!(requests[1].tools.is_empty());
}

#[tokio::test]
async fn live_events_report_steps_and_tools() {
    let (_dir, store, header) = store_and_header();
    let provider = Arc::new(ScriptedProvider::new(vec![
        CompletionResponse {
            text: String::new(),
            tool_calls: vec![ToolCall {
                id: "call-2".to_owned(),
                name: "write_note".to_owned(),
                arguments: json!({ "value": "y" }),
            }],
            blocks: Vec::new(),
        },
        CompletionResponse {
            text: "done".to_owned(),
            tool_calls: Vec::new(),
            blocks: Vec::new(),
        },
    ]));
    let registry = Arc::new(ToolRegistry::new());
    registry
        .register(Arc::new(NoteTool {
            seen: Arc::new(Mutex::new(Vec::new())),
        }))
        .expect("register");
    let agent = AgentLoop::new(provider, "scripted").with_tools(registry);

    let writer = store.create(header.clone()).expect("create");
    let state = SessionState::new(&header);
    let ctx = Ctx::root();
    let events = Arc::new(Mutex::new(Vec::new()));
    let collected = events.clone();
    ctx.on_parallel::<AgentStepStarted, _>(move |event| {
        collected
            .lock()
            .expect("events lock")
            .push(format!("step:{}", event.step));
        Box::pin(async {})
    })
    .expect("step listener");
    let collected = events.clone();
    ctx.on_parallel::<AgentToolStarted, _>(move |event| {
        collected
            .lock()
            .expect("events lock")
            .push(format!("start:{}", event.name));
        Box::pin(async {})
    })
    .expect("tool start listener");
    let collected = events.clone();
    ctx.on_parallel::<AgentToolFinished, _>(move |event| {
        collected
            .lock()
            .expect("events lock")
            .push(format!("finish:{}:{:?}", event.name, event.status));
        Box::pin(async {})
    })
    .expect("tool finish listener");

    agent
        .run_turn(&ctx, &writer, &state, "do it")
        .await
        .expect("turn");

    let events = events.lock().expect("events lock");
    assert!(events.contains(&"step:1".to_owned()));
    assert!(events.contains(&"start:write_note".to_owned()));
    assert!(
        events
            .iter()
            .any(|event| event.starts_with("finish:write_note:"))
    );
    assert!(events.contains(&"step:2".to_owned()));
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
                ..LoopConfig::default()
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
                ..LoopConfig::default()
            },
        )
        .expect_err("missing provider");
    assert!(error.to_string().contains("missing"));
}

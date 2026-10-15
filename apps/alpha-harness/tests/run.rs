use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use alpha_harness::{
    ResumeOptions, RunOptions, compose_openai_runtime_with_http, compose_with_driver,
    compose_with_provider, resume_session, run_prompt, run_prompt_in,
};
use alpha_kernel::Ctx;
use alpha_module_http::{Cassette, CassetteEntry, HttpClient, ReplayHttpClient};
use alpha_module_loop::{
    AgentDriver, AgentTextDelta, ChatMessage, CompletionChunk, CompletionRequest,
    CompletionResponse, LoopError, ModelProvider, ProviderError, ToolCall, TurnOutcome,
};
use alpha_module_permission::ApprovalChannel;
use alpha_module_session::{
    MessageRecord, SessionEvent, SessionState, SessionStore, SessionWriter, ToolStatus,
    TurnEndReason,
};
use alpha_schema::{EventEnvelope, MessageId, PermissionMode};
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use serde_json::json;

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

struct DeltaProvider;

impl ModelProvider for DeltaProvider {
    fn complete(
        &self,
        _request: CompletionRequest,
    ) -> BoxFuture<'_, Result<CompletionResponse, ProviderError>> {
        Box::pin(async move {
            Ok(CompletionResponse {
                text: "hello".to_owned(),
                tool_calls: Vec::new(),
                blocks: Vec::new(),
            })
        })
    }

    fn stream(
        self: Arc<Self>,
        _request: CompletionRequest,
    ) -> BoxStream<'static, Result<CompletionChunk, ProviderError>> {
        Box::pin(futures::stream::iter(vec![
            Ok(CompletionChunk::Text("hel".to_owned())),
            Ok(CompletionChunk::Text("lo".to_owned())),
        ]))
    }
}

fn write_script() -> Vec<CompletionResponse> {
    vec![
        CompletionResponse {
            text: String::new(),
            tool_calls: vec![ToolCall {
                id: "call-1".to_owned(),
                name: "write".to_owned(),
                arguments: json!({
                    "path": "hello.txt",
                    "content": "hi from tool"
                }),
            }],
            blocks: Vec::new(),
        },
        CompletionResponse {
            text: "done".to_owned(),
            tool_calls: Vec::new(),
            blocks: Vec::new(),
        },
    ]
}

fn decided_sources(events: &[EventEnvelope<SessionEvent>], allowed: bool) -> Vec<String> {
    events
        .iter()
        .filter_map(|envelope| match &envelope.payload {
            SessionEvent::PermissionDecided {
                allowed: value,
                source,
                ..
            } if *value == allowed => Some(source.clone()),
            _ => None,
        })
        .collect()
}

async fn run_script(
    mode: PermissionMode,
    approval: Option<Arc<dyn ApprovalChannel>>,
) -> (
    tempfile::TempDir,
    tempfile::TempDir,
    alpha_harness::RunReport,
    std::sync::Arc<ScriptedProvider>,
) {
    let workspace = tempfile::tempdir().expect("workspace");
    let state = tempfile::tempdir().expect("state");
    let provider = Arc::new(ScriptedProvider::new(write_script()));
    let composition = compose_with_provider(
        workspace.path(),
        mode,
        "scripted",
        "scripted",
        approval,
        provider.clone(),
    )
    .expect("compose");
    let store = SessionStore::new(state.path());
    let report = run_prompt(
        RunOptions {
            workspace: workspace.path().to_path_buf(),
            mode,
            model: "scripted".to_owned(),
            prompt: "write a file".to_owned(),
        },
        &composition,
        &store,
    )
    .await
    .expect("run");
    (workspace, state, report, provider)
}

#[tokio::test]
async fn run_prompt_edits_a_file_end_to_end() {
    let (workspace, state, report, provider) = run_script(PermissionMode::Free, None).await;
    assert_eq!(report.assistant_text, "done");
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("hello.txt")).expect("file"),
        "hi from tool"
    );

    let store = SessionStore::new(state.path());
    let reader = store.open_read(report.session_id).expect("read");
    assert!(reader.events().iter().any(|envelope| matches!(
        &envelope.payload,
        SessionEvent::ToolResult {
            status: ToolStatus::Completed,
            ..
        }
    )));
    assert!(decided_sources(reader.events(), true).contains(&"default".to_owned()));

    let requests = provider.requests.lock().expect("request lock");
    assert!(requests[1].messages.iter().any(|message| matches!(
        message,
        ChatMessage::ToolResult { content, .. }
            if content == "Wrote 12 bytes to hello.txt"
    )));
}

#[tokio::test]
async fn run_prompt_in_streams_text_deltas_through_kernel_events() {
    let workspace = tempfile::tempdir().expect("workspace");
    let state = tempfile::tempdir().expect("state");
    let composition = compose_with_provider(
        workspace.path(),
        PermissionMode::Free,
        "delta",
        "delta",
        None,
        Arc::new(DeltaProvider),
    )
    .expect("compose");
    let ctx = composition.ctx.child().expect("agent context");
    let deltas = Arc::new(Mutex::new(Vec::new()));
    let collected = deltas.clone();
    ctx.on_parallel::<AgentTextDelta, _>(move |event| {
        collected
            .lock()
            .expect("delta lock")
            .push(event.text.clone());
        Box::pin(async {})
    })
    .expect("delta listener");

    let report = run_prompt_in(
        &ctx,
        RunOptions {
            workspace: workspace.path().to_path_buf(),
            mode: PermissionMode::Free,
            model: "delta".to_owned(),
            prompt: "hello".to_owned(),
        },
        &composition,
        &SessionStore::new(state.path()),
    )
    .await
    .expect("run");

    assert_eq!(report.assistant_text, "hello");
    assert_eq!(*deltas.lock().expect("delta lock"), vec!["hel", "lo"]);
}

#[tokio::test]
async fn read_mode_blocks_writes_end_to_end() {
    let (workspace, state, report, _provider) = run_script(PermissionMode::Read, None).await;
    assert_eq!(report.assistant_text, "done");
    assert!(!workspace.path().join("hello.txt").exists());

    let store = SessionStore::new(state.path());
    let reader = store.open_read(report.session_id).expect("read");
    assert!(reader.events().iter().any(|envelope| matches!(
        &envelope.payload,
        SessionEvent::ToolResult {
            status: ToolStatus::Failed,
            content,
            ..
        } if content.contains("permission denied")
    )));
    assert!(decided_sources(reader.events(), false).contains(&"default".to_owned()));
}

#[tokio::test]
async fn auto_mode_allows_workspace_writes_without_approval() {
    let (workspace, state, report, _provider) = run_script(PermissionMode::Auto, None).await;
    assert_eq!(report.assistant_text, "done");
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("hello.txt")).expect("file"),
        "hi from tool"
    );

    let store = SessionStore::new(state.path());
    let reader = store.open_read(report.session_id).expect("read");
    assert!(!decided_sources(reader.events(), true).contains(&"approval".to_owned()));
    assert!(decided_sources(reader.events(), true).contains(&"default".to_owned()));
}

#[tokio::test]
async fn resume_replays_history_and_runs_a_follow_up_turn() {
    let (workspace, state, report, _provider) = run_script(PermissionMode::Free, None).await;
    let store = SessionStore::new(state.path());

    let provider = Arc::new(ScriptedProvider::new(vec![CompletionResponse {
        text: "second".to_owned(),
        tool_calls: Vec::new(),
        blocks: Vec::new(),
    }]));
    let composition = compose_with_provider(
        workspace.path(),
        PermissionMode::Free,
        "scripted",
        "scripted",
        None,
        provider.clone(),
    )
    .expect("compose");

    let idle = resume_session(
        ResumeOptions {
            session_id: report.session_id,
            prompt: None,
        },
        &composition,
        &store,
    )
    .await
    .expect("idle resume");
    assert!(idle.assistant_text.is_none());
    assert_eq!(idle.messages, 5);
    assert_eq!(idle.repairs, 0);

    let resumed = resume_session(
        ResumeOptions {
            session_id: report.session_id,
            prompt: Some("say hi again".to_owned()),
        },
        &composition,
        &store,
    )
    .await
    .expect("resume");
    assert_eq!(resumed.assistant_text.as_deref(), Some("second"));
    assert_eq!(resumed.messages, 7);
    assert!(!resumed.turn_open);

    let requests = provider.requests.lock().expect("request lock");
    assert!(requests[0].messages.iter().any(|message| matches!(
        message,
        ChatMessage::AssistantContent(blocks)
            if blocks.iter().any(|block| matches!(
                block,
                alpha_schema::AssistantContentBlock::LocalToolCall { name, .. }
                    if name == "write"
            ))
    )));
    assert!(requests[0].messages.iter().any(|message| matches!(
        message,
        ChatMessage::ToolResult { content, .. } if content.contains("Wrote 12 bytes")
    )));
    assert!(requests[0].messages.iter().any(|message| matches!(
        message,
        ChatMessage::AssistantContent(blocks)
            if blocks.iter().any(|block| matches!(
                block,
                alpha_schema::AssistantContentBlock::Text { text } if text == "done"
            ))
    )));
}

#[tokio::test]
async fn cassette_replay_reproduces_the_same_session_state() {
    let workspace = tempfile::tempdir().expect("workspace");
    let state = tempfile::tempdir().expect("state");
    let store = SessionStore::new(state.path());

    let cassette = || Cassette {
        entries: vec![CassetteEntry {
            method: "POST".to_owned(),
            url: "https://api.test/v1/chat/completions".to_owned(),
            request_body: Vec::new(),
            status: 200,
            headers: Default::default(),
            response_body: serde_json::to_vec(&json!({
                "choices": [{ "message": { "content": "cassette text" } }]
            }))
            .expect("body"),
        }],
    };

    let mut runs = Vec::new();
    for _ in 0..2 {
        let http: Arc<dyn HttpClient> = Arc::new(ReplayHttpClient::new(cassette()));
        let composition = compose_openai_runtime_with_http(
            workspace.path(),
            PermissionMode::Free,
            "cassette-model",
            "https://api.test/v1",
            "key",
            None,
            http,
        )
        .expect("compose");
        let report = run_prompt(
            RunOptions {
                workspace: workspace.path().to_path_buf(),
                mode: PermissionMode::Free,
                model: "cassette-model".to_owned(),
                prompt: "hello".to_owned(),
            },
            &composition,
            &store,
        )
        .await
        .expect("run");
        let reader = store.open_read(report.session_id).expect("read");
        let messages: Vec<String> = reader
            .state()
            .messages
            .iter()
            .map(|record| match record {
                MessageRecord::User { text, .. } => format!("user:{text}"),
                MessageRecord::Assistant { text, .. } => format!("assistant:{text}"),
                _ => "other".to_owned(),
            })
            .collect();
        runs.push(messages);
    }

    assert_eq!(runs[0], runs[1]);
    assert_eq!(
        runs[0],
        vec![
            "user:hello".to_owned(),
            "assistant:cassette text".to_owned()
        ]
    );
}

struct FixedDriver;

impl AgentDriver for FixedDriver {
    fn run_turn<'a>(
        &'a self,
        _ctx: &'a Ctx,
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
            let assistant = MessageId::new();
            writer.append(SessionEvent::AssistantMessage {
                id: assistant,
                blocks: Vec::new(),
                text: "fixed driver".to_owned(),
            })?;
            writer.append(SessionEvent::TurnEnded {
                reason: TurnEndReason::Completed,
            })?;
            Ok(TurnOutcome {
                assistant_message: assistant,
                assistant_text: "fixed driver".to_owned(),
            })
        })
    }
}

#[tokio::test]
async fn custom_drivers_can_replace_the_loop() {
    let workspace = tempfile::tempdir().expect("workspace");
    let state = tempfile::tempdir().expect("state");
    let store = SessionStore::new(state.path());
    let composition = compose_with_driver(
        workspace.path(),
        PermissionMode::Free,
        None,
        Arc::new(FixedDriver),
    )
    .expect("compose");
    let report = run_prompt(
        RunOptions {
            workspace: workspace.path().to_path_buf(),
            mode: PermissionMode::Free,
            model: "unused".to_owned(),
            prompt: "hello".to_owned(),
        },
        &composition,
        &store,
    )
    .await
    .expect("run");

    assert_eq!(report.assistant_text, "fixed driver");
    let reader = store.open_read(report.session_id).expect("read");
    assert!(
        reader
            .events()
            .iter()
            .any(|envelope| envelope.payload.assistant_text().as_deref() == Some("fixed driver"))
    );
}

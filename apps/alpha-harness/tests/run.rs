use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use alpha_harness::{ResumeOptions, RunOptions, compose_with_provider, resume_session, run_prompt};
use alpha_module_loop::{
    ChatMessage, CompletionRequest, CompletionResponse, ModelProvider, ProviderError, ToolCall,
};
use alpha_module_permission::{ApprovalChannel, ApprovalDecision, ApprovalPrompt};
use alpha_module_session::{SessionEvent, SessionStore, ToolStatus};
use alpha_schema::{EventEnvelope, PermissionMode};
use futures::future::BoxFuture;
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

struct AllowOnce;

impl ApprovalChannel for AllowOnce {
    fn decide(&self, _prompt: ApprovalPrompt) -> BoxFuture<'_, ApprovalDecision> {
        Box::pin(async move { ApprovalDecision::AllowOnce })
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
        },
        CompletionResponse {
            text: "done".to_owned(),
            tool_calls: Vec::new(),
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
async fn auto_mode_uses_the_approval_channel() {
    let (workspace, state, report, _provider) =
        run_script(PermissionMode::Auto, Some(Arc::new(AllowOnce))).await;
    assert_eq!(report.assistant_text, "done");
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("hello.txt")).expect("file"),
        "hi from tool"
    );

    let store = SessionStore::new(state.path());
    let reader = store.open_read(report.session_id).expect("read");
    assert!(decided_sources(reader.events(), true).contains(&"approval".to_owned()));
}

#[tokio::test]
async fn auto_mode_without_a_channel_fails_closed() {
    let (workspace, state, report, _provider) = run_script(PermissionMode::Auto, None).await;
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
        } if content.contains("no approval channel")
    )));
    assert!(decided_sources(reader.events(), false).contains(&"no_channel".to_owned()));
}

#[tokio::test]
async fn resume_replays_history_and_runs_a_follow_up_turn() {
    let (workspace, state, report, _provider) = run_script(PermissionMode::Free, None).await;
    let store = SessionStore::new(state.path());

    let provider = Arc::new(ScriptedProvider::new(vec![CompletionResponse {
        text: "second".to_owned(),
        tool_calls: Vec::new(),
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
    assert_eq!(idle.messages, 4);
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
    assert_eq!(resumed.messages, 6);
    assert!(!resumed.turn_open);

    let requests = provider.requests.lock().expect("request lock");
    assert!(requests[0].messages.iter().any(|message| matches!(
        message,
        ChatMessage::AssistantToolCalls { calls, .. }
            if calls.iter().any(|call| call.name == "write")
    )));
    assert!(requests[0].messages.iter().any(|message| matches!(
        message,
        ChatMessage::ToolResult { content, .. } if content.contains("Wrote 12 bytes")
    )));
    assert!(
        requests[0]
            .messages
            .iter()
            .any(|message| matches!(message, ChatMessage::Assistant(text) if text == "done"))
    );
}

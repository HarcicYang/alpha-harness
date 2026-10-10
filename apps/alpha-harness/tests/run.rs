use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use alpha_harness::{RunOptions, compose_with_provider, run_prompt};
use alpha_module_loop::{
    ChatMessage, CompletionRequest, CompletionResponse, ModelProvider, ProviderError, ToolCall,
};
use alpha_module_session::{SessionEvent, SessionStore, ToolStatus};
use alpha_schema::PermissionMode;
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

#[tokio::test]
async fn run_prompt_edits_a_file_end_to_end() {
    let workspace = tempfile::tempdir().expect("workspace");
    let state = tempfile::tempdir().expect("state");
    let provider = Arc::new(ScriptedProvider::new(write_script()));
    let (_ctx, agent) = compose_with_provider(
        workspace.path(),
        PermissionMode::Free,
        "scripted",
        "scripted",
        provider.clone(),
    )
    .expect("compose");
    let store = SessionStore::new(state.path());

    let report = run_prompt(
        RunOptions {
            workspace: workspace.path().to_path_buf(),
            mode: PermissionMode::Free,
            model: "scripted".to_owned(),
            prompt: "write a file".to_owned(),
        },
        &agent,
        &store,
    )
    .await
    .expect("run");

    assert_eq!(report.assistant_text, "done");
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("hello.txt")).expect("file"),
        "hi from tool"
    );

    let reader = store.open_read(report.session_id).expect("read");
    assert!(reader.events().iter().any(|envelope| matches!(
        &envelope.payload,
        SessionEvent::ToolResult {
            status: ToolStatus::Completed,
            ..
        }
    )));

    let requests = provider.requests.lock().expect("request lock");
    assert!(requests[1].messages.iter().any(|message| matches!(
        message,
        ChatMessage::ToolResult { content, .. }
            if content == "Wrote 12 bytes to hello.txt"
    )));
}

#[tokio::test]
async fn read_mode_blocks_writes_end_to_end() {
    let workspace = tempfile::tempdir().expect("workspace");
    let state = tempfile::tempdir().expect("state");
    let provider = Arc::new(ScriptedProvider::new(write_script()));
    let (_ctx, agent) = compose_with_provider(
        workspace.path(),
        PermissionMode::Read,
        "scripted",
        "scripted",
        provider,
    )
    .expect("compose");
    let store = SessionStore::new(state.path());

    let report = run_prompt(
        RunOptions {
            workspace: workspace.path().to_path_buf(),
            mode: PermissionMode::Read,
            model: "scripted".to_owned(),
            prompt: "write a file".to_owned(),
        },
        &agent,
        &store,
    )
    .await
    .expect("run");

    assert_eq!(report.assistant_text, "done");
    assert!(!workspace.path().join("hello.txt").exists());

    let reader = store.open_read(report.session_id).expect("read");
    assert!(reader.events().iter().any(|envelope| matches!(
        &envelope.payload,
        SessionEvent::ToolResult {
            status: ToolStatus::Failed,
            content,
            ..
        } if content.contains("permission denied")
    )));
}

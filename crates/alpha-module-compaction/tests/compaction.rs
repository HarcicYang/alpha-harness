use std::sync::{Arc, Mutex};

use alpha_kernel::{Ctx, Module};
use alpha_module_compaction::{
    CompactionConfig, CompactionModule, ConservativeTokenEstimator, DefaultContextCompactor,
    TokenEstimator,
};
use alpha_module_loop::{
    CompletionRequest, CompletionResponse, ContextPreparer, ModelProvider, ProviderError,
};
use alpha_module_session::{SessionEvent, SessionHeader, SessionState, SessionStore, WorkspaceRef};
use alpha_schema::{MessageId, PermissionMode, SessionId};
use futures::future::BoxFuture;

struct SummaryProvider {
    text: String,
    fail: bool,
    requests: Mutex<Vec<CompletionRequest>>,
}

impl SummaryProvider {
    fn new(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            fail: false,
            requests: Mutex::new(Vec::new()),
        }
    }

    fn failing() -> Self {
        Self {
            text: String::new(),
            fail: true,
            requests: Mutex::new(Vec::new()),
        }
    }
}

impl ModelProvider for SummaryProvider {
    fn complete(
        &self,
        request: CompletionRequest,
    ) -> BoxFuture<'_, Result<CompletionResponse, ProviderError>> {
        self.requests.lock().expect("request lock").push(request);
        let fail = self.fail;
        let text = self.text.clone();
        Box::pin(async move {
            if fail {
                Err(ProviderError::Request("summary failed".to_owned()))
            } else {
                Ok(CompletionResponse {
                    text,
                    tool_calls: Vec::new(),
                    blocks: Vec::new(),
                })
            }
        })
    }
}

fn header() -> SessionHeader {
    SessionHeader::new(
        SessionId::new(),
        PermissionMode::Auto,
        WorkspaceRef {
            root: "/workspace".to_owned(),
            cwd: "/workspace".to_owned(),
        },
    )
}

fn append_history(writer: &alpha_module_session::SessionWriter) -> SessionState {
    let header = writer.header().clone();
    let events = vec![
        writer
            .append(SessionEvent::UserMessage {
                id: MessageId::new(),
                text: "a".repeat(200),
            })
            .expect("first user"),
        writer
            .append(SessionEvent::AssistantMessage {
                id: MessageId::new(),
                blocks: Vec::new(),
                text: "b".repeat(200),
            })
            .expect("first assistant"),
        writer
            .append(SessionEvent::UserMessage {
                id: MessageId::new(),
                text: "c".repeat(200),
            })
            .expect("second user"),
    ];
    SessionState::from_events(&header, &events)
}

fn config() -> CompactionConfig {
    CompactionConfig {
        context_window_tokens: 100,
        threshold_percent: 50,
        keep_recent_percent: 10,
        max_summary_input_tokens: 100,
    }
}

#[tokio::test]
async fn compaction_replaces_a_complete_prefix() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SessionStore::new(dir.path());
    let header = header();
    let writer = store.create(header).expect("create");
    let mut state = append_history(&writer);
    let provider = SummaryProvider::new("compressed history");
    let compactor = DefaultContextCompactor::new(config(), Arc::new(ConservativeTokenEstimator));

    compactor
        .prepare(
            &writer,
            &mut state,
            &provider,
            "summary-model",
            &"d".repeat(200),
        )
        .await
        .expect("prepare");

    let compaction = state.compaction.expect("compaction");
    assert_eq!(compaction.summary, "compressed history");
    assert_eq!(compaction.from_seq.get(), 1);
    assert_eq!(compaction.through_seq.get(), 2);
    assert!(!provider.requests.lock().expect("requests").is_empty());
}

#[tokio::test]
async fn compaction_does_not_run_below_the_threshold() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SessionStore::new(dir.path());
    let header = header();
    let writer = store.create(header).expect("create");
    let mut state = SessionState::new(writer.header());
    let provider = SummaryProvider::new("unused");
    let compactor = DefaultContextCompactor::new(config(), Arc::new(ConservativeTokenEstimator));

    compactor
        .prepare(&writer, &mut state, &provider, "summary-model", "small")
        .await
        .expect("prepare");

    assert!(state.compaction.is_none());
    assert_eq!(writer.last_seq().get(), 0);
    assert!(provider.requests.lock().expect("requests").is_empty());
}

#[tokio::test]
async fn failed_summarization_leaves_history_untouched() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SessionStore::new(dir.path());
    let header = header();
    let id = header.id;
    let writer = store.create(header).expect("create");
    let mut state = append_history(&writer);
    let provider = SummaryProvider::failing();
    let compactor = DefaultContextCompactor::new(config(), Arc::new(ConservativeTokenEstimator));

    let error = compactor
        .prepare(
            &writer,
            &mut state,
            &provider,
            "summary-model",
            &"d".repeat(200),
        )
        .await
        .expect_err("summary failure");
    assert!(error.to_string().contains("summary failed"));
    assert!(state.compaction.is_none());
    assert_eq!(writer.last_seq().get(), 3);
    drop(writer);

    let reader = store.open_read(id).expect("read");
    assert!(
        !reader
            .events()
            .iter()
            .any(|event| matches!(event.payload, SessionEvent::ContextCompacted { .. }))
    );
}

#[test]
fn conservative_estimator_counts_ascii_and_non_ascii() {
    let estimator = ConservativeTokenEstimator;
    assert_eq!(estimator.estimate("abcdefgh"), 2);
    assert_eq!(estimator.estimate("历史压缩"), 4);
    assert_eq!(estimator.estimate("ab历史"), 3);
}

#[test]
fn module_registers_and_unregisters_compaction() {
    let ctx = Ctx::root();
    CompactionModule
        .activate(&ctx, &config())
        .expect("activate");
    assert!(ctx.get::<dyn ContextPreparer>().is_some());
    assert!(ctx.get::<dyn TokenEstimator>().is_some());
    ctx.dispose();
    assert!(ctx.get::<dyn ContextPreparer>().is_none());
}

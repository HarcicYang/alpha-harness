use std::sync::{Arc, Mutex};

use alpha_kernel::{Ctx, Module};
use alpha_module_compaction::{
    CompactionConfig, CompactionModule, CompactionScope, ConservativeTokenEstimator,
    DefaultContextCompactor, TokenEstimator,
};
use alpha_module_loop::{
    CompletionRequest, CompletionResponse, ContextPreparer, ModelProvider, ProviderError,
};
use alpha_schema::{MessageId, PermissionMode, SessionId};
use alpha_session_log::{SessionEvent, SessionHeader, SessionState, SessionStore, WorkspaceRef};
use futures::future::BoxFuture;

struct SummaryProvider {
    text: String,
    fail: bool,
    context_window: Option<usize>,
    requests: Mutex<Vec<CompletionRequest>>,
}

impl SummaryProvider {
    fn new(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            fail: false,
            context_window: None,
            requests: Mutex::new(Vec::new()),
        }
    }

    fn failing() -> Self {
        Self {
            text: String::new(),
            fail: true,
            context_window: None,
            requests: Mutex::new(Vec::new()),
        }
    }

    fn with_context_window(mut self, context_window: usize) -> Self {
        self.context_window = Some(context_window);
        self
    }
}

impl ModelProvider for SummaryProvider {
    fn context_window(&self, _model: &str) -> Option<usize> {
        self.context_window
    }

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

fn append_history(writer: &alpha_session_log::SessionWriter) -> SessionState {
    let header = writer.header().clone();
    let events = vec![
        writer
            .append(SessionEvent::UserMessage {
                id: MessageId::new(),
                text: "a".repeat(200),
                images: Vec::new(),
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
                images: Vec::new(),
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
        scope: CompactionScope::Total,
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

    let ctx = Ctx::root();
    let progress = Arc::new(Mutex::new(Vec::new()));
    let collected = progress.clone();
    ctx.on_parallel::<alpha_module_loop::AgentCompactionProgress, _>(move |event| {
        collected
            .lock()
            .expect("progress lock")
            .push((event.done, event.total));
        Box::pin(async {})
    })
    .expect("progress listener");
    compactor
        .prepare(
            &ctx,
            &writer,
            &mut state,
            &provider,
            "summary-model",
            &"d".repeat(200),
        )
        .await
        .expect("prepare");

    let progress = progress.lock().expect("progress lock").clone();
    assert_eq!(progress.first(), Some(&(0, progress[0].1)));
    assert!(
        progress
            .iter()
            .all(|(_, total)| *total == progress.len() as u64)
    );
    let compaction = state.compaction.expect("compaction");
    assert!(
        compaction.summary.starts_with("compressed history"),
        "{}",
        compaction.summary
    );
    // The model keeps a durable pointer to the history that was replaced.
    assert!(compaction.summary.contains("still on disk at"));
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

    let ctx = Ctx::root();
    compactor
        .prepare(
            &ctx,
            &writer,
            &mut state,
            &provider,
            "summary-model",
            "small",
        )
        .await
        .expect("prepare");

    assert!(state.compaction.is_none());
    assert_eq!(writer.last_seq().get(), 0);
    assert!(provider.requests.lock().expect("requests").is_empty());
}

#[tokio::test]
async fn failed_summarization_asks_for_a_new_session() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SessionStore::new(dir.path());
    let header = header();
    let id = header.id;
    let writer = store.create(header).expect("create");
    let mut state = append_history(&writer);
    let provider = SummaryProvider::failing();
    let compactor = DefaultContextCompactor::new(config(), Arc::new(ConservativeTokenEstimator));

    let ctx = Ctx::root();
    let error = compactor
        .prepare(
            &ctx,
            &writer,
            &mut state,
            &provider,
            "summary-model",
            &"d".repeat(200),
        )
        .await
        .expect_err("overflow");
    assert!(error.requires_new_session());
    assert!(state.compaction.is_none());
    let last_seq = writer.last_seq().get();
    drop(writer);

    let reader = store.open_read(id).expect("read");
    assert_eq!(reader.events().len() as u64, last_seq);
    assert!(
        !reader
            .events()
            .iter()
            .any(|event| matches!(event.payload, SessionEvent::ContextCompacted { .. }))
    );
}

#[tokio::test]
async fn model_context_window_takes_precedence_over_the_fallback() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SessionStore::new(dir.path());
    let header = header();
    let writer = store.create(header).expect("create");
    let mut state = append_history(&writer);
    let provider = SummaryProvider::new("unused").with_context_window(1_000);
    let compactor = DefaultContextCompactor::new(config(), Arc::new(ConservativeTokenEstimator));

    let ctx = Ctx::root();
    compactor
        .prepare(
            &ctx,
            &writer,
            &mut state,
            &provider,
            "summary-model",
            &"d".repeat(200),
        )
        .await
        .expect("prepare");

    assert!(state.compaction.is_none());
    assert!(provider.requests.lock().expect("requests").is_empty());
}

#[tokio::test]
async fn body_after_prefix_scope_ignores_the_carried_prefix() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SessionStore::new(dir.path());
    let header = header();
    let writer = store.create(header).expect("create");
    let mut state = append_history(&writer);
    state.system_prompt = Some("s".repeat(4_000));
    let provider = SummaryProvider::new("unused").with_context_window(2_000);
    let mut compact = config();
    compact.scope = CompactionScope::BodyAfterPrefix;
    let compactor = DefaultContextCompactor::new(compact, Arc::new(ConservativeTokenEstimator));

    let ctx = Ctx::root();
    compactor
        .prepare(
            &ctx,
            &writer,
            &mut state,
            &provider,
            "summary-model",
            &"d".repeat(200),
        )
        .await
        .expect("prepare");

    assert!(state.compaction.is_none());
    assert!(provider.requests.lock().expect("requests").is_empty());
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

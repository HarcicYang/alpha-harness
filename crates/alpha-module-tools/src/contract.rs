//! Tool contract: results, errors, and execution context.

use alpha_schema::{ImageAttachment, MessageId, SessionId, ToolCallId};
use futures::future::BoxFuture;
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

/// Durable identity and cancellation for one tool call.
#[derive(Clone, Debug)]
pub struct ToolContext {
    /// Session identity.
    pub session_id: SessionId,
    /// Assistant message containing the call.
    pub message_id: MessageId,
    /// Call identity.
    pub call_id: ToolCallId,
    /// Cancellation token for the call.
    pub cancellation: CancellationToken,
    output: Option<UnboundedSender<String>>,
}

impl ToolContext {
    /// Create a context for one call.
    pub fn new(session_id: SessionId, message_id: MessageId, call_id: ToolCallId) -> Self {
        Self {
            session_id,
            message_id,
            call_id,
            cancellation: CancellationToken::new(),
            output: None,
        }
    }

    /// Attach the stream that receives user-visible tool output.
    pub fn with_output_sink(mut self, output: UnboundedSender<String>) -> Self {
        self.output = Some(output);
        self
    }

    /// Emit an output delta for the running tool.
    pub fn emit_output(&self, text: impl Into<String>) {
        if let Some(output) = &self.output {
            let _ = output.send(text.into());
        }
    }
}

/// Three-value tool response.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolResponse {
    /// Model-visible content. Must be non-empty.
    pub content: String,
    /// Optional typed machine value.
    pub output: Option<Value>,
    /// Optional UI metadata.
    pub metadata: Option<Value>,
    /// Images returned to the model alongside the text.
    pub images: Vec<ImageAttachment>,
}

impl ToolResponse {
    /// Build a text-only response.
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            output: None,
            metadata: None,
            images: Vec::new(),
        }
    }

    /// Attach a machine value.
    pub fn with_output(mut self, output: Value) -> Self {
        self.output = Some(output);
        self
    }

    /// Attach UI metadata.
    pub fn with_metadata(mut self, metadata: Value) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// Attach images to the model-visible result.
    pub fn with_images(mut self, images: Vec<ImageAttachment>) -> Self {
        self.images = images;
        self
    }
}

/// Tool failures.
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    /// Expected, model-visible failure.
    #[error("{0}")]
    Failure(String),
    /// Operational failure that is not a model-visible tool result.
    #[error("operational error: {0}")]
    Operational(String),
}

impl ToolError {
    /// Build a model-visible failure.
    pub fn failure(message: impl Into<String>) -> Self {
        Self::Failure(message.into())
    }

    /// Build an operational failure.
    pub fn operational(message: impl Into<String>) -> Self {
        Self::Operational(message.into())
    }
}

/// One executable tool.
pub trait Tool: Send + Sync + 'static {
    /// Stable tool name.
    fn name(&self) -> &'static str;
    /// Model-facing description. Dynamic so tools can list available options.
    fn description(&self) -> String;
    /// JSON schema for the model-facing input.
    fn input_schema(&self) -> Value;
    /// Whether the tool may run in parallel with other parallel-safe calls.
    fn parallel_safe(&self) -> bool {
        false
    }
    /// Prepare model-facing metadata before a schema is advertised.
    fn prepare(&self) -> BoxFuture<'_, Result<(), ToolError>> {
        Box::pin(async { Ok(()) })
    }
    /// Execute one call.
    fn execute(
        &self,
        input: Value,
        context: ToolContext,
    ) -> BoxFuture<'_, Result<ToolResponse, ToolError>>;
}

pub(crate) fn truncate_head_tail(text: &str, max_bytes: usize) -> (String, bool) {
    if text.len() <= max_bytes {
        return (text.to_owned(), false);
    }
    let head_budget = max_bytes * 3 / 5;
    let tail_budget = max_bytes - head_budget;
    let head_end = floor_char_boundary(text, head_budget);
    let tail_start = ceil_char_boundary(text, text.len() - tail_budget);
    let omitted = tail_start - head_end;
    (
        format!(
            "{}\n... [{omitted} bytes omitted] ...\n{}",
            &text[..head_end],
            &text[tail_start..]
        ),
        true,
    )
}

fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_char_boundary(text: &str, mut index: usize) -> usize {
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

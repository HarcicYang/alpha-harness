//! Default conservative token estimator and compaction implementation.

use alpha_module_loop::{
    ChatMessage, CompletionRequest, ContextError, ContextPreparer, ModelProvider,
};
use alpha_session_log::{MessageRecord, SessionEvent, SessionState, SessionWriter, ToolStatus};
use futures::future::BoxFuture;

use crate::{CompactionConfig, CompactionScope};

const HARD_LIMIT_PERCENT: usize = 95;

/// Estimates context size without a model-specific tokenizer.
pub trait TokenEstimator: Send + Sync + 'static {
    /// Estimate the number of tokens in one text fragment.
    fn estimate(&self, text: &str) -> usize;
}

/// Conservative estimator: one token per non-ASCII character and one token per
/// four ASCII bytes.
#[derive(Clone, Copy, Debug, Default)]
pub struct ConservativeTokenEstimator;

impl TokenEstimator for ConservativeTokenEstimator {
    fn estimate(&self, text: &str) -> usize {
        let mut ascii = 0usize;
        let mut non_ascii = 0usize;
        for character in text.chars() {
            if character.is_ascii() {
                ascii += 1;
            } else {
                non_ascii += 1;
            }
        }
        ascii.div_ceil(4) + non_ascii
    }
}

/// Default compactor used by the built-in module.
pub struct DefaultContextCompactor {
    config: CompactionConfig,
    estimator: std::sync::Arc<dyn TokenEstimator>,
}

impl DefaultContextCompactor {
    /// Create a compactor with an explicit estimator.
    pub fn new(config: CompactionConfig, estimator: std::sync::Arc<dyn TokenEstimator>) -> Self {
        Self { config, estimator }
    }

    fn estimate_state(&self, state: &SessionState, next_prompt: &str) -> usize {
        let mut total = self.estimator.estimate(next_prompt);
        if let Some(prompt) = &state.system_prompt {
            total += self.estimator.estimate(prompt);
        }
        if let Some(compaction) = &state.compaction {
            total += self.estimator.estimate(&compaction.summary);
        }
        for record in effective_records(state) {
            total += self.estimate_record(record);
        }
        total
    }

    fn estimate_prefix(&self, state: &SessionState) -> usize {
        let mut total = 0usize;
        if let Some(prompt) = &state.system_prompt {
            total += self.estimator.estimate(prompt);
        }
        if let Some(compaction) = &state.compaction {
            total += self.estimator.estimate(&compaction.summary);
        }
        total
    }

    fn estimate_record(&self, record: &MessageRecord) -> usize {
        match record {
            MessageRecord::User { text, .. }
            | MessageRecord::Context { text, .. }
            | MessageRecord::Assistant { text, .. } => self.estimator.estimate(text),
            MessageRecord::ToolCall { name, input, .. } => {
                self.estimator.estimate(name) + self.estimator.estimate(&input.to_string())
            }
            MessageRecord::ToolResult { content, .. } => self.estimator.estimate(content),
        }
    }

    fn select_prefix<'a>(
        &self,
        state: &'a SessionState,
        context_window: usize,
    ) -> Option<Vec<&'a MessageRecord>> {
        let records = effective_records(state);
        if records.len() < 3 {
            return None;
        }
        let keep_budget =
            context_window.saturating_mul(usize::from(self.config.keep_recent_percent)) / 100;
        let mut kept = 0usize;
        let mut candidate = records.len();
        for (index, record) in records.iter().enumerate().rev() {
            kept += self.estimate_record(record);
            if kept > keep_budget {
                candidate = index;
                break;
            }
        }
        if candidate >= records.len() {
            return None;
        }

        let mut cut = None;
        for index in (1..=candidate).rev() {
            if matches!(
                records[index],
                MessageRecord::User { .. } | MessageRecord::Context { .. }
            ) {
                cut = Some(index);
                break;
            }
        }
        cut.map(|index| records[..index].to_vec())
    }

    fn should_compact(
        &self,
        total: usize,
        body_after_prefix: usize,
        context_window: usize,
    ) -> bool {
        let threshold =
            context_window.saturating_mul(usize::from(self.config.threshold_percent)) / 100;
        let hard_limit = context_window.saturating_mul(HARD_LIMIT_PERCENT) / 100;
        match self.config.scope {
            CompactionScope::Total => total > threshold,
            CompactionScope::BodyAfterPrefix => body_after_prefix > threshold || total > hard_limit,
        }
    }

    async fn summarize(
        &self,
        provider: &dyn ModelProvider,
        model: &str,
        session_id: &str,
        state: &SessionState,
        records: &[&MessageRecord],
    ) -> Result<String, ContextError> {
        let chunks = self.render_chunks(records);
        if chunks.is_empty() {
            return Err(ContextError::message("no records to summarize"));
        }
        let mut summary = state
            .compaction
            .as_ref()
            .map(|compaction| compaction.summary.clone())
            .unwrap_or_default();

        for chunk in chunks {
            let mut input = String::new();
            if !summary.is_empty() {
                input.push_str("Existing summary:\n");
                input.push_str(&summary);
                input.push_str("\n\nNew records to merge:\n");
            }
            input.push_str(&chunk);
            let response = provider
                .complete(CompletionRequest {
                    session_id: session_id.to_owned(),
                    model: model.to_owned(),
                    reasoning: Default::default(),
                    messages: vec![
                        ChatMessage::System(SUMMARY_PROMPT.to_owned()),
                        ChatMessage::User(input),
                    ],
                    tools: Vec::new(),
                    provider_tools: Vec::new(),
                })
                .await
                .map_err(|error| ContextError::message(error.to_string()))?;
            let text = response.text.trim();
            if text.is_empty() {
                return Err(ContextError::message("summarizer returned empty text"));
            }
            summary = text.to_owned();
        }
        Ok(summary)
    }

    fn render_chunks(&self, records: &[&MessageRecord]) -> Vec<String> {
        let max_tokens = self.config.max_summary_input_tokens;
        let mut chunks = Vec::new();
        let mut current = String::new();
        let mut current_tokens = 0usize;

        for record in records {
            let mut piece = render_record(record);
            let piece_tokens = self.estimator.estimate(&piece);
            if piece_tokens > max_tokens {
                piece = truncate_to_tokens(&piece, max_tokens, self.estimator.as_ref());
                piece.push_str("\n[record truncated]");
            }
            let piece_tokens = self.estimator.estimate(&piece);
            if !current.is_empty() && current_tokens + piece_tokens > max_tokens {
                chunks.push(std::mem::take(&mut current));
                current_tokens = 0;
            }
            if !current.is_empty() {
                current.push_str("\n\n");
            }
            current.push_str(&piece);
            current_tokens += piece_tokens + 1;
        }
        if !current.is_empty() {
            chunks.push(current);
        }
        chunks
    }

    fn fallback_summary(&self, records: &[&MessageRecord]) -> String {
        let rendered = records
            .iter()
            .map(|record| render_record(record))
            .collect::<Vec<_>>()
            .join("\n\n");
        let excerpt = truncate_to_tokens(
            &rendered,
            self.config.fallback_summary_tokens,
            self.estimator.as_ref(),
        );
        format!(
            "# Fallback Compacted History\n\n\
             Model-generated summarization was unavailable. This deterministic excerpt preserves \
             the oldest replaced records:\n\n{excerpt}"
        )
    }
}

impl ContextPreparer for DefaultContextCompactor {
    fn prepare<'a>(
        &'a self,
        writer: &'a SessionWriter,
        state: &'a mut SessionState,
        provider: &'a dyn ModelProvider,
        model: &'a str,
        prompt: &'a str,
    ) -> BoxFuture<'a, Result<(), ContextError>> {
        Box::pin(async move {
            if self.config.context_window_tokens == 0 {
                return Ok(());
            }
            let context_window = provider
                .context_window(model)
                .filter(|window| *window > 0)
                .unwrap_or(self.config.context_window_tokens);
            let total = self.estimate_state(state, prompt);
            let body_after_prefix = total.saturating_sub(self.estimate_prefix(state));
            if !self.should_compact(total, body_after_prefix, context_window) {
                return Ok(());
            }
            let Some(records) = self.select_prefix(state, context_window) else {
                return Ok(());
            };
            if records.is_empty() {
                return Ok(());
            }
            let summary = match self
                .summarize(
                    provider,
                    model,
                    &writer.header().id.to_string(),
                    state,
                    &records,
                )
                .await
            {
                Ok(summary) => summary,
                Err(_) => self.fallback_summary(&records),
            };
            let from_seq = records.first().expect("non-empty records").seq();
            let through_seq = records.last().expect("non-empty records").seq();
            let envelope = writer
                .append(SessionEvent::ContextCompacted {
                    summary,
                    from_seq,
                    through_seq,
                })
                .map_err(|error| ContextError::message(error.to_string()))?;
            state.apply(&envelope);
            Ok(())
        })
    }
}

fn effective_records(state: &SessionState) -> Vec<&MessageRecord> {
    state
        .messages
        .iter()
        .filter(|record| {
            state
                .compaction
                .as_ref()
                .is_none_or(|compaction| record.seq() > compaction.through_seq)
        })
        .collect()
}

fn render_record(record: &MessageRecord) -> String {
    match record {
        MessageRecord::User { text, .. } => format!("user:\n{text}"),
        MessageRecord::Context { key, text, .. } => format!("context {key}:\n{text}"),
        MessageRecord::Assistant { text, .. } => format!("assistant:\n{text}"),
        MessageRecord::ToolCall { name, input, .. } => {
            format!("tool_call {name}:\n{input}")
        }
        MessageRecord::ToolResult {
            status, content, ..
        } => format!("tool_result {}:\n{content}", status_name(*status)),
    }
}

fn status_name(status: ToolStatus) -> &'static str {
    match status {
        ToolStatus::Completed => "completed",
        ToolStatus::Failed => "failed",
        ToolStatus::Interrupted => "interrupted",
    }
}

fn truncate_to_tokens(text: &str, max_tokens: usize, estimator: &dyn TokenEstimator) -> String {
    let mut end = text.len();
    while end > 0 && estimator.estimate(&text[..end]) > max_tokens {
        end = end.saturating_sub(64);
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
    }
    text[..end].to_owned()
}

const SUMMARY_PROMPT: &str = "\
You are the context compactor for a coding agent. Condense the input records into a factual summary that lets later tasks continue the work.

Must preserve:
- The user's goals, explicit constraints, and current task state
- Confirmed architectural decisions, implementation approaches, and file paths
- Important files that were modified or read, and why
- Verification commands that were run or must be run, and their results
- Unresolved issues, risks, blockers, and next steps

Rules:
- Do not add information absent from the records and do not speculate.
- Distinguish completed, failed, and not-yet-executed items.
- Keep exact paths, commands, error text, and identifiers.
- Remove pleasantries, repeated process, and irrelevant output.
- Output compact Markdown sections.";

//! Default agent driver.

use std::collections::BTreeMap;
use std::sync::Arc;

use alpha_agent_api::{AgentDriver, ContextError, ContextInjector, LoopError, TurnOutcome};
use alpha_kernel::{Ctx, Event, ParallelEvent};
use alpha_module_tools::{ToolContext, ToolError, ToolRegistry};
use alpha_schema::{
    AssistantContentBlock, ImageAttachment, MessageId, ReasoningLevel, ToolCallId, TurnId,
};
use alpha_session_log::{
    MessageRecord, SessionError, SessionEvent, SessionState, SessionWriter, ToolStatus,
    TurnEndReason,
};
use futures::StreamExt;
use futures::future::BoxFuture;
use serde_json::Value;

use crate::provider::{
    ChatMessage, CompletionRequest, CompletionResponse, ModelProvider, ModelRoute,
    ModelRouteOverride, ProviderError, ToolCall, ToolDefinition,
};

/// One steer message waiting for a safe injection boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SteerMessage {
    /// Stable steer identity.
    pub steer_id: MessageId,
    /// Turn receiving the steer.
    pub turn_id: TurnId,
    /// Steer text.
    pub text: String,
    /// Images attached to the steer.
    pub images: Vec<alpha_schema::ImageAttachment>,
}

/// Queue drained by the loop at step boundaries.
pub trait SteerQueue: Send + Sync + 'static {
    /// Drain messages waiting for injection.
    fn drain(&self) -> Vec<SteerMessage>;
}

/// Default driver: one model request over the recorded history.
#[derive(Clone)]
pub struct AgentLoop {
    provider: Arc<dyn ModelProvider>,
    model: String,
    tools: Option<Arc<ToolRegistry>>,
    context_preparer: Option<Arc<dyn ContextPreparer>>,
    context_injector: Option<Arc<dyn ContextInjector>>,
    steer_queue: Option<Arc<dyn SteerQueue>>,
}

impl AgentLoop {
    /// Create a loop around one provider.
    pub fn new(provider: Arc<dyn ModelProvider>, model: impl Into<String>) -> Self {
        Self {
            provider,
            model: model.into(),
            tools: None,
            context_preparer: None,
            context_injector: None,
            steer_queue: None,
        }
    }

    /// Attach a tool registry.
    pub fn with_tools(mut self, tools: Arc<ToolRegistry>) -> Self {
        self.tools = Some(tools);
        self
    }

    /// Attach a context-preparation pipeline.
    pub fn with_context_preparer(mut self, preparer: Arc<dyn ContextPreparer>) -> Self {
        self.context_preparer = Some(preparer);
        self
    }

    /// Attach a dynamic context injector.
    pub fn with_context_injector(mut self, injector: Arc<dyn ContextInjector>) -> Self {
        self.context_injector = Some(injector);
        self
    }

    /// Attach a steer queue checked at model step boundaries.
    pub fn with_steer_queue(mut self, queue: Arc<dyn SteerQueue>) -> Self {
        self.steer_queue = Some(queue);
        self
    }

    /// The configured model identifier.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Run one turn and record it in the session log.
    ///
    /// The turn runs model steps until the model stops requesting tools.
    /// `state` is the projection before this turn.
    pub async fn run_turn(
        &self,
        ctx: &Ctx,
        writer: &SessionWriter,
        state: &SessionState,
        prompt: &str,
    ) -> Result<TurnOutcome, LoopError> {
        self.run_turn_impl(ctx, writer, state, prompt, &[]).await
    }

    /// Run one turn with images attached to the user message.
    pub async fn run_turn_with_images(
        &self,
        ctx: &Ctx,
        writer: &SessionWriter,
        state: &SessionState,
        prompt: &str,
        images: &[ImageAttachment],
    ) -> Result<TurnOutcome, LoopError> {
        self.run_turn_impl(ctx, writer, state, prompt, images).await
    }

    async fn run_turn_impl(
        &self,
        ctx: &Ctx,
        writer: &SessionWriter,
        state: &SessionState,
        prompt: &str,
        images: &[ImageAttachment],
    ) -> Result<TurnOutcome, LoopError> {
        let session_id = writer.header().id;
        let initial_route = self.route(ctx);
        // Open the turn before any preparation: the user message has to be
        // visible and steerable even while context preparation runs, and a
        // cancelled turn must never end without a start.
        writer.append(SessionEvent::TurnStarted)?;
        let user_id = MessageId::new();
        writer.append(SessionEvent::UserMessage {
            id: user_id,
            text: prompt.to_owned(),
            images: images.to_vec(),
        })?;
        let mut effective_state = state.clone();
        let preparation = self
            .prepare_turn(
                ctx,
                writer,
                &mut effective_state,
                &initial_route.model,
                prompt,
            )
            .await;
        if let Err(error) = preparation {
            writer.append(SessionEvent::TurnEnded {
                reason: TurnEndReason::Failed,
            })?;
            return Err(error);
        }
        let mut messages = HistoryBuilder::from_state(&effective_state);
        if let Some(injector) = &self.context_injector {
            injector
                .inject(ctx, writer, &mut effective_state, &mut messages)
                .await?;
        }
        if let Some(tools) = &self.tools {
            tools
                .prepare_all()
                .await
                .map_err(|error| LoopError::Tool(error.to_string()))?;
        }
        let tools = self.tool_definitions();

        if images.is_empty() {
            messages.push(ChatMessage::User(prompt.to_owned()));
        } else {
            messages.push(ChatMessage::UserContent {
                text: prompt.to_owned(),
                images: images.to_vec(),
            });
        }

        let mut last_text = String::new();
        let mut last_message_id: Option<MessageId> = None;

        let mut step = 1usize;
        loop {
            self.inject_steers(writer, &mut messages)?;
            if let Some(injector) = &self.context_injector {
                injector
                    .inject(ctx, writer, &mut effective_state, &mut messages)
                    .await?;
            }
            let route = self.route(ctx);
            let provider_tools = self.provider_tools(ctx);
            let context_window = self.provider.context_window(&route.model);
            let max_output_tokens = output_budget(
                context_window,
                self.provider.output_limit(&route.model),
                effective_state.last_input_tokens,
            );
            writer.append(SessionEvent::RequestHeader {
                provider: route.provider.clone(),
                model: route.model.clone(),
                reasoning: route.reasoning,
                tools: tools.iter().map(|tool| tool.name.clone()).collect(),
                provider_tools: provider_tools.clone(),
                context_window,
                max_output_tokens,
            })?;
            ctx.parallel(AgentStepStarted { step }).await;
            let request = CompletionRequest {
                session_id: session_id.to_string(),
                model: route.model,
                reasoning: route.reasoning,
                messages: messages.clone(),
                tools: tools.clone(),
                provider_tools,
                max_output_tokens,
            };
            let StreamedResponse { response, usage } =
                match self.stream_response(ctx, request).await {
                    Ok(response) => response,
                    Err(failure) => {
                        let StreamFailure { error, blocks } = failure;
                        record_partial_blocks(writer, blocks)?;
                        ctx.parallel(AgentError {
                            message: error.to_string(),
                        })
                        .await;
                        writer.append(SessionEvent::TurnEnded {
                            reason: TurnEndReason::Failed,
                        })?;
                        return Err(LoopError::Provider(error));
                    }
                };
            if let Some(usage) = usage {
                writer.append(SessionEvent::ContextUsage {
                    input_tokens: usage.input_tokens,
                    output_tokens: usage.output_tokens,
                })?;
            }

            let message_id = MessageId::new();
            let text = response.text;
            let calls = response.tool_calls;
            let blocks = normalized_blocks(response.blocks, &text, &calls);
            if !blocks.is_empty() {
                writer.append(SessionEvent::AssistantMessage {
                    id: message_id,
                    blocks: blocks.clone(),
                    text: String::new(),
                })?;
                last_text = text.clone();
                last_message_id = Some(message_id);
            }

            if calls.is_empty() {
                if !blocks.is_empty() {
                    messages.push(ChatMessage::AssistantContent(blocks));
                }
                let assistant_id = match last_message_id {
                    Some(id) => id,
                    None => {
                        let id = MessageId::new();
                        writer.append(SessionEvent::AssistantMessage {
                            id,
                            blocks: vec![AssistantContentBlock::Text {
                                text: "(no content)".to_owned(),
                            }],
                            text: String::new(),
                        })?;
                        last_text = "(no content)".to_owned();
                        id
                    }
                };
                writer.append(SessionEvent::TurnEnded {
                    reason: TurnEndReason::Completed,
                })?;
                ctx.parallel(AgentTurnFinished {
                    assistant_message: assistant_id,
                })
                .await;
                return Ok(TurnOutcome {
                    assistant_message: assistant_id,
                    assistant_text: last_text,
                });
            }

            messages.push(ChatMessage::AssistantContent(blocks));
            for call in calls {
                self.execute_tool(ctx, writer, session_id, message_id, &mut messages, call)
                    .await?;
            }
            step = step.saturating_add(1);
        }
    }

    /// Run the context-preparation pipeline for one turn.
    ///
    /// Preparation can rewrite the projected state (compaction) and can talk to
    /// the provider, so the turn is already open when it runs.
    async fn prepare_turn(
        &self,
        ctx: &Ctx,
        writer: &SessionWriter,
        state: &mut SessionState,
        model: &str,
        prompt: &str,
    ) -> Result<(), LoopError> {
        let Some(preparer) = &self.context_preparer else {
            return Ok(());
        };
        preparer
            .prepare(ctx, writer, state, self.provider.as_ref(), model, prompt)
            .await?;
        Ok(())
    }

    async fn stream_response(
        &self,
        ctx: &Ctx,
        request: CompletionRequest,
    ) -> Result<StreamedResponse, StreamFailure> {
        let mut stream = self.provider.clone().stream(request);
        let mut text = String::new();
        let mut calls: BTreeMap<usize, ToolCallBuilder> = BTreeMap::new();
        let mut blocks: Vec<AssistantContentBlock> = Vec::new();
        let mut usage: Option<TokenUsage> = None;

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| StreamFailure {
                error,
                blocks: blocks.clone(),
            })?;
            match chunk {
                crate::provider::CompletionChunk::Text(delta) => {
                    if !delta.is_empty() {
                        text.push_str(&delta);
                        if let Some(AssistantContentBlock::Text { text }) = blocks.last_mut() {
                            text.push_str(&delta);
                        } else {
                            blocks.push(AssistantContentBlock::Text {
                                text: delta.clone(),
                            });
                        }
                        ctx.parallel(AgentTextDelta { text: delta }).await;
                    }
                }
                crate::provider::CompletionChunk::ToolCall(delta) => {
                    let call = calls.entry(delta.index).or_default();
                    if let Some(id) = delta.id {
                        call.id = Some(id);
                    }
                    if let Some(name) = delta.name {
                        call.name = Some(name);
                    }
                    call.arguments.push_str(&delta.arguments);
                }
                crate::provider::CompletionChunk::Thinking {
                    text: delta,
                    signature,
                } => {
                    if let Some(AssistantContentBlock::Thinking {
                        text: current,
                        signature: current_signature,
                    }) = blocks.last_mut()
                    {
                        current.push_str(&delta);
                        if signature.is_some() {
                            *current_signature = signature;
                        }
                    } else {
                        blocks.push(AssistantContentBlock::Thinking {
                            text: delta.clone(),
                            signature,
                        });
                    }
                    if !delta.is_empty() {
                        ctx.parallel(AgentThinkingDelta { text: delta }).await;
                    }
                }
                crate::provider::CompletionChunk::ProviderToolCall { id, name, input } => {
                    blocks.push(AssistantContentBlock::ProviderToolCall { id, name, input });
                }
                crate::provider::CompletionChunk::ProviderToolResult { id, name, content } => {
                    blocks.push(AssistantContentBlock::ProviderToolResult { id, name, content });
                }
                crate::provider::CompletionChunk::Citation {
                    url,
                    title,
                    snippet,
                } => {
                    blocks.push(AssistantContentBlock::Citation {
                        url,
                        title,
                        snippet,
                    });
                }
                crate::provider::CompletionChunk::Usage {
                    input_tokens,
                    output_tokens,
                } => {
                    usage = Some(TokenUsage {
                        input_tokens,
                        output_tokens,
                    })
                }
            }
        }

        let mut tool_calls = Vec::with_capacity(calls.len());
        for (index, call) in calls {
            let id = call.id.unwrap_or_else(|| format!("call_{index}"));
            let name = call.name.ok_or_else(|| StreamFailure {
                error: ProviderError::Request(format!("stream tool call {index} has no name")),
                blocks: blocks.clone(),
            })?;
            let arguments = if call.arguments.trim().is_empty() {
                Value::Null
            } else {
                serde_json::from_str(&call.arguments).map_err(|error| StreamFailure {
                    error: ProviderError::Request(format!(
                        "invalid tool arguments for `{name}`: {error}"
                    )),
                    blocks: blocks.clone(),
                })?
            };
            let call = ToolCall {
                id,
                name,
                arguments,
            };
            blocks.push(AssistantContentBlock::LocalToolCall {
                id: call.id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
            });
            tool_calls.push(call);
        }

        Ok(StreamedResponse {
            response: CompletionResponse {
                text,
                tool_calls,
                blocks,
            },
            usage,
        })
    }

    fn tool_definitions(&self) -> Vec<ToolDefinition> {
        let Some(registry) = &self.tools else {
            return Vec::new();
        };
        registry
            .names()
            .into_iter()
            .filter_map(|name| registry.get(&name))
            .map(|tool| ToolDefinition {
                name: tool.name().to_owned(),
                description: tool.description(),
                input_schema: tool.input_schema(),
            })
            .collect()
    }

    fn provider_tools(&self, ctx: &Ctx) -> Vec<String> {
        ctx.get::<crate::ProviderToolsOverride>()
            .and_then(|override_value| override_value.get())
            .unwrap_or_else(|| self.provider.capabilities().provider_tools)
    }

    fn route(&self, ctx: &Ctx) -> ModelRoute {
        let mut route = ctx
            .get::<ModelRouteOverride>()
            .map(|override_value| override_value.get())
            .unwrap_or_else(|| ModelRoute::new("", self.model.clone(), ReasoningLevel::Auto));
        if route.model.is_empty() {
            route.model = self.model.clone();
        }
        route
    }

    fn inject_steers(
        &self,
        writer: &SessionWriter,
        messages: &mut Vec<ChatMessage>,
    ) -> Result<(), LoopError> {
        let Some(queue) = &self.steer_queue else {
            return Ok(());
        };
        for steer in queue.drain() {
            let message_id = MessageId::new();
            writer.append(SessionEvent::SteerInjected {
                steer_id: steer.steer_id,
                turn_id: steer.turn_id,
                message_id,
                text: steer.text.clone(),
                images: steer.images.clone(),
            })?;
            messages.push(if steer.images.is_empty() {
                ChatMessage::User(steer.text)
            } else {
                ChatMessage::UserContent {
                    text: steer.text,
                    images: steer.images,
                }
            });
        }
        Ok(())
    }

    async fn execute_tool(
        &self,
        ctx: &Ctx,
        writer: &SessionWriter,
        session_id: alpha_schema::SessionId,
        message_id: MessageId,
        messages: &mut Vec<ChatMessage>,
        call: ToolCall,
    ) -> Result<(), LoopError> {
        let call_id = ToolCallId::new();
        writer.append(SessionEvent::ToolCall {
            id: call_id,
            message_id,
            name: call.name.clone(),
            input: call.arguments.clone(),
        })?;

        ctx.parallel(AgentToolStarted {
            call_id,
            name: call.name.clone(),
            input: call.arguments.clone(),
        })
        .await;

        let (output_tx, mut output_rx) = tokio::sync::mpsc::unbounded_channel();
        let execution = match &self.tools {
            Some(registry) => match registry.get(&call.name) {
                Some(tool) => {
                    let context = ToolContext::new(session_id, message_id, call_id)
                        .with_output_sink(output_tx);
                    futures::future::Either::Left(async move {
                        tool.execute(call.arguments, context).await
                    })
                }
                None => futures::future::Either::Right(std::future::ready(Err(
                    ToolError::failure(format!("unknown tool `{}`", call.name)),
                ))),
            },
            None => futures::future::Either::Right(std::future::ready(Err(ToolError::failure(
                "no tools are registered",
            )))),
        };
        futures::pin_mut!(execution);
        let outcome = loop {
            tokio::select! {
                Some(text) = output_rx.recv() => {
                    if !text.is_empty() {
                        ctx.parallel(AgentToolOutput { call_id, text }).await;
                    }
                }
                outcome = &mut execution => break outcome,
            }
        };
        while let Ok(text) = output_rx.try_recv() {
            if !text.is_empty() {
                ctx.parallel(AgentToolOutput { call_id, text }).await;
            }
        }

        let (status, content, images) = match outcome {
            Ok(response) => (ToolStatus::Completed, response.content, response.images),
            Err(ToolError::Failure(message)) => (ToolStatus::Failed, message, Vec::new()),
            Err(ToolError::Operational(message)) => {
                ctx.parallel(AgentError {
                    message: message.clone(),
                })
                .await;
                writer.append(SessionEvent::TurnEnded {
                    reason: TurnEndReason::Failed,
                })?;
                return Err(LoopError::Tool(message));
            }
        };
        ctx.parallel(AgentToolFinished {
            call_id,
            name: call.name.clone(),
            status,
            content: content.clone(),
        })
        .await;
        writer.append(SessionEvent::ToolResult {
            id: call_id,
            status,
            content: content.clone(),
            images: images.clone(),
        })?;
        messages.push(ChatMessage::ToolResult {
            call_id: call.id,
            content,
            images,
        });
        Ok(())
    }
}

#[derive(Default)]
struct ToolCallBuilder {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

struct StreamFailure {
    error: ProviderError,
    blocks: Vec<AssistantContentBlock>,
}

/// Provider-reported token counts for one request.
#[derive(Clone, Copy, Debug)]
struct TokenUsage {
    input_tokens: u64,
    output_tokens: u64,
}

/// One streamed response plus the usage the provider reported for it.
struct StreamedResponse {
    response: CompletionResponse,
    usage: Option<TokenUsage>,
}

/// Output budget that still fits next to the context already in flight.
///
/// Reserving a model's full output budget on top of a nearly full window is
/// rejected by providers before the model runs, so the reservation shrinks to
/// the headroom the last reported prompt left behind.
fn output_budget(
    window: Option<usize>,
    model_limit: Option<u64>,
    used: Option<u64>,
) -> Option<u64> {
    let limit = model_limit?;
    let Some(window) = window.filter(|window| *window > 0) else {
        return Some(limit);
    };
    // Without a reported prompt size the request only has room for a quarter of
    // the window: a session that was opened before usage was recorded must not
    // reserve a full output budget on top of an almost full context.
    let available = match used {
        Some(used) => {
            let used = usize::try_from(used).unwrap_or(usize::MAX);
            window.saturating_sub(used).saturating_sub(window / 20)
        }
        None => window / 4,
    }
    .max(1024);
    Some(u64::try_from(available).unwrap_or(u64::MAX).min(limit))
}

fn record_partial_blocks(
    writer: &SessionWriter,
    blocks: Vec<AssistantContentBlock>,
) -> Result<(), SessionError> {
    if blocks.is_empty() {
        return Ok(());
    }
    writer.append(SessionEvent::AssistantMessage {
        id: MessageId::new(),
        blocks,
        text: String::new(),
    })?;
    Ok(())
}

/// A model step is about to start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentStepStarted {
    /// One-based model step.
    pub step: usize,
}

impl ParallelEvent for AgentStepStarted {}
impl Event for AgentStepStarted {}

/// Assistant text was received.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentTextDelta {
    /// Text fragment.
    pub text: String,
}

impl ParallelEvent for AgentTextDelta {}
impl Event for AgentTextDelta {}

/// Assistant thinking content was received.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentThinkingDelta {
    /// Thinking fragment.
    pub text: String,
}

impl ParallelEvent for AgentThinkingDelta {}
impl Event for AgentThinkingDelta {}

/// A completed tool call is about to execute.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentToolStarted {
    /// Durable call identity.
    pub call_id: ToolCallId,
    /// Tool name.
    pub name: String,
    /// Decoded tool input.
    pub input: Value,
}

impl ParallelEvent for AgentToolStarted {}
impl Event for AgentToolStarted {}

/// Incremental output from a running tool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentToolOutput {
    /// Durable call identity.
    pub call_id: ToolCallId,
    /// Output delta.
    pub text: String,
}

impl ParallelEvent for AgentToolOutput {}
impl Event for AgentToolOutput {}

/// A tool call settled.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentToolFinished {
    /// Durable call identity.
    pub call_id: ToolCallId,
    /// Tool name.
    pub name: String,
    /// Tool status.
    pub status: ToolStatus,
    /// Model-visible result.
    pub content: String,
}

impl ParallelEvent for AgentToolFinished {}
impl Event for AgentToolFinished {}

/// One turn completed successfully.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgentTurnFinished {
    /// Final assistant message.
    pub assistant_message: MessageId,
}

impl ParallelEvent for AgentTurnFinished {}
impl Event for AgentTurnFinished {}

/// One turn failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentError {
    /// Human-readable failure.
    pub message: String,
}

impl ParallelEvent for AgentError {}
impl Event for AgentError {}

/// Optional pre-turn context transformation.
///
/// Implementations may append durable context events and update the supplied
/// session projection. Returning an error leaves the original history intact.
pub trait ContextPreparer: Send + Sync + 'static {
    /// Prepare the projected context for the next turn.
    fn prepare<'a>(
        &'a self,
        ctx: &'a Ctx,
        writer: &'a SessionWriter,
        state: &'a mut SessionState,
        provider: &'a dyn ModelProvider,
        model: &'a str,
        prompt: &'a str,
    ) -> BoxFuture<'a, Result<(), ContextError>>;
}

/// One compaction chunk finished.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgentCompactionProgress {
    /// Chunks already summarized.
    pub done: u64,
    /// Chunks this compaction will summarize.
    pub total: u64,
}

impl ParallelEvent for AgentCompactionProgress {}
impl Event for AgentCompactionProgress {}

impl AgentDriver for AgentLoop {
    fn run_turn<'a>(
        &'a self,
        ctx: &'a Ctx,
        writer: &'a SessionWriter,
        state: &'a SessionState,
        prompt: &'a str,
    ) -> BoxFuture<'a, Result<TurnOutcome, LoopError>> {
        Box::pin(self.run_turn_impl(ctx, writer, state, prompt, &[]))
    }

    fn run_turn_with_images<'a>(
        &'a self,
        ctx: &'a Ctx,
        writer: &'a SessionWriter,
        state: &'a SessionState,
        prompt: &'a str,
        images: &'a [ImageAttachment],
    ) -> BoxFuture<'a, Result<TurnOutcome, LoopError>> {
        Box::pin(self.run_turn_impl(ctx, writer, state, prompt, images))
    }
}

fn normalized_blocks(
    blocks: Vec<AssistantContentBlock>,
    text: &str,
    calls: &[ToolCall],
) -> Vec<AssistantContentBlock> {
    if !blocks.is_empty() {
        return blocks;
    }
    let mut normalized = Vec::new();
    if !text.is_empty() {
        normalized.push(AssistantContentBlock::Text {
            text: text.to_owned(),
        });
    }
    for call in calls {
        normalized.push(AssistantContentBlock::LocalToolCall {
            id: call.id.clone(),
            name: call.name.clone(),
            arguments: call.arguments.clone(),
        });
    }
    normalized
}

/// Rebuilds provider-facing history from the durable projection.
#[derive(Default)]
struct HistoryBuilder {
    messages: Vec<ChatMessage>,
    pending_text: Option<(MessageId, String)>,
    pending_blocks: Option<(MessageId, Vec<AssistantContentBlock>)>,
    pending_calls: Vec<ToolCall>,
    // Durable call and result events interleave, so keep the whole result group together.
    pending_tool_results: Vec<(String, String, Vec<ImageAttachment>)>,
    calls_for: Option<MessageId>,
    tool_result_ids: BTreeMap<String, String>,
}

impl HistoryBuilder {
    fn from_state(state: &SessionState) -> Vec<ChatMessage> {
        let mut builder = Self::default();
        if let Some(prompt) = &state.system_prompt {
            builder.messages.push(ChatMessage::System(prompt.clone()));
        }
        if let Some(compaction) = &state.compaction {
            builder.messages.push(ChatMessage::System(format!(
                "# Compacted History Summary\n\n{}",
                compaction.summary
            )));
        }
        for record in &state.messages {
            if state
                .compaction
                .as_ref()
                .is_some_and(|compaction| record.seq() <= compaction.through_seq)
            {
                continue;
            }
            builder.push(record);
        }
        builder.finish()
    }

    fn push(&mut self, record: &MessageRecord) {
        match record {
            MessageRecord::User { text, images, .. } => {
                self.flush();
                if images.is_empty() {
                    self.messages.push(ChatMessage::User(text.clone()));
                } else {
                    self.messages.push(ChatMessage::UserContent {
                        text: text.clone(),
                        images: images.clone(),
                    });
                }
            }
            MessageRecord::Context { text, .. } => {
                self.flush();
                self.messages.push(ChatMessage::User(text.clone()));
            }
            MessageRecord::Assistant {
                id, blocks, text, ..
            } => {
                self.flush();
                self.pending_text = Some((*id, text.clone()));
                self.pending_blocks = Some((*id, blocks.clone()));
            }
            MessageRecord::ToolCall {
                message_id,
                id,
                name,
                input,
                ..
            } => {
                if self.calls_for != Some(*message_id) {
                    let has_pending_message = self
                        .pending_blocks
                        .as_ref()
                        .is_some_and(|(pending_id, _)| pending_id == message_id);
                    if !has_pending_message {
                        self.flush();
                    }
                    self.calls_for = Some(*message_id);
                }
                self.pending_calls.push(ToolCall {
                    id: id.to_string(),
                    name: name.clone(),
                    arguments: input.clone(),
                });
            }
            MessageRecord::ToolResult {
                id,
                content,
                images,
                ..
            } => {
                self.pending_tool_results
                    .push((id.to_string(), content.clone(), images.clone()));
            }
        }
    }

    fn flush(&mut self) {
        let calls = std::mem::take(&mut self.pending_calls);
        let caller = self.calls_for.take();
        let pending_blocks = self.pending_blocks.take();
        let pending_text = self.pending_text.take();

        if calls.is_empty() {
            match (pending_blocks, pending_text) {
                (Some((_, blocks)), _) if !blocks.is_empty() => {
                    self.messages.push(ChatMessage::AssistantContent(blocks));
                }
                (_, Some((_, text))) => {
                    self.messages.push(ChatMessage::Assistant(text));
                }
                _ => {}
            }
        } else {
            let (content_id, mut blocks) = match pending_blocks {
                Some((id, blocks)) if !blocks.is_empty() => (Some(id), blocks),
                _ => {
                    let text = pending_text.map(|(_, text)| text).unwrap_or_default();
                    (None, text_to_blocks(text))
                }
            };
            if let (Some(content_id), Some(caller)) = (content_id, caller)
                && content_id != caller
            {
                self.messages.push(ChatMessage::AssistantContent(blocks));
                blocks = Vec::new();
            }
            let mut matched_blocks = vec![false; blocks.len()];
            for call in calls {
                let matched = blocks.iter().enumerate().find_map(|(index, block)| {
                    let AssistantContentBlock::LocalToolCall { id, name, .. } = block else {
                        return None;
                    };
                    (!matched_blocks[index] && name == &call.name).then(|| {
                        matched_blocks[index] = true;
                        id.clone()
                    })
                });
                let call_id = if let Some(call_id) = matched {
                    call_id
                } else {
                    blocks.push(AssistantContentBlock::LocalToolCall {
                        id: call.id.clone(),
                        name: call.name.clone(),
                        arguments: call.arguments.clone(),
                    });
                    matched_blocks.push(true);
                    call.id.clone()
                };
                self.tool_result_ids.insert(call.id, call_id);
            }
            self.messages.push(ChatMessage::AssistantContent(blocks));
        }

        for (local_id, content, images) in std::mem::take(&mut self.pending_tool_results) {
            let call_id = self
                .tool_result_ids
                .get(&local_id)
                .cloned()
                .unwrap_or(local_id);
            self.messages.push(ChatMessage::ToolResult {
                call_id,
                content,
                images,
            });
        }
    }

    fn finish(mut self) -> Vec<ChatMessage> {
        self.flush();
        self.messages
    }
}

fn text_to_blocks(text: String) -> Vec<AssistantContentBlock> {
    if text.is_empty() {
        Vec::new()
    } else {
        vec![AssistantContentBlock::Text { text }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alpha_schema::{EventEnvelope, PermissionMode, Seq, SessionId, WorkspaceRef};
    use alpha_session_log::SessionHeader;

    #[test]
    fn interleaved_tool_results_follow_the_complete_assistant_message() {
        let header = SessionHeader::new(
            SessionId::new(),
            PermissionMode::Auto,
            WorkspaceRef {
                root: "/workspace".to_owned(),
                cwd: "/workspace".to_owned(),
            },
        );
        let message_id = MessageId::new();
        let first_call = ToolCallId::new();
        let second_call = ToolCallId::new();
        let events = vec![
            EventEnvelope::new(
                Seq::new(1),
                SessionEvent::UserMessage {
                    id: MessageId::new(),
                    text: "inspect the workspace".to_owned(),
                    images: Vec::new(),
                },
            ),
            EventEnvelope::new(
                Seq::new(2),
                SessionEvent::AssistantMessage {
                    id: message_id,
                    blocks: vec![
                        AssistantContentBlock::LocalToolCall {
                            id: "provider-call-1".to_owned(),
                            name: "shell".to_owned(),
                            arguments: serde_json::json!({ "command": "pwd" }),
                        },
                        AssistantContentBlock::LocalToolCall {
                            id: "provider-call-2".to_owned(),
                            name: "shell".to_owned(),
                            arguments: serde_json::json!({ "command": "ls" }),
                        },
                    ],
                    text: String::new(),
                },
            ),
            EventEnvelope::new(
                Seq::new(3),
                SessionEvent::ToolCall {
                    id: first_call,
                    message_id,
                    name: "shell".to_owned(),
                    input: serde_json::json!({ "command": "pwd" }),
                },
            ),
            EventEnvelope::new(
                Seq::new(4),
                SessionEvent::ToolResult {
                    id: first_call,
                    status: ToolStatus::Completed,
                    content: "workspace".to_owned(),
                    images: Vec::new(),
                },
            ),
            EventEnvelope::new(
                Seq::new(5),
                SessionEvent::ToolCall {
                    id: second_call,
                    message_id,
                    name: "shell".to_owned(),
                    input: serde_json::json!({ "command": "ls" }),
                },
            ),
            EventEnvelope::new(
                Seq::new(6),
                SessionEvent::ToolResult {
                    id: second_call,
                    status: ToolStatus::Completed,
                    content: "Cargo.toml".to_owned(),
                    images: Vec::new(),
                },
            ),
        ];
        let state = SessionState::from_events(&header, &events);

        let messages = HistoryBuilder::from_state(&state);

        assert_eq!(messages.len(), 4);
        let ChatMessage::AssistantContent(blocks) = &messages[1] else {
            panic!("assistant message should retain structured tool calls");
        };
        assert_eq!(
            blocks
                .iter()
                .filter(|block| matches!(block, AssistantContentBlock::LocalToolCall { .. }))
                .count(),
            2
        );
        assert!(matches!(
            &messages[2],
            ChatMessage::ToolResult { call_id, .. } if call_id == "provider-call-1"
        ));
        assert!(matches!(
            &messages[3],
            ChatMessage::ToolResult { call_id, .. } if call_id == "provider-call-2"
        ));
    }
}

//! Application bootstrap for the `alpha` binary.

include!(concat!(env!("OUT_DIR"), "/alpha_modules.rs"));

use std::io::{BufRead, Write};
use std::path::Path;
use std::sync::Arc;

use alpha_api_anthropic_messages::AnthropicMessagesApiModule;
use alpha_api_openai_chat::OpenAiChatApiModule;
use alpha_api_openai_responses::OpenAiResponsesApiModule;
use alpha_kernel::{ActivateError, Ctx, Module};
use alpha_module_compaction::{CompactionConfig, CompactionModule};
use alpha_module_credential::{CredentialConfig, CredentialModule, CredentialStore};
use alpha_module_fs::{FsConfig, FsLocalModule};
use alpha_module_http::{HttpClient, HttpConfig, HttpLocalModule};
use alpha_module_loop::{AgentDriver, LoopConfig, LoopModule, ModelProvider, ProviderRegistry};
use alpha_module_permission::{
    ApprovalChannel, ApprovalDecision, ApprovalPrompt, AuditSink, CollectingAuditSink,
    PermissionConfig, PermissionModule,
};
use alpha_module_process::{ProcessConfig, ProcessLocalModule};
use alpha_module_prompt::{PromptAssembler, PromptConfig, PromptModule};
use alpha_module_provider_anthropic_messages::AnthropicMessagesProviderModule;
use alpha_module_provider_config::{
    ProviderConfigConfig, ProviderConfigModule, ProviderConfigStore,
};
use alpha_module_provider_deepseek::DeepSeekProviderModule;
use alpha_module_provider_openai::{OpenAiConfig, OpenAiProviderModule, PROVIDER_NAME};
use alpha_module_provider_openai_chat_completions::OpenAiChatProviderModule;
use alpha_module_provider_openai_responses::OpenAiResponsesProviderModule;
use alpha_module_provider_opencode_go::OpenCodeGoProviderModule;
use alpha_module_session::{
    SessionError, SessionEvent, SessionHeader, SessionState, SessionStore, SessionWriter,
    WorkspaceRef,
};
use alpha_module_tools::{ToolsConfig, ToolsModule};
use alpha_protocol::{
    FramingError, Id, Message, Response, Router, RpcError, read_message, write_message,
};
use alpha_provider_api::{
    ApiAdapterRegistry, ProviderApiModule, ProviderBuildContext, ProviderInstance,
    ProviderInstanceRegistry, ProviderKindRegistry,
};
use alpha_schema::{EventEnvelope, PermissionMode, SessionId};
use futures::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Value, json};

/// Module keys enabled by the Cargo dependency graph.
pub fn module_keys() -> &'static [&'static str] {
    ALPHA_MODULE_KEYS
}

/// Errors from one `alpha run` invocation.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// Session storage failure.
    #[error(transparent)]
    Session(#[from] SessionError),
    /// Module activation failure.
    #[error(transparent)]
    Activate(#[from] ActivateError),
    /// Missing or invalid service lookup.
    #[error("service error: {0}")]
    Service(String),
    /// Agent loop failure.
    #[error("loop error: {0}")]
    Loop(String),
    /// Prompt assembly failure.
    #[error("prompt error: {0}")]
    Prompt(String),
}

/// Options for one headless turn.
#[derive(Clone, Debug)]
pub struct RunOptions {
    /// Workspace root and cwd.
    pub workspace: std::path::PathBuf,
    /// Permission mode.
    pub mode: PermissionMode,
    /// Model identifier.
    pub model: String,
    /// Prompt text.
    pub prompt: String,
}

/// Result of one headless turn.
#[derive(Clone, Debug)]
pub struct RunReport {
    /// Session identity.
    pub session_id: SessionId,
    /// Final assistant text.
    pub assistant_text: String,
}

/// Summary of one stored session.
#[derive(Clone, Debug)]
pub struct SessionSummary {
    /// Session identity.
    pub id: SessionId,
    /// Creation time in milliseconds since the Unix epoch.
    pub created_at_ms: i64,
    /// Permission mode recorded at creation.
    pub mode: PermissionMode,
    /// Workspace root.
    pub workspace: String,
}

/// Options for resuming a session.
#[derive(Clone, Debug)]
pub struct ResumeOptions {
    /// Session identity.
    pub session_id: SessionId,
    /// Optional follow-up prompt.
    pub prompt: Option<String>,
}

/// Result of a resume.
#[derive(Clone, Debug)]
pub struct ResumeReport {
    /// Session identity.
    pub session_id: SessionId,
    /// Assistant text when a follow-up prompt ran.
    pub assistant_text: Option<String>,
    /// Number of messages after resume.
    pub messages: usize,
    /// Whether a turn is still open.
    pub turn_open: bool,
    /// Repair events appended while opening.
    pub repairs: usize,
}

/// A composed runtime ready for one session.
pub struct Composition {
    /// Root context; keep it alive for the lifetime of the run.
    pub ctx: Ctx,
    /// The default agent loop.
    pub agent: Arc<dyn AgentDriver>,
    /// Audit records collected during the run.
    pub audit: Arc<CollectingAuditSink>,
}

/// Core-side runtime state used by the hidden stdio server.
pub struct CoreRuntime {
    /// Root context containing global provider and credential services.
    pub ctx: Ctx,
    /// Session storage used by session methods.
    pub store: SessionStore,
}

impl CoreRuntime {
    /// Compose the core services required by frontends.
    pub fn new(store: SessionStore) -> Result<Self, RunError> {
        let ctx = Ctx::root();
        activate_provider_modules(&ctx)?;
        ProviderConfigModule.activate(&ctx, &ProviderConfigConfig::default())?;
        CredentialModule.activate(&ctx, &CredentialConfig::default())?;
        HttpLocalModule.activate(&ctx, &HttpConfig {})?;
        Ok(Self { ctx, store })
    }
}

/// Approval channel that prompts on the terminal.
pub struct TtyApprovalChannel;

impl ApprovalChannel for TtyApprovalChannel {
    fn decide(&self, prompt: ApprovalPrompt) -> BoxFuture<'_, ApprovalDecision> {
        Box::pin(async move {
            eprintln!(
                "permission requested: {} ({}){}",
                prompt.request.action,
                prompt.request.resources.join(", "),
                if prompt.protected { " [protected]" } else { "" }
            );
            eprintln!("reason: {}", prompt.reason);
            eprintln!("[a]llow once  [s]ession  [d]eny");
            let mut line = String::new();
            if std::io::stdin().read_line(&mut line).is_err() {
                return ApprovalDecision::Deny;
            }
            match line.trim().to_ascii_lowercase().as_str() {
                "a" | "y" | "yes" => ApprovalDecision::AllowOnce,
                "s" | "session" => ApprovalDecision::AllowSession,
                _ => ApprovalDecision::Deny,
            }
        })
    }
}

/// Compose the production runtime with the OpenAI-compatible provider.
pub fn compose_openai_runtime(
    workspace: &Path,
    mode: PermissionMode,
    model: &str,
    base_url: &str,
    api_key: &str,
    approval: Option<Arc<dyn ApprovalChannel>>,
) -> Result<Composition, RunError> {
    let ctx = Ctx::root();
    let audit = Arc::new(CollectingAuditSink::new());
    provide_provider_registry(&ctx)?;
    HttpLocalModule.activate(&ctx, &HttpConfig {})?;
    OpenAiProviderModule.activate(
        &ctx,
        &OpenAiConfig {
            base_url: base_url.to_owned(),
            api_key_env: "OPENAI_API_KEY".to_owned(),
            api_key: Some(api_key.to_owned()),
        },
    )?;
    activate_core_modules(&ctx, workspace, mode, approval, audit.clone())?;
    let agent = activate_loop(&ctx, PROVIDER_NAME, model)?;
    Ok(Composition { ctx, agent, audit })
}

/// Compose the production runtime with an injected HTTP client.
pub fn compose_openai_runtime_with_http(
    workspace: &Path,
    mode: PermissionMode,
    model: &str,
    base_url: &str,
    api_key: &str,
    approval: Option<Arc<dyn ApprovalChannel>>,
    http: Arc<dyn HttpClient>,
) -> Result<Composition, RunError> {
    let ctx = Ctx::root();
    let audit = Arc::new(CollectingAuditSink::new());
    provide_provider_registry(&ctx)?;
    ctx.provide::<dyn HttpClient>(http)
        .map_err(|error| RunError::Service(error.to_string()))?;
    OpenAiProviderModule.activate(
        &ctx,
        &OpenAiConfig {
            base_url: base_url.to_owned(),
            api_key_env: "OPENAI_API_KEY".to_owned(),
            api_key: Some(api_key.to_owned()),
        },
    )?;
    activate_core_modules(&ctx, workspace, mode, approval, audit.clone())?;
    let agent = activate_loop(&ctx, PROVIDER_NAME, model)?;
    Ok(Composition { ctx, agent, audit })
}

/// Compose a runtime with an injected provider, used by tests.
pub fn compose_with_provider(
    workspace: &Path,
    mode: PermissionMode,
    model: &str,
    provider_name: &str,
    approval: Option<Arc<dyn ApprovalChannel>>,
    provider: Arc<dyn ModelProvider>,
) -> Result<Composition, RunError> {
    let ctx = Ctx::root();
    let audit = Arc::new(CollectingAuditSink::new());
    let registry = Arc::new(ProviderRegistry::new());
    registry
        .register(provider_name, provider)
        .map_err(|error| RunError::Service(error.to_string()))?;
    ctx.provide::<ProviderRegistry>(registry)
        .map_err(|error| RunError::Service(error.to_string()))?;
    activate_core_modules(&ctx, workspace, mode, approval, audit.clone())?;
    let agent = activate_loop(&ctx, provider_name, model)?;
    Ok(Composition { ctx, agent, audit })
}

/// Compose a runtime with an injected agent driver, used by tests.
pub fn compose_with_driver(
    workspace: &Path,
    mode: PermissionMode,
    approval: Option<Arc<dyn ApprovalChannel>>,
    driver: Arc<dyn AgentDriver>,
) -> Result<Composition, RunError> {
    let ctx = Ctx::root();
    let audit = Arc::new(CollectingAuditSink::new());
    ctx.provide::<dyn AgentDriver>(driver)
        .map_err(|error| RunError::Service(error.to_string()))?;
    activate_core_modules(&ctx, workspace, mode, approval, audit.clone())?;
    let agent = ctx
        .require::<dyn AgentDriver>()
        .map_err(|error| RunError::Service(error.to_string()))?;
    Ok(Composition { ctx, agent, audit })
}

fn provide_provider_registry(ctx: &Ctx) -> Result<(), RunError> {
    ctx.provide::<ProviderRegistry>(Arc::new(ProviderRegistry::new()))
        .map_err(|error| RunError::Service(error.to_string()))?;
    Ok(())
}

fn activate_core_modules(
    ctx: &Ctx,
    workspace: &Path,
    mode: PermissionMode,
    approval: Option<Arc<dyn ApprovalChannel>>,
    audit: Arc<CollectingAuditSink>,
) -> Result<(), RunError> {
    activate_provider_modules(ctx)?;
    CredentialModule.activate(ctx, &CredentialConfig::default())?;
    FsLocalModule.activate(
        ctx,
        &FsConfig {
            root: workspace.to_path_buf(),
        },
    )?;
    PromptModule.activate(ctx, &PromptConfig::default())?;
    ProcessLocalModule.activate(
        ctx,
        &ProcessConfig {
            cwd: workspace.to_path_buf(),
            ..ProcessConfig::default()
        },
    )?;
    if let Some(channel) = approval {
        ctx.provide::<dyn ApprovalChannel>(channel)
            .map_err(|error| RunError::Service(error.to_string()))?;
    }
    let audit_service: Arc<dyn AuditSink> = audit;
    ctx.provide::<dyn AuditSink>(audit_service)
        .map_err(|error| RunError::Service(error.to_string()))?;
    PermissionModule.activate(
        ctx,
        &PermissionConfig {
            mode,
            ..PermissionConfig::default()
        },
    )?;
    ToolsModule.activate(ctx, &ToolsConfig::default())?;
    CompactionModule.activate(ctx, &CompactionConfig::default())?;
    Ok(())
}

fn activate_provider_modules(ctx: &Ctx) -> Result<(), RunError> {
    ProviderApiModule.activate(ctx, &())?;
    OpenAiChatApiModule.activate(ctx, &())?;
    OpenAiResponsesApiModule.activate(ctx, &())?;
    AnthropicMessagesApiModule.activate(ctx, &())?;
    OpenAiChatProviderModule.activate(ctx, &())?;
    OpenAiResponsesProviderModule.activate(ctx, &())?;
    AnthropicMessagesProviderModule.activate(ctx, &())?;
    DeepSeekProviderModule.activate(ctx, &())?;
    OpenCodeGoProviderModule.activate(ctx, &())?;
    Ok(())
}

fn activate_loop(ctx: &Ctx, provider: &str, model: &str) -> Result<Arc<dyn AgentDriver>, RunError> {
    LoopModule.activate(
        ctx,
        &LoopConfig {
            provider: provider.to_owned(),
            model: model.to_owned(),
            ..LoopConfig::default()
        },
    )?;
    ctx.require::<dyn AgentDriver>()
        .map_err(|error| RunError::Service(error.to_string()))
}

/// Run one headless turn and record it in a session.
pub async fn run_prompt(
    options: RunOptions,
    composition: &Composition,
    store: &SessionStore,
) -> Result<RunReport, RunError> {
    let agent_ctx = composition
        .ctx
        .child()
        .map_err(|error| RunError::Service(error.to_string()))?;
    let result = run_prompt_in(&agent_ctx, options, composition, store).await;
    agent_ctx.dispose();
    result
}

/// Run one turn inside an existing agent-scoped context.
pub async fn run_prompt_in(
    agent_ctx: &Ctx,
    options: RunOptions,
    composition: &Composition,
    store: &SessionStore,
) -> Result<RunReport, RunError> {
    let path = options.workspace.display().to_string();
    let header = SessionHeader::new(
        SessionId::new(),
        options.mode,
        WorkspaceRef {
            root: path.clone(),
            cwd: path,
        },
    );
    let session_id = header.id;
    let mut writer = store.create(header.clone())?;
    let mut state = SessionState::new(&header);
    ensure_system_prompt(composition, &mut writer, &mut state)?;
    let outcome = composition
        .agent
        .run_turn(agent_ctx, &mut writer, &state, &options.prompt)
        .await
        .map_err(|error| RunError::Loop(error.to_string()))?;
    record_audit(&mut writer, composition)?;
    Ok(RunReport {
        session_id,
        assistant_text: outcome.assistant_text,
    })
}

fn record_audit(writer: &mut SessionWriter, composition: &Composition) -> Result<(), RunError> {
    for record in composition.audit.drain() {
        writer.append(SessionEvent::PermissionRequested {
            action: record.request.action.clone(),
            resources: record.request.resources.clone(),
        })?;
        writer.append(SessionEvent::PermissionDecided {
            action: record.request.action,
            resources: record.request.resources,
            allowed: record.outcome.is_allowed(),
            source: record.source.label().to_owned(),
        })?;
    }
    Ok(())
}

/// List stored sessions, newest first.
pub fn list_sessions(store: &SessionStore) -> Result<Vec<SessionSummary>, RunError> {
    let headers = store.list()?;
    Ok(headers
        .into_iter()
        .map(|header| SessionSummary {
            id: header.id,
            created_at_ms: header.created_at.as_millis(),
            mode: header.mode,
            workspace: header.workspace.root,
        })
        .collect())
}

/// Resume a session, optionally running one follow-up prompt.
pub async fn resume_session(
    options: ResumeOptions,
    composition: &Composition,
    store: &SessionStore,
) -> Result<ResumeReport, RunError> {
    let agent_ctx = composition
        .ctx
        .child()
        .map_err(|error| RunError::Service(error.to_string()))?;
    let result = resume_session_in(&agent_ctx, options, composition, store).await;
    agent_ctx.dispose();
    result
}

/// Resume a session inside an existing agent-scoped context.
pub async fn resume_session_in(
    agent_ctx: &Ctx,
    options: ResumeOptions,
    composition: &Composition,
    store: &SessionStore,
) -> Result<ResumeReport, RunError> {
    let reader = store.open_read(options.session_id)?;
    let mut state = reader.state();
    drop(reader);

    let mut writer = store.open_write(options.session_id)?;
    let repairs = writer.repairs().len();
    ensure_system_prompt(composition, &mut writer, &mut state)?;
    let prompt = options
        .prompt
        .as_deref()
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty());
    let assistant_text = match prompt {
        Some(prompt) => Some(
            composition
                .agent
                .run_turn(agent_ctx, &mut writer, &state, prompt)
                .await
                .map_err(|error| RunError::Loop(error.to_string()))?
                .assistant_text,
        ),
        None => None,
    };
    record_audit(&mut writer, composition)?;
    drop(writer);

    let reader = store.open_read(options.session_id)?;
    let final_state = reader.state();
    Ok(ResumeReport {
        session_id: options.session_id,
        assistant_text,
        messages: final_state.messages.len(),
        turn_open: final_state.turn_open,
        repairs,
    })
}

fn ensure_system_prompt(
    composition: &Composition,
    writer: &mut SessionWriter,
    state: &mut SessionState,
) -> Result<(), RunError> {
    if state.system_prompt.is_some() {
        return Ok(());
    }
    let Some(assembler) = composition.ctx.get::<dyn PromptAssembler>() else {
        return Ok(());
    };
    let prompt = assembler
        .assemble(&composition.ctx, state)
        .map_err(|error| RunError::Prompt(error.to_string()))?;
    let envelope = writer.append(SessionEvent::SystemPromptSet { text: prompt })?;
    state.apply(&envelope);
    Ok(())
}

/// Errors from the stdio server.
#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    /// Core composition failed.
    #[error(transparent)]
    Core(#[from] RunError),
    /// Protocol framing error.
    #[error(transparent)]
    Framing(#[from] FramingError),
    /// Failed to start the async runtime.
    #[error("failed to start async runtime: {0}")]
    Runtime(std::io::Error),
}

/// Serve JSON-RPC over stdio using the default session store.
pub fn serve_stdio(store: SessionStore) -> Result<(), ServeError> {
    serve_core_stdio(store)
}

/// Serve the managed core protocol over stdio.
pub fn serve_core_stdio(store: SessionStore) -> Result<(), ServeError> {
    let runtime = CoreRuntime::new(store)?;
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut input = stdin.lock();
    let mut output = stdout.lock();
    serve_router_with(
        &mut input,
        &mut output,
        build_router_with(runtime.store, Some(runtime.ctx)),
    )
}

/// Serve JSON-RPC over an arbitrary reader and writer.
pub fn serve_with<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    store: SessionStore,
) -> Result<(), ServeError> {
    serve_router_with(reader, writer, build_router(store))
}

fn serve_router_with<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    router: Router,
) -> Result<(), ServeError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(ServeError::Runtime)?;

    loop {
        match read_message(reader) {
            Ok(Some(message)) => {
                if let Some(response) = runtime.block_on(router.handle(message)) {
                    write_message(writer, &response)?;
                }
            }
            Ok(None) => return Ok(()),
            Err(error @ (FramingError::Parse(_) | FramingError::TooLarge { .. })) => {
                let response = Message::Response(Response::failure(
                    Id::Null,
                    RpcError::parse_error(error.to_string()),
                ));
                write_message(writer, &response)?;
            }
            Err(error) => return Err(ServeError::Framing(error)),
        }
    }
}

fn build_router(store: SessionStore) -> Router {
    build_router_with(store, None)
}

fn build_router_with(store: SessionStore, ctx: Option<Ctx>) -> Router {
    let store = Arc::new(store);
    let router = Router::new()
        .method("body.hello", |_params: Value| async move {
            Ok::<Value, RpcError>(json!({
                "protocol": "alpha-protocol/1",
                "coreInstanceId": format!("core-{}", std::process::id()),
                "pid": std::process::id(),
            }))
        })
        .method("body.shutdown", |_params: Value| async move {
            Ok::<Value, RpcError>(json!({ "ok": true }))
        })
        .method("alpha.ping", |_params: Value| async move {
            Ok::<Value, RpcError>(json!({
                "pong": true,
                "version": env!("CARGO_PKG_VERSION"),
            }))
        })
        .method("session.list", {
            let store = store.clone();
            move |_params: Value| {
                let store = store.clone();
                async move {
                    let headers = store
                        .list()
                        .map_err(|error| RpcError::internal(error.to_string()))?;
                    serde_json::to_value(headers)
                        .map_err(|error| RpcError::internal(error.to_string()))
                }
            }
        })
        .method("session.attach", {
            let store = store.clone();
            move |params: Value| {
                let store = store.clone();
                async move {
                    let params: AttachParams = serde_json::from_value(params)
                        .map_err(|error| RpcError::invalid_params(error.to_string()))?;
                    let reader = store
                        .open_read(params.session_id)
                        .map_err(|error| RpcError::internal(error.to_string()))?;
                    let after = params.after_seq.unwrap_or(0);
                    let events: Vec<&EventEnvelope<SessionEvent>> = reader
                        .events()
                        .iter()
                        .filter(|envelope| envelope.seq.get() > after)
                        .collect();
                    serde_json::to_value(events)
                        .map_err(|error| RpcError::internal(error.to_string()))
                }
            }
        })
        .method("session.resume", {
            let store = store.clone();
            move |params: Value| {
                let store = store.clone();
                async move {
                    let params: ResumeParams = serde_json::from_value(params)
                        .map_err(|error| RpcError::invalid_params(error.to_string()))?;
                    let reader = store
                        .open_read(params.session_id)
                        .map_err(|error| RpcError::internal(error.to_string()))?;
                    let mode = reader.header().mode;
                    let workspace = reader.header().workspace.root.clone();
                    drop(reader);
                    let writer = store
                        .open_write(params.session_id)
                        .map_err(|error| RpcError::internal(error.to_string()))?;
                    let repairs = writer.repairs().len();
                    drop(writer);
                    let reader = store
                        .open_read(params.session_id)
                        .map_err(|error| RpcError::internal(error.to_string()))?;
                    let state = reader.state();
                    Ok(json!({
                        "sessionId": params.session_id,
                        "mode": mode,
                        "workspace": workspace,
                        "messages": state.messages.len(),
                        "turnOpen": state.turn_open,
                        "repairs": repairs,
                    }))
                }
            }
        });
    match ctx {
        Some(ctx) => add_provider_methods(router, ctx),
        None => router,
    }
}

fn add_provider_methods(router: Router, ctx: Ctx) -> Router {
    let kind_registry = ctx
        .require::<ProviderKindRegistry>()
        .expect("provider kind registry is required by the core");
    let instance_registry = ctx
        .require::<ProviderInstanceRegistry>()
        .expect("provider instance registry is required by the core");
    let config_store = ctx.get::<dyn ProviderConfigStore>();

    let router = router.method("provider.list_kinds", {
        let registry = kind_registry.clone();
        move |_params: Value| {
            let registry = registry.clone();
            async move {
                serde_json::to_value(registry.descriptors())
                    .map_err(|error| RpcError::internal(error.to_string()))
            }
        }
    });
    let router = router.method("provider.describe_kind", {
        let registry = kind_registry.clone();
        move |params: Value| {
            let registry = registry.clone();
            async move {
                let params: KindParams = serde_json::from_value(params)
                    .map_err(|error| RpcError::invalid_params(error.to_string()))?;
                let kind = registry
                    .get(&params.kind)
                    .ok_or_else(|| RpcError::invalid_params("unknown provider kind"))?;
                serde_json::to_value(kind.descriptor())
                    .map_err(|error| RpcError::internal(error.to_string()))
            }
        }
    });
    let router = router.method("provider.list_instances", {
        let registry = instance_registry.clone();
        move |_params: Value| {
            let registry = registry.clone();
            async move {
                serde_json::to_value(registry.list())
                    .map_err(|error| RpcError::internal(error.to_string()))
            }
        }
    });
    let router = router.method("provider.create_instance", {
        let kinds = kind_registry.clone();
        let instances = instance_registry.clone();
        let config_store = config_store.clone();
        move |params: Value| {
            let kinds = kinds.clone();
            let instances = instances.clone();
            let config_store = config_store.clone();
            async move {
                let instance: ProviderInstance = serde_json::from_value(params)
                    .map_err(|error| RpcError::invalid_params(error.to_string()))?;
                if kinds.get(&instance.kind).is_none() {
                    return Err(RpcError::invalid_params(format!(
                        "unknown provider kind `{}`",
                        instance.kind
                    )));
                }
                if let Some(store) = config_store {
                    store
                        .upsert(instance.clone())
                        .map_err(|error| RpcError::internal(error.to_string()))?;
                } else {
                    instances.upsert(instance.clone());
                }
                serde_json::to_value(instance)
                    .map_err(|error| RpcError::internal(error.to_string()))
            }
        }
    });
    let router = router.method("provider.update_instance", {
        let kinds = kind_registry.clone();
        let instances = instance_registry.clone();
        let config_store = config_store.clone();
        move |params: Value| {
            let kinds = kinds.clone();
            let instances = instances.clone();
            let config_store = config_store.clone();
            async move {
                let instance: ProviderInstance = serde_json::from_value(params)
                    .map_err(|error| RpcError::invalid_params(error.to_string()))?;
                if kinds.get(&instance.kind).is_none() {
                    return Err(RpcError::invalid_params(format!(
                        "unknown provider kind `{}`",
                        instance.kind
                    )));
                }
                if let Some(store) = config_store {
                    store
                        .upsert(instance.clone())
                        .map_err(|error| RpcError::internal(error.to_string()))?;
                } else {
                    instances.upsert(instance.clone());
                }
                serde_json::to_value(instance)
                    .map_err(|error| RpcError::internal(error.to_string()))
            }
        }
    });
    let router = router.method("provider.remove_instance", {
        let instances = instance_registry.clone();
        let config_store = config_store.clone();
        move |params: Value| {
            let instances = instances.clone();
            let config_store = config_store.clone();
            async move {
                let params: InstanceIdParams = serde_json::from_value(params)
                    .map_err(|error| RpcError::invalid_params(error.to_string()))?;
                let removed = if let Some(store) = config_store {
                    store
                        .remove(&params.id)
                        .map_err(|error| RpcError::internal(error.to_string()))?
                } else {
                    instances.remove(&params.id).is_some()
                };
                Ok(json!({ "removed": removed }))
            }
        }
    });
    let router = router.method("provider.set_default_instance", {
        let config_store = config_store.clone();
        move |params: Value| {
            let config_store = config_store.clone();
            async move {
                let params: DefaultInstanceParams = serde_json::from_value(params)
                    .map_err(|error| RpcError::invalid_params(error.to_string()))?;
                let store = config_store
                    .ok_or_else(|| RpcError::internal("provider config store is unavailable"))?;
                store
                    .set_default_instance(params.id)
                    .map_err(|error| RpcError::internal(error.to_string()))?;
                Ok(json!({ "ok": true }))
            }
        }
    });
    router.method("provider.test_instance", {
        let kinds = kind_registry;
        let instances = instance_registry;
        let ctx = ctx.clone();
        move |params: Value| {
            let kinds = kinds.clone();
            let instances = instances.clone();
            let ctx = ctx.clone();
            async move {
                let params: InstanceIdParams = serde_json::from_value(params)
                    .map_err(|error| RpcError::invalid_params(error.to_string()))?;
                let instance = instances
                    .get(&params.id)
                    .ok_or_else(|| RpcError::invalid_params("unknown provider instance"))?;
                let kind = kinds
                    .get(&instance.kind)
                    .ok_or_else(|| RpcError::invalid_params("unknown provider kind"))?;
                let adapters = ctx
                    .require::<ApiAdapterRegistry>()
                    .map_err(|error| RpcError::internal(error.to_string()))?;
                let http = ctx
                    .require::<dyn HttpClient>()
                    .map_err(|error| RpcError::internal(error.to_string()))?;
                let credential = match instance.auth_ref.as_deref() {
                    Some(auth_ref) => {
                        let store = ctx
                            .require::<dyn CredentialStore>()
                            .map_err(|error| RpcError::internal(error.to_string()))?;
                        Some(
                            store
                                .read(auth_ref)
                                .map_err(|error| RpcError::internal(error.to_string()))?,
                        )
                    }
                    None => None,
                };
                kind.build(
                    &instance,
                    &ProviderBuildContext {
                        adapters,
                        http,
                        credential,
                    },
                )
                .map_err(|error| RpcError::internal(error.to_string()))?;
                Ok(json!({ "ok": true, "id": instance.id, "kind": instance.kind }))
            }
        }
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AttachParams {
    session_id: SessionId,
    #[serde(default)]
    after_seq: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResumeParams {
    session_id: SessionId,
}

#[derive(Debug, Deserialize)]
struct KindParams {
    kind: String,
}

#[derive(Debug, Deserialize)]
struct InstanceIdParams {
    id: String,
}

#[derive(Debug, Deserialize)]
struct DefaultInstanceParams {
    id: Option<String>,
}

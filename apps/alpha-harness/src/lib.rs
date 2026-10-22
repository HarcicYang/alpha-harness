//! Application bootstrap for the `alpha` binary.

include!(concat!(env!("OUT_DIR"), "/alpha_modules.rs"));

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use alpha_api_anthropic_messages::AnthropicMessagesApiModule;
use alpha_api_openai_chat::OpenAiChatApiModule;
use alpha_api_openai_responses::OpenAiResponsesApiModule;
use alpha_kernel::{ActivateError, Ctx, Module};
use alpha_module_agent_host::{
    AgentHostError, AgentHostService, SessionRuntime, SessionRuntimeFactory, SessionRuntimeRequest,
};
use alpha_module_compaction::{CompactionConfig, CompactionModule};
use alpha_module_credential::{CredentialConfig, CredentialModule, CredentialStore};
use alpha_module_fs::{FsConfig, FsLocalModule};
use alpha_module_http::{HttpClient, HttpConfig, HttpLocalModule};
use alpha_module_loop::{
    AgentDriver, LoopConfig, LoopModule, ModelProvider, ModelRoute, ModelRouteOverride,
    ProviderRegistry, ProviderToolsOverride, SteerQueue,
};
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
    FramingError, Id, Message, Notification, OutboundTransport, Request, Response, ResponsePayload,
    Router, RpcError, read_message, write_message,
};
use alpha_provider_api::{
    ApiAdapterRegistry, DynamicProvider, ProviderApiModule, ProviderBuildContext, ProviderInstance,
    ProviderInstanceRegistry, ProviderKindRegistry, build_provider_instance, default_model_id,
    discover_models, resolve_credential, resolve_provider_choice,
};
use alpha_schema::{
    EventEnvelope, PermissionMode, SessionId, SessionListEntry as FrontSessionListEntry,
};
use futures::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

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

/// Optional services injected into a core runtime.
#[derive(Default)]
pub struct CoreRuntimeOptions {
    /// Credential store override.
    pub credential: Option<Arc<dyn CredentialStore>>,
    /// HTTP client override.
    pub http: Option<Arc<dyn HttpClient>>,
    /// Provider configuration path override.
    pub provider_config_path: Option<PathBuf>,
}

impl CoreRuntime {
    /// Compose the core services required by frontends.
    pub fn new(store: SessionStore) -> Result<Self, RunError> {
        Self::new_with_services(store, None, None)
    }

    /// Compose the core with optional injected credential and HTTP services.
    pub fn new_with_services(
        store: SessionStore,
        credential: Option<Arc<dyn CredentialStore>>,
        http: Option<Arc<dyn HttpClient>>,
    ) -> Result<Self, RunError> {
        Self::new_with_options(
            store,
            CoreRuntimeOptions {
                credential,
                http,
                provider_config_path: None,
            },
        )
    }

    /// Compose the core with explicit runtime options.
    pub fn new_with_options(
        store: SessionStore,
        options: CoreRuntimeOptions,
    ) -> Result<Self, RunError> {
        let ctx = Ctx::root();
        activate_provider_modules(&ctx)?;
        ProviderConfigModule.activate(
            &ctx,
            &ProviderConfigConfig {
                path: options.provider_config_path,
            },
        )?;
        migrate_loaded_provider_instances(&ctx)?;
        if let Some(credential) = options.credential {
            ctx.provide::<dyn CredentialStore>(credential)
                .map_err(|error| RunError::Service(error.to_string()))?;
        } else {
            CredentialModule.activate(&ctx, &CredentialConfig::default())?;
        }
        if let Some(http) = options.http {
            ctx.provide::<dyn HttpClient>(http)
                .map_err(|error| RunError::Service(error.to_string()))?;
        } else {
            HttpLocalModule.activate(&ctx, &HttpConfig {})?;
        }
        Ok(Self { ctx, store })
    }
}

fn migrate_loaded_provider_instances(ctx: &Ctx) -> Result<(), RunError> {
    let Some(store) = ctx.get::<dyn ProviderConfigStore>() else {
        return Ok(());
    };
    let kinds = ctx
        .require::<ProviderKindRegistry>()
        .map_err(|error| RunError::Service(error.to_string()))?;
    for mut instance in store.list() {
        let Some(kind) = kinds.get(&instance.kind) else {
            instance.disabled = true;
            store
                .upsert(instance)
                .map_err(|error| RunError::Service(error.to_string()))?;
            continue;
        };
        let current = kind.config_version();
        if instance.config_version == current {
            continue;
        }
        match kind.migrate_config(instance.config_version, instance.config.clone()) {
            Ok(config) => {
                instance.config = config;
                instance.config_version = current;
                instance.disabled = false;
            }
            Err(_) => {
                instance.disabled = true;
            }
        }
        store
            .upsert(instance)
            .map_err(|error| RunError::Service(error.to_string()))?;
    }
    Ok(())
}

struct AppSessionRuntimeFactory {
    root: Ctx,
    transport: Arc<dyn OutboundTransport>,
    locations: Mutex<HashMap<LocationKey, Ctx>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct LocationKey {
    root: String,
    cwd: String,
}

impl AppSessionRuntimeFactory {
    fn location_context(&self, workspace: &WorkspaceRef) -> Result<Ctx, AgentHostError> {
        let key = LocationKey {
            root: workspace.root.clone(),
            cwd: workspace.cwd.clone(),
        };
        let mut locations = self
            .locations
            .lock()
            .expect("location registry lock poisoned");
        if let Some(context) = locations.get(&key) {
            return Ok(context.clone());
        }
        let location = self
            .root
            .child()
            .map_err(|error| AgentHostError::Runtime(error.to_string()))?;
        FsLocalModule
            .activate(
                &location,
                &FsConfig {
                    root: workspace.root.clone().into(),
                    cwd: workspace.cwd.clone().into(),
                },
            )
            .map_err(|error| AgentHostError::Runtime(error.to_string()))?;
        PromptModule
            .activate(&location, &PromptConfig::default())
            .map_err(|error| AgentHostError::Runtime(error.to_string()))?;
        ProcessLocalModule
            .activate(
                &location,
                &ProcessConfig {
                    cwd: workspace.cwd.clone().into(),
                    ..ProcessConfig::default()
                },
            )
            .map_err(|error| AgentHostError::Runtime(error.to_string()))?;
        CompactionModule
            .activate(&location, &CompactionConfig::default())
            .map_err(|error| AgentHostError::Runtime(error.to_string()))?;
        locations.insert(key, location.clone());
        Ok(location)
    }
}

impl SessionRuntimeFactory for AppSessionRuntimeFactory {
    fn build(&self, request: SessionRuntimeRequest) -> Result<SessionRuntime, AgentHostError> {
        let model_route = Arc::new(ModelRouteOverride::default());
        let provider = build_session_provider(&self.root, &request, model_route.clone())?;
        let location = self.location_context(&request.workspace)?;
        let session_ctx = location
            .child()
            .map_err(|error| AgentHostError::Runtime(error.to_string()))?;
        session_ctx
            .provide::<ModelRouteOverride>(model_route.clone())
            .map_err(|error| AgentHostError::Runtime(error.to_string()))?;
        session_ctx
            .provide::<dyn SteerQueue>(request.steer_queue.clone())
            .map_err(|error| AgentHostError::Runtime(error.to_string()))?;
        let provider_tools_override = Arc::new(ProviderToolsOverride::default());
        provider_tools_override.set(request.provider_tools_override);
        session_ctx
            .provide::<ProviderToolsOverride>(provider_tools_override.clone())
            .map_err(|error| AgentHostError::Runtime(error.to_string()))?;
        let approval: Arc<dyn ApprovalChannel> = Arc::new(ProtocolApprovalChannel {
            session_id: request.session_id,
            transport: self.transport.clone(),
            timeout: Duration::from_secs(300),
        });
        session_ctx
            .provide::<dyn ApprovalChannel>(approval)
            .map_err(|error| AgentHostError::Runtime(error.to_string()))?;
        let audit: Arc<dyn AuditSink> = Arc::new(CollectingAuditSink::new());
        session_ctx
            .provide::<dyn AuditSink>(audit)
            .map_err(|error| AgentHostError::Runtime(error.to_string()))?;

        PermissionModule
            .activate(
                &session_ctx,
                &PermissionConfig {
                    mode: request.mode,
                    ..PermissionConfig::default()
                },
            )
            .map_err(|error| AgentHostError::Runtime(error.to_string()))?;
        ToolsModule
            .activate(&session_ctx, &ToolsConfig::default())
            .map_err(|error| AgentHostError::Runtime(error.to_string()))?;
        let registry = Arc::new(ProviderRegistry::new());
        registry
            .register("session", provider)
            .map_err(|error| AgentHostError::Runtime(error.to_string()))?;
        session_ctx
            .provide::<ProviderRegistry>(registry)
            .map_err(|error| AgentHostError::Runtime(error.to_string()))?;
        LoopModule
            .activate(
                &session_ctx,
                &LoopConfig {
                    provider: "session".to_owned(),
                    model: model_route.get().model,
                    ..LoopConfig::default()
                },
            )
            .map_err(|error| AgentHostError::Runtime(error.to_string()))?;
        let agent = session_ctx
            .require::<dyn AgentDriver>()
            .map_err(|error| AgentHostError::Runtime(error.to_string()))?;
        Ok(SessionRuntime {
            ctx: session_ctx,
            agent,
            provider_tools_override,
            model_route,
        })
    }
}

struct ProtocolApprovalChannel {
    session_id: SessionId,
    transport: Arc<dyn OutboundTransport>,
    timeout: Duration,
}

impl ApprovalChannel for ProtocolApprovalChannel {
    fn decide(&self, prompt: ApprovalPrompt) -> BoxFuture<'_, ApprovalDecision> {
        let transport = self.transport.clone();
        let session_id = self.session_id;
        let timeout = self.timeout;
        Box::pin(async move {
            let request = transport.request(
                "interaction.request".to_owned(),
                json!({
                    "sessionId": session_id,
                    "kind": "permission",
                    "prompt": {
                        "request": prompt.request,
                        "reason": prompt.reason,
                        "protected": prompt.protected,
                    },
                }),
            );
            match tokio::time::timeout(timeout, request).await {
                Ok(Ok(value)) => match value.get("decision").and_then(Value::as_str) {
                    Some("once") => ApprovalDecision::AllowOnce,
                    Some("session") => ApprovalDecision::AllowSession,
                    Some("timeout") => ApprovalDecision::Timeout,
                    _ => ApprovalDecision::Deny,
                },
                Ok(Err(_)) => ApprovalDecision::Deny,
                Err(_) => ApprovalDecision::Timeout,
            }
        })
    }
}

fn build_session_provider(
    root: &Ctx,
    request: &SessionRuntimeRequest,
    model_route: Arc<ModelRouteOverride>,
) -> Result<Arc<dyn ModelProvider>, AgentHostError> {
    let (instance, kind) = if request.provider.trim().is_empty() {
        let instances = root
            .require::<ProviderInstanceRegistry>()
            .map_err(|error| AgentHostError::Runtime(error.to_string()))?;
        let default = root
            .get::<dyn ProviderConfigStore>()
            .and_then(|store| store.default_instance());
        if let Some(instance) = default.and_then(|id| instances.get(&id)) {
            let kinds = root
                .require::<ProviderKindRegistry>()
                .map_err(|error| AgentHostError::Runtime(error.to_string()))?;
            let kind = kinds.get(&instance.kind).ok_or_else(|| {
                AgentHostError::Runtime(format!("provider kind `{}` is missing", instance.kind))
            })?;
            (instance, kind)
        } else {
            resolve_provider_choice(root, "")
                .map_err(|error| AgentHostError::Runtime(error.to_string()))?
        }
    } else {
        resolve_provider_choice(root, &request.provider)
            .map_err(|error| AgentHostError::Runtime(error.to_string()))?
    };
    let model = if request.model.trim().is_empty() {
        default_model_id(&instance, kind.as_ref()).ok_or_else(|| {
            AgentHostError::Runtime(format!("provider kind `{}` has no models", instance.kind))
        })?
    } else {
        request.model.clone()
    };
    build_provider_instance(root, &instance.id)
        .map_err(|error| AgentHostError::Runtime(error.to_string()))?;
    model_route.set(ModelRoute::new(
        instance.id.clone(),
        model,
        request.reasoning,
    ));
    let provider: Arc<dyn ModelProvider> = Arc::new(DynamicProvider::new_routed(
        root.clone(),
        instance.id,
        model_route,
    ));
    Ok(provider)
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
            api_key: api_key.to_owned(),
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
            api_key: api_key.to_owned(),
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
            cwd: workspace.to_path_buf(),
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
    let writer = store.create(header.clone())?;
    let mut state = SessionState::new(&header);
    ensure_system_prompt(composition, &writer, &mut state)?;
    let outcome = composition
        .agent
        .run_turn(agent_ctx, &writer, &state, &options.prompt)
        .await
        .map_err(|error| RunError::Loop(error.to_string()))?;
    record_audit(&writer, composition)?;
    Ok(RunReport {
        session_id,
        assistant_text: outcome.assistant_text,
    })
}

fn record_audit(writer: &SessionWriter, composition: &Composition) -> Result<(), RunError> {
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

    let writer = store.open_write(options.session_id)?;
    let repairs = writer.repairs().len();
    ensure_system_prompt(composition, &writer, &mut state)?;
    let prompt = options
        .prompt
        .as_deref()
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty());
    let assistant_text = match prompt {
        Some(prompt) => Some(
            composition
                .agent
                .run_turn(agent_ctx, &writer, &state, prompt)
                .await
                .map_err(|error| RunError::Loop(error.to_string()))?
                .assistant_text,
        ),
        None => None,
    };
    record_audit(&writer, composition)?;
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
    writer: &SessionWriter,
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

type PendingInteractions = Arc<Mutex<HashMap<Id, oneshot::Sender<Result<Value, RpcError>>>>>;

struct ServerTransport {
    outbound: mpsc::UnboundedSender<Message>,
    pending: PendingInteractions,
    next_id: AtomicI64,
}

impl ServerTransport {
    fn channel() -> (Arc<Self>, mpsc::UnboundedReceiver<Message>) {
        let (outbound, receiver) = mpsc::unbounded_channel();
        (
            Arc::new(Self {
                outbound,
                pending: Arc::new(Mutex::new(HashMap::new())),
                next_id: AtomicI64::new(1),
            }),
            receiver,
        )
    }

    fn resolve(&self, id: Id, result: Result<Value, RpcError>) -> Result<(), RpcError> {
        let sender = self
            .pending
            .lock()
            .expect("protocol pending lock poisoned")
            .remove(&id)
            .ok_or_else(|| RpcError::invalid_params("unknown interaction request"))?;
        sender
            .send(result)
            .map_err(|_| RpcError::internal("interaction waiter was dropped"))
    }

    fn fail_pending(&self, reason: &str) {
        let mut pending = self.pending.lock().expect("protocol pending lock poisoned");
        for (_, sender) in pending.drain() {
            let _ = sender.send(Err(RpcError::internal(reason)));
        }
    }
}

impl OutboundTransport for ServerTransport {
    fn notify(&self, method: String, params: Value) -> Result<(), RpcError> {
        self.outbound
            .send(Message::Notification(Notification::new(
                method,
                Some(params),
            )))
            .map_err(|_| RpcError::internal("core connection is closed"))
    }

    fn request(
        &self,
        method: String,
        params: Value,
    ) -> BoxFuture<'static, Result<Value, RpcError>> {
        let id = Id::Number(self.next_id.fetch_add(1, Ordering::Relaxed));
        let (sender, receiver) = oneshot::channel();
        self.pending
            .lock()
            .expect("protocol pending lock poisoned")
            .insert(id.clone(), sender);
        if self
            .outbound
            .send(Message::Request(Request::new(
                id.clone(),
                method,
                Some(params),
            )))
            .is_err()
        {
            self.pending
                .lock()
                .expect("protocol pending lock poisoned")
                .remove(&id);
            return Box::pin(async { Err(RpcError::internal("core connection is closed")) });
        }
        Box::pin(async move {
            receiver
                .await
                .map_err(|_| RpcError::internal("interaction response channel closed"))?
        })
    }
}

/// Serve JSON-RPC over stdio using the default session store.
pub fn serve_stdio(store: SessionStore) -> Result<(), ServeError> {
    serve_core_stdio(store)
}

/// Serve the managed core protocol over stdio.
pub fn serve_core_stdio(store: SessionStore) -> Result<(), ServeError> {
    serve_core_stdio_with_services(store, None, None)
}

/// Serve the managed core over stdio with optional injected services.
pub fn serve_core_stdio_with_services(
    store: SessionStore,
    credential: Option<Arc<dyn CredentialStore>>,
    http: Option<Arc<dyn HttpClient>>,
) -> Result<(), ServeError> {
    serve_core_stdio_with_options(
        store,
        CoreRuntimeOptions {
            credential,
            http,
            provider_config_path: None,
        },
    )
}

/// Serve the managed core over stdio with explicit runtime options.
pub fn serve_core_stdio_with_options(
    store: SessionStore,
    options: CoreRuntimeOptions,
) -> Result<(), ServeError> {
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    serve_protocol_with_options(&mut input, std::io::stdout(), store, options)
}

/// Serve the managed core protocol over an arbitrary reader and writer.
pub fn serve_protocol_with_services<R: BufRead, W: Write + Send + 'static>(
    reader: &mut R,
    writer: W,
    store: SessionStore,
    credential: Option<Arc<dyn CredentialStore>>,
    http: Option<Arc<dyn HttpClient>>,
) -> Result<(), ServeError> {
    serve_protocol_with_options(
        reader,
        writer,
        store,
        CoreRuntimeOptions {
            credential,
            http,
            provider_config_path: None,
        },
    )
}

/// Serve the managed core protocol with explicit runtime options.
pub fn serve_protocol_with_options<R: BufRead, W: Write + Send + 'static>(
    reader: &mut R,
    writer: W,
    store: SessionStore,
    options: CoreRuntimeOptions,
) -> Result<(), ServeError> {
    let runtime = CoreRuntime::new_with_options(store, options)?;
    let (transport, outbound) = ServerTransport::channel();
    let outbound_service: Arc<dyn OutboundTransport> = transport.clone();
    runtime
        .ctx
        .provide::<dyn OutboundTransport>(outbound_service)
        .map_err(|error| ServeError::Core(RunError::Service(error.to_string())))?;
    let factory = Arc::new(AppSessionRuntimeFactory {
        root: runtime.ctx.clone(),
        transport: transport.clone(),
        locations: Mutex::new(HashMap::new()),
    });
    let agent_host = Arc::new(AgentHostService::new(
        runtime.store.clone(),
        transport.clone(),
        factory,
    ));
    runtime
        .ctx
        .provide::<AgentHostService>(agent_host)
        .map_err(|error| ServeError::Core(RunError::Service(error.to_string())))?;
    let shutdown_ctx = runtime.ctx.clone();
    let router =
        build_router_with_transport(runtime.store, Some(runtime.ctx), Some(transport.clone()));
    serve_router_concurrent(reader, writer, router, transport, outbound, shutdown_ctx)
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

fn serve_router_concurrent<R: BufRead, W: Write + Send + 'static>(
    reader: &mut R,
    writer: W,
    router: Router,
    transport: Arc<ServerTransport>,
    mut outbound: mpsc::UnboundedReceiver<Message>,
    shutdown_ctx: Ctx,
) -> Result<(), ServeError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(ServeError::Runtime)?;
    let writer_thread = thread::spawn(move || {
        let mut writer = writer;
        while let Some(message) = outbound.blocking_recv() {
            write_message(&mut writer, &message)?;
        }
        Ok::<(), FramingError>(())
    });
    let router = Arc::new(router);
    let loop_result = loop {
        match read_message(reader) {
            Ok(Some(Message::Request(request))) if request.method == "body.shutdown" => {
                let _ = transport.outbound.send(Message::Response(Response::success(
                    request.id,
                    json!({ "ok": true }),
                )));
                break Ok(());
            }
            Ok(Some(Message::Response(response))) => {
                let result = match response.payload {
                    ResponsePayload::Success { result } => Ok(result),
                    ResponsePayload::Failure { error } => Err(error),
                };
                let _ = transport.resolve(response.id, result);
            }
            Ok(Some(message)) => {
                let router = router.clone();
                let outbound = transport.outbound.clone();
                runtime.spawn(async move {
                    if let Some(response) = router.handle(message).await {
                        let _ = outbound.send(response);
                    }
                });
            }
            Ok(None) => break Ok(()),
            Err(error @ (FramingError::Parse(_) | FramingError::TooLarge { .. })) => {
                let response = Message::Response(Response::failure(
                    Id::Null,
                    RpcError::parse_error(error.to_string()),
                ));
                let _ = transport.outbound.send(response);
            }
            Err(error) => break Err(ServeError::Framing(error)),
        }
    };
    transport.fail_pending("core connection closed");
    shutdown_ctx.dispose();
    runtime.shutdown_timeout(Duration::from_secs(1));
    drop(router);
    drop(transport);
    let writer_result = writer_thread.join();
    match loop_result {
        Ok(()) => match writer_result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(ServeError::Framing(error)),
            Err(_) => Ok(()),
        },
        Err(error) => Err(error),
    }
}

fn build_router(store: SessionStore) -> Router {
    build_router_with(store, None)
}

fn build_router_with(store: SessionStore, ctx: Option<Ctx>) -> Router {
    build_router_with_transport(store, ctx, None)
}

fn build_router_with_transport(
    store: SessionStore,
    ctx: Option<Ctx>,
    transport: Option<Arc<ServerTransport>>,
) -> Router {
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
                    let sessions = store
                        .list_entries()
                        .map_err(|error| RpcError::internal(error.to_string()))?
                        .into_iter()
                        .map(|entry| FrontSessionListEntry {
                            id: entry.header.id,
                            created_at: entry.header.created_at,
                            mode: entry.header.mode,
                            workspace: entry.header.workspace,
                            provider: entry.header.provider,
                            model: entry.header.model,
                            first_message: entry.first_message,
                        })
                        .collect::<Vec<_>>();
                    serde_json::to_value(sessions)
                        .map_err(|error| RpcError::internal(error.to_string()))
                }
            }
        })
        .method("session.delete", {
            let store = store.clone();
            move |params: Value| {
                let store = store.clone();
                async move {
                    let params: SessionIdParams = serde_json::from_value(params)
                        .map_err(|error| RpcError::invalid_params(error.to_string()))?;
                    store
                        .delete(params.session_id)
                        .map_err(|error| RpcError::internal(error.to_string()))?;
                    Ok(json!({ "ok": true }))
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
    let router = match ctx {
        Some(ctx) => {
            let agent_host = ctx.get::<AgentHostService>();
            let router = add_provider_methods(router, ctx);
            match agent_host {
                Some(agent_host) => add_agent_host_methods(router, agent_host),
                None => router,
            }
        }
        None => router,
    };
    match transport {
        Some(transport) => router.method("interaction.response", move |params: Value| {
            let transport = transport.clone();
            async move {
                let params: InteractionResponseParams = serde_json::from_value(params)
                    .map_err(|error| RpcError::invalid_params(error.to_string()))?;
                transport.resolve(params.interaction_id, Ok(params.result))?;
                Ok(json!({ "ok": true }))
            }
        }),
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

    let router = router.method("provider.credential_status", {
        let ctx = ctx.clone();
        move |_params: Value| {
            let ctx = ctx.clone();
            async move {
                let credentials = ctx
                    .require::<dyn CredentialStore>()
                    .map_err(|error| RpcError::internal(error.to_string()))?;
                serde_json::to_value(credentials.status())
                    .map_err(|error| RpcError::internal(error.to_string()))
            }
        }
    });
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
    let router = router.method("provider.list_models", {
        let kinds = kind_registry.clone();
        let instances = instance_registry.clone();
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
                let credential = resolve_credential(&ctx, &instance)
                    .map_err(|error| RpcError::internal(error.to_string()))?;
                let http = ctx
                    .require::<dyn HttpClient>()
                    .map_err(|error| RpcError::internal(error.to_string()))?;
                let models = discover_models(&instance, kind.as_ref(), http, credential)
                    .await
                    .map_err(|error| RpcError::internal(error.to_string()))?;
                serde_json::to_value(models).map_err(|error| RpcError::internal(error.to_string()))
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
                let mut instance: ProviderInstance = serde_json::from_value(params)
                    .map_err(|error| RpcError::invalid_params(error.to_string()))?;
                if instance.id.trim().is_empty() {
                    return Err(RpcError::invalid_params("provider instance id is empty"));
                }
                let kind = kinds.get(&instance.kind).ok_or_else(|| {
                    RpcError::invalid_params(format!("unknown provider kind `{}`", instance.kind))
                })?;
                if let Err(error) = migrate_instance_config(&mut instance, kind.as_ref()) {
                    instance.disabled = true;
                    persist_provider_instance(&instances, &config_store, instance)?;
                    return Err(RpcError::invalid_params(error.to_string()));
                }
                instance.disabled = false;
                if instance.auth_ref.is_none() {
                    instance.auth_ref = Some(default_auth_ref(&instance.id));
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
                let mut instance: ProviderInstance = serde_json::from_value(params)
                    .map_err(|error| RpcError::invalid_params(error.to_string()))?;
                if instances.get(&instance.id).is_none() {
                    return Err(RpcError::invalid_params("unknown provider instance"));
                }
                let kind = kinds.get(&instance.kind).ok_or_else(|| {
                    RpcError::invalid_params(format!("unknown provider kind `{}`", instance.kind))
                })?;
                if let Err(error) = migrate_instance_config(&mut instance, kind.as_ref()) {
                    let mut disabled = instances
                        .get(&instance.id)
                        .unwrap_or_else(|| instance.clone());
                    disabled.disabled = true;
                    persist_provider_instance(&instances, &config_store, disabled)?;
                    return Err(RpcError::invalid_params(error.to_string()));
                }
                instance.disabled = false;
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
        let ctx = ctx.clone();
        move |params: Value| {
            let instances = instances.clone();
            let config_store = config_store.clone();
            let ctx = ctx.clone();
            async move {
                let params: InstanceIdParams = serde_json::from_value(params)
                    .map_err(|error| RpcError::invalid_params(error.to_string()))?;
                let instance = instances
                    .get(&params.id)
                    .ok_or_else(|| RpcError::invalid_params("unknown provider instance"))?;
                let mut credential_removed = true;
                let mut credential_error = None;
                if let Some(auth_ref) = instance.auth_ref.as_deref() {
                    let credentials = ctx
                        .require::<dyn CredentialStore>()
                        .map_err(|error| RpcError::internal(error.to_string()))?;
                    match credentials.delete(auth_ref) {
                        Ok(()) => {}
                        Err(error) => {
                            credential_removed = false;
                            credential_error = Some(error.to_string());
                        }
                    }
                }
                let removed = if let Some(store) = config_store {
                    store
                        .remove(&params.id)
                        .map_err(|error| RpcError::internal(error.to_string()))?
                } else {
                    instances.remove(&params.id).is_some()
                };
                Ok(json!({
                    "removed": removed,
                    "credentialRemoved": credential_removed,
                    "credentialError": credential_error,
                }))
            }
        }
    });
    let router = router.method("provider.set_default_instance", {
        let instances = instance_registry.clone();
        let config_store = config_store.clone();
        move |params: Value| {
            let instances = instances.clone();
            let config_store = config_store.clone();
            async move {
                let params: DefaultInstanceParams = serde_json::from_value(params)
                    .map_err(|error| RpcError::invalid_params(error.to_string()))?;
                if let Some(id) = params.id.as_deref() {
                    if instances.get(id).is_none() {
                        return Err(RpcError::invalid_params("unknown provider instance"));
                    }
                }
                let store = config_store
                    .ok_or_else(|| RpcError::internal("provider config store is unavailable"))?;
                store
                    .set_default_instance(params.id)
                    .map_err(|error| RpcError::internal(error.to_string()))?;
                Ok(json!({ "ok": true }))
            }
        }
    });
    let router = router.method("provider.set_default_model", {
        let instances = instance_registry.clone();
        let config_store = config_store.clone();
        move |params: Value| {
            let instances = instances.clone();
            let config_store = config_store.clone();
            async move {
                let params: DefaultModelParams = serde_json::from_value(params)
                    .map_err(|error| RpcError::invalid_params(error.to_string()))?;
                if instances.get(&params.id).is_none() {
                    return Err(RpcError::invalid_params("unknown provider instance"));
                }
                let store = config_store
                    .ok_or_else(|| RpcError::internal("provider config store is unavailable"))?;
                if !store
                    .set_default_model(&params.id, &params.model)
                    .map_err(|error| RpcError::internal(error.to_string()))?
                {
                    return Err(RpcError::invalid_params("unknown provider instance"));
                }
                Ok(json!({ "ok": true }))
            }
        }
    });
    let router = router.method("provider.authenticate_instance", {
        let instances = instance_registry.clone();
        let config_store = config_store.clone();
        let ctx = ctx.clone();
        move |params: Value| {
            let instances = instances.clone();
            let config_store = config_store.clone();
            let ctx = ctx.clone();
            async move {
                let params: AuthInstanceParams = serde_json::from_value(params)
                    .map_err(|error| RpcError::invalid_params(error.to_string()))?;
                if params.secret.trim().is_empty() {
                    return Err(RpcError::invalid_params("credential is empty"));
                }
                let mut instance = instances
                    .get(&params.id)
                    .ok_or_else(|| RpcError::invalid_params("unknown provider instance"))?;
                let auth_ref = instance
                    .auth_ref
                    .clone()
                    .unwrap_or_else(|| default_auth_ref(&instance.id));
                let credentials = ctx
                    .require::<dyn CredentialStore>()
                    .map_err(|error| RpcError::internal(error.to_string()))?;
                credentials
                    .write(&auth_ref, params.secret.as_bytes())
                    .map_err(|error| RpcError::internal(error.to_string()))?;
                instance.auth_ref = Some(auth_ref.clone());
                if let Some(store) = config_store {
                    store
                        .upsert(instance.clone())
                        .map_err(|error| RpcError::internal(error.to_string()))?;
                } else {
                    instances.upsert(instance);
                }
                Ok(json!({ "ok": true, "authRef": auth_ref }))
            }
        }
    });
    let router = router.method("provider.remove_credential", {
        let instances = instance_registry.clone();
        let ctx = ctx.clone();
        move |params: Value| {
            let instances = instances.clone();
            let ctx = ctx.clone();
            async move {
                let params: InstanceIdParams = serde_json::from_value(params)
                    .map_err(|error| RpcError::invalid_params(error.to_string()))?;
                let instance = instances
                    .get(&params.id)
                    .ok_or_else(|| RpcError::invalid_params("unknown provider instance"))?;
                let Some(auth_ref) = instance.auth_ref else {
                    return Ok(json!({ "removed": false }));
                };
                let credentials = ctx
                    .require::<dyn CredentialStore>()
                    .map_err(|error| RpcError::internal(error.to_string()))?;
                credentials
                    .delete(&auth_ref)
                    .map_err(|error| RpcError::internal(error.to_string()))?;
                Ok(json!({ "removed": true }))
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

fn add_agent_host_methods(router: Router, host: Arc<AgentHostService>) -> Router {
    let router = router.method("session.create", {
        let host = host.clone();
        move |params: Value| {
            let host = host.clone();
            async move {
                let request: alpha_module_agent_host::CreateSessionRequest =
                    serde_json::from_value(params)
                        .map_err(|error| RpcError::invalid_params(error.to_string()))?;
                let created = host.create_session(request).map_err(agent_host_rpc_error)?;
                serde_json::to_value(created).map_err(|error| RpcError::internal(error.to_string()))
            }
        }
    });
    let router = router.method("session.submit", {
        let host = host.clone();
        move |params: Value| {
            let host = host.clone();
            async move {
                let params: SubmitParams = serde_json::from_value(params)
                    .map_err(|error| RpcError::invalid_params(error.to_string()))?;
                let turn_id = host
                    .submit(params.session_id, params.prompt)
                    .map_err(agent_host_rpc_error)?;
                Ok(json!({ "turnId": turn_id }))
            }
        }
    });
    let router = router.method("session.cancel", {
        let host = host.clone();
        move |params: Value| {
            let host = host.clone();
            async move {
                let params: CancelParams = serde_json::from_value(params)
                    .map_err(|error| RpcError::invalid_params(error.to_string()))?;
                host.cancel(params.session_id, params.turn_id)
                    .await
                    .map_err(agent_host_rpc_error)?;
                Ok(json!({ "ok": true }))
            }
        }
    });
    let router = router.method("session.steer", {
        let host = host.clone();
        move |params: Value| {
            let host = host.clone();
            async move {
                let params: SteerParams = serde_json::from_value(params)
                    .map_err(|error| RpcError::invalid_params(error.to_string()))?;
                host.steer(params.session_id, params.text)
                    .await
                    .map_err(agent_host_rpc_error)?;
                Ok(json!({ "ok": true }))
            }
        }
    });
    let router = router.method("session.confirm_steers", {
        let host = host.clone();
        move |params: Value| {
            let host = host.clone();
            async move {
                let params: ConfirmSteersParams = serde_json::from_value(params)
                    .map_err(|error| RpcError::invalid_params(error.to_string()))?;
                let discarded = host
                    .confirm_steers(params.session_id, params.accept)
                    .await
                    .map_err(agent_host_rpc_error)?;
                Ok(json!({ "ok": true, "steers": discarded }))
            }
        }
    });
    let router = router.method("session.attach", {
        let host = host.clone();
        move |params: Value| {
            let host = host.clone();
            async move {
                let params: AttachHostParams = serde_json::from_value(params)
                    .map_err(|error| RpcError::invalid_params(error.to_string()))?;
                let attached = host
                    .attach(params.session_id, params.after_seq.unwrap_or(0))
                    .map_err(agent_host_rpc_error)?;
                serde_json::to_value(attached)
                    .map_err(|error| RpcError::internal(error.to_string()))
            }
        }
    });
    let router = router.method("session.detach", {
        let host = host.clone();
        move |params: Value| {
            let host = host.clone();
            async move {
                let params: SessionIdParams = serde_json::from_value(params)
                    .map_err(|error| RpcError::invalid_params(error.to_string()))?;
                host.detach(params.session_id)
                    .map_err(agent_host_rpc_error)?;
                Ok(json!({ "ok": true }))
            }
        }
    });
    let router = router.method("session.delete", {
        let host = host.clone();
        move |params: Value| {
            let host = host.clone();
            async move {
                let params: SessionIdParams = serde_json::from_value(params)
                    .map_err(|error| RpcError::invalid_params(error.to_string()))?;
                host.delete_session(params.session_id)
                    .await
                    .map_err(agent_host_rpc_error)?;
                Ok(json!({ "ok": true }))
            }
        }
    });
    let router = router.method("session.set_mode", {
        let host = host.clone();
        move |params: Value| {
            let host = host.clone();
            async move {
                let params: SetModeParams = serde_json::from_value(params)
                    .map_err(|error| RpcError::invalid_params(error.to_string()))?;
                host.set_mode(params.session_id, params.mode)
                    .await
                    .map_err(agent_host_rpc_error)?;
                Ok(json!({ "ok": true }))
            }
        }
    });
    let router = router.method("session.set_route", {
        let host = host.clone();
        move |params: Value| {
            let host = host.clone();
            async move {
                let params: SetRouteParams = serde_json::from_value(params)
                    .map_err(|error| RpcError::invalid_params(error.to_string()))?;
                host.set_route(
                    params.session_id,
                    ModelRoute::new(params.provider, params.model, params.reasoning),
                )
                .await
                .map_err(agent_host_rpc_error)?;
                Ok(json!({ "ok": true }))
            }
        }
    });
    let router = router.method("session.set_capabilities", {
        let host = host.clone();
        move |params: Value| {
            let host = host.clone();
            async move {
                let params: SetCapabilitiesParams = serde_json::from_value(params)
                    .map_err(|error| RpcError::invalid_params(error.to_string()))?;
                host.set_capabilities(params.session_id, params.provider_tools)
                    .await
                    .map_err(agent_host_rpc_error)?;
                Ok(json!({ "ok": true }))
            }
        }
    });
    router.method("session.resume", {
        let host = host.clone();
        move |params: Value| {
            let host = host.clone();
            async move {
                let params: SessionIdParams = serde_json::from_value(params)
                    .map_err(|error| RpcError::invalid_params(error.to_string()))?;
                let resumed = host
                    .resume_session(params.session_id)
                    .map_err(agent_host_rpc_error)?;
                serde_json::to_value(resumed).map_err(|error| RpcError::internal(error.to_string()))
            }
        }
    })
}

fn default_auth_ref(instance_id: &str) -> String {
    format!("provider/{instance_id}/api-key")
}

fn agent_host_rpc_error(error: AgentHostError) -> RpcError {
    match error {
        AgentHostError::SessionBusy(_) => RpcError::new(-32010, "session_busy"),
        AgentHostError::SessionNotFound(_) => RpcError::new(-32011, "session_not_found"),
        other => RpcError::internal(other.to_string()),
    }
}

fn migrate_instance_config(
    instance: &mut ProviderInstance,
    kind: &dyn alpha_provider_api::ProviderKind,
) -> Result<(), RpcError> {
    let current = kind.config_version();
    if instance.config_version == current {
        return Ok(());
    }
    instance.config = kind
        .migrate_config(instance.config_version, instance.config.clone())
        .map_err(|error| RpcError::invalid_params(error.to_string()))?;
    instance.config_version = current;
    Ok(())
}

fn persist_provider_instance(
    instances: &ProviderInstanceRegistry,
    store: &Option<Arc<dyn ProviderConfigStore>>,
    instance: ProviderInstance,
) -> Result<(), RpcError> {
    if let Some(store) = store {
        store
            .upsert(instance)
            .map_err(|error| RpcError::internal(error.to_string()))
    } else {
        instances.upsert(instance);
        Ok(())
    }
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

#[derive(Debug, Deserialize)]
struct DefaultModelParams {
    id: String,
    model: String,
}

#[derive(Debug, Deserialize)]
struct AuthInstanceParams {
    id: String,
    secret: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InteractionResponseParams {
    interaction_id: Id,
    result: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubmitParams {
    session_id: SessionId,
    prompt: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CancelParams {
    session_id: SessionId,
    turn_id: alpha_schema::TurnId,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SteerParams {
    session_id: SessionId,
    text: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfirmSteersParams {
    session_id: SessionId,
    accept: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AttachHostParams {
    session_id: SessionId,
    #[serde(default)]
    after_seq: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionIdParams {
    session_id: SessionId,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetModeParams {
    session_id: SessionId,
    mode: PermissionMode,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetRouteParams {
    session_id: SessionId,
    provider: String,
    model: String,
    #[serde(default)]
    reasoning: alpha_schema::ReasoningLevel,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetCapabilitiesParams {
    session_id: SessionId,
    #[serde(default)]
    provider_tools: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoopTransport;

    impl OutboundTransport for NoopTransport {
        fn notify(&self, _method: String, _params: Value) -> Result<(), RpcError> {
            Ok(())
        }

        fn request(
            &self,
            _method: String,
            _params: Value,
        ) -> BoxFuture<'static, Result<Value, RpcError>> {
            Box::pin(async { Ok(Value::Null) })
        }
    }

    struct PendingTransport;

    impl OutboundTransport for PendingTransport {
        fn notify(&self, _method: String, _params: Value) -> Result<(), RpcError> {
            Ok(())
        }

        fn request(
            &self,
            _method: String,
            _params: Value,
        ) -> BoxFuture<'static, Result<Value, RpcError>> {
            Box::pin(futures::future::pending())
        }
    }

    #[test]
    fn locations_are_shared_by_root_and_cwd() {
        let workspace = tempfile::tempdir().expect("workspace");
        std::fs::create_dir(workspace.path().join("nested")).expect("nested directory");
        let factory = AppSessionRuntimeFactory {
            root: Ctx::root(),
            transport: Arc::new(NoopTransport),
            locations: Mutex::new(HashMap::new()),
        };
        let binding = WorkspaceRef {
            root: workspace.path().display().to_string(),
            cwd: workspace.path().display().to_string(),
        };
        let first = factory.location_context(&binding).expect("first location");
        let second = factory.location_context(&binding).expect("second location");
        let first_fs = first
            .require::<dyn alpha_module_fs::FileSystem>()
            .expect("first filesystem");
        let second_fs = second
            .require::<dyn alpha_module_fs::FileSystem>()
            .expect("second filesystem");
        assert!(Arc::ptr_eq(&first_fs, &second_fs));

        let other = WorkspaceRef {
            root: workspace.path().display().to_string(),
            cwd: workspace.path().join("nested").display().to_string(),
        };
        let third = factory.location_context(&other).expect("third location");
        let third_fs = third
            .require::<dyn alpha_module_fs::FileSystem>()
            .expect("third filesystem");
        assert!(!Arc::ptr_eq(&first_fs, &third_fs));
    }

    #[tokio::test]
    async fn protocol_approval_times_out_to_deny() {
        let channel = ProtocolApprovalChannel {
            session_id: SessionId::new(),
            transport: Arc::new(PendingTransport),
            timeout: Duration::from_millis(1),
        };
        let decision = channel
            .decide(ApprovalPrompt {
                request: alpha_module_permission::PermissionRequest::new(
                    "fs.write",
                    ["workspace/main.rs".to_owned()],
                ),
                reason: "test".to_owned(),
                protected: false,
            })
            .await;
        assert_eq!(decision, ApprovalDecision::Timeout);
    }
}

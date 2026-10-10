//! Application bootstrap for the `alpha` binary.

include!(concat!(env!("OUT_DIR"), "/alpha_modules.rs"));

use std::io::{BufRead, Write};
use std::path::Path;
use std::sync::Arc;

use alpha_kernel::{ActivateError, Ctx, Module};
use alpha_module_fs::{FsConfig, FsLocalModule};
use alpha_module_http::{HttpConfig, HttpLocalModule};
use alpha_module_loop::{AgentLoop, LoopConfig, LoopModule, ModelProvider, ProviderRegistry};
use alpha_module_permission::{
    ApprovalChannel, ApprovalDecision, ApprovalPrompt, AuditSink, CollectingAuditSink,
    PermissionConfig, PermissionModule,
};
use alpha_module_process::{ProcessConfig, ProcessLocalModule};
use alpha_module_provider_openai::{OpenAiConfig, OpenAiProviderModule, PROVIDER_NAME};
use alpha_module_session::{
    SessionError, SessionEvent, SessionHeader, SessionState, SessionStore, SessionWriter,
    WorkspaceRef,
};
use alpha_module_tools::{ToolsConfig, ToolsModule};
use alpha_protocol::{
    FramingError, Id, Message, Response, Router, RpcError, read_message, write_message,
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
    pub agent: Arc<AgentLoop>,
    /// Audit records collected during the run.
    pub audit: Arc<CollectingAuditSink>,
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
    FsLocalModule.activate(
        ctx,
        &FsConfig {
            root: workspace.to_path_buf(),
        },
    )?;
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
    Ok(())
}

fn activate_loop(ctx: &Ctx, provider: &str, model: &str) -> Result<Arc<AgentLoop>, RunError> {
    LoopModule.activate(
        ctx,
        &LoopConfig {
            provider: provider.to_owned(),
            model: model.to_owned(),
        },
    )?;
    ctx.require::<AgentLoop>()
        .map_err(|error| RunError::Service(error.to_string()))
}

/// Run one headless turn and record it in a session.
pub async fn run_prompt(
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
    let state = SessionState::new(&header);
    let outcome = composition
        .agent
        .run_turn(&mut writer, &state, &options.prompt)
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
    let reader = store.open_read(options.session_id)?;
    let state = reader.state();
    drop(reader);

    let mut writer = store.open_write(options.session_id)?;
    let repairs = writer.repairs().len();
    let prompt = options
        .prompt
        .as_deref()
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty());
    let assistant_text = match prompt {
        Some(prompt) => Some(
            composition
                .agent
                .run_turn(&mut writer, &state, prompt)
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

/// Errors from the stdio server.
#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    /// Protocol framing error.
    #[error(transparent)]
    Framing(#[from] FramingError),
    /// Failed to start the async runtime.
    #[error("failed to start async runtime: {0}")]
    Runtime(std::io::Error),
}

/// Serve JSON-RPC over stdio using the default session store.
pub fn serve_stdio(store: SessionStore) -> Result<(), ServeError> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut input = stdin.lock();
    let mut output = stdout.lock();
    serve_with(&mut input, &mut output, store)
}

/// Serve JSON-RPC over an arbitrary reader and writer.
pub fn serve_with<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    store: SessionStore,
) -> Result<(), ServeError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(ServeError::Runtime)?;
    let router = build_router(store);

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
    let store = Arc::new(store);
    Router::new()
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

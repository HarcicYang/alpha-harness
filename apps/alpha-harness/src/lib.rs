//! Application bootstrap for the `alpha` binary.

include!(concat!(env!("OUT_DIR"), "/alpha_modules.rs"));

use std::io::{BufRead, Write};
use std::path::Path;
use std::sync::Arc;

use alpha_kernel::{ActivateError, Ctx, Module};
use alpha_module_fs::{FsConfig, FsLocalModule};
use alpha_module_http::{HttpConfig, HttpLocalModule};
use alpha_module_loop::{AgentLoop, LoopConfig, LoopModule, ModelProvider, ProviderRegistry};
use alpha_module_permission::{PermissionConfig, PermissionModule};
use alpha_module_process::{ProcessConfig, ProcessLocalModule};
use alpha_module_provider_openai::{OpenAiConfig, OpenAiProviderModule, PROVIDER_NAME};
use alpha_module_session::{SessionError, SessionHeader, SessionState, SessionStore, WorkspaceRef};
use alpha_module_tools::{ToolsConfig, ToolsModule};
use alpha_protocol::{
    FramingError, Id, Message, Response, Router, RpcError, read_message, write_message,
};
use alpha_schema::{PermissionMode, SessionId};
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

/// Compose the production runtime with the OpenAI-compatible provider.
pub fn compose_openai_runtime(
    workspace: &Path,
    mode: PermissionMode,
    model: &str,
    base_url: &str,
    api_key: &str,
) -> Result<(Ctx, Arc<AgentLoop>), RunError> {
    let ctx = Ctx::root();
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
    activate_core_modules(&ctx, workspace, mode)?;
    let agent = activate_loop(&ctx, PROVIDER_NAME, model)?;
    Ok((ctx, agent))
}

/// Compose a runtime with an injected provider, used by tests.
pub fn compose_with_provider(
    workspace: &Path,
    mode: PermissionMode,
    model: &str,
    provider_name: &str,
    provider: Arc<dyn ModelProvider>,
) -> Result<(Ctx, Arc<AgentLoop>), RunError> {
    let ctx = Ctx::root();
    let registry = Arc::new(ProviderRegistry::new());
    registry
        .register(provider_name, provider)
        .map_err(|error| RunError::Service(error.to_string()))?;
    ctx.provide::<ProviderRegistry>(registry)
        .map_err(|error| RunError::Service(error.to_string()))?;
    activate_core_modules(&ctx, workspace, mode)?;
    let agent = activate_loop(&ctx, provider_name, model)?;
    Ok((ctx, agent))
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
    agent: &AgentLoop,
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
    let outcome = agent
        .run_turn(&mut writer, &state, &options.prompt)
        .await
        .map_err(|error| RunError::Loop(error.to_string()))?;
    Ok(RunReport {
        session_id,
        assistant_text: outcome.assistant_text,
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
    Router::new()
        .method("alpha.ping", |_params: Value| async move {
            Ok::<Value, RpcError>(json!({
                "pong": true,
                "version": env!("CARGO_PKG_VERSION"),
            }))
        })
        .method("session.list", move |_params: Value| {
            let store = store.clone();
            async move {
                let headers = store
                    .list()
                    .map_err(|error| RpcError::internal(error.to_string()))?;
                serde_json::to_value(headers).map_err(|error| RpcError::internal(error.to_string()))
            }
        })
}

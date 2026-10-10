//! Application bootstrap for the `alpha` binary.

use std::io::{BufRead, Write};

use alpha_module_session::SessionStore;
use alpha_protocol::{
    FramingError, Id, Message, Response, Router, RpcError, read_message, write_message,
};
use serde_json::{Value, json};

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

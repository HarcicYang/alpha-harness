//! Outbound transport used by server-side handlers.

use futures::future::BoxFuture;
use serde_json::Value;

use crate::RpcError;

/// Sends notifications and server-initiated requests over one connection.
///
/// Implementations are supplied by the transport owner. Handlers depend on
/// this trait instead of owning a reader or writer directly.
pub trait OutboundTransport: Send + Sync + 'static {
    /// Send a notification. Delivery failures are operational errors.
    fn notify(&self, method: String, params: Value) -> Result<(), RpcError>;

    /// Send a server-initiated request and await its response.
    fn request(&self, method: String, params: Value)
    -> BoxFuture<'static, Result<Value, RpcError>>;
}

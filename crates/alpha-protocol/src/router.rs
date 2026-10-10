//! Method routing for requests and notifications.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

use futures::future::BoxFuture;
use serde_json::Value;

use crate::message::{Id, Message, Notification, Request, Response, RpcError};

type Handler = Arc<dyn Fn(Value) -> BoxFuture<'static, Result<Value, RpcError>> + Send + Sync>;

/// Routes JSON-RPC methods to asynchronous handlers.
#[derive(Default)]
pub struct Router {
    handlers: HashMap<String, Handler>,
}

impl Router {
    /// Create an empty router.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a method handler.
    pub fn method<F, Fut>(mut self, name: impl Into<String>, handler: F) -> Self
    where
        F: Fn(Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Value, RpcError>> + Send + 'static,
    {
        let handler = Arc::new(
            move |params: Value| -> BoxFuture<'static, Result<Value, RpcError>> {
                Box::pin(handler(params))
            },
        );
        self.handlers.insert(name.into(), handler);
        self
    }

    /// Whether a method is registered.
    pub fn contains(&self, method: &str) -> bool {
        self.handlers.contains_key(method)
    }

    /// Handle one incoming message.
    ///
    /// Requests produce a response. Notifications run their handler and
    /// produce no response. Responses are ignored.
    pub async fn handle(&self, message: Message) -> Option<Message> {
        match message {
            Message::Request(request) => {
                Some(Message::Response(self.handle_request(request).await))
            }
            Message::Notification(notification) => {
                self.handle_notification(notification).await;
                None
            }
            Message::Response(_) => None,
        }
    }

    async fn handle_request(&self, request: Request) -> Response {
        let Request {
            id, method, params, ..
        } = request;
        match self.handlers.get(&method) {
            Some(handler) => match handler(params.unwrap_or(Value::Null)).await {
                Ok(result) => Response::success(id, result),
                Err(error) => Response::failure(id, error),
            },
            None => Response::failure(id, RpcError::method_not_found(&method)),
        }
    }

    async fn handle_notification(&self, notification: Notification) {
        let Notification { method, params, .. } = notification;
        if let Some(handler) = self.handlers.get(&method) {
            let _ = handler(params.unwrap_or(Value::Null)).await;
        }
    }
}

/// Convenience for building an id from a number.
pub fn numeric_id(value: i64) -> Id {
    Id::Number(value)
}

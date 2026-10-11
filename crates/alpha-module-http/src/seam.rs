//! HTTP seam types.

use std::collections::BTreeMap;

use futures::future::BoxFuture;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};

/// One HTTP request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpRequest {
    /// HTTP method.
    pub method: String,
    /// Absolute request URL.
    pub url: String,
    /// Request headers.
    pub headers: BTreeMap<String, String>,
    /// Request body.
    pub body: Vec<u8>,
}

impl HttpRequest {
    /// Build a GET request.
    pub fn get(url: impl Into<String>) -> Self {
        Self {
            method: "GET".to_owned(),
            url: url.into(),
            headers: BTreeMap::new(),
            body: Vec::new(),
        }
    }

    /// Build a POST request with a body.
    pub fn post(url: impl Into<String>, body: Vec<u8>) -> Self {
        Self {
            method: "POST".to_owned(),
            url: url.into(),
            headers: BTreeMap::new(),
            body,
        }
    }

    /// Add a header.
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(name.into(), value.into());
        self
    }
}

/// One HTTP response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpResponse {
    /// HTTP status code.
    pub status: u16,
    /// Response headers.
    pub headers: BTreeMap<String, String>,
    /// Response body.
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// Whether the status is 2xx.
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// Case-insensitive header lookup.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// One streaming HTTP response.
pub struct HttpResponseStream {
    /// HTTP status code.
    pub status: u16,
    /// Response headers.
    pub headers: BTreeMap<String, String>,
    /// Response body chunks.
    pub body: BoxStream<'static, Result<Vec<u8>, HttpError>>,
}

/// HTTP seam errors.
#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    /// The request could not be completed.
    #[error("http request failed: {0}")]
    Request(String),
}

/// HTTP contract.
pub trait HttpClient: Send + Sync + 'static {
    /// Perform one request.
    fn request(&self, request: HttpRequest) -> BoxFuture<'_, Result<HttpResponse, HttpError>>;

    /// Perform one request and expose the response body as chunks.
    ///
    /// The default implementation buffers the complete response and yields it
    /// as one chunk.
    fn request_stream(
        &self,
        request: HttpRequest,
    ) -> BoxFuture<'_, Result<HttpResponseStream, HttpError>> {
        Box::pin(async move {
            let response = self.request(request).await?;
            let body = futures::stream::once(async move { Ok(response.body) });
            Ok(HttpResponseStream {
                status: response.status,
                headers: response.headers,
                body: Box::pin(body),
            })
        })
    }
}

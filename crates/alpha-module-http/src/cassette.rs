//! Deterministic record and replay for HTTP traffic.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};

use crate::seam::{HttpClient, HttpError, HttpRequest, HttpResponse};

/// One recorded request and response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CassetteEntry {
    /// Request method.
    pub method: String,
    /// Request URL.
    pub url: String,
    /// Request body.
    pub request_body: Vec<u8>,
    /// Response status.
    pub status: u16,
    /// Response headers.
    pub headers: BTreeMap<String, String>,
    /// Response body.
    pub response_body: Vec<u8>,
}

impl CassetteEntry {
    fn matches(&self, request: &HttpRequest) -> bool {
        self.method == request.method
            && self.url == request.url
            && self.request_body == request.body
    }

    fn response(&self) -> HttpResponse {
        HttpResponse {
            status: self.status,
            headers: self.headers.clone(),
            body: self.response_body.clone(),
        }
    }
}

/// A recorded set of HTTP exchanges.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cassette {
    /// Recorded entries in request order.
    pub entries: Vec<CassetteEntry>,
}

/// Replays responses from a cassette.
pub struct ReplayHttpClient {
    entries: Vec<CassetteEntry>,
    used: Mutex<Vec<bool>>,
}

impl ReplayHttpClient {
    /// Create a replay client over a cassette.
    pub fn new(cassette: Cassette) -> Self {
        let used = vec![false; cassette.entries.len()];
        Self {
            entries: cassette.entries,
            used: Mutex::new(used),
        }
    }
}

impl HttpClient for ReplayHttpClient {
    fn request(&self, request: HttpRequest) -> BoxFuture<'_, Result<HttpResponse, HttpError>> {
        Box::pin(async move {
            let mut used = self.used.lock().expect("cassette lock poisoned");
            for (index, entry) in self.entries.iter().enumerate() {
                if !used[index] && entry.matches(&request) {
                    used[index] = true;
                    return Ok(entry.response());
                }
            }
            Err(HttpError::Request(format!(
                "no cassette entry for {} {}",
                request.method, request.url
            )))
        })
    }
}

/// Records every request made through an inner client.
pub struct RecordingHttpClient {
    inner: Arc<dyn HttpClient>,
    entries: Mutex<Vec<CassetteEntry>>,
}

impl RecordingHttpClient {
    /// Wrap an inner client.
    pub fn new(inner: Arc<dyn HttpClient>) -> Self {
        Self {
            inner,
            entries: Mutex::new(Vec::new()),
        }
    }

    /// Snapshot the recorded exchanges.
    pub fn cassette(&self) -> Cassette {
        Cassette {
            entries: self.entries.lock().expect("cassette lock poisoned").clone(),
        }
    }
}

impl HttpClient for RecordingHttpClient {
    fn request(&self, request: HttpRequest) -> BoxFuture<'_, Result<HttpResponse, HttpError>> {
        Box::pin(async move {
            let response = self.inner.request(request.clone()).await?;
            self.entries
                .lock()
                .expect("cassette lock poisoned")
                .push(CassetteEntry {
                    method: request.method,
                    url: request.url,
                    request_body: request.body,
                    status: response.status,
                    headers: response.headers.clone(),
                    response_body: response.body.clone(),
                });
            Ok(response)
        })
    }
}

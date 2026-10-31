//! Local HTTP driver backed by `ureq`.

use std::collections::BTreeMap;
use std::io::Read;
use std::time::Duration;

use alpha_http_api::seam::{HttpClient, HttpError, HttpRequest, HttpResponse, HttpResponseStream};
use futures::future::BoxFuture;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const READ_TIMEOUT: Duration = Duration::from_secs(300);
const WRITE_TIMEOUT: Duration = Duration::from_secs(60);

/// HTTP seam backed by the local machine.
#[derive(Clone, Debug, Default)]
pub struct LocalHttpClient;

impl LocalHttpClient {
    /// Create a local client.
    pub fn new() -> Self {
        Self
    }
}

impl HttpClient for LocalHttpClient {
    fn request(&self, request: HttpRequest) -> BoxFuture<'_, Result<HttpResponse, HttpError>> {
        Box::pin(async move {
            tokio::task::spawn_blocking(move || execute(request))
                .await
                .map_err(|error| HttpError::Request(format!("http task failed: {error}")))?
        })
    }

    fn request_stream(
        &self,
        request: HttpRequest,
    ) -> BoxFuture<'_, Result<HttpResponseStream, HttpError>> {
        Box::pin(async move {
            let (head_tx, head_rx) = tokio::sync::oneshot::channel();
            let (body_tx, body_rx) = tokio::sync::mpsc::channel(16);

            tokio::task::spawn_blocking(move || {
                let agent = ureq::AgentBuilder::new()
                    .timeout_connect(CONNECT_TIMEOUT)
                    .timeout_read(READ_TIMEOUT)
                    .timeout_write(WRITE_TIMEOUT)
                    .build();
                let mut call = agent.request(&request.method, &request.url);
                for (name, value) in &request.headers {
                    call = call.set(name, value);
                }
                let result = if request.body.is_empty() {
                    call.call()
                } else {
                    call.send_bytes(&request.body)
                };
                let response = match result {
                    Ok(response) => response,
                    Err(ureq::Error::Status(_, response)) => response,
                    Err(ureq::Error::Transport(error)) => {
                        let _ = head_tx.send(Err(HttpError::Request(error.to_string())));
                        return;
                    }
                };
                let head = ResponseHead::from_response(&response);
                if head_tx.send(Ok(head)).is_err() {
                    return;
                }
                let mut reader = response.into_reader();
                let mut buffer = [0u8; 8192];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(read) => {
                            if body_tx.blocking_send(Ok(buffer[..read].to_vec())).is_err() {
                                return;
                            }
                        }
                        Err(error) => {
                            let _ =
                                body_tx.blocking_send(Err(HttpError::Request(error.to_string())));
                            return;
                        }
                    }
                }
            });

            let head = head_rx
                .await
                .map_err(|error| HttpError::Request(format!("http task failed: {error}")))??;
            let body = futures::stream::unfold(body_rx, |mut receiver| async move {
                receiver.recv().await.map(|chunk| (chunk, receiver))
            });
            Ok(HttpResponseStream {
                status: head.status,
                headers: head.headers,
                body: Box::pin(body),
            })
        })
    }
}

struct ResponseHead {
    status: u16,
    headers: BTreeMap<String, String>,
}

impl ResponseHead {
    fn from_response(response: &ureq::Response) -> Self {
        let status = response.status();
        let headers: BTreeMap<String, String> = response
            .headers_names()
            .into_iter()
            .filter_map(|name| {
                response
                    .header(&name)
                    .map(|value| (name.clone(), value.to_owned()))
            })
            .collect();
        Self { status, headers }
    }
}

fn execute(request: HttpRequest) -> Result<HttpResponse, HttpError> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(CONNECT_TIMEOUT)
        .timeout_read(READ_TIMEOUT)
        .timeout_write(WRITE_TIMEOUT)
        .build();
    let mut call = agent.request(&request.method, &request.url);
    for (name, value) in &request.headers {
        call = call.set(name, value);
    }
    let result = if request.body.is_empty() {
        call.call()
    } else {
        call.send_bytes(&request.body)
    };
    match result {
        Ok(response) => Ok(to_response(response)),
        // Non-2xx responses are still valid HTTP responses.
        Err(ureq::Error::Status(_, response)) => Ok(to_response(response)),
        Err(ureq::Error::Transport(error)) => Err(HttpError::Request(error.to_string())),
    }
}

fn to_response(response: ureq::Response) -> HttpResponse {
    let status = response.status();
    let headers: BTreeMap<String, String> = response
        .headers_names()
        .into_iter()
        .filter_map(|name| {
            response
                .header(&name)
                .map(|value| (name.clone(), value.to_owned()))
        })
        .collect();
    let mut body = Vec::new();
    let mut reader = response.into_reader();
    let _ = reader.read_to_end(&mut body);
    HttpResponse {
        status,
        headers,
        body,
    }
}

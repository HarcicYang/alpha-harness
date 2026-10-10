use std::sync::Arc;

use alpha_kernel::{Ctx, Module};
use alpha_module_http::{
    HttpClient, HttpConfig, HttpLocalModule, HttpRequest, LocalHttpClient, RecordingHttpClient,
    ReplayHttpClient,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

async fn one_shot_server() -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("addr");
    let handle = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept");
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            let read = socket.read(&mut chunk).await.expect("read");
            if read == 0 {
                break;
            }
            buffer.extend_from_slice(&chunk[..read]);
            if let Some(header_end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&buffer[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let lower = line.to_ascii_lowercase();
                        lower
                            .strip_prefix("content-length:")
                            .and_then(|value| value.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                if buffer.len() >= header_end + 4 + content_length {
                    break;
                }
            }
        }
        let request = String::from_utf8_lossy(&buffer);
        assert!(
            request.starts_with("POST /v1/chat HTTP/1.1"),
            "unexpected request: {request}"
        );
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\nok",
            )
            .await
            .expect("write");
    });
    (format!("http://{address}"), handle)
}

#[tokio::test]
async fn local_driver_sends_requests_and_reads_responses() {
    let (base, server) = one_shot_server().await;
    let client = LocalHttpClient::new();
    let response = client
        .request(
            HttpRequest::post(format!("{base}/v1/chat"), b"hello".to_vec())
                .with_header("content-type", "text/plain"),
        )
        .await
        .expect("request");

    assert_eq!(response.status, 200);
    assert_eq!(response.body, b"ok");
    assert_eq!(response.header("content-type"), Some("text/plain"));
    server.await.expect("server");
}

#[tokio::test]
async fn cassettes_record_once_and_replay_without_network() {
    let (base, server) = one_shot_server().await;
    let recorder = RecordingHttpClient::new(Arc::new(LocalHttpClient::new()));
    let request = HttpRequest::post(format!("{base}/v1/chat"), b"hello".to_vec());

    let response = recorder.request(request.clone()).await.expect("record");
    assert_eq!(response.status, 200);
    server.await.expect("server");

    let cassette = recorder.cassette();
    assert_eq!(cassette.entries.len(), 1);

    let replay = ReplayHttpClient::new(cassette);
    let replayed = replay.request(request).await.expect("replay");
    assert_eq!(replayed.body, b"ok");

    let error = replay
        .request(HttpRequest::get("http://example.invalid/health"))
        .await
        .expect_err("cache miss");
    assert!(error.to_string().contains("no cassette entry"));
}

#[test]
fn module_provides_the_seam() {
    let ctx = Ctx::root();
    HttpLocalModule
        .activate(&ctx, &HttpConfig {})
        .expect("activate");
    assert!(ctx.get::<dyn HttpClient>().is_some());
    ctx.dispose();
    assert!(ctx.get::<dyn HttpClient>().is_none());
}

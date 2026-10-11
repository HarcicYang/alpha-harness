use std::collections::BTreeMap;
use std::io::BufReader;
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use alpha_front_client::{ConnectionEvent, ProtocolConnection};
use alpha_harness::{CoreRuntimeOptions, serve_protocol_with_options};
use alpha_module_credential::{CredentialStore, MemoryCredentialStore};
use alpha_module_http::{HttpClient, HttpError, HttpRequest, HttpResponse, HttpResponseStream};
use alpha_module_session::SessionStore;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use serde_json::{Value, json};

struct FakeOpenAiHttp {
    requests: Mutex<Vec<HttpRequest>>,
}

impl HttpClient for FakeOpenAiHttp {
    fn request(&self, _request: HttpRequest) -> BoxFuture<'_, Result<HttpResponse, HttpError>> {
        Box::pin(async move {
            Ok(HttpResponse {
                status: 200,
                headers: BTreeMap::new(),
                body: Vec::new(),
            })
        })
    }

    fn request_stream(
        &self,
        request: HttpRequest,
    ) -> BoxFuture<'_, Result<HttpResponseStream, HttpError>> {
        self.requests
            .lock()
            .expect("request lock poisoned")
            .push(request);
        Box::pin(async move {
            let chunks = vec![
                b"data: {\"choices\":[{\"delta\":{\"content\":\"hello \"}}]}\n\n".to_vec(),
                b"data: {\"choices\":[{\"delta\":{\"content\":\"from fake\"}}]}\n\n".to_vec(),
                b"data: [DONE]\n\n".to_vec(),
            ];
            let body = futures::stream::iter(
                chunks
                    .into_iter()
                    .map(Ok)
                    .collect::<Vec<Result<Vec<u8>, HttpError>>>(),
            );
            Ok(HttpResponseStream {
                status: 200,
                headers: BTreeMap::new(),
                body: Box::pin(body) as BoxStream<'static, Result<Vec<u8>, HttpError>>,
            })
        })
    }
}

#[test]
fn core_protocol_runs_a_turn_through_a_configured_provider() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let client = TcpStream::connect(address).expect("client connect");
    let (server, _) = listener.accept().expect("server accept");
    let server_reader = BufReader::new(server.try_clone().expect("clone server"));
    let state = tempfile::tempdir().expect("state");
    let config = tempfile::tempdir().expect("config");
    let workspace = tempfile::tempdir().expect("workspace");
    let credential: Arc<dyn CredentialStore> = Arc::new(MemoryCredentialStore::default());
    let http = Arc::new(FakeOpenAiHttp {
        requests: Mutex::new(Vec::new()),
    });
    let server_http = http.clone();
    let server_thread = thread::spawn(move || {
        let mut server_reader = server_reader;
        serve_protocol_with_options(
            &mut server_reader,
            server,
            SessionStore::new(state.path()),
            CoreRuntimeOptions {
                credential: Some(credential),
                http: Some(server_http),
                provider_config_path: Some(config.path().join("config.toml")),
            },
        )
        .expect("serve core");
    });

    let connection = ProtocolConnection::new(
        BufReader::new(client.try_clone().expect("clone client")),
        client,
    );
    connection
        .request(
            "provider.create_instance",
            Some(json!({
                "id": "fake-openai",
                "kind": "openai-chat-completions",
                "config_version": 1,
                "config": {
                    "base_url": "http://fake/v1",
                    "headers": {},
                    "options": {}
                }
            })),
        )
        .expect("create provider");
    connection
        .request(
            "provider.authenticate_instance",
            Some(json!({ "id": "fake-openai", "secret": "test-key" })),
        )
        .expect("authenticate provider");
    let created = connection
        .request(
            "session.create",
            Some(json!({
                "workspace": workspace.path(),
                "cwd": workspace.path(),
                "mode": "free",
                "provider": "fake-openai",
                "model": "gpt-4o-mini"
            })),
        )
        .expect("create session");
    let session_id = created["sessionId"]
        .as_str()
        .expect("session id")
        .to_owned();
    connection
        .request(
            "session.submit",
            Some(json!({ "sessionId": session_id, "prompt": "hello" })),
        )
        .expect("submit");

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut streamed = String::new();
    loop {
        assert!(Instant::now() < deadline, "turn did not finish");
        match connection
            .recv_timeout(Duration::from_millis(100))
            .expect("connection event")
        {
            Some(ConnectionEvent::Notification(notification))
                if notification.method == "session.event" =>
            {
                let params = notification.params.unwrap_or(Value::Null);
                if params["kind"] == "live"
                    && params["event"]["type"] == "text_delta"
                    && let Some(text) = params["event"]["text"].as_str()
                {
                    streamed.push_str(text);
                }
                if params["kind"] == "durable" && params["event"]["payload"]["type"] == "turn_ended"
                {
                    break;
                }
            }
            Some(ConnectionEvent::Request(request)) => {
                connection
                    .respond(request.id, Ok(json!({ "decision": "deny" })))
                    .expect("deny interaction");
            }
            Some(ConnectionEvent::Closed(error)) => panic!("core closed: {error}"),
            _ => {}
        }
    }
    assert_eq!(streamed, "hello from fake");

    let listed = connection
        .request("provider.list_instances", None)
        .expect("list providers");
    let mut updated = listed
        .as_array()
        .and_then(|instances| instances.first())
        .cloned()
        .expect("provider instance");
    updated["config"]["base_url"] = json!("http://fake/v2");
    connection
        .request("provider.update_instance", Some(updated))
        .expect("update provider");
    connection
        .request(
            "session.submit",
            Some(json!({ "sessionId": session_id, "prompt": "hello again" })),
        )
        .expect("submit second turn");
    let mut second = String::new();
    let second_deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(
            Instant::now() < second_deadline,
            "second turn did not finish"
        );
        match connection
            .recv_timeout(Duration::from_millis(100))
            .expect("connection event")
        {
            Some(ConnectionEvent::Notification(notification))
                if notification.method == "session.event" =>
            {
                let params = notification.params.unwrap_or(Value::Null);
                if params["kind"] == "live"
                    && params["event"]["type"] == "text_delta"
                    && let Some(text) = params["event"]["text"].as_str()
                {
                    second.push_str(text);
                }
                if params["kind"] == "durable" && params["event"]["payload"]["type"] == "turn_ended"
                {
                    break;
                }
            }
            Some(ConnectionEvent::Request(request)) => {
                connection
                    .respond(request.id, Ok(json!({ "decision": "deny" })))
                    .expect("deny interaction");
            }
            Some(ConnectionEvent::Closed(error)) => panic!("core closed: {error}"),
            _ => {}
        }
    }
    assert_eq!(second, "hello from fake");
    let requests = http.requests.lock().expect("request lock poisoned");
    assert!(
        requests
            .get(1)
            .is_some_and(|request| request.url.contains("/v2/"))
    );
    drop(requests);
    connection
        .request("body.shutdown", None)
        .expect("shutdown core");
    drop(connection);
    server_thread.join().expect("server thread");
}

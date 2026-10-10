use std::io::Cursor;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use alpha_protocol::{
    FramingError, Id, Message, Notification, Request, Response, ResponsePayload, Router, RpcError,
    codes, read_message, read_message_with_limit, write_message,
};
use serde_json::{Value, json};

fn request() -> Message {
    Message::Request(Request::new(
        Id::Number(1),
        "session.create",
        Some(json!({ "workspace": "/tmp/project" })),
    ))
}

#[test]
fn requests_round_trip() {
    let message = request();
    let text = serde_json::to_string(&message).expect("serialize");
    let parsed: Message = serde_json::from_str(&text).expect("deserialize");
    assert_eq!(parsed, message);
}

#[test]
fn notifications_do_not_deserialize_as_requests() {
    let message =
        Message::Notification(Notification::new("agent/cancel", Some(json!({ "id": 3 }))));
    let text = serde_json::to_string(&message).expect("serialize");
    let parsed: Message = serde_json::from_str(&text).expect("deserialize");
    assert_eq!(parsed, message);
    assert!(matches!(parsed, Message::Notification(_)));
}

#[test]
fn responses_round_trip_in_both_shapes() {
    let success = Message::Response(Response::success(
        Id::String("a".to_owned()),
        json!({ "ok": 1 }),
    ));
    let text = serde_json::to_string(&success).expect("serialize");
    let parsed: Message = serde_json::from_str(&text).expect("deserialize");
    assert_eq!(parsed, success);

    let failure = Message::Response(Response::failure(
        Id::Number(2),
        RpcError::method_not_found("nope"),
    ));
    let text = serde_json::to_string(&failure).expect("serialize");
    let parsed: Message = serde_json::from_str(&text).expect("deserialize");
    assert_eq!(parsed, failure);
    match parsed {
        Message::Response(Response {
            payload: ResponsePayload::Failure { error },
            ..
        }) => assert_eq!(error.code, codes::METHOD_NOT_FOUND),
        other => panic!("unexpected message: {other:?}"),
    }
}

#[test]
fn ndjson_framing_round_trips() {
    let mut buffer = Vec::new();
    write_message(&mut buffer, &request()).expect("write request");
    write_message(
        &mut buffer,
        &Message::Notification(Notification::new("alpha/note", None)),
    )
    .expect("write notification");
    assert_eq!(buffer.iter().filter(|byte| **byte == b'\n').count(), 2);

    let mut cursor = Cursor::new(buffer);
    let first = read_message(&mut cursor)
        .expect("read first")
        .expect("first");
    let second = read_message(&mut cursor)
        .expect("read second")
        .expect("second");
    assert_eq!(first, request());
    assert!(matches!(second, Message::Notification(_)));
    assert!(read_message(&mut cursor).expect("eof").is_none());
}

#[test]
fn oversized_lines_are_rejected() {
    let line = format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"{}\"}}\n",
        "x".repeat(64)
    );
    let mut cursor = Cursor::new(line.into_bytes());
    let error = read_message_with_limit(&mut cursor, 32).expect_err("too large");
    assert!(matches!(error, FramingError::TooLarge { limit: 32 }));
}

#[tokio::test]
async fn router_dispatches_requests_and_notifications() {
    let notified = Arc::new(AtomicBool::new(false));
    let flag = notified.clone();
    let router = Router::new()
        .method("alpha.ping", |_params: Value| async move {
            Ok::<Value, RpcError>(json!({ "pong": true }))
        })
        .method("alpha.note", move |_params: Value| {
            let flag = flag.clone();
            async move {
                flag.store(true, Ordering::SeqCst);
                Ok::<Value, RpcError>(Value::Null)
            }
        });

    let response = router
        .handle(Message::Request(Request::new(
            Id::Number(1),
            "alpha.ping",
            None,
        )))
        .await
        .expect("response");
    match response {
        Message::Response(Response {
            payload: ResponsePayload::Success { result },
            ..
        }) => assert_eq!(result, json!({ "pong": true })),
        other => panic!("unexpected response: {other:?}"),
    }

    let missing = router
        .handle(Message::Request(Request::new(
            Id::Number(2),
            "alpha.nope",
            None,
        )))
        .await
        .expect("response");
    match missing {
        Message::Response(Response {
            payload: ResponsePayload::Failure { error },
            ..
        }) => assert_eq!(error.code, codes::METHOD_NOT_FOUND),
        other => panic!("unexpected response: {other:?}"),
    }

    let none = router
        .handle(Message::Notification(Notification::new("alpha.note", None)))
        .await;
    assert!(none.is_none());
    assert!(notified.load(Ordering::SeqCst));
}

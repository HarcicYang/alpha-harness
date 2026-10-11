use std::io::Cursor;

use alpha_harness::serve_with;
use alpha_module_session::{
    SessionEvent, SessionHeader, SessionStore, TurnEndReason, WorkspaceRef,
};
use alpha_protocol::{Id, Message, Response, ResponsePayload, codes, read_message};
use alpha_schema::{MessageId, PermissionMode, SessionId, ToolCallId};
use serde_json::{Value, json};

fn result_of(message: Message) -> serde_json::Value {
    match message {
        Message::Response(Response {
            payload: ResponsePayload::Success { result },
            ..
        }) => result,
        other => panic!("unexpected response: {other:?}"),
    }
}

#[test]
fn serve_answers_ping_and_lists_sessions() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SessionStore::new(dir.path());
    let mut input = Cursor::new(
        concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"alpha.ping\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":\"list\",\"method\":\"session.list\"}\n"
        )
        .as_bytes(),
    );
    let mut output = Vec::new();

    serve_with(&mut input, &mut output, store).expect("serve");

    let mut responses = Cursor::new(output);
    let ping = read_message(&mut responses)
        .expect("read ping")
        .expect("ping response");
    assert_eq!(result_of(ping)["pong"], json!(true));

    let list = read_message(&mut responses)
        .expect("read list")
        .expect("list response");
    assert_eq!(result_of(list), json!([]));
    assert!(read_message(&mut responses).expect("eof").is_none());
}

#[test]
fn parse_errors_use_a_null_id() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SessionStore::new(dir.path());
    let mut input = Cursor::new(b"{not json}\n".as_slice());
    let mut output = Vec::new();

    serve_with(&mut input, &mut output, store).expect("serve");

    let mut responses = Cursor::new(output);
    let message = read_message(&mut responses)
        .expect("read error")
        .expect("error response");
    match message {
        Message::Response(Response {
            id,
            payload: ResponsePayload::Failure { error },
            ..
        }) => {
            assert_eq!(id, Id::Null);
            assert_eq!(error.code, codes::PARSE_ERROR);
            assert!(!error.message.is_empty());
        }
        other => panic!("unexpected response: {other:?}"),
    }
}

#[test]
fn attach_replays_events_after_a_sequence() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SessionStore::new(dir.path());
    let header = SessionHeader::new(
        SessionId::new(),
        PermissionMode::Auto,
        WorkspaceRef {
            root: "/tmp/workspace".to_owned(),
            cwd: "/tmp/workspace".to_owned(),
        },
    );
    let id = header.id;
    {
        let writer = store.create(header).expect("create");
        writer
            .append(SessionEvent::TurnStarted)
            .expect("turn start");
        writer
            .append(SessionEvent::TurnEnded {
                reason: TurnEndReason::Completed,
            })
            .expect("turn end");
    }

    let input = format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"session.attach\",\"params\":{{\"sessionId\":\"{id}\",\"afterSeq\":1}}}}\n"
    );
    let mut reader = Cursor::new(input.into_bytes());
    let mut output = Vec::new();
    serve_with(&mut reader, &mut output, store).expect("serve");

    let mut responses = Cursor::new(output);
    let message = read_message(&mut responses)
        .expect("read attach")
        .expect("attach response");
    let result: Value = result_of(message);
    let events = result.as_array().expect("events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["seq"], json!(2));
}

#[test]
fn resume_repairs_interrupted_work_and_reports_state() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SessionStore::new(dir.path());
    let header = SessionHeader::new(
        SessionId::new(),
        PermissionMode::Auto,
        WorkspaceRef {
            root: "/tmp/workspace".to_owned(),
            cwd: "/tmp/workspace".to_owned(),
        },
    );
    let id = header.id;
    {
        let writer = store.create(header).expect("create");
        writer
            .append(SessionEvent::TurnStarted)
            .expect("turn start");
        writer
            .append(SessionEvent::ToolCall {
                id: ToolCallId::new(),
                message_id: MessageId::new(),
                name: "shell".to_owned(),
                input: json!({ "command": "cargo test" }),
            })
            .expect("tool call");
    }

    let input = format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"session.resume\",\"params\":{{\"sessionId\":\"{id}\"}}}}\n"
    );
    let mut reader = Cursor::new(input.into_bytes());
    let mut output = Vec::new();
    serve_with(&mut reader, &mut output, store.clone()).expect("serve");

    let message = read_message(&mut Cursor::new(output))
        .expect("read resume")
        .expect("resume response");
    let result = result_of(message);
    assert_eq!(result["repairs"], json!(2));
    assert_eq!(result["turnOpen"], json!(false));

    let reader = store.open_read(id).expect("open read");
    assert_eq!(reader.events().len(), 4);
}

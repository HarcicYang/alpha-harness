use std::fs;
use std::io::Write;

use alpha_session_log::{
    MessageRecord, SessionError, SessionEvent, SessionHeader, SessionStore, ToolStatus,
    TurnEndReason, WorkspaceRef,
};
use alpha_schema::{
    AssistantContentBlock, MessageId, PermissionMode, ReasoningLevel, SessionId, Timestamp,
    ToolCallId,
};

fn header() -> SessionHeader {
    SessionHeader::new(
        SessionId::new(),
        PermissionMode::Auto,
        WorkspaceRef {
            root: "/tmp/workspace".to_owned(),
            cwd: "/tmp/workspace".to_owned(),
        },
    )
}

#[test]
fn schema_one_assistant_text_migrates_to_a_text_block() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SessionStore::new(dir.path());
    let header = header();
    let id = header.id;
    drop(store.create(header).expect("create"));

    let message_id = MessageId::new();
    let line = serde_json::json!({
        "seq": 1,
        "at": 123,
        "schema": 1,
        "payload": {
            "type": "assistant_message",
            "id": message_id,
            "text": "legacy"
        }
    });
    let path = dir.path().join(id.to_string()).join("session.v0.jsonl");
    fs::write(path, format!("{line}\n")).expect("write legacy log");

    let reader = store.open_read(id).expect("read");
    let state = reader.state();
    let MessageRecord::Assistant { blocks, text, .. } = &state.messages[0] else {
        panic!("expected assistant message");
    };
    assert_eq!(text, "legacy");
    assert_eq!(
        blocks,
        &vec![AssistantContentBlock::Text {
            text: "legacy".to_owned()
        }]
    );
}

#[test]
fn create_append_and_reopen() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SessionStore::new(dir.path());
    let header = header();
    let id = header.id;

    {
        let writer = store.create(header).expect("create");
        writer
            .append(SessionEvent::TurnStarted)
            .expect("append turn start");
        writer
            .append(SessionEvent::UserMessage {
                id: MessageId::new(),
                text: "hello".to_owned(),
            })
            .expect("append user message");
        writer
            .append(SessionEvent::ModeChanged {
                mode: PermissionMode::Read,
            })
            .expect("append mode change");
        writer
            .append(SessionEvent::TurnEnded {
                reason: TurnEndReason::Completed,
            })
            .expect("append turn end");
    }

    let reader = store.open_read(id).expect("open read");
    assert_eq!(reader.header().id, id);
    assert_eq!(reader.events().len(), 4);
    let state = reader.state();
    assert_eq!(state.mode, PermissionMode::Read);
    assert_eq!(state.messages.len(), 1);
    assert!(!state.turn_open);
    assert_eq!(state.last_seq.get(), 4);
}

#[test]
fn system_prompt_snapshot_is_projected_from_the_log() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SessionStore::new(dir.path());
    let header = header();
    let id = header.id;

    {
        let writer = store.create(header).expect("create");
        writer
            .append(SessionEvent::SystemPromptSet {
                text: "framework rules".to_owned(),
            })
            .expect("append prompt");
    }

    let reader = store.open_read(id).expect("open read");
    assert_eq!(
        reader.state().system_prompt.as_deref(),
        Some("framework rules")
    );
}

#[test]
fn context_compaction_is_projected_from_the_log() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SessionStore::new(dir.path());
    let header = header();
    let id = header.id;

    {
        let writer = store.create(header).expect("create");
        writer
            .append(SessionEvent::ContextCompacted {
                summary: "summary".to_owned(),
                from_seq: alpha_schema::Seq::new(1),
                through_seq: alpha_schema::Seq::new(2),
            })
            .expect("append compaction");
    }

    let reader = store.open_read(id).expect("open read");
    let compaction = reader.state().compaction.expect("compaction");
    assert_eq!(compaction.summary, "summary");
    assert_eq!(compaction.from_seq.get(), 1);
    assert_eq!(compaction.through_seq.get(), 2);
}

#[test]
fn route_changes_are_projected_from_the_log() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SessionStore::new(dir.path());
    let header = header();
    let id = header.id;

    {
        let writer = store.create(header).expect("create");
        writer
            .append(SessionEvent::RouteChanged {
                provider: "provider-b".to_owned(),
                model: "model-b".to_owned(),
                reasoning: ReasoningLevel::High,
            })
            .expect("append route");
    }

    let reader = store.open_read(id).expect("open read");
    let state = reader.state();
    assert_eq!(state.provider, "provider-b");
    assert_eq!(state.model, "model-b");
    assert_eq!(state.reasoning, ReasoningLevel::High);
}

#[test]
fn read_open_does_not_modify_files() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SessionStore::new(dir.path());
    let header = header();
    let id = header.id;

    {
        let writer = store.create(header).expect("create");
        writer
            .append(SessionEvent::TurnStarted)
            .expect("append turn start");
        writer
            .append(SessionEvent::TurnEnded {
                reason: TurnEndReason::Completed,
            })
            .expect("append turn end");
    }

    let session_dir = store.session_dir(id);
    let header_before = fs::read(session_dir.join("header.json")).expect("read header");
    let events_before = fs::read(session_dir.join("session.v0.jsonl")).expect("read events");

    let _reader = store.open_read(id).expect("open read");

    assert_eq!(
        fs::read(session_dir.join("header.json")).expect("read header"),
        header_before
    );
    assert_eq!(
        fs::read(session_dir.join("session.v0.jsonl")).expect("read events"),
        events_before
    );
}

#[test]
fn torn_tail_is_dropped_and_write_open_publishes_a_successor() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SessionStore::new(dir.path());
    let header = header();
    let id = header.id;

    {
        let writer = store.create(header).expect("create");
        writer
            .append(SessionEvent::TurnStarted)
            .expect("append turn start");
        writer
            .append(SessionEvent::TurnEnded {
                reason: TurnEndReason::Completed,
            })
            .expect("append turn end");
    }

    let session_dir = store.session_dir(id);
    let source = session_dir.join("session.v0.jsonl");
    {
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&source)
            .expect("open events");
        file.write_all(b"{\"torn\":").expect("write torn tail");
    }
    let source_before = fs::read(&source).expect("read source");

    let reader = store.open_read(id).expect("open read");
    assert_eq!(reader.events().len(), 2);
    assert!(reader.tail_damage().is_some());

    {
        let writer = store.open_write(id).expect("open write");
        assert!(writer.repairs().is_empty());
        writer
            .append(SessionEvent::TurnStarted)
            .expect("append after repair");
    }

    assert_eq!(fs::read(&source).expect("read source"), source_before);
    assert!(session_dir.join("session.v1.jsonl").exists());

    let reader = store.open_read(id).expect("open read after repair");
    assert_eq!(reader.generation(), 1);
    assert_eq!(reader.events().len(), 3);
    assert!(reader.tail_damage().is_none());
}

#[test]
fn interrupted_turn_and_tool_call_are_repaired_on_write_open() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SessionStore::new(dir.path());
    let header = header();
    let id = header.id;
    let call_id = ToolCallId::new();

    {
        let writer = store.create(header).expect("create");
        writer
            .append(SessionEvent::TurnStarted)
            .expect("append turn start");
        writer
            .append(SessionEvent::ToolCall {
                id: call_id,
                message_id: MessageId::new(),
                name: "shell".to_owned(),
                input: serde_json::json!({ "command": "cargo test" }),
            })
            .expect("append tool call");
    }

    {
        let writer = store.open_write(id).expect("open write");
        assert_eq!(writer.repairs().len(), 2);
        assert_eq!(writer.last_seq().get(), 4);
        assert!(matches!(
            writer.repairs()[0].payload,
            SessionEvent::ToolResult {
                status: ToolStatus::Interrupted,
                ..
            }
        ));
        assert!(matches!(
            writer.repairs()[1].payload,
            SessionEvent::TurnEnded {
                reason: TurnEndReason::Interrupted
            }
        ));
    }

    let reader = store.open_read(id).expect("open read");
    let state = reader.state();
    assert!(!state.turn_open);
    assert!(state.pending_tool_calls.is_empty());
}

#[test]
fn writer_lease_is_exclusive() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SessionStore::new(dir.path());
    let header = header();
    let id = header.id;
    let first = store.create(header).expect("create");

    let second = store.open_write(id);
    assert!(matches!(second, Err(SessionError::WriterBusy { .. })));

    drop(first);
    assert!(store.open_write(id).is_ok());
}

#[test]
fn list_returns_headers_newest_first() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SessionStore::new(dir.path());

    let mut older = header();
    older.created_at = Timestamp::from_millis(1_000);
    let mut newer = header();
    newer.created_at = Timestamp::from_millis(2_000);

    drop(store.create(older.clone()).expect("create older"));
    drop(store.create(newer.clone()).expect("create newer"));

    let headers = store.list().expect("list");
    assert_eq!(headers.len(), 2);
    assert_eq!(headers[0].id, newer.id);
    assert_eq!(headers[1].id, older.id);
}

#[test]
fn list_entries_includes_the_first_user_message() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SessionStore::new(dir.path());
    let header = header();
    let id = header.id;

    {
        let writer = store.create(header).expect("create");
        writer
            .append(SessionEvent::UserMessage {
                id: MessageId::new(),
                text: "inspect this repository".to_owned(),
            })
            .expect("append first user message");
        writer
            .append(SessionEvent::UserMessage {
                id: MessageId::new(),
                text: "second message".to_owned(),
            })
            .expect("append second user message");
    }

    let entries = store.list_entries().expect("list entries");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].header.id, id);
    assert_eq!(
        entries[0].first_message.as_deref(),
        Some("inspect this repository")
    );
}

#[test]
fn delete_rejects_active_writers_and_removes_idle_sessions() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SessionStore::new(dir.path());
    let header = header();
    let id = header.id;
    let writer = store.create(header).expect("create");

    assert!(matches!(
        store.delete(id),
        Err(SessionError::WriterBusy { .. })
    ));
    drop(writer);

    store.delete(id).expect("delete");
    assert!(!store.session_dir(id).exists());
    assert!(matches!(store.delete(id), Err(SessionError::NotFound(_))));
}

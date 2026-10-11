//! Client wrapper around the single alpha cross-process protocol.

#![forbid(unsafe_code)]

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use alpha_protocol::{
    FramingError, Id, Message, Notification, Request, Response, ResponsePayload, RpcError,
    read_message, write_message,
};
use serde_json::Value;

type PendingResponses = Arc<Mutex<HashMap<Id, Sender<Result<Value, ClientError>>>>>;

/// One event delivered by a multiplexed protocol connection.
#[derive(Debug)]
pub enum ConnectionEvent {
    /// A notification from the remote side.
    Notification(Notification),
    /// A request initiated by the remote side.
    Request(Request),
    /// The connection closed.
    Closed(ClientError),
}

/// Failure while using an alpha-protocol connection.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// Protocol framing failed.
    #[error(transparent)]
    Framing(#[from] FramingError),
    /// The remote side returned an RPC error.
    #[error("remote error: {0}")]
    Remote(RpcError),
    /// The connection was closed locally.
    #[error("connection is closed")]
    Closed,
    /// The stream ended before a response arrived.
    #[error("connection ended before a response arrived")]
    Ended,
}

/// Multiplexed blocking connection over one alpha-protocol stream.
///
/// A dedicated reader thread owns the read half. Requests are correlated by
/// id, while notifications and server-initiated requests are delivered through
/// [`ConnectionEvent`]. The write half is shared so a front can answer a
/// server-initiated request while another request is in flight.
pub struct ProtocolConnection {
    writer: Arc<Mutex<Option<Box<dyn Write + Send>>>>,
    next_id: Arc<AtomicI64>,
    pending: PendingResponses,
    events: Receiver<ConnectionEvent>,
    reader: Option<JoinHandle<()>>,
}

impl ProtocolConnection {
    /// Start a connection over a reader and writer.
    pub fn new<R, W>(reader: R, writer: W) -> Self
    where
        R: BufRead + Send + 'static,
        W: Write + Send + 'static,
    {
        let writer: Arc<Mutex<Option<Box<dyn Write + Send>>>> =
            Arc::new(Mutex::new(Some(Box::new(writer))));
        let next_id = Arc::new(AtomicI64::new(1));
        let pending: PendingResponses = Arc::new(Mutex::new(HashMap::new()));
        let (events_tx, events) = mpsc::channel();
        let reader_pending = pending.clone();
        let reader = thread::spawn(move || {
            let mut reader = reader;
            loop {
                match read_message(&mut reader) {
                    Ok(Some(Message::Response(response))) => {
                        let id = response.id.clone();
                        let result = match response.payload {
                            ResponsePayload::Success { result } => Ok(result),
                            ResponsePayload::Failure { error } => Err(ClientError::Remote(error)),
                        };
                        if let Some(sender) = reader_pending
                            .lock()
                            .expect("protocol pending lock poisoned")
                            .remove(&id)
                        {
                            let _ = sender.send(result);
                        }
                    }
                    Ok(Some(Message::Notification(notification))) => {
                        let _ = events_tx.send(ConnectionEvent::Notification(notification));
                    }
                    Ok(Some(Message::Request(request))) => {
                        let _ = events_tx.send(ConnectionEvent::Request(request));
                    }
                    Ok(None) => {
                        let _ = events_tx.send(ConnectionEvent::Closed(ClientError::Ended));
                        close_pending(&reader_pending);
                        return;
                    }
                    Err(error) => {
                        let _ =
                            events_tx.send(ConnectionEvent::Closed(ClientError::Framing(error)));
                        close_pending(&reader_pending);
                        return;
                    }
                }
            }
        });
        Self {
            writer,
            next_id,
            pending,
            events,
            reader: Some(reader),
        }
    }

    /// Send a request and wait for its response.
    pub fn request(
        &self,
        method: impl Into<String>,
        params: Option<Value>,
    ) -> Result<Value, ClientError> {
        let id = Id::Number(self.next_id.fetch_add(1, Ordering::Relaxed));
        let (tx, rx) = mpsc::channel();
        self.pending
            .lock()
            .expect("protocol pending lock poisoned")
            .insert(id.clone(), tx);
        if let Err(error) = self.write(&Message::Request(Request::new(id.clone(), method, params)))
        {
            self.pending
                .lock()
                .expect("protocol pending lock poisoned")
                .remove(&id);
            return Err(error);
        }
        rx.recv().unwrap_or(Err(ClientError::Closed))
    }

    /// Send a notification.
    pub fn notify(
        &self,
        method: impl Into<String>,
        params: Option<Value>,
    ) -> Result<(), ClientError> {
        self.write(&Message::Notification(Notification::new(method, params)))
    }

    /// Answer a server-initiated request.
    pub fn respond(&self, id: Id, result: Result<Value, RpcError>) -> Result<(), ClientError> {
        let response = match result {
            Ok(result) => Response::success(id, result),
            Err(error) => Response::failure(id, error),
        };
        self.write(&Message::Response(response))
    }

    /// Receive one event, waiting up to `timeout`.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<Option<ConnectionEvent>, ClientError> {
        match self.events.recv_timeout(timeout) {
            Ok(event) => Ok(Some(event)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(ClientError::Ended),
        }
    }

    /// Take one already-buffered event without waiting.
    pub fn try_recv(&self) -> Result<Option<ConnectionEvent>, ClientError> {
        match self.events.try_recv() {
            Ok(event) => Ok(Some(event)),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => Err(ClientError::Ended),
        }
    }

    /// Close the write side, which lets the core observe stdin EOF.
    pub fn close_writer(&self) {
        *self.writer.lock().expect("protocol writer lock poisoned") = None;
    }

    fn write(&self, message: &Message) -> Result<(), ClientError> {
        let mut writer = self.writer.lock().expect("protocol writer lock poisoned");
        let writer = writer.as_mut().ok_or(ClientError::Closed)?;
        write_message(writer, message)?;
        writer.flush().map_err(FramingError::from)?;
        Ok(())
    }
}

impl Drop for ProtocolConnection {
    fn drop(&mut self) {
        self.close_writer();
        self.reader.take();
    }
}

fn close_pending(pending: &PendingResponses) {
    let mut pending = pending.lock().expect("protocol pending lock poisoned");
    for (_, sender) in pending.drain() {
        let _ = sender.send(Err(ClientError::Ended));
    }
}

/// Blocking alpha-protocol client.
pub struct ProtocolClient<R, W> {
    reader: R,
    writer: Option<W>,
    next_id: i64,
    pending: VecDeque<Message>,
}

impl<R, W> ProtocolClient<R, W>
where
    R: BufRead,
    W: Write,
{
    /// Create a client over one reader and writer.
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            reader,
            writer: Some(writer),
            next_id: 1,
            pending: VecDeque::new(),
        }
    }

    /// Send a request and wait for its response.
    pub fn request(
        &mut self,
        method: impl Into<String>,
        params: Option<Value>,
    ) -> Result<Value, ClientError> {
        let id = Id::Number(self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        let request = Message::Request(Request::new(id.clone(), method, params));
        self.write(&request)?;
        loop {
            let message = self.read_next()?.ok_or(ClientError::Ended)?;
            match message {
                Message::Response(response) if response.id == id => {
                    return match response.payload {
                        ResponsePayload::Success { result } => Ok(result),
                        ResponsePayload::Failure { error } => Err(ClientError::Remote(error)),
                    };
                }
                message => self.pending.push_back(message),
            }
        }
    }

    /// Send a notification.
    pub fn notify(
        &mut self,
        method: impl Into<String>,
        params: Option<Value>,
    ) -> Result<(), ClientError> {
        let notification = Message::Notification(Notification::new(method, params));
        self.write(&notification)
    }

    /// Take one buffered notification or response.
    pub fn take_pending(&mut self) -> Option<Message> {
        self.pending.pop_front()
    }

    /// Close the write side of the connection.
    pub fn close_writer(&mut self) {
        self.writer = None;
    }

    fn write(&mut self, message: &Message) -> Result<(), ClientError> {
        let writer = self.writer.as_mut().ok_or(ClientError::Closed)?;
        write_message(writer, message)?;
        Ok(())
    }

    fn read_next(&mut self) -> Result<Option<Message>, ClientError> {
        read_message(&mut self.reader).map_err(ClientError::from)
    }
}

/// A successful `body.hello` response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoreInfo {
    /// Protocol version advertised by the core.
    pub protocol: String,
    /// Core process instance identity.
    pub core_instance_id: String,
    /// Core process id.
    pub pid: u32,
}

impl CoreInfo {
    /// Parse a core hello response.
    pub fn from_value(value: Value) -> Result<Self, ClientError> {
        let protocol = value
            .get("protocol")
            .and_then(Value::as_str)
            .unwrap_or("alpha-protocol/1")
            .to_owned();
        let core_instance_id = value
            .get("coreInstanceId")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned();
        let pid = value.get("pid").and_then(Value::as_u64).unwrap_or_default() as u32;
        Ok(Self {
            protocol,
            core_instance_id,
            pid,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use alpha_protocol::{Id, Message, Request, ResponsePayload, read_message};
    use serde_json::json;

    use super::{ConnectionEvent, ProtocolConnection};

    struct SharedWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedWriter {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("writer lock poisoned")
                .extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn multiplexed_connection_delivers_and_answers_server_requests() {
        let request = Message::Request(Request::new(
            Id::Number(7),
            "interaction.request",
            Some(json!({ "kind": "permission" })),
        ));
        let mut input = serde_json::to_vec(&request).expect("request JSON");
        input.push(b'\n');
        let output = Arc::new(Mutex::new(Vec::new()));
        let connection = ProtocolConnection::new(Cursor::new(input), SharedWriter(output.clone()));

        let event = connection
            .recv_timeout(Duration::from_secs(1))
            .expect("receive")
            .expect("event");
        let ConnectionEvent::Request(request) = event else {
            panic!("expected server request");
        };
        connection
            .respond(request.id, Ok(json!({ "decision": "once" })))
            .expect("respond");

        let bytes = output.lock().expect("output lock poisoned").clone();
        let mut reader = Cursor::new(bytes);
        let response = read_message(&mut reader)
            .expect("read response")
            .expect("response");
        let Message::Response(response) = response else {
            panic!("expected response");
        };
        assert_eq!(response.id, Id::Number(7));
        let ResponsePayload::Success { result } = response.payload else {
            panic!("expected success");
        };
        assert_eq!(result["decision"], "once");
    }
}

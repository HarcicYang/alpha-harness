//! Client wrapper around the single alpha cross-process protocol.

#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::io::{BufRead, Write};

use alpha_protocol::{
    FramingError, Id, Message, Notification, Request, ResponsePayload, RpcError, read_message,
    write_message,
};
use serde_json::Value;

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

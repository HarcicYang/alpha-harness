//! JSON-RPC 2.0 over NDJSON framing and request routing.
//!
//! stdout carries protocol messages only; logs belong on stderr.

#![forbid(unsafe_code)]

pub mod framing;
pub mod message;
pub mod router;

pub use framing::{
    DEFAULT_MAX_LINE_BYTES, FramingError, read_message, read_message_with_limit, write_message,
};
pub use message::{Id, Message, Notification, Request, Response, ResponsePayload, RpcError, codes};
pub use router::Router;

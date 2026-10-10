//! NDJSON framing over byte streams.

use std::io::{BufRead, Write};

use thiserror::Error;

use crate::message::Message;

/// Default maximum size of one protocol line.
pub const DEFAULT_MAX_LINE_BYTES: usize = 8 * 1024 * 1024;

/// Framing errors.
#[derive(Debug, Error)]
pub enum FramingError {
    /// Underlying IO error.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// The line exceeded the configured limit.
    #[error("protocol line exceeds {limit} bytes")]
    TooLarge {
        /// Configured limit.
        limit: usize,
    },
    /// The line was not a valid JSON-RPC message.
    #[error("invalid JSON-RPC message: {0}")]
    Parse(#[from] serde_json::Error),
}

/// Read one NDJSON message.
///
/// Returns `Ok(None)` at end of stream. Blank lines are skipped.
pub fn read_message<R: BufRead>(reader: &mut R) -> Result<Option<Message>, FramingError> {
    read_message_with_limit(reader, DEFAULT_MAX_LINE_BYTES)
}

/// Read one NDJSON message with an explicit line limit.
pub fn read_message_with_limit<R: BufRead>(
    reader: &mut R,
    limit: usize,
) -> Result<Option<Message>, FramingError> {
    loop {
        let mut line = Vec::new();
        loop {
            let available = reader.fill_buf()?;
            if available.is_empty() {
                if line.is_empty() {
                    return Ok(None);
                }
                break;
            }
            let take = available
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(available.len(), |index| index + 1);
            if line.len() + take > limit {
                return Err(FramingError::TooLarge { limit });
            }
            let ended = available[take - 1] == b'\n';
            line.extend_from_slice(&available[..take]);
            reader.consume(take);
            if ended {
                break;
            }
        }
        while line
            .last()
            .is_some_and(|byte| *byte == b'\n' || *byte == b'\r')
        {
            line.pop();
        }
        if line.is_empty() {
            continue;
        }
        let message = serde_json::from_slice(&line)?;
        return Ok(Some(message));
    }
}

/// Write one NDJSON message and flush it.
pub fn write_message<W: Write>(writer: &mut W, message: &Message) -> Result<(), FramingError> {
    serde_json::to_writer(&mut *writer, message)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

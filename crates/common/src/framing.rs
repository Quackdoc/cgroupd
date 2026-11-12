//! Varlink-compatible framing: one JSON object per message, terminated by a
//! single `NUL` byte (`0x00`), capped at `MAX_MESSAGE_BYTES`. A raw `NUL`
//! never appears inside valid JSON (strings escape it as `\u0000`), so the
//! terminator is unambiguous. Both sides enforce identical limits.

use std::io::{BufRead, BufReader};

use crate::MAX_MESSAGE_BYTES;

/// Framing failure. Transport errors carry the `io::Error` so each side can
/// render it in its own error type.
#[derive(Debug)]
pub enum FrameError {
    Io(std::io::Error),
    TooLarge,
    Empty,
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::Io(e) => write!(f, "{e}"),
            FrameError::TooLarge => write!(f, "message too large"),
            FrameError::Empty => write!(f, "empty message"),
        }
    }
}

impl std::error::Error for FrameError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FrameError::Io(e) => Some(e),
            _ => None,
        }
    }
}

/// Read one `NUL`-terminated frame (terminator excluded). `TooLarge` once
/// more than `MAX_MESSAGE_BYTES + 1` bytes arrive without a terminator;
/// `Empty` on immediate EOF (clean client disconnect).
pub fn read_frame<R: std::io::Read>(stream: R) -> Result<Vec<u8>, FrameError> {
    let mut reader = BufReader::new(stream);
    let mut line = Vec::new();
    let mut total = 0usize;
    loop {
        let chunk = reader.fill_buf().map_err(FrameError::Io)?;
        if chunk.is_empty() {
            break;
        }
        let take = chunk
            .iter()
            .position(|&b| b == 0)
            .map(|i| i + 1)
            .unwrap_or(chunk.len());
        total += take;
        if total > MAX_MESSAGE_BYTES + 1 {
            return Err(FrameError::TooLarge);
        }
        line.extend_from_slice(&chunk[..take]);
        reader.consume(take);
        if line.ends_with(b"\0") {
            break;
        }
    }
    if line.is_empty() {
        return Err(FrameError::Empty);
    }
    line.pop();
    Ok(line)
}

/// Serialize `value` as JSON plus the `NUL` terminator, enforcing the size cap.
pub fn encode_frame(value: &serde_json::Value) -> Result<Vec<u8>, String> {
    let mut buf = serde_json::to_vec(value).map_err(|e| format!("encode reply: {e}"))?;
    buf.push(0);
    if buf.len() > MAX_MESSAGE_BYTES + 1 {
        return Err("reply too large".to_string());
    }
    Ok(buf)
}

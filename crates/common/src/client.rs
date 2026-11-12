use std::fmt;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use crate::MAX_MESSAGE_BYTES;
use crate::framing::{FrameError, read_frame};
use crate::proto::{Request, Response};

/// Transport error talking to the daemon.
#[derive(Debug)]
pub struct ClientError(pub String);

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cgroupd client: {}", self.0)
    }
}

impl std::error::Error for ClientError {}

impl From<std::io::Error> for ClientError {
    fn from(e: std::io::Error) -> Self {
        ClientError(e.to_string())
    }
}

impl From<serde_json::Error> for ClientError {
    fn from(e: serde_json::Error) -> Self {
        ClientError(e.to_string())
    }
}

/// One Varlink call in, one reply out (`NUL`-delimited JSON; see `varlink`).
pub fn send(socket_path: &str, req: &Request, timeout: Duration) -> Result<Response, ClientError> {
    let mut stream = UnixStream::connect(socket_path)
        .map_err(|e| ClientError(format!("connect {socket_path}: {e}")))?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;

    let mut buf = serde_json::to_vec(&crate::varlink::build_call(req).map_err(ClientError)?)?;
    buf.push(0);
    if buf.len() > MAX_MESSAGE_BYTES {
        return Err(ClientError("request too large".to_string()));
    }
    stream.write_all(&buf)?;
    stream.flush()?;

    let frame = read_frame(&stream).map_err(|e| match e {
        FrameError::Io(e) => ClientError::from(e),
        FrameError::TooLarge => ClientError("response too large".to_string()),
        FrameError::Empty => ClientError("empty response from daemon".to_string()),
    })?;
    let reply: serde_json::Value = serde_json::from_slice(&frame)?;
    match crate::varlink::split_reply(&reply) {
        Ok(detail) => Ok(Response::Ok { detail }),
        Err((name, message)) => Ok(Response::Err {
            msg: format!("{name}: {message}"),
        }),
    }
}

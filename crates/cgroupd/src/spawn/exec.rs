//! Daemon side of the exec helper: start it, hand it a [`Spec`], and run the
//! `go` / `errno` handshake (protocol in `crate::exec_helper`).

use std::io::{Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::process::Stdio;

use crate::exec_helper::{self, Spec};

use super::proc::waitpid_blocking;

/// The daemon's end of the helper's socket.
pub(super) struct Barrier(UnixStream);

impl Barrier {
    /// Let the helper run, then wait for its exec to succeed or fail.
    /// `Ok(None)`: exec succeeded. `Ok(Some(errno))`: the helper failed.
    pub(super) fn release_and_wait(mut self) -> Result<Option<i32>, String> {
        let _ = self.0.write_all(b"x");
        let mut buf = [0u8; 4];
        let mut got = 0;
        while got < buf.len() {
            match self.0.read(&mut buf[got..]) {
                Ok(0) => break,
                Ok(n) => got += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(format!("exec barrier: {e}")),
            }
        }
        Ok((got == buf.len()).then(|| i32::from_ne_bytes(buf)))
    }
}

/// Start a helper that is waiting for `spec`'s go-ahead. The caller must put
/// the returned pid in its cgroup, then call [`Barrier::release_and_wait`].
pub(super) fn start(spec: &Spec) -> Result<(libc::pid_t, Barrier), String> {
    let json = serde_json::to_vec(spec).map_err(|e| format!("encode spec: {e}"))?;
    let (mine, theirs) = UnixStream::pair().map_err(|e| format!("socketpair: {e}"))?;
    let child = exec_helper::command()
        .stdin(Stdio::from(OwnedFd::from(theirs)))
        .spawn()
        .map_err(|e| format!("start exec helper: {e}"))?;
    let pid = child.id() as libc::pid_t;

    let mut barrier = Barrier(mine);
    let sent = barrier
        .0
        .write_all(&(json.len() as u32).to_le_bytes())
        .and_then(|()| barrier.0.write_all(&json));
    if let Err(e) = sent {
        kill_and_reap(pid);
        return Err(format!("send spec to exec helper: {e}"));
    }
    Ok((pid, barrier))
}

/// Kill a helper that never got to exec, and collect it.
pub(super) fn kill_and_reap(pid: libc::pid_t) {
    if let Some(p) = rustix::process::Pid::from_raw(pid) {
        let _ = rustix::process::kill_process(p, rustix::process::Signal::KILL);
    }
    let _ = waitpid_blocking(pid);
}

//! `cgroupd --exec-helper`: the process that becomes a spawned app or hook.
//!
//! The daemon never forks its own (multi-threaded) address space. It starts
//! this binary again with a socket as stdin and talks to it:
//!
//! 1. daemon -> helper: `u32` length, then a JSON [`Spec`]
//! 2. the daemon moves the helper's pid into the app cgroup
//! 3. daemon -> helper: one `go` byte (EOF instead means the daemon died: exit)
//! 4. the helper drops privileges, opens stdio as the target user, and execs
//! 5. helper -> daemon: on failure, the `errno` (4 bytes). A successful exec
//!    closes the socket (it is `CLOEXEC`), which the daemon reads as EOF.
//!
//! The helper is single-threaded, so the per-thread `set_thread_*` calls below
//! apply to the whole process.

use std::convert::Infallible;
use std::io::{Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use cgroupd_common::StdioSpec;
use rustix::fs::{Mode, OFlags};
use serde::{Deserialize, Serialize};

/// The argument that selects helper mode; must be `argv[1]`, nothing else.
pub const FLAG: &str = "--exec-helper";

/// Largest spec the helper will read (args and env are capped far below this).
const MAX_SPEC_BYTES: usize = 16 << 20;

/// Everything the helper needs to become the program.
#[derive(Serialize, Deserialize)]
pub struct Spec {
    pub uid: u32,
    pub gid: u32,
    pub cwd: Option<String>,
    pub umask: Option<u32>,
    pub stdout: StdioSpec,
    pub stderr: StdioSpec,
    pub program: String,
    pub args: Vec<String>,
    /// `KEY=VALUE` entries; the program gets exactly these.
    pub env: Vec<String>,
}

// ---------------------------------------------------------------------------
// Daemon side
// ---------------------------------------------------------------------------

/// The daemon's own binary, opened once at startup so replacing the file on
/// disk (an upgrade) cannot change which helper a running daemon starts.
static SELF_EXE: OnceLock<OwnedFd> = OnceLock::new();

pub fn pin_self() {
    if let Ok(fd) = rustix::fs::open(
        "/proc/self/exe",
        OFlags::PATH | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        let _ = SELF_EXE.set(fd);
    }
}

/// The command that starts a helper (stdin is set by the caller).
pub fn command() -> Command {
    use std::os::fd::AsRawFd;
    let path = match SELF_EXE.get() {
        Some(fd) => format!("/proc/self/fd/{}", fd.as_raw_fd()),
        None => "/proc/self/exe".to_string(),
    };
    let mut cmd = Command::new(path);
    cmd.arg0("cgroupd").arg(FLAG);
    cmd
}

// ---------------------------------------------------------------------------
// Helper side
// ---------------------------------------------------------------------------

/// Entry point of `--exec-helper`. Never returns.
pub fn run() -> ! {
    let Ok(stdin) = std::io::stdin().as_fd().try_clone_to_owned() else {
        std::process::exit(127);
    };
    let mut sock = UnixStream::from(stdin);
    let Err(errno) = serve(&mut sock);
    let _ = sock.write_all(&errno.to_ne_bytes());
    std::process::exit(127);
}

/// Read the spec, wait for `go`, then exec. Returns only on failure, as `errno`.
fn serve(sock: &mut UnixStream) -> Result<Infallible, i32> {
    let spec = read_spec(sock)?;
    let mut go = [0u8; 1];
    if sock.read_exact(&mut go).is_err() {
        // The daemon went away before placing us in the cgroup: do not run.
        std::process::exit(1);
    }

    drop_privileges(spec.uid, spec.gid)?;
    // Relative stdio paths resolve against the app's cwd, as they always did.
    if let Some(dir) = &spec.cwd {
        rustix::process::chdir(dir.as_str()).map_err(|e| e.raw_os_error())?;
    }
    if let Some(mask) = spec.umask {
        rustix::process::umask(Mode::from_bits_truncate(mask));
    }
    let stdout = open_stdio(&spec.stdout)?;
    let stderr = open_stdio(&spec.stderr)?;

    let err = Command::new(&spec.program)
        .args(&spec.args)
        .env_clear()
        .envs(spec.env.iter().filter_map(|e| e.split_once('=')))
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .exec();
    Err(err.raw_os_error().unwrap_or(libc::EIO))
}

fn read_spec(sock: &mut UnixStream) -> Result<Spec, i32> {
    let mut len = [0u8; 4];
    sock.read_exact(&mut len).map_err(|_| libc::EPIPE)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_SPEC_BYTES {
        return Err(libc::E2BIG);
    }
    let mut json = vec![0u8; len];
    sock.read_exact(&mut json).map_err(|_| libc::EPIPE)?;
    serde_json::from_slice(&json).map_err(|_| libc::EINVAL)
}

/// As root, become `uid:gid` with no supplementary groups. As anyone else the
/// target must be ourselves: a non-root daemon cannot switch users.
fn drop_privileges(uid: u32, gid: u32) -> Result<(), i32> {
    use rustix::process::{Gid, Uid, getegid, geteuid};
    let (euid, egid) = (geteuid().as_raw(), getegid().as_raw());
    if euid != 0 {
        return if (uid, gid) == (euid, egid) {
            Ok(())
        } else {
            Err(libc::EPERM)
        };
    }
    let errno = |e: rustix::io::Errno| e.raw_os_error();
    rustix::thread::set_thread_groups(&[]).map_err(errno)?;
    rustix::thread::set_thread_gid(Gid::from_raw(gid)).map_err(errno)?;
    rustix::thread::set_thread_uid(Uid::from_raw(uid)).map_err(errno)
}

fn open_stdio(spec: &StdioSpec) -> Result<Stdio, i32> {
    let flags = OFlags::WRONLY | OFlags::CREATE | OFlags::CLOEXEC | OFlags::NONBLOCK;
    let (path, flags) = match spec {
        StdioSpec::Inherit => return Ok(Stdio::inherit()),
        StdioSpec::Null => return Ok(Stdio::null()),
        StdioSpec::File(p) | StdioSpec::Truncate(p) => (p, flags | OFlags::TRUNC),
        StdioSpec::Append(p) => (p, flags | OFlags::APPEND),
    };
    let fd = rustix::fs::open(path.as_str(), flags, Mode::from_bits_truncate(0o644))
        .map_err(|e| e.raw_os_error())?;
    Ok(Stdio::from(fd))
}

//! Process helpers: waiting, signalling and walking a cgroup's pids.

use std::path::Path;
use std::time::{Duration, Instant};

use super::state::StateKind;

/// Passwd lookup per spawn; spawns are rare, so no cache.
pub(super) fn primary_gid(uid: u32) -> u32 {
    uzers::get_user_by_uid(uid)
        .map(|u| u.primary_group_id())
        .unwrap_or(uid)
}

/// Polling waitpid. `deadline: None` blocks until the child exits;
/// `Some` returns `Ok(None)` on timeout (or when the pid is not ours).
pub(super) fn waitpid_until(
    pid: libc::pid_t,
    deadline: Option<Instant>,
) -> Result<Option<rustix::process::WaitStatus>, String> {
    use rustix::process::{Pid, WaitOptions, waitpid};
    let Some(want) = Pid::from_raw(pid) else {
        return Err(format!("waitpid: bad pid {pid}"));
    };
    loop {
        match waitpid(Some(want), WaitOptions::NOHANG) {
            Ok(Some((_p, status))) => return Ok(Some(status)),
            Ok(None) => {}
            Err(rustix::io::Errno::INTR) => continue,
            Err(rustix::io::Errno::CHILD) => return Ok(None),
            Err(e) => return Err(format!("waitpid: {e}")),
        }
        if deadline.is_some_and(|d| Instant::now() >= d) {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

pub(super) fn waitpid_blocking(pid: libc::pid_t) -> Result<rustix::process::WaitStatus, String> {
    match waitpid_until(pid, None)? {
        Some(st) => Ok(st),
        // NOHANG loop only returns None on CHILD (pid not ours).
        None => Err(format!("waitpid: pid {pid} not a child")),
    }
}

/// 100ms-poll until a unit tree is empty or the deadline passes.
pub(super) fn wait_until_empty(dir: &Path, deadline: Instant) {
    while Instant::now() < deadline {
        if tree_pids_alive(dir).is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub(super) fn decode_status(
    st: rustix::process::WaitStatus,
) -> (StateKind, Option<i32>, Option<i32>) {
    if let Some(code) = st.exit_status() {
        if code == 0 {
            (StateKind::Inactive, Some(0), None)
        } else {
            (StateKind::Failed, Some(code), None)
        }
    } else if let Some(sig) = st.terminating_signal() {
        (StateKind::Failed, None, Some(sig))
    } else {
        (StateKind::Failed, None, None)
    }
}

pub(super) fn tree_pids(dir: &Path) -> Vec<u32> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    let mut truncated = false;
    while let Some(d) = stack.pop() {
        if out.len() > 4096 {
            truncated = true;
            break;
        }
        if let Ok(content) = std::fs::read_to_string(d.join("cgroup.procs")) {
            for tok in content.split_whitespace() {
                if let Ok(pid) = tok.parse::<u32>() {
                    out.push(pid);
                }
            }
        }
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                }
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    if truncated {
        crate::log::warning(&format!(
            "tree_pids {} truncated at 4096 (pids may remain)",
            dir.display()
        ));
    }
    out
}

pub(super) fn signal_pids(pids: &[u32], sig: rustix::process::Signal) {
    use rustix::process::{Pid, kill_process};
    for &pid in pids {
        let Ok(raw): Result<i32, _> = pid.try_into() else {
            continue;
        };
        if let Some(p) = Pid::from_raw(raw) {
            let _ = kill_process(p, sig);
        }
    }
}

pub(super) fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

pub(super) fn tree_pids_alive(dir: &Path) -> Vec<u32> {
    tree_pids(dir)
        .into_iter()
        .filter(|&p| pid_alive(p))
        .collect()
}

pub(super) fn scrub_dead_pids(dir: &Path, dead: &[u32]) {
    if dead.is_empty() {
        return;
    }
    if dir.join("cgroup.events").exists() {
        return;
    }
    let procs = dir.join("cgroup.procs");
    let Ok(content) = std::fs::read_to_string(&procs) else {
        return;
    };
    let dead_s: Vec<String> = dead.iter().map(u32::to_string).collect();
    let rest: Vec<&str> = content
        .split_whitespace()
        .filter(|t| !dead_s.iter().any(|d| d == t))
        .collect();
    let want = if rest.is_empty() {
        String::new()
    } else {
        rest.join("\n") + "\n"
    };
    if want != content {
        let _ = std::fs::write(&procs, want);
    }
}

pub(super) fn write_pid_to_procs(dir: &Path, pid: u32) -> Result<(), String> {
    let procs = dir.join("cgroup.procs");
    std::fs::write(&procs, format!("{pid}\n"))
        .map_err(|e| format!("write {}: {e}", procs.display()))
}

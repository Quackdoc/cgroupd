//! Pre/post hooks.

use std::path::Path;
use std::time::{Duration, Instant};

use cgroupd_common::StdioSpec;

use super::exec::{self, kill_and_reap};
use super::proc::{
    decode_status, scrub_dead_pids, waitpid_blocking, waitpid_until, write_pid_to_procs,
};
use super::state::StateKind;
use crate::exec_helper::Spec;

pub(super) const MAX_HOOKS: usize = 16;

pub(super) const MAX_GRACE_SECS: u64 = 300;

pub(super) const MAX_START_TIMEOUT_SECS: u64 = 600;

/// Clamped `Instant::now() + secs`.
pub(super) fn deadline_in(secs: u64, max: u64) -> Instant {
    Instant::now() + Duration::from_secs(secs.clamp(5, max))
}

pub(super) fn run_hook(
    cgroup_dir: &Path,
    uid: u32,
    gid: u32,
    argv: &[String],
    env: &[String],
    deadline: Instant,
) -> Result<i32, String> {
    if argv.is_empty() {
        return Err("empty hook command".to_string());
    }
    cgroupd_common::validate_program(&argv[0])?;
    cgroupd_common::validate_args(&argv[1..])?;
    let (pid, barrier) = exec::start(&Spec {
        uid,
        gid,
        cwd: None,
        umask: None,
        stdout: StdioSpec::Inherit,
        stderr: StdioSpec::Inherit,
        program: argv[0].clone(),
        args: argv[1..].to_vec(),
        env: env.to_vec(),
    })?;
    let _ = write_pid_to_procs(cgroup_dir, pid as u32);
    match barrier.release_and_wait() {
        Ok(None) => {}
        Ok(Some(errno)) => {
            let _ = waitpid_blocking(pid);
            scrub_dead_pids(cgroup_dir, &[pid as u32]);
            return Err(format!(
                "exec {}: {}",
                argv[0],
                std::io::Error::from_raw_os_error(errno)
            ));
        }
        Err(e) => {
            kill_and_reap(pid);
            return Err(e);
        }
    }
    match waitpid_until(pid, Some(deadline))? {
        Some(status) => {
            scrub_dead_pids(cgroup_dir, &[pid as u32]);
            let (kind, code, _) = decode_status(status);
            if kind == StateKind::Inactive {
                Ok(0)
            } else {
                Err(format!("hook exited with code {}", code.unwrap_or(-1)))
            }
        }
        None => {
            kill_and_reap(pid);
            scrub_dead_pids(cgroup_dir, &[pid as u32]);
            Err("hook timeout".to_string())
        }
    }
}

pub(super) fn run_hooks(
    cgroup_dir: &Path,
    uid: u32,
    gid: u32,
    hooks: &[Vec<String>],
    env: &[String],
    deadline: Instant,
    label: &str,
) -> Result<(), String> {
    for (i, h) in hooks.iter().enumerate() {
        if Instant::now() >= deadline {
            return Err(format!("{label} hooks: start timeout"));
        }
        run_hook(cgroup_dir, uid, gid, h, env, deadline)
            .map_err(|e| format!("{label} hook {i}: {e}"))?;
    }
    Ok(())
}

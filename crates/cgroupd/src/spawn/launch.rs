//! Fork+setuid+exec into a delegated app cgroup, and reaping the child.
//!
//! `spawn_app` runs these steps in order:
//!
//! 1. **Validate** every field of the request and the per-user / global caps.
//! 2. **Place**: pick the login session and create `<sid>/app/<aid>`.
//! 3. **Prepare**: apply `--limit`s and run the `pre` hooks.
//! 4. **Start** an exec helper (`crate::exec_helper`); it blocks until told to go.
//! 5. **Contain**: the daemon writes the helper's pid into the app cgroup, and
//!    only then releases it, so the program never runs outside its cgroup.
//! 6. **Exec**: the helper drops privileges and execs. If anything fails it
//!    sends `errno` back; a successful exec closes its socket, which the
//!    daemon reads as EOF.
//! 7. **Track**: record the app and reap it from a background thread.

use std::path::PathBuf;

use cgroupd_common as C;
use cgroupd_common::{Limit, Roots, Scope, StdioSpec, app_unit_path};

use crate::config::Config;
use crate::exec_helper::Spec;

use super::exec::{self, kill_and_reap};
use super::hooks::{MAX_GRACE_SECS, MAX_HOOKS, MAX_START_TIMEOUT_SECS, deadline_in, run_hooks};
use super::limits::apply_limits;
use super::proc::{
    decode_status, primary_gid, scrub_dead_pids, tree_pids_alive, waitpid_blocking,
    write_pid_to_procs,
};
use super::state::{SpawnRecord, StateKind, check_app_limits, prune_done, states};

pub struct SpawnParams<'a> {
    pub uid: u32,
    /// The login session the app belongs to (default: the user's only session).
    pub sid: Option<&'a str>,
    pub aid: &'a str,
    pub program: &'a str,
    pub args: &'a [String],
    pub env: &'a [String],
    pub stdout: &'a StdioSpec,
    pub stderr: &'a StdioSpec,
    pub cwd: &'a Option<String>,
    pub umask: Option<u32>,
    pub limits: &'a [Limit],
    pub pre: &'a [Vec<String>],
    pub post: &'a [Vec<String>],
}

pub fn spawn_app(roots: &Roots, cfg: &Config, p: &SpawnParams) -> Result<String, String> {
    validate(cfg, p)?;

    let sid = session_for(roots, p.uid, p.sid)?;
    let cgroup = crate::fs::flat_app_dir(roots, cfg, p.uid, &sid, p.aid)?;
    apply_limits(&cgroup, p.limits)?;

    let gid = primary_gid(p.uid);
    let deadline = deadline_in(cfg.spawn_start_timeout_secs, MAX_START_TIMEOUT_SECS);
    run_hooks(&cgroup, p.uid, gid, p.pre, p.env, deadline, "pre")?;

    let (pid, barrier) = exec::start(&Spec {
        uid: p.uid,
        gid,
        cwd: p.cwd.clone(),
        umask: p.umask,
        stdout: p.stdout.clone(),
        stderr: p.stderr.clone(),
        program: p.program.to_string(),
        args: p.args.to_vec(),
        env: p.env.to_vec(),
    })?;
    if let Err(e) = write_pid_to_procs(&cgroup, pid as u32) {
        kill_and_reap(pid);
        return Err(format!("move to {}: {e}", cgroup.display()));
    }
    match barrier.release_and_wait() {
        Ok(None) => {}
        Ok(Some(errno)) => {
            let _ = waitpid_blocking(pid);
            return Err(format!(
                "exec {}: {}",
                p.program,
                std::io::Error::from_raw_os_error(errno)
            ));
        }
        Err(e) => {
            kill_and_reap(pid);
            return Err(e);
        }
    }

    track(roots, cfg, p, pid)?;
    Ok(format!("spawned app {} pid {pid}", p.aid))
}

/// Step 1: reject anything malformed or over quota before touching the system.
fn validate(cfg: &Config, p: &SpawnParams) -> Result<(), String> {
    if !cfg.allow_spawn {
        return Err("spawn disabled (allow_spawn=false)".to_string());
    }
    C::validate_uid(p.uid, cfg.allow_root)?;
    C::validate_sid(p.aid)?;
    C::validate_program(p.program)?;
    C::validate_args(p.args)?;
    C::validate_env(p.env)?;
    C::validate_stdio(p.stdout)?;
    C::validate_stdio(p.stderr)?;
    C::validate_cwd(p.cwd)?;
    C::validate_umask(p.umask)?;
    for lim in p.limits {
        C::validate_limit(lim)?;
    }
    if p.pre.len() > MAX_HOOKS || p.post.len() > MAX_HOOKS {
        return Err(format!("too many hooks (max {MAX_HOOKS} each)"));
    }
    for hook in p.pre.iter().chain(p.post) {
        let (program, args) = hook.split_first().ok_or("empty hook command")?;
        C::validate_program(program)?;
        C::validate_args(args)?;
    }
    check_app_limits(p.uid)
}

/// Step 2: the session an app runs in: the named one, else the user's only one.
fn session_for(roots: &Roots, uid: u32, sid: Option<&str>) -> Result<String, String> {
    if let Some(sid) = sid {
        C::validate_sid(sid)?;
        return Ok(sid.to_string());
    }
    let mut mine = C::flat_sessions(roots, Some(uid));
    match mine.len() {
        1 => Ok(mine.remove(0)),
        0 => Err(format!("uid {uid} has no session to run the app in")),
        _ => Err("several sessions: say which one (sid)".to_string()),
    }
}

/// Step 7: remember the app and reap it in the background.
fn track(roots: &Roots, cfg: &Config, p: &SpawnParams, pid: libc::pid_t) -> Result<(), String> {
    let record = SpawnRecord {
        pid: pid as u32,
        state: StateKind::Active,
        exit_code: None,
        signal: None,
        post: p.post.to_vec(),
        env: p.env.to_vec(),
    };
    states()
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?
        .insert((p.uid, Scope::App, p.aid.to_string()), record);
    prune_done();

    let (roots, cfg) = (roots.clone(), cfg.clone());
    let (uid, aid) = (p.uid, p.aid.to_string());
    std::thread::spawn(move || reap_child(&roots, &cfg, uid, &aid, pid));
    Ok(())
}

/// What a finished app's record says still needs doing.
struct PostExit {
    post: Vec<Vec<String>>,
    env: Vec<String>,
}

/// Store how the app ended. Returns the post hooks to run only if the app was
/// still `Active`; otherwise (already stopping, or replaced) whoever changed
/// its state owns the cleanup.
fn record_exit(
    uid: u32,
    aid: &str,
    pid: libc::pid_t,
    (kind, code, sig): (StateKind, Option<i32>, Option<i32>),
) -> Option<PostExit> {
    let mut states = states().lock().ok()?;
    let rec = states.get_mut(&(uid, Scope::App, aid.to_string()))?;
    if rec.pid != pid as u32 {
        return None;
    }
    let was_active = rec.state == StateKind::Active;
    (rec.state, rec.exit_code, rec.signal) = (kind, code, sig);
    was_active.then(|| PostExit {
        post: rec.post.clone(),
        env: rec.env.clone(),
    })
}

fn describe_exit(code: Option<i32>, sig: Option<i32>) -> String {
    match (code, sig) {
        (Some(0), _) => "code 0".to_string(),
        (Some(c), _) => format!("code {c}"),
        (_, Some(s)) => format!("signal {s}"),
        _ => "unknown".to_string(),
    }
}

/// Wait for the app, run its post hooks, and remove its cgroup once empty.
pub(super) fn reap_child(roots: &Roots, cfg: &Config, uid: u32, aid: &str, pid: libc::pid_t) {
    let Ok(status) = waitpid_blocking(pid) else {
        return;
    };
    let exit = decode_status(status);
    let Some(after) = record_exit(uid, aid, pid, exit) else {
        return;
    };

    let cgroup: PathBuf = app_unit_path(roots, uid, aid);
    if !after.post.is_empty() {
        let deadline = deadline_in(cfg.default_grace_secs, MAX_GRACE_SECS);
        let gid = primary_gid(uid);
        let _ = run_hooks(&cgroup, uid, gid, &after.post, &after.env, deadline, "post");
    }
    scrub_dead_pids(&cgroup, &[pid as u32]);
    if tree_pids_alive(&cgroup).is_empty() {
        let _ = crate::fs::remove_target(roots, cfg, uid, Scope::App, aid);
    }
    crate::log::info(&format!(
        "app {aid} pid {pid} exited: {}",
        describe_exit(exit.1, exit.2)
    ));
}

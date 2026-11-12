//! Stop, state and wait for running apps.

use std::time::{Duration, Instant};

use cgroupd_common::{Scope, unit_path};

use crate::config::Config;

use super::hooks::{MAX_GRACE_SECS, deadline_in, run_hooks};
use super::proc::{
    pid_alive, primary_gid, scrub_dead_pids, signal_pids, tree_pids, tree_pids_alive,
    wait_until_empty,
};
use super::state::{StateKind, WaitGuard, drop_state, state_json, states};

pub fn stop_target(
    roots: &cgroupd_common::Roots,
    cfg: &Config,
    uid: u32,
    scope: Scope,
    id: &str,
    grace_secs: u64,
) -> Result<String, String> {
    cgroupd_common::validate_uid(uid, cfg.allow_root)?;
    cgroupd_common::validate_sid(id)?;
    let noun = scope.noun();
    let grace_secs = grace_secs.min(MAX_GRACE_SECS);
    let sp = unit_path(roots, uid, scope, id);
    if !sp.exists() {
        drop_state(uid, scope, id);
        return Ok(format!("{noun} {id} already gone"));
    }
    // Active -> Stopping wins the post-hook race with the reaper.
    let (post, env) = {
        let Ok(mut m) = states().lock() else {
            return Err("state lock poisoned".to_string());
        };
        match m.get_mut(&(uid, scope, id.to_string())) {
            Some(rec) if rec.state == StateKind::Active => {
                rec.state = StateKind::Stopping;
                (rec.post.clone(), rec.env.clone())
            }
            _ => (Vec::new(), Vec::new()),
        }
    };
    let deadline = Instant::now() + Duration::from_secs(grace_secs);
    signal_pids(&tree_pids_alive(&sp), rustix::process::Signal::TERM);
    wait_until_empty(&sp, deadline);
    if !post.is_empty() {
        let gid = primary_gid(uid);
        let hook_deadline = deadline_in(grace_secs, MAX_GRACE_SECS);
        let _ = run_hooks(&sp, uid, gid, &post, &env, hook_deadline, "post");
    }
    let left = tree_pids_alive(&sp);
    if !left.is_empty() {
        signal_pids(&left, rustix::process::Signal::KILL);
        wait_until_empty(&sp, Instant::now() + Duration::from_secs(5));
    }
    let dead: Vec<u32> = tree_pids(&sp)
        .into_iter()
        .filter(|p| !pid_alive(*p))
        .collect();
    scrub_dead_pids(&sp, &dead);
    let msg = if tree_pids_alive(&sp).is_empty() {
        match crate::fs::remove_target(roots, cfg, uid, scope, id) {
            Ok(d) => d,
            Err(e) => format!("{noun} {id} stopped, cleanup: {e}"),
        }
    } else {
        format!("{noun} {id} stopped, pids remain")
    };
    Ok(msg)
}

/// Back-compat wrapper: sessions predate scopes.
pub fn get_state(
    roots: &cgroupd_common::Roots,
    cfg: &Config,
    uid: u32,
    scope: Scope,
    id: &str,
) -> Result<String, String> {
    cgroupd_common::validate_uid(uid, cfg.allow_root)?;
    cgroupd_common::validate_sid(id)?;
    let noun = scope.noun();
    if let Ok(m) = states().lock()
        && let Some(rec) = m.get(&(uid, scope, id.to_string()))
    {
        let sp = unit_path(roots, uid, scope, id);
        if !sp.exists() {
            drop(m);
            drop_state(uid, scope, id);
            return Ok(format!("{noun} {id} already gone"));
        }
        let pids = tree_pids_alive(&sp);
        return Ok(state_json(rec, scope, id, true, &pids));
    }
    // Unmanaged units were placed via CreateSession (sessions) or
    // `cgctl enter` (either scope).
    let sp = unit_path(roots, uid, scope, id);
    if !sp.exists() {
        return Ok(format!("{noun} {id} already gone"));
    }
    let pids = tree_pids_alive(&sp);
    if pids.is_empty() {
        Ok(serde_json::json!(
        {
            "scope": scope.noun(),
            "session": id,
            "state": "inactive",
            "managed": false,
            "pids": pids
        })
        .to_string())
    } else {
        Ok(serde_json::json!(
        {
            "scope": scope.noun(),
            "session": id,
            "state": "active",
            "managed": false,
            "pids": pids
        })
        .to_string())
    }
}

pub fn wait_session(
    roots: &cgroupd_common::Roots,
    cfg: &Config,
    uid: u32,
    scope: Scope,
    id: &str,
    timeout_secs: u64,
) -> Result<String, String> {
    cgroupd_common::validate_uid(uid, cfg.allow_root)?;
    cgroupd_common::validate_sid(id)?;
    let noun = scope.noun();
    let _guard = match WaitGuard::try_enter() {
        Some(g) => g,
        None => return Err("server busy (too many waiters)".to_string()),
    };
    let timeout_secs = timeout_secs.min(3600);
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        let done = {
            let Ok(m) = states().lock() else {
                return Err("state lock poisoned".to_string());
            };
            match m.get(&(uid, scope, id.to_string())) {
                Some(rec) if rec.state == StateKind::Inactive || rec.state == StateKind::Failed => {
                    let sp = unit_path(roots, uid, scope, id);
                    Some(state_json(rec, scope, id, true, &tree_pids(&sp)))
                }
                Some(_) => None,
                None => {
                    let sp = unit_path(roots, uid, scope, id);
                    if !sp.exists() {
                        Some(format!("{noun} {id} already gone"))
                    } else if tree_pids_alive(&sp).is_empty() {
                        Some(serde_json::json!({"scope": scope.noun(), "session": id, "state": "inactive", "managed": false}).to_string())
                    } else {
                        None
                    }
                }
            }
        };
        if let Some(d) = done {
            return Ok(d);
        }
        if Instant::now() >= deadline {
            return Err("wait timeout".to_string());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

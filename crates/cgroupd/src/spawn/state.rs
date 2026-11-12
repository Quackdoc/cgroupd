//! Per-app state records, spawn caps and the wait-slot budget.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use cgroupd_common::Scope;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StateKind {
    Active,
    Stopping,
    Inactive,
    Failed,
}

impl StateKind {
    fn as_str(self) -> &'static str {
        match self {
            StateKind::Active => "active",
            StateKind::Stopping => "stopping",
            StateKind::Inactive => "inactive",
            StateKind::Failed => "failed",
        }
    }
}

pub(super) struct SpawnRecord {
    pub(super) pid: u32,
    pub(super) state: StateKind,
    pub(super) exit_code: Option<i32>,
    pub(super) signal: Option<i32>,
    pub(super) post: Vec<Vec<String>>,
    pub(super) env: Vec<String>,
}

/// (owning uid, scope, id) of a spawned app.
pub(super) type AppKey = (u32, Scope, String);

pub(super) fn states() -> &'static Mutex<HashMap<AppKey, SpawnRecord>> {
    static STATES: OnceLock<Mutex<HashMap<AppKey, SpawnRecord>>> = OnceLock::new();
    STATES.get_or_init(|| Mutex::new(HashMap::new()))
}

// Separate budget for blocking WaitSession so wait-floods can't exhaust the
// 64-slot connection pool (login/placement traffic always gets a slot).
pub(super) static ACTIVE_WAITERS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

pub(crate) const MAX_WAITERS: usize = 32;

pub(crate) struct WaitGuard;

impl WaitGuard {
    pub(crate) fn try_enter() -> Option<Self> {
        let n = ACTIVE_WAITERS.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        if n > MAX_WAITERS {
            ACTIVE_WAITERS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            return None;
        }
        Some(WaitGuard)
    }
}

impl Drop for WaitGuard {
    fn drop(&mut self) {
        ACTIVE_WAITERS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

pub(super) fn prune_done() {
    let Ok(mut m) = states().lock() else { return };
    if m.len() <= 1024 {
        return;
    }
    m.retain(|_, r| r.state == StateKind::Active || r.state == StateKind::Stopping);
}

pub(super) fn state_json(
    rec: &SpawnRecord,
    scope: Scope,
    id: &str,
    managed: bool,
    pids: &[u32],
) -> String {
    let mut o = serde_json::json!({
        "scope": match scope {
            Scope::Session => "session",
            Scope::App => "app",
            Scope::Service => "service",
        },
        "session": id,
        "state": rec.state.as_str(),
        "pid": rec.pid,
        "managed": managed,
        "pids": pids,
    });
    if let Some(c) = rec.exit_code {
        o["exit_code"] = c.into();
    }
    if let Some(s) = rec.signal {
        o["signal"] = s.into();
    }
    o.to_string()
}

pub fn drop_state(uid: u32, scope: Scope, id: &str) {
    if let Ok(mut m) = states().lock() {
        m.remove(&(uid, scope, id.to_string()));
    }
}

pub(crate) const MAX_APPS_PER_UID: usize = 32;

pub(crate) const MAX_APPS_GLOBAL: usize = 256;

pub(super) fn check_app_limits(uid: u32) -> Result<(), String> {
    let Ok(m) = states().lock() else {
        return Err("state lock poisoned".to_string());
    };
    let mut per_uid = 0usize;
    let mut total = 0usize;
    for ((u, _scope, _), rec) in m.iter() {
        if rec.state == StateKind::Active || rec.state == StateKind::Stopping {
            total += 1;
            if *u == uid {
                per_uid += 1;
            }
        }
    }
    if per_uid >= MAX_APPS_PER_UID {
        return Err(format!(
            "too many active apps for uid {uid} (max {MAX_APPS_PER_UID})"
        ));
    }
    if total >= MAX_APPS_GLOBAL {
        return Err(format!(
            "too many active apps globally (max {MAX_APPS_GLOBAL})"
        ));
    }
    Ok(())
}

//! Removing empty units and pruning their session container.

use std::path::Path;

use cgroupd_common::{Roots, Scope, owns_session, session_dir};

use crate::config::Config;

/// Drop a session container (`<sid>/`) once it holds no session and no apps.
/// Only if `uid` owns it; `rmdir` refuses while anything is left inside.
pub(super) fn prune_container(container: &Path, uid: u32) {
    use std::os::unix::fs::MetadataExt;
    if std::fs::metadata(container).is_ok_and(|m| m.uid() == uid) {
        let _ = std::fs::remove_dir(container.join("app"));
        let _ = std::fs::remove_dir(container);
    }
}

/// The id a unit is known by (a session's is its container's name).
pub(super) fn unit_id(scope: Scope, sp: &Path) -> &str {
    let named = if scope == Scope::Session {
        sp.parent()
    } else {
        Some(sp)
    };
    named
        .and_then(Path::file_name)
        .and_then(|n| n.to_str())
        .unwrap_or("?")
}

/// The session container a unit lives in (none for services).
pub(super) fn container_of(scope: Scope, sp: &Path) -> Option<&Path> {
    match scope {
        Scope::Session => sp.parent(),
        Scope::App => sp.parent()?.parent(),
        Scope::Service => None,
    }
}

pub(super) fn populated(sp: &Path) -> Option<bool> {
    let content = std::fs::read_to_string(sp.join("cgroup.events")).ok()?;
    for line in content.lines() {
        if let Some(v) = line.strip_prefix("populated") {
            return match v.trim() {
                "0" => Some(false),
                "1" => Some(true),
                _ => None,
            };
        }
    }
    None
}

pub(super) fn procs_busy(sp: &Path) -> bool {
    match std::fs::read_to_string(sp.join("cgroup.procs")) {
        Ok(c) => !c.trim().is_empty(),
        Err(_) => false,
    }
}

pub fn remove_target(
    roots: &Roots,
    cfg: &Config,
    uid: u32,
    scope: Scope,
    id: &str,
) -> Result<String, String> {
    cgroupd_common::validate_uid(uid, cfg.allow_root)?;
    cgroupd_common::validate_sid(id)?;
    let sp = cgroupd_common::unit_path(roots, uid, scope, id);
    if !sp.exists() {
        if scope == Scope::Session && owns_session(roots, uid, id) {
            prune_container(&session_dir(roots, id), uid);
        }
        return Ok(format!("{} {id} already gone", scope.noun()));
    }
    Ok(remove_at(uid, scope, &sp))
}

/// Remove the unit at `sp` if it is empty, then prune its session container.
/// The reaper calls this directly with the path it already holds.
pub(crate) fn remove_at(uid: u32, scope: Scope, sp: &Path) -> String {
    let label = format!("{} {}", scope.noun(), unit_id(scope, sp));
    let busy = match populated(sp) {
        Some(p) => p,
        None => procs_busy(sp),
    };
    if busy {
        return format!("{label} busy, left in place");
    }
    let result = match std::fs::remove_dir(sp) {
        Ok(()) => format!("{label} removed"),
        Err(e) => format!("{label} not empty, left ({e})"),
    };
    if let Some(c) = container_of(scope, sp) {
        prune_container(c, uid);
    }
    result
}

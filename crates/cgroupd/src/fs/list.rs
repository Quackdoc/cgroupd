//! Listing units and the startup/overflow sweep.

use std::path::PathBuf;

use cgroupd_common::{
    Roots, Scope, flat_containers, flat_sessions, session_app_dir, session_dir, session_path,
};

use crate::config::Config;

use super::remove::{prune_container, remove_at};

/// True when a directory entry is a subdirectory. Every cgroup dir is
/// itself a cgroup on real cgroupfs, so container dirs also hold kernel
/// interface files (`cgroup.procs`, ...) that must be skipped when
/// enumerating units.
pub(super) fn is_unit_dir(e: &std::fs::DirEntry) -> bool {
    e.file_type()
        .map(|t| t.is_dir())
        .unwrap_or_else(|_| e.path().is_dir())
}

/// List subdirectories of `dir`, skipping kernel interface files.
pub(super) fn list_child_dirs(dir: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            if let Some(n) = e.file_name().to_str()
                && is_unit_dir(&e)
            {
                out.push(n.to_string());
            }
        }
    }
    out.sort();
    out
}

pub fn list_sessions(roots: &Roots, cfg: &Config, uid: u32) -> Result<String, String> {
    cgroupd_common::validate_uid(uid, cfg.allow_root)?;
    Ok(format!(
        "sessions: {}",
        flat_sessions(roots, Some(uid)).join(",")
    ))
}

pub fn list_apps(roots: &Roots, cfg: &Config, uid: u32) -> Result<String, String> {
    cgroupd_common::validate_uid(uid, cfg.allow_root)?;
    let mut apps: Vec<String> = flat_containers(roots, Some(uid))
        .iter()
        .flat_map(|sid| list_child_dirs(&session_app_dir(roots, sid)))
        .collect();
    apps.sort();
    Ok(format!("apps: {}", apps.join(",")))
}

pub fn sweep(roots: &Roots) -> String {
    use std::os::unix::fs::MetadataExt;
    let mut removed = 0u32;
    let mut kept = 0u32;
    for sid in flat_containers(roots, None) {
        let container = session_dir(roots, &sid);
        let Ok(uid) = std::fs::metadata(&container).map(|m| m.uid()) else {
            continue;
        };
        let mut tally = |r: String| {
            if r.contains("removed") {
                removed += 1;
            } else {
                kept += 1;
            }
        };
        let mut unit = |scope: Scope, sp: PathBuf| {
            // Watch before checking so an emptying between the two isn't missed.
            crate::reaper::watch(&sp, uid, scope);
            tally(remove_at(uid, scope, &sp));
        };
        let apps = session_app_dir(roots, &sid);
        for aid in list_child_dirs(&apps) {
            unit(Scope::App, apps.join(aid));
        }
        let sp = session_path(roots, &sid);
        if sp.is_dir() {
            unit(Scope::Session, sp);
        } else {
            prune_container(&container, uid);
        }
    }
    format!("sweep: removed {removed} kept {kept}")
}

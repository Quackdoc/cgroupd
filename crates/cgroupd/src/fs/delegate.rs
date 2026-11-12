//! Ownership and controller delegation of cgroup directories.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use cgroupd_common::{Roots, delegate_files, is_fatal_delegate};

use crate::config::Config;

pub fn ensure_dir(path: &Path) -> Result<(), String> {
    match std::fs::create_dir_all(path) {
        Ok(()) => Ok(()),
        Err(e) => Err(format!("mkdir {}: {e}", path.display())),
    }
}

pub(super) fn chown_uid(path: &Path, uid: u32) -> Result<(), String> {
    rustix::fs::chown(
        path,
        Some(rustix::process::Uid::from_raw(uid)),
        Some(rustix::process::Gid::from_raw(uid)),
    )
    .map_err(|e| format!("chown {} {uid}: {e}", path.display()))
}

pub(super) fn chown_if_exists(path: &Path, uid: u32) {
    if let Err(e) = chown_uid(path, uid)
        && !e.ends_with("No such file or directory (os error 2)")
    {
        crate::log::warning(&e);
    }
}

pub(super) fn chmod_if_exists(path: &Path, mode: u32) {
    if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        && e.kind() != std::io::ErrorKind::NotFound
    {
        crate::log::warning(&format!("chmod {} {:o}: {e}", path.display(), mode));
    }
}

pub(super) fn mark_delegated(path: &Path) {
    let _ = rustix::fs::setxattr(path, "user.delegate", b"1", rustix::fs::XattrFlags::empty());
}

/// Delegate one cgroup dir to `uid`.
pub(crate) fn delegate_dir(path: &Path, uid: u32) -> Result<(), String> {
    chown_uid(path, uid)?;
    chmod_if_exists(path, 0o755);
    for f in delegate_files() {
        let p = path.join(f);
        if !p.exists() {
            continue;
        }
        if let Err(e) = chown_uid(&p, uid) {
            if is_fatal_delegate(f) {
                crate::log::warning(&e);
            }
            continue;
        }
        chmod_if_exists(&p, 0o644);
    }
    mark_delegated(path);
    Ok(())
}

/// Make `dir` a directory owned by `uid` (created if needed). Refuses to touch an
/// existing directory owned by someone else, so a login can never grab (and chown)
/// an unrelated top-level cgroup such as a service's.
pub(super) fn ensure_owned_dir(dir: &Path, uid: u32) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    match std::fs::metadata(dir) {
        Ok(m) if m.is_dir() && m.uid() == uid => return Ok(()),
        Ok(_) => {
            return Err(format!(
                "{} exists and is not owned by uid {uid}",
                dir.display()
            ));
        }
        Err(_) => {}
    }
    ensure_dir(dir)?;
    chown_uid(dir, uid)?;
    chmod_if_exists(dir, 0o755);
    Ok(())
}

pub fn init_hierarchy(roots: &Roots, cfg: &Config) -> Result<(), String> {
    ensure_dir(&roots.sessions)?;
    enable_controllers(roots, cfg);
    Ok(())
}

pub(super) fn write_one_controller(ctl: &std::path::Path, controller: &str) {
    let body = format!("+{controller}");
    match std::fs::read_to_string(ctl) {
        // Kernel reports enabled controllers bare (`cpu`); writes use `+cpu`.
        Ok(current)
            if current
                .split_whitespace()
                .any(|w| w == body || w == controller) => {}
        _ => match std::fs::write(ctl, &body) {
            Ok(()) => crate::log::info(&format!(
                "enabled controller {controller} {}",
                ctl.display()
            )),
            Err(e) => crate::log::warning(&format!(
                "subtree_control {} +{controller}: {e} (continuing)",
                ctl.display()
            )),
        },
    }
}

/// Delegate controllers from the top-level cgroup to sessions. The top level is
/// init's, so this only happens when `manage_ancestors` is set.
pub fn enable_controllers(roots: &Roots, cfg: &Config) {
    if !cfg.manage_ancestors {
        return;
    }
    let ctl = roots.sessions.join("cgroup.subtree_control");
    for c in &cfg.controllers {
        write_one_controller(&ctl, c);
    }
}

/// Enable the configured controllers for `dir`'s children, as far as `dir` itself
/// has them (i.e. its parent delegated them).
pub(super) fn enable_in(dir: &Path, cfg: &Config) {
    let Ok(have) = std::fs::read_to_string(dir.join("cgroup.controllers")) else {
        return;
    };
    let ctl = dir.join("cgroup.subtree_control");
    for c in cfg
        .controllers
        .iter()
        .filter(|c| have.split_whitespace().any(|h| h == c.as_str()))
    {
        write_one_controller(&ctl, c);
    }
}

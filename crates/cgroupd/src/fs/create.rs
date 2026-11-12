//! Creating users, sessions, apps and services.

use cgroupd_common::{
    Roots, Scope, locate_app, new_app_path, owns_session, pid_session, session_app_dir,
    session_dir, user_path,
};

use crate::config::Config;

use super::delegate::{
    chmod_if_exists, chown_if_exists, chown_uid, delegate_dir, enable_in, ensure_dir,
    ensure_owned_dir,
};
use super::remove::remove_at;

pub fn ensure_user(roots: &Roots, uid: u32, allow_root: bool) -> Result<(), String> {
    cgroupd_common::validate_uid(uid, allow_root)?;
    let up = user_path(roots, uid);
    // Sessions are top-level `<sid>/`; the user's own cgroup only holds services.
    ensure_owned_dir(&up, uid)?;
    let svc = up.join("services");
    ensure_dir(&svc)?;
    chown_uid(&svc, uid)?;
    chmod_if_exists(&svc, 0o755);
    Ok(())
}

pub fn create_session(
    roots: &Roots,
    cfg: &Config,
    uid: u32,
    scope: Scope,
    id: &str,
    pid: u32,
) -> Result<String, String> {
    cgroupd_common::validate_uid(uid, cfg.allow_root)?;
    cgroupd_common::validate_sid(id)?;
    if pid == 0 {
        return Err("pid 0 invalid".to_string());
    }
    let noun = scope.noun();
    if scope == Scope::Service {
        ensure_user(roots, uid, cfg.allow_root)?;
    }
    let sp = match scope {
        Scope::Session => create_flat_session(roots, cfg, uid, id)?,
        Scope::App => create_flat_app(roots, cfg, uid, id, pid)?,
        Scope::Service => create_service(roots, uid, id)?,
    };
    // Watch before the pid lands so the first populated flip is seen.
    crate::reaper::watch(&sp, uid, scope);
    let procs = sp.join("cgroup.procs");
    if let Err(e) = std::fs::write(&procs, format!("{pid}\n")) {
        // Never populated, so no event would ever reap the empty unit.
        remove_at(uid, scope, &sp);
        return Err(format!("write {}: {e}", procs.display()));
    }
    chown_if_exists(&procs, uid);
    Ok(format!("{noun} {id} pid {pid}"))
}

/// `<user>/services/<name>`, independent of any login session.
pub(super) fn create_service(
    roots: &Roots,
    uid: u32,
    name: &str,
) -> Result<std::path::PathBuf, String> {
    let sp = cgroupd_common::service_path(roots, uid, name);
    ensure_dir(&sp)?;
    delegate_dir(&sp, uid)?;
    Ok(sp)
}

/// `<sessions>/<sid>/session`: the login's leader and descendants.
pub(super) fn create_flat_session(
    roots: &Roots,
    cfg: &Config,
    uid: u32,
    sid: &str,
) -> Result<std::path::PathBuf, String> {
    let dir = session_dir(roots, sid);
    ensure_owned_dir(&dir, uid)?;
    enable_in(&dir, cfg);
    let sp = dir.join("session");
    ensure_dir(&sp)?;
    delegate_dir(&sp, uid)?;
    Ok(sp)
}

/// `<sessions>/<session of pid>/app/<aid>`: bound to the session `pid` lives in.
pub(super) fn create_flat_app(
    roots: &Roots,
    cfg: &Config,
    uid: u32,
    aid: &str,
    pid: u32,
) -> Result<std::path::PathBuf, String> {
    let sid = pid_session(pid).ok_or_else(|| format!("pid {pid} is not in a session cgroup"))?;
    flat_app_dir(roots, cfg, uid, &sid, aid)
}

/// Create (or reuse) a delegated `app/<aid>` under session `sid` of `uid`.
pub(crate) fn flat_app_dir(
    roots: &Roots,
    cfg: &Config,
    uid: u32,
    sid: &str,
    aid: &str,
) -> Result<std::path::PathBuf, String> {
    if !owns_session(roots, uid, sid) {
        return Err(format!("{sid} is not a session of uid {uid}"));
    }
    let want = new_app_path(roots, sid, aid);
    if let Some(have) = locate_app(roots, uid, aid)
        && have != want
    {
        return Err(format!("app {aid} already exists in another session"));
    }
    enable_in(&session_dir(roots, sid), cfg);
    let apps = session_app_dir(roots, sid);
    ensure_dir(&apps)?;
    chown_uid(&apps, uid)?;
    chmod_if_exists(&apps, 0o755);
    enable_in(&apps, cfg);
    ensure_dir(&want)?;
    delegate_dir(&want, uid)?;
    Ok(want)
}

//! The operations behind every request, shared by the Unix socket and D-Bus
//! front ends. Both authenticate their peer and build a `Request`; this
//! resolves the target uid, applies policy and runs it.

use cgroupd_common::{Request, Roots, varlink};

use crate::config::Config;

/// A failed call: Varlink error name + human message.
pub type CallError = (&'static str, String);

fn internal(e: String) -> CallError {
    (varlink::ERR_INTERNAL, e)
}

/// The uid a request names, if any (`None`: the caller).
pub fn request_target(req: &Request) -> Option<u32> {
    match req {
        Request::EnsureUser { uid }
        | Request::CreateSession { uid, .. }
        | Request::RemoveSession { uid, .. }
        | Request::ListSessions { uid }
        | Request::ListApps { uid }
        | Request::SpawnApp { uid, .. }
        | Request::StopSession { uid, .. }
        | Request::GetState { uid, .. }
        | Request::WaitSession { uid, .. } => *uid,
    }
}

fn pid_owner(pid: u32) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(format!("/proc/{pid}"))
        .ok()
        .map(|m| m.uid())
}

// Non-root callers may only migrate their own pids (else they could
// throttle other users' processes via limits); root peers (cgctl pam) bypass.
fn check_session_pid(peer: u32, want_uid: u32, pid: u32) -> Result<(), String> {
    if peer == 0 {
        return Ok(());
    }
    if pid == 0 {
        return Err("pid 0 invalid".to_string());
    }
    match pid_owner(pid) {
        Some(owner) if owner == want_uid => Ok(()),
        Some(owner) => Err(format!(
            "pid {pid} owned by uid {owner}, not {want_uid} (refusing cross-user move)"
        )),
        None => Err(format!("no such pid {pid}")),
    }
}

pub fn execute(roots: &Roots, cfg: &Config, req: &Request, peer: u32) -> Result<String, CallError> {
    // Single choke point: the wire `uid` is never trusted. Non-root callers
    // resolve to their peer UID; root must name an explicit target.
    // Auth failures are PermissionDenied; op-layer failures (validation and
    // operational alike, details in the message) are InternalError.
    let uid = match cgroupd_common::resolve_uid(request_target(req), peer, cfg.allow_root) {
        Ok(u) => u,
        Err(e) => {
            crate::log::warning(&format!("denied peer {peer}: {e}"));
            return Err((varlink::ERR_PERMISSION_DENIED, e));
        }
    };
    match req {
        Request::EnsureUser { .. } => crate::fs::ensure_user(roots, uid, cfg.allow_root)
            .map(|()| format!("user {uid} ensured"))
            .map_err(internal),
        Request::CreateSession {
            sid, pid, scope, ..
        } => {
            if let Err(e) = check_session_pid(peer, uid, *pid) {
                crate::log::warning(&format!(
                    "denied create-session pid {pid} for uid {uid}: {e}"
                ));
                return Err((varlink::ERR_PERMISSION_DENIED, e));
            }
            crate::fs::create_session(roots, cfg, uid, *scope, sid, *pid).map_err(internal)
        }
        Request::RemoveSession { sid, scope, .. } => {
            let d = crate::fs::remove_target(roots, cfg, uid, *scope, sid).map_err(internal)?;
            if d.contains("removed") || d.contains("already gone") {
                crate::spawn::drop_state(uid, *scope, sid);
            }
            Ok(d)
        }
        Request::ListSessions { .. } => crate::fs::list_sessions(roots, cfg, uid).map_err(internal),
        Request::ListApps { .. } => crate::fs::list_apps(roots, cfg, uid).map_err(internal),
        Request::SpawnApp {
            sid: spawn_sid,
            aid,
            program,
            args,
            env,
            stdout,
            stderr,
            cwd,
            umask,
            limits,
            pre,
            post,
            ..
        } => {
            let params = crate::spawn::SpawnParams {
                uid,
                sid: spawn_sid.as_deref(),
                aid,
                program,
                args,
                env,
                stdout,
                stderr,
                cwd,
                umask: *umask,
                limits,
                pre,
                post,
            };
            crate::spawn::spawn_app(roots, cfg, &params).map_err(internal)
        }
        Request::StopSession {
            sid,
            scope,
            grace_secs,
            ..
        } => crate::spawn::stop_target(roots, cfg, uid, *scope, sid, *grace_secs).map_err(internal),
        Request::GetState { sid, scope, .. } => {
            crate::spawn::get_state(roots, cfg, uid, *scope, sid).map_err(internal)
        }
        Request::WaitSession {
            sid,
            scope,
            timeout_secs,
            ..
        } => crate::spawn::wait_session(roots, cfg, uid, *scope, sid, *timeout_secs)
            .map_err(internal),
    }
}

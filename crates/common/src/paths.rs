use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// Where cgroups live. libelogind/polkit take the *first path component* of
/// `/proc/<pid>/cgroup` as the login session id, so a session is a top-level
/// cgroup named exactly like its id:
///
/// ```text
/// <sessions>/<sid>/            owned by uid, no processes
///     session/                 login leader and its descendants
///     app/<aid>/               apps launched from this session
/// <sessions>/user@<uid>/       the user's own cgroup (not a session: the `@`)
///     services/<name>/         per-user services, independent of logins
/// ```
///
/// `FAKE_CGROUP_ROOT` prefixes the root in tests.
#[derive(Debug, Clone)]
pub struct Roots {
    pub sessions: PathBuf,
}

/// The cgroup2 mountpoint sessions are created under.
pub const SESSION_ROOT: &str = "/sys/fs/cgroup";

pub fn resolve_roots() -> Roots {
    let fake = std::env::var("FAKE_CGROUP_ROOT")
        .ok()
        .filter(|s| !s.is_empty());
    resolve_roots_with(fake.as_deref(), SESSION_ROOT)
}

/// Testable core of `resolve_roots` (avoids env mutation in tests).
pub fn resolve_roots_with(fake: Option<&str>, sessions: &str) -> Roots {
    match fake {
        Some(f) if !f.is_empty() => Roots {
            sessions: PathBuf::from(f).join(strip_leading_slash(sessions)),
        },
        _ => Roots {
            sessions: PathBuf::from(sessions),
        },
    }
}

fn strip_leading_slash(p: &str) -> &str {
    p.trim_start_matches('/')
}

/// The user's own cgroup, outside any session: `<sessions>/user@<uid>` (the `@` keeps
/// libelogind from reading it as a session id). Caller validates `uid`.
pub fn user_path(roots: &Roots, uid: u32) -> PathBuf {
    roots.sessions.join(format!("user@{uid}"))
}

/// `user@<uid>/services/<name>`: a per-user service, independent of login sessions.
pub fn service_path(roots: &Roots, uid: u32, name: &str) -> PathBuf {
    user_path(roots, uid).join("services").join(name)
}

/// The top-level `<sessions>/<sid>` container. Caller validates `sid`.
pub fn session_dir(roots: &Roots, sid: &str) -> PathBuf {
    roots.sessions.join(sid)
}

/// The unit cgroup holding a session's processes: `<sessions>/<sid>/session`.
pub fn session_path(roots: &Roots, sid: &str) -> PathBuf {
    session_dir(roots, sid).join("session")
}

/// `<sessions>/<sid>/app`.
pub fn session_app_dir(roots: &Roots, sid: &str) -> PathBuf {
    session_dir(roots, sid).join("app")
}

/// Sids of the sessions owned by `uid` (any uid when `None`). A session
/// container is a directory owned by the user that has a `session` child; this
/// skips unrelated top-level cgroups (services, init's, ...).
pub fn flat_sessions(roots: &Roots, uid: Option<u32>) -> Vec<String> {
    flat_dirs(roots, uid, false)
}

/// Like `flat_sessions`, but also session containers whose login has ended while
/// apps launched from it still run (`<sid>/app` without `<sid>/session`).
pub fn flat_containers(roots: &Roots, uid: Option<u32>) -> Vec<String> {
    flat_dirs(roots, uid, true)
}

fn flat_dirs(roots: &Roots, uid: Option<u32>, with_orphans: bool) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(&roots.sessions) else {
        return out;
    };
    for e in rd.flatten() {
        let Ok(meta) = e.metadata() else { continue };
        let p = e.path();
        if !meta.is_dir() || !(p.join("session").is_dir() || with_orphans && p.join("app").is_dir())
        {
            continue;
        }
        if uid.is_some_and(|u| meta.uid() != u) {
            continue;
        }
        if let Some(n) = e.file_name().to_str() {
            out.push(n.to_string());
        }
    }
    out.sort();
    out
}

/// The existing `app/<aid>` cgroup of `uid`, in whichever of its sessions it
/// was launched from.
pub fn locate_app(roots: &Roots, uid: u32, aid: &str) -> Option<PathBuf> {
    flat_containers(roots, Some(uid))
        .into_iter()
        .map(|sid| session_app_dir(roots, &sid).join(aid))
        .find(|p| p.is_dir())
}

/// Session id of a process: the first component of its cgroup path (after
/// init's own cgroup path), i.e. what libelogind reports.
pub fn pid_session(pid: u32) -> Option<String> {
    let cg = |pid: &str| -> Option<String> {
        let t = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
        t.lines()
            .find_map(|l| l.strip_prefix("0::"))
            .map(str::to_string)
    };
    let path = cg(&pid.to_string())?;
    let root = cg("1").unwrap_or_default();
    let rel = path
        .strip_prefix(root.trim_end_matches('/'))
        .unwrap_or(&path);
    let first = rel.trim_start_matches('/').split('/').next()?;
    (!first.is_empty()).then(|| first.to_string())
}

/// Where `uid`'s app `aid` lives: wherever it exists (it was created in the
/// session that launched it); if it does not exist, a path that never exists.
/// Caller validates `aid`.
pub fn app_unit_path(roots: &Roots, uid: u32, aid: &str) -> PathBuf {
    locate_app(roots, uid, aid).unwrap_or_else(|| {
        roots
            .sessions
            .join(".no-such-session")
            .join("app")
            .join(aid)
    })
}

/// Where a *new* app `aid` goes: `<sessions>/<sid>/app/<aid>`.
pub fn new_app_path(roots: &Roots, sid: &str, aid: &str) -> PathBuf {
    session_app_dir(roots, sid).join(aid)
}

/// True when `<sessions>/<sid>` is a directory owned by `uid`.
pub fn owns_session(roots: &Roots, uid: u32, sid: &str) -> bool {
    std::fs::metadata(Path::new(&session_dir(roots, sid)))
        .is_ok_and(|m| m.is_dir() && m.uid() == uid)
}

/// Full unit path for a scope id.
pub fn unit_path(roots: &Roots, uid: u32, scope: crate::proto::Scope, id: &str) -> PathBuf {
    match scope {
        crate::proto::Scope::Session => session_path(roots, id),
        crate::proto::Scope::App => app_unit_path(roots, uid, id),
        crate::proto::Scope::Service => service_path(roots, uid, id),
    }
}

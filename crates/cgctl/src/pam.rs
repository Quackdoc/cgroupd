use cgroupd_common::{Request, Scope};

use crate::TIMEOUT;

pub(crate) fn pam_warn(msg: &str) {
    let flat: String = msg
        .chars()
        .map(|c| if c == '\n' { ' ' } else { c })
        .collect();
    let line = format!("<84>cgctl-pam[{}]: {flat}", std::process::id());
    if let Ok(sock) = std::os::unix::net::UnixDatagram::unbound() {
        let _ = sock.send_to(line.as_bytes(), "/dev/log");
    }
}

/// pam_exec hook. Fail-open: always exits 0 so a dead daemon never blocks
/// logins. The session leader is our parent (pam_exec forks us).
pub(crate) fn run_pam(socket: &str) -> ! {
    pam_session(socket);
    std::process::exit(0);
}

/// Session id for the login led by `ppid`. It must equal the id login1 reports (polkit and
/// libelogind take it from the cgroup name), so prefer the id the session tracker already
/// assigned: `$XDG_SESSION_ID`, then turnstile's `/run/turnstiled/sessions/<id>` whose
/// `LEADER` is this login, else `pam-<ppid>`. On close the leader is still in its session
/// cgroup, which names the session exactly.
fn pam_session_id(uid: u32, ppid: u32, opening: bool) -> String {
    let valid = |v: &str| cgroupd_common::validate_sid(v).is_ok();
    if !opening
        && let Some(sid) = cgroupd_common::pid_session(ppid)
        && valid(&sid)
        && std::fs::metadata(format!("/sys/fs/cgroup/{sid}")).is_ok_and(|m| {
            use std::os::unix::fs::MetadataExt;
            m.uid() == uid
        })
    {
        return sid;
    }
    if let Ok(v) = std::env::var("XDG_SESSION_ID")
        && valid(&v)
    {
        return v;
    }
    // pam_turnstile ran earlier in the stack; its session file may lag slightly.
    for _ in 0..(if opening { 10 } else { 1 }) {
        if let Some(sid) = turnstile_session(ppid) {
            return sid;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    format!("pam-{ppid}")
}

fn turnstile_session(leader: u32) -> Option<String> {
    let rd = std::fs::read_dir("/run/turnstiled/sessions").ok()?;
    for e in rd.flatten() {
        let Ok(text) = std::fs::read_to_string(e.path()) else {
            continue;
        };
        let found = text.lines().any(|l| {
            l.strip_prefix("LEADER=")
                .is_some_and(|v| v.trim() == leader.to_string())
        });
        let id = e.file_name().to_string_lossy().into_owned();
        if found && cgroupd_common::validate_sid(&id).is_ok() {
            return Some(id);
        }
    }
    None
}

pub(crate) fn pam_session(socket: &str) {
    let ty = std::env::var("PAM_TYPE").unwrap_or_default();
    if ty != "open_session" && ty != "close_session" {
        return;
    }
    let Ok(user) = std::env::var("PAM_USER") else {
        pam_warn("PAM_USER unset, fail-open");
        return;
    };
    let Some(uid) = uzers::get_user_by_name(&user).map(|u| u.uid()) else {
        pam_warn(&format!("unknown user {user}, fail-open"));
        return;
    };
    if uid == 0 {
        return;
    }
    let ppid = std::os::unix::process::parent_id();
    let opening = ty == "open_session";
    let sid = pam_session_id(uid, ppid, opening);
    let call = |req: &Request| match crate::request::call(socket, req, TIMEOUT) {
        Ok(_) => true,
        Err(e) => {
            pam_warn(&format!("{e}, fail-open"));
            false
        }
    };
    if ty == "open_session" {
        if call(&Request::EnsureUser { uid: Some(uid) }) {
            call(&Request::CreateSession {
                uid: Some(uid),
                scope: Scope::Session,
                sid,
                pid: ppid,
            });
        }
    } else {
        call(&Request::RemoveSession {
            uid: Some(uid),
            scope: Scope::Session,
            sid,
        });
    }
}

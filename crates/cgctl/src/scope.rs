use std::os::unix::process::CommandExt as _;
use std::process::Command;

use cgroupd_common::{Request, Scope};

use crate::TIMEOUT;

/// Default app id mirroring cgrun.py: `app-<basename>-<pid>`.
pub(crate) fn default_aid(program: &str, pid: u32) -> String {
    let base = program.rsplit('/').next().unwrap_or(program);
    let clean: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect();
    let base = if clean.is_empty() {
        "app".to_string()
    } else {
        clean
    };
    format!("app-{base}-{pid}").chars().take(64).collect()
}

fn die(code: i32, msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(code);
}

/// Register `pid` as an app cgroup member; returns the daemon's detail.
fn scope_register(
    socket: &str,
    as_uid: Option<u32>,
    aid: &str,
    pid: u32,
) -> Result<String, String> {
    let call = |req: &Request| crate::request::call(socket, req, TIMEOUT);
    call(&Request::EnsureUser { uid: as_uid })?;
    call(&Request::CreateSession {
        uid: as_uid,
        scope: Scope::App,
        sid: aid.to_string(),
        pid,
    })
}

/// systemd-run --scope equivalent: move this process into a fresh app cgroup,
/// then exec the program in place (same pid, no fork, signals and exit status
/// go straight to the program).
pub(crate) fn run_scope(
    socket: &str,
    as_uid: Option<u32>,
    aid: Option<String>,
    program: &[String],
) -> ! {
    let Some((prog, args)) = program.split_first() else {
        die(2, "scope needs a program after `--`");
    };
    let me = std::process::id();
    let aid = aid.unwrap_or_else(|| default_aid(prog, me));
    if let Err(e) = cgroupd_common::validate_sid(&aid) {
        die(2, &format!("invalid aid: {e}"));
    }
    match scope_register(socket, as_uid, &aid, me) {
        Ok(detail) => eprintln!("{detail}"),
        Err(e) => die(1, &e),
    }
    let err = Command::new(prog).args(args).exec();
    die(127, &format!("exec {prog}: {err}"));
}

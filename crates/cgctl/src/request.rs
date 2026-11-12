use std::time::Duration;

use cgroupd_common::{Limit, Request, Response, Scope, StdioSpec};

use crate::TIMEOUT;

pub(crate) fn parse_umask(s: &str) -> u32 {
    u32::from_str_radix(s.trim_start_matches("0o"), 8).unwrap_or_else(|_| {
        eprintln!("invalid umask (octal): {s}");
        std::process::exit(2);
    })
}

pub(crate) fn parse_stdio(s: &str) -> StdioSpec {
    cgroupd_common::parse_stdio_spec(s).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(2);
    })
}

/// Whitespace word split for --pre/--post (no quoting).
pub(crate) fn split_words(s: &str) -> Vec<String> {
    s.split_whitespace().map(str::to_string).collect()
}

/// Pull `"pids":[...]` out of a GetState detail.
pub(crate) fn extract_pids(detail: &str) -> Vec<u32> {
    let Some(start) = detail.find("\"pids\"") else {
        return Vec::new();
    };
    let rest = &detail[start..];
    let Some(lb) = rest.find('[') else {
        return Vec::new();
    };
    let rest = &rest[lb + 1..];
    let end = rest.find(']').unwrap_or(rest.len());
    rest[..end]
        .split(|c: char| !(c.is_ascii_digit()))
        .filter(|t| !t.is_empty())
        .filter_map(|t| t.parse().ok())
        .collect()
}

/// One daemon call; a daemon-side error and a transport error both become `Err`.
pub(crate) fn call(socket: &str, req: &Request, timeout: Duration) -> Result<String, String> {
    match cgroupd_common::send(socket, req, timeout) {
        Ok(Response::Ok { detail }) => Ok(detail),
        Ok(Response::Err { msg }) => Err(format!("daemon error: {msg}")),
        Err(e) => Err(format!("error: {e}")),
    }
}

fn or_exit(r: Result<String, String>) -> String {
    r.unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    })
}

pub(crate) fn run_request(socket: &str, req: &Request) {
    run_request_to(socket, req, TIMEOUT);
}

pub(crate) fn run_request_to(socket: &str, req: &Request, timeout: Duration) {
    println!("{}", or_exit(call(socket, req, timeout)));
}

/// Fetch a `GetState` detail string (for `ps`), exiting on daemon/transport error.
pub(crate) fn fetch_state(socket: &str, uid: Option<u32>, scope: Scope, sid: String) -> String {
    or_exit(call(
        socket,
        &Request::GetState { uid, scope, sid },
        TIMEOUT,
    ))
}

pub(crate) fn scope_of3(app: bool, service: bool) -> Scope {
    if service {
        Scope::Service
    } else {
        scope_of(app)
    }
}

pub(crate) fn scope_of(app: bool) -> Scope {
    if app { Scope::App } else { Scope::Session }
}

pub(crate) fn parse_limits(raw: &[String]) -> Vec<Limit> {
    raw.iter()
        .map(|s| {
            let Some(eq) = s.find('=') else {
                eprintln!("--limit needs FILE=VALUE");
                std::process::exit(2);
            };
            Limit {
                file: s[..eq].to_string(),
                value: s[eq + 1..].to_string(),
            }
        })
        .collect()
}

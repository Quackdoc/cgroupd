#![forbid(unsafe_code)]

mod cli;
mod enter;
mod pam;
mod request;
mod scope;

use std::time::Duration;

use cgroupd_common::{DEFAULT_SOCKET_PATH, Request, StdioSpec};
use cli::{Commands, Ctl};
use request::*;

const TIMEOUT: Duration = Duration::from_secs(2);

fn main() {
    let ctl = Ctl::parse();
    let socket = ctl.socket.as_deref().unwrap_or(DEFAULT_SOCKET_PATH);

    match ctl.command {
        Commands::EnsureUser(a) => {
            run_request(socket, &Request::EnsureUser { uid: a.as_uid });
        }
        Commands::CreateSession(a) => {
            let pid = a.pid.unwrap_or_else(std::process::id);
            if let Err(e) = cgroupd_common::validate_sid(&a.sid) {
                eprintln!("invalid sid: {e}");
                std::process::exit(2);
            }
            run_request(
                socket,
                &Request::CreateSession {
                    uid: a.as_uid,
                    scope: scope_of3(a.app, a.service),
                    sid: a.sid,
                    pid,
                },
            );
        }
        Commands::RemoveSession(a) => {
            run_request(
                socket,
                &Request::RemoveSession {
                    uid: a.as_uid,
                    scope: scope_of3(a.app, a.service),
                    sid: a.sid,
                },
            );
        }
        Commands::ListSessions(a) => {
            run_request(socket, &Request::ListSessions { uid: a.as_uid });
        }
        Commands::ListApps(a) => {
            run_request(socket, &Request::ListApps { uid: a.as_uid });
        }
        Commands::Spawn(a) => {
            if let Err(e) = cgroupd_common::validate_sid(&a.aid) {
                eprintln!("invalid aid: {e}");
                std::process::exit(2);
            }
            let req = Request::SpawnApp {
                uid: a.as_uid,
                sid: a
                    .session
                    .or_else(|| cgroupd_common::pid_session(std::process::id())),
                aid: a.aid,
                program: a.program[0].clone(),
                args: a.program[1..].to_vec(),
                env: a.env,
                stdout: a
                    .stdout
                    .as_deref()
                    .map(parse_stdio)
                    .unwrap_or(StdioSpec::Null),
                stderr: a
                    .stderr
                    .as_deref()
                    .map(parse_stdio)
                    .unwrap_or(StdioSpec::Null),
                cwd: a.cwd,
                umask: a.umask.as_deref().map(parse_umask),
                limits: parse_limits(&a.limit),
                pre: a.pre.iter().map(|s| split_words(s)).collect(),
                post: a.post.iter().map(|s| split_words(s)).collect(),
            };
            run_request(socket, &req);
        }
        Commands::Stop(a) => {
            run_request(
                socket,
                &Request::StopSession {
                    uid: a.as_uid,
                    scope: scope_of(a.app),
                    sid: a.sid,
                    grace_secs: a.grace,
                },
            );
        }
        Commands::State(a) => {
            run_request(
                socket,
                &Request::GetState {
                    uid: a.as_uid,
                    scope: scope_of(a.app),
                    sid: a.sid,
                },
            );
        }
        Commands::Wait(a) => {
            // Wait can legitimately take a while; give the RPC headroom.
            let wait_timeout = std::time::Duration::from_secs(a.timeout.saturating_add(10).max(5));
            run_request_to(
                socket,
                &Request::WaitSession {
                    uid: a.as_uid,
                    scope: scope_of(a.app),
                    sid: a.sid,
                    timeout_secs: a.timeout,
                },
                wait_timeout,
            );
        }
        Commands::Ps(a) => {
            let detail = fetch_state(socket, a.as_uid, scope_of(a.app), a.sid);
            let pids = extract_pids(&detail);
            if pids.is_empty() {
                println!("(empty)");
            } else {
                for pid in pids {
                    println!("{pid}");
                }
            }
        }
        Commands::Enter(a) => enter::run_enter(&a.path),
        Commands::Scope(a) => {
            scope::run_scope(socket, a.as_uid, a.aid, &a.program);
        }
        Commands::Pam => {
            pam::run_pam(socket);
        }
    }
}

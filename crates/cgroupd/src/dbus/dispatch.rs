//! Routes a method call to its operation.
//!
//! Methods come in four argument shapes, each handled by one helper that
//! parses the arguments, resolves the caller and wraps the reply:
//! `uid_only` (none), `with_id` (`s`), `with_id_num` (`s,u`), plus `SpawnApp`
//! (`s,s`). `Wait*` answers later from a thread, so it returns no reply here.

use std::sync::mpsc::Sender;

use cgroupd_common::{Request, Roots, Scope, resolve_uid, varlink};

use super::ops::spawn_request;
use super::wire::{
    self, bus_peer, dispatch_response, err_detail, ok_detail, reply_err, split_sid, split_sid_str,
    split_sid_u32,
};
use crate::api;
use crate::config::Config;

/// Everything a handler needs for one call.
struct Call<'a> {
    conn: &'a dbus::ffidisp::Connection,
    msg: &'a dbus::Message,
    roots: &'a Roots,
    cfg: &'a Config,
    wait_tx: &'a Sender<dbus::Message>,
    member: &'a str,
}

type Reply = Option<dbus::Message>;

impl Call<'_> {
    fn bad_args(&self, shape: &str) -> Reply {
        Some(reply_err(
            self.msg,
            wire::ERROR_INVALID,
            format!("{} needs {shape}", self.member),
        ))
    }

    /// Run `req` as the bus-authenticated caller. A refused caller is a bus
    /// error; any other failure is reported in the reply detail, like the
    /// socket API. `Wait*` runs on its own thread so it never blocks the bus
    /// loop; its reply goes out later through `wait_tx`.
    fn exec(&self, mut req: Request) -> Reply {
        let peer = match bus_peer(self.conn, self.msg) {
            Ok(p) => p,
            Err(e) => return Some(reply_err(self.msg, wire::ERROR_DENIED, e)),
        };
        if let Request::WaitSession { uid, .. } = &mut req {
            // Resolve now so a refusal is a bus error, not a late detail.
            match resolve_uid(*uid, peer, self.cfg.allow_root) {
                Ok(u) => *uid = Some(u),
                Err(e) => return Some(reply_err(self.msg, wire::ERROR_DENIED, e)),
            }
            let (roots, cfg) = (self.roots.clone(), self.cfg.clone());
            let tx = self.wait_tx.clone();
            let shell = self.msg.method_return();
            std::thread::spawn(move || {
                let detail = match api::execute(&roots, &cfg, &req, peer) {
                    Ok(d) => ok_detail(d),
                    Err((_, e)) => err_detail(e),
                };
                let _ = tx.send(shell.append1(detail));
            });
            return None;
        }
        Some(match api::execute(self.roots, self.cfg, &req, peer) {
            Err((varlink::ERR_PERMISSION_DENIED, e)) => reply_err(self.msg, wire::ERROR_DENIED, e),
            Err((_, e)) => dispatch_response(self.msg, Err(e)),
            Ok(d) => dispatch_response(self.msg, Ok(d)),
        })
    }

    /// `()` or `(u)`.
    fn uid_only(&self, f: impl FnOnce(Option<u32>) -> Request) -> Reply {
        self.exec(f(self.msg.get1::<u32>()))
    }

    /// `(s)` or `(u,s)`.
    fn with_id(&self, f: impl FnOnce(Option<u32>, String) -> Request) -> Reply {
        match split_sid(self.msg) {
            Some((uid, id)) => self.exec(f(uid, id)),
            None => self.bad_args("(s) or (u,s)"),
        }
    }

    /// `(s,u)` or `(u,s,u)`.
    fn with_id_num(&self, f: impl FnOnce(Option<u32>, String, u64) -> Request) -> Reply {
        match split_sid_u32(self.msg) {
            Some((uid, id, n)) => self.exec(f(uid, id, n as u64)),
            None => self.bad_args("(s,u) or (u,s,u)"),
        }
    }

    fn route(&self) -> Reply {
        use Request as R;
        use Scope::{App, Session};
        let create = |scope| {
            move |uid, sid, pid: u64| R::CreateSession {
                uid,
                scope,
                sid,
                pid: pid as u32,
            }
        };
        let stop = |scope| {
            move |uid, sid, grace_secs| R::StopSession {
                uid,
                scope,
                sid,
                grace_secs,
            }
        };
        let wait = |scope| {
            move |uid, sid, timeout_secs| R::WaitSession {
                uid,
                scope,
                sid,
                timeout_secs,
            }
        };
        let remove = |scope| move |uid, sid| R::RemoveSession { uid, scope, sid };
        let state = |scope| move |uid, sid| R::GetState { uid, scope, sid };
        match self.member {
            "EnsureUser" => self.uid_only(|uid| R::EnsureUser { uid }),
            "ListSessions" => self.uid_only(|uid| R::ListSessions { uid }),
            "ListApps" => self.uid_only(|uid| R::ListApps { uid }),
            "CreateSession" => self.with_id_num(create(Session)),
            "CreateApp" => self.with_id_num(create(App)),
            "StopSession" => self.with_id_num(stop(Session)),
            "StopApp" => self.with_id_num(stop(App)),
            "WaitSession" => self.with_id_num(wait(Session)),
            "WaitApp" => self.with_id_num(wait(App)),
            "RemoveSession" => self.with_id(remove(Session)),
            "RemoveApp" => self.with_id(remove(App)),
            "GetState" => self.with_id(state(Session)),
            "GetAppState" => self.with_id(state(App)),
            "SpawnApp" => {
                let Some((uid, aid, spec)) = split_sid_str(self.msg) else {
                    return self.bad_args("(s,s) or (u,s,s)");
                };
                match spawn_request(uid, aid, &spec) {
                    Ok(req) => self.exec(req),
                    Err(e) => Some(dispatch_response(self.msg, Err(e))),
                }
            }
            other => self.msg.path().is_some().then(|| {
                reply_err(
                    self.msg,
                    wire::ERROR_NOT_FOUND,
                    format!("unknown method {other}"),
                )
            }),
        }
    }
}

/// Handle one incoming message; `None` means no immediate reply (not for us,
/// or a `Wait*` that answers later).
pub(super) fn handle_message(
    conn: &dbus::ffidisp::Connection,
    msg: &dbus::Message,
    roots: &Roots,
    cfg: &Config,
    wait_tx: &Sender<dbus::Message>,
) -> Reply {
    if msg.msg_type() != dbus::MessageType::MethodCall {
        return None;
    }
    if msg.path().is_none_or(|p| &*p != wire::OBJECT_PATH) {
        return None;
    }
    let iface = msg.interface().map(|i| i.to_string()).unwrap_or_default();
    let member = msg.member().map(|m| m.to_string()).unwrap_or_default();

    if iface == wire::INTROSPECTABLE && member == "Introspect" {
        return Some(
            msg.method_return()
                .append1(wire::INTROSPECT_DATA.to_string()),
        );
    }
    if iface != wire::INTERFACE {
        return None;
    }
    Call {
        conn,
        msg,
        roots,
        cfg,
        wait_tx,
        member: &member,
    }
    .route()
}

//! What goes over the bus: names, introspection XML, argument shapes, caller
//! identity and reply building. Every method takes an optional leading `u`
//! (target uid): the long shape wins, the short one acts on the caller.

use cgroupd_common::Response;

pub const SERVICE: &str = "org.cgroupd";
pub const OBJECT_PATH: &str = "/org/cgroupd/Manager";
pub const INTERFACE: &str = "org.cgroupd.Manager";
pub const INTROSPECTABLE: &str = "org.freedesktop.DBus.Introspectable";
pub const ERROR_DENIED: &str = "org.cgroupd.Error.Denied";
pub const ERROR_INVALID: &str = "org.cgroupd.Error.Invalid";
pub const ERROR_NOT_FOUND: &str = "org.cgroupd.Error.NotFound";

pub const INTROSPECT_DATA: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE node PUBLIC "-//freedesktop//DTD D-BUS Object Introspection 1.0//EN" "http://www.freedesktop.org/standards/dbus/1.0/introspect.dtd">
<node name="/org/cgroupd/Manager">
  <interface name="org.cgroupd.Manager">
    <annotation name="org.cgroupd.uid-first" value="every method accepts an optional leading u (target uid, root callers only); the short form acts on the caller"/>
    <method name="EnsureUser"><arg name="ret" type="s" direction="out"/></method>
    <method name="CreateSession"><arg name="sid" type="s" direction="in"/><arg name="pid" type="u" direction="in"/><arg name="ret" type="s" direction="out"/></method>
    <method name="CreateApp"><arg name="aid" type="s" direction="in"/><arg name="pid" type="u" direction="in"/><arg name="ret" type="s" direction="out"/></method>
    <method name="RemoveSession"><arg name="sid" type="s" direction="in"/><arg name="ret" type="s" direction="out"/></method>
    <method name="RemoveApp"><arg name="aid" type="s" direction="in"/><arg name="ret" type="s" direction="out"/></method>
    <method name="ListSessions"><arg name="ret" type="s" direction="out"/></method>
    <method name="ListApps"><arg name="ret" type="s" direction="out"/></method>
    <method name="SpawnApp"><arg name="aid" type="s" direction="in"/><arg name="spec" type="s" direction="in"/><arg name="ret" type="s" direction="out"/></method>
    <method name="StopSession"><arg name="sid" type="s" direction="in"/><arg name="grace_secs" type="u" direction="in"/><arg name="ret" type="s" direction="out"/></method>
    <method name="StopApp"><arg name="aid" type="s" direction="in"/><arg name="grace_secs" type="u" direction="in"/><arg name="ret" type="s" direction="out"/></method>
    <method name="GetState"><arg name="sid" type="s" direction="in"/><arg name="ret" type="s" direction="out"/></method>
    <method name="GetAppState"><arg name="aid" type="s" direction="in"/><arg name="ret" type="s" direction="out"/></method>
    <method name="WaitSession"><arg name="sid" type="s" direction="in"/><arg name="timeout_secs" type="u" direction="in"/><arg name="ret" type="s" direction="out"/></method>
    <method name="WaitApp"><arg name="aid" type="s" direction="in"/><arg name="timeout_secs" type="u" direction="in"/><arg name="ret" type="s" direction="out"/></method>
  </interface>
</node>"#;

pub(super) fn ok_detail(detail: String) -> String {
    serde_json::to_string(&Response::ok(detail)).unwrap_or_else(|_| "{}".into())
}

pub(super) fn err_detail(msg: String) -> String {
    serde_json::to_string(&Response::err(msg)).unwrap_or_else(|_| "{}".into())
}

fn reply_ok(msg: &dbus::Message, detail: String) -> dbus::Message {
    msg.method_return().append1(detail)
}

pub(super) fn reply_err(msg: &dbus::Message, name: &str, err: String) -> dbus::Message {
    let err = std::ffi::CString::new(err)
        .unwrap_or_else(|_| std::ffi::CString::new("error").expect("static fallback has no NUL"));
    // `dbus::strings::ErrorName` implements `From<&str>`; fall back to a
    // generic name if the string is somehow invalid.
    let owned = name.to_string();
    let ename = dbus::strings::ErrorName::from(&owned as &str);
    msg.error(&ename, &err)
}

pub(super) fn dispatch_response(msg: &dbus::Message, r: Result<String, String>) -> dbus::Message {
    match r {
        Ok(d) => reply_ok(msg, ok_detail(d)),
        Err(e) => reply_ok(msg, err_detail(e)),
    }
}

/// Kernel-authenticated caller UID for this method call, via the bus daemon.
/// The sender name (`:1.N`) is bus-assigned and unspoofable.
pub(super) fn bus_peer(
    conn: &dbus::ffidisp::Connection,
    msg: &dbus::Message,
) -> Result<u32, String> {
    let sender = msg
        .sender()
        .map(|s| s.to_string())
        .ok_or_else(|| "no sender on method call".to_string())?;
    let proxy = conn.with_path("org.freedesktop.DBus", "/", 5000);
    let (uid,): (u32,) = proxy
        .method_call("org.freedesktop.DBus", "GetConnectionUnixUser", (sender,))
        .map_err(|e| format!("GetConnectionUnixUser: {e}"))?;
    Ok(uid)
}

/// Split `(s) | (u,s)`: the long shape wins, the short shape acts on the caller.
pub(super) fn split_sid(msg: &dbus::Message) -> Option<(Option<u32>, String)> {
    match msg.get2::<u32, String>() {
        (Some(t), Some(s)) => Some((Some(t), s)),
        _ => msg.get1::<String>().map(|s| (None, s)),
    }
}

/// Split `(s,u) | (u,s,u)`.
pub(super) fn split_sid_u32(msg: &dbus::Message) -> Option<(Option<u32>, String, u32)> {
    match msg.get3::<u32, String, u32>() {
        (Some(t), Some(s), Some(n)) => Some((Some(t), s, n)),
        _ => match msg.get2::<String, u32>() {
            (Some(s), Some(n)) => Some((None, s, n)),
            _ => None,
        },
    }
}

/// Split `(s,s) | (u,s,s)`.
pub(super) fn split_sid_str(msg: &dbus::Message) -> Option<(Option<u32>, String, String)> {
    match msg.get3::<u32, String, String>() {
        (Some(t), Some(a), Some(b)) => Some((Some(t), a, b)),
        _ => match msg.get2::<String, String>() {
            (Some(a), Some(b)) => Some((None, a, b)),
            _ => None,
        },
    }
}

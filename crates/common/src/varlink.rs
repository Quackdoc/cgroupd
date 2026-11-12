//! Hand-rolled Varlink wire format for the daemon socket.
//!
//! Calls look like `{"method": "org.cgroupd.Manager.EnsureUser",
//! "parameters": {...}}`, replies like `{"parameters": {"detail": ...}}`,
//! errors like `{"error": "org.varlink.service.PermissionDenied",
//! "parameters": {"message": ...}}`. Messages are `NUL`-delimited (see
//! `framing`). No `oneway`/`more`/`upgrade` semantics beyond honoring
//! `oneway` (reply suppressed); a `more` call gets a single reply, which
//! the spec allows.
//!
//! Auth stays outside the format: the daemon resolves every caller's UID
//! from `SO_PEERCRED`, exactly as before.

use crate::proto::Request;

pub const INTERFACE: &str = "org.cgroupd.Manager";
pub const SERVICE_INTERFACE: &str = "org.varlink.service";

pub const ERR_PERMISSION_DENIED: &str = "org.varlink.service.PermissionDenied";
pub const ERR_INVALID_PARAMETER: &str = "org.varlink.service.InvalidParameter";
pub const ERR_METHOD_NOT_FOUND: &str = "org.varlink.service.MethodNotFound";
pub const ERR_INTERFACE_NOT_FOUND: &str = "org.varlink.service.InterfaceNotFound";
pub const ERR_INTERNAL: &str = "org.cgroupd.Manager.InternalError";

/// Fully-qualified method name for a request.
pub fn method_name(req: &Request) -> &'static str {
    match req {
        Request::EnsureUser { .. } => "org.cgroupd.Manager.EnsureUser",
        Request::CreateSession { .. } => "org.cgroupd.Manager.CreateSession",
        Request::RemoveSession { .. } => "org.cgroupd.Manager.RemoveSession",
        Request::ListSessions { .. } => "org.cgroupd.Manager.ListSessions",
        Request::ListApps { .. } => "org.cgroupd.Manager.ListApps",
        Request::SpawnApp { .. } => "org.cgroupd.Manager.SpawnApp",
        Request::StopSession { .. } => "org.cgroupd.Manager.StopSession",
        Request::GetState { .. } => "org.cgroupd.Manager.GetState",
        Request::WaitSession { .. } => "org.cgroupd.Manager.WaitSession",
    }
}

/// Whether `method` names one of our calls (vs an unknown method on our
/// interface, which is `MethodNotFound` rather than `InvalidParameter`).
pub fn method_known(method: &str) -> bool {
    op_for_method(method).is_some()
}

/// The `op` tag our `Request` serde form uses, per method suffix.
fn op_for_method(method: &str) -> Option<&'static str> {
    match method {
        "org.cgroupd.Manager.EnsureUser" => Some("ensure_user"),
        "org.cgroupd.Manager.CreateSession" => Some("create_session"),
        "org.cgroupd.Manager.RemoveSession" => Some("remove_session"),
        "org.cgroupd.Manager.ListSessions" => Some("list_sessions"),
        "org.cgroupd.Manager.ListApps" => Some("list_apps"),
        "org.cgroupd.Manager.SpawnApp" => Some("spawn_app"),
        "org.cgroupd.Manager.StopSession" => Some("stop_session"),
        "org.cgroupd.Manager.GetState" => Some("get_state"),
        "org.cgroupd.Manager.WaitSession" => Some("wait_session"),
        _ => None,
    }
}

/// Build the call object for `req`. The `Request` serde form is
/// `{"op": ..., ...flat params}`, so parameters are everything but `op`.
pub fn build_call(req: &Request) -> Result<serde_json::Value, String> {
    let mut params = serde_json::to_value(req).map_err(|e| format!("encode request: {e}"))?;
    let has_op = params
        .as_object_mut()
        .map(|o| o.remove("op").is_some())
        .unwrap_or(false);
    if !has_op {
        return Err("request has no op".to_string());
    }
    Ok(serde_json::json!({"method": method_name(req), "parameters": params}))
}

/// Parse a call's `parameters` back into a `Request` for a known method.
pub fn parse_call(method: &str, parameters: serde_json::Value) -> Result<Request, String> {
    let Some(op) = op_for_method(method) else {
        return Err(format!("unknown method {method}"));
    };
    let mut obj = match parameters {
        serde_json::Value::Object(o) => o,
        serde_json::Value::Null => serde_json::Map::new(),
        _ => return Err("parameters must be an object".to_string()),
    };
    obj.insert("op".to_string(), serde_json::Value::String(op.to_string()));
    serde_json::from_value(serde_json::Value::Object(obj))
        .map_err(|e| format!("bad parameters for {method}: {e}"))
}

/// Success reply for a `detail` string.
pub fn ok_reply(detail: String) -> serde_json::Value {
    serde_json::json!({"parameters": {"detail": detail}})
}

/// Error reply. `message` carries the human text (spec-sanctioned pattern).
pub fn err_reply(name: &str, message: String) -> serde_json::Value {
    serde_json::json!({"error": name, "parameters": {"message": message}})
}

/// Split a reply into `Ok(detail)` / `Err((name, message))`.
pub fn split_reply(reply: &serde_json::Value) -> Result<String, (String, String)> {
    if let Some(name) = reply.get("error").and_then(|v| v.as_str()) {
        let message = reply
            .get("parameters")
            .and_then(|p| p.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error")
            .to_string();
        return Err((name.to_string(), message));
    }
    match reply
        .get("parameters")
        .and_then(|p| p.get("detail"))
        .and_then(|d| d.as_str())
    {
        Some(d) => Ok(d.to_string()),
        None => Err((
            ERR_INTERNAL.to_string(),
            "malformed reply (no parameters.detail)".to_string(),
        )),
    }
}

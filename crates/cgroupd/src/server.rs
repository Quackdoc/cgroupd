//! Unix socket server: Varlink framing, peer-cred policy, dispatch.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use cgroupd_common::Roots;
use cgroupd_common::read_frame;
use cgroupd_common::varlink;

use crate::config::Config;

const IDL: &str = include_str!("../../../etc/org.cgroupd.Manager.varlink");

pub fn peer_uid(stream: &UnixStream) -> Result<u32, String> {
    rustix::net::sockopt::socket_peercred(stream)
        .map(|cred| cred.uid.as_raw())
        .map_err(|e| format!("SO_PEERCRED: {e}"))
}

fn info_reply() -> serde_json::Value {
    serde_json::json!({"parameters": {
        "vendor": "cgroupd",
        "product": "cgroupd",
        "version": env!("CARGO_PKG_VERSION"),
        "url": "https://github.com/anomalyco/cgroupd",
        "interfaces": [varlink::INTERFACE, varlink::SERVICE_INTERFACE],
    }})
}

/// Handle one call object. Returns `None` when the reply is suppressed
/// (`oneway`) or the connection should close.
fn handle_call(frame: &[u8], roots: &Roots, cfg: &Config, peer: u32) -> Option<serde_json::Value> {
    let call: serde_json::Value = match serde_json::from_slice(frame) {
        Ok(v) => v,
        Err(e) => {
            return Some(varlink::err_reply(
                varlink::ERR_INVALID_PARAMETER,
                format!("bad JSON: {e}"),
            ));
        }
    };
    let method = call
        .get("method")
        .and_then(|m| m.as_str())
        .unwrap_or_default()
        .to_string();
    let oneway = call
        .get("oneway")
        .and_then(|o| o.as_bool())
        .unwrap_or(false);

    let reply = if method == "org.varlink.service.GetInfo" {
        info_reply()
    } else if method == "org.varlink.service.GetInterfaceDescription" {
        let iface = call
            .get("parameters")
            .and_then(|p| p.get("interface"))
            .and_then(|i| i.as_str())
            .unwrap_or_default();
        if iface == varlink::INTERFACE {
            serde_json::json!({"parameters": {"description": IDL}})
        } else {
            varlink::err_reply(
                varlink::ERR_INTERFACE_NOT_FOUND,
                format!("unknown interface {iface}"),
            )
        }
    } else if method.starts_with(varlink::INTERFACE)
        || method.starts_with(varlink::SERVICE_INTERFACE)
    {
        if !varlink::method_known(&method) {
            varlink::err_reply(
                varlink::ERR_METHOD_NOT_FOUND,
                format!("unknown method {method}"),
            )
        } else {
            let params = call
                .get("parameters")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            match varlink::parse_call(&method, params) {
                Ok(req) => match crate::api::execute(roots, cfg, &req, peer) {
                    Ok(detail) => varlink::ok_reply(detail),
                    Err((name, message)) => varlink::err_reply(name, message),
                },
                Err(e) => varlink::err_reply(varlink::ERR_INVALID_PARAMETER, e),
            }
        }
    } else {
        varlink::err_reply(
            varlink::ERR_INTERFACE_NOT_FOUND,
            format!("unknown method {method}"),
        )
    };
    if oneway { None } else { Some(reply) }
}

/// Handle a connection: serve Varlink calls until EOF or protocol error.
/// Peer credentials are resolved once; every call on the connection shares
/// the caller identity.
pub fn handle_connection(mut stream: UnixStream, roots: &Roots, cfg: &Config) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));

    let peer = match peer_uid(&stream) {
        Ok(p) => p,
        Err(e) => {
            crate::log::warning(&format!("peercred failed: {e}"));
            let reply = varlink::err_reply(
                varlink::ERR_INTERNAL,
                "cannot determine peer credentials".to_string(),
            );
            if let Ok(buf) = cgroupd_common::framing::encode_frame(&reply) {
                let _ = stream.write_all(&buf);
                let _ = stream.flush();
            }
            return;
        }
    };
    loop {
        let frame = match read_frame(&stream) {
            Ok(f) => f,
            Err(cgroupd_common::FrameError::Empty) => break,
            Err(cgroupd_common::FrameError::TooLarge) => {
                let reply = varlink::err_reply(
                    varlink::ERR_INVALID_PARAMETER,
                    "request too large".to_string(),
                );
                if cgroupd_common::framing::encode_frame(&reply)
                    .map(|buf| stream.write_all(&buf).and_then(|()| stream.flush()))
                    .is_err()
                {
                    break;
                }
                break;
            }
            Err(cgroupd_common::FrameError::Io(_)) => break,
        };
        let Some(reply) = handle_call(&frame, roots, cfg, peer) else {
            continue;
        };
        let buf = match cgroupd_common::framing::encode_frame(&reply) {
            Ok(b) => b,
            Err(e) => {
                crate::log::warning(&format!("encode reply: {e}"));
                break;
            }
        };
        if stream
            .write_all(&buf)
            .and_then(|()| stream.flush())
            .is_err()
        {
            break;
        }
    }
}

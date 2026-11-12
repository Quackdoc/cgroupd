//! Syslog via `/dev/log`, plus stderr for supervisors.
//!
//! The old code only mirrored to stderr on a tty, which meant zero output
//! under `dinit`/`runit`/`s6` (pipe, not tty). Now: `/dev/log` + stderr by
//! default; `--log-target` / `CGROUPD_LOG` selects `syslog|stderr|both`.

use std::io::Write;
use std::sync::atomic::{AtomicU8, Ordering};

// PRI = facility*8 + severity; facility LOG_DAEMON (3).
const PRI_INFO: u8 = 3 * 8 + 6;
const PRI_WARNING: u8 = 3 * 8 + 4;
const PRI_ERR: u8 = 3 * 8 + 3;

const TARGET_BOTH: u8 = 0;
const TARGET_SYSLOG: u8 = 1;
const TARGET_STDERR: u8 = 2;

static TARGET: AtomicU8 = AtomicU8::new(TARGET_BOTH);

pub fn init(_ident: &str) {
    if let Ok(v) = std::env::var("CGROUPD_LOG") {
        set_target(&v);
    }
}

pub fn set_target(s: &str) {
    let t = match s.to_ascii_lowercase().as_str() {
        "syslog" => TARGET_SYSLOG,
        "stderr" => TARGET_STDERR,
        _ => TARGET_BOTH,
    };
    TARGET.store(t, Ordering::Relaxed);
}

fn emit(pri: u8, msg: &str) {
    let flat: String = msg
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    let flat = if flat.len() > 2048 {
        &flat[..flat.floor_char_boundary(2048)]
    } else {
        &flat
    };
    let t = TARGET.load(Ordering::Relaxed);
    if t == TARGET_BOTH || t == TARGET_SYSLOG {
        let line = format!("<{pri}>cgroupd[{}]: {flat}", std::process::id());
        if let Ok(sock) = std::os::unix::net::UnixDatagram::unbound() {
            let _ = sock.send_to(line.as_bytes(), "/dev/log");
        }
    }
    if t == TARGET_BOTH || t == TARGET_STDERR {
        let _ = std::io::stderr().write_all(format!("{msg}\n").as_bytes());
    }
}

pub fn info(msg: &str) {
    emit(PRI_INFO, msg);
}

pub fn warning(msg: &str) {
    emit(PRI_WARNING, msg);
}

pub fn err(msg: &str) {
    emit(PRI_ERR, msg);
}

//! D-Bus interface mirroring the socket ops, on the **system** bus.
//!
//! Identity is per-call via `org.freedesktop.DBus.GetConnectionUnixUser`:
//! implicit (caller) methods resolve to the sender UID, `*As` methods take
//! an explicit target UID and are honored only for root senders.

mod dispatch;
mod ops;
mod wire;

use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use cgroupd_common::Roots;

use crate::config::Config;

pub fn run(roots: Roots, cfg: Config) {
    // Fail-open escape hatch: the socket API works without D-Bus.
    // Set CGROUPD_NO_DBUS=1 (or pass --no-dbus) on systems without a
    // system bus or when the bus policy is unavailable.
    if std::env::var("CGROUPD_NO_DBUS")
        .ok()
        .filter(|s| s == "1" || s.eq_ignore_ascii_case("true"))
        .is_some()
    {
        crate::log::info("D-Bus disabled via CGROUPD_NO_DBUS, socket only");
        return;
    }
    let (wait_tx, wait_rx): (Sender<dbus::Message>, Receiver<dbus::Message>) = mpsc::channel();
    let mut attempts: u64 = 0;
    loop {
        match serve_once(&roots, &cfg, &wait_tx, &wait_rx) {
            Ok(()) => {
                crate::log::warning("cgroupd D-Bus loop ended, retrying in 5s");
                std::thread::sleep(Duration::from_secs(5));
            }
            Err(e) => {
                attempts += 1;
                if attempts == 1 {
                    crate::log::warning(&format!(
                        "cgroupd D-Bus unavailable: {e}; retry in 30s (socket still live; set CGROUPD_NO_DBUS=1 to silence)"
                    ));
                } else {
                    crate::log::warning(&format!(
                        "cgroupd D-Bus unavailable: {e}; retry in 30s (attempt {attempts})"
                    ));
                }
                while wait_rx.try_recv().is_ok() {}
                std::thread::sleep(Duration::from_secs(30));
            }
        }
    }
}

fn bus_type() -> dbus::ffidisp::BusType {
    // System bus is correct for a root service. Session is dev-only:
    // `CGROUPD_DBUS_BUS=session` for manual testing without a system bus.
    match std::env::var("CGROUPD_DBUS_BUS").as_deref() {
        Ok("session") => dbus::ffidisp::BusType::Session,
        _ => dbus::ffidisp::BusType::System,
    }
}

/// Connect, claim the name and serve until the bus connection fails.
fn serve_once(
    roots: &Roots,
    cfg: &Config,
    wait_tx: &Sender<dbus::Message>,
    wait_rx: &Receiver<dbus::Message>,
) -> Result<(), String> {
    let bt = bus_type();
    let label = match bt {
        dbus::ffidisp::BusType::System => "system",
        _ => "session",
    };
    let conn =
        dbus::ffidisp::Connection::get_private(bt).map_err(|e| format!("{label} bus: {e}"))?;
    conn.register_name(wire::SERVICE, 0)
        .map_err(|e| format!("request name: {e}"))?;
    conn.register_object_path(wire::OBJECT_PATH)
        .map_err(|e| format!("register path: {e}"))?;
    crate::log::info(&format!(
        "D-Bus service {} on {} ({}) via {label} bus",
        wire::SERVICE,
        wire::OBJECT_PATH,
        wire::INTERFACE
    ));
    loop {
        for msg in conn.incoming(100) {
            if let Some(reply) = dispatch::handle_message(&conn, &msg, roots, cfg, wait_tx) {
                let _ = conn.send(reply);
            }
        }
        // Replies from `Wait*` threads, which answer after the call returned.
        while let Ok(reply) = wait_rx.try_recv() {
            let _ = conn.send(reply);
        }
    }
}

//! Shutdown signals, handled with `signal-hook` (no blocked masks, no unsafe).

use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
use signal_hook::iterator::Signals;

/// Start catching TERM/INT/HUP. Call once, before the worker threads start.
pub fn install() -> std::io::Result<Signals> {
    Signals::new([SIGTERM, SIGINT, SIGHUP])
}

/// Serve the signals forever. `socket_path` is unlinked on TERM/INT before
/// exit so restarts never see a stale socket. HUP only logs (config reload
/// needs a shared-state refactor; restart the daemon to apply config).
pub fn wait_forever(mut signals: Signals, socket_path: String) -> ! {
    for sig in signals.forever() {
        if sig == SIGHUP {
            crate::log::warning("SIGHUP: config reload not implemented, restart cgroupd");
            continue;
        }
        crate::log::info(&format!("signal {sig}: shutting down"));
        let _ = std::fs::remove_file(&socket_path);
        std::process::exit(0);
    }
    unreachable!("signal iterator ended")
}

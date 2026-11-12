//! Event-driven reaper: remove session/app cgroups as soon as they empty.
//!
//! Each registered unit gets an inotify watch on its `cgroup.events`; the
//! kernel notifies when `populated` flips, so the daemon sleeps in `read`
//! with no timers. A sweep at startup adopts and cleans pre-existing units.
//! Without inotify it falls back to a periodic sweep.

use std::collections::HashMap;
use std::mem::MaybeUninit;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use cgroupd_common::{Roots, Scope};
use rustix::fs::inotify::{self, CreateFlags, ReadFlags, WatchFlags};

struct Watcher {
    fd: OwnedFd,
    units: Mutex<HashMap<i32, (u32, Scope, PathBuf)>>,
}

static WATCHER: OnceLock<Watcher> = OnceLock::new();

/// Watch a unit's `cgroup.events`. Best effort and idempotent (re-adding the
/// same inode returns the same wd). No-op before `run` or where the file
/// does not exist (e.g. fake test roots).
pub fn watch(unit_dir: &Path, uid: u32, scope: Scope) {
    let Some(w) = WATCHER.get() else { return };
    let events = unit_dir.join("cgroup.events");
    let Ok(wd) = inotify::add_watch(&w.fd, &events, WatchFlags::MODIFY) else {
        return;
    };
    if let Ok(mut m) = w.units.lock() {
        m.insert(wd, (uid, scope, unit_dir.to_path_buf()));
    }
}

fn log_sweep(summary: &str) {
    // Quiet when nothing was removed.
    if !summary.starts_with("sweep: removed 0 ") {
        crate::log::info(summary);
    }
}

pub fn run(roots: Roots) {
    let fd = match inotify::init(CreateFlags::CLOEXEC) {
        Ok(fd) => fd,
        Err(e) => {
            crate::log::err(&format!("inotify_init: {e}; falling back to polling"));
            return poll_forever(roots);
        }
    };
    let w = WATCHER.get_or_init(|| Watcher {
        fd,
        units: Mutex::new(HashMap::new()),
    });

    // Adopt and clean up whatever exists (watch before check; see fs::sweep).
    crate::log::info(&crate::fs::sweep(&roots));

    let mut buf = [MaybeUninit::<u8>::uninit(); 4096];
    let mut reader = inotify::Reader::new(&w.fd, &mut buf);
    loop {
        let ev = match reader.next() {
            Ok(ev) => ev,
            Err(rustix::io::Errno::INTR) => continue,
            Err(e) => {
                crate::log::err(&format!("inotify read: {e}"));
                std::thread::sleep(Duration::from_secs(5));
                continue;
            }
        };
        let (wd, flags) = (ev.wd(), ev.events());
        if flags.contains(ReadFlags::QUEUE_OVERFLOW) {
            log_sweep(&crate::fs::sweep(&roots));
            continue;
        }
        if flags.contains(ReadFlags::IGNORED) {
            // Watched file is gone (unit removed).
            if let Ok(mut m) = w.units.lock() {
                m.remove(&wd);
            }
            continue;
        }
        if !flags.contains(ReadFlags::MODIFY) {
            continue;
        }
        let unit = w.units.lock().ok().and_then(|m| m.get(&wd).cloned());
        if let Some((uid, scope, sp)) = unit {
            let d = crate::fs::remove_at(uid, scope, &sp);
            if d.contains("removed") {
                crate::log::info(&d);
            }
        }
    }
}

/// Fallback when inotify is unavailable.
fn poll_forever(roots: Roots) {
    loop {
        std::thread::sleep(Duration::from_secs(60));
        log_sweep(&crate::fs::sweep(&roots));
    }
}

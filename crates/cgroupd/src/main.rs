#![forbid(unsafe_code)]

mod api;
mod config;
#[cfg(feature = "dbus")]
mod dbus;
mod exec_helper;
mod fs;
mod log;
mod reaper;
mod server;
mod signals;
mod spawn;

use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;

use config::Config;

const DEFAULT_CONFIG: &str = "/etc/cgroupd.conf";

fn print_help() {
    eprintln!(
        "usage: cgroupd [--config PATH] [--socket PATH] [--log-target syslog|stderr|both] [--no-dbus] [--help]"
    );
    eprintln!("Foreground-only system service. No daemonize/pidfile.");
    eprintln!("Callers are identified implicitly (SO_PEERCRED / D-Bus peer).");
    eprintln!("--no-dbus (or CGROUPD_NO_DBUS=1) serves the Unix socket only.");
}

fn is_test_mode() -> bool {
    std::env::var("FAKE_CGROUP_ROOT")
        .ok()
        .filter(|s| !s.is_empty())
        .is_some()
}

fn check_root() {
    if is_test_mode() {
        return;
    }
    if rustix::process::geteuid().as_raw() != 0 {
        eprintln!("cgroupd must run as root (or set FAKE_CGROUP_ROOT for tests)");
        std::process::exit(1);
    }
}

fn check_cgroup2() {
    if is_test_mode() {
        return;
    }
    let mounted = std::fs::read_to_string("/proc/self/mountinfo")
        .map(|c| {
            c.lines().any(|l| {
                l.split_whitespace().any(|w| w == "cgroup2") && l.contains("/sys/fs/cgroup")
            })
        })
        .unwrap_or(false);
    if !mounted {
        log::warning("cgroup2 not mounted at /sys/fs/cgroup (continuing, hierarchy init may fail)");
    }
}

/// Hold an exclusive lock for the socket path for the daemon lifetime.
/// Returns the lock file handle (must be kept alive / intentionally leaked).
fn take_singleton_lock(socket_path: &str) -> std::fs::File {
    let lock_path = format!("{socket_path}.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .unwrap_or_else(|e| {
            eprintln!("lock {lock_path}: {e}");
            std::process::exit(1);
        });
    use std::os::unix::io::AsFd;
    match rustix::fs::flock(
        file.as_fd(),
        rustix::fs::FlockOperation::NonBlockingLockExclusive,
    ) {
        Ok(()) => file,
        Err(_) => {
            eprintln!("another cgroupd instance holds {lock_path}");
            std::process::exit(1);
        }
    }
}

fn main() {
    // Helper mode comes first: it must not touch config, locks or logging.
    if std::env::args().nth(1).as_deref() == Some(exec_helper::FLAG) {
        exec_helper::run();
    }
    let mut config_path = DEFAULT_CONFIG.to_string();
    let mut socket_override: Option<String> = None;
    let mut no_dbus = std::env::var("CGROUPD_NO_DBUS")
        .ok()
        .filter(|s| s == "1" || s.eq_ignore_ascii_case("true"))
        .is_some();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--config" => {
                if let Some(v) = args.next() {
                    config_path = v;
                } else {
                    eprintln!("--config needs a value");
                    std::process::exit(2);
                }
            }
            "--socket" => {
                if let Some(v) = args.next() {
                    socket_override = Some(v);
                } else {
                    eprintln!("--socket needs a value");
                    std::process::exit(2);
                }
            }
            "--log-target" => {
                if let Some(v) = args.next() {
                    log::set_target(&v);
                } else {
                    eprintln!("--log-target needs a value");
                    std::process::exit(2);
                }
            }
            "--no-dbus" => {
                no_dbus = true;
            }
            "--help" | "-h" => {
                print_help();
                return;
            }
            other => {
                eprintln!("unknown arg: {other}");
                print_help();
                std::process::exit(2);
            }
        }
    }

    log::init("cgroupd");
    let signals = signals::install().unwrap_or_else(|e| {
        log::err(&format!("signal handlers: {e}"));
        std::process::exit(1);
    });
    exec_helper::pin_self();

    check_root();
    check_cgroup2();

    if !std::path::Path::new(&config_path).exists() {
        log::warning(&format!(
            "config {config_path} missing, using built-in defaults"
        ));
    }
    let mut cfg = match Config::load(&config_path) {
        Ok(c) => c,
        Err(e) => {
            log::err(&format!("config: {e}"));
            eprintln!("config error: {e}");
            std::process::exit(1);
        }
    };
    if let Some(s) = socket_override {
        cfg.socket_path = s;
    }
    if let Err(e) = cfg.validate() {
        log::err(&format!("config: {e}"));
        eprintln!("config error: {e}");
        std::process::exit(1);
    }
    let roots = cgroupd_common::resolve_roots();

    // Singleton first: second instance exits here instead of stealing the socket.
    let _lock = take_singleton_lock(&cfg.socket_path);

    // Bind early so logins queue during hierarchy init instead of failing.
    let _ = std::fs::remove_file(&cfg.socket_path);
    let listener = match UnixListener::bind(&cfg.socket_path) {
        Ok(l) => l,
        Err(e) => {
            log::err(&format!("bind {}: {e}", cfg.socket_path));
            eprintln!("bind {}: {e}", cfg.socket_path);
            std::process::exit(1);
        }
    };
    // World-connectable; auth is per-request via SO_PEERCRED + resolve_uid.
    // (Deliberate: 0700 would lock out the very users we serve.)
    let _ = std::fs::set_permissions(&cfg.socket_path, std::fs::Permissions::from_mode(0o777));

    match fs::init_hierarchy(&roots, &cfg) {
        Ok(()) => log::info("hierarchy ready"),
        Err(e) => {
            log::err(&e);
            eprintln!("init: {e}");
            let _ = std::fs::remove_file(&cfg.socket_path);
            std::process::exit(1);
        }
    }
    log::info(&format!("listening on {}", cfg.socket_path));

    {
        let sock = cfg.socket_path.clone();
        std::thread::spawn(move || signals::wait_forever(signals, sock));
    }
    {
        let roots = roots.clone();
        std::thread::spawn(move || reaper::run(roots));
    }

    #[cfg(feature = "dbus")]
    {
        if no_dbus {
            log::info("D-Bus disabled via --no-dbus/CGROUPD_NO_DBUS, socket only");
        } else {
            let roots = roots.clone();
            let cfg = cfg.clone();
            std::thread::spawn(move || dbus::run(roots, cfg));
        }
    }

    // Bounded at 64 concurrent connections; excess is dropped. WaitSession
    // has its own 32-slot budget inside spawn/state.rs so waits can't wedge logins.
    use std::sync::atomic::{AtomicUsize, Ordering};
    static ACTIVE_CONNS: AtomicUsize = AtomicUsize::new(0);
    const MAX_CONNS: usize = 64;
    for conn in listener.incoming() {
        match conn {
            Ok(stream) => {
                let n = ACTIVE_CONNS.fetch_add(1, Ordering::SeqCst) + 1;
                if n > MAX_CONNS {
                    ACTIVE_CONNS.fetch_sub(1, Ordering::SeqCst);
                    crate::log::warning("too many connections, dropping");
                    drop(stream);
                    continue;
                }
                let roots = roots.clone();
                let cfg = cfg.clone();
                std::thread::spawn(move || {
                    struct Guard;
                    impl Drop for Guard {
                        fn drop(&mut self) {
                            ACTIVE_CONNS.fetch_sub(1, Ordering::SeqCst);
                        }
                    }
                    let _g = Guard;
                    server::handle_connection(stream, &roots, &cfg);
                });
            }
            Err(e) => {
                log::warning(&format!("accept: {e}"));
            }
        }
    }
}

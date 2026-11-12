#!/usr/bin/env python3
"""cgrun: launch an app in a new cgroup via cgroupd's D-Bus API.

Mirrors how libcosmic launches apps with systemd
(`spawn_desktop_exec` in libcosmic/src/desktop.rs):

1. Double-fork + setsid locally, so the app inherits our full environment
   (WAYLAND_DISPLAY, XDG_RUNTIME_DIR, ...) with no --env flags needed.
2. Register the grandchild pid with the daemon, which moves it into a
   fresh ``app/<aid>/`` cgroup (cf. libcosmic's ``StartTransientUnit``
   for ``app-cosmic-<app_id>-<pid>.scope`` with ``PIDs=[pid]``).

Usage:
    cgrun.py [--aid AID] [--bus system|session] [--wait] [--stdio inherit]
             [--exec "prog args..."] -- prog [args...]
    cgrun.py --scope [--aid AID] [--bus system|session] [--as UID]
             [--exec "prog args..."] -- prog [args...]

Scope mode (--scope) behaves like `systemd-run --scope`: the command runs
attached to this terminal in a fresh ``app/<aid>/`` cgroup. Single fork
(no setsid), the child registers itself synchronously then execs in place;
the parent waits, forwards terminal signals, and propagates the exit status.
The default detached mode above is for GUI launchers instead.

Examples:
    cgrun.py -- /usr/bin/cosmic-term
    cgrun.py --aid game1 -- /usr/bin/game --fullscreen
    cgrun.py --exec "cosmic-term -e htop"   # desktop-Entry style, %codes skipped
    CGROUPD_DBUS_BUS=session cgrun.py --bus session -- /bin/sleep 60
"""

import argparse
import json
import os
import re
import shlex
import sys
import time

AID_RE = re.compile(r"^[A-Za-z0-9_-]{1,64}$")

SERVICE = "org.cgroupd"
OBJECT_PATH = "/org/cgroupd/Manager"
INTERFACE = "org.cgroupd.Manager"


def sanitize_aid(aid):
    aid = re.sub(r"[^A-Za-z0-9_-]", "_", aid)[:64]
    if not aid:
        aid = "app"
    return aid


def default_aid(executable, pid):
    base = sanitize_aid(os.path.basename(executable))
    # Like libcosmic's app-cosmic-<app_id>-<pid>.scope naming.
    aid = f"app-{base}-{pid}"[:64]
    return aid


def split_exec_string(exec_str):
    """Desktop-Entry style parsing: shlex words, skip % field codes."""
    words = [w for w in shlex.split(exec_str) if not w.startswith("%")]
    if not words:
        raise ValueError("empty --exec string")
    return words


def spawn_detached(argv, null_stdio):
    """libcosmic process::spawn: double fork + setsid, pid back over a pipe."""
    read_fd, write_fd = os.pipe2(os.O_CLOEXEC)
    pid1 = os.fork()
    if pid1 > 0:
        # Original parent: read grandchild pid, reap intermediate.
        os.close(write_fd)
        with os.fdopen(read_fd, "rb") as f:
            data = f.read(4)
        _, _ = os.waitpid(pid1, 0)
        if len(data) != 4:
            raise RuntimeError("failed to launch command (exec probably failed)")
        return int.from_bytes(data, "big")
    # Intermediate child.
    os.close(read_fd)
    try:
        os.setsid()
        pid2 = os.fork()
        if pid2 > 0:
            os.write(write_fd, pid2.to_bytes(4, "big"))
            os.close(write_fd)
            os._exit(0)
        # Grandchild: the app. Inherits our full environment.
        os.close(write_fd)
        if null_stdio:
            null = os.open(os.devnull, os.O_RDWR)
            for fd in (0, 1, 2):
                try:
                    os.dup2(null, fd)
                except OSError:
                    pass
            if null > 2:
                os.close(null)
        os.execvp(argv[0], argv)
    except Exception as e:  # noqa: BLE001 - must not return through forks
        try:
            sys.stderr.write(f"cgrun: child launch failed: {e}\n")
        except OSError:
            pass
        os._exit(1)


def dbus_iface(bus_kind):
    import dbus

    if bus_kind == "session":
        bus = dbus.SessionBus()
    else:
        bus = dbus.SystemBus()
    obj = bus.get_object(SERVICE, OBJECT_PATH)
    return dbus.Interface(obj, dbus_interface=INTERFACE)


def dbus_call(iface, method, *args):
    import dbus

    try:
        reply = getattr(iface, method)(*args)
    except dbus.DBusException as e:
        sys.stderr.write(f"cgrun: D-Bus {method} failed: {e.get_dbus_name()}: {e}\n")
        sys.exit(1)
    try:
        payload = json.loads(str(reply))
    except ValueError:
        sys.stderr.write(f"cgrun: bad daemon reply: {reply!r}\n")
        sys.exit(1)
    if payload.get("status") == "ok":
        return payload.get("detail", "")
    sys.stderr.write(f"cgrun: daemon error: {payload.get('msg', payload)}\n")
    sys.exit(1)


def wait_app(iface, aid, timeout):
    import dbus

    deadline = time.monotonic() + timeout
    while True:
        try:
            reply = iface.GetAppState(aid)
            payload = json.loads(str(reply))
        except dbus.DBusException as e:
            sys.stderr.write(f"cgrun: wait failed: {e}\n")
            return 1
        detail = payload.get("detail", "") if payload.get("status") == "ok" else ""
        if "already gone" in detail:
            print(detail)
            return 0
        try:
            state = json.loads(detail) if detail.startswith("{") else {}
        except ValueError:
            state = {}
        if state.get("state") in ("inactive", "failed"):
            print(detail)
            return 0
        if time.monotonic() >= deadline:
            sys.stderr.write("cgrun: wait timeout\n")
            return 1
        time.sleep(0.2)


SCOPED_SIGNALS = ("SIGINT", "SIGTERM", "SIGHUP", "SIGQUIT", "SIGWINCH")


def scoped_child(args, cmd):
    """Register this process in a fresh app/<aid>/, then exec.

    Returns an exit code; the caller must os._exit() it. Never raises
    through the fork: dbus_call() exits via sys.exit, converted here.
    The bus connection is created after the fork, never shared across it.
    """
    try:
        import dbus

        as_args = (dbus.UInt32(args.as_uid),) if args.as_uid is not None else ()
        iface = dbus_iface(args.bus)
        aid = sanitize_aid(args.aid) if args.aid else default_aid(cmd[0], os.getpid())
        if not AID_RE.match(aid):
            sys.stderr.write(f"cgrun: bad aid {aid!r} (want [A-Za-z0-9_-]{{1,64}})\n")
            return 2
        dbus_call(iface, "EnsureUser", *as_args)
        dbus_call(iface, "CreateApp", *as_args, aid, dbus.UInt32(os.getpid()))
    except SystemExit as e:
        return e.code if isinstance(e.code, int) else 1
    except Exception as e:  # noqa: BLE001 - report everything, never traceback through fork
        sys.stderr.write(f"cgrun: scope setup failed: {e}\n")
        return 1
    try:
        os.execvp(cmd[0], cmd)
    except Exception as e:  # noqa: BLE001
        sys.stderr.write(f"cgrun: exec {cmd[0]} failed: {e}\n")
        return 127
    return 127


def run_scoped(args, cmd):
    """systemd-run --scope equivalent: fork (no setsid), child registers
    itself synchronously then execs in place; parent forwards terminal
    signals, waits, and propagates the exit status."""
    import signal

    try:
        pid = os.fork()
    except OSError as e:
        sys.stderr.write(f"cgrun: fork failed: {e}\n")
        return 1
    if pid == 0:
        os._exit(scoped_child(args, cmd))
    # Parent: same process group and terminal (no setsid), so terminal
    # signals already reach the child; still forward direct kills.
    def fwd(signum, _frame):
        try:
            os.kill(pid, signum)
        except ProcessLookupError:
            pass

    prev = {}
    for name in SCOPED_SIGNALS:
        sig = getattr(signal, name, None)
        if sig is None:
            continue
        try:
            prev[sig] = signal.signal(sig, fwd)
        except (OSError, ValueError):
            pass
    status = None
    while True:
        try:
            _, status = os.waitpid(pid, 0)
            break
        except InterruptedError:
            continue
        except ChildProcessError:
            break
    for sig, handler in prev.items():
        try:
            signal.signal(sig, handler)
        except (OSError, ValueError):
            pass
    if status is None:
        return 1
    if os.WIFEXITED(status):
        return os.WEXITSTATUS(status)
    if os.WIFSIGNALED(status):
        sig = os.WTERMSIG(status)
        # Die with the same signal so callers see a signaled death.
        try:
            signal.signal(sig, signal.SIG_DFL)
        except (OSError, ValueError):
            return 128 + sig
        os.kill(os.getpid(), sig)
        return 128 + sig
    return 1


def main(argv=None):
    ap = argparse.ArgumentParser(
        description="Launch an app in a fresh app/<aid>/ cgroup via cgroupd D-Bus."
    )
    ap.add_argument("--aid", help="app id (default: app-<prog>-<pid>)")
    ap.add_argument("--bus", choices=("system", "session"), default="system")
    ap.add_argument("--as", dest="as_uid", type=int, help="target uid (root only)")
    ap.add_argument("--exec", help="desktop-style command string (%%codes skipped)")
    ap.add_argument("--wait", action="store_true", help="wait until the app exits")
    ap.add_argument("--timeout", type=int, default=30, help="wait timeout (with --wait)")
    ap.add_argument(
        "--scope",
        action="store_true",
        help="run attached like systemd-run --scope: stay on this terminal "
        "in a fresh app/<aid>/, wait, propagate exit status",
    ) 
    ap.add_argument(
        "--stdio",
        choices=("null", "inherit"),
        default="null",
        help="null: detach quietly like a launcher (default); inherit: keep stdio",
    )
    ap.add_argument("command", nargs=argparse.REMAINDER, help="-- prog [args...]")
    args = ap.parse_args(argv)

    if args.exec:
        cmd = split_exec_string(args.exec)
    else:
        cmd = args.command
        if cmd and cmd[0] == "--":
            cmd = cmd[1:]
        if not cmd:
            ap.error("need -- prog [args...] or --exec STRING")

    as_args = ()
    if args.as_uid is not None:
        import dbus

        as_args = (dbus.UInt32(args.as_uid),)

    if args.scope:
        if args.stdio == "null":
            ap.error("--scope needs inherited stdio (pass --stdio inherit)")
        if args.wait:
            ap.error("--wait is inherent in --scope mode")
        return run_scoped(args, cmd)

    iface = dbus_iface(args.bus)
    dbus_call(iface, "EnsureUser", *as_args)

    pid = spawn_detached(cmd, null_stdio=(args.stdio == "null"))
    aid = sanitize_aid(args.aid) if args.aid else default_aid(cmd[0], pid)
    if not AID_RE.match(aid):
        sys.stderr.write(f"cgrun: bad aid {aid!r} (want [A-Za-z0-9_-]{{1,64}})\n")
        sys.exit(2)

    import dbus

    detail = dbus_call(iface, "CreateApp", *as_args, aid, dbus.UInt32(pid))
    print(detail)
    if args.wait:
        return wait_app(iface, aid, args.timeout)
    return 0


if __name__ == "__main__":
    sys.exit(main())

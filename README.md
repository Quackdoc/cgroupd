# cgroupd — cgroup v2 management without systemd

Automatic cgroup v2 management for non-systemd inits `dinit`, `openrc`,
`runit`, `s6`.

The goal of this is to be close enough to a drop in replacement for systemd/elogind
that we can entirely replace them. The current app is enough to run cosmic when paired
with login1-liberated (not currently released). Currently you need to run the 
systemd-shim.py to replace it completely. However the shim is not typically necessary 
for basic use on most DEs.

I am using dbus cgroupd for systemd shim but it may be replaced with varlink in the future.

Alternatively, use `cgrun` from [cgrun-rs](https://github.com/quackdoc/cgrun-rs) if you
just need a cgroup daemon agnostic runner.

Components:

- **`cgroupd`** — root daemon, serves `/run/cgroupd.sock` and dbus
- **`cgctl`** — CLI for managing users, sessions, and spawned apps
- **`cgctl pam`** — `pam_exec.so` helper placing each login into its own cgroup
- **Interfaces** — Unix socket (Varlink JSON) plus optional D-Bus
  (`org.cgroupd.Manager`, built by default, `--no-dbus` to disable at runtime)
- **`scripts/cgrun.py`** — client-side app launcher over D-Bus
- **`scripts/systemd-shim.py`** — minimal `org.freedesktop.systemd1` shim on the
  session bus so desktop apps (kde, cosmic, maybe gnome) place processes in app cgroups

## AI Use Disclosure

AI was used extensively for checking our behavior against systemd and elogind. The goal
of this program is to not only be a usable cgroup daemon for non systemd systems,
but also to be an improvement to the authors taste.

Because of this AI was used extensively to prototype and rapidly iterate and fix 
breaking behavior in order to find the best compromises. All code has been human reviewed.

Documentation and code comments was largely made using AI for proof reading. AI was also
used for non dinit install.

## Install

```bash
# Build (D-Bus support is on by default; `just build-release --no-default-features` to drop it)
just build-release       # -> target/release/{cgroupd,cgctl}

# Install binaries, config, D-Bus policy, varlink interface, PAM example
sudo just install                     # prefix=/usr, config in /etc
sudo just prefix=/usr/local install   # note: init files hardcode /usr/bin/cgroupd

# Install the service for your init system
sudo just install-dinit    # or install-openrc / install-runit / install-s6
                           # (runit on Void: sv-dir=/etc/sv)

# Packagers: stage into a directory
just rootdir="$pkgdir" prefix=/usr install install-dinit

# Remove
sudo just uninstall
```

After installing the D-Bus policy, reload the system bus
(`dbus-send --system --dest=org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus.ReloadConfig`
or restart `dbus`).

## Configuration

`etc/cgroupd.conf.example`:

```toml
controllers = ["cpu", "cpuset", "memory", "pids", "io"]
socket_path = "/run/cgroupd.sock"
allow_root = false
allow_spawn = true
manage_ancestors = false
default_grace_secs = 2
spawn_start_timeout_secs = 90
```

All keys are optional; unknown keys are ignored. `socket_path` must be under
`/run/` or `/var/run/`; `default_grace_secs` ≤ 300; `spawn_start_timeout_secs`
5..=600. Place at `/etc/cgroupd.conf` (or pass `--config PATH`). Missing file →
warning + built-in defaults. `allow_root = true` lets uid `0` be managed;
managing other users' sessions works with `false`.

## CLI (`cgctl`)

The daemon identifies you automatically; `--as UID` works for root only.

```bash
# `--socket` is optional (default /run/cgroupd.sock); shown here for clarity.

# Ensure own user directory exists
cgctl --socket /run/cgroupd.sock ensure-user

# Create a session
cgctl --socket /run/cgroupd.sock create-session abc-123_X --pid 1234

# List own sessions and apps
cgctl --socket /run/cgroupd.sock list-sessions
cgctl --socket /run/cgroupd.sock list-apps

# Remove a session
cgctl --socket /run/cgroupd.sock remove-session abc-123_X

# Root acting on another user
sudo cgctl --socket /run/cgroupd.sock ensure-user --as 1000

# Run an app as a transient unit (daemon re-execs a helper that drops privileges and execs
# into app/<aid>/, cf. systemd's app.slice; sessions are logins)
cgctl --socket /run/cgroupd.sock spawn game1 \
  --env DISPLAY=:0 --stdout file:/tmp/game.log \
  --pre "/bin/setup-save" --post "/bin/upload-save" \
  --limit cpu.weight=500 -- /usr/bin/game --fullscreen

# Run in a specific login session (default: the caller's own)
cgctl --socket /run/cgroupd.sock spawn --session c2 game2 -- /usr/bin/game

# Per-user service outside any session (survives logout)
cgctl --socket /run/cgroupd.sock create-session --service pipewire --pid 1234

# Query / wait / stop (sessions by default, --app for apps)
cgctl --socket /run/cgroupd.sock state --app game1
cgctl --socket /run/cgroupd.sock wait --app game1 --timeout 30
cgctl --socket /run/cgroupd.sock ps --app game1
cgctl --socket /run/cgroupd.sock stop --app game1 --grace 2

# Write current pid into a cgroup (no daemon)
cgctl enter /sys/fs/cgroup/abc-123_X/session

# Remove an app / service instead of a session
cgctl --socket /run/cgroupd.sock remove-session --app game1

# Run attached in a fresh app cgroup, like systemd-run --scope:
# stays on this terminal, waits, propagates exit status
cgctl scope -- /usr/bin/game --fullscreen
cgctl scope --aid game1 -- bash
```

## Examples 

(`scripts/cgrun.py`)
Client-side launches like systemd dbus (`StartTransientUnit` for
`app-<id>-<pid>.scope`): the script double-forks locally so the app
inherits your full environment,
then registers the pid via D-Bus `CreateApp` into `app/<aid>/`.

```bash
scripts/cgrun.py -- /usr/bin/cosmic-term
scripts/cgrun.py --aid game1 -- /usr/bin/game --fullscreen
scripts/cgrun.py --exec "cosmic-term -e htop"  # desktop-Entry style
scripts/cgrun.py --wait -- /bin/sleep 5        # wait until it exits
CGROUPD_DBUS_BUS=session scripts/cgrun.py --bus session -- /bin/sleep 60
```

Attached mode, like `systemd-run --scope` (stays on this terminal,
waits, propagates exit status); the default above detaches like a GUI
launcher instead:

```bash
scripts/cgrun.py --scope --stdio inherit -- bash
scripts/cgrun.py --scope --stdio inherit --aid game1 -- /usr/bin/game --fullscreen
```

## Login sessions (PAM)

Add to `/etc/pam.d/system-login`, `/etc/pam.d/sshd`, etc.:

```
session optional pam_exec.so quiet /usr/bin/cgctl pam
```

`pam_exec` runs `cgctl pam` as root; the session leader is its parent pid.
The sid is `$XDG_SESSION_ID` if set, else `pam-<ppid>`. Pass `--socket PATH`
before `pam` to use a non-default socket.

Each login gets its own session cgroup; logout removes it (best-effort —
the daemon's reaper sweeps leftovers). Root (`uid 0`) logins are skipped.
Fail-open: a dead daemon logs a warning and lets the login proceed, never
locks users out.

## D-Bus

### THIS MAY BE REMOVED

System bus name `org.cgroupd`, object `/org/cgroupd/Manager`, interface
`org.cgroupd.Manager` — same operations as the socket, JSON responses.
Methods: `EnsureUser`, `ListSessions`, `ListApps`, `SpawnApp`,
`CreateSession`/`CreateApp`, `RemoveSession`/`RemoveApp`,
`StopSession`/`StopApp`, `WaitSession`/`WaitApp`, `GetState`/`GetAppState`.
(The socket uses one method per operation with a `scope` parameter instead.)
Policy: `etc/dbus-1/system.d/org.cgroupd.conf`. Build without it with
`--no-default-features`; at runtime use `--no-dbus` or `CGROUPD_NO_DBUS=1`.

Why removal? The original goal for this was to be a drop in replacement for systemd
however if we take the systemd dbus this may conflict with other reimplementations
as such I plan on eventually making a crate that will bind multiple smaller projects
together. As such dbus doesnt have any personal use to me. I may remove this in the 
future and go with a pure varlink interface in the future.

## Scripting the socket

Varlink JSON over the Unix socket; the contract is
`etc/org.cgroupd.Manager.varlink` (also served live via
`org.varlink.service`). Example:

```bash
python3 -c "
import json, socket
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect('/run/cgroupd.sock')
s.sendall(json.dumps({'method': 'org.cgroupd.Manager.ListSessions', 'parameters': {}}).encode() + b'\0')
buf = b''
while not buf.endswith(b'\0'): buf += s.recv(4096)
print(buf[:-1].decode())"
```

Omit `uid` to act as yourself; only root may name another user.


### Cgroup layout

libelogind (and so polkit and maybe more ...) maps a pid to its login session using the
**first path component** of `/proc/<pid>/cgroup`; nothing deeper and no other naming
works. So a login is a top-level cgroup named exactly like its session id:

```
/sys/fs/cgroup/
├── <sid>/                  # owned by the user; holds no processes itself
│   ├── session/            # the login leader and its descendants
│   └── app/<aid>/          # apps launched from this session (spawn / CreateApp)
├── user@<uid>/             # the user's own cgroup, NOT a session (the '@' makes libelogind
│   └── services/<name>/    #   report "no session"); per-user services, survive logout
└── ...                     # anything else (init's own cgroups) is left alone
```

User services (pipewire, the user's dinit instance, ...) are not part of any login, so they
will eventually live next to the sessions, not inside them: ending a session 
(`cgctl stop`/PAM close) never touches `user@<uid>/`. Place one with:

```bash
cgctl create-session --service pipewire --pid 1234    # -> user@1000/services/pipewire
cgctl remove-session --service pipewire
```

(libelogind rejects our own implementation of  systemd's nesting tree so the
user grouping cannot sit above the sessions; they are linked by name and owner only.)

- `<sid>` is the id the session tracker assigned: `$XDG_SESSION_ID`, else turnstile's
  `/run/turnstiled/sessions/<id>` whose `LEADER` is the login, else `pam-<ppid>`. login1
  must use the same id (polkit compares them).
- Apps belong to the session they were started in, so two sessions of one user are
  isolated: `CreateApp` takes the session from the pid's own cgroup, `spawn` from
  `--session` (default: the caller's session). App ids are unique per user.
- A login never adopts an existing top-level cgroup it does not own.
- Do not place other processes in non-session top-level cgroups: libelogind would
  report them as a session named after the cgroup.

## Logging

Targets `/dev/log` and `stderr` (both by default). Select with
`--log-target syslog|stderr|both` (or `CGROUPD_LOG` env). The daemon is
foreground-only (no daemonize/pidfile), must run as root, and takes a
singleton lock on `<socket_path>.lock`:

```bash
./target/debug/cgroupd [--config /etc/cgroupd.conf] [--socket PATH] [--no-dbus]
```

## Development

```bash
just check              # clippy (pedantic)
./scripts/test-all.sh   # fmt, clippy, build, smoke test; no root needed
./scripts/smoke.sh      # daemon + cgctl against a fake root (FAKE_CGROUP_ROOT)
```

`FAKE_CGROUP_ROOT=<dir>` runs the daemon unprivileged against a fake cgroup tree.
See `docs/unsafe.md`: all crates `#![forbid(unsafe_code)]`.

## Operational notes

- **Reaper**: event-driven. Each session / `CreateApp` unit has an inotify
  watch on its `cgroup.events`; when it empties the dir is removed at once
  (no timers, no idle wakeups). A sweep at startup adopts and cleans
  pre-existing units. Units with live
  processes are always kept. `spawn` apps are cleaned by the spawn engine.
- **Spawn caps**: max 32 active spawned apps per user, 256 globally;
  max 32 concurrent `wait` calls (excess gets `server busy`). Login
  sessions don't consume the spawn budget.
- **Logout cleanup is best-effort**; leftover sessions are reaped, not leaked.
- **Short RPC timeouts (2s)** between PAM and daemon prevent hung logins.

## License

GPL-2.0-or-later, see `LICENSE`.

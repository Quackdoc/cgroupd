#!/bin/sh
# Smoke test: daemon + cgctl against a fake cgroup root. No root needed.
# Usage: ./scripts/smoke.sh
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FAKE="$(mktemp -d)"
SOCK="$FAKE/cgroupd.sock"
trap 'kill $DAEMON 2>/dev/null || true; rm -rf "$FAKE"' EXIT INT TERM

printf 'socket_path = "%s"\nallow_root = true\nreaper_interval_secs = 60\n' "$SOCK" > "$FAKE/conf.toml"
export FAKE_CGROUP_ROOT="$FAKE"

"$ROOT/target/debug/cgroupd" --config "$FAKE/conf.toml" &
DAEMON=$!
sleep 0.7
kill -0 $DAEMON 2>/dev/null || { echo "FAIL: daemon died"; exit 1; };

MYUID="$(id -u)"
CGCTL="$ROOT/target/debug/cgctl"
pass=0; fail=0

check() { # check <desc> <expected-substring> <command...>
  desc="$1"; want="$2"; shift 2
  out="$("$@" 2>&1)" || { echo "FAIL: $desc (exit $?) :: $out"; fail=$((fail+1)); return; };
  case "$out" in
    *"$want"*) echo "ok: $desc"; pass=$((pass+1));;
    *) echo "FAIL: $desc :: got '$out', want '*$want*'"; fail=$((fail+1));;
  esac
}

expect_fail() { # expect_fail <desc> <command...>
  desc="$1"; shift
  if out="$("$@" 2>&1)"; then
    echo "FAIL: $desc (expected failure, got '$out')"; fail=$((fail+1))
  else
    echo "ok: $desc (failed as expected :: $out)"; pass=$((pass+1))
  fi
}

# Implicit UID: no uid arg; daemon resolves caller via SO_PEERCRED.
check "ensure-user (self)" "ensured" "$CGCTL" --socket "$SOCK" ensure-user
check "create-session" "testsess1" "$CGCTL" --socket "$SOCK" create-session testsess1
check "list-sessions" "testsess1" "$CGCTL" --socket "$SOCK" list-sessions
check "list-apps empty" "apps:" "$CGCTL" --socket "$SOCK" list-apps
check "create-app" "app app1 pid" "$CGCTL" --socket "$SOCK" create-session --app app1
check "list-apps one" "app1" "$CGCTL" --socket "$SOCK" list-apps

PROCS="$FAKE/sys/fs/cgroup/testsess1/session/cgroup.procs"
if [ -f "$PROCS" ] && grep -qE '^[0-9]+$' "$PROCS"; then
  echo "ok: cgroup.procs holds a pid ($(cat "$PROCS"))"; pass=$((pass+1))
else
  echo "FAIL: cgroup.procs missing or bad"; fail=$((fail+1))
fi

# Busy session must be kept.
echo 99999 > "$PROCS"
check "busy kept" "busy" "$CGCTL" --socket "$SOCK" remove-session testsess1
# Empty session removes.
printf '' > "$PROCS"
check "empty removed" "removed" "$CGCTL" --socket "$SOCK" remove-session testsess1

# Raw varlink interop: a generic client (no cgctl) speaking NUL-delimited
# JSON, two calls on one connection (multi-call per connection).
out="$(python3 - "$SOCK" <<'EOF'
import json, socket, sys
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect(sys.argv[1])
def call(method, parameters):
    s.sendall(json.dumps({"method": method, "parameters": parameters}).encode() + b"\0")
    buf = b""
    while not buf.endswith(b"\0"):
        chunk = s.recv(4096)
        if not chunk:
            break
        buf += chunk
    return json.loads(buf[:-1].decode())
print(json.dumps(call("org.cgroupd.Manager.ListSessions", {})))
print(json.dumps(call("org.varlink.service.GetInterfaceDescription", {"interface": "org.cgroupd.Manager"})))
EOF
)" || { echo "FAIL: raw varlink (exit $?) :: $out"; fail=$((fail+1)); }
case "$out" in
  *'"detail": "sessions'*'interface org.cgroupd.Manager'*)
    echo "ok: raw varlink multi-call + introspection"; pass=$((pass+1));;
  *) echo "FAIL: raw varlink :: $out"; fail=$((fail+1));;
esac

# --as other-uid must be denied for non-root callers.
expect_fail "cross-user --as denied" "$CGCTL" --socket "$SOCK" ensure-user --as 0
expect_fail "bad sid rejected" "$CGCTL" --socket "$SOCK" create-session '../evil'

# Spawn engine: fork /bin/sleep as a transient app, observe active, stop it.
check "spawn sleep" "spawned app" "$CGCTL" --socket "$SOCK" spawn smokesleep -- /bin/sleep 60
check "list-apps" "smokesleep" "$CGCTL" --socket "$SOCK" list-apps
check "state active" '"state":"active"' "$CGCTL" --socket "$SOCK" state --app smokesleep
# ps prints pids; ensure exit 0 with numeric output
out="$("$CGCTL" --socket "$SOCK" ps --app smokesleep 2>&1)" || { echo "FAIL: ps (exit $?) :: $out"; fail=$((fail+1)); }
case "$out" in
  *[!0-9[:space:]]*) echo "FAIL: ps non-numeric :: $out"; fail=$((fail+1));;
  *) echo "ok: ps numeric ($out)"; pass=$((pass+1));;
esac
expect_fail "wait times out" "$CGCTL" --socket "$SOCK" wait --app smokesleep --timeout 1
check "stop removes" "removed" "$CGCTL" --socket "$SOCK" stop --app smokesleep --grace 1

# Spawn with env + stdio + pre-hook; /bin/true exits immediately.
check "spawn env/stdio/hooks" "spawned app" "$CGCTL" --socket "$SOCK" spawn smokehook --env "SMOKE=1" --stdout "file:$FAKE/hook.log" --pre "/bin/true" --post "/bin/touch $FAKE/post-ok" -- /bin/true
sleep 1
# Either still present+inactive or already reaped are both success.
out="$("$CGCTL" --socket "$SOCK" state --app smokehook 2>&1)" || { echo "FAIL: hook state (exit $?) :: $out"; fail=$((fail+1)); }
case "$out" in
  *'"state":"inactive"'*|*'already gone'*) echo "ok: hook settled ($out)"; pass=$((pass+1));;
  *) echo "FAIL: hook state :: $out"; fail=$((fail+1));;
esac
if [ -f "$FAKE/post-ok" ]; then echo "ok: post-hook ran"; pass=$((pass+1)); else echo "FAIL: post-hook missing"; fail=$((fail+1)); fi

# Failure paths.
expect_fail "relative program" "$CGCTL" --socket "$SOCK" spawn badrel -- relative/prog
expect_fail "bad limit file" "$CGCTL" --socket "$SOCK" spawn badlim --limit 'cgroup.procs=x' -- /bin/true
expect_fail "fd stdio rejected" "$CGCTL" --socket "$SOCK" spawn badstdio --stdout 'fd:stdout' -- /bin/true

echo "----"
echo "pass=$pass fail=$fail"
[ "$fail" -eq 0 ]

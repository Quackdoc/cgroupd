/// UID; root (0) rejected unless `allow_root`.
pub fn validate_uid(uid: u32, allow_root: bool) -> Result<(), String> {
    if uid == 0 && !allow_root {
        return Err("refusing uid 0 (root)".to_string());
    }
    Ok(())
}

/// Resolve the effective target UID for a request.
///
/// `req` is the client-supplied `uid` field (`None` = self).
/// `peer` is the kernel/bus-authenticated caller.
///
/// - `peer == 0` (root daemon client / cgctl pam): `Some(target)` required.
/// - otherwise: `None` resolves to `peer`; `Some(u)` must equal `peer`.
///
/// The resolved UID is then passed through `validate_uid`.
/// The daemon must only ever use the return value, never `req` directly.
pub fn resolve_uid(req: Option<u32>, peer: u32, allow_root: bool) -> Result<u32, String> {
    let target = if peer == 0 {
        match req {
            Some(t) => t,
            None => return Err("root must specify a target uid".to_string()),
        }
    } else {
        match req {
            None => peer,
            Some(u) if u == peer => peer,
            Some(_) => return Err("permission denied (uid mismatch)".to_string()),
        }
    };
    validate_uid(target, allow_root)?;
    Ok(target)
}

/// Session id: `[A-Za-z0-9_-]{1,64}` (no `.`/`/`/empty, so no dir escape).
pub fn validate_sid(sid: &str) -> Result<(), String> {
    if sid.is_empty() {
        return Err("empty sid".to_string());
    }
    if sid.len() > 64 {
        return Err("sid too long (max 64)".to_string());
    }
    let ok = sid
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if !ok {
        return Err("sid must match [A-Za-z0-9_-]{1,64}".to_string());
    }
    Ok(())
}

fn has_nul(s: &str) -> bool {
    s.bytes().any(|b| b == 0)
}

fn no_nul(s: &str, what: &str) -> Result<(), String> {
    if has_nul(s) {
        return Err(format!("{what} contains NUL"));
    }
    Ok(())
}

/// Empty + NUL + absolute-path checks shared by program/cwd/stdio paths.
fn check_path(s: &str, what: &str) -> Result<(), String> {
    if s.is_empty() {
        return Err(format!("empty {what}"));
    }
    no_nul(s, what)?;
    if !s.starts_with('/') {
        return Err(format!("{what} must be an absolute path"));
    }
    Ok(())
}

/// Length (8KiB) + NUL checks shared by env/arg entries.
fn check_entry(s: &str, what: &str) -> Result<(), String> {
    if s.len() > 8192 {
        return Err(format!("{what} too long (max 8KiB)"));
    }
    no_nul(s, what)
}

/// Executable path: absolute, non-empty, NUL-free. No `PATH` search.
pub fn validate_program(program: &str) -> Result<(), String> {
    check_path(program, "program")
}

fn valid_env_key(key: &str) -> bool {
    let mut bytes = key.bytes();
    match bytes.next() {
        Some(b) if b.is_ascii_alphabetic() || b == b'_' => {}
        _ => return false,
    }
    bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Environment array (`KEY=VAL`, max 256 entries of 8KiB).
pub fn validate_env(env: &[String]) -> Result<(), String> {
    if env.len() > 256 {
        return Err("too many env entries (max 256)".to_string());
    }
    for e in env {
        check_entry(e, "env entry")?;
        let Some(eq) = e.find('=') else {
            return Err(format!("env entry missing '=': {e}"));
        };
        if !valid_env_key(&e[..eq]) {
            return Err(format!("bad env key: {}", &e[..eq]));
        }
    }
    Ok(())
}

/// Argv: NUL-free, max 256 entries of 8KiB.
pub fn validate_args(args: &[String]) -> Result<(), String> {
    if args.len() > 256 {
        return Err("too many args (max 256)".to_string());
    }
    for a in args {
        check_entry(a, "arg")?;
    }
    Ok(())
}

/// Working directory: absolute, NUL-free.
pub fn validate_cwd(cwd: &Option<String>) -> Result<(), String> {
    if let Some(d) = cwd {
        check_path(d, "cwd")?;
    }
    Ok(())
}

/// Umask: 0..=0o777.
pub fn validate_umask(umask: Option<u32>) -> Result<(), String> {
    if let Some(m) = umask
        && m > 0o777
    {
        return Err("umask out of range (max 0777)".to_string());
    }
    Ok(())
}

fn validate_stdio_path(path: &str) -> Result<(), String> {
    check_path(path, "stdio path")
}

pub fn validate_stdio(spec: &crate::proto::StdioSpec) -> Result<(), String> {
    use crate::proto::StdioSpec as S;
    match spec {
        S::Null | S::Inherit => Ok(()),
        S::File(p) | S::Append(p) | S::Truncate(p) => validate_stdio_path(p),
    }
}

/// Parse `null|inherit|file:/p|append:/p|truncate:/p` (`fd:` rejected).
pub fn parse_stdio_spec(s: &str) -> Result<crate::proto::StdioSpec, String> {
    use crate::proto::StdioSpec as S;
    match s {
        "null" => Ok(S::Null),
        "inherit" => Ok(S::Inherit),
        _ => {
            for (prefix, mk) in [
                ("file:", S::File as fn(String) -> S),
                ("append:", S::Append as fn(String) -> S),
                ("truncate:", S::Truncate as fn(String) -> S),
            ] {
                if let Some(path) = s.strip_prefix(prefix) {
                    validate_stdio_path(path)?;
                    return Ok(mk(path.to_string()));
                }
            }
            Err(format!(
                "bad stdio spec (want null|inherit|file:/p|append:/p|truncate:/p): {s}"
            ))
        }
    }
}

/// Limit file name: bare `<controller>.<name>`, no path, no `cgroup.*`.
/// Controller liveness is checked at spawn time against `cgroup.controllers`.
pub fn validate_limit_name(file: &str) -> Result<(), String> {
    if file.is_empty() {
        return Err("empty limit file".to_string());
    }
    if file.len() > 128 {
        return Err("limit file too long".to_string());
    }
    no_nul(file, "limit file")?;
    let ok = file
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-');
    if !ok {
        return Err("limit file must match [A-Za-z0-9_.-]+ (no slashes)".to_string());
    }
    if file.starts_with("cgroup.") {
        return Err("cgroup.* internals are not writable via limits".to_string());
    }
    if !file.contains('.') {
        return Err("limit file must be <controller>.<name>".to_string());
    }
    Ok(())
}

/// Full limit entry: name statically, value size only.
pub fn validate_limit(limit: &crate::proto::Limit) -> Result<(), String> {
    validate_limit_name(&limit.file)?;
    if limit.value.len() > 4096 {
        return Err("limit value too long (max 4KiB)".to_string());
    }
    no_nul(&limit.value, "limit value")
}

/// Controller prefix (`cpu` in `cpu.weight`).
pub fn limit_controller(file: &str) -> &str {
    match file.find('.') {
        Some(i) => &file[..i],
        None => file,
    }
}

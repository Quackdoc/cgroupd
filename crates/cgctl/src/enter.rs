/// Join a cgroup by writing our pid to its `cgroup.procs`.
pub(crate) fn run_enter(path: &str) {
    // Must never run setuid: it writes the caller's pid on kernel auth.
    if is_setuid_or_setgid() {
        eprintln!("refusing setuid/setgid cgctl enter (must run unprivileged)");
        std::process::exit(1);
    }
    let target = std::path::Path::new(path).join("cgroup.procs");
    let pid = std::process::id();
    if let Err(e) = std::fs::write(&target, format!("{pid}\n")) {
        eprintln!("write {}: {e}", target.display());
        std::process::exit(1);
    }
}

/// Setuid/setgid check via real vs effective uid/gid in `/proc/self/status`.
fn is_setuid_or_setgid() -> bool {
    let Ok(content) = std::fs::read_to_string("/proc/self/status") else {
        return false;
    };
    let mut ruid = None;
    let mut euid = None;
    let mut rgid = None;
    let mut egid = None;
    for line in content.lines() {
        if let Some(v) = line.strip_prefix("Uid:") {
            let nums: Vec<u32> = v
                .split_whitespace()
                .filter_map(|t| t.parse().ok())
                .collect();
            if nums.len() >= 2 {
                ruid = Some(nums[0]);
                euid = Some(nums[1]);
            }
        } else if let Some(v) = line.strip_prefix("Gid:") {
            let nums: Vec<u32> = v
                .split_whitespace()
                .filter_map(|t| t.parse().ok())
                .collect();
            if nums.len() >= 2 {
                rgid = Some(nums[0]);
                egid = Some(nums[1]);
            }
        }
    }
    ruid != euid || rgid != egid
}

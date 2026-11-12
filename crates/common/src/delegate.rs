use std::path::Path;

pub const DELEGATE_KERNEL_FILE: &str = "/sys/kernel/cgroup/delegate";

const FALLBACK: &[&str] = &[
    "cgroup.procs",
    "cgroup.subtree_control",
    "cgroup.threads",
    "memory.oom.group",
    "memory.reclaim",
];

/// Delegation breaks without these; the rest are best-effort.
pub const FATAL_DELEGATE_FILES: &[&str] = &["cgroup.procs", "cgroup.subtree_control"];

pub fn is_fatal_delegate(name: &str) -> bool {
    FATAL_DELEGATE_FILES.contains(&name)
}

/// Cgroup files to chown-delegate; kernel list plus mandatory files.
pub fn delegate_files() -> &'static [String] {
    static FILES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    FILES.get_or_init(|| delegate_files_from(Path::new(DELEGATE_KERNEL_FILE)))
}

pub fn delegate_files_from(path: &Path) -> Vec<String> {
    match std::fs::read_to_string(path) {
        Ok(content) => {
            let mut out: Vec<String> = content
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect();
            for required in ["cgroup.procs", "cgroup.subtree_control"] {
                if !out.iter().any(|f| f == required) {
                    out.push(required.to_string());
                }
            }
            out
        }
        Err(_) => FALLBACK.iter().map(|s| s.to_string()).collect(),
    }
}

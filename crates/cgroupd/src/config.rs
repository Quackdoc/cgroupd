use serde::Deserialize;

/// Daemon settings (`/etc/cgroupd.conf`). Unknown keys are ignored.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub controllers: Vec<String>,
    pub socket_path: String,
    pub allow_root: bool,
    pub allow_spawn: bool,
    /// Write the root `cgroup.subtree_control` (delegates controllers to sessions).
    pub manage_ancestors: bool,
    pub default_grace_secs: u64,
    pub spawn_start_timeout_secs: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            controllers: ["cpu", "cpuset", "memory", "pids", "io"]
                .map(String::from)
                .to_vec(),
            socket_path: cgroupd_common::DEFAULT_SOCKET_PATH.to_string(),
            allow_root: false,
            allow_spawn: true,
            manage_ancestors: false,
            default_grace_secs: 2,
            spawn_start_timeout_secs: 90,
        }
    }
}

impl Config {
    pub fn load(path: &str) -> Result<Self, String> {
        let cfg = match std::fs::read_to_string(path) {
            Ok(content) => basic_toml::from_str::<Config>(&content)
                .map_err(|e| format!("parse {path}: {e}"))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Config::default(),
            Err(e) => return Err(format!("read {path}: {e}")),
        };
        cfg.validate()?;
        Ok(cfg)
    }

    /// Rejects a bad socket path and out-of-range timeouts.
    /// `FAKE_CGROUP_ROOT` relaxes the socket path prefix check for tests.
    pub fn validate(&self) -> Result<(), String> {
        let fake_test = std::env::var("FAKE_CGROUP_ROOT")
            .ok()
            .filter(|s| !s.is_empty())
            .is_some();
        let p = &self.socket_path;
        if p.is_empty() || p.len() > 4096 {
            return Err("socket_path: bad length".to_string());
        }
        if p.bytes().any(|b| b == 0) {
            return Err("socket_path: contains NUL".to_string());
        }
        if !p.starts_with('/') {
            return Err(format!("socket_path must be absolute: {p}"));
        }
        if p == "/" {
            return Err("socket_path: refusing /".to_string());
        }
        if !fake_test && !(p.starts_with("/run/") || p.starts_with("/var/run/")) {
            return Err(format!("socket_path must be under /run/ or /var/run/: {p}"));
        }
        for c in &self.controllers {
            if c.is_empty() || c.len() > 32 {
                return Err(format!("bad controller {c:?}"));
            }
            if !c
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            {
                return Err(format!("bad controller {c:?} (want [a-z0-9_]+)"));
            }
        }
        if self.default_grace_secs > 300 {
            return Err("default_grace_secs must be <= 300".to_string());
        }
        if self.spawn_start_timeout_secs < 5 || self.spawn_start_timeout_secs > 600 {
            return Err("spawn_start_timeout_secs must be 5..=600".to_string());
        }
        Ok(())
    }
}

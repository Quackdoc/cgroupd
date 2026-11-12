//! Applying `--limit FILE=VALUE` to a unit's cgroup files.

use std::path::Path;

use cgroupd_common::validate_limit;
use cgroupd_common::{Limit, limit_controller};

pub(super) fn live_controllers(dir: &Path) -> Option<Vec<String>> {
    let content = std::fs::read_to_string(dir.join("cgroup.controllers")).ok()?;
    Some(content.split_whitespace().map(str::to_string).collect())
}

pub(super) fn open_nofollow(
    dir: &Path,
    file: &str,
    create: bool,
    mode: u32,
) -> Result<std::fs::File, String> {
    use std::os::unix::fs::OpenOptionsExt;
    let full = dir.join(file);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true)
        .create(create)
        .append(create)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .mode(mode);
    opts.open(&full)
        .map_err(|e| format!("open {}: {e}", full.display()))
}

/// Write `limits` into the unit's cgroup files. A controller its parent
/// (`app/`) doesn't have enabled is skipped with a warning.
pub(super) fn apply_limits(dir: &Path, limits: &[Limit]) -> Result<(), String> {
    if limits.is_empty() {
        return Ok(());
    }
    if limits.len() > 64 {
        return Err("too many limits (max 64)".to_string());
    }
    let user_dir = dir.parent().unwrap_or(dir);
    let live = live_controllers(user_dir);
    for lim in limits {
        validate_limit(lim)?;
        let ctl = limit_controller(&lim.file);
        if let Some(ref live) = live
            && !live.iter().any(|c| c == ctl)
        {
            crate::log::warning(&format!(
                "limit {} skipped: controller {ctl} not live in {}",
                lim.file,
                user_dir.display()
            ));
            continue;
        }
        // Unit dirs are user-chowned; refuse non-regular files so a
        // planted FIFO can't hang the open (O_NONBLOCK also fails fast).
        match std::fs::symlink_metadata(dir.join(&lim.file)) {
            Ok(meta) => {
                let ft = meta.file_type();
                if ft.is_symlink() {
                    crate::log::warning(&format!("limit {} skipped: symlink", lim.file));
                    continue;
                }
                if !ft.is_file() {
                    crate::log::warning(&format!("limit {} skipped: not a regular file", lim.file));
                    continue;
                }
            }
            Err(_) => {
                // Missing file fails closed in `open_nofollow` below.
            }
        }
        match open_nofollow(dir, &lim.file, false, 0o644) {
            Ok(mut f) => {
                use std::io::Write;
                let r = f
                    .write_all(lim.value.as_bytes())
                    .and_then(|()| f.write_all(b"\n"))
                    .map_err(|e| format!("write: {e}"));
                if let Err(e) = r {
                    crate::log::warning(&format!("limit {} not applied: {e}", lim.file));
                } else {
                    crate::log::info(&format!(
                        "limit {}={} {}",
                        lim.file,
                        lim.value,
                        dir.display()
                    ));
                }
            }
            Err(e) => {
                crate::log::warning(&format!("limit {} skipped: {e}", lim.file));
            }
        }
    }
    Ok(())
}

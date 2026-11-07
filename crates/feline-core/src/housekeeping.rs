// SPDX-License-Identifier: GPL-3.0-or-later
//! Privacy and storage actions for the launcher's Settings page: deleting
//! logs (Roblox's own and Feline's), forgetting the saved Roblox login of
//! Feline's runtime, clearing Roblox's content cache, and how much space
//! each uses. Everything here only touches Feline's own folders.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Feline's runtime data: `~/.local/share/feline/runtime`.
fn runtime_dir() -> Option<PathBuf> {
    Some(crate::paths::data_home()?.join("feline").join("runtime"))
}

/// Where Roblox (in Feline's runtime) writes its own logs.
pub fn roblox_log_dir() -> Option<PathBuf> {
    Some(runtime_dir()?.join("files/appData/logs"))
}

/// Roblox's content cache (game models, textures, sounds).
pub fn roblox_cache_dir() -> Option<PathBuf> {
    Some(runtime_dir()?.join("cache"))
}

/// The saved Roblox login of Feline's runtime.
pub fn saved_login() -> Option<PathBuf> {
    Some(runtime_dir()?.join("roblox-cookie"))
}

/// Downloaded Roblox versions (see `crate::engine`).
pub fn roblox_versions_dir() -> Option<PathBuf> {
    Some(crate::engine::engine_dir()?.join("versions"))
}

/// Bytes used by everything under `path` (0 if it doesn't exist).
pub fn size(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if !meta.is_dir() {
        return meta.len();
    }
    std::fs::read_dir(path)
        .map(|entries| entries.filter_map(Result::ok).map(|e| size(&e.path())).sum())
        .unwrap_or(0)
}

/// "702 MB", "8.5 MB", "12 KB".
pub fn human(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= 1024.0 * MB {
        format!("{:.1} GB", b / (1024.0 * MB))
    } else if b >= 10.0 * MB {
        format!("{:.0} MB", b / MB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else {
        format!("{:.0} KB", (b / 1024.0).ceil())
    }
}

/// Deletes the files directly in `dir` (not folders). Files modified in the
/// last `keep` are left alone. The number deleted.
fn delete_files(dir: &Path, keep: Duration) -> usize {
    let now = SystemTime::now();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter(|e| {
            e.metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|t| now.duration_since(t).unwrap_or_default() >= keep)
        })
        .filter(|e| std::fs::remove_file(e.path()).is_ok())
        .count()
}

/// Deletes Roblox's own logs. The number of files deleted.
pub fn delete_roblox_logs() -> usize {
    roblox_log_dir().map_or(0, |d| delete_files(&d, Duration::ZERO))
}

/// Deletes Feline's logs (the runtime's and the launcher's), except the
/// last minute's (a game may be writing one). The number deleted.
pub fn delete_feline_logs() -> usize {
    crate::paths::log_dir().map_or(0, |d| delete_files(&d, Duration::from_secs(60)))
}

/// Deletes both kinds of logs older than `days` days. The number deleted.
pub fn delete_old_logs(days: u32) -> usize {
    if days == 0 {
        return 0;
    }
    let keep = Duration::from_secs(u64::from(days) * 24 * 3600);
    roblox_log_dir().map_or(0, |d| delete_files(&d, keep))
        + crate::paths::log_dir().map_or(0, |d| delete_files(&d, keep))
}

/// Forgets the saved Roblox login of Feline's runtime (the next start asks
/// to sign in again). True if there was one.
pub fn forget_login() -> bool {
    saved_login().is_some_and(|p| std::fs::remove_file(p).is_ok())
}

/// Clears Roblox's content cache. Games load a bit slower the first time
/// after. True if it was cleared (or already empty).
pub fn clear_roblox_cache() -> bool {
    match roblox_cache_dir() {
        Some(dir) if dir.exists() => std::fs::remove_dir_all(&dir).is_ok(),
        Some(_) => true,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_sizes() {
        assert_eq!(human(12 * 1024), "12 KB");
        assert_eq!(human(8_912_896), "8.5 MB");
        assert_eq!(human(736_100_000), "702 MB");
        assert_eq!(human(2_400_000_000), "2.2 GB");
    }

    #[test]
    fn deletes_only_old_files() {
        let dir = std::env::temp_dir().join(format!("feline-test-housekeeping-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("new.log"), "x").unwrap();
        std::fs::write(dir.join("sub/kept.log"), "x").unwrap();
        assert_eq!(delete_files(&dir, Duration::from_secs(3600)), 0);
        assert_eq!(delete_files(&dir, Duration::ZERO), 1);
        assert!(dir.join("sub/kept.log").exists(), "folders are left alone");
        assert_eq!(size(&dir), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

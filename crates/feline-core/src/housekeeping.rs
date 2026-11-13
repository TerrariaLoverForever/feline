// SPDX-License-Identifier: GPL-3.0-or-later
//! Privacy and storage actions for the launcher's Settings page: deleting
//! logs (Roblox's own and Feline's), forgetting the saved Roblox login of
//! Feline's runtime, clearing Roblox's content cache, and how much space
//! each uses. On Linux, everything here only touches Feline's own folders.
//!
//! On Windows, the same actions cover what Roblox's own installer leaves
//! behind in `%LOCALAPPDATA%\Roblox`: its logs, its download cache, and old
//! version folders it didn't clean up (see [`leftover_versions`]).

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Feline's runtime data: `~/.local/share/feline/runtime`.
fn runtime_dir() -> Option<PathBuf> {
    Some(crate::paths::data_home()?.join("feline").join("runtime"))
}

/// `%LOCALAPPDATA%\Roblox` (Windows).
#[cfg_attr(not(windows), allow(dead_code))]
fn windows_roblox_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .map(|p| p.join("Roblox"))
}

/// Where Roblox writes its own logs (Feline's runtime; on Windows, Roblox's
/// official client).
pub fn roblox_log_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    return Some(windows_roblox_dir()?.join("logs"));
    #[cfg(not(windows))]
    return Some(runtime_dir()?.join("files/appData/logs"));
}

/// Roblox's content cache (game models, textures, sounds). On Windows,
/// Roblox's installer download cache (`Downloads`), which it fills again only
/// when it updates.
pub fn roblox_cache_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    return Some(windows_roblox_dir()?.join("Downloads"));
    #[cfg(not(windows))]
    return Some(runtime_dir()?.join("cache"));
}

/// Windows: old `Versions\version-*` folders with neither Roblox's player nor
/// Studio in them, untouched for a day. Roblox normally deletes these after
/// an update; sometimes one stays behind. A folder Roblox is installing into
/// right now has no program yet either, which is why recent ones are kept.
pub fn leftover_versions() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        let Some(dir) = windows_roblox_dir().map(|d| d.join("Versions")) else {
            return Vec::new();
        };
        leftovers_in(&dir, Duration::from_secs(24 * 3600))
    }
    #[cfg(not(windows))]
    Vec::new()
}

#[cfg_attr(not(windows), allow(dead_code))]
fn leftovers_in(versions: &Path, min_age: Duration) -> Vec<PathBuf> {
    let now = SystemTime::now();
    let Ok(entries) = std::fs::read_dir(versions) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with("version-"))
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .filter(|dir| {
            !["RobloxPlayerBeta.exe", "RobloxStudioBeta.exe"]
                .iter()
                .any(|exe| dir.join(exe).exists())
        })
        .filter(|dir| {
            dir.metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|t| now.duration_since(t).unwrap_or_default() >= min_age)
        })
        .collect()
}

/// Programs that mean Roblox is running or installing: nothing in its
/// folders may be deleted then.
pub const ROBLOX_PROGRAMS: &[&str] = &[
    "RobloxPlayerBeta.exe",
    "RobloxPlayerInstaller.exe",
    "RobloxStudioBeta.exe",
    "RobloxStudioInstaller.exe",
    "RobloxCrashHandler.exe",
];

/// Deletes [`leftover_versions`], unless Roblox is running or installing.
/// The bytes freed.
pub fn delete_leftover_versions() -> Result<u64, String> {
    if crate::runtime::config::any_process_named(ROBLOX_PROGRAMS) {
        return Err("Roblox is running or updating. Close it first.".to_owned());
    }
    let mut freed = 0;
    for dir in leftover_versions() {
        let bytes = size(&dir);
        if std::fs::remove_dir_all(&dir).is_ok() {
            freed += bytes;
        }
    }
    Ok(freed)
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
    // Signing out also drops that account from the saved ones: otherwise
    // one click would sign it back in.
    let had = saved_login().is_some_and(|p| p.exists());
    crate::accounts::remove_active();
    saved_login().is_some_and(|p| std::fs::remove_file(p).is_ok()) || had
}

/// Clears Roblox's content cache. Games load a bit slower the first time
/// after. True if it was cleared (or already empty).
pub fn clear_roblox_cache() -> bool {
    if cfg!(windows) && crate::runtime::config::any_process_named(ROBLOX_PROGRAMS) {
        return false;
    }
    match roblox_cache_dir() {
        Some(dir) if dir.exists() => {
            let ok = std::fs::remove_dir_all(&dir).is_ok();
            // Keep the (now empty) folder where Roblox expects it.
            let _ = std::fs::create_dir_all(&dir);
            ok
        }
        Some(_) => true,
        None => false,
    }
}

// ---------------------------------------------------------------------------
// Clear Roblox: a fresh start
// ---------------------------------------------------------------------------

/// Roblox Studio's own things in `%LOCALAPPDATA%\Roblox` (kept unless Studio
/// is cleared too).
#[cfg_attr(not(windows), allow(dead_code))]
const STUDIO_ITEMS: &[&str] = &[
    "RobloxStudio",
    "RobloxStudioInstaller",
    "OTAPatchBackups",
    "OTAPlugins",
    "placeIDEState",
];

#[cfg_attr(not(windows), allow(dead_code))]
fn is_studio(name: &str) -> bool {
    STUDIO_ITEMS.contains(&name) || name.contains("_Studio")
}

/// What "Clear Roblox" would delete: everything Roblox keeps for the user
/// (login, caches, settings, logs, app data), but never the installed
/// Roblox itself (`Versions`). Studio's things only with `studio`.
pub fn roblox_data(studio: bool) -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        let mut out = Vec::new();
        if let Some(dir) = windows_roblox_dir()
            && let Ok(entries) = std::fs::read_dir(&dir)
        {
            for entry in entries.filter_map(Result::ok) {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.eq_ignore_ascii_case("Versions") || (!studio && is_studio(&name)) {
                    continue;
                }
                out.push(entry.path());
            }
        }
        // Roblox's web cache in the temporary folder.
        if let Some(temp) = std::env::var_os("TEMP").map(PathBuf::from) {
            let web = temp.join("Roblox");
            if web.exists() {
                out.push(web);
            }
        }
        out
    }
    #[cfg(not(windows))]
    {
        // Feline's own runtime: everything but the unpacked Roblox app.
        let _ = studio;
        let Some(dir) = runtime_dir() else { return Vec::new() };
        std::fs::read_dir(&dir)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|e| e.file_name() != "assets")
                    .map(|e| e.path())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// What "Clear Roblox" did.
#[derive(Debug, Default)]
pub struct Cleared {
    pub freed: u64,
    /// Things that couldn't be deleted (in use, permissions).
    pub kept: Vec<String>,
    /// Studio logins removed from Windows' Credential Manager.
    pub logins: usize,
}

/// Deletes everything [`roblox_data`] lists: Roblox starts like on a new
/// computer (signed out, default settings, empty caches), still installed.
/// Refuses while Roblox (or Studio, or their installers) runs.
pub fn clear_roblox(studio: bool) -> Result<Cleared, String> {
    let running: &[&str] = if cfg!(windows) { ROBLOX_PROGRAMS } else { &["feline-runtime"] };
    if crate::runtime::config::any_process_named(running) {
        return Err("Roblox (or Roblox Studio) is running. Close it first.".to_owned());
    }
    let mut result = Cleared::default();
    for path in roblox_data(studio) {
        let bytes = size(&path);
        let removed = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        match removed {
            Ok(()) => result.freed += bytes,
            Err(e) => result.kept.push(format!(
                "{} ({e})",
                path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
            )),
        }
    }
    #[cfg(windows)]
    if studio {
        result.logins = feline_win::forget_credentials("https://www.roblox.com:RobloxStudioAuth");
    }
    Ok(result)
}

// ---------------------------------------------------------------------------
// Shader caches
// ---------------------------------------------------------------------------

/// The graphics driver's shader caches: compiled shaders every game
/// (Roblox too) keeps. A broken one after a driver update causes stutter or
/// crashes; clearing it is safe, they're rebuilt while playing.
pub fn shader_cache_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    #[cfg(windows)]
    if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        for sub in [
            r"NVIDIA\DXCache",
            r"NVIDIA\GLCache",
            r"AMD\DxCache",
            r"AMD\DxcCache",
            r"AMD\VkCache",
            r"AMD\GLCache",
            r"Intel\ShaderCache",
            "D3DSCache",
        ] {
            out.push(local.join(sub));
        }
        if let Some(parent) = local.parent() {
            out.push(parent.join(r"LocalLow\NVIDIA\PerDriverVersion\DXCache"));
            out.push(parent.join(r"LocalLow\NVIDIA\PerDriverVersion\GLCache"));
        }
    }
    #[cfg(not(windows))]
    {
        let cache = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")));
        if let Some(cache) = cache {
            for sub in ["mesa_shader_cache", "mesa_shader_cache_db", "nvidia/GLCache", "radv_builtin_shaders64"] {
                out.push(cache.join(sub));
            }
        }
    }
    out.retain(|d| d.is_dir());
    out
}

/// Deletes the files under `dir`, then its emptied subfolders (not `dir`).
/// Files in use are skipped. (bytes freed, files skipped)
fn empty_dir(dir: &Path) -> (u64, usize) {
    let (mut freed, mut skipped) = (0, 0);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (0, 0);
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        match entry.file_type() {
            Ok(t) if t.is_dir() => {
                let (f, s) = empty_dir(&path);
                freed += f;
                skipped += s;
                let _ = std::fs::remove_dir(&path);
            }
            Ok(_) => {
                let bytes = entry.metadata().map_or(0, |m| m.len());
                if std::fs::remove_file(&path).is_ok() {
                    freed += bytes;
                } else {
                    skipped += 1;
                }
            }
            Err(_) => skipped += 1,
        }
    }
    (freed, skipped)
}

/// Clears [`shader_cache_dirs`] while Roblox is closed. Files another
/// program still uses stay. (bytes freed, files kept)
pub fn clear_shader_cache() -> Result<(u64, usize), String> {
    let running: &[&str] = if cfg!(windows) { ROBLOX_PROGRAMS } else { &["feline-runtime", "sober"] };
    if crate::runtime::config::any_process_named(running) {
        return Err("Roblox is running. Close it first.".to_owned());
    }
    Ok(shader_cache_dirs().iter().map(|d| empty_dir(d)).fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1)))
}

// ---------------------------------------------------------------------------
// Tidy up on a schedule
// ---------------------------------------------------------------------------

fn tidy_file() -> Option<PathBuf> {
    Some(crate::paths::state_dir()?.join("tidy.json"))
}

/// When Feline last tidied up on its own, and how much it freed:
/// (unix time, bytes).
pub fn last_tidy() -> Option<(u64, u64)> {
    let text = std::fs::read_to_string(tidy_file()?).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    Some((v["when"].as_u64()?, v["freed"].as_u64().unwrap_or(0)))
}

/// Whether a tidy-up every `days` days is due.
pub fn tidy_due(days: u32) -> bool {
    days > 0
        && last_tidy().is_none_or(|(when, _)| {
            crate::history::now().saturating_sub(when) >= u64::from(days) * 86_400
        })
}

/// The scheduled tidy-up: what's safe to delete without asking. Old logs
/// (a week), and on Windows Roblox's download cache and old versions; on
/// Linux the content cache only once it's over 2 GB. Skipped while Roblox
/// runs. Bytes freed.
pub fn tidy() -> Result<u64, String> {
    let running: &[&str] = if cfg!(windows) { ROBLOX_PROGRAMS } else { &["feline-runtime"] };
    if crate::runtime::config::any_process_named(running) {
        return Err("Roblox is running; Feline will tidy up another time.".to_owned());
    }
    let measure = |d: Option<PathBuf>| d.map_or(0, |d| size(&d));
    let before = measure(roblox_log_dir()) + measure(crate::paths::log_dir());
    delete_old_logs(7);
    let mut freed = before.saturating_sub(measure(roblox_log_dir()) + measure(crate::paths::log_dir()));
    let cache = measure(roblox_cache_dir());
    if (cfg!(windows) || cache > 2 * 1024 * 1024 * 1024) && cache > 0 && clear_roblox_cache() {
        freed += cache;
    }
    if cfg!(windows) {
        freed += delete_leftover_versions().unwrap_or(0);
    }
    if let Some(file) = tidy_file() {
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(
            file,
            serde_json::json!({ "when": crate::history::now(), "freed": freed }).to_string(),
        );
    }
    Ok(freed)
}

// ---------------------------------------------------------------------------
// Clear on exit
// ---------------------------------------------------------------------------

/// What "clear on exit" did: lines like "12 Roblox logs".
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ExitClear {
    pub done: Vec<String>,
    /// Why something was skipped (Roblox still running).
    pub skipped: Vec<String>,
}

/// Feline is closing: deletes what `choice` asks for. The play history is
/// brought up to date from the logs under `log_roots` first, so deleting
/// Roblox's logs doesn't lose any playtime (unless the history goes too).
/// Nothing Roblox is using right now is touched.
pub fn clear_on_exit(
    choice: crate::settings::ClearOnExit,
    log_roots: &[PathBuf],
) -> ExitClear {
    let mut result = ExitClear::default();
    if !choice.any() {
        return result;
    }
    let running: &[&str] = if cfg!(windows) { ROBLOX_PROGRAMS } else { &["feline-runtime"] };
    let roblox_running = crate::runtime::config::any_process_named(running);
    let mut history = crate::history::History::load();
    let scanned = history.scan(log_roots);
    if choice.history {
        history.clear_sessions();
        result.done.push("play history".to_owned());
    }
    if scanned || choice.history {
        let _ = history.save();
    }
    if choice.roblox_logs {
        if roblox_running {
            result.skipped.push("Roblox's logs (Roblox is running)".to_owned());
        } else {
            result.done.push(format!("{} Roblox logs", delete_roblox_logs()));
        }
    }
    if choice.feline_logs {
        result.done.push(format!("{} Feline logs", delete_feline_logs()));
    }
    if choice.roblox_cache {
        if roblox_running {
            result.skipped.push("Roblox's cache (Roblox is running)".to_owned());
        } else if clear_roblox_cache() {
            result.done.push("Roblox's cache".to_owned());
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Space history
// ---------------------------------------------------------------------------

/// Everything Roblox takes on this computer: its data and (on Windows) its
/// installed versions; on Linux Feline's runtime and its Roblox versions.
pub fn roblox_total_size() -> u64 {
    #[cfg(windows)]
    {
        roblox_data(true).iter().map(|p| size(p)).sum::<u64>()
            + windows_roblox_dir().map_or(0, |d| size(&d.join("Versions")))
    }
    #[cfg(not(windows))]
    {
        runtime_dir().map_or(0, |d| size(&d)) + roblox_versions_dir().map_or(0, |d| size(&d))
    }
}

fn space_file() -> Option<PathBuf> {
    Some(crate::paths::state_dir()?.join("space-history.json"))
}

/// The days kept in the space history.
const SPACE_DAYS: usize = 60;

/// Space used per day: (day number since 1970, local time; bytes), oldest
/// first.
pub fn space_history() -> Vec<(i64, u64)> {
    let Some(text) = space_file().and_then(|f| std::fs::read_to_string(f).ok()) else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<(i64, u64)>>(&text).unwrap_or_default()
}

/// Adds today's size (replacing an earlier one from today). The history.
pub fn record_space(bytes: u64) -> Vec<(i64, u64)> {
    let today = (crate::history::now() as i64 + crate::history::local_offset_seconds()) / 86_400;
    let mut history = space_history();
    add_day(&mut history, today, bytes);
    if let Some(file) = space_file() {
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string(&history) {
            let _ = std::fs::write(file, text);
        }
    }
    history
}

fn add_day(history: &mut Vec<(i64, u64)>, day: i64, bytes: u64) {
    history.retain(|(d, _)| *d != day);
    history.push((day, bytes));
    history.sort_unstable();
    if history.len() > SPACE_DAYS {
        history.drain(..history.len() - SPACE_DAYS);
    }
}

#[cfg(test)]
mod leftover_tests {
    use super::*;

    #[test]
    fn only_old_empty_version_folders_are_leftovers() {
        let dir = std::env::temp_dir().join(format!("feline-test-leftovers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for name in ["version-player", "version-studio", "version-old", "version-new"] {
            std::fs::create_dir_all(dir.join(name)).unwrap();
        }
        std::fs::write(dir.join("version-player/RobloxPlayerBeta.exe"), "x").unwrap();
        std::fs::write(dir.join("version-studio/RobloxStudioBeta.exe"), "x").unwrap();
        std::fs::write(dir.join("RobloxStudioInstaller.exe"), "x").unwrap();
        // With no minimum age, both program-less folders count.
        let mut found = leftovers_in(&dir, Duration::ZERO);
        found.sort();
        assert_eq!(found, [dir.join("version-new"), dir.join("version-old")]);
        // Fresh folders (an update in progress) are kept.
        assert!(leftovers_in(&dir, Duration::from_secs(3600)).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_space_history_keeps_one_value_a_day() {
        let mut h = Vec::new();
        add_day(&mut h, 10, 5);
        add_day(&mut h, 10, 7);
        add_day(&mut h, 9, 1);
        assert_eq!(h, [(9, 1), (10, 7)]);
        for day in 0..100 {
            add_day(&mut h, 100 + day, 1);
        }
        assert_eq!(h.len(), SPACE_DAYS);
        assert_eq!(h.last(), Some(&(199, 1)));
    }

    #[test]
    fn emptying_keeps_the_folder() {
        let dir = std::env::temp_dir().join(format!("feline-empty-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("a/b")).unwrap();
        std::fs::write(dir.join("a/b/x.bin"), [0u8; 10]).unwrap();
        std::fs::write(dir.join("y.bin"), [0u8; 5]).unwrap();
        assert_eq!(empty_dir(&dir), (15, 0));
        assert!(dir.is_dir());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn studio_things_are_recognised() {
        for name in ["RobloxStudio", "GlobalBasicSettings_13_Studio.xml", "OTAPlugins"] {
            assert!(is_studio(name), "{name}");
        }
        for name in ["LocalStorage", "rbx-storage", "GlobalBasicSettings_13.xml", "logs"] {
            assert!(!is_studio(name), "{name}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn the_installed_roblox_is_never_on_the_list() {
        for path in roblox_data(true) {
            assert_ne!(path.file_name().unwrap(), "Versions", "Roblox itself must stay");
        }
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

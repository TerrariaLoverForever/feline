// SPDX-License-Identifier: GPL-3.0-or-later
//! Roblox's own, official client on Windows.
//!
//! Windows doesn't need a runtime: Roblox makes a Windows client. Roblox's
//! installer puts each version in its own folder,
//! `%LOCALAPPDATA%\Roblox\Versions\version-<hash>\RobloxPlayerBeta.exe`, and
//! registers that program for `roblox://` and `roblox-player:` links. Feline
//! starts it the same ways Roblox's own shortcut and link handler do:
//!
//! ```text
//! RobloxPlayerBeta.exe                  the Roblox app (its home screen)
//! RobloxPlayerBeta.exe <roblox-link>    join the experience in the link
//! ```
//!
//! Feline ships none of Roblox's files: it only starts a copy that Roblox's
//! installer put on the computer.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::config::{Choice, Kind, SettingDef, Val};
use super::{
    DetectContext, InstallKind, Installation, LaunchPlan, LaunchTarget, Origin, RuntimeBackend,
    RuntimeError, SettingSection, SettingTab,
};

/// The Roblox player's program file.
pub const PLAYER_EXE: &str = "RobloxPlayerBeta.exe";

// ---------------------------------------------------------------------------
// Roblox's own settings
// ---------------------------------------------------------------------------
//
// These are the settings of Roblox's in-game menu, stored in
// `%LOCALAPPDATA%\Roblox\GlobalBasicSettings_<n>.xml` (UserGameSettings).
// Keys and value types were read from a real file; enum values are Roblox's
// own (`Enum.SavedQualitySetting`). Feline only changes values that are
// already in the file, and only while Roblox is closed (Roblox writes the
// file when it exits).

const fn toggle(key: &'static str, label: &'static str, help: &'static str, default: bool) -> SettingDef {
    SettingDef {
        key,
        label,
        help,
        kind: Kind::Toggle,
        default: Val::Bool(default),
        link: None,
    }
}

const fn choice(c: &'static str, label: &'static str) -> Choice {
    Choice { value: c, label }
}

static SETTINGS: &[SettingDef] = &[
    SettingDef {
        key: "SavedQualityLevel",
        label: "Graphics quality",
        help: "Roblox's graphics slider. Automatic lets Roblox pick as you play.",
        kind: Kind::Choice(&[
            choice("0", "Automatic"),
            choice("1", "1 (fastest)"),
            choice("2", "2"),
            choice("3", "3"),
            choice("4", "4"),
            choice("5", "5"),
            choice("6", "6"),
            choice("7", "7"),
            choice("8", "8"),
            choice("9", "9"),
            choice("10", "10 (prettiest)"),
        ]),
        default: Val::Str("0"),
        link: None,
    },
    SettingDef {
        key: "FramerateCap",
        label: "Maximum frame rate",
        help: "Roblox's own frame rate limit. Higher needs a screen that can show it.",
        kind: Kind::Choice(&[
            choice("-1", "Roblox's default"),
            choice("30", "30 FPS (battery saver)"),
            choice("60", "60 FPS"),
            choice("120", "120 FPS"),
            choice("144", "144 FPS"),
            choice("240", "240 FPS"),
        ]),
        default: Val::Str("-1"),
        link: None,
    },
    toggle("Fullscreen", "Start in fullscreen", "Alt+Enter or F11 still switches while you play.", false),
    toggle(
        "StartMaximized",
        "Start maximized",
        "Opens the Roblox window at full size when it isn't fullscreen.",
        true,
    ),
    SettingDef {
        key: "MasterVolume",
        label: "Volume",
        help: "Roblox's master volume.",
        kind: Kind::Choice(&[
            choice("0", "Muted"),
            choice("0.1", "10%"),
            choice("0.2", "20%"),
            choice("0.3", "30%"),
            choice("0.4", "40%"),
            choice("0.5", "50%"),
            choice("0.6", "60%"),
            choice("0.7", "70%"),
            choice("0.8", "80%"),
            choice("0.9", "90%"),
            choice("1", "100%"),
        ]),
        default: Val::Str("1"),
        link: None,
    },
    SettingDef {
        key: "MouseSensitivity",
        label: "Camera sensitivity",
        help: "How fast the camera turns with the mouse.",
        kind: Kind::Choice(&[
            choice("0.2", "Very slow"),
            choice("0.5", "Slow"),
            choice("0.75", "A bit slow"),
            choice("1", "Normal"),
            choice("1.5", "A bit fast"),
            choice("2", "Fast"),
            choice("3", "Very fast"),
            choice("4", "Zoomies"),
        ]),
        default: Val::Str("1"),
        link: None,
    },
    toggle("CameraYInverted", "Invert camera up/down", "", false),
    toggle(
        "PerformanceStatsVisible",
        "Show performance stats",
        "Roblox's own overlay with frame rate, memory and ping.",
        false,
    ),
    toggle(
        "ReducedMotion",
        "Reduce motion",
        "Fewer camera shakes and moving effects, for comfort.",
        false,
    ),
    toggle("ChatVisible", "Show chat", "", true),
    toggle("PlayerNamesEnabled", "Show player names", "", true),
];

static TABS: &[SettingTab] = &[
    SettingTab {
        name: "Graphics",
        sections: &[SettingSection {
            title: "Picture",
            keys: &["SavedQualityLevel", "FramerateCap", "Fullscreen", "StartMaximized"],
        }],
    },
    SettingTab {
        name: "Sound & controls",
        sections: &[
            SettingSection {
                title: "Sound",
                keys: &["MasterVolume"],
            },
            SettingSection {
                title: "Camera",
                keys: &["MouseSensitivity", "CameraYInverted"],
            },
        ],
    },
    SettingTab {
        name: "Interface",
        sections: &[SettingSection {
            title: "",
            keys: &[
                "PerformanceStatsVisible",
                "ReducedMotion",
                "ChatVisible",
                "PlayerNamesEnabled",
            ],
        }],
    },
];

/// Roblox's settings file: the newest `GlobalBasicSettings_<n>.xml` (the
/// number goes up when Roblox changes the format; `_Studio` is Studio's).
pub fn settings_file(roblox_dir: &Path) -> Option<PathBuf> {
    fs::read_dir(roblox_dir)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let number: u32 = name
                .strip_prefix("GlobalBasicSettings_")?
                .strip_suffix(".xml")?
                .parse()
                .ok()?;
            Some((number, e.path()))
        })
        .max_by_key(|(number, _)| *number)
        .map(|(_, path)| path)
}

/// The one instance of the backend (it has no state).
pub static ROBLOX: Roblox = Roblox;

pub struct Roblox;

impl RuntimeBackend for Roblox {
    fn id(&self) -> &'static str {
        "roblox"
    }

    fn name(&self) -> &'static str {
        "Roblox"
    }

    fn origin(&self) -> Origin {
        Origin::Official
    }

    fn detect(&'static self, ctx: &DetectContext) -> Vec<Installation> {
        let Some(versions) = versions_dir(ctx) else {
            return Vec::new();
        };
        newest_player(&versions)
            .map(|exe| Installation {
                backend: self,
                known_version: exe_version(&exe),
                kind: InstallKind::Official(exe),
            })
            .into_iter()
            .collect()
    }

    fn version(&self, installation: &Installation) -> Result<String, RuntimeError> {
        match &installation.kind {
            InstallKind::Official(exe) => exe_version(exe).ok_or(RuntimeError::UnexpectedOutput {
                what: "Roblox's version information",
            }),
            _ => Err(RuntimeError::Unsupported {
                what: "a version query",
            }),
        }
    }

    fn process_names(&self) -> &'static [&'static str] {
        &[PLAYER_EXE]
    }

    fn settings(&self) -> &'static [SettingDef] {
        SETTINGS
    }

    fn setting_tabs(&self) -> &'static [SettingTab] {
        TABS
    }

    fn config_path(&self, _installation: &Installation, ctx: &DetectContext) -> Option<PathBuf> {
        settings_file(&ctx.local_app_data.as_ref()?.join("Roblox"))
    }

    fn mod_target(
        &self,
        installation: &Installation,
        _ctx: &DetectContext,
    ) -> Option<crate::mods::ModTarget> {
        // Roblox's own version folder, as Bloxstrap modifies it. A new
        // version folder after an update starts clean; mods are applied to
        // it at the next start.
        match &installation.kind {
            InstallKind::Official(exe) => Some(crate::mods::ModTarget {
                dir: exe.parent()?.to_owned(),
                overlay: false,
            }),
            _ => None,
        }
    }

    fn roblox_log_roots(&self, _installation: &Installation, ctx: &DetectContext) -> Vec<PathBuf> {
        // Checked on a real install: `<version>_<time>_Player_<id>_last.log`.
        ctx.local_app_data
            .as_ref()
            .map(|dir| dir.join("Roblox").join("logs"))
            .into_iter()
            .collect()
    }

    fn plan_launch(
        &self,
        installation: &Installation,
        target: &LaunchTarget,
    ) -> Result<LaunchPlan, RuntimeError> {
        let mut command = installation.command();
        if let InstallKind::Official(exe) = &installation.kind
            && let Some(dir) = exe.parent()
        {
            // Roblox finds its content folders next to itself either way,
            // but its own shortcut starts it there too.
            command.current_dir(dir);
        }
        if let LaunchTarget::Game(link) = target {
            // The same as Roblox's own link handler: `"…\RobloxPlayerBeta.exe" %1`.
            command.arg(link.launch_uri());
        }
        Ok(LaunchPlan {
            command,
            note: None,
        })
    }
}

// ---------------------------------------------------------------------------
// Around starting the game (Feline's own Windows options)
// ---------------------------------------------------------------------------

/// Who signs Roblox's client.
pub const ROBLOX_SIGNER: &str = "Roblox Corporation";

/// Before Roblox's own client starts: checks that it's really Roblox's
/// (if that's on) and applies the GPU choice. An `Err` means "don't start
/// it", with the reason for the user; `Ok` may carry warnings worth showing.
/// Does nothing for other runtimes.
pub fn prepare(
    installation: &Installation,
    settings: &crate::settings::Settings,
) -> Result<Vec<String>, String> {
    let InstallKind::Official(exe) = &installation.kind else {
        return Ok(Vec::new());
    };
    #[cfg(windows)]
    {
        let mut warnings = Vec::new();
        if settings.close_stuck_roblox {
            let ended = feline_win::game::end_processes(PLAYER_EXE);
            if ended > 0 {
                warnings.push(format!(
                    "Shooed away {ended} Roblox that {} still running in the background.",
                    if ended == 1 { "was" } else { "were" }
                ));
            }
        }
        if settings.verify_roblox {
            match feline_win::game::signer(exe) {
                Ok(signer) if signer == ROBLOX_SIGNER => {}
                Ok(other) => {
                    return Err(format!(
                        "Feline didn't start {}: it's signed by \"{other}\", not by {ROBLOX_SIGNER}. \
                         Reinstall Roblox from roblox.com. (Settings → Privacy can switch this check off.)",
                        exe.display()
                    ));
                }
                Err(why) => {
                    return Err(format!(
                        "Feline didn't start {}: {why}. Reinstall Roblox from roblox.com. \
                         (Settings → Privacy can switch this check off.)",
                        exe.display()
                    ));
                }
            }
        }
        if let Err(e) = feline_win::game::set_high_performance_gpu(exe, settings.fast_gpu) {
            warnings.push(format!("The GPU choice wasn't applied: {e}."));
        }
        Ok(warnings)
    }
    #[cfg(not(windows))]
    {
        let _ = (exe, settings);
        Ok(Vec::new())
    }
}

/// Right after Roblox's own client started: gives it the chosen priority.
/// Returns a warning if Windows refused.
pub fn after_start(pid: u32, settings: &crate::settings::Settings) -> Option<String> {
    use crate::settings::GamePriority;
    if settings.game_priority == GamePriority::Normal {
        return None;
    }
    #[cfg(windows)]
    {
        let priority = match settings.game_priority {
            GamePriority::Normal => feline_win::game::Priority::Normal,
            GamePriority::AboveNormal => feline_win::game::Priority::AboveNormal,
            GamePriority::High => feline_win::game::Priority::High,
        };
        (!feline_win::game::set_priority(pid, priority))
            .then(|| "Windows didn't accept the game's priority.".to_owned())
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
        None
    }
}

/// Feline's icon as a Windows `.ico`, written to Feline's state folder the
/// first time it's needed (Windows loads window icons from files).
pub fn feline_icon_file() -> Option<PathBuf> {
    const ICO: &[u8] = include_bytes!("../../../../assets/icons/feline.ico");
    let path = crate::paths::state_dir()?.join("feline.ico");
    if fs::read(&path).ok().as_deref() != Some(ICO) {
        fs::create_dir_all(path.parent()?).ok()?;
        fs::write(&path, ICO).ok()?;
    }
    Some(path)
}

/// Decides when the player has left the game, for "close Roblox when you
/// leave a game". Feed it one [`LeaveWatch::tick`] every couple of seconds.
///
/// Left means: after being in a game, Roblox has sat in its own menus for
/// two ticks in a row (a teleport between places never shows the menus), or
/// its window has been gone for three ticks while the process lingers.
#[derive(Debug, Default, Clone)]
pub struct LeaveWatch {
    seen_game: bool,
    menu_ticks: u32,
    gone_ticks: u32,
}

impl LeaveWatch {
    const MENU_TICKS: u32 = 2;
    const GONE_TICKS: u32 = 3;

    /// `in_game`/`at_menu`: what Roblox's log says right now; `has_window`:
    /// whether Roblox has a visible window. Returns true once: end it now.
    pub fn tick(&mut self, in_game: bool, at_menu: bool, has_window: bool) -> bool {
        self.seen_game |= in_game;
        if !self.seen_game {
            return false;
        }
        self.menu_ticks = if at_menu { self.menu_ticks + 1 } else { 0 };
        self.gone_ticks = if has_window { 0 } else { self.gone_ticks + 1 };
        let left = self.menu_ticks >= Self::MENU_TICKS || self.gone_ticks >= Self::GONE_TICKS;
        if left {
            // Only once.
            self.seen_game = false;
            self.menu_ticks = 0;
            self.gone_ticks = 0;
        }
        left
    }
}

/// Whether the Roblox process `pid` has a visible window (always true off
/// Windows, where this isn't used).
pub fn has_window(pid: u32) -> bool {
    #[cfg(windows)]
    {
        !feline_win::window::windows_of(pid).is_empty()
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
        true
    }
}

/// Ends the Roblox Feline started (Windows). Returns whether it ended.
pub fn end_game(pid: u32) -> bool {
    #[cfg(windows)]
    {
        feline_win::game::end_process(pid)
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
        false
    }
}

/// While the game runs: gives its windows Feline's icon and title bar
/// colours, as chosen. `done` remembers the windows already dressed, so
/// call this every few seconds (Roblox opens its window a bit after
/// starting). Does nothing when both options are off, or off Windows.
pub fn dress_windows(pid: u32, settings: &crate::settings::Settings, done: &mut Vec<isize>) {
    use crate::settings::TitleBar;
    if !settings.roblox_icon && settings.title_bar == TitleBar::Normal {
        return;
    }
    #[cfg(windows)]
    {
        let icon = settings.roblox_icon.then(feline_icon_file).flatten();
        for window in feline_win::window::windows_of(pid) {
            if done.contains(&window) {
                continue;
            }
            if let Some(icon) = &icon {
                feline_win::window::set_icon(window, icon);
                // Its own taskbar button, with the cat (otherwise Windows
                // shows the icon of Roblox's program file there). Pinning it
                // starts Roblox through Feline.
                let relaunch = std::env::current_exe()
                    .map(|exe| format!("\"{}\"", exe.display()))
                    .unwrap_or_default();
                feline_win::window::set_taskbar_identity(
                    window,
                    "TerrariaLoverForever.Feline.Roblox",
                    &format!("{},0", icon.display()),
                    "Roblox (Feline)",
                    &relaunch,
                );
            }
            match settings.title_bar {
                TitleBar::Normal => {}
                TitleBar::Purple => {
                    feline_win::window::set_title_colors(window, Some(0x8F7EF7), Some(0xFFFFFF), Some(0x8F7EF7));
                }
                TitleBar::Night => {
                    feline_win::window::set_title_colors(window, Some(0x1E1630), Some(0xECE8FF), Some(0x8F7EF7));
                }
            }
            done.push(window);
        }
    }
    #[cfg(not(windows))]
    let _ = (pid, done);
}

/// Repairs Roblox: takes Feline's mods out, then runs Roblox's own installer
/// (which checks every file and downloads what's missing or damaged). The
/// installer must carry Roblox's signature. Returns what happened, for the
/// user.
pub fn repair(installation: &Installation, ctx: &DetectContext) -> Result<String, String> {
    let InstallKind::Official(exe) = &installation.kind else {
        return Err("only Roblox's own client can be repaired here".to_owned());
    };
    if installation.is_running() {
        return Err("Roblox is running. Close it first.".to_owned());
    }
    let restored = crate::mods::restore_now(installation, ctx).unwrap_or(0);
    let installer = exe
        .parent()
        .map(|d| d.join("RobloxPlayerInstaller.exe"))
        .filter(|p| p.is_file())
        .ok_or("Roblox's installer isn't there; download Roblox again from roblox.com")?;
    #[cfg(windows)]
    match feline_win::game::signer(&installer) {
        Ok(signer) if signer == ROBLOX_SIGNER => {}
        Ok(other) => return Err(format!("the installer is signed by \"{other}\", not Roblox; not running it")),
        Err(why) => return Err(format!("the installer's signature failed: {why}; not running it")),
    }
    let mut command = std::process::Command::new(&installer);
    if let Some(dir) = installer.parent() {
        command.current_dir(dir);
    }
    crate::launch::detached(command)
        .spawn()
        .map_err(|e| format!("Roblox's installer couldn't start: {e}"))?;
    Ok(if restored > 0 {
        format!("Mods are out ({restored} files back) and Roblox's installer is checking everything. Your mods go back in at the next Play.")
    } else {
        "Roblox's installer is checking every file and fixing what's broken.".to_owned()
    })
}

// ---------------------------------------------------------------------------
// Staying on a version
// ---------------------------------------------------------------------------

/// Feline's kept copies of Roblox versions, for "stay on this version":
/// Roblox's updater deletes the old version folder, so Feline keeps one
/// of its own (hard links: no extra disk space while Roblox's copy exists).
pub fn kept_versions_dir() -> Option<PathBuf> {
    Some(crate::paths::state_dir()?.join("Roblox versions"))
}

/// The kept version `name`'s program, if Feline has it.
pub fn kept_player(name: &str) -> Option<PathBuf> {
    if !name.starts_with("version-") || name.contains(['/', '\\']) {
        return None;
    }
    let exe = kept_versions_dir()?.join(name).join(PLAYER_EXE);
    exe.is_file().then_some(exe)
}

/// Keeps the version `installation` runs, so Feline can go on starting it
/// after Roblox updates. Takes Feline's mods out of it first (they go back
/// in at the next Play), so the kept copy is Roblox's own. Returns the
/// version folder's name.
pub fn keep_version(installation: &Installation, ctx: &DetectContext) -> Result<String, String> {
    let InstallKind::Official(exe) = &installation.kind else {
        return Err("only Roblox's own Windows client can stay on a version".to_owned());
    };
    let source = exe.parent().ok_or("Roblox's folder wasn't found")?;
    let name = source
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| n.starts_with("version-"))
        .ok_or("that isn't one of Roblox's version folders")?;
    if kept_player(&name).is_some() {
        return Ok(name);
    }
    if installation.is_running() {
        return Err("Roblox is running. Close it first.".to_owned());
    }
    crate::mods::restore_now(installation, ctx).map_err(|e| format!("Feline's mods couldn't be taken out first: {e}"))?;
    let kept = kept_versions_dir().ok_or("no folder for Feline's state")?;
    let unpacking = kept.join(format!(".keeping-{}", std::process::id()));
    let _ = fs::remove_dir_all(&unpacking);
    let mut stack = vec![source.to_owned()];
    while let Some(dir) = stack.pop() {
        let rel = dir.strip_prefix(source).map_err(|e| e.to_string())?;
        fs::create_dir_all(unpacking.join(rel)).map_err(|e| e.to_string())?;
        for entry in fs::read_dir(&dir).map_err(|e| e.to_string())?.filter_map(Result::ok) {
            let path = entry.path();
            let to = unpacking.join(rel).join(entry.file_name());
            match entry.file_type() {
                Ok(t) if t.is_dir() => stack.push(path),
                Ok(t) if t.is_file() && fs::hard_link(&path, &to).is_err() => {
                    fs::copy(&path, &to).map_err(|e| format!("couldn't copy {}: {e}", path.display()))?;
                }
                _ => {}
            }
        }
    }
    fs::rename(&unpacking, kept.join(&name)).map_err(|e| e.to_string())?;
    Ok(name)
}

/// Deletes a kept version (and what Feline remembers of mods in it).
pub fn forget_version(name: &str) {
    let Some(exe) = kept_player(name) else { return };
    if let Some(dir) = exe.parent() {
        crate::mods::forget_target(dir);
        let _ = fs::remove_dir_all(dir);
    }
}

/// The kept versions, newest first.
pub fn kept_versions() -> Vec<String> {
    let Some(dir) = kept_versions_dir() else { return Vec::new() };
    let mut found: Vec<(SystemTime, String)> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let exe = kept_player(&name)?;
            Some((exe.metadata().ok()?.modified().ok()?, name))
        })
        .collect();
    found.sort_by_key(|(time, _)| std::cmp::Reverse(*time));
    found.into_iter().map(|(_, n)| n).collect()
}

/// `installation` switched to the kept version `name` (Roblox's own client
/// only). `None` if that version isn't kept.
pub fn pinned(installation: &Installation, name: &str) -> Option<Installation> {
    if !matches!(installation.kind, InstallKind::Official(_)) {
        return None;
    }
    let exe = kept_player(name)?;
    Some(Installation {
        backend: installation.backend,
        known_version: exe_version(&exe),
        kind: InstallKind::Official(exe),
    })
}

/// `%LOCALAPPDATA%\Roblox\Versions`, if it exists.
fn versions_dir(ctx: &DetectContext) -> Option<PathBuf> {
    let dir = ctx.local_app_data.as_ref()?.join("Roblox").join("Versions");
    dir.is_dir().then_some(dir)
}

/// The newest `version-*\RobloxPlayerBeta.exe` in `versions`. Roblox Studio
/// lives in the same folder (with `RobloxStudioBeta.exe` instead), and an
/// old version can stay behind after an update, so the newest player wins.
pub fn newest_player(versions: &Path) -> Option<PathBuf> {
    fs::read_dir(versions)
        .ok()?
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with("version-"))
        .map(|e| e.path().join(PLAYER_EXE))
        .filter_map(|exe| {
            let modified = exe.metadata().ok()?.modified().ok()?;
            exe.is_file().then_some((modified, exe))
        })
        .max_by_key(|(modified, _): &(SystemTime, PathBuf)| *modified)
        .map(|(_, exe)| exe)
}

/// The version Roblox's program says it is, e.g. `0.746.0.7461267`, the same
/// as the start of its log files' names.
fn exe_version(exe: &Path) -> Option<String> {
    #[cfg(windows)]
    {
        feline_win::file_version(exe)
    }
    #[cfg(not(windows))]
    {
        let _ = exe;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link::GameLink;
    use std::time::Duration;

    #[test]
    fn leaving_is_seen_at_the_menus_or_when_the_window_goes() {
        // Starting up: menus and no window yet don't count.
        let mut w = LeaveWatch::default();
        assert!(!w.tick(false, true, false));
        assert!(!w.tick(false, false, false));
        assert!(!w.tick(false, true, false));
        // In a game; one menu blip (a teleport) isn't leaving.
        assert!(!w.tick(true, false, true));
        assert!(!w.tick(false, true, true));
        assert!(!w.tick(true, false, true));
        // Back at the menus for two ticks: left, once.
        assert!(!w.tick(false, true, true));
        assert!(w.tick(false, true, true));
        assert!(!w.tick(false, true, true));
        // Window closed but the process lingers: three ticks.
        let mut w = LeaveWatch::default();
        assert!(!w.tick(true, false, true));
        assert!(!w.tick(true, false, false));
        assert!(!w.tick(true, false, false));
        assert!(w.tick(true, false, false));
    }

    fn ctx(local_app_data: PathBuf) -> DetectContext {
        DetectContext {
            home: None,
            path: None,
            flatpak_system_dir: PathBuf::from("/nonexistent"),
            config_home: None,
            local_app_data: Some(local_app_data),
        }
    }

    fn args(plan: &LaunchPlan) -> Vec<String> {
        plan.command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn finds_the_newest_player_and_plans_launches() {
        let root = std::env::temp_dir().join(format!("feline-test-roblox-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let versions = root.join("Roblox").join("Versions");
        let old = versions.join("version-0000000000000001");
        let new = versions.join("version-0000000000000002");
        let studio = versions.join("version-0000000000000003");
        for dir in [&old, &new, &studio] {
            fs::create_dir_all(dir).unwrap();
        }
        fs::write(old.join(PLAYER_EXE), "old").unwrap();
        // Different modification times, newest last.
        std::thread::sleep(Duration::from_millis(50));
        fs::write(new.join(PLAYER_EXE), "new").unwrap();
        std::thread::sleep(Duration::from_millis(50));
        fs::write(studio.join("RobloxStudioBeta.exe"), "studio").unwrap();

        let found = ROBLOX.detect(&ctx(root.clone()));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, InstallKind::Official(new.join(PLAYER_EXE)));
        // The key doesn't change when Roblox updates itself.
        assert_eq!(found[0].key(), "roblox:official");

        let home = found[0].plan_launch(&LaunchTarget::Home).unwrap();
        assert!(args(&home).is_empty());
        assert_eq!(home.command.get_current_dir(), Some(new.as_path()));
        let game = found[0]
            .plan_launch(&LaunchTarget::Game(GameLink::Experience { place_id: 42 }))
            .unwrap();
        assert_eq!(args(&game), ["roblox://experiences/start?placeId=42"]);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn the_newest_settings_file_is_used() {
        let dir = std::env::temp_dir().join(format!("feline-test-roblox-xml-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for name in [
            "GlobalBasicSettings_9.xml",
            "GlobalBasicSettings_13.xml",
            "GlobalBasicSettings_13_Studio.xml",
        ] {
            fs::write(dir.join(name), "x").unwrap();
        }
        assert_eq!(settings_file(&dir), Some(dir.join("GlobalBasicSettings_13.xml")));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_real_settings_file_reads_if_roblox_is_installed() {
        let Some(local) = std::env::var_os("LOCALAPPDATA") else {
            return;
        };
        let Some(path) = settings_file(&Path::new(&local).join("Roblox")) else {
            return;
        };
        let file = super::super::config::ConfigFile::load(&path).expect("parses");
        for def in SETTINGS {
            // Every listed setting reads as an allowed value.
            assert!(def.accepts(&file.get(def)), "{}", def.key);
        }
        // And nothing changes when written back unchanged.
        assert_eq!(file.to_text(), fs::read_to_string(&path).unwrap());
    }

    #[test]
    fn nothing_found_without_roblox() {
        let root = std::env::temp_dir().join("feline-test-roblox-none");
        assert!(ROBLOX.detect(&ctx(root)).is_empty());
    }
}

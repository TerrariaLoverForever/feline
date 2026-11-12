// SPDX-License-Identifier: GPL-3.0-or-later
//! Where Feline keeps its files.
//!
//! On Linux (and other Unix systems), following the XDG Base Directory spec:
//!
//! | What | Variable | Default |
//! |---|---|---|
//! | settings | `$XDG_CONFIG_HOME/feline` | `~/.config/feline` |
//! | logs | `$XDG_STATE_HOME/feline` | `~/.local/state/feline` |
//! | menu entry, icons | `$XDG_DATA_HOME` | `~/.local/share` |
//!
//! The spec says relative values must be ignored, so only absolute paths from
//! the variables are used.
//!
//! On Windows, the usual per-user folders:
//!
//! | What | Folder |
//! |---|---|
//! | settings | `%APPDATA%\Feline` (roams with the account) |
//! | logs, state | `%LOCALAPPDATA%\Feline` |
//! | data | `%LOCALAPPDATA%` |

use std::path::PathBuf;

/// Reverse-DNS application ID. Used as the Wayland/X11 app ID and as the name
/// of the installed `.desktop` file and icons, which is how desktops find
/// Feline's icon for its window.
pub const APP_ID: &str = "io.github.TerrariaLoverForever.Feline";

#[cfg(not(windows))]
fn xdg_dir(variable: &str, fallback_under_home: &str) -> Option<PathBuf> {
    std::env::var_os(variable)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .map(|home| home.join(fallback_under_home))
        })
}

/// A Windows folder from an environment variable such as `APPDATA`.
#[cfg(windows)]
fn known_dir(variable: &str) -> Option<PathBuf> {
    std::env::var_os(variable)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

/// `~/.config/feline` (Windows: `%APPDATA%\Feline`)
pub fn config_dir() -> Option<PathBuf> {
    #[cfg(not(windows))]
    return xdg_dir("XDG_CONFIG_HOME", ".config").map(|p| p.join("feline"));
    #[cfg(windows)]
    return known_dir("APPDATA").map(|p| p.join("Feline"));
}

/// `~/.local/state/feline` (Windows: `%LOCALAPPDATA%\Feline`)
pub fn state_dir() -> Option<PathBuf> {
    #[cfg(not(windows))]
    return xdg_dir("XDG_STATE_HOME", ".local/state").map(|p| p.join("feline"));
    #[cfg(windows)]
    return known_dir("LOCALAPPDATA").map(|p| p.join("Feline"));
}

/// `~/.local/state/feline/logs` (Windows: `%LOCALAPPDATA%\Feline\logs`)
pub fn log_dir() -> Option<PathBuf> {
    state_dir().map(|p| p.join("logs"))
}

/// `~/.local/share` (Windows: `%LOCALAPPDATA%`)
pub fn data_home() -> Option<PathBuf> {
    #[cfg(not(windows))]
    return xdg_dir("XDG_DATA_HOME", ".local/share");
    #[cfg(windows)]
    return known_dir("LOCALAPPDATA");
}

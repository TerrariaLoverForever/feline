// SPDX-License-Identifier: GPL-3.0-or-later
//! Where Feline keeps its files, following the XDG Base Directory spec:
//!
//! | What | Variable | Default |
//! |---|---|---|
//! | settings | `$XDG_CONFIG_HOME/feline` | `~/.config/feline` |
//! | logs | `$XDG_STATE_HOME/feline` | `~/.local/state/feline` |
//! | menu entry, icons | `$XDG_DATA_HOME` | `~/.local/share` |
//!
//! The spec says relative values must be ignored, so only absolute paths from
//! the variables are used.

use std::path::PathBuf;

/// Reverse-DNS application ID. Used as the Wayland/X11 app ID and as the name
/// of the installed `.desktop` file and icons, which is how desktops find
/// Feline's icon for its window.
pub const APP_ID: &str = "io.github.TerrariaLoverForever.Feline";

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

/// `~/.config/feline`
pub fn config_dir() -> Option<PathBuf> {
    xdg_dir("XDG_CONFIG_HOME", ".config").map(|p| p.join("feline"))
}

/// `~/.local/state/feline`
pub fn state_dir() -> Option<PathBuf> {
    xdg_dir("XDG_STATE_HOME", ".local/state").map(|p| p.join("feline"))
}

/// `~/.local/state/feline/logs`
pub fn log_dir() -> Option<PathBuf> {
    state_dir().map(|p| p.join("logs"))
}

/// `~/.local/share`
pub fn data_home() -> Option<PathBuf> {
    xdg_dir("XDG_DATA_HOME", ".local/share")
}

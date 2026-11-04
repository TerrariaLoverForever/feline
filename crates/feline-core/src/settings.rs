// SPDX-License-Identifier: GPL-3.0-or-later
//! Feline's saved settings, in `~/.config/feline/settings.conf`.
//!
//! The format is plain `key = value` lines, readable and editable by hand:
//!
//! ```text
//! # Feline settings
//! runtime = cordial:flatpak-system
//! account_notice = 1
//! renderer = opengl
//! gamemode = on
//! ```
//!
//! Stability rules:
//! - Unknown keys and malformed lines are ignored, so an old Feline can read a
//!   newer file (and a hand-edit can't stop Feline from starting).
//! - Saving is atomic: write a temporary file, then rename it over the old
//!   one. A crash mid-save leaves the old file intact, never half a file.

use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use crate::paths;

/// The current version of the account-risk notice shown on first start.
/// Bump it when the notice's content changes meaningfully, so everyone who
/// accepted an older version sees the new one once.
pub const ACCOUNT_NOTICE_VERSION: u32 = 1;

/// Feline's renderer override for all runtimes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Renderer {
    /// Don't override: each runtime uses its own setting.
    #[default]
    Runtime,
    Vulkan,
    OpenGl,
}

/// An on/off override, or "let each runtime decide".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Toggle {
    #[default]
    Runtime,
    On,
    Off,
}

/// Feline-wide settings that override the matching setting of every runtime
/// (see `runtime::config`). The default overrides nothing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Overrides {
    pub renderer: Renderer,
    pub gamemode: Toggle,
    pub close_on_leave: Toggle,
}

impl Renderer {
    fn parse(value: &str) -> Self {
        match value {
            "vulkan" => Self::Vulkan,
            "opengl" => Self::OpenGl,
            _ => Self::Runtime,
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
            Self::Vulkan => "vulkan",
            Self::OpenGl => "opengl",
        }
    }
}

impl Toggle {
    fn parse(value: &str) -> Self {
        match value {
            "on" => Self::On,
            "off" => Self::Off,
            _ => Self::Runtime,
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
            Self::On => "on",
            Self::Off => "off",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    /// [`Installation::key`](crate::runtime::Installation::key) of the runtime
    /// the user chose. `None` until they choose one: Feline never picks.
    pub runtime: Option<String>,
    /// The version of the account-risk notice the user accepted; 0 = never.
    pub account_notice: u32,
    pub overrides: Overrides,
}

impl Settings {
    /// True if the account-risk notice should be shown (never accepted, or
    /// only an older version of it was).
    pub fn needs_account_notice(&self) -> bool {
        self.account_notice < ACCOUNT_NOTICE_VERSION
    }

    /// The settings file path, `~/.config/feline/settings.conf`.
    pub fn path() -> Option<PathBuf> {
        paths::config_dir().map(|d| d.join("settings.conf"))
    }

    pub fn parse(text: &str) -> Self {
        let mut settings = Self::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "runtime" if !value.is_empty() => settings.runtime = Some(value.to_owned()),
                // A value that isn't a number counts as "never accepted", so
                // the notice is shown again rather than skipped.
                "account_notice" => settings.account_notice = value.parse().unwrap_or(0),
                // Unknown values mean "don't override", the safe choice.
                "renderer" => settings.overrides.renderer = Renderer::parse(value),
                "gamemode" => settings.overrides.gamemode = Toggle::parse(value),
                "close_on_leave" => settings.overrides.close_on_leave = Toggle::parse(value),
                _ => {}
            }
        }
        settings
    }

    pub fn to_text(&self) -> String {
        let mut text = String::from("# Feline settings. Safe to edit while Feline is closed.\n");
        if let Some(runtime) = &self.runtime {
            text.push_str(&format!("runtime = {runtime}\n"));
        }
        if self.account_notice > 0 {
            text.push_str(&format!("account_notice = {}\n", self.account_notice));
        }
        let o = &self.overrides;
        if o.renderer != Renderer::Runtime {
            text.push_str(&format!("renderer = {}\n", o.renderer.as_str()));
        }
        if o.gamemode != Toggle::Runtime {
            text.push_str(&format!("gamemode = {}\n", o.gamemode.as_str()));
        }
        if o.close_on_leave != Toggle::Runtime {
            text.push_str(&format!("close_on_leave = {}\n", o.close_on_leave.as_str()));
        }
        text
    }

    /// Loads from `path`. A missing file means default settings.
    pub fn load_from(path: &Path) -> io::Result<Self> {
        match fs::read_to_string(path) {
            Ok(text) => Ok(Self::parse(&text)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }

    /// Saves to `path` atomically, creating the folder if needed.
    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let temp = path.with_extension("conf.tmp");
        {
            let mut file = fs::File::create(&temp)?;
            file.write_all(self.to_text().as_bytes())?;
            // Make sure the data is on disk before the rename makes it live.
            file.sync_all()?;
        }
        fs::rename(&temp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_is_lenient() {
        let text = "# comment\n\nruntime = cordial:flatpak-system\nfuture_key = 1\nnonsense\n";
        assert_eq!(
            Settings::parse(text).runtime.as_deref(),
            Some("cordial:flatpak-system")
        );
        assert_eq!(Settings::parse("runtime =   \n").runtime, None);
    }

    #[test]
    fn overrides_parse_and_default_to_runtime() {
        let s = Settings::parse("renderer = vulkan\ngamemode = on\nclose_on_leave = maybe\n");
        assert_eq!(s.overrides.renderer, Renderer::Vulkan);
        assert_eq!(s.overrides.gamemode, Toggle::On);
        assert_eq!(s.overrides.close_on_leave, Toggle::Runtime);
        // Nothing overridden → nothing written.
        assert!(!Settings::default().to_text().contains("renderer"));
    }

    #[test]
    fn account_notice_versions() {
        assert!(Settings::default().needs_account_notice());
        assert!(Settings::parse("account_notice = banana\n").needs_account_notice());

        let accepted = Settings::parse(&format!("account_notice = {ACCOUNT_NOTICE_VERSION}\n"));
        assert!(!accepted.needs_account_notice());
        // A newer notice than the one accepted must be shown again.
        let older = Settings {
            account_notice: ACCOUNT_NOTICE_VERSION - 1,
            ..Settings::default()
        };
        assert!(older.needs_account_notice());
    }

    #[test]
    fn round_trip_and_missing_file() {
        let dir = std::env::temp_dir().join(format!("feline-test-settings-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("nested/settings.conf");

        assert_eq!(Settings::load_from(&path).unwrap(), Settings::default());

        let settings = Settings {
            runtime: Some("cordial:appimage:/home/me/My Apps/Cordial.AppImage".into()),
            account_notice: ACCOUNT_NOTICE_VERSION,
            overrides: Overrides {
                renderer: Renderer::OpenGl,
                gamemode: Toggle::Off,
                close_on_leave: Toggle::On,
            },
        };
        settings.save_to(&path).unwrap();
        let loaded = Settings::load_from(&path).unwrap();
        let leftover_temp = path.with_extension("conf.tmp").exists();
        let _ = fs::remove_dir_all(&dir);

        assert_eq!(loaded, settings);
        assert!(!leftover_temp);
    }
}

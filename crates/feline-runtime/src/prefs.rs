// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! The runtime's settings: `~/.config/feline/runtime.json` (or under
//! `$XDG_CONFIG_HOME`), which Feline's launcher shows and edits (see
//! feline-core's `runtime::feline`). Created with the defaults on the first
//! start; read once per start.
//!
//! | Key | What it does |
//! |---|---|
//! | `maxFps` | Roblox's frame-rate target (`DFIntTaskSchedulerTargetFps`); 0 = no limit |
//! | `mouseSensitivity` | percent; Feline scales mouse movement before Roblox sees it |
//! | `graphicsQuality` | 1–21 fixes Roblox's quality level (`DFIntDebugFRMQualityLevelOverride`); 0 = Roblox decides |
//! | `theme` | `"dark"` or `"light"`: the app's theme |
//! | `startFullscreen` | open the window in fullscreen (F11 switches either way) |

use std::path::PathBuf;
use std::sync::OnceLock;

use serde_json::{Map, Value, json};

#[derive(Debug, Clone, PartialEq)]
pub struct Prefs {
    pub max_fps: u32,
    pub mouse_sensitivity: u32,
    pub graphics_quality: u32,
    pub theme: String,
    pub start_fullscreen: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            max_fps: 60,
            mouse_sensitivity: 100,
            graphics_quality: 0,
            theme: "dark".into(),
            start_fullscreen: false,
        }
    }
}

/// The settings file.
pub fn path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("feline/runtime.json"))
}

const HEADER: &str = "// Feline runtime settings. Feline's launcher edits this file.\n";

impl Prefs {
    /// Values from the file's JSON; missing or invalid ones keep the default.
    fn from_json(json: &Map<String, Value>) -> Self {
        let d = Self::default();
        let number = |key: &str, max: u64, default: u32| {
            json.get(key)
                .and_then(Value::as_u64)
                .filter(|&n| n <= max)
                .map_or(default, |n| n as u32)
        };
        Self {
            max_fps: number("maxFps", 1000, d.max_fps),
            mouse_sensitivity: number("mouseSensitivity", 500, d.mouse_sensitivity).max(10),
            graphics_quality: number("graphicsQuality", 21, d.graphics_quality),
            theme: match json.get("theme").and_then(Value::as_str) {
                Some(t @ ("dark" | "light")) => t.to_owned(),
                _ => d.theme,
            },
            start_fullscreen: json
                .get("startFullscreen")
                .and_then(Value::as_bool)
                .unwrap_or(d.start_fullscreen),
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "maxFps": self.max_fps,
            "mouseSensitivity": self.mouse_sensitivity,
            "graphicsQuality": self.graphics_quality,
            "theme": self.theme,
            "startFullscreen": self.start_fullscreen,
        })
    }

    /// Roblox client settings ("FastFlags") these settings need.
    pub fn flags(&self) -> Vec<(&'static str, String)> {
        let mut flags = Vec::new();
        let fps = if self.max_fps == 0 { 9999 } else { self.max_fps };
        flags.push(("DFIntTaskSchedulerTargetFps", fps.to_string()));
        if fps > 240 {
            flags.push(("FFlagTaskSchedulerLimitTargetFpsTo2402", "False".to_owned()));
        }
        if (1..=21).contains(&self.graphics_quality) {
            flags.push((
                "DFIntDebugFRMQualityLevelOverride",
                self.graphics_quality.to_string(),
            ));
        }
        flags
    }
}

/// The settings for this start (read once; the file is created with the
/// defaults if it's missing, and missing keys are added).
pub fn get() -> &'static Prefs {
    static PREFS: OnceLock<Prefs> = OnceLock::new();
    PREFS.get_or_init(|| {
        let Some(path) = path() else {
            return Prefs::default();
        };
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let body: String = text
            .lines()
            .skip_while(|l| l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        let json = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|v| v.as_object().cloned());
        let prefs = json.as_ref().map(Prefs::from_json).unwrap_or_default();
        // Write the file back when it's missing or lacks keys, so the
        // launcher has every setting to show. Unknown keys are kept.
        let Value::Object(current) = prefs.to_json() else {
            return prefs;
        };
        let complete = json
            .as_ref()
            .is_some_and(|j| current.keys().all(|k| j.contains_key(k)));
        if !complete {
            let mut merged = json.unwrap_or_default();
            for (key, value) in current {
                merged.entry(key).or_insert(value);
            }
            let pretty = serde_json::to_string_pretty(&Value::Object(merged)).unwrap_or_default();
            let _ = std::fs::create_dir_all(path.parent().unwrap_or(&path));
            if let Err(e) = std::fs::write(&path, format!("{HEADER}{pretty}\n")) {
                eprintln!("feline-runtime: couldn't write {}: {e}", path.display());
            }
        }
        eprintln!("feline-runtime: settings {prefs:?}");
        prefs
    })
}

/// Sets `flags` in the client settings JSON (`{"applicationSettings": {…}}`
/// or a plain object), over what's there.
pub fn add_flags(settings: &str, flags: &[(&'static str, String)]) -> String {
    let Ok(Value::Object(mut root)) = serde_json::from_str::<Value>(settings) else {
        return settings.to_owned();
    };
    let target = match root.get_mut("applicationSettings") {
        Some(Value::Object(inner)) => inner,
        _ => &mut root,
    };
    for (key, value) in flags {
        target.insert((*key).to_owned(), Value::String(value.clone()));
    }
    Value::Object(root).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_and_flags() {
        let json = json!({"maxFps": 144, "mouseSensitivity": 3, "graphicsQuality": 30, "theme": "pink"});
        let p = Prefs::from_json(json.as_object().unwrap());
        assert_eq!(p.max_fps, 144);
        assert_eq!(p.mouse_sensitivity, 10);
        assert_eq!(p.graphics_quality, 0);
        assert_eq!(p.theme, "dark");
        assert_eq!(p.flags(), vec![("DFIntTaskSchedulerTargetFps", "144".to_owned())]);
        let unlimited = Prefs {
            max_fps: 0,
            graphics_quality: 5,
            ..Prefs::default()
        };
        assert_eq!(unlimited.flags().len(), 3);
        let merged = add_flags(r#"{"applicationSettings":{"A":"1"}}"#, &unlimited.flags());
        let v: Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(v["applicationSettings"]["DFIntTaskSchedulerTargetFps"], "9999");
        assert_eq!(v["applicationSettings"]["A"], "1");
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! The runtime's settings: `~/.config/feline/runtime.json` (or under
//! `$XDG_CONFIG_HOME`), which Feline's launcher shows and edits (see
//! feline-core's `runtime::feline`). Created with the defaults on the first
//! start; read once per start.
//!
//! | Key | What it does |
//! |---|---|
//! | `theme` | `"dark"` or `"light"`: the app's theme |
//! | `unlimitedFps` | set Roblox's in-game Maximum Frame Rate to no limit at each start |
//! | `startFullscreen` | open the window in fullscreen (F11 switches either way) |
//!
//! Frame rate, camera sensitivity and graphics quality are Roblox's own
//! in-game settings (Esc → Settings), as on PC; see [`Prefs::flags`].

use std::path::PathBuf;
use std::sync::OnceLock;

use serde_json::{Map, Value, json};

#[derive(Debug, Clone, PartialEq)]
pub struct Prefs {
    pub theme: String,
    pub unlimited_fps: bool,
    pub start_fullscreen: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            theme: "dark".into(),
            unlimited_fps: false,
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
        Self {
            theme: match json.get("theme").and_then(Value::as_str) {
                Some(t @ ("dark" | "light")) => t.to_owned(),
                _ => d.theme,
            },
            unlimited_fps: json
                .get("unlimitedFps")
                .and_then(Value::as_bool)
                .unwrap_or(d.unlimited_fps),
            start_fullscreen: json
                .get("startFullscreen")
                .and_then(Value::as_bool)
                .unwrap_or(d.start_fullscreen),
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "theme": self.theme,
            "unlimitedFps": self.unlimited_fps,
            "startFullscreen": self.start_fullscreen,
        })
    }

    /// Roblox client settings ("FastFlags") Feline sets: the in-game
    /// "Maximum Frame Rate" setting, which Roblox turns on for PC
    /// (`FFlagGameBasicSettingsFramerateCap5` is True in the PC client's
    /// settings and absent from Android's).
    /// (`unlimitedFps` isn't a flag: Roblox stops `DFIntTaskSchedulerTargetFps`
    /// at 240. See [`write_unlimited_framerate_cap`].)
    pub fn flags(&self) -> Vec<(&'static str, String)> {
        vec![("FFlagGameBasicSettingsFramerateCap5", "True".to_owned())]
    }
}

/// What Roblox's `FramerateCap` setting holds for "no limit", as Mocktail
/// and Nightcap write it (<https://github.com/CoderDayton/nightcap>,
/// Apache-2.0, docs/PERFORMANCE.md).
const UNLIMITED_FRAMERATE_CAP: i32 = 9999;

/// Sets Roblox's own in-game Maximum Frame Rate (`FramerateCap` in
/// `GlobalBasicSettings_13.xml`) to no limit, before the engine reads it.
/// Does nothing if Roblox hasn't written the file yet (first start).
pub fn write_unlimited_framerate_cap(app_data: &std::path::Path) {
    let file = app_data.join("GlobalBasicSettings_13.xml");
    let Ok(text) = std::fs::read_to_string(&file) else {
        return;
    };
    if let Some(updated) = set_framerate_cap(&text, UNLIMITED_FRAMERATE_CAP)
        && updated != text
    {
        match std::fs::write(&file, updated) {
            Ok(()) => eprintln!("feline-runtime: Roblox's frame rate cap set to unlimited"),
            Err(e) => eprintln!("feline-runtime: couldn't write {}: {e}", file.display()),
        }
    }
}

/// `text` (Roblox's settings XML) with `FramerateCap` set to `fps`, added
/// to `<Properties>` if missing. None if the file isn't as expected.
fn set_framerate_cap(text: &str, fps: i32) -> Option<String> {
    const OPEN: &str = "<int name=\"FramerateCap\">";
    let mut text = text.to_owned();
    if let Some(found) = text.find(OPEN) {
        let start = found + OPEN.len();
        let end = start + text[start..].find("</int>")?;
        text.replace_range(start..end, &fps.to_string());
    } else {
        let at = text.find("<Properties>")? + "<Properties>".len();
        text.insert_str(at, &format!("\n\t\t{OPEN}{fps}</int>"));
    }
    Some(text)
}

/// Settings earlier versions had, now Roblox's own in-game ones.
const OBSOLETE: [&str; 3] = ["maxFps", "mouseSensitivity", "graphicsQuality"];

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
        let complete = json.as_ref().is_some_and(|j| {
            current.keys().all(|k| j.contains_key(k)) && !OBSOLETE.iter().any(|k| j.contains_key(*k))
        });
        if !complete {
            let mut merged = json.unwrap_or_default();
            for key in OBSOLETE {
                merged.remove(key);
            }
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
    fn framerate_cap_is_replaced_or_added() {
        let set = "<roblox><Item><Properties>\n\t\t<int name=\"FramerateCap\">-1</int>\n</Properties></Item></roblox>";
        assert!(set_framerate_cap(set, 9999).unwrap().contains("<int name=\"FramerateCap\">9999</int>"));
        let unset = "<roblox><Item><Properties>\n</Properties></Item></roblox>";
        assert!(set_framerate_cap(unset, 9999).unwrap().contains("<Properties>\n\t\t<int name=\"FramerateCap\">9999</int>"));
        assert_eq!(set_framerate_cap("garbage", 9999), None);
    }

    #[test]
    fn values_and_flags() {
        let json = json!({"theme": "pink", "startFullscreen": true, "maxFps": 144});
        let p = Prefs::from_json(json.as_object().unwrap());
        assert_eq!(p.theme, "dark");
        assert!(p.start_fullscreen);
        let merged = add_flags(r#"{"applicationSettings":{"A":"1"}}"#, &p.flags());
        let v: Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(v["applicationSettings"]["FFlagGameBasicSettingsFramerateCap5"], "True");
        assert_eq!(v["applicationSettings"]["A"], "1");
    }
}

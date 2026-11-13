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
//! | `fpsLimit` | 0: Roblox's in-game Maximum Frame Rate decides; otherwise that frame rate (e.g. 600), set as Roblox's cap at each start |
//! | `startFullscreen` | open the window in fullscreen (F11 switches either way) |
//! | `presentMode` | `"mailbox"` (no tearing, no waiting), `"immediate"` (lowest latency, may tear) or `"fifo"` (VSync: waits for the monitor) |
//! | `framePacing` | with an `fpsLimit`: Feline holds frames to an even rhythm at presentation (pacing.rs) |
//! | `throttleUnfocused` | 30 FPS while the window isn't focused |
//! | `renderer` | `"vulkan"` (when the desktop has a Vulkan driver) or `"opengl"` (OpenGL ES; see vulkan.rs) |
//! | `graphicsQuality` | 0: Roblox decides; 1–10: Roblox's graphics quality, set at each start |
//! | `microphone` | let voice chat use the microphone (off: others hear silence) |
//! | `controllers` | pass gamepads to Roblox |
//! | `blockTracking` | Roblox's analytics/telemetry hosts look nonexistent to it (see net.rs) |
//! | `proxy` | "" or an `http://`, `https://`, `socks5://` or `socks5h://` address for Roblox's web traffic |
//! | `detailedLogs` | log every Java call the engine makes (for bug reports) |
//!
//! Frame rate, camera sensitivity and graphics quality are Roblox's own
//! in-game settings (Esc → Settings), as on PC; see [`Prefs::flags`].

use std::path::PathBuf;
use std::sync::OnceLock;

use serde_json::{Map, Value, json};

#[derive(Debug, Clone, PartialEq)]
pub struct Prefs {
    pub theme: String,
    /// Frames per second to cap at; 0 leaves it to Roblox's own setting.
    pub fps_limit: u32,
    pub start_fullscreen: bool,
    /// "mailbox", "immediate" or "fifo" (VSync).
    pub present_mode: String,
    /// Feline paces frames at presentation (with an FPS limit).
    pub frame_pacing: bool,
    /// 30 FPS while the window isn't focused.
    pub throttle_unfocused: bool,
    /// "vulkan" or "opengl".
    pub renderer: String,
    /// Roblox's graphics quality 1–10; 0 leaves it to Roblox.
    pub graphics_quality: u32,
    pub microphone: bool,
    pub controllers: bool,
    pub block_tracking: bool,
    /// "" (no proxy) or a checked proxy address.
    pub proxy: String,
    pub detailed_logs: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            theme: "dark".into(),
            fps_limit: 0,
            start_fullscreen: false,
            present_mode: "mailbox".into(),
            frame_pacing: true,
            throttle_unfocused: false,
            renderer: "vulkan".into(),
            graphics_quality: 0,
            microphone: true,
            controllers: true,
            block_tracking: true,
            proxy: String::new(),
            detailed_logs: false,
        }
    }
}

/// Whether `proxy` is a proxy address Feline passes on: empty (none) or
/// `scheme://host[:port]` with a scheme Roblox's HTTP client understands.
pub fn valid_proxy(proxy: &str) -> bool {
    if proxy.is_empty() {
        return true;
    }
    let Some((scheme, rest)) = proxy.split_once("://") else {
        return false;
    };
    matches!(scheme, "http" | "https" | "socks5" | "socks5h" | "socks4")
        && !rest.is_empty()
        && !rest.contains(char::is_whitespace)
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
    /// Waits for the monitor (present mode "fifo").
    pub fn vsync(&self) -> bool {
        self.present_mode == "fifo"
    }

    /// Values from the file's JSON; missing or invalid ones keep the default.
    fn from_json(json: &Map<String, Value>) -> Self {
        let d = Self::default();
        Self {
            theme: match json.get("theme").and_then(Value::as_str) {
                Some(t @ ("dark" | "light")) => t.to_owned(),
                _ => d.theme,
            },
            fps_limit: match json.get("fpsLimit").and_then(Value::as_u64) {
                Some(n) => (n as u32).min(MAX_FPS_LIMIT),
                // The earlier "Unlimited FPS" switch.
                None if json.get("unlimitedFps").and_then(Value::as_bool) == Some(true) => {
                    MAX_FPS_LIMIT
                }
                None => d.fps_limit,
            },
            start_fullscreen: json
                .get("startFullscreen")
                .and_then(Value::as_bool)
                .unwrap_or(d.start_fullscreen),
            present_mode: match json.get("presentMode").and_then(Value::as_str) {
                Some(m @ ("mailbox" | "immediate" | "fifo")) => m.to_owned(),
                // The earlier VSync switch.
                _ if json.get("vsync").and_then(Value::as_bool) == Some(true) => "fifo".into(),
                _ => d.present_mode,
            },
            frame_pacing: json.get("framePacing").and_then(Value::as_bool).unwrap_or(d.frame_pacing),
            throttle_unfocused: json
                .get("throttleUnfocused")
                .and_then(Value::as_bool)
                .unwrap_or(d.throttle_unfocused),
            renderer: match json.get("renderer").and_then(Value::as_str) {
                Some(r @ ("vulkan" | "opengl")) => r.to_owned(),
                _ => d.renderer,
            },
            graphics_quality: json
                .get("graphicsQuality")
                .and_then(Value::as_u64)
                .map_or(d.graphics_quality, |q| q.min(10) as u32),
            microphone: json.get("microphone").and_then(Value::as_bool).unwrap_or(d.microphone),
            controllers: json.get("controllers").and_then(Value::as_bool).unwrap_or(d.controllers),
            block_tracking: json
                .get("blockTracking")
                .and_then(Value::as_bool)
                .unwrap_or(d.block_tracking),
            proxy: json
                .get("proxy")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|p| valid_proxy(p))
                .map_or(d.proxy, str::to_owned),
            detailed_logs: json
                .get("detailedLogs")
                .and_then(Value::as_bool)
                .unwrap_or(d.detailed_logs),
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "theme": self.theme,
            "fpsLimit": self.fps_limit,
            "startFullscreen": self.start_fullscreen,
            "presentMode": self.present_mode,
            "framePacing": self.frame_pacing,
            "throttleUnfocused": self.throttle_unfocused,
            "renderer": self.renderer,
            "graphicsQuality": self.graphics_quality,
            "microphone": self.microphone,
            "controllers": self.controllers,
            "blockTracking": self.block_tracking,
            "proxy": self.proxy,
            "detailedLogs": self.detailed_logs,
        })
    }

    /// Roblox client settings ("FastFlags") Feline sets: the in-game
    /// "Maximum Frame Rate" setting, which Roblox turns on for PC
    /// (`FFlagGameBasicSettingsFramerateCap5` is True in the PC client's
    /// settings and absent from Android's).
    /// (`fpsLimit` isn't a flag: see [`write_framerate_cap`].)
    pub fn flags(&self) -> Vec<(&'static str, String)> {
        vec![("FFlagGameBasicSettingsFramerateCap5", "True".to_owned())]
    }
}

/// The highest `fpsLimit`: what Roblox's `FramerateCap` holds for "no
/// limit", as Mocktail and Nightcap write it
/// (<https://github.com/CoderDayton/nightcap>, Apache-2.0, docs/PERFORMANCE.md).
pub const MAX_FPS_LIMIT: u32 = 9999;

/// Sets Roblox's own in-game Maximum Frame Rate (`FramerateCap` in
/// `GlobalBasicSettings_13.xml`) to `fps`, before the engine reads it.
/// (Roblox stops `DFIntTaskSchedulerTargetFps` at 240; this setting
/// isn't.) Does nothing if Roblox hasn't written the file yet (first start).
pub fn write_framerate_cap(app_data: &std::path::Path, fps: u32) {
    let file = app_data.join("GlobalBasicSettings_13.xml");
    let Ok(text) = std::fs::read_to_string(&file) else {
        return;
    };
    if let Some(updated) = set_framerate_cap(&text, fps as i32)
        && updated != text
    {
        match std::fs::write(&file, updated) {
            Ok(()) => eprintln!("feline-runtime: Roblox's frame rate cap set to {fps}"),
            Err(e) => eprintln!("feline-runtime: couldn't write {}: {e}", file.display()),
        }
    }
}

/// `text` (Roblox's settings XML) with `FramerateCap` set to `fps`, added
/// to `<Properties>` if missing. None if the file isn't as expected.
fn set_framerate_cap(text: &str, fps: i32) -> Option<String> {
    set_int(text, "FramerateCap", fps)
}

/// `text` (Roblox's settings XML) with the int setting `name` set to
/// `value`, added to `<Properties>` if missing.
fn set_int(text: &str, name: &str, value: i32) -> Option<String> {
    set_value(text, "int", name, value)
}

/// Like [`set_int`] for a setting stored as `<kind name="…">` (Roblox uses
/// `int` and `token`, e.g. for enums).
fn set_value(text: &str, kind: &str, name: &str, value: i32) -> Option<String> {
    let open = format!("<{kind} name=\"{name}\">");
    let close = format!("</{kind}>");
    let mut text = text.to_owned();
    if let Some(found) = text.find(&open) {
        let start = found + open.len();
        let end = start + text[start..].find(&close)?;
        text.replace_range(start..end, &value.to_string());
    } else {
        let at = text.find("<Properties>")? + "<Properties>".len();
        text.insert_str(at, &format!("\n\t\t{open}{value}{close}"));
    }
    Some(text)
}

/// Sets Roblox's graphics quality (its in-game "Graphics Quality" slider,
/// `SavedQualityLevel` 1–10 in `GlobalBasicSettings_13.xml`) before the
/// engine reads it. Does nothing if Roblox hasn't written the file yet.
pub fn write_graphics_quality(app_data: &std::path::Path, level: u32) {
    let file = app_data.join("GlobalBasicSettings_13.xml");
    let Ok(text) = std::fs::read_to_string(&file) else {
        return;
    };
    // Roblox keeps the slider (a token, 1–10) and its internal level (an
    // int, 2 × slider + 1: 10 ↔ 21, 1 ↔ 3) side by side.
    let level = level.clamp(1, 10) as i32;
    if let Some(updated) = set_value(&text, "token", "SavedQualityLevel", level)
        .and_then(|t| set_value(&t, "int", "GraphicsQualityLevel", 2 * level + 1))
        && updated != text
    {
        match std::fs::write(&file, updated) {
            Ok(()) => eprintln!("feline-runtime: Roblox's graphics quality set to {level}"),
            Err(e) => eprintln!("feline-runtime: couldn't write {}: {e}", file.display()),
        }
    }
}

/// Settings earlier versions had, now Roblox's own in-game ones.
/// (`graphicsQuality` and `renderer` were too, and are back as Feline's.)
/// (`vsync` became `presentMode` "fifo".)
const OBSOLETE: [&str; 4] = ["maxFps", "mouseSensitivity", "unlimitedFps", "vsync"];

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
    fn quality_is_written_as_roblox_stores_it() {
        let xml = "<Properties>\n<int name=\"GraphicsQualityLevel\">21</int>\n<token name=\"SavedQualityLevel\">10</token>\n</Properties>";
        let out = set_value(xml, "token", "SavedQualityLevel", 3)
            .and_then(|t| set_value(&t, "int", "GraphicsQualityLevel", 7))
            .unwrap();
        assert!(out.contains("<token name=\"SavedQualityLevel\">3</token>"));
        assert!(out.contains("<int name=\"GraphicsQualityLevel\">7</int>"));
        assert!(!out.contains("<int name=\"SavedQualityLevel\">"));
    }

    #[test]
    fn new_settings_and_proxy_check() {
        let p = Prefs::from_json(json!({"proxy": "socks5://127.0.0.1:9050", "graphicsQuality": 99, "microphone": false}).as_object().unwrap());
        assert_eq!(p.proxy, "socks5://127.0.0.1:9050");
        assert_eq!(p.graphics_quality, 10);
        assert!(!p.microphone);
        assert!(p.block_tracking, "tracking is blocked unless turned off");
        let bad = Prefs::from_json(json!({"proxy": "not a url"}).as_object().unwrap());
        assert_eq!(bad.proxy, "", "an invalid proxy isn't used");
        assert!(valid_proxy("") && valid_proxy("http://proxy:8080") && !valid_proxy("ftp://x"));
    }

    #[test]
    fn fps_limit_and_the_old_unlimited_switch() {
        let from = |j: serde_json::Value| Prefs::from_json(j.as_object().unwrap()).fps_limit;
        assert_eq!(from(json!({})), 0);
        assert_eq!(from(json!({"fpsLimit": 600})), 600);
        assert_eq!(from(json!({"fpsLimit": 123456})), MAX_FPS_LIMIT);
        assert_eq!(from(json!({"unlimitedFps": true})), MAX_FPS_LIMIT);
        assert_eq!(from(json!({"unlimitedFps": false})), 0);
        assert_eq!(from(json!({"fpsLimit": 144, "unlimitedFps": true})), 144);
    }

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

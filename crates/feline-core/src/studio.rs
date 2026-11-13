// SPDX-License-Identifier: GPL-3.0-or-later
//! Roblox Studio (Windows): starting it from Feline with your mods in it,
//! its own FastFlags, and a Discord status while you build.
//!
//! Studio lives next to the player in `%LOCALAPPDATA%\Roblox\Versions`
//! (a version folder with `RobloxStudioBeta.exe`). Roblox's FastFlag
//! allowlist is for the player only, so Studio's flags are written as you
//! give them (names and values are still checked to be plain flag text).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::{Map, Value};

use crate::fflags::FlagSet;

pub const STUDIO_EXE: &str = "RobloxStudioBeta.exe";

/// The newest Roblox Studio on this computer: Roblox's own folder on
/// Windows; on Linux, the versions Vinegar installed (its own program or
/// its Flatpak).
pub fn find(local_app_data: Option<&Path>) -> Option<PathBuf> {
    let mut places: Vec<PathBuf> = local_app_data.map(|d| d.join("Roblox").join("Versions")).into_iter().collect();
    if !cfg!(windows) {
        places.extend(crate::paths::data_home().map(|d| d.join("vinegar").join("versions")));
        places.extend(
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|h| h.join(".var/app/org.vinegarhq.Vinegar/data/vinegar/versions")),
        );
    }
    places
        .iter()
        .filter_map(|dir| fs::read_dir(dir).ok())
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path().join(STUDIO_EXE))
        .filter_map(|exe| Some((exe.metadata().ok()?.modified().ok()?, exe)))
        .max_by_key(|(t, _): &(SystemTime, PathBuf)| *t)
        .map(|(_, exe)| exe)
}

/// A flag name as Roblox writes them: letters, digits and `_`.
fn plain_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 128 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Studio's FastFlags from JSON (`{"Name": value, …}`), any flags. Values
/// become text, as Roblox reads them.
pub fn flags_from_json(text: &str) -> Result<FlagSet, String> {
    let text = text.trim().trim_start_matches('\u{feff}');
    if text.is_empty() {
        return Ok(FlagSet::new());
    }
    let v: Value = serde_json::from_str(text).map_err(|e| format!("that isn't FastFlag JSON ({e})"))?;
    let Value::Object(map) = v else {
        return Err("that isn't FastFlag JSON (it should look like {\"Name\": \"value\"})".to_owned());
    };
    let mut set = FlagSet::new();
    for (name, value) in map {
        if !plain_name(&name) {
            return Err(format!("\"{name}\" isn't a flag name"));
        }
        let value = match value {
            Value::String(s) => s,
            Value::Bool(b) => if b { "True".to_owned() } else { "False".to_owned() },
            Value::Number(n) => n.to_string(),
            _ => return Err(format!("{name}'s value should be text, a number or true/false")),
        };
        set.insert(name, value);
    }
    Ok(set)
}

fn flags_path() -> Option<PathBuf> {
    crate::paths::state_dir().map(|d| d.join("studio-fflags.json"))
}

/// Studio's saved FastFlags.
pub fn flags() -> FlagSet {
    flags_path()
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|t| flags_from_json(&t).ok())
        .unwrap_or_default()
}

pub fn save_flags(set: &FlagSet) -> std::io::Result<()> {
    let path = flags_path().ok_or_else(|| std::io::Error::other("no state folder"))?;
    fs::create_dir_all(path.parent().unwrap_or(&path))?;
    fs::write(path, flags_json(set))
}

pub fn flags_json(set: &FlagSet) -> String {
    let map: Map<String, Value> = set.iter().map(|(k, v)| (k.clone(), Value::from(v.as_str()))).collect();
    serde_json::to_string_pretty(&Value::Object(map)).unwrap_or_default()
}

/// Writes the flags into Studio's folder (keeping a file that wasn't
/// Feline's aside, and putting it back once there are no flags).
pub fn write_flags(studio_dir: &Path, set: &FlagSet) -> Result<(), String> {
    let file = studio_dir.join("ClientSettings").join("ClientAppSettings.json");
    let backup = studio_dir.join("ClientSettings").join("ClientAppSettings.before-feline.json");
    let marker = studio_dir.join("ClientSettings").join(".feline");
    if set.is_empty() {
        if marker.exists() {
            let _ = fs::remove_file(&file);
            if backup.exists() {
                let _ = fs::rename(&backup, &file);
            }
            let _ = fs::remove_file(&marker);
        }
        return Ok(());
    }
    fs::create_dir_all(file.parent().unwrap_or(studio_dir)).map_err(|e| e.to_string())?;
    if !marker.exists() && file.exists() && !backup.exists() {
        fs::rename(&file, &backup).map_err(|e| e.to_string())?;
    }
    fs::write(&file, flags_json(set)).map_err(|e| e.to_string())?;
    fs::write(&marker, "Feline wrote ClientAppSettings.json").map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Class icons (the little pictures in Studio's Explorer)
// ---------------------------------------------------------------------------

/// Where Studio keeps one icon per class: dark and light, normal and 2×.
const ICON_DIRS: [&str; 2] = [
    "content/studio_svg_textures/Shared/InsertableObjects/Dark/Standard",
    "content/studio_svg_textures/Shared/InsertableObjects/Light/Standard",
];

/// The mod holding your Studio icons.
pub const ICONS_MOD: &str = "Feline Studio Icons";

/// Studio's classes with an icon (names, sorted), and the dark icon file.
pub fn class_icons(studio_dir: &Path) -> Vec<(String, PathBuf)> {
    let dir = studio_dir.join(ICON_DIRS[0]);
    let mut list: Vec<(String, PathBuf)> = fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let class = name.strip_suffix(".png")?;
            (!class.contains('@')).then(|| (class.to_owned(), e.path()))
        })
        .collect();
    list.sort_by_key(|(name, _)| name.to_ascii_lowercase());
    list
}

/// Your picture as a class's icon (both themes, both sizes, the sizes
/// Studio's own have), in the icons mod.
pub fn set_class_icon(mods: &Path, studio_dir: &Path, class: &str, picture: &crate::png::Rgba) -> Result<(), String> {
    if !class.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err("that isn't a class name".to_owned());
    }
    for dir in ICON_DIRS {
        for (suffix, fallback) in [("", 16), ("@2x", 32)] {
            let rel = format!("{dir}/{class}{suffix}.png");
            let size = fs::read(studio_dir.join(&rel))
                .ok()
                .and_then(|d| crate::png::decode(&d).ok())
                .map_or(fallback, |i| i.width.max(i.height));
            let to = mods.join(ICONS_MOD).join(&rel);
            fs::create_dir_all(to.parent().unwrap_or(mods)).map_err(|e| e.to_string())?;
            fs::write(to, crate::png::encode(&crate::png::fit_square(picture, size))).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Studio's own icon for a class back.
pub fn clear_class_icon(mods: &Path, class: &str) {
    for dir in ICON_DIRS {
        for suffix in ["", "@2x"] {
            let _ = fs::remove_file(mods.join(ICONS_MOD).join(format!("{dir}/{class}{suffix}.png")));
        }
    }
}

/// The classes you gave an icon.
pub fn changed_icons(mods: &Path) -> Vec<String> {
    let dir = mods.join(ICONS_MOD).join(ICON_DIRS[0]);
    let mut list: Vec<String> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().to_string_lossy().strip_suffix(".png").map(str::to_owned))
        .filter(|c| !c.contains('@'))
        .collect();
    list.sort();
    list
}

// ---------------------------------------------------------------------------
// Studio on Linux: Vinegar
// ---------------------------------------------------------------------------

/// How to start Vinegar (Studio for Linux), if it's installed: its own
/// program, or its Flatpak.
pub fn vinegar() -> Option<Vec<String>> {
    if cfg!(windows) {
        return None;
    }
    let on_path = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .map(|d| d.join("vinegar"))
        .find(|p| p.is_file());
    if let Some(program) = on_path {
        return Some(vec![program.to_string_lossy().into_owned()]);
    }
    let flatpak = std::process::Command::new("flatpak")
        .args(["info", "org.vinegarhq.Vinegar"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    flatpak.then(|| vec!["flatpak".to_owned(), "run".to_owned(), "org.vinegarhq.Vinegar".to_owned()])
}

/// The command that opens Studio through Vinegar (with a place file, if
/// given): `vinegar run [file]`.
pub fn vinegar_command(vinegar: &[String], place: Option<&Path>) -> Option<std::process::Command> {
    let (program, args) = vinegar.split_first()?;
    let mut command = std::process::Command::new(program);
    command.args(args).arg("run");
    if let Some(place) = place {
        command.arg(place);
    }
    Some(command)
}

/// The place being edited, from Studio's window title ("Baseplate - Roblox
/// Studio" → "Baseplate"). `None` at Studio's start page.
pub fn place_from_title(title: &str) -> Option<String> {
    let place = title.strip_suffix(" - Roblox Studio")?.trim();
    // Unsaved files are "Place1 @ 12 Nov 2026" and the like.
    let place = place.split(" @ ").next().unwrap_or(place).trim();
    (!place.is_empty() && place != "Roblox Studio").then(|| place.chars().take(100).collect())
}

/// The script open in Studio's editor, as Feline's Studio plugin reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenScript {
    pub name: String,
    /// "Script", "LocalScript" or "ModuleScript".
    pub class: String,
    pub lines: u32,
}

/// What Feline's Studio plugin last said.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PluginReport {
    /// The place's name (`game.Name`).
    pub place: Option<String>,
    pub script: Option<OpenScript>,
}

/// Plain, short text for a Discord status: no control characters.
fn short_text(value: Option<&Value>, max: usize) -> Option<String> {
    let text: String = value?.as_str()?.chars().filter(|c| !c.is_control()).take(max).collect();
    let text = text.trim().to_owned();
    (!text.is_empty()).then_some(text)
}

/// The plugin's message: `{"place": "Obby", "script": "Main", "class":
/// "Script", "lines": 240}` (the script fields only with a script open).
pub fn parse_report(body: &str) -> Option<PluginReport> {
    let v: Value = serde_json::from_str(body).ok()?;
    let v = v.as_object()?;
    let script = short_text(v.get("script"), 60).map(|name| OpenScript {
        name,
        class: match v.get("class").and_then(Value::as_str) {
            Some(c @ ("Script" | "LocalScript" | "ModuleScript")) => c.to_owned(),
            _ => "Script".to_owned(),
        },
        lines: v.get("lines").and_then(Value::as_u64).map_or(0, |n| n.min(9_999_999) as u32),
    });
    Some(PluginReport {
        place: short_text(v.get("place"), 100),
        script,
    })
}

/// The Discord status while building. With a script open (from Feline's
/// Studio plugin): "Editing Main (Script, 240 lines)".
pub fn activity(place: Option<&str>, script: Option<&OpenScript>, since: u64) -> Value {
    let building = match place {
        Some(p) => format!("Building {p}"),
        None => "In Roblox Studio".to_owned(),
    };
    let (details, state) = match script {
        Some(s) => {
            let kind = match s.class.as_str() {
                "LocalScript" => "local script",
                "ModuleScript" => "module",
                _ => "script",
            };
            let lines = match s.lines {
                0 => String::new(),
                1 => ", 1 line".to_owned(),
                n => format!(", {n} lines"),
            };
            (format!("Editing {} ({kind}{lines})", s.name), building)
        }
        None => (building, "in Roblox Studio".to_owned()),
    };
    serde_json::json!({
        "type": 0,
        "details": details,
        "state": state,
        "timestamps": { "start": since },
        "assets": { "large_image": "feline", "large_text": crate::presence::FELINE_LABEL },
    })
}

/// The port Feline's Studio plugin talks to, on this computer only.
pub const PLUGIN_PORT: u16 = 47821;
/// The plugin's file in Studio's Plugins folder.
pub const PLUGIN_FILE: &str = "FelineStudioStatus.rbxmx";

/// The plugin's code: every few seconds it tells Feline (on this computer,
/// nowhere else) the place's name and which script is open.
const PLUGIN_SOURCE: &str = r#"-- Feline's Studio status plugin (GPL-3.0-or-later).
-- Tells Feline, on this computer only, which place and script are open, for
-- your Discord status. Switch it off in Feline: Studio -> "Show the script
-- I'm editing". Studio asks once whether this plugin may talk to localhost.
local HttpService = game:GetService("HttpService")
local StudioService = game:GetService("StudioService")
local RunService = game:GetService("RunService")
local URL = "http://localhost:__PORT__/status"

-- Only the editing session reports (not the copies made for play tests).
if not RunService:IsEdit() then
	return
end

local function report()
	local info = { place = game.Name }
	local open = StudioService.ActiveScript
	if open then
		info.script = open.Name
		info.class = open.ClassName
		local ok, source = pcall(function()
			return open.Source
		end)
		if ok and type(source) == "string" then
			local _, breaks = string.gsub(source, "\n", "\n")
			info.lines = breaks + 1
		end
	end
	return HttpService:JSONEncode(info)
end

while true do
	pcall(function()
		HttpService:RequestAsync({
			Url = URL,
			Method = "POST",
			Headers = { ["Content-Type"] = "application/json" },
			Body = report(),
		})
	end)
	task.wait(5)
end
"#;

/// The plugin as a Roblox model file (`.rbxmx`, Roblox's XML format): one
/// Script, which Studio runs as a plugin from its Plugins folder.
pub fn plugin_file() -> String {
    let source = PLUGIN_SOURCE.replace("__PORT__", &PLUGIN_PORT.to_string());
    debug_assert!(!source.contains("]]>"));
    format!(
        "<roblox version=\"4\">\n\t<Item class=\"Script\" referent=\"RBX0\">\n\t\t<Properties>\n\
         \t\t\t<string name=\"Name\">FelineStudioStatus</string>\n\
         \t\t\t<ProtectedString name=\"Source\"><![CDATA[{source}]]></ProtectedString>\n\
         \t\t</Properties>\n\t</Item>\n</roblox>\n"
    )
}

/// Studio's Plugins folder (`%LOCALAPPDATA%\Roblox\Plugins`).
pub fn plugins_dir(local_app_data: &Path) -> PathBuf {
    local_app_data.join("Roblox").join("Plugins")
}

/// Puts Feline's plugin into Studio's Plugins folder (`on`), or takes it
/// out again.
pub fn set_plugin(plugins: &Path, on: bool) -> std::io::Result<()> {
    let file = plugins.join(PLUGIN_FILE);
    if on {
        fs::create_dir_all(plugins)?;
        let text = plugin_file();
        if fs::read_to_string(&file).is_ok_and(|old| old == text) {
            return Ok(());
        }
        fs::write(file, text)
    } else {
        match fs::remove_file(file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

/// The body of one `POST /status` request read from `stream`, if that's
/// what it is (at most 4 KiB; anything else is dropped).
fn read_report(stream: &mut impl std::io::Read) -> Option<String> {
    const MAX: usize = 4096;
    let mut data = Vec::new();
    let mut chunk = [0u8; 1024];
    let (head_end, length) = loop {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        data.extend_from_slice(&chunk[..n]);
        if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = std::str::from_utf8(&data[..end]).ok()?;
            let mut lines = head.split("\r\n");
            if !lines.next()?.starts_with("POST /status ") {
                return None;
            }
            let length: usize = lines
                .filter_map(|l| l.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, value)| value.trim().parse().ok())?;
            break (end + 4, length);
        }
        if data.len() > MAX {
            return None;
        }
    };
    if length > MAX {
        return None;
    }
    while data.len() < head_end + length {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        data.extend_from_slice(&chunk[..n]);
    }
    String::from_utf8(data[head_end..head_end + length].to_vec()).ok()
}

/// Listens for Feline's Studio plugin on this computer (127.0.0.1 only).
pub struct PluginListener {
    latest: std::sync::Arc<std::sync::Mutex<Option<(std::time::Instant, PluginReport)>>>,
    port: u16,
}

impl PluginListener {
    /// Starts listening on `port` (0: any free one, for tests).
    pub fn start(port: u16) -> std::io::Result<Self> {
        use std::io::Write;
        let listener = std::net::TcpListener::bind(("127.0.0.1", port))?;
        let port = listener.local_addr()?.port();
        let latest = std::sync::Arc::new(std::sync::Mutex::new(None));
        let shared = std::sync::Arc::clone(&latest);
        std::thread::Builder::new().name("feline-studio-plugin".into()).spawn(move || {
            for mut stream in listener.incoming().filter_map(Result::ok) {
                let local = stream.peer_addr().is_ok_and(|a| a.ip().is_loopback());
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(2)));
                let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(2)));
                let report = if local { read_report(&mut stream).and_then(|b| parse_report(&b)) } else { None };
                let answer = if report.is_some() { "204 No Content" } else { "400 Bad Request" };
                let _ = stream.write_all(format!("HTTP/1.1 {answer}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes());
                if let Some(report) = report {
                    *shared.lock().unwrap_or_else(|p| p.into_inner()) = Some((std::time::Instant::now(), report));
                }
            }
        })?;
        Ok(Self { latest, port })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// What the plugin said, if it spoke in the last `fresh` (it reports
    /// every 5 seconds while Studio is open).
    pub fn latest(&self, fresh: std::time::Duration) -> Option<PluginReport> {
        let latest = self.latest.lock().unwrap_or_else(|p| p.into_inner());
        latest.as_ref().filter(|(at, _)| at.elapsed() <= fresh).map(|(_, r)| r.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn studio_flags_take_any_flag_but_only_plain_ones() {
        let set = flags_from_json(r#"{"FFlagSomethingNew": true, "DFIntTaskSchedulerTargetFps": 240}"#).unwrap();
        assert_eq!(set.get("FFlagSomethingNew").map(String::as_str), Some("True"));
        assert_eq!(set.get("DFIntTaskSchedulerTargetFps").map(String::as_str), Some("240"));
        assert!(flags_from_json(r#"{"bad name!": "1"}"#).is_err());
        assert!(flags_from_json(r#"{"A": [1]}"#).is_err());
        assert!(flags_from_json("").unwrap().is_empty());
        assert_eq!(flags_from_json(&flags_json(&set)).unwrap(), set);
    }

    #[test]
    fn flags_go_in_and_come_out_again() {
        let dir = std::env::temp_dir().join(format!("feline-studio-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let file = dir.join("ClientSettings/ClientAppSettings.json");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "{\"Theirs\": \"1\"}").unwrap();
        let set = flags_from_json(r#"{"Mine": "2"}"#).unwrap();
        write_flags(&dir, &set).unwrap();
        assert!(fs::read_to_string(&file).unwrap().contains("Mine"));
        write_flags(&dir, &FlagSet::new()).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "{\"Theirs\": \"1\"}", "theirs is back");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn class_icons_are_listed_and_replaced() {
        let root = std::env::temp_dir().join(format!("feline-icons-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let studio = root.join("studio");
        let dark = studio.join(ICON_DIRS[0]);
        fs::create_dir_all(&dark).unwrap();
        let icon = crate::png::Rgba { width: 16, height: 16, pixels: vec![9; 16 * 16 * 4] };
        for name in ["Part.png", "Part@2x.png", "Model.png"] {
            fs::write(dark.join(name), crate::png::encode(&icon)).unwrap();
        }
        let classes: Vec<String> = class_icons(&studio).into_iter().map(|(c, _)| c).collect();
        assert_eq!(classes, ["Model", "Part"]);
        let mods = root.join("mods");
        let picture = crate::png::Rgba { width: 100, height: 100, pixels: vec![200; 100 * 100 * 4] };
        set_class_icon(&mods, &studio, "Part", &picture).unwrap();
        assert!(set_class_icon(&mods, &studio, "../x", &picture).is_err());
        assert_eq!(changed_icons(&mods), ["Part"]);
        let made = crate::png::decode(&fs::read(mods.join(ICONS_MOD).join(ICON_DIRS[1]).join("Part@2x.png")).unwrap()).unwrap();
        assert_eq!(made.width, 32, "2× is 32 where Studio has none to copy");
        clear_class_icon(&mods, "Part");
        assert!(changed_icons(&mods).is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn places_come_from_titles() {
        assert_eq!(place_from_title("Obby World - Roblox Studio").as_deref(), Some("Obby World"));
        assert_eq!(place_from_title("Place1 @ 12 Nov 2026 20:55 - Roblox Studio").as_deref(), Some("Place1"));
        assert_eq!(place_from_title("Roblox Studio"), None);
        assert_eq!(activity(Some("Obby"), None, 5)["details"], "Building Obby");
    }

    #[test]
    fn the_open_script_is_in_the_status() {
        let report = parse_report(r#"{"place":"Obby World","script":"Main\nLoop","class":"ModuleScript","lines":240}"#).unwrap();
        assert_eq!(report.place.as_deref(), Some("Obby World"));
        let script = report.script.clone().unwrap();
        assert_eq!(script, OpenScript { name: "MainLoop".into(), class: "ModuleScript".into(), lines: 240 });
        let a = activity(Some("Obby World"), Some(&script), 5);
        assert_eq!(a["details"], "Editing MainLoop (module, 240 lines)");
        assert_eq!(a["state"], "Building Obby World");
        // No script open, odd classes, junk.
        assert_eq!(parse_report(r#"{"place":"Obby"}"#).unwrap().script, None);
        assert_eq!(parse_report(r#"{"script":"X","class":"Weird"}"#).unwrap().script.unwrap().class, "Script");
        assert_eq!(parse_report("[1]"), None);
        assert_eq!(parse_report("nope"), None);
        let one = OpenScript { name: "A".into(), class: "LocalScript".into(), lines: 1 };
        assert_eq!(activity(None, Some(&one), 5)["details"], "Editing A (local script, 1 line)");
    }

    #[test]
    fn the_plugin_file_is_a_roblox_model() {
        let text = plugin_file();
        assert!(text.starts_with("<roblox version=\"4\">"));
        assert!(text.contains(&format!("http://localhost:{PLUGIN_PORT}/status")));
        assert!(!text.contains("__PORT__"));
        assert_eq!(text.matches("]]>").count(), 1);
        let dir = std::env::temp_dir().join(format!("feline-plugin-{}", std::process::id()));
        set_plugin(&dir, true).unwrap();
        assert_eq!(fs::read_to_string(dir.join(PLUGIN_FILE)).unwrap(), text);
        set_plugin(&dir, false).unwrap();
        assert!(!dir.join(PLUGIN_FILE).exists());
        set_plugin(&dir, false).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_plugin_is_heard_over_http() {
        use std::io::{Read, Write};
        let listener = PluginListener::start(0).unwrap();
        let fresh = std::time::Duration::from_secs(30);
        assert_eq!(listener.latest(fresh), None);
        let send = |request: &str| {
            let mut stream = std::net::TcpStream::connect(("127.0.0.1", listener.port())).unwrap();
            stream.write_all(request.as_bytes()).unwrap();
            let mut answer = String::new();
            let _ = stream.read_to_string(&mut answer);
            answer
        };
        let body = r#"{"place":"Obby","script":"Main","class":"Script","lines":3}"#;
        let answer = send(&format!("POST /status HTTP/1.1\r\nHost: localhost\r\ncontent-length: {}\r\n\r\n{body}", body.len()));
        assert!(answer.starts_with("HTTP/1.1 204"), "{answer}");
        let heard = listener.latest(fresh).unwrap();
        assert_eq!(heard.script.unwrap().lines, 3);
        // Other requests are refused and change nothing.
        assert!(send("GET / HTTP/1.1\r\nHost: localhost\r\n\r\n").starts_with("HTTP/1.1 400"));
        assert!(send("POST /status HTTP/1.1\r\nContent-Length: 999999\r\n\r\n").starts_with("HTTP/1.1 400"));
        assert_eq!(listener.latest(fresh).unwrap().place.as_deref(), Some("Obby"));
        assert_eq!(listener.latest(std::time::Duration::ZERO), None);
    }
}

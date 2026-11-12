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

/// The Discord status while building.
pub fn activity(place: Option<&str>, since: u64) -> Value {
    serde_json::json!({
        "type": 0,
        "details": match place {
            Some(p) => format!("Building {p}"),
            None => "In Roblox Studio".to_owned(),
        },
        "state": "in Roblox Studio",
        "timestamps": { "start": since },
        "assets": { "large_image": "feline", "large_text": crate::presence::FELINE_LABEL },
    })
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
        assert_eq!(activity(Some("Obby"), 5)["details"], "Building Obby");
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! The mod library: mods to download in one click.
//!
//! Feline's own list is `mod-library.json` in Feline's repository:
//!
//! ```json
//! {"mods": [{"id": "cat-sounds", "name": "Cat sounds", "author": "…",
//!   "description": "…", "thumbnail": "https://….png",
//!   "download": "https://….zip", "sha256": "…"}]}
//! ```
//!
//! Optionally, Froststrap's community list too (its plain mods: misc,
//! skyboxes, cursors, avatar editor backgrounds; not its colour templates
//! or launcher themes, which only Froststrap can use). Every download is
//! an ordinary shared mod `.zip`, added through the same checks
//! ([`crate::mods::import_zip`]): no programs, no scripts, no escaping the
//! mods folder.

use std::fs;
use std::path::Path;
use std::time::Duration;

use serde_json::Value;

/// Feline's own list.
pub const FELINE_LIST: &str = "https://raw.githubusercontent.com/TerrariaLoverForever/feline/main/mod-library.json";
/// Froststrap's community list (opt-in).
pub const FROSTSTRAP_LIST: &str = "https://raw.githubusercontent.com/Froststrap/config/refs/heads/main/Data.json";

/// The Feline Theme, built into Feline (no download).
const BUILTIN_THEME: &str = "builtin:feline-theme";

fn feline_theme() -> LibraryMod {
    LibraryMod {
        id: "feline-theme".into(),
        name: "The Feline Theme".into(),
        author: "Feline".into(),
        description: "Cat-shaped top bar icons for every game: a cat-head menu button, chat and backpack with ears, a paw for emotes, toe beans for \"more\".".into(),
        thumbnail: None,
        download: BUILTIN_THEME.into(),
        sha256: None,
        source: "Feline",
    }
}

/// The folder name a library mod gets in the mods folder.
pub fn folder_name(m: &LibraryMod) -> String {
    if m.download == BUILTIN_THEME { crate::mods::FELINE_THEME.to_owned() } else { crate::shortcut::file_name(&m.name) }
}

/// Bigger downloads are refused.
const MAX_DOWNLOAD: usize = 300 * 1024 * 1024;

/// One mod in the library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryMod {
    pub id: String,
    pub name: String,
    pub author: String,
    pub description: String,
    pub thumbnail: Option<String>,
    pub download: String,
    pub sha256: Option<String>,
    /// "Feline" or "Froststrap".
    pub source: &'static str,
}

fn agent(seconds: u64) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(seconds)))
        .user_agent(format!("Feline/{}", crate::VERSION))
        .build()
        .into()
}

fn https(url: &str) -> Option<String> {
    url.starts_with("https://").then(|| url.to_owned())
}

/// Reads one list. `froststrap`: Froststrap's format (only its plain mods).
pub fn parse_list(v: &Value, froststrap: bool) -> Vec<LibraryMod> {
    let key = if froststrap { "communityMods" } else { "mods" };
    v.get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|m| !froststrap || matches!(m.get("modtype").and_then(Value::as_u64), Some(0 | 2 | 3 | 4)))
        .filter_map(|m| {
            let text = |k: &str| m.get(k).and_then(Value::as_str).unwrap_or_default().chars().take(400).collect::<String>();
            Some(LibraryMod {
                id: m.get("id")?.as_str()?.to_owned(),
                name: m.get("name")?.as_str()?.chars().take(60).collect(),
                author: text("author"),
                description: text("description"),
                thumbnail: m.get("thumbnail").and_then(Value::as_str).and_then(https),
                download: m.get("download").and_then(Value::as_str).and_then(https)?,
                sha256: m
                    .get("sha256")
                    .and_then(Value::as_str)
                    .filter(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()))
                    .map(str::to_ascii_lowercase),
                source: if froststrap { "Froststrap" } else { "Feline" },
            })
        })
        .collect()
}

fn fetch_list(url: &str, froststrap: bool) -> Result<Vec<LibraryMod>, String> {
    let text = agent(15)
        .get(url)
        .call()
        .map_err(|e| match e {
            ureq::Error::StatusCode(404) => "the list isn't published yet".to_owned(),
            e => format!("it couldn't be reached ({e})"),
        })?
        .body_mut()
        .with_config()
        .limit(4 * 1024 * 1024)
        .read_to_string()
        .map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(&text).map_err(|_| "it isn't a mod list".to_owned())?;
    Ok(parse_list(&v, froststrap))
}

/// The library: Feline's list, and Froststrap's if wanted. Problems with
/// a list come back as lines for the user (the other list still shows).
pub fn load(with_froststrap: bool) -> (Vec<LibraryMod>, Vec<String>) {
    let mut mods = vec![feline_theme()];
    let mut problems = Vec::new();
    match fetch_list(FELINE_LIST, false) {
        Ok(list) => mods.extend(list),
        // Not published yet is normal for now; anything else is worth saying.
        Err(e) if e.contains("isn't published") => {}
        Err(e) => problems.push(format!("Feline's list: {e}")),
    }
    if with_froststrap {
        match fetch_list(FROSTSTRAP_LIST, true) {
            Ok(list) => mods.extend(list),
            Err(e) => problems.push(format!("Froststrap's list: {e}")),
        }
    }
    (mods, problems)
}

/// A mod's picture (cached in Feline's state folder).
pub fn thumbnail(m: &LibraryMod) -> Option<std::path::PathBuf> {
    if m.download == BUILTIN_THEME {
        let file = crate::paths::state_dir()?.join("library-thumbnails").join("Feline-feline-theme.png");
        if !file.is_file() {
            // The cat head in the middle of a Feline-purple card.
            let icon = crate::png::fit_square(&crate::png::decode(crate::mods::feline_theme_icon()?).ok()?, 84);
            let (w, h) = (320u32, 112u32);
            let mut card = crate::png::Rgba { width: w, height: h, pixels: [0x2a, 0x21, 0x48, 0xff].repeat((w * h) as usize) };
            let (ox, oy) = ((w - icon.width) / 2, (h - icon.height) / 2);
            for y in 0..icon.height {
                for x in 0..icon.width {
                    let from = ((y * icon.width + x) * 4) as usize;
                    let to = (((oy + y) * w + ox + x) * 4) as usize;
                    let a = u32::from(icon.pixels[from + 3]);
                    for c in 0..3 {
                        let blended = (u32::from(icon.pixels[from + c]) * a + u32::from(card.pixels[to + c]) * (255 - a)) / 255;
                        card.pixels[to + c] = blended as u8;
                    }
                }
            }
            fs::create_dir_all(file.parent()?).ok()?;
            fs::write(&file, crate::png::encode(&card)).ok()?;
        }
        return Some(file);
    }
    let url = m.thumbnail.as_ref()?;
    let id: String = m.id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(60).collect();
    let file = crate::paths::state_dir()?.join("library-thumbnails").join(format!("{}-{id}.png", m.source));
    if file.is_file() {
        return Some(file);
    }
    let data = agent(15).get(url).call().ok()?.body_mut().with_config().limit(4 * 1024 * 1024).read_to_vec().ok()?;
    // Only real pictures (PNG or JPEG).
    if !(data.starts_with(b"\x89PNG") || data.starts_with(&[0xFF, 0xD8])) {
        return None;
    }
    fs::create_dir_all(file.parent()?).ok()?;
    fs::write(&file, data).ok()?;
    Some(file)
}

/// Downloads a mod and adds it to `mods` (switched off; the caller turns
/// it on). Returns its name in the mods folder.
pub fn install(m: &LibraryMod, mods: &Path) -> Result<String, String> {
    if m.download == BUILTIN_THEME {
        crate::mods::install_feline_theme(mods).map_err(|e| e.to_string())?;
        return Ok(crate::mods::FELINE_THEME.to_owned());
    }
    let data = agent(600)
        .get(&m.download)
        .call()
        .map_err(|e| format!("the download didn't start ({e})"))?
        .body_mut()
        .with_config()
        .limit(MAX_DOWNLOAD as u64)
        .read_to_vec()
        .map_err(|e| format!("the download broke off ({e})"))?;
    if let Some(expected) = &m.sha256 {
        let hash = ring::digest::digest(&ring::digest::SHA256, &data);
        let hex: String = hash.as_ref().iter().map(|b| format!("{b:02x}")).collect();
        if &hex != expected {
            return Err("the download doesn't match the library's checksum, so Feline didn't use it".to_owned());
        }
    }
    let temp = crate::paths::state_dir()
        .ok_or("no folder for Feline's state")?
        .join(format!("library-{}.zip", std::process::id()));
    fs::create_dir_all(temp.parent().unwrap_or(mods)).map_err(|e| e.to_string())?;
    fs::write(&temp, &data).map_err(|e| e.to_string())?;
    // Named after the mod, not the download's file name.
    let named = temp.with_file_name(format!("{}.zip", crate::shortcut::file_name(&m.name)));
    let _ = fs::rename(&temp, &named);
    let result = crate::mods::import_zip(&named, mods);
    let _ = fs::remove_file(&named);
    let _ = fs::remove_file(&temp);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_are_read_and_froststrap_templates_left_out() {
        let ours: Value = serde_json::from_str(
            r#"{"mods": [{"id": "cats", "name": "Cat sounds", "author": "me", "description": "Meows.",
                "download": "https://example.com/cats.zip", "sha256": "ABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABAB"},
                {"id": "bad", "name": "Not https", "download": "http://example.com/x.zip"}]}"#,
        )
        .unwrap();
        let list = parse_list(&ours, false);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].sha256.as_deref(), Some("abababababababababababababababababababababababababababababababab"));
        assert_eq!(list[0].source, "Feline");
        let theirs: Value = serde_json::from_str(
            r#"{"communityMods": [
                {"id": "sky", "name": "Aurora", "download": "https://cdn.x/a.zip", "modtype": 2},
                {"id": "tint", "name": "Colour template", "download": "https://cdn.x/c.zip", "modtype": 1},
                {"id": "theme", "name": "Launcher theme", "download": "https://cdn.x/t.zip", "modtype": 5}]}"#,
        )
        .unwrap();
        let list = parse_list(&theirs, true);
        assert_eq!(list.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["Aurora"]);
    }
}

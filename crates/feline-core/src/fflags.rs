// SPDX-License-Identifier: GPL-3.0-or-later
//! FastFlags ("Tweaks"): the small set of Roblox client settings that Roblox
//! still lets players change locally, through `ClientAppSettings.json` in
//! the Roblox version folder.
//!
//! Since 29 September 2025 Roblox only reads the flags on its allowlist
//! (<https://devforum.roblox.com/t/3966569>): any other flag in the file is
//! ignored. Feline only ever writes flags from that list, with values it
//! checked, so nothing here can unlock anything Roblox doesn't allow.
//! Feline never touches `IxpSettings.json` (Roblox treats changing it as
//! cache poisoning).
//!
//! The flags are written each time a game starts (the version folder
//! changes with every Roblox update). A `ClientAppSettings.json` that
//! wasn't Feline's is kept aside and put back when every flag is off again.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// A set of flags: name → value as Roblox reads it ("True", "4", …).
pub type FlagSet = BTreeMap<String, String>;

/// What kind of value a flag takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// "True" or "False".
    Bool,
    /// A whole number in this range.
    Int(i64, i64),
}

/// One allowlisted flag.
#[derive(Debug, Clone, Copy)]
pub struct Flag {
    pub name: &'static str,
    pub kind: Kind,
}

/// Roblox's allowlist (as announced on 29 Sep 2025; it may change).
pub const ALLOWLIST: [Flag; 18] = [
    Flag { name: "DFIntCSGLevelOfDetailSwitchingDistance", kind: Kind::Int(0, 100_000) },
    Flag { name: "DFIntCSGLevelOfDetailSwitchingDistanceL12", kind: Kind::Int(0, 100_000) },
    Flag { name: "DFIntCSGLevelOfDetailSwitchingDistanceL23", kind: Kind::Int(0, 100_000) },
    Flag { name: "DFIntCSGLevelOfDetailSwitchingDistanceL34", kind: Kind::Int(0, 100_000) },
    Flag { name: "FFlagHandleAltEnterFullscreenManually", kind: Kind::Bool },
    Flag { name: "DFFlagTextureQualityOverrideEnabled", kind: Kind::Bool },
    Flag { name: "DFIntTextureQualityOverride", kind: Kind::Int(0, 3) },
    Flag { name: "FIntDebugForceMSAASamples", kind: Kind::Int(0, 8) },
    Flag { name: "DFFlagDisableDPIScale", kind: Kind::Bool },
    Flag { name: "FFlagDebugGraphicsPreferD3D11", kind: Kind::Bool },
    Flag { name: "FFlagDebugSkyGray", kind: Kind::Bool },
    Flag { name: "DFFlagDebugPauseVoxelizer", kind: Kind::Bool },
    Flag { name: "DFIntDebugFRMQualityLevelOverride", kind: Kind::Int(1, 21) },
    Flag { name: "FIntFRMMaxGrassDistance", kind: Kind::Int(0, 10_000) },
    Flag { name: "FIntFRMMinGrassDistance", kind: Kind::Int(0, 10_000) },
    Flag { name: "FFlagDebugGraphicsPreferVulkan", kind: Kind::Bool },
    Flag { name: "FFlagDebugGraphicsPreferOpenGL", kind: Kind::Bool },
    Flag { name: "FIntGrassMovementReducedMotionFactor", kind: Kind::Int(0, 100) },
];

pub fn flag(name: &str) -> Option<Flag> {
    ALLOWLIST.iter().copied().find(|f| f.name == name)
}

/// `value` tidied for `name` ("true" → "True", " 4 " → "4"), or `None` if
/// the flag isn't allowed or the value doesn't fit it.
pub fn checked(name: &str, value: &str) -> Option<String> {
    let value = value.trim();
    match flag(name)?.kind {
        Kind::Bool => match value.to_ascii_lowercase().as_str() {
            "true" | "1" | "on" => Some("True".to_owned()),
            "false" | "0" | "off" => Some("False".to_owned()),
            _ => None,
        },
        Kind::Int(min, max) => {
            let n: i64 = value.parse().ok()?;
            (min..=max).contains(&n).then(|| n.to_string())
        }
    }
}

/// Only the allowed flags with valid values.
pub fn sanitize(set: &FlagSet) -> FlagSet {
    set.iter()
        .filter_map(|(k, v)| checked(k, v).map(|v| (k.clone(), v)))
        .collect()
}

// ---------------------------------------------------------------------------
// The friendly choices (the Tweaks page), each a few flags
// ---------------------------------------------------------------------------

const D3D11: &str = "FFlagDebugGraphicsPreferD3D11";
const VULKAN: &str = "FFlagDebugGraphicsPreferVulkan";
const OPENGL: &str = "FFlagDebugGraphicsPreferOpenGL";
const MSAA: &str = "FIntDebugForceMSAASamples";
const TEX_ON: &str = "DFFlagTextureQualityOverrideEnabled";
const TEX: &str = "DFIntTextureQualityOverride";
const QUALITY: &str = "DFIntDebugFRMQualityLevelOverride";
const GRASS_MIN: &str = "FIntFRMMinGrassDistance";
const GRASS_MAX: &str = "FIntFRMMaxGrassDistance";
const GRASS_MOTION: &str = "FIntGrassMovementReducedMotionFactor";
const SKY: &str = "FFlagDebugSkyGray";
const FULLSCREEN: &str = "FFlagHandleAltEnterFullscreenManually";
const DPI: &str = "DFFlagDisableDPIScale";
const VOXELS: &str = "DFFlagDebugPauseVoxelizer";

/// Graphics API: Roblox decides, Direct3D 11, Vulkan, OpenGL.
pub const RENDERERS: [&str; 4] = ["Roblox decides", "Direct3D 11", "Vulkan", "OpenGL"];
/// Anti-aliasing samples: Roblox decides, then these.
pub const MSAA_STEPS: [u32; 4] = [1, 2, 4, 8];
/// Graphics quality levels Roblox's override takes (1 lowest, 21 highest).
pub const QUALITY_STEPS: [u32; 5] = [1, 5, 10, 15, 21];

/// The Tweaks page's choices, read from and written to a flag set. Index 0
/// is always "Roblox decides" (the flag isn't written).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Choices {
    pub renderer: usize,
    pub msaa: usize,
    /// 0 Roblox decides, 1 lowest … 4 highest (override levels 0–3).
    pub textures: usize,
    pub quality: usize,
    pub hide_grass: bool,
    pub still_grass: bool,
    pub gray_sky: bool,
    pub exclusive_fullscreen: bool,
    pub ignore_dpi: bool,
    pub pause_voxels: bool,
}

impl Choices {
    pub fn read(set: &FlagSet) -> Self {
        let is = |k: &str, v: &str| set.get(k).is_some_and(|x| x.eq_ignore_ascii_case(v));
        let int = |k: &str| set.get(k).and_then(|v| v.parse::<u32>().ok());
        Self {
            renderer: if is(D3D11, "True") {
                1
            } else if is(VULKAN, "True") {
                2
            } else if is(OPENGL, "True") {
                3
            } else {
                0
            },
            msaa: int(MSAA)
                .and_then(|n| MSAA_STEPS.iter().position(|s| *s == n))
                .map_or(0, |i| i + 1),
            textures: if is(TEX_ON, "True") {
                int(TEX).map_or(0, |n| n.min(3) as usize + 1)
            } else {
                0
            },
            quality: int(QUALITY)
                .and_then(|n| QUALITY_STEPS.iter().position(|s| *s == n))
                .map_or(0, |i| i + 1),
            hide_grass: int(GRASS_MAX) == Some(0) && int(GRASS_MIN) == Some(0),
            still_grass: int(GRASS_MOTION) == Some(0),
            gray_sky: is(SKY, "True"),
            exclusive_fullscreen: is(FULLSCREEN, "False"),
            ignore_dpi: is(DPI, "True"),
            pause_voxels: is(VOXELS, "True"),
        }
    }

    /// Writes these choices into `set`, leaving other flags alone.
    pub fn write(&self, set: &mut FlagSet) {
        let mut put = |k: &str, v: Option<String>| match v {
            Some(v) => {
                set.insert(k.to_owned(), v);
            }
            None => {
                set.remove(k);
            }
        };
        let t = || Some("True".to_owned());
        put(D3D11, (self.renderer == 1).then(t).flatten());
        put(VULKAN, (self.renderer == 2).then(t).flatten());
        put(OPENGL, (self.renderer == 3).then(t).flatten());
        put(MSAA, self.msaa.checked_sub(1).and_then(|i| MSAA_STEPS.get(i)).map(u32::to_string));
        put(TEX_ON, (self.textures > 0).then(t).flatten());
        put(TEX, self.textures.checked_sub(1).map(|i| i.min(3).to_string()));
        put(QUALITY, self.quality.checked_sub(1).and_then(|i| QUALITY_STEPS.get(i)).map(u32::to_string));
        let zero = || Some("0".to_owned());
        put(GRASS_MIN, self.hide_grass.then(zero).flatten());
        put(GRASS_MAX, self.hide_grass.then(zero).flatten());
        put(GRASS_MOTION, self.still_grass.then(zero).flatten());
        put(SKY, self.gray_sky.then(t).flatten());
        put(FULLSCREEN, self.exclusive_fullscreen.then(|| "False".to_owned()));
        put(DPI, self.ignore_dpi.then(t).flatten());
        put(VOXELS, self.pause_voxels.then(t).flatten());
    }
}

/// One-click starting points on the Tweaks page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    /// The most frames on a slow PC: lowest quality, no grass, grey sky.
    Potato,
    /// A middle ground.
    Balanced,
    /// Everything up.
    Pretty,
}

impl Preset {
    /// `choices` with the preset's graphics; the graphics API, full screen
    /// and display scaling stay as they were.
    pub fn apply(self, mut choices: Choices) -> Choices {
        let (quality, msaa, textures, sparse) = match self {
            Preset::Potato => (1, 1, 1, true),
            Preset::Balanced => (3, 2, 3, false),
            Preset::Pretty => (5, 4, 4, false),
        };
        choices.quality = quality;
        choices.msaa = msaa;
        choices.textures = textures;
        choices.hide_grass = sparse;
        choices.still_grass = sparse;
        choices.gray_sky = sparse;
        choices
    }
}

/// Reads FastFlags pasted or loaded as JSON (Bloxstrap's and Roblox's
/// `ClientAppSettings.json` format: `{"Name": value, …}`, values as strings,
/// numbers or booleans). Returns the allowed ones (checked and tidied) and
/// the names Roblox would ignore, which Feline leaves out.
pub fn from_json(text: &str) -> Result<(FlagSet, Vec<String>), String> {
    let value: Value = serde_json::from_str(text.trim().trim_start_matches('\u{feff}'))
        .map_err(|e| format!("that isn't FastFlag JSON ({e})"))?;
    let Value::Object(map) = value else {
        return Err("that isn't FastFlag JSON (it should look like {\"Name\": \"value\"})".to_owned());
    };
    let mut set = FlagSet::new();
    let mut ignored = Vec::new();
    for (name, value) in map {
        let text = match &value {
            Value::String(s) => s.clone(),
            Value::Bool(b) => b.to_string(),
            Value::Number(n) => n.to_string(),
            _ => String::new(),
        };
        match checked(&name, &text) {
            Some(v) => {
                set.insert(name, v);
            }
            None => ignored.push(name),
        }
    }
    Ok((set, ignored))
}

/// The flags as JSON, for copying out or sharing (same as the file).
pub fn to_json(set: &FlagSet) -> String {
    file_text(&sanitize(set))
}

// ---------------------------------------------------------------------------
// Profiles
// ---------------------------------------------------------------------------

fn profiles_path() -> Option<PathBuf> {
    crate::paths::state_dir().map(|d| d.join("fflag-profiles.json"))
}

/// Saved flag sets by name.
pub fn profiles() -> BTreeMap<String, FlagSet> {
    profiles_path()
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str::<BTreeMap<String, FlagSet>>(&t).ok())
        .map(|all| all.into_iter().map(|(k, v)| (k, sanitize(&v))).collect())
        .unwrap_or_default()
}

pub fn save_profiles(all: &BTreeMap<String, FlagSet>) -> std::io::Result<()> {
    let path = profiles_path().ok_or_else(|| std::io::Error::other("no state folder"))?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, serde_json::to_string_pretty(all)?)
}

/// The flags for a game: its own profile if it has one (and the profile
/// still exists), otherwise the everyday set.
pub fn for_game(settings: &crate::settings::Settings, place: Option<u64>) -> FlagSet {
    if let Some(name) = place.and_then(|p| settings.fflag_games.get(&p))
        && let Some(set) = profiles().remove(name)
    {
        return set;
    }
    sanitize(&settings.fflags)
}

// ---------------------------------------------------------------------------
// Writing the file
// ---------------------------------------------------------------------------

/// `ClientAppSettings.json` text for `set` (string values, like Roblox's
/// own examples and Bloxstrap).
pub fn file_text(set: &FlagSet) -> String {
    let map: Map<String, Value> = sanitize(set)
        .into_iter()
        .map(|(k, v)| (k, Value::String(v)))
        .collect();
    serde_json::to_string_pretty(&Value::Object(map)).unwrap_or_else(|_| "{}".to_owned())
}

/// Writes `set` into `version_dir`, keeping a file that wasn't Feline's in
/// `state` (and putting it back once `set` is empty). Returns what changed.
pub fn write_into(version_dir: &Path, state: &Path, set: &FlagSet) -> Result<Change, String> {
    let file = version_dir.join("ClientSettings").join("ClientAppSettings.json");
    let marker = state.join("fflags-written.txt");
    let backup = state.join("ClientAppSettings.original.json");
    let ours = fs::read_to_string(&marker)
        .is_ok_and(|p| Path::new(p.trim()) == file.as_path());
    let set = sanitize(set);
    if set.is_empty() {
        if !ours {
            return Ok(Change::None);
        }
        if backup.exists() {
            fs::copy(&backup, &file).map_err(|e| e.to_string())?;
            let _ = fs::remove_file(&backup);
        } else {
            let _ = fs::remove_file(&file);
        }
        let _ = fs::remove_file(&marker);
        return Ok(Change::Removed);
    }
    fs::create_dir_all(state).map_err(|e| e.to_string())?;
    if !ours && file.exists() && !backup.exists() {
        fs::copy(&file, &backup).map_err(|e| e.to_string())?;
    }
    let text = file_text(&set);
    if ours && fs::read_to_string(&file).is_ok_and(|t| t == text) {
        return Ok(Change::None);
    }
    fs::create_dir_all(file.parent().unwrap_or(version_dir)).map_err(|e| e.to_string())?;
    fs::write(&file, text).map_err(|e| e.to_string())?;
    fs::write(&marker, file.display().to_string()).map_err(|e| e.to_string())?;
    Ok(Change::Written(set.len()))
}

/// What [`write_into`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    None,
    Written(usize),
    Removed,
}

/// Before a game starts: writes the flags for this game into Roblox's
/// version folder (Roblox's official client only; other runtimes take their
/// flags in their own settings). Returns lines worth showing.
pub fn apply_for_launch(
    installation: &crate::runtime::Installation,
    ctx: &crate::runtime::DetectContext,
    settings: &crate::settings::Settings,
    place: Option<u64>,
) -> Vec<String> {
    if installation.backend.origin() != crate::runtime::Origin::Official {
        return Vec::new();
    }
    let set = for_game(settings, place);
    let Some(target) = installation.mod_target(ctx) else {
        return Vec::new();
    };
    let Some(state) = crate::paths::state_dir() else {
        return Vec::new();
    };
    if installation.is_running() && !set.is_empty() {
        return vec!["Tweaks weren't changed: Roblox is already running.".to_owned()];
    }
    match write_into(&target.dir, &state, &set) {
        Ok(Change::Written(n)) => vec![format!("Tweaks applied: {n} FastFlags.")],
        Ok(Change::Removed) => vec!["Tweaks are off: Roblox's own settings are back.".to_owned()],
        Ok(Change::None) => Vec::new(),
        Err(e) => vec![format!("Tweaks couldn't be applied ({e}); Roblox starts without them.")],
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn json_imports_keep_allowed_flags_and_name_the_rest() {
        let text = r#"{"FIntDebugForceMSAASamples": 4, "FFlagDebugSkyGray": true,
            "DFIntTaskSchedulerTargetFps": "9999", "FIntDebugForceMSAASamples2": "1",
            "DFIntTextureQualityOverride": "7"}"#;
        let (set, mut ignored) = from_json(text).unwrap();
        ignored.sort();
        assert_eq!(set.get("FIntDebugForceMSAASamples").map(String::as_str), Some("4"));
        assert_eq!(set.get("FFlagDebugSkyGray").map(String::as_str), Some("True"));
        assert_eq!(set.len(), 2);
        assert_eq!(ignored, ["DFIntTaskSchedulerTargetFps", "DFIntTextureQualityOverride", "FIntDebugForceMSAASamples2"]);
        assert_eq!(from_json(&to_json(&set)).unwrap().0, set);
        assert!(from_json("[1, 2]").is_err());
        assert!(from_json("nope").is_err());
    }

    #[test]
    fn presets_keep_the_graphics_api() {
        let mut c = Choices { renderer: 2, ..Choices::default() };
        c = Preset::Potato.apply(c);
        assert_eq!((c.renderer, c.quality, c.hide_grass), (2, 1, true));
        c = Preset::Pretty.apply(c);
        assert_eq!((c.quality, c.msaa, c.textures, c.hide_grass), (5, 4, 4, false));
    }

    use super::*;

    #[test]
    fn only_allowed_flags_with_valid_values_survive() {
        let set: FlagSet = [
            ("FIntDebugForceMSAASamples", " 4 "),
            ("DFFlagDebugPauseVoxelizer", "true"),
            ("DFIntTaskSchedulerTargetFps", "999"),
            ("DFIntTextureQualityOverride", "9"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
        let clean = sanitize(&set);
        assert_eq!(clean.len(), 2);
        assert_eq!(clean["FIntDebugForceMSAASamples"], "4");
        assert_eq!(clean["DFFlagDebugPauseVoxelizer"], "True");
    }

    #[test]
    fn choices_round_trip_through_flags() {
        let c = Choices {
            renderer: 2,
            msaa: 3,
            textures: 1,
            quality: 5,
            hide_grass: true,
            still_grass: true,
            gray_sky: true,
            exclusive_fullscreen: true,
            ignore_dpi: true,
            pause_voxels: false,
        };
        let mut set = FlagSet::new();
        set.insert("DFIntCSGLevelOfDetailSwitchingDistance".into(), "250".into());
        c.write(&mut set);
        assert_eq!(set[VULKAN], "True");
        assert_eq!(set[MSAA], "4");
        assert_eq!(set[TEX], "0");
        assert_eq!(set[QUALITY], "21");
        assert_eq!(set[FULLSCREEN], "False");
        assert_eq!(Choices::read(&set), c);
        // Other flags stay; "Roblox decides" removes the flags again.
        Choices::default().write(&mut set);
        assert_eq!(set.len(), 1);
        assert!(set.iter().all(|(k, v)| checked(k, v).as_ref() == Some(v)));
    }

    #[test]
    fn a_players_own_file_is_kept_and_put_back() {
        let root = std::env::temp_dir().join(format!("feline-fflags-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let version = root.join("version-1");
        let state = root.join("state");
        let file = version.join("ClientSettings/ClientAppSettings.json");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "{\"Theirs\": \"1\"}").unwrap();

        let mut set = FlagSet::new();
        set.insert(SKY.into(), "True".into());
        assert_eq!(write_into(&version, &state, &set), Ok(Change::Written(1)));
        assert!(fs::read_to_string(&file).unwrap().contains(SKY));
        assert_eq!(write_into(&version, &state, &set), Ok(Change::None));
        assert_eq!(write_into(&version, &state, &FlagSet::new()), Ok(Change::Removed));
        assert_eq!(fs::read_to_string(&file).unwrap(), "{\"Theirs\": \"1\"}");
        // Nothing of Feline's there: an empty set leaves the file alone.
        assert_eq!(write_into(&version, &state, &FlagSet::new()), Ok(Change::None));
        let _ = fs::remove_dir_all(&root);
    }
}

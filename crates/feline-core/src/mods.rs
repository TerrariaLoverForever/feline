// SPDX-License-Identifier: GPL-3.0-or-later
//! Mods: files that replace Roblox's own sounds, fonts, textures, cursors….
//!
//! # The format
//!
//! A mod is a folder laid out like Roblox's own files, the same layout as
//! Bloxstrap's `Modifications` folder (and its forks': Fishstrap, Froststrap,
//! Voidstrap), so their mods work unchanged:
//!
//! ```text
//! <mods folder>/My mod/content/sounds/ouch.ogg
//! <mods folder>/My mod/content/fonts/…
//! <mods folder>/My mod/PlatformContent/pc/textures/…
//! ```
//!
//! Feline's mods folder holds one folder per mod; each can be switched on and
//! off. When two mods change the same file, the later one (by name) wins.
//! `ClientSettings` folders are ignored: FastFlags aren't mods. Code is never
//! applied either ([`is_code`]): mods change how Roblox looks and sounds,
//! not what it does.
//!
//! Feline ships no mods and no Roblox files: mods are files you add.
//!
//! # Font patches
//!
//! Roblox draws its top bar and menu icons from an icon font. A mod can
//! restyle single icons without shipping Roblox's font: a font in its
//! `font-patches/` folder, at the same path as the Roblox font it patches
//! (e.g. `font-patches/ExtraContent/…/BuilderIcons-Regular.ttf`), holds
//! replacement glyphs named like Roblox's icons. When the mod is applied,
//! Feline makes a copy of the user's own Roblox font with those glyphs
//! swapped in ([`crate::fontpatch`]) and puts that copy in place, backed up
//! and restored like any other mod file.
//!
//! # Tints
//!
//! A mod with a `tint.json` (`{"color": "#CDB8FF"}`) recolours Roblox's UI
//! images ([`TINT_FOLDERS`]): grey and white pixels take the colour, colourful
//! ones stay ([`crate::png::tint`]). Like font patches, the recoloured copies
//! are made from the user's own files, never shipped. Files another enabled
//! mod replaces are left to that mod, so a tint and icon mods work together.
//!
//! # Applying ([`apply`])
//!
//! Before a game starts, Feline copies the enabled mods' files into the
//! runtime's [`ModTarget`]: Roblox's version folder on Windows, the unpacked
//! assets of Feline's own runtime, or Sober's asset overlay. Rules:
//! - Every original file is backed up (in Feline's state folder) before it's
//!   replaced, and put back as soon as no enabled mod replaces it.
//! - Only files Feline wrote are ever removed or restored, and only if they
//!   still are what Feline wrote (Roblox may have updated them since).
//! - In a real install folder (not an overlay), a mod file goes only where
//!   Roblox has a folder of that name: a mod can't add new top-level
//!   folders, and no path may leave the target (`..` is refused).
//! - Nothing is changed while the game runs (the caller checks).
//! - Nothing is copied again if nothing changed since the last start.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde_json::{Value, json};

use crate::paths;

/// Where a runtime's mods go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModTarget {
    pub dir: PathBuf,
    /// An overlay folder (Sober): starts empty and holds only mod files, so
    /// folders may be created in it. Otherwise the runtime's real files.
    pub overlay: bool,
}

/// Feline's mods folder: `~/.local/share/feline/mods`, on Windows
/// `%LOCALAPPDATA%\Feline\Mods`.
pub fn mods_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    return paths::state_dir().map(|d| d.join("Mods"));
    #[cfg(not(windows))]
    return paths::data_home().map(|d| d.join("feline").join("mods"));
}

/// One mod in Feline's mods folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModPack {
    /// The folder's name, shown to the user and saved when it's enabled.
    pub name: String,
    pub path: PathBuf,
    /// How many files it would apply.
    pub files: usize,
    /// What kinds of things it changes, e.g. "sounds, fonts".
    pub summary: String,
}

/// The mods in `dir`, sorted by name (the order they're applied in).
pub fn list_packs(dir: &Path) -> Vec<ModPack> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut packs: Vec<ModPack> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| {
            let files = mod_files(&e.path());
            let patches = font_patches(&e.path());
            let mut summary = summarize(files.keys());
            if !patches.is_empty() {
                summary = if summary.is_empty() {
                    "icons".to_owned()
                } else {
                    format!("icons, {summary}")
                };
            }
            if pack_tint(&e.path()).is_some() {
                summary = if summary.is_empty() {
                    "UI colour".to_owned()
                } else {
                    format!("UI colour, {summary}")
                };
            }
            ModPack {
                name: e.file_name().to_string_lossy().into_owned(),
                summary,
                files: files.len() + patches.len(),
                path: e.path(),
            }
        })
        .collect();
    packs.sort_by_key(|p| p.name.to_lowercase());
    packs
}

/// "sounds, fonts and 2 more"-style description of what files change.
fn summarize<'a>(paths: impl Iterator<Item = &'a String>) -> String {
    let mut kinds: Vec<&str> = Vec::new();
    for path in paths {
        let lower = path.to_ascii_lowercase();
        let kind = if lower.contains("/sounds/") || lower.ends_with(".ogg") || lower.ends_with(".mp3") {
            "sounds"
        } else if lower.contains("/fonts/") || lower.ends_with(".ttf") || lower.ends_with(".otf") {
            "fonts"
        } else if lower.contains("cursor") {
            "cursors"
        } else if lower.contains("/sky/") {
            "sky"
        } else if lower.contains("/textures/") || lower.ends_with(".png") || lower.ends_with(".dds") {
            "textures"
        } else if lower.contains("shaders") {
            "shaders"
        } else {
            "other files"
        };
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    kinds.join(", ")
}

/// A mod's files: relative path (with `/`) → file. Skips `ClientSettings`
/// and anything that isn't a plain relative path.
pub fn mod_files(root: &Path) -> BTreeMap<String, PathBuf> {
    let mut files = BTreeMap::new();
    let mut stack = vec![root.to_owned()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let Ok(kind) = entry.file_type() else { continue };
            let path = entry.path();
            if kind.is_dir() {
                stack.push(path);
            } else if kind.is_file()
                && let Some(rel) = relative(root, &path)
                && !rel.to_ascii_lowercase().starts_with("clientsettings/")
                && !rel.to_ascii_lowercase().starts_with("font-patches/")
                && rel != "tint.json"
                && rel != "recolour.json"
                && rel != "preview.png"
                && rel != "font.json"
                && !rel.starts_with(CURSOR_ANIMATION)
                && !is_code(&rel)
            {
                files.insert(rel, path);
            }
        }
    }
    files
}

/// A mod's font patches: the path of the Roblox font each one patches →
/// the patch font.
pub fn font_patches(root: &Path) -> BTreeMap<String, PathBuf> {
    let dir = root.join("font-patches");
    let mut patches = BTreeMap::new();
    let mut stack = vec![dir.clone()];
    while let Some(folder) = stack.pop() {
        let Ok(entries) = fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            match entry.file_type() {
                Ok(t) if t.is_dir() => stack.push(path),
                Ok(t) if t.is_file() => {
                    if let Some(rel) = relative(&dir, &path)
                        && rel.to_ascii_lowercase().ends_with(".ttf")
                    {
                        patches.insert(rel, path);
                    }
                }
                _ => {}
            }
        }
    }
    patches
}

/// Scripts and models (Roblox's `.rbxm`/`.rbxmx`, Lua source). Some theme
/// packs replace Roblox's own scripts (avatar animation, emotes); that
/// changes what the client does, which Feline doesn't do. Skipped.
pub fn is_code(rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    [".rbxm", ".rbxmx", ".lua", ".luau"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

/// `path` relative to `root` as `a/b/c`, if it's a plain path inside it.
fn relative(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let mut parts = Vec::new();
    for component in rel.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_str()?.to_owned()),
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

// ---------------------------------------------------------------------------
// Tints
// ---------------------------------------------------------------------------

/// Where Roblox keeps its UI images (Windows layout; other runtimes simply
/// don't have these folders, so tints do nothing there).
pub const TINT_FOLDERS: &[&str] = &[
    "content/textures/ui",
    "ExtraContent/textures/ui",
    "ExtraContent/LuaPackages/Packages/_Index/FoundationImages/FoundationImages/SpriteSheets",
    "PlatformContent/pc/textures/ui",
];

/// The colours offered for Feline's own tint mod.
pub const TINT_COLOURS: &[(&str, u32)] = &[
    ("Feline purple", 0xCDB8FF),
    ("Sakura pink", 0xFFC2E2),
    ("Mint", 0xBDF5DC),
    ("Ocean", 0xB8D8FF),
    ("Gold", 0xFFE3A8),
];

/// The name of Feline's own tint mod.
pub const FELINE_TINT: &str = "Feline Tint";

/// Groups of Roblox's UI images a tint can recolour, chosen separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    /// The icon sheets: top bar, menu and button icons.
    Icons,
    /// The Esc menu, settings, prompts, backpack, loading screen.
    Menus,
    Chat,
    /// Every other grey or white UI image.
    Other,
}

impl Part {
    pub const ALL: [Part; 4] = [Part::Icons, Part::Menus, Part::Chat, Part::Other];

    fn as_str(self) -> &'static str {
        match self {
            Part::Icons => "icons",
            Part::Menus => "menus",
            Part::Chat => "chat",
            Part::Other => "other",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Part::ALL.into_iter().find(|p| p.as_str() == text)
    }

    /// Which part a UI image (relative path) belongs to.
    pub fn of(rel: &str) -> Self {
        let lower = rel.to_ascii_lowercase();
        let has = |words: &[&str]| words.iter().any(|w| lower.contains(w));
        if has(&["/spritesheets/", "/imageset/", "/icons/", "topbar"]) {
            Part::Icons
        } else if has(&["chat"]) {
            Part::Chat
        } else if has(&[
            "ingamemenu", "/settings/", "/luaapp/", "errorprompt", "inspectmenu", "avatarexperience",
            "/lobby/", "loadingscreen", "/backpack/", "/emotes/", "/capture/",
        ]) {
            Part::Menus
        } else {
            Part::Other
        }
    }
}

/// What a tint mod recolours, and how (its `tint.json`):
///
/// ```json
/// {"color": "#CDB8FF", "color2": "#FFC2E2", "direction": "diagonal",
///  "parts": ["icons", "menus", "chat", "other"]}
/// ```
///
/// Only `color` is required: no `color2` is one colour, no `parts` is
/// everything (so older tint mods keep working).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tint {
    pub color: u32,
    /// The gradient's end colour; `None` for one colour.
    pub color2: Option<u32>,
    pub direction: crate::png::Direction,
    pub parts: Vec<Part>,
}

impl Tint {
    pub fn solid(color: u32) -> Self {
        Self {
            color,
            color2: None,
            direction: crate::png::Direction::Across,
            parts: Part::ALL.to_vec(),
        }
    }

    fn parse(text: &str) -> Option<Self> {
        let v: Value = serde_json::from_str(text).ok()?;
        let colour = |key: &str| {
            let hex = v.get(key)?.as_str()?.trim_start_matches('#');
            (hex.len() == 6).then(|| u32::from_str_radix(hex, 16).ok()).flatten()
        };
        let direction = match v.get("direction").and_then(Value::as_str) {
            Some("down") => crate::png::Direction::Down,
            Some("diagonal") => crate::png::Direction::Diagonal,
            _ => crate::png::Direction::Across,
        };
        let parts = match v.get("parts").and_then(Value::as_array) {
            Some(list) => list.iter().filter_map(|p| Part::parse(p.as_str()?)).collect(),
            None => Part::ALL.to_vec(),
        };
        Some(Self {
            color: colour("color")?,
            color2: colour("color2"),
            direction,
            parts,
        })
    }

    pub fn to_json(&self) -> String {
        let mut v = json!({ "color": format!("#{:06X}", self.color) });
        if let Some(c2) = self.color2 {
            v["color2"] = json!(format!("#{c2:06X}"));
            v["direction"] = json!(match self.direction {
                crate::png::Direction::Across => "across",
                crate::png::Direction::Down => "down",
                crate::png::Direction::Diagonal => "diagonal",
            });
        }
        if self.parts != Part::ALL {
            v["parts"] = json!(self.parts.iter().map(|p| p.as_str()).collect::<Vec<_>>());
        }
        serde_json::to_string_pretty(&v).unwrap_or_default() + "\n"
    }

    /// Recolours `image`; true if anything changed.
    fn apply(&self, image: &mut crate::png::Rgba) -> bool {
        match self.color2 {
            Some(to) => crate::png::tint_gradient(image, self.color, to, self.direction),
            None => crate::png::tint(image, self.color),
        }
    }

    /// Recolours one of Feline's own images (the Feline Theme's icons):
    /// their violet follows the tint too. True if anything changed.
    fn apply_theme(&self, image: &mut crate::png::Rgba) -> bool {
        let to = self.color2.unwrap_or(self.color);
        crate::png::tint_theme(image, self.color, to, self.direction)
    }
}

/// A mod's tint, if it has a `tint.json`.
pub fn pack_tint(root: &Path) -> Option<Tint> {
    Tint::parse(&fs::read_to_string(root.join("tint.json")).ok()?)
}

/// Makes (or changes) Feline's own tint mod.
pub fn install_tint(mods: &Path, tint: &Tint) -> io::Result<()> {
    let dir = mods.join(FELINE_TINT);
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("tint.json"), tint.to_json())
}

/// A strip of Roblox's own icons from `target` (a runtime's files), with
/// `tint` on them as it would be in the game, for the generator's preview.
/// `None` if this runtime has no icon sheets (only Roblox on Windows does).
/// With `theme` (the Feline Theme's folder, when it's on), its icons lead
/// the strip, recoloured the way the tint recolours them in the game.
pub fn tint_preview(target: &Path, tint: &Tint, theme: Option<&Path>) -> Option<crate::png::Rgba> {
    // Roblox's originals: where a mod (say, an earlier tint) is in place,
    // Feline's backup of the file.
    let state = state_for(target)?;
    let old = load_records(&state.join("applied.json"));
    let backups = state.join("backup");
    let mod_target = ModTarget { dir: target.to_owned(), overlay: false };
    let sheets = (1..=12).filter_map(|n| {
        let rel = format!("{}/img_set_2x_{n}.png", TINT_FOLDERS[2]);
        let data = fs::read(original_of(&mod_target, &old, &backups, &rel)).ok()?;
        crate::png::decode(&data).ok()
    });
    let recolour = tint.parts.contains(&Part::Icons);
    let mut icons: Vec<crate::png::Rgba> = theme
        .into_iter()
        .flat_map(|dir| THEME_PREVIEW.iter().map(move |name| dir.join(THEME_ICONS).join(name)))
        .filter_map(|path| crate::png::decode(&fs::read(path).ok()?).ok())
        .map(|mut icon| {
            if recolour {
                tint.apply_theme(&mut icon);
            }
            icon
        })
        .collect();
    let mut roblox = crate::png::pick_icons(sheets, 14 - icons.len());
    if roblox.is_empty() {
        return None; // not a runtime tints work on
    }
    if recolour {
        for icon in &mut roblox {
            tint.apply(icon);
        }
    }
    icons.append(&mut roblox);
    crate::png::strip_of(&icons)
}

/// Where the Feline Theme keeps its top bar icons, and the ones the
/// generator's preview shows.
const THEME_ICONS: &str = "content/textures/ui/TopBar";
const THEME_PREVIEW: &[&str] = &[
    "coloredlogo@2x.png",
    "chatOn@2x.png",
    "emotesOn@2x.png",
    "inventoryOn@2x.png",
    "leaderboardOn@2x.png",
    "moreOn@2x.png",
];

/// The UI images under the tint folders of `target` (relative paths).
fn tint_targets(target: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for folder in TINT_FOLDERS {
        let root = target.join(folder);
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else { continue };
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("png"))
                    && let Some(rel) = relative(target, &path)
                {
                    out.push(rel);
                }
            }
        }
    }
    out
}

/// Recoloured copies of the target's UI images in `generated`, for files no
/// other mod provides, and of the Feline Theme's images (`theme`, its folder
/// when it's on). Remembers what it made (`tint-cache.json`), so an
/// unchanged setup costs no image work at the next start.
fn tinted_files(
    target: &ModTarget,
    old: &[Record],
    backups: &Path,
    generated: &Path,
    tint: &Tint,
    theme: Option<&Path>,
    wanted: &mut BTreeMap<String, PathBuf>,
) {
    // Part of every image's cache key: a changed tint remakes them all.
    let spec = tint.to_json();
    let cache_file = generated.join("tint-cache.json");
    let cache: Value = fs::read_to_string(&cache_file)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(Value::Null);
    let mut new_cache = serde_json::Map::new();
    // The Feline Theme's own images (where no later mod replaces them).
    let from_theme = |wanted: &BTreeMap<String, PathBuf>, rel: &str| {
        theme.and_then(|dir| wanted.get(rel).filter(|p| p.starts_with(dir)).cloned())
    };
    let mut rels = tint_targets(&target.dir);
    let theme_rels: Vec<String> = wanted
        .keys()
        .filter(|rel| rel.to_ascii_lowercase().ends_with(".png") && from_theme(wanted, rel).is_some())
        .filter(|rel| TINT_FOLDERS.iter().any(|f| rel.starts_with(&format!("{f}/"))))
        .cloned()
        .collect();
    rels.extend(theme_rels.into_iter().filter(|rel| !target.dir.join(rel).is_file()));
    let mut jobs = Vec::new();
    for rel in rels {
        let themed = from_theme(wanted, &rel);
        if (wanted.contains_key(&rel) && themed.is_none()) || !tint.parts.contains(&Part::of(&rel)) {
            continue;
        }
        let original = themed.clone().unwrap_or_else(|| original_of(target, old, backups, &rel));
        let Some(source_stamp) = stamp(&original) else { continue };
        let out = generated.join(&rel);
        let key = json!([source_stamp.0, source_stamp.1, spec, themed.is_some()]);
        // Made before from this very original in this colour?
        match cache.get(&rel) {
            Some(Value::Bool(false)) if cache.get(format!("{rel}#key")) == Some(&key) => {
                new_cache.insert(rel.clone(), Value::Bool(false));
                new_cache.insert(format!("{rel}#key"), key);
                continue; // nothing to tint in it
            }
            Some(Value::Bool(true)) if cache.get(format!("{rel}#key")) == Some(&key) && out.is_file() => {
                new_cache.insert(rel.clone(), Value::Bool(true));
                new_cache.insert(format!("{rel}#key"), key);
                wanted.insert(rel, out);
                continue;
            }
            _ => {}
        }
        jobs.push(TintJob { rel, original, out, key, themed: themed.is_some() });
    }
    // The image work, on every core: decoding, recolouring and encoding
    // hundreds of PNGs is most of a tint's cost. The biggest first, so a
    // few huge pictures don't end up last on one core.
    jobs.sort_by_key(|job| std::cmp::Reverse(fs::metadata(&job.original).map_or(0, |m| m.len())));
    for (job, changed) in run_tint_jobs(&jobs, tint) {
        let Some(changed) = changed else { continue };
        if changed {
            wanted.insert(job.rel.clone(), job.out.clone());
        }
        new_cache.insert(job.rel.clone(), Value::Bool(changed));
        new_cache.insert(format!("{}#key", job.rel), job.key.clone());
    }
    let _ = fs::create_dir_all(generated);
    let _ = fs::write(cache_file, Value::Object(new_cache).to_string());
}

/// One image for [`tinted_files`] to recolour.
struct TintJob {
    rel: String,
    original: PathBuf,
    out: PathBuf,
    key: Value,
    themed: bool,
}

/// Recolours the jobs' images on all cores. Per job: whether it changed
/// (`None`: it couldn't be read or written).
fn run_tint_jobs<'a>(jobs: &'a [TintJob], tint: &Tint) -> Vec<(&'a TintJob, Option<bool>)> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let one = |job: &TintJob| -> Option<bool> {
        let data = fs::read(&job.original).ok()?;
        let mut image = crate::png::decode(&data).ok()?;
        let changed = if job.themed { tint.apply_theme(&mut image) } else { tint.apply(&mut image) };
        if changed {
            write_fresh(&job.out, &crate::png::encode(&image)).ok()?;
        }
        Some(changed)
    };
    let workers = std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(1, 16).min(jobs.len().max(1));
    let next = AtomicUsize::new(0);
    let mut results: Vec<Option<bool>> = vec![None; jobs.len()];
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(job) = jobs.get(i) else { break };
                        done.push((i, one(job)));
                    }
                    done
                })
            })
            .collect();
        for handle in handles {
            for (i, changed) in handle.join().unwrap_or_default() {
                results[i] = changed;
            }
        }
    });
    jobs.iter().zip(results).collect()
}

// ---------------------------------------------------------------------------
// Feline Picks: your own death sound, cursor and font
// ---------------------------------------------------------------------------

/// The name of the mod that holds the user's picks.
pub const FELINE_PICKS: &str = "Feline Picks";

const DEATH_SOUND: &str = "content/sounds/ouch.ogg";
const CURSORS: &[&str] = &[
    "content/textures/Cursors/KeyboardMouse/ArrowCursor.png",
    "content/textures/Cursors/KeyboardMouse/ArrowFarCursor.png",
];
const CUSTOM_FONT: &str = "content/fonts/CustomFont.ttf";
/// Roblox's colour emoji font.
const EMOJI_FONT: &str = "content/fonts/TwemojiMozilla.ttf";
/// An animated cursor's own file in the Picks mod (not a Roblox file):
/// `cursor-animation.gif` or `.ani`.
const CURSOR_ANIMATION: &str = "cursor-animation";

/// The pointer while shift lock holds the camera.
const SHIFT_LOCK: &str = "content/textures/MouseLockedCursor.png";

/// What can be picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    DeathSound,
    Cursor,
    Font,
    Emoji,
    ShiftLock,
}

/// Emoji sets Feline can put in for Roblox's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmojiSet {
    /// Windows' own emoji (Segoe UI Emoji), from this computer.
    Windows,
    /// Catmoji: Twemoji with cat faces, from Bloxstrap's emoji collection.
    Catmoji,
}

/// Catmoji, pinned to one commit of Bloxstrap's emoji collection and
/// checked against its SHA-256, so the download can't change under us.
const CATMOJI_URL: &str =
    "https://raw.githubusercontent.com/bloxstraplabs/rbxcustom-fontemojis/95b73e1c141592a268f847f976b7cf4afb092031/Catmoji.ttf";
const CATMOJI_SHA256: &str = "58d781ff4800ab10144a6fc0bb30479881f5048ecd57196148c9ab791fdab622";

/// Gets an emoji set's font (reads Windows' own, or downloads Catmoji).
pub fn emoji_font(set: EmojiSet) -> Result<Vec<u8>, String> {
    match set {
        EmojiSet::Windows => {
            let windows = std::env::var_os("WINDIR").map(PathBuf::from).ok_or("this isn't Windows")?;
            fs::read(windows.join("Fonts").join("seguiemj.ttf"))
                .map_err(|e| format!("Windows' emoji font couldn't be read ({e})"))
        }
        EmojiSet::Catmoji => {
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .timeout_global(Some(std::time::Duration::from_secs(60)))
                .user_agent(format!("Feline/{}", crate::VERSION))
                .build()
                .into();
            let bytes = agent
                .get(CATMOJI_URL)
                .call()
                .map_err(|e| format!("GitHub couldn't be reached ({e})"))?
                .body_mut()
                .with_config()
                .limit(16 * 1024 * 1024)
                .read_to_vec()
                .map_err(|e| e.to_string())?;
            let hash = ring::digest::digest(&ring::digest::SHA256, &bytes);
            let hex: String = hash.as_ref().iter().map(|b| format!("{b:02x}")).collect();
            if hex != CATMOJI_SHA256 {
                return Err("the download wasn't the expected Catmoji font, so Feline didn't use it".to_owned());
            }
            Ok(bytes)
        }
    }
}

/// Puts an emoji font (bytes of a `.ttf`) into the Feline Picks mod.
pub fn set_emoji(mods: &Path, font: &[u8]) -> Result<(), String> {
    if !(font.starts_with(&[0, 1, 0, 0]) || font.starts_with(b"OTTO") || font.starts_with(b"true")) {
        return Err("that isn't a font".to_owned());
    }
    let to = mods.join(FELINE_PICKS).join(EMOJI_FONT);
    fs::create_dir_all(to.parent().unwrap_or(mods)).map_err(|e| e.to_string())?;
    fs::write(to, font).map_err(|e| e.to_string())
}

/// The Picks mod's animated cursor file, if it has one.
pub fn animated_cursor(mods: &Path) -> Option<PathBuf> {
    ["gif", "ani"]
        .iter()
        .map(|ext| mods.join(FELINE_PICKS).join(format!("{CURSOR_ANIMATION}.{ext}")))
        .find(|p| p.is_file())
}

/// Puts `file` into the Feline Picks mod as the death sound (an `.ogg` or
/// `.mp3`), the cursor (any PNG, scaled to Roblox's 64×64) or the font
/// for every game text (a `.ttf`/`.otf`).
pub fn set_pick(mods: &Path, pick: Pick, file: &Path) -> Result<(), String> {
    let dir = mods.join(FELINE_PICKS);
    let ext = file
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let put = |rel: &str, bytes: &[u8]| -> Result<(), String> {
        let to = dir.join(rel);
        fs::create_dir_all(to.parent().unwrap_or(&dir)).map_err(|e| e.to_string())?;
        fs::write(to, bytes).map_err(|e| e.to_string())
    };
    let data = fs::read(file).map_err(|e| format!("couldn't read {}: {e}", file.display()))?;
    match pick {
        Pick::DeathSound => {
            if !matches!(ext.as_str(), "ogg" | "mp3") {
                return Err("pick an .ogg or .mp3 sound".to_owned());
            }
            if data.len() > 8 * 1024 * 1024 {
                return Err("that sound is over 8 MB; pick a short one".to_owned());
            }
            put(DEATH_SOUND, &data)
        }
        Pick::Cursor => {
            let animation = crate::cursor::load(&data)?;
            // A still picture for Roblox: the first frame, its tip in the
            // middle where Roblox puts the pointer. Plain PNGs keep
            // Feline's old way (scaled to fit, centred).
            let still = if data.starts_with(b"\x89PNG") && !animation.is_animated() {
                crate::png::fit_square(&animation.frames[0].image, 64)
            } else {
                animation.still_for_roblox(64)
            };
            let cursor = crate::png::encode(&still);
            CURSORS.iter().try_for_each(|rel| put(rel, &cursor))?;
            for ext in ["gif", "ani"] {
                let _ = fs::remove_file(dir.join(format!("{CURSOR_ANIMATION}.{ext}")));
            }
            if animation.is_animated() {
                let ext = if data.starts_with(b"GIF") { "gif" } else { "ani" };
                put(&format!("{CURSOR_ANIMATION}.{ext}"), &data)?;
            }
            Ok(())
        }
        Pick::Emoji => {
            if !matches!(ext.as_str(), "ttf" | "otf") {
                return Err("pick a .ttf emoji font".to_owned());
            }
            set_emoji(mods, &data)
        }
        Pick::ShiftLock => {
            let image = crate::cursor::load(&data)?;
            // Roblox's own is 32 × 32, centred on the screen's middle.
            put(SHIFT_LOCK, &crate::png::encode(&crate::png::fit_square(&image.frames[0].image, 32)))
        }
        Pick::Font => {
            if !matches!(ext.as_str(), "ttf" | "otf") || !(data.starts_with(&[0, 1, 0, 0]) || data.starts_with(b"OTTO")) {
                return Err("pick a .ttf or .otf font".to_owned());
            }
            put(CUSTOM_FONT, &data)?;
            fs::write(dir.join("font.json"), "{\"file\": \"CustomFont.ttf\"}\n").map_err(|e| e.to_string())
        }
    }
}

/// Puts one of Feline's classic arrows in as the cursor (white or black).
pub fn set_classic_cursor(mods: &Path, white: bool) -> Result<(), String> {
    let dir = mods.join(FELINE_PICKS);
    let cursor = crate::png::encode(&crate::cursor::classic(white).still_for_roblox(64));
    for rel in CURSORS {
        let to = dir.join(rel);
        fs::create_dir_all(to.parent().unwrap_or(&dir)).map_err(|e| e.to_string())?;
        fs::write(to, &cursor).map_err(|e| e.to_string())?;
    }
    for ext in ["gif", "ani"] {
        let _ = fs::remove_file(dir.join(format!("{CURSOR_ANIMATION}.{ext}")));
    }
    Ok(())
}

/// Takes a pick out of the Feline Picks mod again.
pub fn clear_pick(mods: &Path, pick: Pick) {
    let dir = mods.join(FELINE_PICKS);
    match pick {
        Pick::DeathSound => {
            let _ = fs::remove_file(dir.join(DEATH_SOUND));
        }
        Pick::Cursor => {
            CURSORS.iter().for_each(|rel| {
                let _ = fs::remove_file(dir.join(rel));
            });
            for ext in ["gif", "ani"] {
                let _ = fs::remove_file(dir.join(format!("{CURSOR_ANIMATION}.{ext}")));
            }
        }
        Pick::Emoji => {
            let _ = fs::remove_file(dir.join(EMOJI_FONT));
        }
        Pick::ShiftLock => {
            let _ = fs::remove_file(dir.join(SHIFT_LOCK));
        }
        Pick::Font => {
            let _ = fs::remove_file(dir.join(CUSTOM_FONT));
            let _ = fs::remove_file(dir.join("font.json"));
        }
    }
}

/// Which picks are set (death sound, cursor, font, emoji, shift lock).
pub fn picks(mods: &Path) -> [bool; 5] {
    let dir = mods.join(FELINE_PICKS);
    [
        dir.join(DEATH_SOUND).is_file(),
        dir.join(CURSORS[0]).is_file(),
        dir.join(CUSTOM_FONT).is_file(),
        dir.join(EMOJI_FONT).is_file(),
        dir.join(SHIFT_LOCK).is_file(),
    ]
}

// ---------------------------------------------------------------------------
// Cursor sets: saved cursor picks to switch between
// ---------------------------------------------------------------------------

fn cursor_sets_dir() -> Option<PathBuf> {
    crate::paths::state_dir().map(|d| d.join("cursor-sets"))
}

/// The cursor files of the Picks mod (still and animated), relative.
fn cursor_files(dir: &Path) -> Vec<String> {
    let mut files: Vec<String> = CURSORS.iter().map(|r| (*r).to_owned()).collect();
    files.extend(["gif", "ani"].map(|e| format!("{CURSOR_ANIMATION}.{e}")));
    files.retain(|r| dir.join(r).is_file());
    files
}

/// Saves the picked cursor under `name`.
pub fn save_cursor_set(mods: &Path, name: &str) -> Result<(), String> {
    let name = crate::shortcut::file_name(name);
    let picks = mods.join(FELINE_PICKS);
    let files = cursor_files(&picks);
    if files.is_empty() {
        return Err("pick a cursor first".to_owned());
    }
    let dest = cursor_sets_dir().ok_or("no folder for Feline's state")?.join(&name);
    let _ = fs::remove_dir_all(&dest);
    for rel in files {
        let to = dest.join(&rel);
        fs::create_dir_all(to.parent().unwrap_or(&dest)).map_err(|e| e.to_string())?;
        fs::copy(picks.join(&rel), to).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// The saved cursor sets, by name.
pub fn cursor_sets() -> Vec<String> {
    let mut names: Vec<String> = cursor_sets_dir()
        .and_then(|d| fs::read_dir(d).ok())
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Makes a saved set the picked cursor.
pub fn use_cursor_set(mods: &Path, name: &str) -> Result<(), String> {
    let source = cursor_sets_dir().ok_or("no folder for Feline's state")?.join(crate::shortcut::file_name(name));
    let files = cursor_files(&source);
    if files.is_empty() {
        return Err("that set is empty".to_owned());
    }
    clear_pick(mods, Pick::Cursor);
    let picks = mods.join(FELINE_PICKS);
    for rel in files {
        let to = picks.join(&rel);
        fs::create_dir_all(to.parent().unwrap_or(&picks)).map_err(|e| e.to_string())?;
        fs::copy(source.join(&rel), to).map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn delete_cursor_set(name: &str) {
    if let Some(dir) = cursor_sets_dir() {
        let _ = fs::remove_dir_all(dir.join(crate::shortcut::file_name(name)));
    }
}

// ---------------------------------------------------------------------------
// Colour pieces: the cursor, shift lock and emote wheel in a colour
// ---------------------------------------------------------------------------

/// The mod holding the colours (its `recolour.json`).
pub const FELINE_COLOURS: &str = "Feline Colours";

/// Something that can get its own colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Piece {
    Cursor,
    ShiftLock,
    EmoteWheel,
}

impl Piece {
    pub const ALL: [Piece; 3] = [Piece::Cursor, Piece::ShiftLock, Piece::EmoteWheel];

    fn key(self) -> &'static str {
        match self {
            Piece::Cursor => "cursor",
            Piece::ShiftLock => "shiftlock",
            Piece::EmoteWheel => "emotes",
        }
    }

    /// Roblox's images for it.
    fn files(self) -> &'static [&'static str] {
        match self {
            Piece::Cursor => CURSORS,
            Piece::ShiftLock => &[
                SHIFT_LOCK,
                "content/textures/ui/mouseLock_on.png",
                "content/textures/ui/mouseLock_on@2x.png",
                "content/textures/ui/mouseLock_off.png",
                "content/textures/ui/mouseLock_off@2x.png",
            ],
            Piece::EmoteWheel => &[
                "content/textures/ui/Emotes/Editor/Large/Wheel.png",
                "content/textures/ui/Emotes/Editor/Large/Wheel@2x.png",
                "content/textures/ui/Emotes/Editor/Large/Wheel@3x.png",
                "content/textures/ui/Emotes/Editor/Small/Wheel.png",
                "content/textures/ui/Emotes/Editor/Small/Wheel@2x.png",
                "content/textures/ui/Emotes/Editor/Small/Wheel@3x.png",
                "content/textures/ui/Emotes/Editor/TenFoot/Wheel.png",
                "content/textures/ui/Emotes/Editor/TenFoot/Wheel@2x.png",
                "content/textures/ui/Emotes/Editor/TenFoot/Wheel@3x.png",
                "content/textures/ui/Emotes/EmotesRadialIcon.png",
            ],
        }
    }
}

/// The colours set for each piece (in [`Piece::ALL`]'s order).
pub fn piece_colours(mods: &Path) -> [Option<u32>; 3] {
    let v: Value = fs::read_to_string(mods.join(FELINE_COLOURS).join("recolour.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(Value::Null);
    Piece::ALL.map(|p| {
        let hex = v.get(p.key())?.as_str()?.trim_start_matches('#');
        (hex.len() == 6).then(|| u32::from_str_radix(hex, 16).ok()).flatten()
    })
}

/// Sets (or with `None`, removes) a piece's colour.
pub fn set_piece_colour(mods: &Path, piece: Piece, colour: Option<u32>) -> io::Result<()> {
    let dir = mods.join(FELINE_COLOURS);
    let mut colours = piece_colours(mods);
    let i = Piece::ALL.iter().position(|p| *p == piece).unwrap_or(0);
    colours[i] = colour;
    let mut v = serde_json::Map::new();
    for (p, c) in Piece::ALL.iter().zip(colours) {
        if let Some(c) = c {
            v.insert(p.key().to_owned(), Value::from(format!("#{c:06X}")));
        }
    }
    if v.is_empty() {
        let _ = fs::remove_dir_all(&dir);
        return Ok(());
    }
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("recolour.json"), Value::Object(v).to_string())
}

/// Recoloured copies of the pieces' images: of the file another mod puts
/// there (say, your picked cursor), or else of Roblox's own.
fn recoloured_pieces(
    target: &ModTarget,
    old: &[Record],
    backups: &Path,
    generated: &Path,
    colours: [Option<u32>; 3],
    wanted: &mut BTreeMap<String, PathBuf>,
) {
    for (piece, colour) in Piece::ALL.iter().zip(colours) {
        let Some(colour) = colour else { continue };
        for rel in piece.files() {
            let source = match wanted.get(*rel) {
                Some(file) => file.clone(),
                None => original_of(target, old, backups, rel),
            };
            let Ok(data) = fs::read(&source) else { continue };
            let Ok(mut image) = crate::png::decode(&data) else { continue };
            if !crate::png::tint(&mut image, colour) {
                continue;
            }
            let out = generated.join("recolour").join(rel);
            let bytes = crate::png::encode(&image);
            if fs::read(&out).ok().as_deref() == Some(bytes.as_slice()) || write_fresh(&out, &bytes).is_ok() {
                wanted.insert((*rel).to_owned(), out);
            }
        }
    }
}

/// Roblox's font family files with every face pointed at the mod's font
/// (`font.json`), made from the target's own files (Bloxstrap's way of
/// changing the font of every game).
fn font_families(
    target: &ModTarget,
    old: &[Record],
    backups: &Path,
    generated: &Path,
    font_file: &str,
    wanted: &mut BTreeMap<String, PathBuf>,
) {
    let families = target.dir.join("content/fonts/families");
    let Ok(entries) = fs::read_dir(&families) else { return };
    for entry in entries.filter_map(Result::ok) {
        let Some(rel) = relative(&target.dir, &entry.path()) else { continue };
        if !rel.ends_with(".json") || wanted.contains_key(&rel) {
            continue;
        }
        let original = original_of(target, old, backups, &rel);
        let Ok(text) = fs::read_to_string(&original) else { continue };
        let Ok(mut v) = serde_json::from_str::<Value>(&text) else { continue };
        let Some(faces) = v.get_mut("faces").and_then(Value::as_array_mut) else { continue };
        for face in faces {
            face["assetId"] = Value::from(format!("rbxasset://fonts/{font_file}"));
        }
        let out = generated.join(&rel);
        let new = serde_json::to_string_pretty(&v).unwrap_or_default();
        if fs::read_to_string(&out).ok().as_deref() != Some(new.as_str()) && write_fresh(&out, new.as_bytes()).is_err() {
            continue;
        }
        wanted.insert(rel, out);
    }
}

/// The font file a mod's `font.json` names, if it has one.
fn pack_font(root: &Path) -> Option<String> {
    let v: Value = serde_json::from_str(&fs::read_to_string(root.join("font.json")).ok()?).ok()?;
    let file = v.get("file")?.as_str()?;
    // Only a plain file name in content/fonts.
    (!file.contains(['/', '\\']) && root.join("content/fonts").join(file).is_file()).then(|| file.to_owned())
}

// ---------------------------------------------------------------------------
// Conflicts, previews, mod profiles
// ---------------------------------------------------------------------------

/// Files that more than one of `packs` changes: path → the mods, in order
/// (the last one wins).
pub fn conflicts(packs: &[ModPack]) -> Vec<(String, Vec<String>)> {
    let mut by_file: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for pack in packs {
        for rel in mod_files(&pack.path).into_keys().chain(font_patches(&pack.path).into_keys()) {
            by_file.entry(rel).or_default().push(pack.name.clone());
        }
    }
    by_file.into_iter().filter(|(_, mods)| mods.len() > 1).collect()
}

/// A picture from the mod to show as its preview (the first image found).
pub fn preview_image(pack: &Path) -> Option<PathBuf> {
    let own = pack.join("preview.png");
    if own.is_file() {
        return Some(own);
    }
    mod_files(pack)
        .into_values()
        .find(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("png")))
}

/// A sound from the mod, to listen to (the death sound first).
pub fn preview_sound(pack: &Path) -> Option<PathBuf> {
    let files = mod_files(pack);
    files
        .get(DEATH_SOUND)
        .cloned()
        .or_else(|| {
            files.into_values().find(|p| {
                p.extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("ogg") || e.eq_ignore_ascii_case("mp3"))
            })
        })
}

fn mod_profiles_file() -> Option<PathBuf> {
    crate::paths::config_dir().map(|d| d.join("mod-profiles.json"))
}

/// Saved sets of enabled mods, by name.
pub fn mod_profiles() -> BTreeMap<String, Vec<String>> {
    mod_profiles_file()
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .map(|m| {
            m.into_iter()
                .map(|(name, list)| {
                    let mods = list
                        .as_array()
                        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                        .unwrap_or_default();
                    (name, mods)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Saves (`Some`) or deletes (`None`) a mod profile.
pub fn set_mod_profile(name: &str, mods: Option<&[String]>) -> io::Result<()> {
    let mut all = mod_profiles();
    match mods {
        Some(list) => {
            all.insert(name.trim().to_owned(), list.to_vec());
        }
        None => {
            all.remove(name);
        }
    }
    let path = mod_profiles_file().ok_or_else(|| io::Error::other("no settings folder"))?;
    fs::create_dir_all(path.parent().unwrap_or(&path))?;
    fs::write(path, serde_json::to_string_pretty(&json!(all))?)
}

// ---------------------------------------------------------------------------
// The Feline Theme (built in)
// ---------------------------------------------------------------------------

/// The name of Feline's own mod in the mods folder.
pub const FELINE_THEME: &str = "Feline Theme";

/// Feline-style icons for Roblox's top bar: a cat-head menu button, a chat
/// bubble with ears, a paw for emotes, a backpack with ears, a podium with a
/// star, toe beans for "more", and Feline's purple close button and icon
/// background. Drawn from scratch by `assets/mods/make_feline_theme.py`;
/// the paths and sizes are the ones Roblox uses for those icons.
const FELINE_THEME_FILES: &[(&str, &[u8])] = include!("../../../assets/mods/feline_theme_files.rs");

/// The Feline Theme's cat-head icon (its picture in the mod library).
pub fn feline_theme_icon() -> Option<&'static [u8]> {
    FELINE_THEME_FILES.iter().find(|(rel, _)| rel.ends_with("coloredlogo@3x.png")).map(|(_, b)| *b)
}

/// Writes the Feline Theme into `mods` (replacing an older copy of it).
pub fn install_feline_theme(mods: &Path) -> io::Result<()> {
    let dest = mods.join(FELINE_THEME);
    // Start clean, so files of an older version of the theme don't linger.
    if dest.exists() {
        fs::remove_dir_all(&dest)?;
    }
    for (rel, bytes) in FELINE_THEME_FILES {
        let path = dest.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, bytes)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Bloxstrap and its forks
// ---------------------------------------------------------------------------

/// A bootstrapper's mods folder found on this computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundBootstrapper {
    pub name: &'static str,
    pub dir: PathBuf,
    pub files: usize,
}

/// Bloxstrap-family bootstrappers whose `Modifications` folder Feline can
/// import. They're Windows programs; each keeps its folder in `%LOCALAPPDATA%`.
pub const BOOTSTRAPPERS: &[&str] = &["Bloxstrap", "Fishstrap", "Froststrap", "Voidstrap"];

/// The bootstrappers' mods folders that have files in them.
pub fn find_bootstrappers(local_app_data: Option<&Path>) -> Vec<FoundBootstrapper> {
    let Some(base) = local_app_data else {
        return Vec::new();
    };
    BOOTSTRAPPERS
        .iter()
        .filter_map(|&name| {
            let dir = base.join(name).join("Modifications");
            let files = mod_files(&dir).len();
            (files > 0).then_some(FoundBootstrapper { name, dir, files })
        })
        .collect()
}

/// Copies a folder (a bootstrapper's `Modifications`, or any mod folder) into
/// Feline's mods folder as a new mod named `name` (or `name 2`, … if taken).
/// Returns the new mod's name.
pub fn import(source: &Path, mods: &Path, name: &str) -> io::Result<String> {
    let files = mod_files(source);
    if files.is_empty() {
        return Err(io::Error::other("there are no mod files in it"));
    }
    let mut chosen = name.to_owned();
    let mut n = 2;
    while mods.join(&chosen).exists() {
        chosen = format!("{name} {n}");
        n += 1;
    }
    let dest = mods.join(&chosen);
    for (rel, file) in &files {
        let to = dest.join(rel);
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(file, &to)?;
    }
    Ok(chosen)
}

// ---------------------------------------------------------------------------
// Sharing mods as .zip files
// ---------------------------------------------------------------------------

/// Folders (and files) that sit at the top of a mod. A zip whose files are
/// all inside one other folder (`My mod/content/…`, `Modifications/…`) has
/// that folder taken off.
const MOD_TOP_LEVEL: &[&str] = &[
    "content",
    "extracontent",
    "platformcontent",
    "clientsettings",
    "font-patches",
    "tint.json",
    "font.json",
    "shaders",
    "ssl",
];

/// Programs and scripts a shared mod must never carry (a zip from a
/// stranger is the classic way to pass one along).
fn is_program(rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    [
        ".exe", ".dll", ".bat", ".cmd", ".com", ".scr", ".msi", ".ps1", ".vbs", ".js", ".jar", ".sh",
        ".lnk", ".url", ".reg",
    ]
    .iter()
    .any(|ext| lower.ends_with(ext))
}

/// Most a shared mod may hold, unpacked.
const ZIP_LIMIT: usize = 1024 * 1024 * 1024;

/// Packs a mod folder into a zip (in memory): everything Feline would use
/// from it, laid out like the folder, so Bloxstrap and its forks can use it
/// too (they read the same layout). Code files are left out.
pub fn export_zip(pack: &Path) -> Result<Vec<u8>, String> {
    let mut zip = crate::bugreport::Zip::new();
    let mut files = 0usize;
    let mut total = 0usize;
    let mut stack = vec![pack.to_owned()];
    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(&dir).map_err(|e| format!("couldn't read {} ({e})", dir.display()))?;
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                stack.push(path);
                continue;
            }
            let Some(rel) = relative(pack, &path) else { continue };
            if !kind.is_file() || is_code(&rel) || is_program(&rel) || rel.to_ascii_lowercase().starts_with("clientsettings/") {
                continue;
            }
            let data = fs::read(&path).map_err(|e| format!("couldn't read {rel} ({e})"))?;
            total += data.len();
            if total > ZIP_LIMIT {
                return Err("that mod is over 1 GB, too big to share as a zip".to_owned());
            }
            zip.add(&rel, &data);
            files += 1;
        }
    }
    if files == 0 {
        return Err("that mod has no files".to_owned());
    }
    Ok(zip.finish())
}

/// A zip entry's name as a safe relative path (`a/b.png`), or `None` for
/// folders, Mac clutter and anything that could leave the mod's folder.
fn zip_path(name: &str) -> Option<String> {
    let name = name.replace('\\', "/");
    if name.ends_with('/') || name.starts_with('/') || name.contains(':') {
        return None;
    }
    let parts: Vec<&str> = name.split('/').collect();
    let bad = |p: &&str| p.is_empty() || *p == "." || *p == ".." || *p == "__MACOSX" || *p == ".DS_Store";
    (!parts.iter().any(bad)).then(|| parts.join("/"))
}

/// Unpacks a shared mod (`.zip`) into Feline's mods folder as a new mod,
/// named after the folder inside the zip, or the zip itself. Returns its
/// name. Works for zips of a Bloxstrap `Modifications` folder too.
pub fn import_zip(file: &Path, mods: &Path) -> Result<String, String> {
    let size = fs::metadata(file).map_err(|e| e.to_string())?.len();
    if size > ZIP_LIMIT as u64 {
        return Err("that zip is over 1 GB".to_owned());
    }
    let data = fs::read(file).map_err(|e| format!("couldn't read it ({e})"))?;
    let entries = crate::engine::zip::entries(&data).ok_or("that isn't a zip file Feline can read")?;
    let mut files: Vec<(String, crate::engine::zip::Entry)> = entries
        .into_iter()
        .filter_map(|(name, entry)| Some((zip_path(&name)?, entry)))
        .collect();
    files.sort_by(|a, b| a.0.cmp(&b.0));

    // Take off folders everything is inside, unless they're Roblox's own.
    let mut inner_name: Option<String> = None;
    while let Some(first) = files.first().and_then(|(p, _)| p.split_once('/')).map(|(top, _)| top.to_owned()) {
        let all_inside = files.iter().all(|(p, _)| p.starts_with(&format!("{first}/")));
        if !all_inside || MOD_TOP_LEVEL.contains(&first.to_ascii_lowercase().as_str()) {
            break;
        }
        for (p, _) in &mut files {
            *p = p[first.len() + 1..].to_owned();
        }
        if !first.eq_ignore_ascii_case("Modifications") {
            inner_name = Some(first);
        }
    }
    files.retain(|(p, _)| !is_code(p) && !is_program(p) && !p.to_ascii_lowercase().starts_with("clientsettings/"));
    if files.is_empty() {
        return Err("there are no mod files in it".to_owned());
    }
    if files.iter().map(|(_, e)| e.size()).sum::<usize>() > ZIP_LIMIT {
        return Err("it unpacks to over 1 GB".to_owned());
    }

    let base = inner_name
        .or_else(|| file.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .map(|n| n.trim().replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_"))
        .filter(|n| !n.is_empty() && !n.starts_with('.'))
        .unwrap_or_else(|| "Shared mod".to_owned());
    let mut name = base.clone();
    let mut n = 2;
    while mods.join(&name).exists() {
        name = format!("{base} {n}");
        n += 1;
    }

    // Unpack beside the mods, then move into place: a bad entry halfway
    // leaves no half-made mod behind.
    let temp = mods.join(format!(".unpacking-{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp);
    let unpack = || -> Result<(), String> {
        for (rel, entry) in &files {
            let bytes = crate::engine::zip::read(&data, entry).map_err(|e| format!("{rel}: {e}"))?;
            let to = temp.join(rel);
            fs::create_dir_all(to.parent().unwrap_or(&temp)).map_err(|e| e.to_string())?;
            fs::write(&to, bytes).map_err(|e| format!("{rel}: {e}"))?;
        }
        fs::rename(&temp, mods.join(&name)).map_err(|e| e.to_string())
    };
    unpack().inspect_err(|_| {
        let _ = fs::remove_dir_all(&temp);
    })?;
    Ok(name)
}

/// Where an exported mod goes: the Downloads folder, as `<name>.zip`
/// (`<name> (2).zip`, … if taken).
pub fn export_path(name: &str) -> Option<PathBuf> {
    let dir = crate::bugreport::output_dir()?;
    let mut path = dir.join(format!("{name}.zip"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{name} ({n}).zip"));
        n += 1;
    }
    Some(path)
}

// ---------------------------------------------------------------------------
// Applying
// ---------------------------------------------------------------------------

/// What [`apply`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applied {
    /// Mod files now in place.
    pub files: usize,
    /// Original files put back (mods switched off since last time).
    pub restored: usize,
    /// Files skipped, with why (shown to the user, a few at most).
    pub skipped: Vec<String>,
    /// True if nothing needed doing.
    pub unchanged: bool,
}

/// Where Feline keeps backups and its record for one target folder.
/// Forgets what Feline applied to `target` (a folder that's being deleted).
pub fn forget_target(target: &Path) {
    if let Some(state) = state_for(target) {
        let _ = fs::remove_dir_all(state);
    }
}

fn state_for(target: &Path) -> Option<PathBuf> {
    // A stable short name for the folder (FNV-1a of its path).
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in target.to_string_lossy().bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    Some(paths::state_dir()?.join("mod-state").join(format!("{hash:016x}")))
}

/// Size and modification time, to tell whether a file is still the one
/// Feline wrote (or a mod file changed since).
fn stamp(path: &Path) -> Option<(u64, u64)> {
    let meta = fs::metadata(path).ok()?;
    let modified = meta
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some((meta.len(), modified))
}

/// One file Feline put into the target.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Record {
    path: String,
    /// The original was backed up (otherwise the file was new).
    backup: bool,
    /// What Feline wrote: size and time of the copy in the target.
    written: (u64, u64),
    /// The mod file it came from, and its size and time then.
    source: String,
    source_stamp: (u64, u64),
    /// The target's file is a hard link to `source` (the same file on disk,
    /// put in place without copying), not a copy. Only Feline's own
    /// generated files are linked: a mod's own files are always copied, so
    /// nothing written into Roblox's folder can reach the mod.
    linked: bool,
}

impl Record {
    /// The target's file is still the one Feline put there (not replaced
    /// by Roblox since).
    fn still_ours(&self, dest: &Path) -> bool {
        stamp(dest) == Some(self.written)
    }
}

/// Writes a generated file as a new file: one that's hard-linked into
/// Roblox must not change under Roblox's feet (it's linked again at the
/// next apply).
fn write_fresh(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let _ = fs::remove_file(path);
    fs::write(path, bytes)
}

/// Puts `source` at `dest` (which doesn't exist): with `link`, a hard link
/// where the system allows it (instant, and no extra disk space), else a
/// copy. Returns whether it's a link.
fn place(source: &Path, dest: &Path, link: bool) -> io::Result<bool> {
    if link && fs::hard_link(source, dest).is_ok() {
        return Ok(true);
    }
    fs::copy(source, dest).map(|_| false)
}

fn load_records(file: &Path) -> Vec<Record> {
    let Ok(text) = fs::read_to_string(file) else {
        return Vec::new();
    };
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    let pair = |v: &Value| Some((v.get(0)?.as_u64()?, v.get(1)?.as_u64()?));
    items
        .iter()
        .filter_map(|item| {
            Some(Record {
                path: item.get("path")?.as_str()?.to_owned(),
                backup: item.get("backup")?.as_bool()?,
                written: pair(item.get("written")?)?,
                source: item.get("source")?.as_str()?.to_owned(),
                source_stamp: pair(item.get("source_stamp")?)?,
                linked: item.get("linked").and_then(Value::as_bool).unwrap_or(false),
            })
        })
        .collect()
}

fn save_records(file: &Path, records: &[Record]) -> io::Result<()> {
    let items: Vec<Value> = records
        .iter()
        .map(|r| {
            json!({
                "path": r.path,
                "backup": r.backup,
                "written": [r.written.0, r.written.1],
                "source": r.source,
                "source_stamp": [r.source_stamp.0, r.source_stamp.1],
                "linked": r.linked,
            })
        })
        .collect();
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir)?;
    }
    let temp = file.with_extension("tmp");
    fs::write(&temp, serde_json::to_string_pretty(&Value::Array(items))?)?;
    fs::rename(temp, file)
}

/// Roblox's own version of `rel`: Feline's backup of it while a mod's copy
/// is in place, else the file itself.
fn original_of(target: &ModTarget, old: &[Record], backups: &Path, rel: &str) -> PathBuf {
    let dest = target.dir.join(rel);
    let ours_in_place = old
        .iter()
        .any(|r| r.path == rel && r.backup && stamp(&dest) == Some(r.written));
    if ours_in_place { backups.join(rel) } else { dest }
}

/// Makes the patched copy of Roblox's font `rel` in `generated`, from the
/// original (Feline's backup of it while a patched copy is in place, else the
/// file itself). Rewrites the copy only when it would change, so an
/// unchanged setup needs no copying at the next start. `None` if the target
/// has no such font.
fn patched_font(
    target: &ModTarget,
    old: &[Record],
    backups: &Path,
    generated: &Path,
    rel: &str,
    patch: &Path,
) -> Result<Option<PathBuf>, String> {
    let original = original_of(target, old, backups, rel);
    if !original.is_file() {
        return Ok(None);
    }
    let font = fs::read(&original).map_err(|e| e.to_string())?;
    let icons = fs::read(patch).map_err(|e| e.to_string())?;
    let (patched, replaced) = crate::fontpatch::replace_glyphs(&font, &icons)?;
    if replaced == 0 {
        return Err("none of its icons are in this Roblox's font".to_owned());
    }
    let out = generated.join(rel);
    if fs::read(&out).ok().as_deref() != Some(patched.as_slice()) {
        write_fresh(&out, &patched).map_err(|e| e.to_string())?;
    }
    Ok(Some(out))
}

/// Puts the enabled `packs` (in order; later ones win) into `target`, and
/// takes out what's no longer wanted. `packs` empty = restore everything.
pub fn apply(target: &ModTarget, packs: &[ModPack]) -> Result<Applied, String> {
    let state = state_for(&target.dir).ok_or("no folder for Feline's state")?;
    apply_with_state(target, packs, &state)
}

fn apply_with_state(target: &ModTarget, packs: &[ModPack], state: &Path) -> Result<Applied, String> {
    apply_with_options(target, packs, state, false)
}

/// [`apply_with_state`]; with `hide_cursor`, Roblox's arrow cursors are
/// see-through (Feline draws an animated one over the game instead).
/// Whether `dir` is the Android app's assets (`android/`, no
/// `PlatformContent/`).
fn is_android_layout(dir: &Path) -> bool {
    dir.join("android").is_dir() && !dir.join("PlatformContent").exists()
}

/// A mod's Windows path as the Android app has it.
fn android_path(rel: &str) -> String {
    match rel.strip_prefix("PlatformContent/pc/") {
        Some(rest) => format!("android/{rest}"),
        None => rel.to_owned(),
    }
}

fn apply_with_options(target: &ModTarget, packs: &[ModPack], state: &Path, hide_cursor: bool) -> Result<Applied, String> {
    let record_file = state.join("applied.json");
    let backups = state.join("backup");
    let old = load_records(&record_file);
    let mut result = Applied::default();

    // What should be in place now: relative path → mod file (later packs win).
    let mut wanted: BTreeMap<String, PathBuf> = BTreeMap::new();
    for pack in packs {
        wanted.extend(mod_files(&pack.path));
    }
    // Font patches: Roblox's own font with the mod's icons swapped in, made
    // in Feline's state folder and then put in place like a mod file.
    for pack in packs {
        for (rel, patch) in font_patches(&pack.path) {
            match patched_font(target, &old, &backups, &state.join("generated"), &rel, &patch) {
                Ok(Some(generated)) => {
                    wanted.insert(rel, generated);
                }
                Ok(None) => {} // this runtime has no such font
                Err(e) => {
                    if result.skipped.len() < 5 {
                        result.skipped.push(format!("{rel} ({e})"));
                    }
                }
            }
        }
    }
    // A custom font: the last enabled mod with a font.json.
    if !target.overlay
        && let Some(font) = packs.iter().rev().find_map(|p| pack_font(&p.path))
    {
        font_families(target, &old, &backups, &state.join("generated"), &font, &mut wanted);
    }
    // Tints: the last enabled tint mod's, on what no mod replaces.
    if !target.overlay
        && let Some(tint) = packs.iter().rev().find_map(|p| pack_tint(&p.path))
    {
        let theme = packs.iter().find(|p| p.name == FELINE_THEME).map(|p| p.path.as_path());
        tinted_files(target, &old, &backups, &state.join("generated"), &tint, theme, &mut wanted);
    }
    // Colour pieces: on top of whatever mod provides those images.
    if !target.overlay
        && let Some(pack) = packs.iter().find(|p| p.name == FELINE_COLOURS)
    {
        let colours = piece_colours(pack.path.parent().unwrap_or(&pack.path));
        recoloured_pieces(target, &old, &backups, &state.join("generated"), colours, &mut wanted);
    }
    if hide_cursor {
        let clear = state.join("generated").join("hidden-cursor.png");
        let png = crate::png::encode(&crate::png::Rgba { width: 64, height: 64, pixels: vec![0; 64 * 64 * 4] });
        if fs::read(&clear).ok().as_deref() == Some(png.as_slice()) || write_fresh(&clear, &png).is_ok() {
            for rel in CURSORS {
                wanted.insert((*rel).to_owned(), clear.clone());
            }
        }
    }
    // Roblox's Android app (Feline's runtime) keeps the PC client's
    // `PlatformContent/pc/` files in `android/` (sky, terrain, …).
    if !target.overlay && is_android_layout(&target.dir) {
        wanted = wanted.into_iter().map(|(rel, file)| (android_path(&rel), file)).collect();
    }
    // A real install: only into folders Roblox has.
    if !target.overlay {
        wanted.retain(|rel, _| {
            let top = rel.split('/').next().unwrap_or_default();
            let ok = target.dir.join(top).is_dir() && rel.contains('/');
            if !ok && result.skipped.len() < 5 {
                result.skipped.push(format!("{rel} (this Roblox has no such folder)"));
            }
            ok
        });
    }

    // Nothing changed since last time? Then don't copy anything.
    let same = old.len() == wanted.len()
        && old.iter().all(|r| {
            wanted.get(&r.path).map(|p| p.to_string_lossy().into_owned()) == Some(r.source.clone())
                && stamp(Path::new(&r.source)) == Some(r.source_stamp)
                && r.still_ours(&target.dir.join(&r.path))
        });
    if same {
        result.files = wanted.len();
        result.unchanged = true;
        return Ok(result);
    }

    // 1. Undo the last application (only files still as Feline wrote them).
    for record in &old {
        let dest = target.dir.join(&record.path);
        let backup = backups.join(&record.path);
        if record.still_ours(&dest) {
            // Removed first, never written through: it may be a link to a
            // mod's file.
            let _ = fs::remove_file(&dest);
            if record.backup && (fs::rename(&backup, &dest).is_ok() || fs::copy(&backup, &dest).is_ok()) {
                result.restored += 1;
            }
        }
        // Either restored, or Roblox replaced the file since: the backup is
        // stale either way.
        let _ = fs::remove_file(&backup);
    }
    if wanted.is_empty() {
        let _ = fs::remove_file(&record_file);
        let _ = fs::remove_dir_all(&backups);
        let _ = fs::remove_dir_all(state.join("generated"));
        return Ok(result);
    }

    // 2. Put the wanted files in place, backing up originals first.
    let generated_dir = state.join("generated");
    let mut records = Vec::new();
    for (rel, source) in &wanted {
        let dest = target.dir.join(rel);
        let had_original = dest.is_file();
        if had_original {
            let backup = backups.join(rel);
            if let Some(parent) = backup.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            // Moved aside (instant on the same drive), or copied and removed.
            if fs::rename(&dest, &backup).is_err() {
                fs::copy(&dest, &backup).map_err(|e| format!("couldn't back up {rel}: {e}"))?;
                fs::remove_file(&dest).map_err(|e| format!("couldn't replace {rel}: {e}"))?;
            }
        } else if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let linked = match place(source, &dest, source.starts_with(&generated_dir)) {
            Ok(linked) => linked,
            Err(e) => {
                // Put the original back rather than leave a hole.
                if had_original {
                    let backup = backups.join(rel);
                    let _ = fs::rename(&backup, &dest).or_else(|_| fs::copy(&backup, &dest).map(drop));
                }
                if result.skipped.len() < 5 {
                    result.skipped.push(format!("{rel} ({e})"));
                }
                continue;
            }
        };
        let (Some(written), Some(source_stamp)) = (stamp(&dest), stamp(source)) else {
            continue;
        };
        records.push(Record {
            path: rel.clone(),
            backup: had_original,
            written,
            source: source.to_string_lossy().into_owned(),
            source_stamp,
            linked,
        });
    }
    result.files = records.len();
    save_records(&record_file, &records).map_err(|e| e.to_string())?;
    Ok(result)
}

/// Before a game starts: applies the enabled mods for `installation`.
/// Returns lines worth showing (nothing if there was nothing to do). Never
/// fails the launch: mods that can't be applied are reported, and the game
/// starts with Roblox's own files.
pub fn apply_for_launch(
    installation: &crate::runtime::Installation,
    ctx: &crate::runtime::DetectContext,
    enabled: &[String],
    hide_cursor: bool,
) -> Vec<String> {
    let Some(target) = installation.mod_target(ctx) else {
        return if enabled.is_empty() {
            Vec::new()
        } else {
            vec![format!(
                "{} can't load mods, so it starts without them.",
                installation.backend.name()
            )]
        };
    };
    let packs: Vec<ModPack> = mods_dir()
        .map(|dir| list_packs(&dir))
        .unwrap_or_default()
        .into_iter()
        .filter(|p| enabled.contains(&p.name))
        .collect();
    // Nothing enabled and nothing applied before: don't even look.
    let Some(state) = state_for(&target.dir) else {
        return Vec::new();
    };
    if packs.is_empty() && !state.join("applied.json").exists() {
        return Vec::new();
    }
    if installation.is_running() {
        return vec![format!(
            "Mods weren't changed: {} is already running.",
            installation.backend.name()
        )];
    }
    match apply_with_options(&target, &packs, &state, hide_cursor) {
        Ok(applied) => {
            let mut lines = Vec::new();
            if !applied.unchanged && applied.files > 0 {
                lines.push(format!("Mods applied: {} files from {} mods.", applied.files, packs.len()));
            } else if !applied.unchanged && applied.restored > 0 {
                lines.push(format!("Mods removed: {} of Roblox's own files are back.", applied.restored));
            }
            if !applied.skipped.is_empty() {
                lines.push(format!("Some mod files were skipped: {}.", applied.skipped.join(", ")));
            }
            lines
        }
        Err(e) => vec![format!("Mods couldn't be applied ({e}); starting with Roblox's own files.")],
    }
}

/// Takes every mod out of `installation` now (the Mods page's "Restore
/// Roblox's files"). Returns how many originals came back.
pub fn restore_now(
    installation: &crate::runtime::Installation,
    ctx: &crate::runtime::DetectContext,
) -> Result<usize, String> {
    let target = installation
        .mod_target(ctx)
        .ok_or_else(|| format!("{} doesn't use mods", installation.backend.name()))?;
    if installation.is_running() {
        return Err(format!("{} is running. Close it first.", installation.backend.name()));
    }
    apply(&target, &[]).map(|a| a.restored)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("feline-test-mods-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn mods_replace_and_restore_originals() {
        let root = temp("apply");
        let roblox = root.join("roblox");
        write(&roblox.join("content/sounds/ouch.ogg"), "original oof");
        write(&roblox.join("content/fonts/a.ttf"), "original font");
        let mods = root.join("mods");
        write(&mods.join("A sounds/content/sounds/ouch.ogg"), "mod oof");
        write(&mods.join("A sounds/content/sounds/new.ogg"), "new sound");
        write(&mods.join("B fonts/content/fonts/a.ttf"), "mod font");
        write(&mods.join("B fonts/ClientSettings/ClientAppSettings.json"), "{}");
        write(&mods.join("B fonts/NoSuchFolder/x.png"), "x");
        write(&mods.join("B fonts/content/avatar/scripts/animate.rbxm"), "script");
        let state = root.join("state");
        let target = ModTarget {
            dir: roblox.clone(),
            overlay: false,
        };
        let read = |rel: &str| fs::read_to_string(roblox.join(rel)).unwrap_or_default();

        let packs = list_packs(&mods);
        assert_eq!(packs.len(), 2);
        assert_eq!(packs[0].summary, "sounds");

        let applied = apply_with_state(&target, &packs, &state).unwrap();
        assert_eq!(applied.files, 3);
        assert_eq!(read("content/sounds/ouch.ogg"), "mod oof");
        assert_eq!(read("content/sounds/new.ogg"), "new sound");
        assert_eq!(read("content/fonts/a.ttf"), "mod font");
        assert!(!roblox.join("ClientSettings").exists(), "FastFlags aren't mods");
        assert!(!roblox.join("NoSuchFolder").exists(), "no new top-level folders");

        // Again: nothing to do.
        assert!(apply_with_state(&target, &packs, &state).unwrap().unchanged);

        // Switch the sound mod off: the original comes back, the new file goes.
        let applied = apply_with_state(&target, &packs[1..], &state).unwrap();
        assert_eq!(applied.files, 1);
        assert_eq!(read("content/sounds/ouch.ogg"), "original oof");
        assert!(!roblox.join("content/sounds/new.ogg").exists());
        assert_eq!(read("content/fonts/a.ttf"), "mod font");

        // All off: everything is as it was.
        apply_with_state(&target, &[], &state).unwrap();
        assert_eq!(read("content/fonts/a.ttf"), "original font");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_roblox_update_isnt_undone() {
        let root = temp("update");
        let roblox = root.join("roblox");
        write(&roblox.join("content/sounds/ouch.ogg"), "original");
        let mods = root.join("mods");
        write(&mods.join("m/content/sounds/ouch.ogg"), "mod");
        let state = root.join("state");
        let target = ModTarget {
            dir: roblox.clone(),
            overlay: false,
        };
        apply_with_state(&target, &list_packs(&mods), &state).unwrap();
        // Roblox updates the file (a different size, so it's clearly new).
        write(&roblox.join("content/sounds/ouch.ogg"), "updated by Roblox");
        apply_with_state(&target, &[], &state).unwrap();
        assert_eq!(
            fs::read_to_string(roblox.join("content/sounds/ouch.ogg")).unwrap(),
            "updated by Roblox",
            "Feline's old backup must not overwrite Roblox's newer file"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn overlays_get_new_folders_and_imports_copy() {
        let root = temp("overlay");
        let source = root.join("Bloxstrap/Modifications");
        write(&source.join("content/textures/Cursors/KeyboardMouse/ArrowCursor.png"), "cursor");
        write(&source.join("ClientSettings/ClientAppSettings.json"), "{}");
        let found = find_bootstrappers(Some(&root));
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].name, found[0].files), ("Bloxstrap", 1));

        let mods = root.join("mods");
        assert_eq!(import(&source, &mods, "From Bloxstrap").unwrap(), "From Bloxstrap");
        assert_eq!(import(&source, &mods, "From Bloxstrap").unwrap(), "From Bloxstrap 2");
        let packs = list_packs(&mods);
        assert_eq!(packs[0].summary, "cursors");

        let overlay = ModTarget {
            dir: root.join("overlay"),
            overlay: true,
        };
        let applied = apply_with_state(&overlay, &packs[..1], &root.join("state")).unwrap();
        assert_eq!(applied.files, 1);
        assert!(root.join("overlay/content/textures/Cursors/KeyboardMouse/ArrowCursor.png").is_file());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn the_feline_theme_installs_as_a_normal_mod() {
        let root = temp("theme");
        install_feline_theme(&root).unwrap();
        let packs = list_packs(&root);
        assert_eq!(packs.len(), 1);
        assert_eq!(packs[0].name, FELINE_THEME);
        assert_eq!(packs[0].files, FELINE_THEME_FILES.len());
        // Every image is a real PNG, every icon font a TrueType font.
        for (rel, bytes) in FELINE_THEME_FILES {
            if rel.ends_with(".png") {
                assert!(bytes.starts_with(b"\x89PNG"), "{rel}");
            } else {
                assert!(rel.ends_with(".ttf") && bytes.starts_with(&[0, 1, 0, 0]), "{rel}");
            }
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn font_patches_swap_icons_and_come_off_again() {
        let root = temp("fontpatch");
        let rel = "ExtraContent/LuaPackages/Packages/_Index/BuilderIcons/BuilderIcons/Font/BuilderIcons-Regular.ttf";
        // A stand-in for Roblox's font: Feline's own filled icons.
        let original: &[u8] = FELINE_THEME_FILES
            .iter()
            .find(|(p, _)| p.ends_with("BuilderIcons-Filled.ttf"))
            .unwrap()
            .1;
        let roblox = root.join("roblox");
        fs::create_dir_all(roblox.join(rel).parent().unwrap()).unwrap();
        fs::write(roblox.join(rel), original).unwrap();
        let mods = root.join("mods");
        install_feline_theme(&mods).unwrap();
        let target = ModTarget {
            dir: roblox.clone(),
            overlay: false,
        };
        let state = root.join("state");
        let packs = list_packs(&mods);
        assert!(packs[0].summary.starts_with("icons"), "{}", packs[0].summary);

        apply_with_state(&target, &packs, &state).unwrap();
        let patched = fs::read(roblox.join(rel)).unwrap();
        assert_ne!(patched, original, "the icons were swapped");
        // Again: the same patched font, nothing to do.
        assert!(apply_with_state(&target, &packs, &state).unwrap().unchanged);
        // Off: Roblox's font is back, byte for byte.
        apply_with_state(&target, &[], &state).unwrap();
        assert_eq!(fs::read(roblox.join(rel)).unwrap(), original);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn tints_recolour_grey_ui_and_leave_other_mods_alone() {
        let root = temp("tint");
        let roblox = root.join("roblox");
        let ui = roblox.join("content/textures/ui");
        fs::create_dir_all(ui.join("TopBar")).unwrap();
        let white = crate::png::Rgba { width: 1, height: 1, pixels: vec![255, 255, 255, 255] };
        let red = crate::png::Rgba { width: 1, height: 1, pixels: vec![255, 0, 0, 255] };
        fs::write(ui.join("panel.png"), crate::png::encode(&white)).unwrap();
        fs::write(ui.join("badge.png"), crate::png::encode(&red)).unwrap();
        fs::write(ui.join("TopBar/chatOff.png"), crate::png::encode(&white)).unwrap();
        let mods = root.join("mods");
        install_tint(&mods, &Tint::solid(0xCDB8FF)).unwrap();
        // Another mod replaces one of the images.
        write(&mods.join("Icons/content/textures/ui/TopBar/chatOff.png"), "icon mod");
        let target = ModTarget { dir: roblox.clone(), overlay: false };
        let state = root.join("state");
        let packs = list_packs(&mods);
        assert!(packs.iter().any(|p| p.summary == "UI colour"));

        apply_with_state(&target, &packs, &state).unwrap();
        let pixel = |p: &str| crate::png::decode(&fs::read(ui.join(p)).unwrap()).unwrap().pixels;
        assert_eq!(pixel("panel.png"), [205, 184, 255, 255], "white became lavender");
        assert_eq!(pixel("badge.png"), [255, 0, 0, 255], "red left alone (not replaced)");
        assert_eq!(fs::read_to_string(ui.join("TopBar/chatOff.png")).unwrap(), "icon mod");
        // Unchanged setup: nothing to do.
        assert!(apply_with_state(&target, &packs, &state).unwrap().unchanged);
        // Off: the originals are back.
        apply_with_state(&target, &[], &state).unwrap();
        assert_eq!(pixel("panel.png"), [255, 255, 255, 255]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn tints_recolour_the_feline_theme_too() {
        let root = temp("tint-theme");
        let roblox = root.join("roblox");
        let ui = roblox.join("content/textures/ui");
        fs::create_dir_all(ui.join("TopBar")).unwrap();
        let white = crate::png::Rgba { width: 1, height: 1, pixels: vec![255, 255, 255, 255] };
        fs::write(ui.join("TopBar/chatOff.png"), crate::png::encode(&white)).unwrap();
        let mods = root.join("mods");
        install_feline_theme(&mods).unwrap();
        install_tint(&mods, &Tint::solid(0xBDF5DC)).unwrap(); // mint
        let target = ModTarget { dir: roblox.clone(), overlay: false };
        let state = root.join("state");
        let packs = list_packs(&mods);
        apply_with_state(&target, &packs, &state).unwrap();

        // The theme's violet "on" chat icon now leans mint, not purple.
        let image = crate::png::decode(&fs::read(ui.join("TopBar/chatOn@2x.png")).unwrap()).unwrap();
        let (mut greenish, mut violet) = (0, 0);
        for px in image.pixels.as_chunks::<4>().0.iter().filter(|p| p[3] > 200) {
            let (r, g, b) = (px[0] as i32, px[1] as i32, px[2] as i32);
            if g > r + 20 && g >= b - 10 {
                greenish += 1;
            }
            if b > g + 40 && b > r + 40 {
                violet += 1;
            }
        }
        assert!(greenish > 0 && violet == 0, "{greenish} mint, {violet} violet");
        // Without the tint, the theme's own icon comes back.
        let theme_only: Vec<_> = packs.into_iter().filter(|p| p.name == FELINE_THEME).collect();
        apply_with_state(&target, &theme_only, &state).unwrap();
        let theme_file = mods.join(FELINE_THEME).join("content/textures/ui/TopBar/chatOn@2x.png");
        assert_eq!(fs::read(ui.join("TopBar/chatOn@2x.png")).unwrap(), fs::read(theme_file).unwrap());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn picks_make_a_normal_mod_and_fonts_redirect_every_family() {
        let root = temp("picks");
        let roblox = root.join("roblox");
        write(&roblox.join("content/sounds/ouch.ogg"), "oof");
        write(
            &roblox.join("content/fonts/families/Arial.json"),
            r#"{"name": "Arial", "faces": [{"name": "Regular", "assetId": "rbxasset://fonts/arial.ttf"}]}"#,
        );
        fs::create_dir_all(roblox.join("content/textures/Cursors/KeyboardMouse")).unwrap();
        let mods = root.join("mods");
        let sound = root.join("meow.ogg");
        fs::write(&sound, "meow").unwrap();
        let cursor = root.join("paw.png");
        let big = crate::png::Rgba { width: 128, height: 96, pixels: vec![200; 128 * 96 * 4] };
        fs::write(&cursor, crate::png::encode(&big)).unwrap();
        let font = root.join("Cat.ttf");
        fs::write(&font, [0, 1, 0, 0, 9, 9]).unwrap();
        set_pick(&mods, Pick::DeathSound, &sound).unwrap();
        set_pick(&mods, Pick::Cursor, &cursor).unwrap();
        set_pick(&mods, Pick::Font, &font).unwrap();
        assert!(set_pick(&mods, Pick::Font, &sound).is_err(), "not a font");
        let emoji = root.join("Cats.ttf");
        fs::write(&emoji, [0, 1, 0, 0, 7, 7]).unwrap();
        set_pick(&mods, Pick::Emoji, &emoji).unwrap();
        assert_eq!(picks(&mods), [true, true, true, true, false]);
        assert_eq!(animated_cursor(&mods), None, "a PNG is a still cursor");

        let target = ModTarget { dir: roblox.clone(), overlay: false };
        apply_with_state(&target, &list_packs(&mods), &root.join("state")).unwrap();
        assert_eq!(fs::read_to_string(roblox.join("content/sounds/ouch.ogg")).unwrap(), "meow");
        let arrow = crate::png::decode(&fs::read(roblox.join(CURSORS[0])).unwrap()).unwrap();
        assert_eq!((arrow.width, arrow.height), (64, 64));
        let family = fs::read_to_string(roblox.join("content/fonts/families/Arial.json")).unwrap();
        assert!(family.contains("rbxasset://fonts/CustomFont.ttf"), "{family}");
        assert!(roblox.join("content/fonts/CustomFont.ttf").is_file());

        assert_eq!(fs::read(roblox.join(EMOJI_FONT)).unwrap(), [0, 1, 0, 0, 7, 7]);

        clear_pick(&mods, Pick::Font);
        apply_with_state(&target, &list_packs(&mods), &root.join("state")).unwrap();
        let family = fs::read_to_string(roblox.join("content/fonts/families/Arial.json")).unwrap();
        assert!(family.contains("arial.ttf"), "the original came back: {family}");

        // An animated cursor: its file stays in the mod (not put in
        // Roblox), Roblox gets its first frame, or nothing while Feline
        // draws the animation.
        let gif = root.join("spin.gif");
        fs::write(&gif, crate::cursor::tests_gif()).unwrap();
        set_pick(&mods, Pick::Cursor, &gif).unwrap();
        assert_eq!(animated_cursor(&mods), Some(mods.join(FELINE_PICKS).join("cursor-animation.gif")));
        assert!(!mod_files(&mods.join(FELINE_PICKS)).keys().any(|k| k.starts_with("cursor-animation")));
        let state = root.join("state");
        apply_with_options(&target, &list_packs(&mods), &state, true).unwrap();
        let arrow = crate::png::decode(&fs::read(roblox.join(CURSORS[0])).unwrap()).unwrap();
        assert!(arrow.pixels.iter().all(|&b| b == 0), "see-through while Feline draws it");
        apply_with_state(&target, &list_packs(&mods), &state).unwrap();
        let arrow = crate::png::decode(&fs::read(roblox.join(CURSORS[0])).unwrap()).unwrap();
        assert!(arrow.pixels.chunks(4).any(|p| p[3] > 0), "the first frame otherwise");
        clear_pick(&mods, Pick::Cursor);
        assert_eq!(animated_cursor(&mods), None);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn pieces_get_their_colour_on_top_of_other_mods() {
        let root = temp("pieces");
        let roblox = root.join("roblox");
        let white = crate::png::Rgba { width: 1, height: 1, pixels: vec![255, 255, 255, 255] };
        for rel in [SHIFT_LOCK, CURSORS[0]] {
            let path = roblox.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, crate::png::encode(&white)).unwrap();
        }
        let mods = root.join("mods");
        // A picked cursor (grey), then colours for the cursor and shift lock.
        let grey = crate::png::Rgba { width: 1, height: 1, pixels: vec![128, 128, 128, 255] };
        let picked = mods.join(FELINE_PICKS).join(CURSORS[0]);
        fs::create_dir_all(picked.parent().unwrap()).unwrap();
        fs::write(&picked, crate::png::encode(&grey)).unwrap();
        set_piece_colour(&mods, Piece::Cursor, Some(0xFF0000)).unwrap();
        set_piece_colour(&mods, Piece::ShiftLock, Some(0x0000FF)).unwrap();
        assert_eq!(piece_colours(&mods), [Some(0xFF0000), Some(0x0000FF), None]);
        let target = ModTarget { dir: roblox.clone(), overlay: false };
        apply_with_state(&target, &list_packs(&mods), &root.join("state")).unwrap();
        let pixel = |rel: &str| crate::png::decode(&fs::read(roblox.join(rel)).unwrap()).unwrap().pixels;
        assert_eq!(pixel(CURSORS[0])[..3], [128, 0, 0], "the picked grey cursor, in red");
        assert_eq!(pixel(SHIFT_LOCK)[..3], [0, 0, 255], "Roblox's white shift lock, in blue");
        set_piece_colour(&mods, Piece::Cursor, None).unwrap();
        set_piece_colour(&mods, Piece::ShiftLock, None).unwrap();
        assert!(!mods.join(FELINE_COLOURS).exists(), "no colours, no mod");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn generated_files_are_linked_and_mod_files_copied() {
        let root = temp("links");
        let roblox = root.join("roblox");
        let ui = roblox.join("content/textures/ui");
        fs::create_dir_all(&ui).unwrap();
        let white = crate::png::Rgba { width: 1, height: 1, pixels: vec![255, 255, 255, 255] };
        fs::write(ui.join("panel.png"), crate::png::encode(&white)).unwrap();
        write(&roblox.join("content/sounds/ouch.ogg"), "oof");
        let mods = root.join("mods");
        write(&mods.join("Cat/content/sounds/ouch.ogg"), "meow");
        install_tint(&mods, &Tint::solid(0xCDB8FF)).unwrap();
        let target = ModTarget { dir: roblox.clone(), overlay: false };
        let state = root.join("state");
        let packs = list_packs(&mods);
        apply_with_state(&target, &packs, &state).unwrap();
        let records = load_records(&state.join("applied.json"));
        let linked = |rel: &str| records.iter().find(|r| r.path == rel).unwrap().linked;
        assert!(linked("content/textures/ui/panel.png"), "Feline's own tinted image is linked");
        assert!(!linked("content/sounds/ouch.ogg"), "the mod's file is a copy");
        // Writing into Roblox's copy never reaches the mod.
        let sound = roblox.join("content/sounds/ouch.ogg");
        fs::write(&sound, "changed").unwrap();
        assert_eq!(fs::read_to_string(mods.join("Cat/content/sounds/ouch.ogg")).unwrap(), "meow");
        // A different tint replaces the linked image, and off restores all.
        install_tint(&mods, &Tint::solid(0xBDF5DC)).unwrap();
        apply_with_state(&target, &packs, &state).unwrap();
        let pixel = |p: &Path| crate::png::decode(&fs::read(p).unwrap()).unwrap().pixels;
        assert_eq!(pixel(&ui.join("panel.png"))[..3], [197, 255, 229], "mint at full brightness");
        apply_with_state(&target, &[], &state).unwrap();
        assert_eq!(pixel(&ui.join("panel.png")), [255, 255, 255, 255]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn conflicts_and_profiles() {
        let root = temp("conflicts");
        write(&root.join("A/content/sounds/ouch.ogg"), "a");
        write(&root.join("B/content/sounds/ouch.ogg"), "b");
        write(&root.join("B/content/fonts/x.ttf"), "b");
        let packs = list_packs(&root);
        assert_eq!(
            conflicts(&packs),
            [("content/sounds/ouch.ogg".to_owned(), vec!["A".to_owned(), "B".to_owned()])]
        );
        assert_eq!(preview_sound(&packs[0].path), Some(root.join("A/content/sounds/ouch.ogg")));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn mods_round_trip_through_a_zip() {
        let root = temp("zip");
        let mods = root.join("mods");
        write(&mods.join("Cat sounds/content/sounds/ouch.ogg"), "meow");
        write(&mods.join("Cat sounds/tint.json"), r##"{"color": "#FFC2E2"}"##);
        write(&mods.join("Cat sounds/content/run.exe"), "code");
        let zip = export_zip(&mods.join("Cat sounds")).unwrap();
        let file = root.join("shared.zip");
        fs::write(&file, &zip).unwrap();

        // Named after the zip file (no folder around it); code isn't shared.
        let name = import_zip(&file, &mods).unwrap();
        assert_eq!(name, "shared");
        assert_eq!(fs::read_to_string(mods.join("shared/content/sounds/ouch.ogg")).unwrap(), "meow");
        assert!(pack_tint(&mods.join("shared")).is_some());
        assert!(!mods.join("shared/content/run.exe").exists());
        let mut sneaky = crate::bugreport::Zip::new();
        sneaky.add("content/sounds/ouch.ogg", b"oof");
        sneaky.add("content/setup.bat", b"no");
        fs::write(root.join("sneaky.zip"), sneaky.finish()).unwrap();
        assert_eq!(import_zip(&root.join("sneaky.zip"), &mods).unwrap(), "sneaky");
        assert!(!mods.join("sneaky/content/setup.bat").exists());
        // Again: a new name, never over the first.
        assert_eq!(import_zip(&file, &mods).unwrap(), "shared 2");

        // A zipped Bloxstrap folder: "Modifications/" comes off, and a
        // folder around it names the mod.
        let mut wrapped = crate::bugreport::Zip::new();
        wrapped.add("Retro/Modifications/content/sounds/ouch.ogg", b"oof");
        wrapped.add("../evil.txt", b"no");
        wrapped.add("__MACOSX/Retro/._x", b"no");
        fs::write(root.join("w.zip"), wrapped.finish()).unwrap();
        assert_eq!(import_zip(&root.join("w.zip"), &mods).unwrap(), "Retro");
        assert!(mods.join("Retro/content/sounds/ouch.ogg").is_file());
        assert!(!root.join("evil.txt").exists());
        assert!(fs::read_dir(&mods).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().starts_with('.')));

        fs::write(root.join("bad.zip"), "not a zip").unwrap();
        assert!(import_zip(&root.join("bad.zip"), &mods).is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn tint_files_read_back_and_old_ones_still_work() {
        let old = Tint::parse(r##"{"color": "#CDB8FF"}"##).unwrap();
        assert_eq!(old, Tint::solid(0xCDB8FF));
        let gradient = Tint {
            color: 0xCDB8FF,
            color2: Some(0xFFC2E2),
            direction: crate::png::Direction::Diagonal,
            parts: vec![Part::Icons, Part::Chat],
        };
        assert_eq!(Tint::parse(&gradient.to_json()), Some(gradient));
        assert_eq!(Part::of("ExtraContent/LuaPackages/Packages/_Index/FoundationImages/FoundationImages/SpriteSheets/img_set_1x_1.png"), Part::Icons);
        assert_eq!(Part::of("ExtraContent/textures/ui/InGameChat/x.png"), Part::Chat);
        assert_eq!(Part::of("content/textures/ui/InGameMenu/x.png"), Part::Menus);
        assert_eq!(Part::of("content/textures/ui/Health-BKG-Center.png"), Part::Other);
    }

    #[test]
    fn paths_outside_the_mod_are_refused() {
        let root = Path::new("/mods/m");
        assert_eq!(relative(root, Path::new("/mods/m/content/a.ogg")).as_deref(), Some("content/a.ogg"));
        assert_eq!(relative(root, Path::new("/other/a.ogg")), None);
    }

    #[test]
    fn platform_files_go_where_the_android_app_has_them() {
        let dir = temp("android");
        let roblox = dir.join("assets");
        write(&roblox.join("android/textures/sky/sky512_bk.tex"), "original");
        write(&roblox.join("content/textures/ui/x.png"), "original");
        let pack = dir.join("mods/Sky");
        write(&pack.join("PlatformContent/pc/textures/sky/sky512_bk.tex"), "mine");
        let target = ModTarget { dir: roblox.clone(), overlay: false };
        let packs = list_packs(&dir.join("mods"));
        let applied = apply_with_options(&target, &packs, &dir.join("state"), false).unwrap();
        assert_eq!(applied.files, 1, "{:?}", applied.skipped);
        assert_eq!(fs::read_to_string(roblox.join("android/textures/sky/sky512_bk.tex")).unwrap(), "mine");
        assert!(!roblox.join("PlatformContent").exists());
        assert_eq!(android_path("content/a.png"), "content/a.png");
        let _ = fs::remove_dir_all(&dir);
    }

}

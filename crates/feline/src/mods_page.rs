// SPDX-License-Identifier: GPL-3.0-or-later
//! The Mods page: lists Feline's mods with their switches, offers to import
//! a Bloxstrap-family mods folder, and says whether the chosen runtime can
//! use mods at all. Applying happens when a game starts (see `gui.rs`).

use feline_core::mods::{self, BOOTSTRAPPERS};
use feline_core::runtime::DetectContext;
use feline_core::mods::{Part, Tint};
use feline_core::png::Direction;
use slint::{Color, ComponentHandle, Image, ModelRc, SharedString, VecModel};

use crate::gui::{AppWindow, FoundRow, ModRow, ModTools, Shared, lock, selected};

/// Fills the page. Cheap enough for the UI thread: a few folder listings.
pub fn refresh(ui: &AppWindow, shared: &Shared) {
    let state = lock(shared);
    let enabled = &state.settings.mods;
    let dir = mods::mods_dir();
    let packs = dir.as_deref().map(mods::list_packs).unwrap_or_default();
    let enabled_packs: Vec<_> = packs.iter().filter(|p| enabled.contains(&p.name)).cloned().collect();
    let rows: Vec<ModRow> = packs
        .into_iter()
        .map(|pack| {
            let preview = mods::preview_image(&pack.path).and_then(|p| Image::load_from_path(&p).ok());
            let has_sound = mods::preview_sound(&pack.path).is_some();
            let files = match pack.files {
                1 => "1 file".to_owned(),
                n => format!("{n} files"),
            };
            ModRow {
                has_preview: preview.is_some(),
                preview: preview.unwrap_or_default(),
                has_sound,
                enabled: enabled.contains(&pack.name),
                target: match state.settings.mod_scope(&pack.name) {
                    feline_core::settings::ModScope::Player => 0,
                    feline_core::settings::ModScope::Studio => 1,
                    feline_core::settings::ModScope::Both => 2,
                },
                detail: if pack.summary.is_empty() {
                    files.into()
                } else {
                    format!("{} · {files}", pack.summary).into()
                },
                name: pack.name.into(),
            }
        })
        .collect();
    ui.set_mods_theme_installed(rows.iter().any(|r| r.name == mods::FELINE_THEME));
    ui.global::<ModTools>().set_installed(rows.iter().any(|r| r.name == mods::FELINE_TINT));
    let picks = dir.as_deref().map(mods::picks).unwrap_or([false; 5]);
    crate::more::refresh_mods(ui);
    ui.set_mods_picks(ModelRc::new(VecModel::from(picks.to_vec())));
    ui.global::<ModTools>()
        .set_cursor_animated(dir.as_deref().and_then(mods::animated_cursor).is_some());
    let profiles: Vec<SharedString> = mods::mod_profiles().into_keys().map(SharedString::from).collect();
    ui.set_mods_profiles(ModelRc::new(VecModel::from(profiles)));
    let conflicts: Vec<SharedString> = mods::conflicts(&enabled_packs)
        .into_iter()
        .take(12)
        .map(|(file, names)| {
            let winner = names.last().cloned().unwrap_or_default();
            format!("{file}: {} (\"{winner}\" wins)", names.join(", ")).into()
        })
        .collect();
    ui.set_mods_conflicts(ModelRc::new(VecModel::from(conflicts)));
    ui.set_mod_rows(ModelRc::new(VecModel::from(rows)));
    ui.set_mods_folder(
        dir.map(|d| crate::gui::tilde(&d))
            .unwrap_or_default()
            .into(),
    );

    let ctx = DetectContext::host();
    let found: Vec<FoundRow> = mods::find_bootstrappers(ctx.local_app_data.as_deref())
        .into_iter()
        .map(|b| FoundRow {
            name: b.name.into(),
            detail: format!("{} files in {}", b.files, crate::gui::tilde(&b.dir)).into(),
        })
        .collect();
    ui.set_mods_found(ModelRc::new(VecModel::from(found)));

    let (supported, line) = match selected(&state) {
        None => (false, "Pick a runtime first; mods are put into the one you play with.".to_owned()),
        Some(f) => {
            let name = f.installation.backend.name();
            if f.installation.mod_target(&ctx).is_some() {
                (true, format!("{name} can use mods. They're applied each time you press Play."))
            } else {
                (
                    false,
                    format!(
                        "{name} has no known way to load mods, so it starts without them. \
                         Mods work with Roblox on Windows, Feline's own runtime and Sober."
                    ),
                )
            }
        }
    };
    ui.set_mods_supported(supported);
    ui.set_mods_support(line.into());
    // The generator's preview uses the chosen runtime's files.
    drop(state);
    update_preview(ui, shared);
}

/// Switches one mod on or off (takes effect at the next Play).
pub fn toggle(ui: &AppWindow, shared: &Shared, name: &str, on: bool) {
    {
        let mut state = lock(shared);
        let list = &mut state.settings.mods;
        list.retain(|m| m != name);
        if on {
            list.push(name.to_owned());
        }
    }
    let status = match crate::settings_page::save_own(shared) {
        Ok(()) if on => format!("{name} is on. It goes in when you press Play."),
        Ok(()) => format!("{name} is off. Roblox's own files come back when you press Play."),
        Err(e) => format!("Couldn't save: {e}"),
    };
    ui.set_mods_status(status.into());
    refresh(ui, shared);
}

/// Sends an enabled mod to the player (0), Studio only (1) or both (2).
pub fn set_target(ui: &AppWindow, shared: &Shared, name: &str, target: i32) {
    use feline_core::settings::ModScope;
    let scope = match target {
        1 => ModScope::Studio,
        2 => ModScope::Both,
        _ => ModScope::Player,
    };
    {
        let mut state = lock(shared);
        if state.settings.mod_scope(name) == scope {
            return;
        }
        state.settings.set_mod_scope(name, scope);
    }
    let status = match crate::settings_page::save_own(shared) {
        Ok(()) => match scope {
            ModScope::Player => format!("{name} goes into Roblox."),
            ModScope::Studio => format!("{name} goes into Roblox Studio only, when Feline opens Studio."),
            ModScope::Both => format!("{name} goes into Roblox and Roblox Studio."),
        },
        Err(e) => format!("Couldn't save: {e}"),
    };
    ui.set_mods_status(status.into());
    refresh(ui, shared);
}

/// Copies a bootstrapper's mods folder into Feline as a new, enabled mod.
pub fn import(ui: &AppWindow, shared: &Shared, which: &str) {
    let ctx = DetectContext::host();
    let Some(found) = mods::find_bootstrappers(ctx.local_app_data.as_deref())
        .into_iter()
        .find(|b| b.name == which && BOOTSTRAPPERS.contains(&b.name))
    else {
        return;
    };
    let Some(dir) = mods::mods_dir() else {
        ui.set_mods_status("Feline's mods folder couldn't be found.".into());
        return;
    };
    match mods::import(&found.dir, &dir, &format!("From {which}")) {
        Ok(name) => {
            lock(shared).settings.mods.push(name.clone());
            let _ = crate::settings_page::save_own(shared);
            ui.set_mods_status(format!("Imported {which}'s mods as \"{name}\" and switched them on.").into());
        }
        Err(e) => ui.set_mods_status(format!("Couldn't import {which}'s mods: {e}").into()),
    }
    refresh(ui, shared);
}

/// Adds Feline's own theme (the cat logo in place of Roblox's) and switches it on.
pub fn get_theme(ui: &AppWindow, shared: &Shared) {
    let Some(dir) = mods::mods_dir() else {
        ui.set_mods_status("Feline's mods folder couldn't be found.".into());
        return;
    };
    match mods::install_feline_theme(&dir) {
        Ok(()) => {
            {
                let mut state = lock(shared);
                let list = &mut state.settings.mods;
                if !list.iter().any(|m| m == mods::FELINE_THEME) {
                    list.push(mods::FELINE_THEME.to_owned());
                }
            }
            let _ = crate::settings_page::save_own(shared);
            ui.set_mods_status(
                "The Feline Theme is on: the cat takes over Roblox's logo when you press Play.".into(),
            );
        }
        Err(e) => ui.set_mods_status(format!("Couldn't add the Feline Theme: {e}").into()),
    }
    refresh(ui, shared);
}

// ---------------------------------------------------------------------------
// Mod generator (the Feline Tint)
// ---------------------------------------------------------------------------

fn to_color(rgb: u32) -> Color {
    Color::from_rgb_u8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

fn from_color(c: Color) -> u32 {
    (u32::from(c.red()) << 16) | (u32::from(c.green()) << 8) | u32::from(c.blue())
}

/// Puts a tint's settings into the generator.
fn show_tint(tools: &ModTools, tint: &Tint) {
    let second = tint.color2.unwrap_or(mods::TINT_COLOURS[1].1);
    tools.set_colour1(to_color(tint.color));
    tools.set_hex1(format!("#{:06X}", tint.color).into());
    tools.set_colour2(to_color(second));
    tools.set_hex2(format!("#{second:06X}").into());
    tools.set_gradient(tint.color2.is_some());
    tools.set_direction(match tint.direction {
        Direction::Across => 0,
        Direction::Down => 1,
        Direction::Diagonal => 2,
    });
    tools.set_icons(tint.parts.contains(&Part::Icons));
    tools.set_menus(tint.parts.contains(&Part::Menus));
    tools.set_chat(tint.parts.contains(&Part::Chat));
    tools.set_other(tint.parts.contains(&Part::Other));
}

/// The tint the generator shows now.
fn current_tint(tools: &ModTools) -> Tint {
    let parts = [
        (tools.get_icons(), Part::Icons),
        (tools.get_menus(), Part::Menus),
        (tools.get_chat(), Part::Chat),
        (tools.get_other(), Part::Other),
    ];
    Tint {
        color: from_color(tools.get_colour1()),
        color2: tools.get_gradient().then(|| from_color(tools.get_colour2())),
        direction: match tools.get_direction() {
            1 => Direction::Down,
            2 => Direction::Diagonal,
            _ => Direction::Across,
        },
        parts: parts.into_iter().filter(|(on, _)| *on).map(|(_, p)| p).collect(),
    }
}

/// Sets the generator up: the presets, and the Feline Tint's settings if
/// it's been made before. Once, at start (later refreshes keep your edits).
pub fn load_generator(ui: &AppWindow, shared: &Shared) {
    let tools = ui.global::<ModTools>();
    let names: Vec<SharedString> = mods::TINT_COLOURS.iter().map(|(n, _)| SharedString::from(*n)).collect();
    let colours: Vec<Color> = mods::TINT_COLOURS.iter().map(|(_, c)| to_color(*c)).collect();
    tools.set_preset_names(ModelRc::new(VecModel::from(names)));
    tools.set_preset_colours(ModelRc::new(VecModel::from(colours)));
    let saved = mods::mods_dir().and_then(|d| mods::pack_tint(&d.join(mods::FELINE_TINT)));
    show_tint(&tools, &saved.unwrap_or_else(|| Tint::solid(mods::TINT_COLOURS[0].1)));
    update_preview(ui, shared);
}

/// Redraws the preview: Roblox's own icons with the tint on.
pub fn update_preview(ui: &AppWindow, shared: &Shared) {
    let tools = ui.global::<ModTools>();
    let (target, theme_on) = {
        let state = lock(shared);
        (
            selected(&state).and_then(|f| f.installation.mod_target(&DetectContext::host())),
            state.settings.mods.iter().any(|m| m == mods::FELINE_THEME),
        )
    };
    let theme = mods::mods_dir().map(|d| d.join(mods::FELINE_THEME)).filter(|d| theme_on && d.is_dir());
    let image = target.and_then(|t| mods::tint_preview(&t.dir, &current_tint(&tools), theme.as_deref()));
    tools.set_has_preview(image.is_some());
    if let Some(image) = image {
        let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
            &image.pixels,
            image.width,
            image.height,
        );
        tools.set_preview(Image::from_rgba8(buffer));
    }
}

/// A preset swatch was clicked for colour 1 or 2.
pub fn choose_preset(ui: &AppWindow, shared: &Shared, which: i32, index: i32) {
    if let Some((_, rgb)) = mods::TINT_COLOURS.get(index.max(0) as usize) {
        set_colour(ui, shared, which, *rgb);
    }
}

/// A colour was typed for colour 1 or 2.
pub fn choose_hex(ui: &AppWindow, shared: &Shared, which: i32, text: &str) {
    match feline_core::look::parse_hex(text) {
        Some(rgb) => set_colour(ui, shared, which, rgb),
        None => ui
            .global::<ModTools>()
            .set_message("That's not a colour Feline knows. Try # and six hex digits, like #ff8fb1.".into()),
    }
}

fn set_colour(ui: &AppWindow, shared: &Shared, which: i32, rgb: u32) {
    let tools = ui.global::<ModTools>();
    tools.set_message(SharedString::new());
    let hex = SharedString::from(format!("#{rgb:06X}"));
    if which == 2 {
        tools.set_colour2(to_color(rgb));
        tools.set_hex2(hex);
    } else {
        tools.set_colour1(to_color(rgb));
        tools.set_hex1(hex);
    }
    update_preview(ui, shared);
}

/// Makes (or changes) the Feline Tint from the generator and switches it on.
pub fn make_tint(ui: &AppWindow, shared: &Shared) {
    let Some(dir) = mods::mods_dir() else {
        ui.set_mods_status("Feline's mods folder couldn't be found.".into());
        return;
    };
    let tint = current_tint(&ui.global::<ModTools>());
    if tint.parts.is_empty() {
        return;
    }
    match mods::install_tint(&dir, &tint) {
        Ok(()) => {
            enable(shared, mods::FELINE_TINT);
            let what = if tint.color2.is_some() { "gradient" } else { "colour" };
            ui.set_mods_status(
                format!("The Feline Tint has its new {what}: Roblox changes colour when you press Play.").into(),
            );
        }
        Err(e) => ui.set_mods_status(format!("Couldn't make the Feline Tint: {e}").into()),
    }
    refresh(ui, shared);
}

// ---------------------------------------------------------------------------
// Sharing mods as .zip files
// ---------------------------------------------------------------------------

/// Packs a mod into `<name>.zip` in Downloads and shows it.
pub fn share(ui: &AppWindow, name: &str) {
    let (Some(dir), Some(out)) = (mods::mods_dir(), mods::export_path(name)) else {
        ui.set_mods_status("Feline's mods folder or your Downloads folder couldn't be found.".into());
        return;
    };
    let result = mods::export_zip(&dir.join(name))
        .and_then(|zip| std::fs::write(&out, zip).map_err(|e| e.to_string()));
    match result {
        Ok(()) => {
            ui.set_mods_status(
                format!(
                    "Saved \"{name}\" as {}. Send it to a friend: they add it with \"Add a .zip…\" (it works in Bloxstrap too, unzipped into its Modifications folder).",
                    crate::gui::tilde(&out)
                )
                .into(),
            );
            reveal(&out);
        }
        Err(e) => ui.set_mods_status(format!("Couldn't share \"{name}\": {e}").into()),
    }
}

/// Shows a file in the file manager (selected, on Windows).
pub(crate) fn reveal(file: &std::path::Path) {
    let mut command = if cfg!(windows) {
        let mut c = std::process::Command::new("explorer");
        c.arg(format!("/select,{}", file.display()));
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(file.parent().unwrap_or(file));
        c
    };
    let _ = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// Asks for a shared mod (.zip) and adds it, switched on.
pub fn import_zip(ui: &AppWindow, shared: &Shared) {
    let Some(file) = crate::picker::pick_file("Add a shared mod", "Zip files", &["*.zip"]) else {
        return;
    };
    let Some(dir) = mods::mods_dir() else {
        ui.set_mods_status("Feline's mods folder couldn't be found.".into());
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    match mods::import_zip(&file, &dir) {
        Ok(name) => {
            enable(shared, &name);
            ui.set_mods_status(format!("Added \"{name}\" and switched it on. It goes in when you press Play.").into());
        }
        Err(e) => ui.set_mods_status(format!("Couldn't add that mod: {e}").into()),
    }
    refresh(ui, shared);
}

pub(crate) fn enable(shared: &Shared, name: &str) {
    {
        let mut state = lock(shared);
        if !state.settings.mods.iter().any(|m| m == name) {
            state.settings.mods.push(name.to_owned());
        }
    }
    let _ = crate::settings_page::save_own(shared);
}

const PICKS: [(mods::Pick, &str, &str, &[&str]); 5] = [
    (mods::Pick::DeathSound, "Choose a death sound", "Sounds", &["*.ogg", "*.mp3"]),
    (mods::Pick::Cursor, "Choose a cursor", "Cursors and pictures", &["*.png", "*.gif", "*.ani", "*.cur"]),
    (mods::Pick::Font, "Choose a font", "Fonts", &["*.ttf", "*.otf"]),
    (mods::Pick::Emoji, "Choose a colour emoji font", "Fonts", &["*.ttf"]),
    (mods::Pick::ShiftLock, "Choose a shift lock pointer", "Pictures", &["*.png", "*.gif", "*.ani", "*.cur"]),
];

/// Puts in an emoji set ("windows" or "catmoji"; Catmoji downloads, so
/// in the background).
pub fn emoji(ui: &AppWindow, shared: &Shared, which: &str) {
    let set = match which {
        "windows" => mods::EmojiSet::Windows,
        _ => mods::EmojiSet::Catmoji,
    };
    let tools = ui.global::<ModTools>();
    tools.set_emoji_busy(true);
    let weak = ui.as_weak();
    let shared = shared.clone();
    let _ = std::thread::Builder::new().name("feline-emoji".into()).spawn(move || {
        let result = mods::emoji_font(set).and_then(|font| {
            let dir = mods::mods_dir().ok_or("no mods folder")?;
            mods::set_emoji(&dir, &font)
        });
        let _ = weak.upgrade_in_event_loop(move |ui| {
            ui.global::<ModTools>().set_emoji_busy(false);
            match result {
                Ok(()) => {
                    enable(&shared, mods::FELINE_PICKS);
                    ui.set_mods_status("New emoji! They go in when you press Play.".into());
                }
                Err(e) => ui.set_mods_status(format!("That didn't work: {e}").into()),
            }
            refresh(&ui, &shared);
        });
    });
}

/// Asks for a file and puts it into the Feline Picks mod.
pub fn pick(ui: &AppWindow, shared: &Shared, which: i32) {
    let Some((pick, title, kind, patterns)) = PICKS.get(which.max(0) as usize) else { return };
    let Some(file) = crate::picker::pick_file(title, kind, patterns) else {
        return;
    };
    let Some(dir) = mods::mods_dir() else { return };
    match mods::set_pick(&dir, *pick, &file) {
        Ok(()) => {
            enable(shared, mods::FELINE_PICKS);
            ui.set_mods_status("Got it! It goes in when you press Play.".into());
        }
        Err(e) => ui.set_mods_status(format!("That didn't work: {e}").into()),
    }
    refresh(ui, shared);
}

pub fn clear_pick(ui: &AppWindow, shared: &Shared, which: i32) {
    let (Some((pick, ..)), Some(dir)) = (PICKS.get(which.max(0) as usize), mods::mods_dir()) else {
        return;
    };
    mods::clear_pick(&dir, *pick);
    ui.set_mods_status("Removed. Roblox's own comes back when you press Play.".into());
    refresh(ui, shared);
}

pub fn save_profile(ui: &AppWindow, shared: &Shared, name: &str) {
    if name.trim().is_empty() {
        ui.set_mods_status("Give the profile a name first.".into());
        return;
    }
    let list = lock(shared).settings.mods.clone();
    let status = match mods::set_mod_profile(name, Some(&list)) {
        Ok(()) => format!("Saved \"{}\" ({} mods on).", name.trim(), list.len()),
        Err(e) => format!("Couldn't save: {e}"),
    };
    ui.set_mods_status(status.into());
    refresh(ui, shared);
}

pub fn apply_profile(ui: &AppWindow, shared: &Shared, name: &str) {
    let Some(list) = mods::mod_profiles().remove(name) else { return };
    lock(shared).settings.mods = list;
    let _ = crate::settings_page::save_own(shared);
    ui.set_mods_status(format!("Switched to \"{name}\". It goes in when you press Play.").into());
    refresh(ui, shared);
}

pub fn delete_profile(ui: &AppWindow, shared: &Shared, name: &str) {
    let _ = mods::set_mod_profile(name, None);
    refresh(ui, shared);
}

/// Plays one of the mod's sounds in the system's usual player.
pub fn listen(ui: &AppWindow, name: &str) {
    let Some(sound) = mods::mods_dir().and_then(|d| mods::preview_sound(&d.join(name))) else {
        return;
    };
    if let Err(e) = crate::picker::open_with_default_app(&sound) {
        ui.set_mods_status(format!("Couldn't play it: {e}").into());
    }
}

/// Takes all mods out of the chosen runtime now, and switches them all off.
pub fn restore(ui: &AppWindow, shared: &Shared) {
    let installation = {
        let state = lock(shared);
        selected(&state).map(|f| f.installation.clone())
    };
    let Some(installation) = installation else {
        ui.set_mods_status("Pick a runtime first.".into());
        return;
    };
    let status = match mods::restore_now(&installation, &DetectContext::host()) {
        Ok(n) => {
            lock(shared).settings.mods.clear();
            let _ = crate::settings_page::save_own(shared);
            match n {
                0 => "All mods are off. There was nothing to put back.".to_owned(),
                n => format!("All mods are off, and {n} of Roblox's own files are back."),
            }
        }
        Err(e) => e,
    };
    ui.set_mods_status(status.into());
    refresh(ui, shared);
}

pub fn open_folder(ui: &AppWindow) {
    if let Some(dir) = mods::mods_dir() {
        crate::gui::open_folder(ui, dir);
    }
}

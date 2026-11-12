// SPDX-License-Identifier: GPL-3.0-or-later
//! How Feline's own window looks beyond its colours (Looks in looks.slint):
//! the font, a gradient or picture behind the pages, Windows 11's glass.

use slint::{ComponentHandle, Image};

use crate::gui::{AppWindow, Looks, Shared, lock};

/// Shows (and applies) the current choices.
pub fn refresh(ui: &AppWindow, shared: &Shared) {
    let looks = ui.global::<Looks>();
    let (font, background, glass) = {
        let s = &lock(shared).settings;
        (s.app_font.clone(), s.app_background.clone(), s.app_glass)
    };
    looks.set_windows(cfg!(windows));
    looks.set_font(font.into());
    let picture = (!background.is_empty() && background != "gradient")
        .then(|| Image::load_from_path(std::path::Path::new(&background)).ok())
        .flatten();
    looks.set_background(match (background.as_str(), &picture) {
        ("gradient", _) => 1,
        (_, Some(_)) => 2,
        _ => 0,
    });
    if let Some(picture) = picture {
        looks.set_picture(picture);
    }
    looks.set_glass(glass as i32);
    apply_glass(ui, glass);
}

/// Asks Windows for the glass behind Feline's window (once it's shown).
fn apply_glass(ui: &AppWindow, glass: u32) {
    #[cfg(windows)]
    {
        let dark = ui.global::<crate::gui::Theme>().get_dark();
        let apply = move || {
            for window in feline_win::window::windows_of(std::process::id()) {
                if feline_win::window::title(window) == "Feline" {
                    feline_win::window::set_backdrop(window, glass, dark);
                }
            }
        };
        // The window may not be on screen yet when Feline starts.
        apply();
        slint::Timer::single_shot(std::time::Duration::from_millis(600), apply);
    }
    #[cfg(not(windows))]
    let _ = (ui, glass);
}

fn change(ui: &AppWindow, shared: &Shared, f: impl FnOnce(&mut feline_core::settings::Settings)) {
    f(&mut lock(shared).settings);
    let message = match crate::settings_page::save_own(shared) {
        Ok(()) => String::new(),
        Err(e) => format!("Couldn't save that: {e}"),
    };
    ui.global::<Looks>().set_message(message.into());
    refresh(ui, shared);
}

/// Hooks up the callbacks.
pub fn wire(ui: &AppWindow, shared: &Shared) {
    let looks = ui.global::<Looks>();
    looks.on_set_font({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |name| {
            if let Some(ui) = weak.upgrade() {
                let name: String = name.trim().chars().take(64).collect();
                change(&ui, &shared, |s| s.app_font = name);
            }
        }
    });
    looks.on_set_background({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |which| {
            if let Some(ui) = weak.upgrade() {
                change(&ui, &shared, |s| s.app_background = if which == 1 { "gradient".into() } else { String::new() });
            }
        }
    });
    looks.on_pick_picture({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move || {
            let Some(ui) = weak.upgrade() else { return };
            let Some(file) = crate::picker::pick_file("Choose a background picture", "Pictures", &["*.png", "*.jpg", "*.jpeg"]) else {
                return;
            };
            // A copy of Feline's own, so moving or deleting the original
            // doesn't take the background away.
            let ext = file.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            let copied = feline_core::paths::state_dir()
                .map(|d| d.join(format!("background.{ext}")))
                .filter(|to| std::fs::create_dir_all(to.parent().unwrap_or(to)).is_ok() && std::fs::copy(&file, to).is_ok());
            match copied {
                Some(path) if Image::load_from_path(&path).is_ok() => {
                    change(&ui, &shared, |s| s.app_background = path.to_string_lossy().into_owned());
                }
                _ => ui.global::<Looks>().set_message("That picture couldn't be used. Try a PNG or JPEG.".into()),
            }
        }
    });
    looks.on_set_glass({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |i| {
            if let Some(ui) = weak.upgrade() {
                change(&ui, &shared, |s| s.app_glass = i.clamp(0, 2) as u32);
            }
        }
    });
    refresh(ui, shared);
}

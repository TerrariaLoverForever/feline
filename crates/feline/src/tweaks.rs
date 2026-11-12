// SPDX-License-Identifier: GPL-3.0-or-later
//! The Tweaks page: Roblox's allowlisted FastFlags (feline-core's
//! fflags.rs), friendly choices, the raw list, and profiles per game. The
//! flags are written when a game starts (see `gui::launch`).

use feline_core::fflags::{self, ALLOWLIST, Choices, Kind};
use feline_core::history::History;
use feline_core::runtime::Origin;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::gui::{AppWindow, FlagRow, GameProfileRow, Shared, TweaksState, lock, selected};

/// Shows the current tweaks.
pub fn refresh(ui: &AppWindow, shared: &Shared) {
    let state = lock(shared);
    let t = ui.global::<TweaksState>();
    // On Windows there's only Roblox's official client (and this may run
    // before detection has finished).
    t.set_supported(
        cfg!(windows)
            || selected(&state).is_some_and(|f| f.installation.backend.origin() == Origin::Official),
    );
    let set = fflags::sanitize(&state.settings.fflags);
    let c = Choices::read(&set);
    t.set_renderer(c.renderer as i32);
    t.set_msaa(c.msaa as i32);
    t.set_textures(c.textures as i32);
    t.set_quality(c.quality as i32);
    t.set_hide_grass(c.hide_grass);
    t.set_still_grass(c.still_grass);
    t.set_gray_sky(c.gray_sky);
    t.set_exclusive_fullscreen(c.exclusive_fullscreen);
    t.set_ignore_dpi(c.ignore_dpi);
    t.set_pause_voxels(c.pause_voxels);
    t.set_count(set.len() as i32);
    let rows: Vec<FlagRow> = ALLOWLIST
        .iter()
        .map(|f| FlagRow {
            name: f.name.into(),
            value: set.get(f.name).cloned().unwrap_or_default().into(),
            hint: match f.kind {
                Kind::Bool => "True or False".into(),
                Kind::Int(min, max) => format!("A number from {min} to {max}").into(),
            },
        })
        .collect();
    t.set_flags(ModelRc::new(VecModel::from(rows)));

    let profiles = fflags::profiles();
    let names: Vec<SharedString> = profiles.keys().map(|n| n.clone().into()).collect();
    let mut choices = vec![SharedString::from("Everyday tweaks")];
    choices.extend(names.iter().cloned());
    let h = History::load();
    let games: Vec<GameProfileRow> = h
        .games_played()
        .iter()
        .take(10)
        .map(|g| GameProfileRow {
            place: g.place_id.to_string().into(),
            name: h.name_of(g).into(),
            index: state
                .settings
                .fflag_games
                .get(&g.place_id)
                .and_then(|p| profiles.keys().position(|n| n == p))
                .map_or(0, |i| i as i32 + 1),
        })
        .collect();
    t.set_profiles(ModelRc::new(VecModel::from(names)));
    t.set_choices(ModelRc::new(VecModel::from(choices)));
    t.set_games(ModelRc::new(VecModel::from(games)));
}

fn save(ui: &AppWindow, shared: &Shared, done: &str) {
    let status = match crate::settings_page::save_own(shared) {
        Ok(()) => done.to_owned(),
        Err(e) => format!("Couldn't save: {e}"),
    };
    ui.global::<TweaksState>().set_status(status.into());
    refresh(ui, shared);
}

/// Hooks up the page's callbacks.
pub fn wire(ui: &AppWindow, shared: &Shared) {
    let t = ui.global::<TweaksState>();
    let on = |f: fn(&AppWindow, &Shared)| {
        let weak = ui.as_weak();
        let shared = shared.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                f(&ui, &shared);
            }
        }
    };
    ui.on_tweaks_opened(on(refresh));
    t.on_reset(on(|ui, shared| {
        lock(shared).settings.fflags.clear();
        save(ui, shared, "All tweaks are off. Roblox's own settings come back when you press Play.");
    }));
    t.on_set({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |which, value| {
            let Some(ui) = weak.upgrade() else { return };
            {
                let mut state = lock(&shared);
                let set = &mut state.settings.fflags;
                let mut c = Choices::read(set);
                let i = value.max(0) as usize;
                let b = value != 0;
                match which.as_str() {
                    "renderer" => c.renderer = i.min(3),
                    "msaa" => c.msaa = i.min(4),
                    "textures" => c.textures = i.min(4),
                    "quality" => c.quality = i.min(5),
                    "hide-grass" => c.hide_grass = b,
                    "still-grass" => c.still_grass = b,
                    "gray-sky" => c.gray_sky = b,
                    "exclusive-fullscreen" => c.exclusive_fullscreen = b,
                    "ignore-dpi" => c.ignore_dpi = b,
                    "pause-voxels" => c.pause_voxels = b,
                    _ => return,
                }
                c.write(set);
            }
            save(&ui, &shared, "Saved. It goes in when you press Play.");
        }
    });
    t.on_set_flag({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |name, value| {
            let Some(ui) = weak.upgrade() else { return };
            let value = value.trim();
            if value.is_empty() {
                lock(&shared).settings.fflags.remove(name.as_str());
                save(&ui, &shared, &format!("{name}: Roblox decides."));
                return;
            }
            match fflags::checked(&name, value) {
                Some(v) => {
                    lock(&shared).settings.fflags.insert(name.to_string(), v.clone());
                    save(&ui, &shared, &format!("{name} = {v}. It goes in when you press Play."));
                }
                None => {
                    let hint = fflags::flag(&name).map_or(String::new(), |f| match f.kind {
                        Kind::Bool => "True or False".to_owned(),
                        Kind::Int(min, max) => format!("a number from {min} to {max}"),
                    });
                    ui.global::<TweaksState>()
                        .set_status(format!("\"{value}\" doesn't fit {name}. It takes {hint}.").into());
                    refresh(&ui, &shared);
                }
            }
        }
    });
    t.on_save_profile({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |name| {
            let Some(ui) = weak.upgrade() else { return };
            let name = name.trim().to_owned();
            if name.is_empty() {
                ui.global::<TweaksState>().set_status("Give the profile a name first.".into());
                return;
            }
            let mut all = fflags::profiles();
            all.insert(name.clone(), fflags::sanitize(&lock(&shared).settings.fflags));
            let status = match fflags::save_profiles(&all) {
                Ok(()) => format!("Saved your current tweaks as \"{name}\"."),
                Err(e) => format!("Couldn't save the profile: {e}"),
            };
            ui.global::<TweaksState>().set_status(status.into());
            refresh(&ui, &shared);
        }
    });
    t.on_use_profile({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |name| {
            let Some(ui) = weak.upgrade() else { return };
            if let Some(set) = fflags::profiles().remove(name.as_str()) {
                lock(&shared).settings.fflags = set;
                save(&ui, &shared, &format!("Switched to \"{name}\". It goes in when you press Play."));
            }
        }
    });
    t.on_delete_profile({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |name| {
            let Some(ui) = weak.upgrade() else { return };
            let mut all = fflags::profiles();
            all.remove(name.as_str());
            let _ = fflags::save_profiles(&all);
            lock(&shared).settings.fflag_games.retain(|_, p| p.as_str() != name.as_str());
            save(&ui, &shared, &format!("Deleted \"{name}\"."));
        }
    });
    t.on_set_game({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |place, index| {
            let Some(ui) = weak.upgrade() else { return };
            let Ok(place) = place.parse::<u64>() else { return };
            let names: Vec<String> = fflags::profiles().into_keys().collect();
            {
                let mut state = lock(&shared);
                match (index > 0).then(|| names.get(index as usize - 1)).flatten() {
                    Some(name) => {
                        state.settings.fflag_games.insert(place, name.clone());
                    }
                    None => {
                        state.settings.fflag_games.remove(&place);
                    }
                }
            }
            save(&ui, &shared, "Saved. That game gets it next time you start it from Feline.");
        }
    });
}

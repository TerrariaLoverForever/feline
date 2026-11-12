// SPDX-License-Identifier: GPL-3.0-or-later
//! The Roblox page's tools (status, versions, profiles, per-game profiles,
//! repair) and the server picker.

use feline_core::history::{self, History};
use feline_core::link::GameLink;
use feline_core::roblox_tools::{self, Server, VersionLog};
use feline_core::runtime::{DetectContext, LaunchTarget};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::gui::{AppWindow, GameProfileRow, Page, ServerRow, Shared, VersionRow, lock, selected};

/// Notes the versions of every runtime found (called after each scan).
pub fn record_versions(shared: &Shared) {
    let state = lock(shared);
    let mut log = VersionLog::load();
    let mut changed = false;
    for f in &state.found {
        if let Some(v) = &f.version {
            changed |= log.record(f.installation.backend.id(), v, history::now());
        }
    }
    if changed {
        let _ = log.save();
    }
}

fn date_label(t: u64) -> String {
    let (today, _) = history::day_and_week_start(history::now(), history::local_offset_seconds());
    match t {
        t if t >= today => "since today".to_owned(),
        t if t + 86_400 >= today => "since yesterday".to_owned(),
        t => format!("since {} days ago", (today - t) / 86_400 + 1),
    }
}

/// Fills the tools for the selected runtime.
pub fn refresh(ui: &AppWindow, shared: &Shared) {
    let state = lock(shared);
    let Some(found) = selected(&state) else {
        ui.set_rx_runtime(SharedString::new());
        ui.set_rx_has_settings(false);
        ui.set_rx_versions(ModelRc::new(VecModel::from(Vec::<VersionRow>::new())));
        return;
    };
    let backend = found.installation.backend;
    ui.set_rx_runtime(backend.name().into());
    ui.set_rx_has_settings(!backend.settings().is_empty());

    let log = VersionLog::load();
    let versions: Vec<VersionRow> = log
        .runtimes
        .get(backend.id())
        .map(|list| {
            list.iter()
                .rev()
                .take(8)
                .map(|(v, t)| VersionRow {
                    version: v.into(),
                    when: date_label(*t).into(),
                })
                .collect()
        })
        .unwrap_or_default();
    ui.set_rx_versions(ModelRc::new(VecModel::from(versions)));

    let profiles = roblox_tools::profiles(backend.id());
    let names: Vec<SharedString> = profiles.iter().map(|p| p.name.clone().into()).collect();
    let mut choices = vec![SharedString::from("No profile")];
    choices.extend(names.iter().cloned());
    let index_of = |name: Option<&String>| -> i32 {
        name.and_then(|n| profiles.iter().position(|p| &p.name == n))
            .map_or(0, |i| i as i32 + 1)
    };
    let game_profiles = &state.settings.game_profiles;
    ui.set_rx_everyday(index_of(game_profiles.get(&0)));
    let h = History::load();
    let rows: Vec<GameProfileRow> = h
        .games_played()
        .iter()
        .take(10)
        .map(|g| GameProfileRow {
            place: g.place_id.to_string().into(),
            name: h.name_of(g).into(),
            index: index_of(game_profiles.get(&g.place_id)),
        })
        .collect();
    ui.set_rx_profiles(ModelRc::new(VecModel::from(names)));
    ui.set_rx_choices(ModelRc::new(VecModel::from(choices)));
    ui.set_rx_game_profiles(ModelRc::new(VecModel::from(rows)));
}

/// Asks status.roblox.com in the background.
pub fn check_health(ui: &AppWindow) {
    ui.set_rx_health_checking(true);
    let weak = ui.as_weak();
    let _ = std::thread::Builder::new()
        .name("feline-status".into())
        .spawn(move || {
            let result = roblox_tools::status();
            let _ = weak.upgrade_in_event_loop(move |ui| {
                ui.set_rx_health_checking(false);
                match result {
                    Ok(s) => {
                        ui.set_rx_health_ok(s.all_good());
                        ui.set_rx_health(if s.all_good() {
                            "Roblox is up: all systems purring 🐾".into()
                        } else {
                            s.headline.into()
                        });
                        let problems: Vec<SharedString> = s
                            .problems
                            .iter()
                            .map(|(name, state)| format!("{name}: {state}").into())
                            .collect();
                        ui.set_rx_health_problems(ModelRc::new(VecModel::from(problems)));
                    }
                    Err(e) => {
                        ui.set_rx_health_ok(false);
                        ui.set_rx_health(e.into());
                    }
                }
            });
        });
}

fn installation(shared: &Shared) -> Option<feline_core::runtime::Installation> {
    let state = lock(shared);
    selected(&state).map(|f| f.installation.clone())
}

pub fn save_profile(ui: &AppWindow, shared: &Shared, name: &str) {
    let Some(inst) = installation(shared) else { return };
    let message = match roblox_tools::save_profile(&inst, &DetectContext::host(), name) {
        Ok(()) => format!("Saved the current settings as \"{}\".", name.trim()),
        Err(e) => format!("Couldn't save the profile: {e}"),
    };
    ui.set_rx_message(message.into());
    refresh(ui, shared);
}

pub fn apply_profile(ui: &AppWindow, shared: &Shared, name: &str) {
    let Some(inst) = installation(shared) else { return };
    let backend = inst.backend.id();
    let Some(profile) = roblox_tools::profiles(backend).into_iter().find(|p| p.name == name) else {
        return;
    };
    let message = match roblox_tools::apply_profile(&inst, &DetectContext::host(), &profile) {
        Ok(0) => format!("\"{name}\" matches the current settings already."),
        Ok(n) => format!("Switched to \"{name}\" ({n} settings changed)."),
        Err(e) => format!("Couldn't switch to \"{name}\": {e}"),
    };
    ui.set_rx_message(message.into());
    crate::settings_page::refresh(ui, shared);
}

pub fn delete_profile(ui: &AppWindow, shared: &Shared, name: &str) {
    let Some(inst) = installation(shared) else { return };
    roblox_tools::delete_profile(inst.backend.id(), name);
    lock(shared).settings.game_profiles.retain(|_, p| p != name);
    let _ = crate::settings_page::save_own(shared);
    ui.set_rx_message(format!("Deleted \"{name}\".").into());
    refresh(ui, shared);
}

/// `index` 0 = no profile, else the profile at `index - 1`.
pub fn set_game_profile(ui: &AppWindow, shared: &Shared, place: &str, index: i32) {
    let (Some(inst), Ok(place)) = (installation(shared), place.parse::<u64>()) else {
        return;
    };
    let profiles = roblox_tools::profiles(inst.backend.id());
    {
        let mut state = lock(shared);
        let map = &mut state.settings.game_profiles;
        match usize::try_from(index - 1).ok().and_then(|i| profiles.get(i)) {
            Some(p) => {
                map.insert(place, p.name.clone());
            }
            None => {
                map.remove(&place);
            }
        }
    }
    let _ = crate::settings_page::save_own(shared);
    refresh(ui, shared);
}

pub fn repair(ui: &AppWindow, shared: &Shared) {
    let Some(inst) = installation(shared) else { return };
    let message = match feline_core::runtime::roblox::repair(&inst, &DetectContext::host()) {
        Ok(m) => m,
        Err(e) => format!("Couldn't repair: {e}"),
    };
    ui.set_rx_message(message.into());
}

// ---------------------------------------------------------------------------
// Server picker
// ---------------------------------------------------------------------------

/// Opens the server picker for a game (from Home).
pub fn open_servers(ui: &AppWindow, shared: &Shared, place: &str) {
    let Ok(place_id) = place.parse::<u64>() else { return };
    let h = History::load();
    let name = h
        .games_played()
        .iter()
        .find(|g| g.any_place_in(&[place_id]))
        .map(|g| h.name_of(g))
        .unwrap_or_else(|| format!("Place {place_id}"));
    lock(shared).server_place = Some(place_id);
    ui.set_servers_game(name.into());
    ui.set_page(Page::Servers);
    load_servers(ui, shared, ui.get_servers_sort());
}

pub fn load_servers(ui: &AppWindow, shared: &Shared, sort: i32) {
    let Some(place_id) = lock(shared).server_place else { return };
    ui.set_servers_loading(true);
    ui.set_servers_error(SharedString::new());
    // A country picked: RoValra's list of servers there instead.
    if let Some(country) = crate::more::chosen_country(ui) {
        crate::more::load_region_servers(ui, place_id, country);
        return;
    }
    let weak = ui.as_weak();
    let _ = std::thread::Builder::new()
        .name("feline-servers".into())
        .spawn(move || {
            let result = roblox_tools::servers(place_id);
            let _ = weak.upgrade_in_event_loop(move |ui| {
                ui.set_servers_loading(false);
                match result {
                    Ok(mut list) => {
                        sort_servers(&mut list, sort);
                        let rows: Vec<ServerRow> = list.iter().map(row).collect();
                        ui.set_servers(ModelRc::new(VecModel::from(rows)));
                    }
                    Err(e) => {
                        ui.set_servers(ModelRc::new(VecModel::from(Vec::<ServerRow>::new())));
                        ui.set_servers_error(e.into());
                    }
                }
            });
        });
}

fn sort_servers(list: &mut [Server], sort: i32) {
    match sort {
        1 => list.sort_by_key(|s| std::cmp::Reverse(s.playing)),
        2 => list.sort_by_key(|s| s.ping.unwrap_or(u32::MAX)),
        _ => list.sort_by_key(|s| s.playing),
    }
}

fn row(s: &Server) -> ServerRow {
    let mut detail = Vec::new();
    if let Some(p) = s.ping {
        detail.push(format!("{p} ms ping"));
    }
    if let Some(f) = s.fps {
        detail.push(format!("{f:.0} FPS"));
    }
    ServerRow {
        id: s.id.clone().into(),
        players: format!("{} / {} players", s.playing, s.max).into(),
        detail: detail.join(" · ").into(),
        fill: if s.max > 0 { s.playing as f32 / s.max as f32 } else { 0.0 },
    }
}

pub fn join_server(ui: &AppWindow, shared: &Shared, server: &str) {
    let Some(place_id) = lock(shared).server_place else { return };
    let target = if server.is_empty() {
        GameLink::Experience { place_id }
    } else {
        GameLink::Uri {
            uri: roblox_tools::server_link(place_id, server),
            place_id: Some(place_id),
        }
    };
    ui.set_page(Page::Home);
    crate::gui::start_launch(ui, shared, LaunchTarget::Game(target));
}

/// Starts the game in the server `server_id` of `place_id`.
pub(crate) fn join_exact(ui: &AppWindow, shared: &Shared, place_id: u64, server_id: &str) {
    let target = GameLink::Uri {
        uri: roblox_tools::server_link(place_id, server_id),
        place_id: Some(place_id),
    };
    ui.set_page(Page::Home);
    crate::gui::start_launch(ui, shared, LaunchTarget::Game(target));
}

/// Home: back into the server you just left.
pub fn rejoin(ui: &AppWindow, shared: &Shared) {
    if let Some(last) = roblox_tools::LastServer::load() {
        join_exact(ui, shared, last.place_id, &last.job_id);
    }
}

/// Finds a server of `place_id` on a background thread (never `avoid`),
/// then joins it, or says why not.
fn find_and_join(ui: &AppWindow, shared: &Shared, place_id: u64, rule: roblox_tools::Pick, avoid: Option<String>) {
    ui.set_servers_loading(true);
    let weak = ui.as_weak();
    let shared = shared.clone();
    let _ = std::thread::Builder::new()
        .name("feline-quick-join".into())
        .spawn(move || {
            let picked = roblox_tools::servers(place_id).map(|list| {
                roblox_tools::pick_server(&list, rule, avoid.as_deref()).map(|s| s.id.clone())
            });
            let _ = weak.upgrade_in_event_loop(move |ui| {
                ui.set_servers_loading(false);
                match picked {
                    Ok(Some(id)) => join_exact(&ui, &shared, place_id, &id),
                    // No other public server: let Roblox pick (or start) one.
                    Ok(None) => {
                        ui.set_page(Page::Home);
                        crate::gui::start_launch(
                            &ui,
                            &shared,
                            LaunchTarget::Game(GameLink::Experience { place_id }),
                        );
                    }
                    Err(e) => {
                        ui.set_servers_error(e.into());
                        ui.set_launch_detail(SharedString::from("Couldn't load the server list, so Feline didn't hop."));
                    }
                }
            });
        });
}

/// Home: a different public server of the game you just left.
pub fn server_hop(ui: &AppWindow, shared: &Shared) {
    if let Some(last) = roblox_tools::LastServer::load() {
        find_and_join(ui, shared, last.place_id, roblox_tools::Pick::Emptiest, Some(last.job_id));
    }
}

/// Server picker: the quietest server (0) or the one with the best ping (1).
pub fn quick_join(ui: &AppWindow, shared: &Shared, rule: i32) {
    let Some(place_id) = lock(shared).server_place else { return };
    let rule = if rule == 1 { roblox_tools::Pick::BestPing } else { roblox_tools::Pick::Emptiest };
    find_and_join(ui, shared, place_id, rule, None);
}

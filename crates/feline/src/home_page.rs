// SPDX-License-Identifier: GPL-3.0-or-later
//! The Home page's personal parts: the greeting, playtime, favourites and
//! recently played games (from Roblox's logs, see `feline_core::history`),
//! and starting one of those games again.

use feline_core::history::{self, GameSummary, History};
use feline_core::link::GameLink;
use feline_core::runtime::{DetectContext, LaunchTarget, detect_all};
use slint::{ComponentHandle, Image, ModelRc, VecModel};

use crate::gui::{AppWindow, GameCard, Shared, TopGame, lock};

/// Every folder Roblox writes player logs to, for every runtime found.
pub fn log_roots(shared: &Shared) -> Vec<std::path::PathBuf> {
    let ctx = DetectContext::host();
    let state = lock(shared);
    let mut roots: Vec<_> = state
        .found
        .iter()
        .flat_map(|f| f.installation.roblox_log_roots(&ctx))
        .collect();
    drop(state);
    if roots.is_empty() {
        roots = detect_all(&ctx)
            .iter()
            .flat_map(|i| i.roblox_log_roots(&ctx))
            .collect();
    }
    // Feline's own runtime and Sober keep logs a few folders down.
    let mut all = Vec::new();
    for root in roots {
        collect_log_dirs(&root, 5, &mut all);
    }
    all.sort();
    all.dedup();
    all
}

fn collect_log_dirs(dir: &std::path::Path, depth: u32, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut has_logs = false;
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            if depth > 0 {
                collect_log_dirs(&path, depth - 1, out);
            }
        } else if !has_logs && entry.file_name().to_string_lossy().ends_with(".log") {
            has_logs = true;
        }
    }
    if has_logs {
        out.push(dir.to_owned());
    }
}

/// A friendly line for the time of day.
pub fn greeting() -> &'static str {
    let local = history::now() as i64 + history::local_offset_seconds();
    match local.rem_euclid(86_400) / 3600 {
        5..=11 => "Morning stretch? ☀️",
        12..=17 => "Ready to pounce?",
        18..=22 => "Evening prowl? 🌆",
        _ => "Late-night zoomies? 🌙",
    }
}

/// "today", "yesterday", "3 days ago".
fn when(last: u64, today_start: u64) -> String {
    if last >= today_start {
        "today".to_owned()
    } else if last + 86_400 >= today_start {
        "yesterday".to_owned()
    } else {
        format!("{} days ago", (today_start - last) / 86_400 + 1)
    }
}

fn card(h: &History, g: &GameSummary, today_start: u64, hidden: &[u64]) -> GameCard {
    let icon = history::icon_file(g.universe_id).and_then(|p| Image::load_from_path(&p).ok());
    GameCard {
        place: g.place_id.to_string().into(),
        name: h.name_of(g).into(),
        detail: format!(
            "{} · {}",
            history::duration_label(g.seconds),
            when(g.last_played, today_start)
        )
        .into(),
        has_icon: icon.is_some(),
        icon: icon.unwrap_or_default(),
        favourite: g.any_place_in(&h.favourites),
        hidden: g.any_place_in(hidden),
    }
}

/// Reads new logs and fills the Home page. Names and icons Feline doesn't
/// have yet are fetched in the background, and the page refreshed after.
pub fn refresh(ui: &AppWindow, shared: &Shared) {
    crate::more::refresh(ui);
    ui.set_greeting(greeting().into());
    let roots = log_roots(shared);
    let mut h = History::load();
    if h.scan(&roots) {
        let _ = h.save();
    }
    show(ui, shared, &h);

    let regions = lock(shared).settings.discord.region;
    if h.unnamed_universes().is_empty() && !regions {
        return;
    }
    let weak = ui.as_weak();
    let shared = shared.clone();
    let _ = std::thread::Builder::new()
        .name("feline-history".into())
        .spawn(move || {
            let mut h = History::load();
            if history::fetch_missing(&mut h, regions) {
                let _ = h.save();
                let _ = weak.upgrade_in_event_loop(move |ui| {
                    let h = History::load();
                    show(&ui, &shared, &h);
                });
            }
        });
}

fn show(ui: &AppWindow, shared: &Shared, h: &History) {
    let (today, week) = history::day_and_week_start(history::now(), history::local_offset_seconds());
    ui.set_play_today(history::duration_label(h.played_since(today)).into());
    ui.set_play_week(history::duration_label(h.played_since(week)).into());
    ui.set_play_total(history::duration_label(h.played_since(0)).into());

    let hidden = lock(shared).settings.discord.hidden.clone();
    let games = h.games_played();
    let recent: Vec<GameCard> = games.iter().take(6).map(|g| card(h, g, today, &hidden)).collect();
    // In the order they were pinned; a game pinned from two of its places
    // shows once.
    let mut pinned: Vec<&GameSummary> = Vec::new();
    for p in &h.favourites {
        if let Some(g) = games.iter().find(|g| g.any_place_in(&[*p]))
            && !pinned.iter().any(|x| std::ptr::eq(*x, g))
        {
            pinned.push(g);
        }
    }
    let favourites: Vec<GameCard> = pinned.iter().map(|g| card(h, g, today, &hidden)).collect();
    let mut by_time = games.clone();
    by_time.sort_by_key(|g| std::cmp::Reverse(g.seconds));
    let most = by_time.first().map_or(1, |g| g.seconds.max(1));
    let top: Vec<TopGame> = by_time
        .iter()
        .take(5)
        .map(|g| TopGame {
            name: h.name_of(g).into(),
            time: history::duration_label(g.seconds).into(),
            share: g.seconds as f32 / most as f32,
        })
        .collect();
    // For the Discord page's "hide" list: the 12 most recent games.
    let discord: Vec<GameCard> = games.iter().take(12).map(|g| card(h, g, today, &hidden)).collect();

    ui.set_recent_games(ModelRc::new(VecModel::from(recent)));
    ui.set_favourite_games(ModelRc::new(VecModel::from(favourites)));
    ui.set_top_games(ModelRc::new(VecModel::from(top)));
    ui.set_discord_games(ModelRc::new(VecModel::from(discord)));
}

pub fn toggle_favourite(ui: &AppWindow, shared: &Shared, place: &str) {
    let Ok(place) = place.parse() else { return };
    let mut h = History::load();
    h.toggle_favourite(place);
    let _ = h.save();
    show(ui, shared, &h);
}

/// Starts a game from Home (recently played or a favourite).
pub fn play(ui: &AppWindow, shared: &Shared, place: &str) {
    if let Ok(place_id) = place.parse() {
        crate::gui::start_launch(ui, shared, LaunchTarget::Game(GameLink::Experience { place_id }));
    }
}

/// Discord page: show this game as "something secret", or not.
pub fn toggle_hidden(ui: &AppWindow, shared: &Shared, place: &str) {
    let Ok(place) = place.parse::<u64>() else { return };
    {
        let mut state = lock(shared);
        // The whole game: unhiding removes every place of it.
        let places = History::load().places_of_game(place);
        let hidden = &mut state.settings.discord.hidden;
        if hidden.iter().any(|p| places.contains(p)) {
            hidden.retain(|p| !places.contains(p));
        } else {
            hidden.push(place);
        }
    }
    let _ = crate::settings_page::save_own(shared);
    show(ui, shared, &History::load());
}

pub fn set_discord_text(shared: &Shared, text: &str) {
    lock(shared).settings.discord.text = text.chars().take(128).collect();
    let _ = crate::settings_page::save_own(shared);
}

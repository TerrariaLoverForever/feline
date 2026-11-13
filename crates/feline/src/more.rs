// SPDX-License-Identifier: GPL-3.0-or-later
//! Bloxstrap-style extras (More in more.slint): game shortcuts, FastFlag
//! presets and JSON, classic cursors, taking over a bootstrapper's settings.

use feline_core::fflags::{self, Choices, Preset};
use feline_core::history::History;
use feline_core::mods;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::gui::{AppWindow, IconRow, LibraryRow, More, ServerRow, Shared, VisitRow, lock};

/// The countries the Servers page offers: (code, name), loaded once.
static COUNTRIES: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());

/// Bootstrappers on this PC with settings Feline can take over.
fn straps() -> Vec<(&'static str, std::path::PathBuf)> {
    let Some(base) = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from) else {
        return Vec::new();
    };
    mods::BOOTSTRAPPERS
        .iter()
        .map(|&name| (name, base.join(name)))
        .filter(|(_, dir)| dir.join("Settings.json").is_file())
        .collect()
}

/// Fills what the pages show.
pub fn refresh(ui: &AppWindow) {
    let more = ui.global::<More>();
    let names: Vec<SharedString> = straps().into_iter().map(|(n, _)| n.into()).collect();
    more.set_straps(ModelRc::new(VecModel::from(names)));
    let now = feline_core::history::now();
    let visits: Vec<VisitRow> = feline_core::serverinfo::visits()
        .into_iter()
        .take(5)
        .map(|v| {
            let ago = match now.saturating_sub(v.at) / 60 {
                0..=1 => "just now".to_owned(),
                m @ 2..=59 => format!("{m} min ago"),
                m if m < 24 * 60 => format!("{} h ago", m / 60),
                m => format!("{} days ago", m / (24 * 60)),
            };
            let detail = if v.region.is_empty() { ago } else { format!("{} · {ago}", v.region) };
            VisitRow {
                game: if v.game.is_empty() { format!("Place {}", v.place_id).into() } else { v.game.into() },
                place: v.place_id.to_string().into(),
                job: v.job_id.into(),
                detail: detail.into(),
            }
        })
        .collect();
    more.set_visits(ModelRc::new(VecModel::from(visits)));
    more.set_sessions(ModelRc::new(VecModel::from(sessions())));
    let regions: Vec<SharedString> = std::iter::once(SharedString::from("Any region"))
        .chain(COUNTRIES.lock().unwrap_or_else(|p| p.into_inner()).iter().map(|(_, n)| n.into()))
        .collect();
    more.set_regions(ModelRc::new(VecModel::from(regions)));
}

/// The Mods page's colour pieces and cursor sets.
pub fn refresh_mods(ui: &AppWindow) {
    let more = ui.global::<More>();
    let colours = mods::mods_dir().map(|d| mods::piece_colours(&d)).unwrap_or([None; 3]);
    let hex: Vec<SharedString> = colours.iter().map(|c| c.map(|c| format!("#{c:06X}")).unwrap_or_default().into()).collect();
    let swatches: Vec<slint::Color> = colours
        .iter()
        .map(|c| match c {
            Some(c) => slint::Color::from_rgb_u8((c >> 16) as u8, (c >> 8) as u8, *c as u8),
            None => slint::Color::from_argb_u8(0, 0, 0, 0),
        })
        .collect();
    more.set_pieces(ModelRc::new(VecModel::from(hex)));
    more.set_piece_colours(ModelRc::new(VecModel::from(swatches)));
    let sets: Vec<SharedString> = mods::cursor_sets().into_iter().map(SharedString::from).collect();
    more.set_cursor_sets(ModelRc::new(VecModel::from(sets)));
}

/// Your last sessions, for Home.
fn sessions() -> Vec<VisitRow> {
    let history = History::load();
    let offset = feline_core::history::local_offset_seconds();
    let games = history.games_played();
    history
        .recent_sessions(8)
        .into_iter()
        .map(|s| {
            let game = games
                .iter()
                .find(|g| g.any_place_in(&[s.place_id]))
                .map(|g| history.name_of(g))
                .unwrap_or_else(|| format!("Place {}", s.place_id));
            let local = s.start as i64 + offset;
            let days = local.div_euclid(86_400);
            let minutes = local.rem_euclid(86_400) / 60;
            // 1970-01-01 was a Thursday.
            let weekday = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"][days.rem_euclid(7) as usize];
            let (_, month, day) = civil(days);
            let month = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"][(month - 1) as usize];
            let mut detail = format!(
                "{weekday} {day} {month}, {:02}:{:02} · {}",
                minutes / 60,
                minutes % 60,
                feline_core::history::duration_label(s.seconds())
            );
            if let Some(region) = s.server.as_ref().and_then(|ip| history.regions.get(ip)) {
                detail.push_str(&format!(" · {region}"));
            }
            VisitRow { game: game.into(), detail: detail.into(), place: s.place_id.to_string().into(), job: SharedString::new() }
        })
        .collect()
}

/// Days since 1970 → (year, month, day).
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// The Studio folder icons and mods go into (Roblox's on Windows,
/// Vinegar's on Linux).
fn studio_dir() -> Option<std::path::PathBuf> {
    feline_core::studio::find(std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from).as_deref())
        .and_then(|e| e.parent().map(std::path::Path::to_path_buf))
}

/// The Studio icons matching `filter` (at most 30 shown).
fn filter_icons(ui: &AppWindow, filter: &str) {
    let more = ui.global::<More>();
    let Some(studio) = studio_dir() else { return };
    let all = feline_core::studio::class_icons(&studio);
    more.set_icon_count(all.len() as i32);
    let changed = mods::mods_dir().map(|d| feline_core::studio::changed_icons(&d)).unwrap_or_default();
    let filter = filter.trim().to_ascii_lowercase();
    let rows: Vec<IconRow> = all
        .into_iter()
        .filter(|(name, _)| if filter.is_empty() { changed.contains(name) } else { name.to_ascii_lowercase().contains(&filter) })
        .take(30)
        .map(|(name, path)| {
            let own = mods::mods_dir()
                .map(|d| d.join(feline_core::studio::ICONS_MOD).join(path.strip_prefix(&studio).unwrap_or(&path)))
                .filter(|p| p.is_file());
            let icon = slint::Image::load_from_path(own.as_deref().unwrap_or(&path)).unwrap_or_default();
            IconRow { changed: changed.contains(&name), name: name.into(), icon }
        })
        .collect();
    more.set_icons(ModelRc::new(VecModel::from(rows)));
}

fn refresh_sky(ui: &AppWindow) {
    let more = ui.global::<More>();
    let preview = mods::mods_dir()
        .map(|d| d.join(feline_core::sky::SKY_MOD).join("preview.png"))
        .filter(|p| p.is_file())
        .and_then(|p| slint::Image::load_from_path(&p).ok());
    more.set_has_sky(preview.is_some());
    if let Some(preview) = preview {
        more.set_sky_preview(preview);
    }
}

fn put_sky(ui: &AppWindow, shared: &Shared, faces: [feline_core::png::Rgba; 6], name: &str) {
    let Some(dir) = mods::mods_dir() else { return };
    match feline_core::sky::install(&dir, &faces) {
        Ok(()) => {
            crate::mods_page::enable(shared, feline_core::sky::SKY_MOD);
            ui.set_mods_status(format!("The sky is {name} now. It goes in when you press Play.").into());
        }
        Err(e) => ui.set_mods_status(format!("The sky couldn't be made: {e}").into()),
    }
    refresh_sky(ui);
    crate::mods_page::refresh(ui, shared);
}

/// Roblox Studio's card.
fn refresh_studio(ui: &AppWindow) {
    let more = ui.global::<More>();
    let exe = feline_core::studio::find(std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from).as_deref());
    more.set_studio_found(if cfg!(windows) { exe.is_some() } else { feline_core::studio::vinegar().is_some() });
    #[cfg(windows)]
    more.set_studio_version(exe.as_deref().and_then(feline_win::file_version).unwrap_or_default().into());
    let set = feline_core::studio::flags();
    more.set_studio_flags(if set.is_empty() { SharedString::new() } else { feline_core::studio::flags_json(&set).into() });
}

/// Opens Roblox Studio (in the background: mods may take a moment).
fn open_studio(ui: &AppWindow, settings: feline_core::settings::Settings) {
    ui.global::<More>().set_studio_status("Opening Studio…".into());
    let weak = ui.as_weak();
    let _ = std::thread::Builder::new().name("feline-studio".into()).spawn(move || {
        let text = match crate::companion::launch_studio(&settings, None) {
            Ok(lines) => lines.join(" "),
            Err(e) => format!("Studio didn't open: {e}."),
        };
        let _ = weak.upgrade_in_event_loop(move |ui| ui.global::<More>().set_studio_status(text.into()));
    });
}

/// The library as last loaded, with each mod's picture file.
static LIBRARY: std::sync::Mutex<Vec<(feline_core::modlibrary::LibraryMod, Option<std::path::PathBuf>)>> =
    std::sync::Mutex::new(Vec::new());
/// The page shown (from 0) and the per-page choice (index into PER_PAGE).
static LIBRARY_VIEW: std::sync::Mutex<(usize, usize)> = std::sync::Mutex::new((0, 0));
const PER_PAGE: [usize; 4] = [6, 9, 12, 24];

fn load_library(ui: &AppWindow, froststrap: bool) {
    let more = ui.global::<More>();
    more.set_library_loading(true);
    more.set_library_status(SharedString::new());
    let weak = ui.as_weak();
    let _ = std::thread::Builder::new().name("feline-library".into()).spawn(move || {
        let (list, problems) = feline_core::modlibrary::load(froststrap);
        let with_thumbs: Vec<_> = list.into_iter().map(|m| {
            let thumb = feline_core::modlibrary::thumbnail(&m);
            (m, thumb)
        }).collect();
        let _ = weak.upgrade_in_event_loop(move |ui| {
            let more = ui.global::<More>();
            more.set_library_loading(false);
            let status = if problems.is_empty() { String::new() } else { format!("Not everything loaded: {}.", problems.join("; ")) };
            more.set_library_status(status.into());
            *LIBRARY.lock().unwrap_or_else(|p| p.into_inner()) = with_thumbs;
            LIBRARY_VIEW.lock().unwrap_or_else(|p| p.into_inner()).0 = 0;
            show_library_page(&ui);
        });
    });
}

/// Shows the library's current page.
fn show_library_page(ui: &AppWindow) {
    let more = ui.global::<More>();
    let list = LIBRARY.lock().unwrap_or_else(|p| p.into_inner());
    let (mut page, per_index) = *LIBRARY_VIEW.lock().unwrap_or_else(|p| p.into_inner());
    let per = PER_PAGE[per_index.min(PER_PAGE.len() - 1)];
    let pages = list.len().div_ceil(per);
    page = page.min(pages.saturating_sub(1));
    let installed: Vec<String> = mods::mods_dir().map(|d| mods::list_packs(&d)).unwrap_or_default().into_iter().map(|p| p.name).collect();
    let rows: Vec<LibraryRow> = list
        .iter()
        .skip(page * per)
        .take(per)
        .map(|(m, thumb)| {
            let image = thumb.as_ref().and_then(|p| slint::Image::load_from_path(p).ok());
            let mut detail = Vec::new();
            if !m.author.is_empty() {
                detail.push(format!("by {}", m.author));
            }
            detail.push(format!("from {}'s list", m.source));
            LibraryRow {
                name: m.name.clone().into(),
                detail: detail.join(" · ").into(),
                description: m.description.clone().into(),
                has_thumb: image.is_some(),
                thumb: image.unwrap_or_default(),
                added: installed.contains(&feline_core::modlibrary::folder_name(m)),
            }
        })
        .collect();
    more.set_library(ModelRc::new(VecModel::from(rows)));
    more.set_library_page(page as i32);
    more.set_library_pages(if list.len() > per { pages as i32 } else { 0 });
    more.set_library_per_page(per_index as i32);
}

/// A mod of the page shown, by its place on the page.
fn library_mod(i: i32) -> Option<feline_core::modlibrary::LibraryMod> {
    let (page, per_index) = *LIBRARY_VIEW.lock().unwrap_or_else(|p| p.into_inner());
    let per = PER_PAGE[per_index.min(PER_PAGE.len() - 1)];
    let index = page * per + usize::try_from(i).ok()?;
    LIBRARY.lock().unwrap_or_else(|p| p.into_inner()).get(index).map(|(m, _)| m.clone())
}

/// A newer Feline, once found.
static UPDATE: std::sync::Mutex<Option<feline_core::selfupdate::Release>> = std::sync::Mutex::new(None);

/// Looks for a newer Feline in the background. `asked`: the user pressed
/// "Check now" (so "nothing new" and errors are worth saying).
fn check_update(ui: &AppWindow, asked: bool) {
    if asked {
        ui.global::<More>().set_update_status("Looking for a new Feline…".into());
    }
    let weak = ui.as_weak();
    let _ = std::thread::Builder::new().name("feline-update-check".into()).spawn(move || {
        let result = feline_core::selfupdate::check();
        let _ = weak.upgrade_in_event_loop(move |ui| {
            let more = ui.global::<More>();
            match result {
                Ok(Some(release)) => {
                    more.set_update_status(format!("Feline {} is out", release.version).into());
                    more.set_update_stage(1);
                    *UPDATE.lock().unwrap_or_else(|p| p.into_inner()) = Some(release);
                }
                Ok(None) if asked => more.set_update_status(format!("Feline {} is the newest", feline_core::VERSION).into()),
                Err(e) if asked => more.set_update_status(format!("Couldn't check: {e}").into()),
                _ => {}
            }
        });
    });
}

fn install_update(ui: &AppWindow) {
    let Some(release) = UPDATE.lock().unwrap_or_else(|p| p.into_inner()).clone() else { return };
    let Ok(exe) = std::env::current_exe() else { return };
    ui.global::<More>().set_update_status("Downloading…".into());
    let weak = ui.as_weak();
    let _ = std::thread::Builder::new().name("feline-update".into()).spawn(move || {
        let progress_ui = weak.clone();
        let mut last = u64::MAX;
        let result = feline_core::selfupdate::install(&release, &exe, &mut |done, total| {
            let percent = (done * 100).checked_div(total).unwrap_or(0);
            if percent != last {
                last = percent;
                let _ = progress_ui.upgrade_in_event_loop(move |ui| {
                    ui.global::<More>().set_update_status(format!("Downloading… {percent}%").into());
                });
            }
        });
        let _ = weak.upgrade_in_event_loop(move |ui| {
            let more = ui.global::<More>();
            match result {
                Ok(_) => {
                    more.set_update_status(format!("Feline {} is ready: restart to use it", release.version).into());
                    more.set_update_stage(2);
                }
                Err(e) => more.set_update_status(format!("The update didn't work: {e}").into()),
            }
        });
    });
}

/// Shows the switches' current values.
pub fn refresh_switches(ui: &AppWindow, shared: &Shared) {
    let more = ui.global::<More>();
    let state = lock(shared);
    more.set_tray(state.settings.tray);
    more.set_game_status(state.settings.game_status);
    more.set_auto_rejoin(state.settings.auto_rejoin);
    more.set_check_updates(state.settings.check_updates);
    more.set_studio_mods(state.settings.studio_mods);
    more.set_studio_discord(state.settings.studio_discord);
    more.set_studio_script(state.settings.studio_script);
    more.set_library_froststrap(state.settings.library_froststrap);
    more.set_no_crash_handler(state.settings.no_crash_handler);
    more.set_keep_running(state.settings.keep_running);
    more.set_place_files(state.settings.rbxl_previous.is_some());
    more.set_wrapper(state.settings.runtime_wrapper.clone().into());
    #[cfg(windows)]
    more.set_start_with_windows(feline_win::game::runs_at_login());
    #[cfg(not(windows))]
    more.set_start_with_windows(feline_core::shortcut::autostart_enabled());
}

/// Loads the list of countries with Roblox servers (once, in the background).
fn load_countries(ui: &AppWindow) {
    if !COUNTRIES.lock().unwrap_or_else(|p| p.into_inner()).is_empty() {
        return;
    }
    let weak = ui.as_weak();
    let _ = std::thread::Builder::new().name("feline-countries".into()).spawn(move || {
        if let Ok(list) = feline_core::serverinfo::countries() {
            *COUNTRIES.lock().unwrap_or_else(|p| p.into_inner()) = list;
            let _ = weak.upgrade_in_event_loop(|ui| refresh(&ui));
        }
    });
}

/// The country picked on the Servers page (its code), if any.
pub fn chosen_country(ui: &AppWindow) -> Option<String> {
    let i = ui.global::<More>().get_region_index();
    let i = usize::try_from(i).ok()?.checked_sub(1)?;
    COUNTRIES.lock().unwrap_or_else(|p| p.into_inner()).get(i).map(|(code, _)| code.clone())
}

/// The Servers page's list for a country (RoValra), newest servers first.
pub fn load_region_servers(ui: &AppWindow, place_id: u64, country: String) {
    let weak = ui.as_weak();
    let _ = std::thread::Builder::new().name("feline-region-servers".into()).spawn(move || {
        let result = feline_core::serverinfo::servers_in(place_id, &country);
        let _ = weak.upgrade_in_event_loop(move |ui| {
            ui.set_servers_loading(false);
            match result {
                Ok(list) if list.is_empty() => {
                    ui.set_servers(ModelRc::new(VecModel::from(Vec::<ServerRow>::new())));
                    ui.set_servers_error("No servers of this game are known in that country right now.".into());
                }
                Ok(list) => {
                    let now = feline_core::history::now();
                    let rows: Vec<ServerRow> = list
                        .iter()
                        .map(|s| {
                            let mut detail = Vec::new();
                            if let Some(started) = s.started {
                                detail.push(format!("up {}", feline_core::serverinfo::uptime_label(started, now)));
                            }
                            if let Some(v) = s.version {
                                detail.push(format!("game version {v}"));
                            }
                            ServerRow {
                                id: s.id.clone().into(),
                                players: s.place().into(),
                                detail: detail.join(" · ").into(),
                                fill: 0.0,
                            }
                        })
                        .collect();
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

/// Hooks up the callbacks.
pub fn wire(ui: &AppWindow, shared: &Shared) {
    let more = ui.global::<More>();
    refresh(ui);
    refresh_switches(ui, shared);
    load_countries(ui);
    refresh_studio(ui);
    more.on_open_studio({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                let settings = lock(&shared).settings.clone();
                open_studio(&ui, settings);
            }
        }
    });
    more.on_save_studio_flags({
        let weak = ui.as_weak();
        move |text| {
            let Some(ui) = weak.upgrade() else { return };
            let status = match feline_core::studio::flags_from_json(&text) {
                Ok(set) => match feline_core::studio::save_flags(&set) {
                    Ok(()) => format!("Saved {} flags. They go in when Feline opens Studio.", set.len()),
                    Err(e) => format!("Couldn't save them: {e}"),
                },
                Err(e) => format!("Hmm: {e}."),
            };
            ui.global::<More>().set_studio_status(status.into());
            refresh_studio(&ui);
        }
    });
    more.on_load_library({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                let froststrap = lock(&shared).settings.library_froststrap;
                load_library(&ui, froststrap);
            }
        }
    });
    more.on_get_library({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |i| {
            let Some(ui) = weak.upgrade() else { return };
            let Some(m) = library_mod(i) else { return };
            let Some(dir) = mods::mods_dir() else { return };
            ui.global::<More>().set_library_status(format!("Getting {}…", m.name).into());
            let weak = ui.as_weak();
            let shared = shared.clone();
            let _ = std::thread::Builder::new().name("feline-library-get".into()).spawn(move || {
                let result = feline_core::modlibrary::install(&m, &dir);
                let _ = weak.upgrade_in_event_loop(move |ui| {
                    let status = match result {
                        Ok(name) => {
                            crate::mods_page::enable(&shared, &name);
                            format!("{name} is in your mods and switched on. It goes in when you press Play.")
                        }
                        Err(e) => format!("{} didn't come in: {e}", m.name),
                    };
                    ui.global::<More>().set_library_status(status.into());
                    crate::mods_page::refresh(&ui, &shared);
                    show_library_page(&ui);
                });
            });
        }
    });
    refresh_sky(ui);
    filter_icons(ui, "");
    more.on_filter_icons({
        let weak = ui.as_weak();
        move |text| {
            if let Some(ui) = weak.upgrade() {
                filter_icons(&ui, &text);
            }
        }
    });
    more.on_replace_icon({
        let weak = ui.as_weak();
        move |class| {
            let Some(ui) = weak.upgrade() else { return };
            let (Some(dir), Some(studio)) = (mods::mods_dir(), studio_dir()) else { return };
            let Some(file) = crate::picker::pick_file(&format!("A picture for {class}"), "Pictures", &["*.png", "*.gif", "*.ani", "*.cur"]) else {
                return;
            };
            let result = std::fs::read(&file)
                .map_err(|e| e.to_string())
                .and_then(|d| feline_core::cursor::load(&d))
                .and_then(|a| feline_core::studio::set_class_icon(&dir, &studio, &class, &a.frames[0].image));
            let status = match result {
                Ok(()) => format!("{class} has your icon. It goes into Studio when Feline opens it."),
                Err(e) => format!("That didn't work: {e}"),
            };
            ui.global::<More>().set_studio_status(status.into());
            filter_icons(&ui, &class);
        }
    });
    more.on_reset_icon({
        let weak = ui.as_weak();
        move |class| {
            let Some(ui) = weak.upgrade() else { return };
            if let Some(dir) = mods::mods_dir() {
                feline_core::studio::clear_class_icon(&dir, &class);
            }
            filter_icons(&ui, &class);
        }
    });
    more.on_sky_preset({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |i| {
            let Some(ui) = weak.upgrade() else { return };
            let Some(preset) = feline_core::sky::Preset::ALL.get(i.max(0) as usize).copied() else { return };
            put_sky(&ui, &shared, preset.faces(), preset.name());
        }
    });
    more.on_sky_picture({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move || {
            let Some(ui) = weak.upgrade() else { return };
            let Some(file) = crate::picker::pick_file("Choose a wide picture for the sky", "PNG pictures", &["*.png"]) else {
                return;
            };
            match std::fs::read(&file).map_err(|e| e.to_string()).and_then(|d| feline_core::png::decode(&d)) {
                Ok(picture) => put_sky(&ui, &shared, feline_core::sky::from_picture(&picture), "your picture"),
                Err(e) => ui.set_mods_status(format!("That picture couldn't be used ({e}). Try a PNG.").into()),
            }
        }
    });
    more.on_sky_remove({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move || {
            let Some(ui) = weak.upgrade() else { return };
            if let Some(dir) = mods::mods_dir() {
                let _ = std::fs::remove_dir_all(dir.join(feline_core::sky::SKY_MOD));
            }
            ui.set_mods_status("Roblox's own sky comes back when you press Play.".into());
            refresh_sky(&ui);
            crate::mods_page::refresh(&ui, &shared);
        }
    });
    more.on_set_wrapper({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |text| {
            let Some(ui) = weak.upgrade() else { return };
            lock(&shared).settings.runtime_wrapper = text.trim().chars().take(300).collect();
            let _ = crate::settings_page::save_own(&shared);
            refresh_switches(&ui, &shared);
        }
    });
    more.on_library_go({
        let weak = ui.as_weak();
        move |page| {
            LIBRARY_VIEW.lock().unwrap_or_else(|p| p.into_inner()).0 = page.max(0) as usize;
            if let Some(ui) = weak.upgrade() {
                show_library_page(&ui);
            }
        }
    });
    more.on_library_set_per_page({
        let weak = ui.as_weak();
        move |choice| {
            *LIBRARY_VIEW.lock().unwrap_or_else(|p| p.into_inner()) = (0, choice.max(0) as usize);
            if let Some(ui) = weak.upgrade() {
                show_library_page(&ui);
            }
        }
    });
    more.set_windows(cfg!(windows));
    more.on_check_update({
        let weak = ui.as_weak();
        move || {
            if let Some(ui) = weak.upgrade() {
                check_update(&ui, true);
            }
        }
    });
    more.on_install_update({
        let weak = ui.as_weak();
        move || {
            if let Some(ui) = weak.upgrade() {
                install_update(&ui);
            }
        }
    });
    more.on_restart_feline(|| {
        if let Ok(exe) = std::env::current_exe() {
            let _ = feline_core::launch::detached(std::process::Command::new(exe)).spawn();
        }
        let _ = slint::quit_event_loop();
    });
    if lock(shared).settings.check_updates {
        check_update(ui, false);
    }
    more.on_rejoin_visit({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |place, job| {
            let Some(ui) = weak.upgrade() else { return };
            if let Ok(place) = place.parse::<u64>() {
                crate::roblox_page::join_exact(&ui, &shared, place, &job);
            }
        }
    });
    more.on_copy_invite({
        let weak = ui.as_weak();
        move |place, job| {
            let Some(ui) = weak.upgrade() else { return };
            let Ok(place) = place.parse::<u64>() else { return };
            let link = feline_core::serverinfo::invite_link(place, &job);
            let status = if feline_core::launch::copy_text(&link) {
                "Invite link copied. Anyone with Roblox who opens it lands in that server (if it's still running and has room).".to_owned()
            } else {
                format!("Couldn't reach the clipboard. Here's the link: {link}")
            };
            ui.global::<More>().set_home_status(status.into());
        }
    });
    more.on_pick_region({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |_| {
            if let Some(ui) = weak.upgrade() {
                crate::roblox_page::load_servers(&ui, &shared, ui.get_servers_sort());
            }
        }
    });
    more.on_set_piece({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |i, text| {
            let Some(ui) = weak.upgrade() else { return };
            let Some(dir) = mods::mods_dir() else { return };
            let Some(piece) = mods::Piece::ALL.get(i.max(0) as usize).copied() else { return };
            let text = text.trim();
            let colour = if text.is_empty() { None } else { feline_core::look::parse_hex(text) };
            if !text.is_empty() && colour.is_none() {
                ui.set_mods_status("That's not a colour Feline knows. Try # and six hex digits, like #ff8fb1.".into());
                return;
            }
            match mods::set_piece_colour(&dir, piece, colour) {
                Ok(()) => {
                    if colour.is_some() {
                        crate::mods_page::enable(&shared, mods::FELINE_COLOURS);
                    }
                    ui.set_mods_status("Got it! It goes in when you press Play.".into());
                }
                Err(e) => ui.set_mods_status(format!("That didn't work: {e}").into()),
            }
            crate::mods_page::refresh(&ui, &shared);
        }
    });
    for (which, f) in [
        ("save", mods::save_cursor_set as fn(&std::path::Path, &str) -> Result<(), String>),
        ("use", mods::use_cursor_set),
    ] {
        let weak = ui.as_weak();
        let shared = shared.clone();
        let handler = move |name: SharedString| {
            let Some(ui) = weak.upgrade() else { return };
            let Some(dir) = mods::mods_dir() else { return };
            let status = match f(&dir, &name) {
                Ok(()) if which == "save" => format!("Saved the cursor as \"{name}\"."),
                Ok(()) => {
                    crate::mods_page::enable(&shared, mods::FELINE_PICKS);
                    format!("Cursor \"{name}\" is picked. It goes in when you press Play.")
                }
                Err(e) => format!("That didn't work: {e}"),
            };
            ui.set_mods_status(status.into());
            crate::mods_page::refresh(&ui, &shared);
        };
        if which == "save" {
            more.on_save_cursor_set(handler);
        } else {
            more.on_use_cursor_set(handler);
        }
    }
    more.on_delete_cursor_set({
        let weak = ui.as_weak();
        move |name| {
            mods::delete_cursor_set(&name);
            if let Some(ui) = weak.upgrade() {
                refresh_mods(&ui);
            }
        }
    });
    more.on_set_switch({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |which, on| {
            let Some(ui) = weak.upgrade() else { return };
            {
                let mut state = lock(&shared);
                match which.as_str() {
                    "tray" => state.settings.tray = on,
                    "game-status" => state.settings.game_status = on,
                    "auto-rejoin" => state.settings.auto_rejoin = on,
                    "check-updates" => state.settings.check_updates = on,
                    "studio-mods" => state.settings.studio_mods = on,
                    "studio-discord" => state.settings.studio_discord = on,
                    "studio-script" => state.settings.studio_script = on,
                    "library-froststrap" => state.settings.library_froststrap = on,
                    "no-crash-handler" => state.settings.no_crash_handler = on,
                    "keep-running" => state.settings.keep_running = on,
                    #[cfg(windows)]
                    "start-with-windows" => {
                        let command = std::env::current_exe().ok().map(|e| format!("\"{}\" --minimized", e.display()));
                        feline_win::game::run_at_login(if on { command.as_deref() } else { None });
                    }
                    #[cfg(not(windows))]
                    "start-with-windows" => {
                        if let Ok(exe) = std::env::current_exe() {
                            let _ = feline_core::shortcut::set_autostart(on, &exe);
                        }
                    }
                    #[cfg(not(windows))]
                    "place-files" => {
                        let previous = state.settings.rbxl_previous.clone();
                        if let Ok(exe) = std::env::current_exe() {
                            match feline_core::shortcut::set_place_files(on, &exe, previous.as_deref()) {
                                Ok(before) if on => state.settings.rbxl_previous = Some(before.unwrap_or_default()),
                                Ok(_) => state.settings.rbxl_previous = None,
                                Err(_) => {}
                            }
                        }
                    }
                    #[cfg(windows)]
                    "place-files" => {
                        let previous = state.settings.rbxl_previous.clone();
                        match crate::companion::set_place_files(on, previous.as_deref()) {
                            Ok(before) if on => state.settings.rbxl_previous = Some(before.unwrap_or_default()),
                            Ok(_) => state.settings.rbxl_previous = None,
                            Err(_) => {}
                        }
                    }
                    _ => return,
                }
            }
            let _ = crate::settings_page::save_own(&shared);
            refresh_switches(&ui, &shared);
        }
    });
    more.on_make_shortcut({
        let weak = ui.as_weak();
        move |place, name| {
            if let Some(ui) = weak.upgrade() {
                make_shortcut(&ui, place.as_str(), name.as_str());
            }
        }
    });
    more.on_flags_preset({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |which| {
            let Some(ui) = weak.upgrade() else { return };
            let preset = match which.as_str() {
                "potato" => Preset::Potato,
                "pretty" => Preset::Pretty,
                _ => Preset::Balanced,
            };
            {
                let mut state = lock(&shared);
                let set = &mut state.settings.fflags;
                preset.apply(Choices::read(set)).write(set);
            }
            save_flags(&ui, &shared, "Done. The graphics tweaks above are set; they go in when you press Play.".to_owned());
        }
    });
    more.on_flags_import({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |text| {
            if let Some(ui) = weak.upgrade() {
                import_flags(&ui, &shared, text.as_str());
            }
        }
    });
    more.on_flags_import_file({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move || {
            let Some(ui) = weak.upgrade() else { return };
            let Some(file) = crate::picker::pick_file("Choose a FastFlag file", "JSON", &["*.json"]) else {
                return;
            };
            match std::fs::read_to_string(&file) {
                Ok(text) => import_flags(&ui, &shared, &text),
                Err(e) => ui.global::<More>().set_tweaks_status(format!("That file couldn't be read: {e}").into()),
            }
        }
    });
    more.on_flags_export({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move || {
            let Some(ui) = weak.upgrade() else { return };
            let set = lock(&shared).settings.fflags.clone();
            let status = match export_flags(&set) {
                Ok(path) => {
                    crate::mods_page::reveal(&path);
                    format!("Saved {} FastFlags to {}.", fflags::sanitize(&set).len(), path.display())
                }
                Err(e) => format!("Couldn't save them: {e}"),
            };
            ui.global::<More>().set_tweaks_status(status.into());
        }
    });
    more.on_classic_cursor({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |white| {
            let Some(ui) = weak.upgrade() else { return };
            let Some(dir) = mods::mods_dir() else { return };
            match mods::set_classic_cursor(&dir, white) {
                Ok(()) => {
                    crate::mods_page::enable(&shared, mods::FELINE_PICKS);
                    ui.set_mods_status("Classic arrow it is! It goes in when you press Play.".into());
                }
                Err(e) => ui.set_mods_status(format!("That didn't work: {e}").into()),
            }
            crate::mods_page::refresh(&ui, &shared);
        }
    });
    more.on_import_strap({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |name| {
            let Some(ui) = weak.upgrade() else { return };
            let Some((_, dir)) = straps().into_iter().find(|(n, _)| *n == name.as_str()) else { return };
            let result = {
                let mut state = lock(&shared);
                feline_core::strap_import::import_settings(&dir, &mut state.settings)
            };
            let status = match result {
                Ok(lines) if lines.is_empty() => format!("{name}'s settings were all Feline's defaults already."),
                Ok(lines) => match crate::settings_page::save_own(&shared) {
                    Ok(()) => format!("Took over from {name}: {}.", lines.join(", ")),
                    Err(e) => format!("Couldn't save: {e}"),
                },
                Err(e) => format!("Couldn't take over {name}'s settings: {e}"),
            };
            ui.global::<More>().set_settings_status(status.into());
            crate::settings_page::refresh(&ui, &shared);
            crate::tweaks::refresh(&ui, &shared);
        }
    });
}

fn save_flags(ui: &AppWindow, shared: &Shared, done: String) {
    let status = match crate::settings_page::save_own(shared) {
        Ok(()) => done,
        Err(e) => format!("Couldn't save: {e}"),
    };
    ui.global::<More>().set_tweaks_status(status.into());
    crate::tweaks::refresh(ui, shared);
}

fn import_flags(ui: &AppWindow, shared: &Shared, text: &str) {
    match fflags::from_json(text) {
        Ok((set, ignored)) => {
            let count = set.len();
            lock(shared).settings.fflags.extend(set);
            let mut line = match count {
                0 => "None of those are flags Roblox still reads, so nothing was added.".to_owned(),
                1 => "Added 1 FastFlag.".to_owned(),
                n => format!("Added {n} FastFlags."),
            };
            if !ignored.is_empty() {
                let shown: Vec<&str> = ignored.iter().take(4).map(String::as_str).collect();
                let more = ignored.len().saturating_sub(shown.len());
                line.push_str(&format!(
                    " Left out (Roblox ignores them now): {}{}.",
                    shown.join(", "),
                    if more > 0 { format!(" and {more} more") } else { String::new() }
                ));
            }
            save_flags(ui, shared, line);
        }
        Err(e) => ui.global::<More>().set_tweaks_status(format!("Hmm: {e}.").into()),
    }
}

fn export_flags(set: &fflags::FlagSet) -> Result<std::path::PathBuf, String> {
    let dir = feline_core::bugreport::output_dir().ok_or("no Downloads folder")?;
    let mut path = dir.join("Feline FastFlags.json");
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("Feline FastFlags ({n}).json"));
        n += 1;
    }
    std::fs::write(&path, fflags::to_json(set)).map_err(|e| e.to_string())?;
    Ok(path)
}

fn make_shortcut(ui: &AppWindow, place: &str, name: &str) {
    let Ok(place) = place.parse::<u64>() else { return };
    let history = History::load();
    let icon = history
        .games_played()
        .iter()
        .find(|g| g.place_id == place)
        .and_then(|g| feline_core::history::icon_file(g.universe_id));
    let status = match std::env::current_exe()
        .map_err(|e| e.to_string())
        .and_then(|exe| feline_core::shortcut::make(place, name, icon.as_deref(), &exe))
    {
        Ok(path) => format!(
            "Made a shortcut for {name}: {}. Double-click it to jump straight in.",
            path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
        ),
        Err(e) => format!("Couldn't make the shortcut: {e}"),
    };
    ui.global::<More>().set_home_status(status.into());
}

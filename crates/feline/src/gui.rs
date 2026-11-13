// SPDX-License-Identifier: GPL-3.0-or-later
//! The Feline window.
//!
//! How Rust and Slint fit together:
//! - `ui/*.slint` describes the interface. `build.rs` compiles it into Rust,
//!   and `include_modules!` below pulls that code in, giving us `AppWindow`
//!   with a `set_…` method for every `in property` and an `on_…` method for
//!   every `callback`.
//! - Slint runs on the main thread. Anything slow (detecting runtimes,
//!   starting one, waiting for the game to end) runs on a background thread
//!   and sends results back with `upgrade_in_event_loop`, so the window never
//!   freezes.
//! - State shared between those threads lives in an `Arc<Mutex<State>>`:
//!   `Arc` lets several threads own it, `Mutex` lets only one change it at a time.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use feline_core::launch::{self, GameExit};
use feline_core::paths;
use feline_core::runtime::{
    DetectContext, InstallKind, Installation, LaunchTarget, Origin, detect_all,
};
use feline_core::settings::{ACCOUNT_NOTICE_VERSION, AfterLaunch, Settings};
use feline_core::system::{CheckLevel, FlatpakNvidia, SessionType, SystemInfo};
use slint::{ModelRc, SharedString, VecModel};

slint::include_modules!();

use crate::settings_page::{self, Edit};

/// A detected runtime and its version (looked up once, during detection).
#[derive(Clone)]
pub(crate) struct Found {
    pub(crate) installation: Installation,
    pub(crate) version: Option<String>,
}

#[derive(Default)]
pub(crate) struct State {
    pub(crate) found: Vec<Found>,
    pub(crate) settings: Settings,
    detecting: bool,
    /// True from pressing Play until the runtime exits.
    pub(crate) busy: bool,
    last_log: Option<PathBuf>,
    /// The last system scan (None until the first one finishes).
    system: Option<SystemInfo>,
    scanning_system: bool,
    /// True while a Diagnostics action is running.
    diag_busy: bool,
    /// The last failed save on the Settings page: (runtime id, message).
    pub(crate) settings_message: Option<(String, String)>,
    /// The game whose servers the server picker shows.
    pub(crate) server_place: Option<u64>,
    /// When Play last asked "Roblox is already open, go ahead?".
    pub(crate) asked_to_confirm: Option<std::time::Instant>,
}

pub(crate) type Shared = Arc<Mutex<State>>;

/// Locks the state. If another thread panicked while holding the lock, the
/// data is still usable for our purposes, so we continue instead of crashing.
pub(crate) fn lock(shared: &Shared) -> MutexGuard<'_, State> {
    shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Open the window minimized (Feline started with Windows).
pub static START_MINIMIZED: AtomicBool = AtomicBool::new(false);

pub fn run() -> Result<(), slint::PlatformError> {
    let screenshot_run = std::env::var_os("FELINE_SCREENSHOT").is_some();
    // Feline is open already (maybe in the background): show that one.
    if !screenshot_run && crate::background::already_running() {
        return Ok(());
    }
    // Creating the window also sets up Slint's platform (Wayland or X11).
    let ui = AppWindow::new()?;
    // The app ID is how Wayland/X11 desktops match the window to Feline's
    // menu entry and icon. It needs the platform, and must be set before the
    // window is shown (which happens in `ui.run()`).
    slint::set_xdg_app_id(paths::APP_ID)?;
    ui.set_version(feline_core::VERSION.into());
    ui.set_windows(cfg!(windows));
    // Developer aid (screenshots, testing a page): FELINE_START_PAGE=system
    match std::env::var("FELINE_START_PAGE").as_deref() {
        Ok("runtimes") => ui.set_page(Page::Runtimes),
        Ok("system") => ui.set_page(Page::System),
        Ok("diagnostics") => ui.set_page(Page::Diagnostics),
        Ok("about") => ui.set_page(Page::About),
        Ok("settings") => ui.set_page(Page::Settings),
        Ok("mods") => ui.set_page(Page::Mods),
        Ok("tweaks") => ui.set_page(Page::Tweaks),
        Ok("studio") => ui.set_page(Page::Studio),
        Ok("library") => ui.set_page(Page::Library),
        _ => {}
    }
    if let Some(tab) = std::env::var("FELINE_START_TAB").ok().and_then(|t| t.parse().ok()) {
        ui.set_settings_tab(tab);
        ui.set_runtime_tab(tab);
        ui.set_about_tab(tab);
    }
    if let Ok(id) = std::env::var("FELINE_START_RUNTIME") {
        ui.set_page(Page::Runtimes);
        ui.set_runtime_open(id.into());
    }
    // Developer aid: FELINE_SCREENSHOT=file.ppm saves the window after
    // 2.5 s (for checking the design) and quits.
    if let Some(path) = std::env::var_os("FELINE_SCREENSHOT") {
        // FELINE_SCREENSHOT_SIZE=960x660 checks the layout in a small window.
        if let Some((w, h)) = std::env::var("FELINE_SCREENSHOT_SIZE").ok().and_then(|s| {
            let (w, h) = s.split_once('x')?;
            Some((w.parse::<f32>().ok()?, h.parse::<f32>().ok()?))
        }) {
            ui.window().set_size(slint::LogicalSize::new(w, h));
        }
        // FELINE_SCREENSHOT_SCROLL=600 scrolls the page down that far first.
        if let Some(by) = std::env::var("FELINE_SCREENSHOT_SCROLL").ok().and_then(|s| s.parse::<f32>().ok()) {
            let weak = ui.as_weak();
            slint::Timer::single_shot(std::time::Duration::from_millis(700), move || {
                let Some(ui) = weak.upgrade() else { return };
                let size = ui.window().size().to_logical(ui.window().scale_factor());
                ui.window().dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                    position: slint::LogicalPosition::new(size.width * 0.6, size.height * 0.5),
                    delta_x: 0.0,
                    delta_y: -by,
                });
            });
        }
        let weak = ui.as_weak();
        let delay = std::env::var("FELINE_SCREENSHOT_DELAY").ok().and_then(|d| d.parse().ok()).unwrap_or(2500);
        slint::Timer::single_shot(std::time::Duration::from_millis(delay), move || {
            let Some(ui) = weak.upgrade() else { return };
            match ui.window().take_snapshot() {
                Ok(image) => {
                    let mut ppm = format!("P6\n{} {}\n255\n", image.width(), image.height()).into_bytes();
                    for px in image.as_slice() {
                        ppm.extend_from_slice(&[px.r, px.g, px.b]);
                    }
                    let _ = std::fs::write(&path, ppm);
                }
                Err(e) => eprintln!("feline: screenshot failed: {e}"),
            }
            let _ = slint::quit_event_loop();
        });
    }
    let log_folder = paths::log_dir()
        .map(|p| tilde(&p))
        .unwrap_or_else(|| "~/.local/state/feline/logs".to_owned());
    ui.set_log_folder(log_folder.into());

    let mut state = State::default();
    match Settings::path().map(|p| Settings::load_from(&p)) {
        Some(Ok(settings)) => state.settings = settings,
        Some(Err(e)) => ui.set_runtimes_error(
            format!("Couldn't read your saved settings ({e}). Your runtime choice may be lost.")
                .into(),
        ),
        None => {}
    }
    ui.set_show_notice(state.settings.needs_account_notice());
    let shared: Shared = Arc::new(Mutex::new(state));

    // The fun extras: colours, season, desk cat, achievements, clear on exit.
    crate::extras::refresh(&ui, &shared);
    // Tweaks (FastFlags).
    crate::tweaks::wire(&ui, &shared);
    crate::tweaks::refresh(&ui, &shared);
    ui.global::<Extras>().on_set({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |which, value| {
            if let Some(ui) = weak.upgrade() {
                crate::extras::set(&ui, &shared, which.as_str(), value);
            }
        }
    });
    // Background music (About → Music).
    ui.global::<Extras>().on_music_add_file({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                crate::music::add_file(&ui, &shared);
            }
        }
    });
    ui.global::<Extras>().on_music_add_link({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |link| {
            if let Some(ui) = weak.upgrade() {
                crate::music::add_link(&ui, &shared, link.as_str());
            }
        }
    });
    ui.global::<Extras>().on_music_remove({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                crate::music::remove(&ui, &shared);
            }
        }
    });
    ui.global::<Extras>().on_music_open_folder(crate::music::open_folder);
    let _music_focus = crate::music::watch_focus(&ui);
    // Bloxstrap-style extras on several pages (More in more.slint).
    crate::more::wire(&ui, &shared);
    crate::accounts_page::wire(&ui);
    if ui.get_page() == Page::Library {
        ui.global::<More>().invoke_load_library();
    }
    // The window's font, background and glass (Looks in looks.slint).
    crate::looks::wire(&ui, &shared);
    // Settings → General → While you play (PlayExtras in play_extras.slint).
    {
        let extras = ui.global::<PlayExtras>();
        let handler = |f: fn(&AppWindow, &Shared)| {
            let weak = ui.as_weak();
            let shared = shared.clone();
            move || {
                if let Some(ui) = weak.upgrade() {
                    f(&ui, &shared);
                }
            }
        };
        extras.on_add_program(handler(crate::play_extras::add_program));
        extras.on_pin(handler(crate::play_extras::pin));
        extras.on_unpin(handler(crate::play_extras::unpin));
        let with_bool = |f: fn(&AppWindow, &Shared, bool)| {
            let weak = ui.as_weak();
            let shared = shared.clone();
            move |on| {
                if let Some(ui) = weak.upgrade() {
                    f(&ui, &shared, on);
                }
            }
        };
        extras.on_set_multi(with_bool(crate::play_extras::set_multi));
        extras.on_set_popup(with_bool(crate::play_extras::set_popup));
        extras.on_set_confirm(with_bool(crate::play_extras::set_confirm));
        extras.on_set_splash(with_bool(crate::play_extras::set_splash));
        extras.on_remove_program({
            let weak = ui.as_weak();
            let shared = shared.clone();
            move |i| {
                if let Some(ui) = weak.upgrade() {
                    crate::play_extras::remove_program(&ui, &shared, i);
                }
            }
        });
        extras.on_set_program_close({
            let weak = ui.as_weak();
            let shared = shared.clone();
            move |i, on| {
                if let Some(ui) = weak.upgrade() {
                    crate::play_extras::set_program_close(&ui, &shared, i, on);
                }
            }
        });
        extras.on_set_program_args({
            let shared = shared.clone();
            move |i, text| crate::play_extras::set_program_args(&shared, i, text.as_str())
        });
    }
    ui.global::<Extras>().on_set_accent_hex({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |text| {
            if let Some(ui) = weak.upgrade() {
                crate::extras::set_accent_hex(&ui, &shared, text.as_str());
            }
        }
    });

    ui.on_accept_notice({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move || {
            let Some(ui) = weak.upgrade() else { return };
            ui.set_show_notice(false);
            let mut state = lock(&shared);
            state.settings.account_notice = ACCOUNT_NOTICE_VERSION;
            let saved = match Settings::path() {
                Some(path) => state.settings.save_to(&path).map_err(|e| e.to_string()),
                None => Err("your home folder couldn't be found".to_owned()),
            };
            // Not fatal: the notice just shows again next time.
            if let Err(e) = saved {
                eprintln!("feline: couldn't save that the notice was accepted: {e}");
            }
        }
    });
    ui.on_quit(|| {
        // Fails only if the event loop already stopped, which is what we want anyway.
        let _ = slint::quit_event_loop();
    });

    ui.on_refresh_system({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                start_system_scan(&ui, &shared);
            }
        }
    });

    ui.on_system_tick({
        let weak = ui.as_weak();
        move || {
            if let Some(ui) = weak.upgrade() {
                crate::checkup::tick(&ui);
            }
        }
    });
    ui.on_open_link(|url| crate::checkup::open_link(url.as_str()));
    ui.on_investigate({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                crate::checkup::investigate(&ui, &shared);
            }
        }
    });
    ui.on_run_network_test({
        let weak = ui.as_weak();
        move || {
            if let Some(ui) = weak.upgrade() {
                crate::checkup::network_test(&ui);
            }
        }
    });
    ui.on_save_report_zip({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                start_diagnostic(&ui, &shared, DiagAction::SaveZip);
            }
        }
    });

    ui.on_settings_opened({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                settings_page::refresh(&ui, &shared);
            }
        }
    });
    ui.on_set_roblox_updates({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |on| {
            if let Some(ui) = weak.upgrade() {
                settings_page::set_roblox_updates(&ui, &shared, on);
            }
        }
    });
    ui.on_set_log_days({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |index| {
            if let Some(ui) = weak.upgrade() {
                settings_page::set_log_days(&ui, &shared, index);
            }
        }
    });
    ui.on_housekeeping({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |action| {
            if let Some(ui) = weak.upgrade() {
                settings_page::housekeeping(&ui, &shared, action.as_str());
            }
        }
    });
    ui.on_set_override({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |which, index| {
            if let Some(ui) = weak.upgrade() {
                settings_page::set_override(&ui, &shared, which.as_str(), index);
            }
        }
    });
    ui.on_set_link_handler({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |on| {
            if let Some(ui) = weak.upgrade() {
                settings_page::set_link_handler(&ui, &shared, on);
            }
        }
    });
    ui.on_set_presence({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |which, value| {
            if let Some(ui) = weak.upgrade() {
                settings_page::set_presence(&ui, &shared, which.as_str(), value);
            }
        }
    });
    ui.on_set_runtime_bool({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |tab, key, value| {
            if let Some(ui) = weak.upgrade() {
                settings_page::set_runtime_value(&ui, &shared, &tab, &key, Edit::Bool(value));
            }
        }
    });
    ui.on_set_runtime_choice({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |tab, key, index| {
            if let Some(ui) = weak.upgrade() {
                // A negative index can't happen with a dropdown; treat it as 0.
                let index = usize::try_from(index).unwrap_or(0);
                settings_page::set_runtime_value(&ui, &shared, &tab, &key, Edit::Choice(index));
            }
        }
    });
    ui.on_set_runtime_text({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |tab, key, value| {
            if let Some(ui) = weak.upgrade() {
                let edit = Edit::Text(value.to_string());
                settings_page::set_runtime_value(&ui, &shared, &tab, &key, edit);
            }
        }
    });
    ui.on_set_runtime_number({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |tab, key, value| {
            if let Some(ui) = weak.upgrade() {
                let edit = Edit::Number(i64::from(value));
                settings_page::set_runtime_value(&ui, &shared, &tab, &key, edit);
            }
        }
    });

    ui.on_run_doctor({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                start_diagnostic(&ui, &shared, DiagAction::Doctor);
            }
        }
    });
    ui.on_run_runtime_info({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                start_diagnostic(&ui, &shared, DiagAction::RuntimeInfo);
            }
        }
    });
    ui.on_build_report({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                start_diagnostic(&ui, &shared, DiagAction::BugReport);
            }
        }
    });

    ui.on_select_runtime({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |key| {
            if let Some(ui) = weak.upgrade() {
                select_runtime(&ui, &shared, key.as_str());
            }
        }
    });

    ui.on_rescan({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                start_detection(&ui, &shared);
            }
        }
    });

    ui.on_play({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                start_launch(&ui, &shared, LaunchTarget::Home);
            }
        }
    });

    ui.on_roblox_opened({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                let _ = &shared;
                crate::roblox_page::refresh(&ui, &shared); if ui.get_rx_health().is_empty() { crate::roblox_page::check_health(&ui); }
            }
        }
    });
    ui.on_rx_check_health({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                let _ = &shared;
                crate::roblox_page::check_health(&ui);
            }
        }
    });
    ui.on_rx_save_profile({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |n| {
            if let Some(ui) = weak.upgrade() {
                let _ = &shared;
                crate::roblox_page::save_profile(&ui, &shared, n.as_str());
            }
        }
    });
    ui.on_rx_apply_profile({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |n| {
            if let Some(ui) = weak.upgrade() {
                let _ = &shared;
                crate::roblox_page::apply_profile(&ui, &shared, n.as_str());
            }
        }
    });
    ui.on_rx_delete_profile({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |n| {
            if let Some(ui) = weak.upgrade() {
                let _ = &shared;
                crate::roblox_page::delete_profile(&ui, &shared, n.as_str());
            }
        }
    });
    ui.on_rx_set_game_profile({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |p, i| {
            if let Some(ui) = weak.upgrade() {
                let _ = &shared;
                crate::roblox_page::set_game_profile(&ui, &shared, p.as_str(), i);
            }
        }
    });
    ui.on_rx_repair({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                let _ = &shared;
                crate::roblox_page::repair(&ui, &shared);
            }
        }
    });
    ui.on_open_servers({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |p| {
            if let Some(ui) = weak.upgrade() {
                let _ = &shared;
                crate::roblox_page::open_servers(&ui, &shared, p.as_str());
            }
        }
    });
    ui.on_reload_servers({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |i| {
            if let Some(ui) = weak.upgrade() {
                let _ = &shared;
                crate::roblox_page::load_servers(&ui, &shared, i);
            }
        }
    });
    ui.on_join_server({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |id| {
            if let Some(ui) = weak.upgrade() {
                let _ = &shared;
                crate::roblox_page::join_server(&ui, &shared, id.as_str());
            }
        }
    });
    // Server tools: quick joins, rejoin and server hop.
    ui.set_can_rejoin(feline_core::roblox_tools::LastServer::load().is_some());
    ui.on_quick_join({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |rule| {
            if let Some(ui) = weak.upgrade() {
                crate::roblox_page::quick_join(&ui, &shared, rule);
            }
        }
    });
    ui.on_rejoin({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                crate::roblox_page::rejoin(&ui, &shared);
            }
        }
    });
    ui.on_server_hop({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                crate::roblox_page::server_hop(&ui, &shared);
            }
        }
    });
    ui.on_home_opened({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                crate::home_page::refresh(&ui, &shared);
            }
        }
    });
    ui.on_play_place({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |place| {
            if let Some(ui) = weak.upgrade() {
                crate::home_page::play(&ui, &shared, place.as_str());
            }
        }
    });
    ui.on_toggle_favourite({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |place| {
            if let Some(ui) = weak.upgrade() {
                crate::home_page::toggle_favourite(&ui, &shared, place.as_str());
            }
        }
    });
    ui.on_toggle_discord_hidden({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |place| {
            if let Some(ui) = weak.upgrade() {
                crate::home_page::toggle_hidden(&ui, &shared, place.as_str());
            }
        }
    });
    ui.on_set_discord_text({
        let shared = shared.clone();
        move |text| crate::home_page::set_discord_text(&shared, text.as_str())
    });
    crate::home_page::refresh(&ui, &shared);
    crate::mods_page::refresh(&ui, &shared);

    ui.on_mods_opened({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                crate::mods_page::refresh(&ui, &shared);
            }
        }
    });
    ui.on_toggle_mod({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |name, on| {
            if let Some(ui) = weak.upgrade() {
                crate::mods_page::toggle(&ui, &shared, name.as_str(), on);
            }
        }
    });
    ui.on_import_mods({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |name| {
            if let Some(ui) = weak.upgrade() {
                crate::mods_page::import(&ui, &shared, name.as_str());
            }
        }
    });
    ui.on_get_theme({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                crate::mods_page::get_theme(&ui, &shared);
            }
        }
    });
    // The mod generator and sharing mods (ModTools in mod_tools.slint).
    {
        let tools = ui.global::<ModTools>();
        let handler = |f: fn(&AppWindow, &Shared)| {
            let weak = ui.as_weak();
            let shared = shared.clone();
            move || {
                if let Some(ui) = weak.upgrade() {
                    f(&ui, &shared);
                }
            }
        };
        tools.on_changed(handler(crate::mods_page::update_preview));
        tools.on_make(handler(crate::mods_page::make_tint));
        tools.on_import_zip(handler(crate::mods_page::import_zip));
        tools.set_windows(cfg!(windows));
        tools.on_emoji({
            let weak = ui.as_weak();
            let shared = shared.clone();
            move |which| {
                if let Some(ui) = weak.upgrade() {
                    crate::mods_page::emoji(&ui, &shared, which.as_str());
                }
            }
        });
        tools.on_choose_preset({
            let weak = ui.as_weak();
            let shared = shared.clone();
            move |which, i| {
                if let Some(ui) = weak.upgrade() {
                    crate::mods_page::choose_preset(&ui, &shared, which, i);
                }
            }
        });
        tools.on_choose_hex({
            let weak = ui.as_weak();
            let shared = shared.clone();
            move |which, text| {
                if let Some(ui) = weak.upgrade() {
                    crate::mods_page::choose_hex(&ui, &shared, which, text.as_str());
                }
            }
        });
        tools.on_share({
            let weak = ui.as_weak();
            move |name| {
                if let Some(ui) = weak.upgrade() {
                    crate::mods_page::share(&ui, name.as_str());
                }
            }
        });
        tools.on_set_target({
            let weak = ui.as_weak();
            let shared = shared.clone();
            move |name, target| {
                if let Some(ui) = weak.upgrade() {
                    crate::mods_page::set_target(&ui, &shared, name.as_str(), target);
                }
            }
        });
        crate::mods_page::load_generator(&ui, &shared);
    }
    ui.on_pick_file({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |k| {
            if let Some(ui) = weak.upgrade() {
                let _ = &shared;
                crate::mods_page::pick(&ui, &shared, k);
            }
        }
    });
    ui.on_clear_pick({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |k| {
            if let Some(ui) = weak.upgrade() {
                let _ = &shared;
                crate::mods_page::clear_pick(&ui, &shared, k);
            }
        }
    });
    ui.on_save_mod_profile({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |n| {
            if let Some(ui) = weak.upgrade() {
                let _ = &shared;
                crate::mods_page::save_profile(&ui, &shared, n.as_str());
            }
        }
    });
    ui.on_apply_mod_profile({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |n| {
            if let Some(ui) = weak.upgrade() {
                let _ = &shared;
                crate::mods_page::apply_profile(&ui, &shared, n.as_str());
            }
        }
    });
    ui.on_delete_mod_profile({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |n| {
            if let Some(ui) = weak.upgrade() {
                let _ = &shared;
                crate::mods_page::delete_profile(&ui, &shared, n.as_str());
            }
        }
    });
    ui.on_listen_mod({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |n| {
            if let Some(ui) = weak.upgrade() {
                let _ = &shared;
                crate::mods_page::listen(&ui, n.as_str());
            }
        }
    });
    ui.on_open_mods_folder({
        let weak = ui.as_weak();
        move || {
            if let Some(ui) = weak.upgrade() {
                crate::mods_page::open_folder(&ui);
            }
        }
    });
    ui.on_restore_mods({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                crate::mods_page::restore(&ui, &shared);
            }
        }
    });
    ui.on_open_logs({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move || {
            let Some(ui) = weak.upgrade() else { return };
            let folder = lock(&shared)
                .last_log
                .as_ref()
                .and_then(|p| p.parent().map(PathBuf::from))
                .or_else(paths::log_dir);
            if let Some(folder) = folder {
                open_folder(&ui, folder);
            }
        }
    });

    start_detection(&ui, &shared);
    start_roblox_update(&ui, &shared);
    // "Delete logs automatically", and the sizes on the Settings page.
    let log_days = lock(&shared).settings.log_days;
    if log_days > 0 {
        let _ = thread::Builder::new()
            .name("feline-old-logs".into())
            .spawn(move || feline_core::housekeeping::delete_old_logs(log_days));
    }
    // "Tidy up automatically", then the sizes on the Settings page.
    let tidy_days = lock(&shared).settings.tidy_days;
    if feline_core::housekeeping::tidy_due(tidy_days) {
        let weak = ui.as_weak();
        let _ = thread::Builder::new().name("feline-tidy".into()).spawn(move || {
            let _ = feline_core::housekeeping::tidy();
            let _ = weak.upgrade_in_event_loop(|ui| settings_page::refresh_sizes(&ui));
        });
    } else {
        settings_page::refresh_sizes(&ui);
    }
    start_system_scan(&ui, &shared);
    crate::checkup::investigate(&ui, &shared);
    if START_MINIMIZED.load(Ordering::Relaxed) {
        let weak = ui.as_weak();
        slint::Timer::single_shot(Duration::from_millis(10), move || {
            if let Some(ui) = weak.upgrade() {
                ui.window().set_minimized(true);
            }
        });
    }
    ui.on_quit_feline(|| {
        let _ = slint::quit_event_loop();
    });
    // Closing the window hides Feline to the tray (keep running), or quits.
    ui.window().on_close_requested({
        let shared = shared.clone();
        move || crate::background::on_close(&shared)
    });
    if !screenshot_run {
        crate::background::start(&ui);
    }
    ui.show()?;
    let result = slint::run_event_loop_until_quit();
    let _ = ui.hide();
    crate::background::stop();
    // Quitting: the desk cat goes home, then "clear when Feline closes"
    // (not for a developer's screenshot run, which isn't the user closing
    // Feline and mustn't wipe their logs and history).
    crate::extras::on_exit(&shared, !screenshot_run);
    result
}

// ---------------------------------------------------------------------------
// System page
// ---------------------------------------------------------------------------

/// Reads the system information on a background thread (it reads many small
/// files, plus the PCI name database, which can take a moment).
fn start_system_scan(ui: &AppWindow, shared: &Shared) {
    {
        let mut state = lock(shared);
        if state.scanning_system {
            return;
        }
        state.scanning_system = true;
    }
    ui.set_system_loading(true);

    let weak = ui.as_weak();
    let shared_for_thread = shared.clone();
    let spawned = thread::Builder::new()
        .name("feline-system".into())
        .spawn(move || {
            let info = SystemInfo::collect();
            let _ = weak.upgrade_in_event_loop(move |ui| {
                show_system(&ui, &info);
                let mut state = lock(&shared_for_thread);
                state.system = Some(info);
                state.scanning_system = false;
                drop(state);
                ui.set_system_loading(false);
            });
        });

    if let Err(e) = spawned {
        lock(shared).scanning_system = false;
        ui.set_system_loading(false);
        ui.set_system_report(format!("Couldn't read the system information: {e}").into());
    }
}

fn show_system(ui: &AppWindow, info: &SystemInfo) {
    let unknown = || "Unknown".to_owned();
    let row = |label: &str, value: String, level: Level| InfoRow {
        label: label.into(),
        value: value.into(),
        level,
    };

    let mut rows = vec![
        row(
            if cfg!(windows) { "System" } else { "Distribution" },
            info.os.clone().unwrap_or_else(unknown),
            Level::Neutral,
        ),
        row(
            // On Windows, `kernel` holds the Windows build number.
            if cfg!(windows) { "Build" } else { "Kernel" },
            info.kernel.clone().unwrap_or_else(unknown),
            Level::Neutral,
        ),
        row(
            "Processor",
            match &info.cpu {
                Some(cpu) => format!("{cpu} · {} threads", info.cpu_threads),
                None => unknown(),
            },
            Level::Neutral,
        ),
        row(
            "Memory",
            info.memory_total_kib
                .map(|kib| format!("{:.1} GiB", kib as f64 / (1024.0 * 1024.0)))
                .unwrap_or_else(unknown),
            Level::Neutral,
        ),
    ];
    if info.gpus.is_empty() {
        rows.push(row("Graphics", "None found".into(), Level::Warning));
    }
    for gpu in &info.gpus {
        let primary = if info.gpus.len() > 1 && gpu.boot_vga {
            " · primary"
        } else {
            ""
        };
        let level = if gpu.driver.is_some() {
            Level::Neutral
        } else {
            Level::Warning
        };
        rows.push(row("Graphics", format!("{gpu}{primary}"), level));
    }
    if cfg!(windows) {
        // No Flatpak, display server or init system to show.
        finish_system(ui, info, rows);
        return;
    }
    if let Some(version) = &info.nvidia_driver {
        rows.push(row("NVIDIA driver", version.clone(), Level::Neutral));
    }
    let (flatpak, flatpak_level) = match &info.flatpak_nvidia {
        FlatpakNvidia::NotApplicable => (None, Level::Neutral),
        FlatpakNvidia::Match { version } => (
            Some(format!("Matches the host driver (GL.nvidia-{version})")),
            Level::Ok,
        ),
        FlatpakNvidia::Missing { .. } => (
            Some("Not installed: Flatpak apps can't use the NVIDIA GPU".to_owned()),
            Level::Warning,
        ),
        FlatpakNvidia::Mismatch { installed, .. } => (
            Some(format!(
                "Doesn't match the host driver (installed: {})",
                installed.join(", ")
            )),
            Level::Warning,
        ),
    };
    if let Some(value) = flatpak {
        rows.push(row("Flatpak NVIDIA", value, flatpak_level));
    }
    let session_level = match info.session {
        SessionType::Wayland | SessionType::X11 => Level::Neutral,
        SessionType::Tty => Level::Problem,
        SessionType::Unknown => Level::Warning,
    };
    rows.push(row("Session", info.session.to_string(), session_level));
    rows.push(row(
        "Desktop",
        info.desktop.clone().unwrap_or_else(unknown),
        Level::Neutral,
    ));
    rows.push(row("Init system", info.init.to_string(), Level::Neutral));
    finish_system(ui, info, rows);
}

/// Shows the rows, the plain-text report and the quick checks.
fn finish_system(ui: &AppWindow, info: &SystemInfo, rows: Vec<InfoRow>) {
    ui.set_system_rows(ModelRc::new(VecModel::from(rows)));
    ui.set_system_report(info.report().into());
    crate::checkup::show_pc(ui, info);

    let checks: Vec<CheckRow> = info
        .checks()
        .into_iter()
        .map(|c| CheckRow {
            level: level_of(c.level),
            title: c.title.into(),
            detail: c.detail.into(),
        })
        .collect();
    ui.set_checks(ModelRc::new(VecModel::from(checks)));
}

fn level_of(level: CheckLevel) -> Level {
    match level {
        CheckLevel::Ok => Level::Ok,
        CheckLevel::Warning => Level::Warning,
        CheckLevel::Problem => Level::Problem,
    }
}

// ---------------------------------------------------------------------------
// Diagnostics page
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum DiagAction {
    /// The runtime's own self-check (`--doctor`).
    Doctor,
    /// The runtime's bug-report block (`--diagnostics`).
    RuntimeInfo,
    /// Feline version + system report + runtime info, in one copyable text.
    BugReport,
    /// The same, plus the crash detective and logs, saved as a zip.
    SaveZip,
}

/// What a Diagnostics action produced.
struct DiagResult {
    level: Level,
    title: String,
    output: String,
}

fn start_diagnostic(ui: &AppWindow, shared: &Shared, action: DiagAction) {
    let (installation, system) = {
        let mut state = lock(shared);
        if state.diag_busy {
            return;
        }
        let installation = selected(&state).map(|f| f.installation.clone());
        if installation.is_none() && !matches!(action, DiagAction::BugReport | DiagAction::SaveZip) {
            return; // the buttons are disabled in this case anyway
        }
        state.diag_busy = true;
        (installation, state.system.clone())
    };
    ui.set_diag_running(true);
    ui.set_diag_title(SharedString::new());
    ui.set_diag_output(SharedString::new());
    let log_dirs = if matches!(action, DiagAction::SaveZip) {
        crate::home_page::log_roots(shared)
    } else {
        Vec::new()
    };

    let weak = ui.as_weak();
    let shared_for_thread = shared.clone();
    let spawned = thread::Builder::new()
        .name("feline-diagnostics".into())
        .spawn(move || {
            let result = run_diagnostic(action, installation.as_ref(), system, &log_dirs);
            let _ = weak.upgrade_in_event_loop(move |ui| {
                lock(&shared_for_thread).diag_busy = false;
                ui.set_diag_running(false);
                ui.set_diag_level(result.level);
                ui.set_diag_title(result.title.into());
                ui.set_diag_output(result.output.into());
            });
        });

    if let Err(e) = spawned {
        lock(shared).diag_busy = false;
        ui.set_diag_running(false);
        ui.set_diag_level(Level::Problem);
        ui.set_diag_title(format!("Feline couldn't start a background task: {e}").into());
    }
}

/// Runs one Diagnostics action (background thread: these start programs).
fn run_diagnostic(
    action: DiagAction,
    installation: Option<&Installation>,
    system: Option<SystemInfo>,
    log_dirs: &[PathBuf],
) -> DiagResult {
    if let DiagAction::SaveZip = action {
        let report = run_diagnostic(DiagAction::BugReport, installation, system, log_dirs);
        let mut text = report.output;
        text.push('\n');
        text.push_str(&crate::checkup::detective_text(log_dirs));
        return match feline_core::bugreport::write(&text, log_dirs) {
            Ok(path) => {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                // Show the zip in the file manager, selected where possible.
                let mut command = std::process::Command::new(if cfg!(windows) { "explorer" } else { "xdg-open" });
                if cfg!(windows) {
                    command.arg(format!("/select,{}", path.display()));
                } else if let Some(dir) = path.parent() {
                    command.arg(dir);
                }
                let _ = command
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
                DiagResult {
                    level: Level::Ok,
                    title: format!("Saved {name} in your Downloads. Attach it to your bug report."),
                    output: format!(
                        "{}\n\nThe zip also has the newest Roblox and Feline logs. Your user name in folder paths and every line that might hold a login secret were hidden first.\n\n{text}",
                        path.display()
                    ),
                }
            }
            Err(e) => DiagResult {
                level: Level::Problem,
                title: e,
                output: text,
            },
        };
    }
    match (action, installation) {
        (DiagAction::Doctor, Some(installation)) => {
            let name = installation.backend.name();
            match installation.doctor() {
                Ok(report) if report.ok => DiagResult {
                    level: Level::Ok,
                    title: format!("{name} found no blocking problems"),
                    output: report.output,
                },
                Ok(report) => DiagResult {
                    level: Level::Problem,
                    title: format!("{name} found something that will stop Roblox starting"),
                    output: report.output,
                },
                Err(e) => DiagResult {
                    level: Level::Problem,
                    title: format!("{name}'s self-check couldn't run"),
                    output: e.to_string(),
                },
            }
        }
        (DiagAction::RuntimeInfo, Some(installation)) => {
            let name = installation.backend.name();
            match installation.diagnostics() {
                Ok(text) => DiagResult {
                    level: Level::Ok,
                    title: format!("{name} runtime info"),
                    output: text,
                },
                Err(e) => DiagResult {
                    level: Level::Problem,
                    title: format!("Couldn't get {name}'s runtime info"),
                    output: e.to_string(),
                },
            }
        }
        (DiagAction::BugReport, installation) => {
            // Reuse the last scan; only read the system again if there's none yet.
            let system = system.unwrap_or_else(SystemInfo::collect);
            let mut output = format!("Feline {}\n\n{}", feline_core::VERSION, system.report());
            match installation {
                None => output.push_str("\nRuntime         none selected\n"),
                Some(installation) => {
                    output.push_str(&format!("\nRuntime         {installation}\n"));
                    if installation.backend.has_diagnostics() {
                        output.push('\n');
                        match installation.diagnostics() {
                            Ok(text) => output.push_str(&text),
                            Err(e) => output.push_str(&format!("(runtime info unavailable: {e})")),
                        }
                        output.push('\n');
                    }
                }
            }
            DiagResult {
                level: Level::Ok,
                title: "Bug report ready. Hit Copy, then paste it wherever you're reporting the bug.".to_owned(),
                output,
            }
        }
        (DiagAction::SaveZip, _) => unreachable!("handled above"),
        // Doctor or RuntimeInfo without a runtime: start_diagnostic prevents this.
        (_, None) => DiagResult {
            level: Level::Warning,
            title: "Choose a runtime first".to_owned(),
            output: String::new(),
        },
    }
}

// ---------------------------------------------------------------------------
// Links
// ---------------------------------------------------------------------------


// ---------------------------------------------------------------------------
// Runtimes
// ---------------------------------------------------------------------------

/// Keeps Roblox's app up to date for Feline's runtime, in the background,
/// when that's the chosen runtime (others bring their own Roblox; no need
/// to download 240 MB for them). The status shows on the Home page.
fn start_roblox_update(ui: &AppWindow, shared: &Shared) {
    let (feline_chosen, wanted) = {
        let state = lock(shared);
        (
            state.settings.runtime.as_deref().is_some_and(|k| k.starts_with("feline:")),
            state.settings.roblox_updates,
        )
    };
    if !feline_chosen || !wanted {
        return;
    }
    run_roblox_update(ui);
}

/// Settings → Updates → "Check for updates": checks now, whatever runtime
/// is chosen.
pub fn check_roblox_update(ui: &AppWindow, _shared: &Shared) {
    ui.set_roblox_status("Checking for a Roblox update…".into());
    run_roblox_update(ui);
}

/// Installs the newest Roblox version in the background, with the status on
/// the Home page and Settings → Updates.
fn run_roblox_update(ui: &AppWindow) {
    let weak = ui.as_weak();
    let show = move |weak: &slint::Weak<AppWindow>, text: String| {
        let _ = weak.upgrade_in_event_loop(move |ui| ui.set_roblox_status(text.into()));
    };
    let _ = thread::Builder::new()
        .name("feline-roblox-update".into())
        .spawn(move || {
            use feline_core::engine::{Outcome, Progress};
            let mut last = None;
            let result = feline_core::engine::update(&mut |p| {
                let text = match p {
                    Progress::Checking => return,
                    Progress::Downloading(done, Some(total)) => {
                        let percent = done * 100 / total.max(1);
                        if last == Some(percent) {
                            return;
                        }
                        last = Some(percent);
                        format!("Downloading a Roblox update… {percent}%")
                    }
                    Progress::Downloading(done, None) => format!("Downloading a Roblox update… {} MB", done >> 20),
                    Progress::Verifying => "Checking the update is signed by Roblox…".to_owned(),
                    Progress::Installing => "Installing the Roblox update…".to_owned(),
                };
                show(&weak, text);
            });
            let text = match result {
                Ok(Outcome::UpToDate(v)) => format!("Roblox {v} (up to date)"),
                Ok(Outcome::Installed { version, .. }) => {
                    format!("Roblox updated to {version}. It's used from the next start.")
                }
                Err(e) => format!("Couldn't update Roblox: {e}"),
            };
            show(&weak, text);
        });
}

fn start_detection(ui: &AppWindow, shared: &Shared) {
    {
        let mut state = lock(shared);
        if state.detecting {
            return;
        }
        state.detecting = true;
    }
    ui.set_detecting(true);

    let weak = ui.as_weak();
    let shared_for_thread = shared.clone();
    let spawned = thread::Builder::new()
        .name("feline-detect".into())
        .spawn(move || {
            // Versions that aren't in a file are asked from the program
            // (`--help`), which takes a moment: one more reason to be here.
            let found: Vec<Found> = detect_all(&DetectContext::host())
                .into_iter()
                .map(|installation| Found {
                    version: installation.version().ok(),
                    installation,
                })
                .collect();
            // Runs on the UI thread. Fails only if the window already closed.
            let _ = weak.upgrade_in_event_loop(move |ui| {
                let mut state = lock(&shared_for_thread);
                state.found = found;
                state.detecting = false;
                drop(state);
                ui.set_detecting(false);
                refresh_runtimes(&ui, &shared_for_thread);
                settings_page::refresh(&ui, &shared_for_thread);
                crate::roblox_page::record_versions(&shared_for_thread);
                crate::roblox_page::refresh(&ui, &shared_for_thread);
                crate::mods_page::refresh(&ui, &shared_for_thread);
            });
        });

    if let Err(e) = spawned {
        lock(shared).detecting = false;
        ui.set_detecting(false);
        ui.set_runtimes_error(format!("Couldn't scan for runtimes: {e}").into());
    }
}

/// Pushes the runtime list and the current selection to the UI.
fn refresh_runtimes(ui: &AppWindow, shared: &Shared) {
    let state = lock(shared);
    let saved = state.settings.runtime.as_deref();

    let (feline, external): (Vec<_>, Vec<_>) = state
        .found
        .iter()
        .partition(|f| f.installation.backend.origin() == Origin::Feline);
    let row = |f: &&Found| RuntimeRow {
            key: f.installation.key().into(),
            id: f.installation.backend.id().into(),
            name: f.installation.backend.name().into(),
            version: f.version.clone().unwrap_or_default().into(),
            kind: format!(
                "{} · {}",
                origin_label(f.installation.backend.origin()),
                kind_label(&f.installation.kind)
            )
            .into(),
            selected: saved == Some(f.installation.key().as_str()),
    };
    let feline: Vec<RuntimeRow> = feline.iter().map(row).collect();
    let external: Vec<RuntimeRow> = external.iter().map(row).collect();
    ui.set_feline_runtimes(ModelRc::new(VecModel::from(feline)));
    ui.set_runtimes(ModelRc::new(VecModel::from(external)));

    match selected(&state) {
        Some(f) => {
            let name = match &f.version {
                Some(v) => format!("{} {v}", f.installation.backend.name()),
                None => f.installation.backend.name().to_owned(),
            };
            ui.set_selected_name(name.into());
            ui.set_selected_has_doctor(f.installation.backend.has_doctor());
            ui.set_selected_has_info(f.installation.backend.has_diagnostics());
            ui.set_selected_detail(
                match f.installation.backend.origin() {
                    Origin::Feline => "Feline's own runtime".to_owned(),
                    Origin::External { .. } => {
                        format!("External runtime · {}", short_kind(&f.installation.kind))
                    }
                    Origin::Official => "Roblox's official client".to_owned(),
                }
                .into(),
            );
        }
        None => {
            ui.set_selected_name(SharedString::new());
            ui.set_selected_detail(SharedString::new());
            ui.set_selected_has_doctor(false);
            ui.set_selected_has_info(false);
        }
    }
}

/// The installation the user chose, if it's still installed.
pub(crate) fn selected(state: &State) -> Option<&Found> {
    let key = state.settings.runtime.as_deref()?;
    state.found.iter().find(|f| f.installation.key() == key)
}

fn select_runtime(ui: &AppWindow, shared: &Shared, key: &str) {
    let result = {
        let mut state = lock(shared);
        state.settings.runtime = Some(key.to_owned());
        match Settings::path() {
            Some(path) => state.settings.save_to(&path).map_err(|e| e.to_string()),
            None => Err("your home folder couldn't be found".to_owned()),
        }
    };
    match result {
        Ok(()) => ui.set_runtimes_error(SharedString::new()),
        Err(e) => ui.set_runtimes_error(
            format!("Selected, but couldn't save the choice for next time: {e}").into(),
        ),
    }
    refresh_runtimes(ui, shared);
    settings_page::refresh(ui, shared);
}

fn origin_label(origin: Origin) -> &'static str {
    match origin {
        Origin::Feline => "Feline",
        Origin::External { .. } => "External",
        Origin::Official => "Official",
    }
}

fn kind_label(kind: &InstallKind) -> String {
    match kind {
        InstallKind::Flatpak { .. } => kind.to_string(),
        InstallKind::AppImage(path) => format!("AppImage · {}", tilde(path)),
        InstallKind::Native(path) => format!("Installed program · {}", tilde(path)),
        InstallKind::Official(path) => format!("Roblox's installer · {}", tilde(path)),
    }
}

fn short_kind(kind: &InstallKind) -> &'static str {
    match kind {
        InstallKind::Flatpak { .. } => "Flatpak",
        InstallKind::AppImage(_) => "AppImage",
        InstallKind::Native(_) => "installed program",
        InstallKind::Official(_) => "Roblox's installer",
    }
}

/// Shows `/home/me/x` as `~/x`.
pub(crate) fn tilde(path: &std::path::Path) -> String {
    match std::env::var_os("HOME").map(PathBuf::from) {
        Some(home) => match path.strip_prefix(&home) {
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => path.display().to_string(),
        },
        None => path.display().to_string(),
    }
}

// ---------------------------------------------------------------------------
// Launching
// ---------------------------------------------------------------------------

fn set_launch(ui: &AppWindow, state: LaunchState, title: &str, detail: &str) {
    ui.set_launch_state(state);
    ui.set_launch_title(title.into());
    ui.set_launch_detail(detail.into());
}

pub(crate) fn start_launch(ui: &AppWindow, shared: &Shared, target: LaunchTarget) {
    let (installation, settings) = {
        let mut state = lock(shared);
        if state.busy {
            // Already starting or running. With "more than one Roblox",
            // Play opens another Roblox window next to it.
            let extra = state.settings.multi_roblox
                && selected(&state).is_some_and(|f| matches!(f.installation.kind, InstallKind::Official(_)));
            if extra && let Some(found) = selected(&state) {
                let (installation, settings) = (found.installation.clone(), state.settings.clone());
                drop(state);
                start_extra(ui, installation, settings, target);
            }
            return;
        }
        let Some(found) = selected(&state) else {
            drop(state);
            set_launch(
                ui,
                LaunchState::Failed,
                "Choose a runtime first",
                "Open the Runtimes page and choose the one Feline should use.",
            );
            return;
        };
        let installation = found.installation.clone();
        // Roblox is already open (started outside Feline): a new one would
        // close it. Ask first; Play again within a minute goes ahead.
        let asked_lately = state.asked_to_confirm.is_some_and(|t| t.elapsed() < Duration::from_secs(60));
        if state.settings.confirm_launch
            && !state.settings.multi_roblox
            && !asked_lately
            && installation.is_running()
        {
            state.asked_to_confirm = Some(std::time::Instant::now());
            drop(state);
            set_launch(
                ui,
                LaunchState::Failed,
                "Roblox is already open",
                "Starting it again closes the one that's open. Press Play again to go ahead, or switch on \"More than one Roblox\" in Settings.",
            );
            return;
        }
        state.asked_to_confirm = None;
        state.busy = true;
        crate::desk_cat::set_playing(true);
        (installation, state.settings.clone())
    };

    let name = installation.backend.name();
    set_launch(ui, LaunchState::Starting, &format!("Waking up {name}…"), "");
    ui.set_stat_time(SharedString::new());
    ui.set_stat_cpu(SharedString::new());
    ui.set_stat_memory(SharedString::new());
    ui.set_stat_game(SharedString::new());
    ui.set_stat_region(SharedString::new());
    let log_roots = crate::home_page::log_roots(shared);

    let weak = ui.as_weak();
    let shared_for_thread = shared.clone();
    let spawned = thread::Builder::new()
        .name("feline-game".into())
        .spawn(move || {
            let presence_ui = weak.clone();
            let on_presence = move |status: String| {
                let _ = presence_ui.upgrade_in_event_loop(move |ui| {
                    ui.set_presence_status(status.into());
                });
            };
            let stats_ui = weak.clone();
            let on_stats = move |stats: Stats| {
                let _ = stats_ui.upgrade_in_event_loop(move |ui| {
                    ui.set_stat_time(stats.time.into());
                    ui.set_stat_cpu(stats.cpu.into());
                    ui.set_stat_memory(stats.memory.into());
                    ui.set_stat_game(stats.game.into());
                    ui.set_stat_region(stats.region.into());
                });
            };
            let outcome = run_game(
                &installation,
                &target,
                &settings,
                log_roots,
                on_presence,
                on_stats,
                |title, detail| {
                    let (title, detail) = (title.to_owned(), detail.to_owned());
                    let after = settings.after_launch;
                    let keep = cfg!(windows) && settings.keep_running;
                    let _ = weak.upgrade_in_event_loop(move |ui| {
                        set_launch(&ui, LaunchState::Running, &title, &detail);
                        match after {
                            AfterLaunch::Stay => {}
                            AfterLaunch::Minimize => ui.window().set_minimized(true),
                            // The game and the Discord status helper are
                            // separate processes; they keep running.
                            // With "keep running", to the icon by the clock.
                            AfterLaunch::Close if keep => {
                                let _ = ui.hide();
                            }
                            AfterLaunch::Close => {
                                let _ = slint::quit_event_loop();
                            }
                        }
                    });
                },
            );
            let _ = weak.upgrade_in_event_loop(move |ui| {
                let mut state = lock(&shared_for_thread);
                state.busy = false;
                if let Some(log) = &outcome.log {
                    state.last_log = Some(log.clone());
                }
                drop(state);
                set_launch(&ui, outcome.state, &outcome.title, &outcome.detail);
                // The game is in the history now.
                crate::home_page::refresh(&ui, &shared_for_thread);
                crate::extras::game_ended(&ui, &shared_for_thread);
            });
        });

    if let Err(e) = spawned {
        lock(shared).busy = false;
        crate::desk_cat::set_playing(false);
        set_launch(
            ui,
            LaunchState::Failed,
            "Couldn't start",
            &format!("Feline couldn't start a background task: {e}"),
        );
    }
}

/// Another Roblox next to the running one (more than one Roblox): started
/// and left to itself (the first one's session has the stats and history).
fn start_extra(ui: &AppWindow, installation: Installation, settings: Settings, target: LaunchTarget) {
    ui.set_launch_detail("Opening another Roblox…".into());
    let weak = ui.as_weak();
    let _ = thread::Builder::new().name("feline-extra-game".into()).spawn(move || {
        let (installation, mut lines) = crate::companion::before(&installation, &settings);
        let started = installation
            .plan_launch(&target)
            .map_err(|e| e.to_string())
            .and_then(|plan| {
                let log_dir = paths::log_dir().ok_or("your home folder couldn't be found")?;
                launch::spawn(plan.command, &log_dir, installation.backend.id()).map_err(|e| e.to_string())
            });
        let text = match started {
            Ok(game) => {
                lines.extend(feline_core::runtime::roblox::after_start(game.pid(), &settings));
                // Nobody waits for it here; a thread collects it when it ends.
                thread::spawn(move || game.wait());
                lines.insert(0, "Another Roblox is on its way.".to_owned());
                lines.join("\n")
            }
            Err(e) => format!("Another Roblox couldn't start: {e}"),
        };
        let _ = weak.upgrade_in_event_loop(move |ui| ui.set_launch_detail(text.into()));
    });
}

struct Outcome {
    state: LaunchState,
    title: String,
    detail: String,
    log: Option<PathBuf>,
}

impl Outcome {
    fn failed(title: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            state: LaunchState::Failed,
            title: title.into(),
            detail: detail.into(),
            log: None,
        }
    }
}

/// Plans, starts and waits for one game session (background thread).
/// `on_running` is called once the runtime process has started.
fn run_game(
    installation: &Installation,
    target: &LaunchTarget,
    settings: &Settings,
    log_roots: Vec<PathBuf>,
    on_presence: impl Fn(String) + Send + 'static,
    on_stats: impl Fn(Stats) + Send + 'static,
    on_running: impl FnOnce(&str, &str),
) -> Outcome {
    let overrides = &settings.overrides;
    let name = installation.backend.name();
    // The kept Roblox version (if you stay on one); room for more Robloxes.
    let (installation, before_lines) = crate::companion::before(installation, settings);
    let installation = &installation;
    let hide_cursor = crate::companion::hide_cursor(installation, settings);

    // Write Feline's overrides into the runtime's settings first. A failure
    // isn't fatal: the game still starts, with the runtime's own settings.
    let settings_warning = installation
        .apply_overrides(&DetectContext::host(), overrides)
        .err()
        .map(|e| format!("Feline's settings weren't applied: {e}"));

    // Roblox's own client (Windows): is it really Roblox's? Which GPU?
    let mut warnings = match feline_core::runtime::roblox::prepare(installation, settings) {
        Ok(warnings) => warnings,
        Err(why) => return Outcome::failed("Feline didn't start Roblox", why),
    };
    // Mods: put the enabled ones in (or take switched-off ones out).
    warnings.extend(before_lines);
    warnings.extend(feline_core::mods::apply_for_launch(
        installation,
        &DetectContext::host(),
        &settings.player_mods(),
        hide_cursor,
    ));

    // The setting profile chosen for this game (or for every other game).
    let place = match target {
        LaunchTarget::Game(link) => link.place_id(),
        LaunchTarget::Home => None,
    };
    // Tweaks (FastFlags): this game's profile, or the everyday ones.
    warnings.extend(feline_core::fflags::apply_for_launch(
        installation,
        &DetectContext::host(),
        settings,
        place,
    ));
    warnings.extend(feline_core::roblox_tools::apply_game_profile(
        installation,
        &DetectContext::host(),
        settings,
        place,
    ));

    let plan = match installation.plan_launch(target) {
        Ok(plan) => plan,
        Err(e) => return Outcome::failed(format!("{name} couldn't be prepared"), e.to_string()),
    };
    let Some(log_dir) = paths::log_dir() else {
        return Outcome::failed("Couldn't start", "Your home folder couldn't be found.");
    };
    // Linux: started through gamescope, distrobox… if asked.
    let command = if cfg!(windows) { plan.command } else { launch::wrap(plan.command, &settings.runtime_wrapper) };
    let game = match launch::spawn(command, &log_dir, installation.backend.id()) {
        Ok(game) => game,
        Err(e) => return Outcome::failed(format!("{name} couldn't be started"), e.to_string()),
    };
    warnings.extend(feline_core::runtime::roblox::after_start(game.pid(), settings));
    // Programs that go with Roblox, and the animated cursor.
    warnings.extend(crate::companion::after(game.pid(), hide_cursor));

    // "in Roblox" would read oddly for Roblox's own client.
    let in_runtime = if installation.backend.origin() == Origin::Official {
        String::new()
    } else {
        format!(" in {name}")
    };
    let title = match (&plan.note, target) {
        (Some(_), _) => format!("{name} is open"),
        (None, LaunchTarget::Home) => format!("Roblox is up and purring{in_runtime}"),
        (None, LaunchTarget::Game(_)) => format!("Pouncing into the game{in_runtime}"),
    };
    // The Discord status runs in its own small process (`feline --presence`),
    // so it keeps going after this window closes and ends with the game.
    let presence = overrides
        .discord_presence
        .then(crate::presence_helper::start)
        .transpose();
    let default_detail = match presence {
        Ok(Some(())) => "Off you pounce. Your Discord status keeps going even if you close Feline.",
        _ => "Off you pounce. You can close Feline; the game keeps running.",
    };
    let mut detail = plan
        .note
        .clone()
        .unwrap_or_else(|| default_detail.to_owned());
    warnings.extend(settings_warning);
    for warning in warnings {
        detail = format!("{warning}\n{detail}");
    }
    on_running(&title, &detail);

    if let Err(e) = presence {
        on_presence(e);
    }
    let monitor = Monitor::start(
        game.pid(),
        settings.clone(),
        log_roots,
        on_stats,
        on_presence,
    );
    let waited = game.wait();
    if let Some(monitor) = monitor {
        monitor.stop();
    }
    match waited {
        Ok(exit) => describe_exit(name, exit),
        Err(e) => Outcome::failed(
            "Lost track of the game",
            format!("Feline couldn't wait for {name} to finish: {e}. It may still be running."),
        ),
    }
}

/// What the Home page shows while a game runs, already formatted.
struct Stats {
    time: String,
    cpu: String,
    memory: String,
    /// The game's name and the server's region, once the log tells.
    game: String,
    region: String,
}

/// Samples the game's CPU and memory every two seconds (background thread).
struct Monitor {
    stop: Arc<AtomicBool>,
    thread: thread::JoinHandle<()>,
}

impl Monitor {
    const INTERVAL: Duration = Duration::from_secs(2);

    /// `None` if the thread couldn't start; the game runs anyway, just
    /// without numbers.
    fn start(
        pid: u32,
        settings: Settings,
        log_roots: Vec<PathBuf>,
        on_stats: impl Fn(Stats) + Send + 'static,
        on_presence: impl Fn(String) + Send + 'static,
    ) -> Option<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = stop.clone();
        let started = std::time::Instant::now();
        let thread = thread::Builder::new()
            .name("feline-monitor".into())
            .spawn(move || {
                let mut sampler = feline_core::monitor::Sampler::new(pid);
                // Which game and server, from Roblox's log (like the Discord
                // status, but for the Home page).
                let mut follower = feline_core::presence::log::LogFollower::new(
                    log_roots,
                    std::time::SystemTime::now() - Duration::from_secs(60),
                );
                let api = feline_core::presence::roblox::Api::new();
                let mut activity = feline_core::presence::Activity::Starting;
                let mut names: std::collections::HashMap<u64, String> = Default::default();
                let mut places: std::collections::HashMap<String, String> = Default::default();
                // The next break reminder, in minutes of play (0 = off).
                let every = u64::from(settings.break_minutes);
                let regions = settings.discord.region || settings.server_popup;
                // Roblox's windows already given Feline's icon/colours.
                let mut dressed: Vec<isize> = Vec::new();
                let mut next_break = every;
                let mut leave = feline_core::runtime::roblox::LeaveWatch::default();
                let mut last_job = String::new();
                // The first sample is only the starting point for CPU.
                sampler.sample();
                while !stop_flag.load(Ordering::Relaxed) {
                    // Sleep in small steps so the game ending stops us quickly.
                    let wake = std::time::Instant::now() + Self::INTERVAL;
                    while std::time::Instant::now() < wake && !stop_flag.load(Ordering::Relaxed) {
                        thread::sleep(Duration::from_millis(100));
                    }
                    feline_core::runtime::roblox::dress_windows(pid, &settings, &mut dressed);
                    let played = started.elapsed().as_secs() / 60;
                    if every > 0 && played >= next_break {
                        let (title, text) = feline_core::launch::break_reminder(next_break);
                        feline_core::launch::notify(&title, &text);
                        next_break += every;
                    }
                    // The Discord helper's status, shown on the Settings page.
                    if let Some(status) = crate::presence_helper::status() {
                        on_presence(status);
                    }
                    let Some(sample) = sampler.sample() else {
                        continue;
                    };
                    for event in follower.poll() {
                        activity = activity.apply(event);
                    }
                    // Remember the server for "Rejoin" and "Server hop".
                    if let feline_core::presence::Activity::Game { place_id, job_id, .. } = &activity
                        && !job_id.is_empty()
                        && *job_id != last_job
                    {
                        last_job = job_id.clone();
                        let _ = feline_core::roblox_tools::LastServer {
                            place_id: *place_id,
                            job_id: job_id.clone(),
                            at: feline_core::history::now(),
                        }
                        .save();
                    }
                    // "Close Roblox when you leave a game": end it once it's
                    // back at its menus or its window is gone, so it doesn't
                    // linger in the taskbar and background. The game thread
                    // then sees it exit as usual.
                    if settings.close_roblox_on_leave {
                        use feline_core::presence::Activity;
                        let in_game = matches!(activity, Activity::Game { .. });
                        let at_menu = matches!(activity, Activity::Menu);
                        let window = feline_core::runtime::roblox::has_window(pid);
                        if leave.tick(in_game, at_menu, window)
                            && feline_core::runtime::roblox::end_game(pid)
                            && let Some(log) = follower.current()
                        {
                            // So the crash detective doesn't call it a crash.
                            feline_core::detective::remember_closed(log);
                        }
                    }
                    let (game, region) = match &activity {
                        feline_core::presence::Activity::Game {
                            universe_id, server, ..
                        } => {
                            let game = universe_id
                                .map(|u| {
                                    names
                                        .entry(u)
                                        .or_insert_with(|| api.game(u).map(|g| g.name).unwrap_or_default())
                                        .clone()
                                })
                                .unwrap_or_default();
                            let region = match server {
                                Some(ip) if regions => places
                                    .entry(ip.clone())
                                    .or_insert_with(|| api.server_region(ip).unwrap_or_default())
                                    .clone(),
                                _ => String::new(),
                            };
                            (game, region)
                        }
                        _ => (String::new(), String::new()),
                    };
                    on_stats(Stats {
                        game,
                        region,
                        time: duration_label(started.elapsed()),
                        cpu: format!("{:.0}%", sample.cpu_percent),
                        memory: format!(
                            "{:.1} GiB",
                            sample.memory_bytes as f64 / (1024.0 * 1024.0 * 1024.0)
                        ),
                    });
                }
            })
            .ok()?;
        Some(Self { stop, thread })
    }

    fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.thread.join();
    }
}

fn describe_exit(name: &str, exit: GameExit) -> Outcome {
    let log = Some(exit.log_path.clone());
    if exit.status.success() {
        return Outcome {
            state: LaunchState::Ended,
            title: format!("Nap time! You played for {}", duration_label(exit.ran_for)),
            detail: String::new(),
            log,
        };
    }
    let how = match exit.status.code() {
        Some(code) => format!("with exit code {code}"),
        None => "after being stopped by a signal".to_owned(),
    };
    let tail = launch::log_tail(&exit.log_path, 6);
    let mut detail = String::new();
    if !tail.is_empty() {
        detail.push_str(&tail);
        detail.push_str("\n\n");
    }
    detail.push_str(&format!("Full log: {}", tilde(&exit.log_path)));
    Outcome {
        state: LaunchState::Failed,
        title: format!(
            "{name} stopped {how} after {}",
            duration_label(exit.ran_for)
        ),
        detail,
        log,
    }
}

fn duration_label(d: Duration) -> String {
    let secs = d.as_secs();
    match secs {
        0..=59 => format!("{secs} s"),
        60..=3599 => format!("{} min", secs / 60),
        _ => format!("{} h {} min", secs / 3600, (secs % 3600) / 60),
    }
}

/// Opens a folder in the file manager (`xdg-open`; on Windows, Explorer).
pub(crate) fn open_folder(ui: &AppWindow, folder: PathBuf) {
    let opener = if cfg!(windows) { "explorer" } else { "xdg-open" };
    let result = std::fs::create_dir_all(&folder).and_then(|()| {
        std::process::Command::new(opener)
            .arg(&folder)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map(drop)
    });
    if let Err(e) = result {
        ui.set_launch_detail(format!("Couldn't open {} ({e}).", folder.display()).into());
    }
}

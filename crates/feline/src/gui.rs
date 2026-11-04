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
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use feline_core::launch::{self, GameExit};
use feline_core::link::{self, LinkError};
use feline_core::paths;
use feline_core::runtime::{DetectContext, InstallKind, Installation, LaunchTarget, detect_all};
use feline_core::settings::{ACCOUNT_NOTICE_VERSION, Settings};
use feline_core::system::{CheckLevel, FlatpakNvidia, SessionType, SystemInfo};
use slint::{ModelRc, SharedString, VecModel};

slint::include_modules!();

use crate::settings_page::{self, Edit};

/// A detected runtime and its version (looked up once, during detection).
#[derive(Clone)]
pub(crate) struct Found {
    pub(crate) installation: Installation,
    version: Option<String>,
}

#[derive(Default)]
pub(crate) struct State {
    pub(crate) found: Vec<Found>,
    pub(crate) settings: Settings,
    detecting: bool,
    /// True from pressing Play until the runtime exits.
    busy: bool,
    last_log: Option<PathBuf>,
    /// The last system scan (None until the first one finishes).
    system: Option<SystemInfo>,
    scanning_system: bool,
    /// True while a Diagnostics action is running.
    diag_busy: bool,
    /// The last failed save on the Settings page: (runtime id, message).
    pub(crate) settings_message: Option<(String, String)>,
}

pub(crate) type Shared = Arc<Mutex<State>>;

/// Locks the state. If another thread panicked while holding the lock, the
/// data is still usable for our purposes, so we continue instead of crashing.
pub(crate) fn lock(shared: &Shared) -> MutexGuard<'_, State> {
    shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn run() -> Result<(), slint::PlatformError> {
    // Creating the window also sets up Slint's platform (Wayland or X11).
    let ui = AppWindow::new()?;
    // The app ID is how Wayland/X11 desktops match the window to Feline's
    // menu entry and icon. It needs the platform, and must be set before the
    // window is shown (which happens in `ui.run()`).
    slint::set_xdg_app_id(paths::APP_ID)?;
    ui.set_version(feline_core::VERSION.into());
    // Developer aid (screenshots, testing a page): FELINE_START_PAGE=system
    match std::env::var("FELINE_START_PAGE").as_deref() {
        Ok("runtimes") => ui.set_page(Page::Runtimes),
        Ok("system") => ui.set_page(Page::System),
        Ok("diagnostics") => ui.set_page(Page::Diagnostics),
        Ok("about") => ui.set_page(Page::About),
        Ok("settings") => ui.set_page(Page::Settings),
        _ => {}
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

    ui.on_settings_opened({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                settings_page::refresh(&ui, &shared);
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

    show_link_check(&ui, "");
    ui.on_link_edited({
        let weak = ui.as_weak();
        move |text| {
            if let Some(ui) = weak.upgrade() {
                show_link_check(&ui, &text);
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

    ui.on_join({
        let weak = ui.as_weak();
        let shared = shared.clone();
        move |text| {
            let Some(ui) = weak.upgrade() else { return };
            match link::parse(&text) {
                Ok(game) => start_launch(&ui, &shared, LaunchTarget::Game(game)),
                Err(e) => show_link_check(&ui, &e.to_string()),
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
    start_system_scan(&ui, &shared);
    ui.run()
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
            "Distribution",
            info.os.clone().unwrap_or_else(unknown),
            Level::Neutral,
        ),
        row(
            "Kernel",
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

    ui.set_system_rows(ModelRc::new(VecModel::from(rows)));
    ui.set_system_report(info.report().into());

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
        if installation.is_none() && !matches!(action, DiagAction::BugReport) {
            return; // the buttons are disabled in this case anyway
        }
        state.diag_busy = true;
        (installation, state.system.clone())
    };
    ui.set_diag_running(true);
    ui.set_diag_title(SharedString::new());
    ui.set_diag_output(SharedString::new());

    let weak = ui.as_weak();
    let shared_for_thread = shared.clone();
    let spawned = thread::Builder::new()
        .name("feline-diagnostics".into())
        .spawn(move || {
            let result = run_diagnostic(action, installation.as_ref(), system);
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
) -> DiagResult {
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
                title: "Bug report ready. Use Copy and paste it into your report.".to_owned(),
                output,
            }
        }
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

/// Validates the link as the user types and shows the result under the field.
fn show_link_check(ui: &AppWindow, text: &str) {
    let (ready, valid, message) = match link::parse(text) {
        Ok(game) => (
            true,
            true,
            match game.place_id() {
                Some(id) => format!("Valid link · experience {id}"),
                None => "Valid Roblox link".to_owned(),
            },
        ),
        // An empty field isn't an error; show nothing.
        Err(LinkError::Empty) => (false, true, String::new()),
        Err(e) => (false, false, e.to_string()),
    };
    ui.set_link_ready(ready);
    ui.set_link_valid(valid);
    ui.set_link_message(message.into());
}

// ---------------------------------------------------------------------------
// Runtimes
// ---------------------------------------------------------------------------

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

    let rows: Vec<RuntimeRow> = state
        .found
        .iter()
        .map(|f| RuntimeRow {
            key: f.installation.key().into(),
            name: f.installation.backend.name().into(),
            version: f.version.clone().unwrap_or_default().into(),
            kind: format!("External · {}", kind_label(&f.installation.kind)).into(),
            selected: saved == Some(f.installation.key().as_str()),
        })
        .collect();
    ui.set_runtimes(ModelRc::new(VecModel::from(rows)));

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
                format!("External runtime · {}", short_kind(&f.installation.kind)).into(),
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
fn selected(state: &State) -> Option<&Found> {
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

fn kind_label(kind: &InstallKind) -> String {
    match kind {
        InstallKind::Flatpak { .. } => kind.to_string(),
        InstallKind::AppImage(path) => format!("AppImage · {}", tilde(path)),
        InstallKind::Native(path) => format!("Installed program · {}", tilde(path)),
    }
}

fn short_kind(kind: &InstallKind) -> &'static str {
    match kind {
        InstallKind::Flatpak { .. } => "Flatpak",
        InstallKind::AppImage(_) => "AppImage",
        InstallKind::Native(_) => "installed program",
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

fn start_launch(ui: &AppWindow, shared: &Shared, target: LaunchTarget) {
    let (installation, settings) = {
        let mut state = lock(shared);
        if state.busy {
            return; // already starting or running
        }
        let Some(found) = selected(&state) else {
            drop(state);
            set_launch(
                ui,
                LaunchState::Failed,
                "Choose a runtime first",
                "Open the Runtimes page and pick the runtime Feline should use.",
            );
            return;
        };
        let installation = found.installation.clone();
        state.busy = true;
        (installation, state.settings.clone())
    };

    let name = installation.backend.name();
    set_launch(ui, LaunchState::Starting, &format!("Starting {name}…"), "");

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
            let outcome = run_game(
                &installation,
                &target,
                &settings,
                on_presence,
                |title, detail| {
                    let (title, detail) = (title.to_owned(), detail.to_owned());
                    let _ = weak.upgrade_in_event_loop(move |ui| {
                        set_launch(&ui, LaunchState::Running, &title, &detail);
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
            });
        });

    if let Err(e) = spawned {
        lock(shared).busy = false;
        set_launch(
            ui,
            LaunchState::Failed,
            "Couldn't start",
            &format!("Feline couldn't start a background task: {e}"),
        );
    }
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
    on_presence: impl Fn(String) + Send + 'static,
    on_running: impl FnOnce(&str, &str),
) -> Outcome {
    let overrides = &settings.overrides;
    let started_at = std::time::SystemTime::now();
    let name = installation.backend.name();

    // Write Feline's overrides into the runtime's settings first. A failure
    // isn't fatal: the game still starts, with the runtime's own settings.
    let settings_warning = installation
        .apply_overrides(&DetectContext::host(), overrides)
        .err()
        .map(|e| format!("Feline's settings weren't applied: {e}"));

    let plan = match installation.plan_launch(target) {
        Ok(plan) => plan,
        Err(e) => return Outcome::failed(format!("{name} couldn't be prepared"), e.to_string()),
    };
    let Some(log_dir) = paths::log_dir() else {
        return Outcome::failed("Couldn't start", "Your home folder couldn't be found.");
    };
    let game = match launch::spawn(plan.command, &log_dir, installation.backend.id()) {
        Ok(game) => game,
        Err(e) => return Outcome::failed(format!("{name} couldn't be started"), e.to_string()),
    };

    let title = match (&plan.note, target) {
        (Some(_), _) => format!("{name} is open"),
        (None, LaunchTarget::Home) => format!("Roblox is running in {name}"),
        (None, LaunchTarget::Game(_)) => format!("Joining the experience in {name}"),
    };
    // Feline's Discord status follows this session's Roblox log.
    let presence = overrides
        .discord_presence
        .then(|| {
            feline_core::presence::Session::start(
                installation.roblox_log_roots(&DetectContext::host()),
                started_at,
                settings.presence,
                on_presence,
            )
        })
        .and_then(Result::ok);

    let default_detail = if presence.is_some() {
        "Have fun! Keep Feline open to show your Discord status."
    } else {
        "Have fun! You can close Feline; the game keeps running."
    };
    let mut detail = plan
        .note
        .clone()
        .unwrap_or_else(|| default_detail.to_owned());
    if let Some(warning) = settings_warning {
        detail = format!("{warning}\n{detail}");
    }
    on_running(&title, &detail);

    let waited = game.wait();
    // Clears the Discord status.
    if let Some(presence) = presence {
        presence.stop();
    }
    match waited {
        Ok(exit) => describe_exit(name, exit),
        Err(e) => Outcome::failed(
            "Lost track of the game",
            format!("Feline couldn't wait for {name} to finish: {e}. It may still be running."),
        ),
    }
}

fn describe_exit(name: &str, exit: GameExit) -> Outcome {
    let log = Some(exit.log_path.clone());
    if exit.status.success() {
        return Outcome {
            state: LaunchState::Ended,
            title: format!("Session ended after {}", duration_label(exit.ran_for)),
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

/// Opens a folder in the file manager via `xdg-open`.
fn open_folder(ui: &AppWindow, folder: PathBuf) {
    let result = std::fs::create_dir_all(&folder).and_then(|()| {
        std::process::Command::new("xdg-open")
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

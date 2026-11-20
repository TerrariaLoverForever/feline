// SPDX-License-Identifier: GPL-3.0-or-later
//! The Settings page: Feline's overrides, and a tab per runtime that edits
//! that runtime's own settings file.
//!
//! Everything here reads or writes small files, so it runs on the UI thread.
//! The slow part of `gui.rs` (detecting runtimes) has already happened by the
//! time this page shows anything.

use feline_core::runtime::config::{ConfigError, Kind, SettingDef, managed_value};
use feline_core::runtime::{ApplyOutcome, DetectContext, Installation, backends};
use feline_core::settings::{AfterLaunch, GamePriority, Overrides, Renderer, Settings, Toggle};
use serde_json::Value;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::gui::{
    AppWindow, Level, RuntimeTab, SettingKind, SettingRow, Shared, State, lock, tilde,
};

/// The installation whose settings a runtime's tab edits: the selected one if
/// it belongs to this runtime, otherwise the first one found.
fn installation_for<'a>(state: &'a State, backend_id: &str) -> Option<&'a Installation> {
    let selected = state
        .settings
        .runtime
        .as_deref()
        .and_then(|key| state.found.iter().find(|f| f.installation.key() == key))
        .filter(|f| f.installation.backend.id() == backend_id);
    selected
        .or_else(|| {
            state
                .found
                .iter()
                .find(|f| f.installation.backend.id() == backend_id)
        })
        .map(|f| &f.installation)
}

/// Rebuilds the whole page from the current state and files.
pub fn refresh(ui: &AppWindow, shared: &Shared) {
    let state = lock(shared);
    let o = state.settings.overrides;
    ui.set_renderer_index(match o.renderer {
        Renderer::Runtime => 0,
        Renderer::Vulkan => 1,
        Renderer::OpenGl => 2,
    });
    ui.set_gamemode_index(toggle_index(o.gamemode));
    ui.set_close_index(toggle_index(o.close_on_leave));
    let p = state.settings.presence;
    ui.set_discord_on(o.discord_presence);
    ui.set_discord_user(p.show_user);
    ui.set_discord_join(p.join_button);
    ui.set_discord_game(p.game_button);
    ui.set_discord_menus(p.in_menus);
    ui.set_discord_text(state.settings.discord.text.clone().into());
    ui.set_discord_region(state.settings.discord.region);
    ui.set_discord_today(state.settings.discord.today);
    ui.set_link_handler(state.settings.link_handler);
    show_link_runtimes(ui, &state);
    ui.set_after_launch_index(match state.settings.after_launch {
        AfterLaunch::Stay => 0,
        AfterLaunch::Minimize => 1,
        AfterLaunch::Close => 2,
    });
    ui.set_priority_index(match state.settings.game_priority {
        GamePriority::Normal => 0,
        GamePriority::AboveNormal => 1,
        GamePriority::High => 2,
    });
    ui.set_fast_gpu(state.settings.fast_gpu);
    ui.set_verify_roblox(state.settings.verify_roblox);
    ui.set_close_stuck(state.settings.close_stuck_roblox);
    ui.set_close_on_leave(state.settings.close_roblox_on_leave);
    ui.set_roblox_icon(state.settings.roblox_icon);
    ui.set_title_bar_index(match state.settings.title_bar {
        feline_core::settings::TitleBar::Normal => 0,
        feline_core::settings::TitleBar::Purple => 1,
        feline_core::settings::TitleBar::Night => 2,
    });
    ui.set_break_index(match state.settings.break_minutes {
        0 => 0,
        1..=30 => 1,
        31..=60 => 2,
        61..=90 => 3,
        _ => 4,
    });
    ui.set_tidy_index(match state.settings.tidy_days {
        0 => 0,
        1..=14 => 1,
        _ => 2,
    });
    ui.set_roblox_updates(state.settings.roblox_updates);
    ui.set_log_days_index(match state.settings.log_days {
        0 => 0,
        1 => 1,
        2..=7 => 2,
        _ => 3,
    });
    ui.set_has_saved_login(
        feline_core::housekeeping::saved_login().is_some_and(|p| p.exists()),
    );
    if let Some(status) = crate::presence_helper::status() {
        ui.set_presence_status(status.into());
    }

    let ctx = DetectContext::host();
    let tabs: Vec<RuntimeTab> = backends()
        .iter()
        .map(|backend| {
            let message = state
                .settings_message
                .as_ref()
                .filter(|(id, _)| id == backend.id())
                .map(|(_, text)| text.clone());
            build_tab(
                backend.id(),
                backend.name(),
                installation_for(&state, backend.id()),
                &ctx,
                &o,
                message,
            )
        })
        .collect();
    ui.set_settings_tabs(ModelRc::new(VecModel::from(tabs)));
    drop(state);
    crate::play_extras::refresh(ui, shared);
}

fn toggle_index(t: Toggle) -> i32 {
    match t {
        Toggle::Runtime => 0,
        Toggle::On => 1,
        Toggle::Off => 2,
    }
}

fn build_tab(
    id: &str,
    name: &str,
    installation: Option<&Installation>,
    ctx: &DetectContext,
    overrides: &Overrides,
    message: Option<String>,
) -> RuntimeTab {
    let mut tab = RuntimeTab {
        id: id.into(),
        name: name.into(),
        source: SharedString::new(),
        status: SharedString::new(),
        status_level: Level::Neutral,
        editable: false,
        tabs: ModelRc::default(),
        rows: ModelRc::default(),
    };

    if installation.is_some_and(|i| i.backend.settings().is_empty()) {
        tab.status = format!(
            "{name} has its own settings window. Feline can't change {name}'s settings yet."
        )
        .into();
        return tab;
    }
    let Some(installation) = installation else {
        tab.status =
            format!("{name} isn't installed. The Runtimes page lists what Feline found.").into();
        return tab;
    };
    if let Some(path) = installation.config_path(ctx) {
        tab.source = format!("{} · {}", kind_name(installation), tilde(&path)).into();
    }

    match installation.load_config(ctx) {
        Ok(file) => {
            let (names, rows) = grouped_rows(installation, &file, overrides);
            tab.tabs = ModelRc::new(VecModel::from(names));
            tab.rows = ModelRc::new(VecModel::from(rows));
            if installation.is_running() {
                tab.status =
                    format!("{name} is running. Close it to change these settings.").into();
                tab.status_level = Level::Warning;
            } else {
                tab.editable = true;
            }
        }
        Err(ConfigError::Missing(_)) => {
            tab.status = format!(
                "{name} hasn't created its settings file yet. Start it once, then come back."
            )
            .into();
        }
        Err(e) => {
            tab.status = e.to_string().into();
            tab.status_level = Level::Problem;
        }
    }
    // A failed save from a moment ago is more important than the above.
    if let Some(message) = message {
        tab.status = message.into();
        tab.status_level = Level::Problem;
    }
    tab
}

fn kind_name(installation: &Installation) -> &'static str {
    use feline_core::runtime::InstallKind;
    match installation.kind {
        InstallKind::Flatpak { .. } => "Flatpak",
        InstallKind::AppImage(_) => "AppImage",
        InstallKind::Native(_) => "installed program",
        InstallKind::Official(_) => "Roblox's installer",
    }
}

/// The runtime's rows in its tabs' order (see `RuntimeBackend::setting_tabs`),
/// each knowing its tab and section, and the tab names.
fn grouped_rows(
    installation: &Installation,
    file: &feline_core::runtime::config::ConfigFile,
    overrides: &Overrides,
) -> (Vec<SharedString>, Vec<SettingRow>) {
    let defs = installation.backend.settings();
    let tabs = installation.backend.setting_tabs();
    if tabs.is_empty() {
        let rows = defs
            .iter()
            .enumerate()
            .map(|(i, def)| SettingRow {
                section_start: i == 0,
                ..row(def, &file.get(def), overrides)
            })
            .collect();
        return (vec!["Settings".into()], rows);
    }
    let mut rows = Vec::new();
    for (t, tab) in tabs.iter().enumerate() {
        for section in tab.sections {
            for (k, key) in section.keys.iter().enumerate() {
                if let Some(def) = defs.iter().find(|d| d.key == *key) {
                    rows.push(SettingRow {
                        tab: t as i32,
                        section: section.title.into(),
                        section_start: k == 0,
                        ..row(def, &file.get(def), overrides)
                    });
                }
            }
        }
    }
    (tabs.iter().map(|t| t.name.into()).collect(), rows)
}

/// One row, showing the value Feline will enforce if the setting is managed.
fn row(def: &SettingDef, current: &Value, overrides: &Overrides) -> SettingRow {
    let managed = def.link.and_then(|link| managed_value(link, overrides));
    let value = managed.as_ref().map_or(current, |(v, _)| v);

    let mut row = SettingRow {
        key: def.key.into(),
        label: def.label.into(),
        help: def.help.into(),
        kind: SettingKind::Toggle,
        on: false,
        choices: ModelRc::default(),
        choice_index: 0,
        number: 0,
        min: 0,
        max: 0,
        text: SharedString::new(),
        placeholder: SharedString::new(),
        managed: managed.is_some(),
        // `as_ref()` borrows instead of moving `managed`, which `value`
        // still borrows from.
        managed_reason: managed
            .as_ref()
            .map(|(_, why)| *why)
            .unwrap_or_default()
            .into(),
        tab: 0,
        section: SharedString::new(),
        section_start: false,
    };
    match def.kind {
        Kind::Toggle => row.on = value.as_bool().unwrap_or(false),
        Kind::Choice(choices) => {
            row.kind = SettingKind::Choice;
            let labels: Vec<SharedString> = choices.iter().map(|c| c.label.into()).collect();
            row.choices = ModelRc::new(VecModel::from(labels));
            row.choice_index = choices
                .iter()
                .position(|c| Some(c.value) == value.as_str())
                .unwrap_or(0) as i32;
        }
        Kind::Text { placeholder, .. } => {
            row.kind = SettingKind::Text;
            row.text = value.as_str().unwrap_or("").into();
            row.placeholder = placeholder.into();
        }
        Kind::Number { min, max } => {
            row.kind = SettingKind::Number;
            row.number = value.as_i64().unwrap_or(min) as i32;
            row.min = min as i32;
            row.max = max as i32;
        }
    }
    row
}

/// What the user changed on a runtime tab.
pub enum Edit {
    Bool(bool),
    Choice(usize),
    Number(i64),
    Text(String),
}

/// Saves one changed runtime setting, then redraws the page. A number
/// that saved fine isn't redrawn: it changes with every digit typed, and
/// redrawing rebuilt its box, which lost the keyboard focus each time.
pub fn set_runtime_value(ui: &AppWindow, shared: &Shared, backend_id: &str, key: &str, edit: Edit) {
    // Text being typed (e.g. half a proxy address) is saved once it's
    // valid; until then nothing happens, so the box keeps its focus.
    if let Edit::Text(text) = &edit {
        let valid = lock(shared)
            .found
            .iter()
            .find(|f| f.installation.backend.id() == backend_id)
            .and_then(|f| f.installation.backend.settings().iter().find(|d| d.key == key))
            .is_some_and(|def| match def.kind {
                Kind::Text { check, .. } => check.accepts(text.trim()),
                _ => false,
            });
        if !valid {
            return;
        }
    }
    let typed_number = matches!(edit, Edit::Number(_) | Edit::Text(_));
    let result = {
        let state = lock(shared);
        let installation = installation_for(&state, backend_id).cloned();
        drop(state);
        match installation {
            None => Err("this runtime isn't installed".to_owned()),
            Some(installation) => {
                let def = installation
                    .backend
                    .settings()
                    .iter()
                    .find(|d| d.key == key);
                match def {
                    None => Err(format!("unknown setting `{key}`")),
                    Some(def) => match edit_to_value(def, edit) {
                        None => Err(format!("that value isn't allowed for `{key}`")),
                        Some(value) => installation
                            .set_config_value(&DetectContext::host(), def, value)
                            .map_err(|e| e.to_string()),
                    },
                }
            }
        }
    };
    let message = result
        .err()
        .map(|e| (backend_id.to_owned(), format!("Not saved: {e}")));
    let unchanged_page = {
        let mut state = lock(shared);
        let unchanged = message.is_none() && state.settings_message.is_none();
        state.settings_message = message;
        unchanged
    };
    if typed_number && unchanged_page {
        return;
    }
    refresh(ui, shared);
}

fn edit_to_value(def: &SettingDef, edit: Edit) -> Option<Value> {
    match (def.kind, edit) {
        (Kind::Toggle, Edit::Bool(b)) => Some(Value::Bool(b)),
        (Kind::Choice(choices), Edit::Choice(i)) => choices.get(i).map(|c| Value::from(c.value)),
        (Kind::Number { .. }, Edit::Number(n)) => Some(Value::from(n)),
        (Kind::Text { check, .. }, Edit::Text(t)) => {
            let t = t.trim().to_owned();
            check.accepts(&t).then(|| Value::from(t))
        }
        _ => None,
    }
}

/// Changes one of Feline's overrides, saves Feline's settings and writes the
/// override into every installed runtime.
pub fn set_override(ui: &AppWindow, shared: &Shared, which: &str, index: i32) {
    let toggle = |i| match i {
        1 => Toggle::On,
        2 => Toggle::Off,
        _ => Toggle::Runtime,
    };
    {
        let mut state = lock(shared);
        // Feline's own options (not written into any runtime's file).
        let own = &mut state.settings;
        match which {
            "after-launch" => {
                own.after_launch = match index {
                    1 => AfterLaunch::Minimize,
                    2 => AfterLaunch::Close,
                    _ => AfterLaunch::Stay,
                };
            }
            "priority" => {
                own.game_priority = match index {
                    1 => GamePriority::AboveNormal,
                    2 => GamePriority::High,
                    _ => GamePriority::Normal,
                };
            }
            "fast-gpu" => own.fast_gpu = index != 0,
            "verify-roblox" => own.verify_roblox = index != 0,
            "close-stuck" => own.close_stuck_roblox = index != 0,
            "close-leave" => own.close_roblox_on_leave = index != 0,
            "roblox-icon" => own.roblox_icon = index != 0,
            "title-bar" => {
                own.title_bar = match index {
                    1 => feline_core::settings::TitleBar::Purple,
                    2 => feline_core::settings::TitleBar::Night,
                    _ => feline_core::settings::TitleBar::Normal,
                }
            }
            "break" => own.break_minutes = [0, 30, 60, 90, 120][index.clamp(0, 4) as usize],
            "tidy" => own.tidy_days = [0, 7, 30][index.clamp(0, 2) as usize],
            _ => {}
        }
        let o = &mut state.settings.overrides;
        match which {
            "after-launch" | "priority" | "fast-gpu" | "verify-roblox" | "close-stuck" | "close-leave" | "break"
            | "roblox-icon" | "title-bar" | "tidy" => {}
            "renderer" => {
                o.renderer = match index {
                    1 => Renderer::Vulkan,
                    2 => Renderer::OpenGl,
                    _ => Renderer::Runtime,
                }
            }
            "gamemode" => o.gamemode = toggle(index),
            "close-on-leave" => o.close_on_leave = toggle(index),
            _ => return,
        }
    }
    save_and_apply(ui, shared);
}

/// Changes one Discord status option. Turning the status on or off also
/// changes the runtimes' own Discord settings, so it's applied like an override.
pub fn set_presence(ui: &AppWindow, shared: &Shared, which: &str, value: bool) {
    {
        let mut state = lock(shared);
        let settings = &mut state.settings;
        match which {
            "enabled" => settings.overrides.discord_presence = value,
            "show-user" => settings.presence.show_user = value,
            "join-button" => settings.presence.join_button = value,
            "game-button" => settings.presence.game_button = value,
            "in-menus" => settings.presence.in_menus = value,
            "region" => settings.discord.region = value,
            "today" => settings.discord.today = value,
            _ => return,
        }
    }
    save_and_apply(ui, shared);
}

/// The choices for "Open them with" (links): the runtime in use, then each
/// installed runtime; and the index of the saved choice.
pub fn link_runtime_choices(state: &State) -> (Vec<SharedString>, i32) {
    let in_use = crate::gui::selected(state)
        .map(|f| format!("The runtime you use ({})", f.installation.backend.name()))
        .unwrap_or_else(|| "The runtime you use".to_owned());
    let mut names = vec![SharedString::from(in_use)];
    for f in &state.found {
        let name = f.installation.backend.name();
        let twins = state.found.iter().filter(|o| o.installation.backend.name() == name).count();
        names.push(if twins > 1 {
            format!("{name} ({})", crate::gui::short_kind(&f.installation.kind)).into()
        } else {
            name.into()
        });
    }
    let index = state
        .settings
        .link_runtime
        .as_deref()
        .and_then(|key| state.found.iter().position(|f| f.installation.key() == key))
        .map_or(0, |i| i as i32 + 1);
    (names, index)
}

/// Shows the "Open them with" choices.
pub fn show_link_runtimes(ui: &AppWindow, state: &State) {
    let (names, index) = link_runtime_choices(state);
    ui.set_link_runtimes(ModelRc::new(VecModel::from(names)));
    ui.set_link_runtime_index(index);
}

/// "Open them with": 0 is the runtime in use, then the installed runtimes.
pub fn set_link_runtime(ui: &AppWindow, shared: &Shared, index: i32) {
    {
        let mut state = lock(shared);
        let key = usize::try_from(index - 1)
            .ok()
            .and_then(|i| state.found.get(i))
            .map(|f| f.installation.key());
        state.settings.link_runtime = key;
        show_link_runtimes(ui, &state);
    }
    save_and_apply(ui, shared);
}

/// Switches opening `roblox://` links with Feline on or off.
pub fn set_link_handler(ui: &AppWindow, shared: &Shared, on: bool) {
    let result = {
        let mut state = lock(shared);
        let result = if on {
            crate::links::enable(&mut state.settings)
        } else {
            crate::links::disable(&mut state.settings)
        };
        if result.is_ok() {
            state.settings.link_handler = on;
        }
        result
    };
    match result {
        Ok(()) if on => {
            ui.set_link_status("Roblox links from the website now open with Feline.".into())
        }
        Ok(()) => {
            ui.set_link_status("Roblox links went back to the app that had them before.".into())
        }
        Err(e) => ui.set_link_status(format!("Not changed: {e}").into()),
    }
    save_and_apply(ui, shared);
}

/// Saves Feline's settings and writes the overrides into every installed runtime.
fn save_and_apply(ui: &AppWindow, shared: &Shared) {
    let (settings, installations) = {
        let mut state = lock(shared);
        state.settings_message = None;
        let installations: Vec<Installation> =
            state.found.iter().map(|f| f.installation.clone()).collect();
        (state.settings.clone(), installations)
    };

    let mut problems = Vec::new();
    match Settings::path() {
        Some(path) => {
            if let Err(e) = settings.save_to(&path) {
                problems.push(format!("Feline's own settings couldn't be saved: {e}"));
            }
        }
        None => problems.push("Feline's settings couldn't be saved: no home folder".to_owned()),
    }

    let (status, level) = apply_everywhere(&installations, &settings.overrides, problems);
    ui.set_apply_status(status.into());
    ui.set_apply_level(level);
    refresh(ui, shared);
}

/// Applies the overrides to each installation and summarises what happened.
fn apply_everywhere(
    installations: &[Installation],
    overrides: &Overrides,
    mut problems: Vec<String>,
) -> (String, Level) {
    let ctx = DetectContext::host();
    let mut updated = Vec::new();
    for installation in installations {
        let name = installation.backend.name();
        match installation.apply_overrides(&ctx, overrides) {
            Ok(ApplyOutcome::Changed(_)) if !updated.contains(&name) => updated.push(name),
            Ok(_) => {}
            // It will be applied when Feline starts it next time.
            Err(ConfigError::Running(_)) => problems.push(format!(
                "{name} is running; it'll be updated next time Feline starts it"
            )),
            Err(e) => problems.push(format!("{name}: {e}")),
        }
    }

    let mut status = if updated.is_empty() {
        "Saved.".to_owned()
    } else {
        format!("Saved and written to {}.", updated.join(", "))
    };
    if problems.is_empty() {
        (status, Level::Ok)
    } else {
        status.push(' ');
        status.push_str(&problems.join(". "));
        status.push('.');
        (status, Level::Warning)
    }
}

/// Saves Feline's own settings file (for settings no runtime uses).
pub(crate) fn save_own(shared: &Shared) -> Result<(), String> {
    let state = lock(shared);
    match Settings::path() {
        Some(path) => state.settings.save_to(&path).map_err(|e| e.to_string()),
        None => Err("no home folder".to_owned()),
    }
}

pub fn set_roblox_updates(ui: &AppWindow, shared: &Shared, on: bool) {
    lock(shared).settings.roblox_updates = on;
    if let Err(e) = save_own(shared) {
        ui.set_housekeeping_status(format!("Couldn't save: {e}").into());
    }
}

/// "Delete logs automatically": Keep them, 1, 7 or 30 days.
pub fn set_log_days(ui: &AppWindow, shared: &Shared, index: i32) {
    let days = [0, 1, 7, 30][index.clamp(0, 3) as usize];
    lock(shared).settings.log_days = days;
    if let Err(e) = save_own(shared) {
        ui.set_housekeeping_status(format!("Couldn't save: {e}").into());
        return;
    }
    if days > 0 {
        let deleted = feline_core::housekeeping::delete_old_logs(days);
        if deleted > 0 {
            ui.set_housekeeping_status(format!("Deleted {deleted} old log files.").into());
        }
        refresh_sizes(ui);
    }
}

/// The Privacy and Storage tabs' buttons.
pub fn housekeeping(ui: &AppWindow, shared: &Shared, action: &str) {
    use feline_core::housekeeping as hk;
    let status = match action {
        "delete-roblox-logs" => Some(match hk::delete_roblox_logs() {
            0 => "There were no Roblox logs to delete.".to_owned(),
            n => format!("Deleted {n} Roblox log files."),
        }),
        "delete-feline-logs" => Some(match hk::delete_feline_logs() {
            0 => "There were no Feline logs to delete.".to_owned(),
            n => format!("Deleted {n} Feline log files."),
        }),
        "forget-login" => Some(if hk::forget_login() {
            "Signed out. Feline's runtime asks you to sign in next time.".to_owned()
        } else {
            "There was no saved login.".to_owned()
        }),
        "clear-cache" => Some(if hk::clear_roblox_cache() {
            "Roblox's cache is cleared.".to_owned()
        } else {
            "Roblox's cache couldn't be cleared (is a game running?).".to_owned()
        }),
        "clear-roblox" | "clear-roblox-studio" => Some(match hk::clear_roblox(action == "clear-roblox-studio") {
            Ok(c) => {
                let mut text = format!("Roblox is fresh: {} freed. Sign in again next time you play.", hk::human(c.freed));
                if c.logins > 0 {
                    text.push_str(&format!(" Studio's saved login is gone too ({} entries).", c.logins));
                }
                if !c.kept.is_empty() {
                    text.push_str(&format!(" Couldn't delete: {}.", c.kept.join(", ")));
                }
                text
            }
            Err(e) => e,
        }),
        "clear-shaders" => Some(match hk::clear_shader_cache() {
            Ok((0, _)) => "The shader cache was already empty.".to_owned(),
            Ok((bytes, 0)) => format!("Shader cache cleared: {} freed.", hk::human(bytes)),
            Ok((bytes, kept)) => format!(
                "Shader cache cleared: {} freed. {kept} files another program is using stayed.",
                hk::human(bytes)
            ),
            Err(e) => e,
        }),
        "tidy" => Some(match hk::tidy() {
            Ok(0) => "Already tidy. Purr.".to_owned(),
            Ok(bytes) => format!("Tidied up: {} freed.", hk::human(bytes)),
            Err(e) => e,
        }),
        "clean-versions" => Some(match hk::delete_leftover_versions() {
            Ok(0) => "There were no old Roblox versions to delete.".to_owned(),
            Ok(bytes) => format!("Deleted old Roblox versions: {} freed.", hk::human(bytes)),
            Err(e) => e,
        }),
        "check-update" => {
            crate::gui::check_roblox_update(ui, shared);
            None
        }
        _ => None,
    };
    if let Some(status) = status {
        ui.set_housekeeping_status(status.into());
    }
    crate::accounts_page::refresh(ui);
    refresh_sizes(ui);
}

/// Measures the folders shown on the Storage and Privacy tabs, off the UI
/// thread (the cache can hold many files).
pub fn refresh_sizes(ui: &AppWindow) {
    let weak = ui.as_weak();
    let _ = std::thread::Builder::new()
        .name("feline-sizes".into())
        .spawn(move || {
            use feline_core::housekeeping as hk;
            let measure = |dir: Option<std::path::PathBuf>| hk::human(dir.map_or(0, |d| hk::size(&d)));
            let cache = measure(hk::roblox_cache_dir());
            let roblox_logs = measure(hk::roblox_log_dir());
            let feline_logs = measure(feline_core::paths::log_dir());
            let shaders = hk::human(hk::shader_cache_dirs().iter().map(|d| hk::size(d)).sum());
            let total = hk::roblox_total_size();
            let space = hk::record_space(total);
            let most = space.iter().map(|(_, b)| *b).max().unwrap_or(1).max(1) as f32;
            let bars: Vec<f32> = space.iter().map(|(_, b)| *b as f32 / most).collect();
            let change = match space.first() {
                Some(&(day, first)) if space.len() > 1 => {
                    let days = space.last().map_or(0, |l| l.0) - day;
                    let since = if days == 1 { "yesterday".to_owned() } else { format!("{days} days ago") };
                    if total >= first {
                        format!("{} more than {since}", hk::human(total - first))
                    } else {
                        format!("{} less than {since}", hk::human(first - total))
                    }
                }
                _ => "Feline keeps a daily record from today on.".to_owned(),
            };
            let last_tidy = match hk::last_tidy() {
                Some((when, freed)) => {
                    let days = feline_core::history::now().saturating_sub(when) / 86_400;
                    let ago = match days {
                        0 => "today".to_owned(),
                        1 => "yesterday".to_owned(),
                        d => format!("{d} days ago"),
                    };
                    format!("Last time: {ago}, {} freed.", hk::human(freed))
                }
                None => String::new(),
            };
            let roblox_data = hk::human(hk::roblox_data(false).iter().map(|p| hk::size(p)).sum());
            let versions = if cfg!(windows) {
                hk::human(hk::leftover_versions().iter().map(|d| hk::size(d)).sum())
            } else {
                measure(hk::roblox_versions_dir())
            };
            let _ = weak.upgrade_in_event_loop(move |ui| {
                ui.set_size_roblox_cache(cache.into());
                ui.set_size_roblox_logs(roblox_logs.into());
                ui.set_size_feline_logs(feline_logs.into());
                ui.set_size_roblox_versions(versions.into());
                ui.set_roblox_data_size(roblox_data.into());
                ui.set_size_shader_cache(shaders.into());
                ui.set_space_total(hk::human(total).into());
                ui.set_space_change(change.into());
                ui.set_space_history(slint::ModelRc::new(slint::VecModel::from(bars)));
                ui.set_last_tidy(last_tidy.into());
            });
        });
}

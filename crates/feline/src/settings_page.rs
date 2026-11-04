// SPDX-License-Identifier: GPL-3.0-or-later
//! The Settings page: Feline's overrides, and a tab per runtime that edits
//! that runtime's own settings file.
//!
//! Everything here reads or writes small files, so it runs on the UI thread.
//! The slow part of `gui.rs` (detecting runtimes) has already happened by the
//! time this page shows anything.

use feline_core::runtime::config::{ConfigError, Kind, SettingDef, managed_value};
use feline_core::runtime::{ApplyOutcome, DetectContext, Installation, backends};
use feline_core::settings::{Overrides, Renderer, Settings, Toggle};
use serde_json::Value;
use slint::{ModelRc, SharedString, VecModel};

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

    let ctx = DetectContext::host();
    let tabs: Vec<RuntimeTab> = backends()
        .iter()
        .filter(|b| !b.settings().is_empty())
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
        rows: ModelRc::default(),
    };
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
            let rows: Vec<SettingRow> = installation
                .backend
                .settings()
                .iter()
                .map(|def| row(def, &file.get(def), overrides))
                .collect();
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
    }
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
        managed: managed.is_some(),
        // `as_ref()` borrows instead of moving `managed`, which `value`
        // still borrows from.
        managed_reason: managed
            .as_ref()
            .map(|(_, why)| *why)
            .unwrap_or_default()
            .into(),
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
}

/// Saves one changed runtime setting, then redraws the page.
pub fn set_runtime_value(ui: &AppWindow, shared: &Shared, backend_id: &str, key: &str, edit: Edit) {
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
    lock(shared).settings_message = result
        .err()
        .map(|e| (backend_id.to_owned(), format!("Not saved: {e}")));
    refresh(ui, shared);
}

fn edit_to_value(def: &SettingDef, edit: Edit) -> Option<Value> {
    match (def.kind, edit) {
        (Kind::Toggle, Edit::Bool(b)) => Some(Value::Bool(b)),
        (Kind::Choice(choices), Edit::Choice(i)) => choices.get(i).map(|c| Value::from(c.value)),
        (Kind::Number { .. }, Edit::Number(n)) => Some(Value::from(n)),
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
    let (settings, installations) = {
        let mut state = lock(shared);
        let o = &mut state.settings.overrides;
        match which {
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

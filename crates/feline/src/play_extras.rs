// SPDX-License-Identifier: GPL-3.0-or-later
//! Settings → General → "While you play" (PlayExtras in play_extras.slint):
//! more than one Roblox, the server pop-up, programs that start with Roblox,
//! and staying on a Roblox version.

use std::thread;

use feline_core::runtime::{DetectContext, InstallKind};
use feline_core::settings::Program;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::gui::{AppWindow, PlayExtras, ProgramRow, Shared, lock, selected};

/// Shows the current settings.
pub fn refresh(ui: &AppWindow, shared: &Shared) {
    let extras = ui.global::<PlayExtras>();
    let state = lock(shared);
    let settings = &state.settings;
    extras.set_windows(cfg!(windows));
    extras.set_multi(settings.multi_roblox);
    extras.set_popup(settings.server_popup);
    extras.set_confirm(settings.confirm_launch);
    extras.set_splash(settings.launch_splash);
    let rows: Vec<ProgramRow> = settings
        .programs
        .iter()
        .map(|p| ProgramRow {
            name: p.name().into(),
            path: p.path.clone().into(),
            args: p.args.clone().into(),
            close_after: p.close_after,
        })
        .collect();
    extras.set_programs(ModelRc::new(VecModel::from(rows)));
    let official = selected(&state).filter(|f| matches!(f.installation.kind, InstallKind::Official(_)));
    extras.set_can_pin(official.is_some());
    extras.set_version_now(
        official
            .and_then(|f| f.installation.known_version.clone())
            .unwrap_or_default()
            .into(),
    );
    let pinned = settings.roblox_pin.as_deref().map(|name| {
        feline_core::runtime::roblox::kept_player(name)
            .and_then(|exe| feline_win_version(&exe))
            .unwrap_or_else(|| name.to_owned())
    });
    extras.set_pinned(pinned.unwrap_or_default().into());
}

#[cfg(windows)]
fn feline_win_version(exe: &std::path::Path) -> Option<String> {
    feline_win::file_version(exe)
}

#[cfg(not(windows))]
fn feline_win_version(_exe: &std::path::Path) -> Option<String> {
    None
}

/// Changes the settings with `change`, saves them and shows them again.
fn change(ui: &AppWindow, shared: &Shared, change: impl FnOnce(&mut feline_core::settings::Settings)) {
    change(&mut lock(shared).settings);
    let message = match crate::settings_page::save_own(shared) {
        Ok(()) => String::new(),
        Err(e) => format!("Feline couldn't save that: {e}"),
    };
    ui.global::<PlayExtras>().set_message(message.into());
    refresh(ui, shared);
}

pub fn set_multi(ui: &AppWindow, shared: &Shared, on: bool) {
    change(ui, shared, |s| s.multi_roblox = on);
}

pub fn set_popup(ui: &AppWindow, shared: &Shared, on: bool) {
    change(ui, shared, |s| s.server_popup = on);
}

pub fn set_confirm(ui: &AppWindow, shared: &Shared, on: bool) {
    change(ui, shared, |s| s.confirm_launch = on);
}

pub fn set_splash(ui: &AppWindow, shared: &Shared, on: bool) {
    change(ui, shared, |s| s.launch_splash = on);
}

pub fn add_program(ui: &AppWindow, shared: &Shared) {
    let patterns: &[&str] = if cfg!(windows) { &["*.exe", "*.bat", "*.cmd", "*.lnk"] } else { &["*"] };
    let Some(file) = crate::picker::pick_file("Choose a program to start with Roblox", "Programs", patterns) else {
        return;
    };
    let path = file.to_string_lossy().into_owned();
    change(ui, shared, |s| {
        if !s.programs.iter().any(|p| p.path == path) {
            s.programs.push(Program { path, args: String::new(), close_after: true });
        }
    });
}

pub fn remove_program(ui: &AppWindow, shared: &Shared, index: i32) {
    change(ui, shared, |s| {
        if (0..s.programs.len() as i32).contains(&index) {
            s.programs.remove(index as usize);
        }
    });
}

pub fn set_program_close(ui: &AppWindow, shared: &Shared, index: i32, on: bool) {
    change(ui, shared, |s| {
        if let Some(p) = s.programs.get_mut(index.max(0) as usize) {
            p.close_after = on;
        }
    });
}

/// Arguments are saved as they're typed, without redrawing the list (that
/// would reset the text field under the cursor).
pub fn set_program_args(shared: &Shared, index: i32, args: &str) {
    if let Some(p) = lock(shared).settings.programs.get_mut(index.max(0) as usize) {
        p.args = args.replace(['\n', '|'], " ");
    }
    let _ = crate::settings_page::save_own(shared);
}

/// Keeps the Roblox version there is now and stays on it (in the
/// background: the first time copies or links a few hundred MB).
pub fn pin(ui: &AppWindow, shared: &Shared) {
    let Some(installation) = selected(&lock(shared)).map(|f| f.installation.clone()) else {
        return;
    };
    let extras = ui.global::<PlayExtras>();
    extras.set_busy(true);
    extras.set_message("Keeping a copy of this Roblox…".into());
    let weak = ui.as_weak();
    let shared = shared.clone();
    let _ = thread::Builder::new().name("feline-keep-version".into()).spawn(move || {
        let result = feline_core::runtime::roblox::keep_version(&installation, &DetectContext::host());
        let _ = weak.upgrade_in_event_loop(move |ui| {
            ui.global::<PlayExtras>().set_busy(false);
            match result {
                Ok(name) => {
                    change(&ui, &shared, |s| s.roblox_pin = Some(name));
                    ui.global::<PlayExtras>()
                        .set_message("Done. Feline starts this version from now on, even after Roblox updates.".into());
                }
                Err(e) => {
                    ui.global::<PlayExtras>().set_message(SharedString::from(format!("Feline couldn't keep it: {e}")));
                    refresh(&ui, &shared);
                }
            }
        });
    });
}

/// Back to the newest Roblox; the kept copy is deleted.
pub fn unpin(ui: &AppWindow, shared: &Shared) {
    let (old, running) = {
        let mut state = lock(shared);
        let running = selected(&state).is_some_and(|f| f.installation.is_running());
        (state.settings.roblox_pin.take(), running)
    };
    change(ui, shared, |_| {});
    // The kept copy goes, unless it may be the Roblox that's running.
    if let Some(name) = old
        && !running
    {
        feline_core::runtime::roblox::forget_version(&name);
    }
    ui.global::<PlayExtras>().set_message("Back to the newest Roblox from the next Play.".into());
}

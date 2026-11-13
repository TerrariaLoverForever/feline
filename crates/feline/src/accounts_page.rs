// SPDX-License-Identifier: GPL-3.0-or-later
//! Settings → Privacy: the Roblox accounts Feline's runtime can sign in
//! with (see feline-core's accounts.rs).

use feline_core::accounts;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::gui::{AccountRow, AppWindow, More};

/// Shows the saved accounts.
pub fn refresh(ui: &AppWindow) {
    let rows: Vec<AccountRow> = accounts::list()
        .into_iter()
        .map(|saved| {
            let a = saved.account;
            let handle = format!("@{}", a.name);
            AccountRow {
                id: a.id.to_string().into(),
                detail: if saved.active { format!("{handle} · in use").into() } else { SharedString::from(handle) },
                name: if a.display_name.is_empty() { a.name.into() } else { a.display_name.into() },
                active: saved.active,
            }
        })
        .collect();
    let more = ui.global::<More>();
    more.set_accounts(ModelRc::new(VecModel::from(rows)));
    more.set_account_unsaved(accounts::unsaved_login());
    ui.set_has_saved_login(feline_core::housekeeping::saved_login().is_some_and(|p| p.exists()));
}

/// "Kit (@kitty)".
fn label(a: &accounts::Account) -> String {
    if a.display_name.is_empty() || a.display_name == a.name {
        format!("@{}", a.name)
    } else {
        format!("{} (@{})", a.display_name, a.name)
    }
}

/// Runs `work` off the UI thread (it opens a window or asks Roblox), then
/// shows what it says.
fn in_background(ui: &AppWindow, work: impl FnOnce() -> String + Send + 'static) {
    let more = ui.global::<More>();
    more.set_accounts_busy(true);
    more.set_accounts_status(SharedString::new());
    let weak = ui.as_weak();
    let spawned = std::thread::Builder::new().name("feline-accounts".into()).spawn(move || {
        let status = work();
        let _ = weak.upgrade_in_event_loop(move |ui| {
            let more = ui.global::<More>();
            more.set_accounts_busy(false);
            more.set_accounts_status(status.into());
            refresh(&ui);
        });
    });
    if spawned.is_err() {
        more.set_accounts_busy(false);
    }
}

/// Whether Feline's runtime is running (a switch then waits for its next start).
fn next_start_note() -> &'static str {
    // By program file: Roblox's engine renames the runtime's main thread.
    let running = std::fs::read_dir("/proc").is_ok_and(|entries| {
        entries.filter_map(Result::ok).any(|e| {
            std::fs::read_link(e.path().join("exe")).is_ok_and(|exe| exe.file_name().is_some_and(|n| n == "feline-runtime"))
        })
    });
    if running {
        " The game that's open keeps its account until you restart it."
    } else {
        ""
    }
}

fn act(ui: &AppWindow, action: &str, id: &str) {
    let id: Option<u64> = id.parse().ok();
    match (action, id) {
        ("add", _) => in_background(ui, || match accounts::add_with_login_window() {
            Ok(a) => format!("{} is added and in use.{}", label(&a), next_start_note()),
            Err(e) => format!("No account was added: {e}."),
        }),
        ("save", _) => in_background(ui, || match accounts::save_current() {
            Ok(Some(a)) => format!("{} is in the list now.", label(&a)),
            Ok(None) => "It's in the list already.".to_owned(),
            Err(e) => format!("Couldn't keep it: {e}."),
        }),
        ("use", Some(id)) => in_background(ui, move || match accounts::switch(id) {
            Ok(a) => format!("Switched to {}.{}", label(&a), next_start_note()),
            Err(e) => format!("Couldn't switch: {e}."),
        }),
        ("remove", Some(id)) => {
            let status = if accounts::remove(id) { "The account is removed from this computer." } else { "That account was gone already." };
            ui.global::<More>().set_accounts_status(status.into());
            refresh(ui);
        }
        _ => {}
    }
}

/// Hooks up the callbacks.
pub fn wire(ui: &AppWindow) {
    refresh(ui);
    ui.global::<More>().on_account({
        let weak = ui.as_weak();
        move |action, id| {
            if let Some(ui) = weak.upgrade() {
                act(&ui, action.as_str(), id.as_str());
            }
        }
    });
}

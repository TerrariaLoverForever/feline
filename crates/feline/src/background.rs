// SPDX-License-Identifier: GPL-3.0-or-later
//! Feline running in the background: closing its window hides it, so the
//! desk cat, music, the game's stats and "close Roblox when you leave" keep
//! going. Opening Feline again shows the copy that's already running
//! instead of starting a second one.
//!
//! Windows: an icon by the clock brings the window back or quits; another
//! copy wakes it through that icon's window. Linux (where a tray isn't a
//! given): another copy wakes it through a small local socket, and the
//! sidebar's Quit button ends it.

use std::sync::atomic::{AtomicBool, Ordering};

use slint::ComponentHandle;

use crate::gui::{AppWindow, Shared, lock};

/// The name other copies find this one's tray by.
#[cfg(windows)]
const TRAY_NAME: &str = "Feline app";

/// Set when Feline quits, so the tray thread takes its icon away.
static STOP: AtomicBool = AtomicBool::new(false);
/// The "still here" card is shown once per run.
static TOLD: AtomicBool = AtomicBool::new(false);

/// True if another Feline window is already running (it was asked to show
/// itself, so this copy should simply end).
pub fn already_running() -> bool {
    #[cfg(windows)]
    {
        match feline_win::game::OnlyOne::take("App") {
            // Ours for as long as this copy runs.
            Some(marker) => {
                std::mem::forget(marker);
                false
            }
            None => {
                // Give the other copy's tray a moment if it's just starting.
                for _ in 0..20 {
                    if feline_win::tray::wake(TRAY_NAME) {
                        return true;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                // No tray answered: the other copy runs without one (keep
                // running is off); open anyway rather than nothing.
                false
            }
        }
    }
    #[cfg(not(windows))]
    {
        use std::io::Write as _;
        let Some(path) = socket_path() else { return false };
        // Someone's listening: that's the running Feline; ask it to show.
        if let Ok(mut stream) = std::os::unix::net::UnixStream::connect(&path) {
            let _ = stream.write_all(b"show\n");
            return true;
        }
        // Nobody: a leftover from a crash. This copy takes over.
        let _ = std::fs::remove_file(&path);
        if let Ok(listener) = std::os::unix::net::UnixListener::bind(&path) {
            let _ = LISTENER.set(listener);
        }
        false
    }
}

/// Where the running Feline listens (Linux): in the session's runtime
/// folder, only for this user.
#[cfg(not(windows))]
fn socket_path() -> Option<std::path::PathBuf> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .or_else(feline_core::paths::state_dir)?;
    Some(dir.join("feline.sock"))
}

#[cfg(not(windows))]
static LISTENER: std::sync::OnceLock<std::os::unix::net::UnixListener> = std::sync::OnceLock::new();

/// What closing the window does: hide to the tray (keep running on), or
/// quit.
pub fn on_close(shared: &Shared) -> slint::CloseRequestResponse {
    let keep = lock(shared).settings.keep_running;
    if keep {
        if !TOLD.swap(true, Ordering::Relaxed) {
            let text = if cfg!(windows) {
                "It keeps running by the clock. Click the cat icon to bring it back, or right-click it to quit."
            } else {
                "It keeps running in the background. Open Feline again from your apps to bring it back; Quit in its sidebar ends it."
            };
            feline_core::launch::notify_quiet("Feline is still here", text);
        }
    } else {
        let _ = slint::quit_event_loop();
    }
    slint::CloseRequestResponse::HideWindow
}

/// Starts the tray icon (Windows).
pub fn start(ui: &AppWindow) {
    #[cfg(windows)]
    {
        let weak = ui.as_weak();
        let _ = std::thread::Builder::new().name("feline-app-tray".into()).spawn(move || {
            use feline_win::tray::{Item, Tray};
            let icon = feline_core::runtime::roblox::feline_icon_file();
            let Some(tray) = Tray::named(icon.as_deref(), "Feline", TRAY_NAME) else { return };
            let show = |weak: &slint::Weak<AppWindow>| {
                let _ = weak.upgrade_in_event_loop(|ui| {
                    let _ = ui.show();
                    ui.window().set_minimized(false);
                });
            };
            while !STOP.load(Ordering::Relaxed) {
                if tray.woken() {
                    show(&weak);
                }
                if tray.clicked() {
                    let items = [
                        Item::Action(1, "Open Feline"),
                        Item::Action(2, "Play Roblox"),
                        Item::Separator,
                        Item::Action(3, "Quit Feline"),
                    ];
                    match tray.menu(&items) {
                        Some(1) => show(&weak),
                        Some(2) => {
                            let _ = weak.upgrade_in_event_loop(|ui| ui.invoke_play());
                        }
                        Some(3) => {
                            let _ = slint::invoke_from_event_loop(|| {
                                let _ = slint::quit_event_loop();
                            });
                        }
                        _ => {}
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(60));
            }
            drop(tray);
        });
    }
    #[cfg(not(windows))]
    if let Some(listener) = LISTENER.get().and_then(|l| l.try_clone().ok()) {
        let weak = ui.as_weak();
        let _ = std::thread::Builder::new().name("feline-wake".into()).spawn(move || {
            for stream in listener.incoming() {
                if STOP.load(Ordering::Relaxed) {
                    break;
                }
                if stream.is_ok() {
                    let _ = weak.upgrade_in_event_loop(|ui| {
                        let _ = ui.show();
                        ui.window().set_minimized(false);
                    });
                }
            }
        });
    }
}

/// Feline is quitting: the tray icon goes.
pub fn stop() {
    STOP.store(true, Ordering::Relaxed);
    #[cfg(not(windows))]
    if let Some(path) = socket_path() {
        let _ = std::fs::remove_file(path);
    }
    std::thread::sleep(std::time::Duration::from_millis(120));
}

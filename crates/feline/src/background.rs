// SPDX-License-Identifier: GPL-3.0-or-later
//! Feline running in the background: closing its window hides it, so the
//! desk cat, music, the game's stats and "close Roblox when you leave" keep
//! going. Opening Feline again shows the copy that's already running
//! instead of starting a second one.
//!
//! Windows: an icon by the clock brings the window back or quits; another
//! copy wakes it through that icon's window. Linux: a tray icon too
//! (freedesktop StatusNotifierItem: KDE Plasma, XFCE, Cinnamon, LXQt,
//! waybar, GNOME with the AppIndicator extension) when the desktop has a
//! tray; another copy wakes it through a small local socket, and the
//! sidebar's Quit button always ends it.

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
/// Linux: a desktop tray took Feline's icon.
#[cfg(target_os = "linux")]
static TRAY_UP: AtomicBool = AtomicBool::new(false);
#[cfg(target_os = "linux")]
static TRAY: std::sync::Mutex<Option<ksni::blocking::Handle<LinuxTray>>> = std::sync::Mutex::new(None);

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
            #[cfg(target_os = "linux")]
            let tray = TRAY_UP.load(Ordering::Relaxed);
            #[cfg(not(target_os = "linux"))]
            let tray = false;
            let text = if cfg!(windows) {
                "It keeps running by the clock. Click the cat icon to bring it back, or right-click it to quit."
            } else if tray {
                "It keeps running in your system tray. Click the cat icon to bring it back, or right-click it to quit."
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

/// Starts the tray icon (Windows, and Linux desktops with a tray).
pub fn start(ui: &AppWindow) {
    #[cfg(target_os = "linux")]
    start_linux_tray(ui);
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
    #[cfg(target_os = "linux")]
    if let Some(tray) = TRAY.lock().ok().and_then(|mut t| t.take()) {
        tray.shutdown().wait();
    }
    #[cfg(not(windows))]
    if let Some(path) = socket_path() {
        let _ = std::fs::remove_file(path);
    }
    std::thread::sleep(std::time::Duration::from_millis(120));
}

/// Feline's tray icon on Linux.
#[cfg(target_os = "linux")]
struct LinuxTray {
    ui: slint::Weak<AppWindow>,
    icons: Vec<ksni::Icon>,
}

#[cfg(target_os = "linux")]
fn show_window(weak: &slint::Weak<AppWindow>) {
    let _ = weak.upgrade_in_event_loop(|ui| {
        let _ = ui.show();
        ui.window().set_minimized(false);
    });
}

#[cfg(target_os = "linux")]
impl ksni::Tray for LinuxTray {
    // Not shown as a menu when clicked (left click opens Feline).
    const MENU_ON_ACTIVATE: bool = false;

    fn id(&self) -> String {
        feline_core::paths::APP_ID.into()
    }
    fn title(&self) -> String {
        "Feline".into()
    }
    fn icon_name(&self) -> String {
        // The installed icon (from `--install-desktop-entry`); trays that
        // can't find it use the pixmaps.
        feline_core::paths::APP_ID.into()
    }
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.icons.clone()
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "Feline".into(),
            description: "Click to open Feline".into(),
            ..Default::default()
        }
    }
    fn activate(&mut self, _x: i32, _y: i32) {
        show_window(&self.ui);
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::StandardItem;
        vec![
            StandardItem {
                label: "Open Feline".into(),
                icon_name: "window-new".into(),
                activate: Box::new(|this: &mut Self| show_window(&this.ui)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Play Roblox".into(),
                icon_name: "media-playback-start".into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.ui.upgrade_in_event_loop(|ui| ui.invoke_play());
                }),
                ..Default::default()
            }
            .into(),
            ksni::MenuItem::Separator,
            StandardItem {
                label: "Quit Feline".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|_: &mut Self| {
                    let _ = slint::invoke_from_event_loop(|| {
                        let _ = slint::quit_event_loop();
                    });
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// Feline's icon as tray pixmaps (ARGB32, big-endian), a few sizes.
#[cfg(target_os = "linux")]
fn tray_icons() -> Vec<ksni::Icon> {
    [
        &include_bytes!("../../../assets/icons/hicolor/32x32/apps/feline.png")[..],
        &include_bytes!("../../../assets/icons/hicolor/64x64/apps/feline.png")[..],
    ]
    .into_iter()
    .filter_map(|png| feline_core::png::decode(png).ok())
    .map(|img| ksni::Icon {
        width: img.width as i32,
        height: img.height as i32,
        data: img
            .pixels
            .chunks_exact(4)
            .flat_map(|p| [p[3], p[0], p[1], p[2]])
            .collect(),
    })
    .collect()
}

/// Puts Feline's icon in the desktop's tray, if it has one (a desktop
/// without a StatusNotifierWatcher, e.g. plain GNOME, just gets none).
#[cfg(target_os = "linux")]
fn start_linux_tray(ui: &AppWindow) {
    use ksni::blocking::TrayMethods as _;
    let tray = LinuxTray {
        ui: ui.as_weak(),
        icons: tray_icons(),
    };
    match tray.spawn() {
        Ok(handle) => {
            TRAY_UP.store(true, Ordering::Relaxed);
            if let Ok(mut slot) = TRAY.lock() {
                *slot = Some(handle);
            }
        }
        Err(e) => eprintln!("feline: no tray icon ({e})"),
    }
}

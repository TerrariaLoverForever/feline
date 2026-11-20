// SPDX-License-Identifier: GPL-3.0-or-later
//! The desk cat's life: a background thread that owns the cat's own
//! see-through window, moves the cat toward the pointer every tick and
//! draws it. Started and stopped from the settings, and from games
//! starting and ending ("only while playing").
//!
//! On Windows the window is a layered window. On Linux it's a Wayland
//! layer-shell surface (see [`wayland`]); where the desktop doesn't tell
//! Feline where the pointer is, the cat wanders around on its own. X11
//! sessions and GNOME (no layer-shell) have no cat yet: [`supported`] is
//! false there and nothing starts.

#[cfg(target_os = "linux")]
mod wayland;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use feline_core::cat::Coat;
use feline_core::settings::DeskCat;

/// Whether the desk cat can live on this system.
pub fn supported() -> bool {
    #[cfg(target_os = "linux")]
    return wayland::supported();
    #[cfg(not(target_os = "linux"))]
    return cfg!(windows);
}

/// A word about the cat on this system for its settings ("" for none).
pub fn note() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        if !wayland::supported() {
            return if std::env::var_os("WAYLAND_DISPLAY").is_none() {
                "Not in X11 sessions yet: the cat needs a Wayland session."
            } else {
                "Not on this desktop: the cat needs layer-shell windows, which KDE Plasma, Hyprland, Sway and most Wayland desktops have, but GNOME doesn't."
            };
        }
        if !wayland::follows_pointer() {
            return "This desktop doesn't tell Feline where the pointer is, so here the cat wanders around the screen on its own. (On KDE Plasma and Hyprland it follows the pointer.)";
        }
        ""
    }
    #[cfg(not(target_os = "linux"))]
    {
        if cfg!(windows) { "" } else { "Not on this system yet." }
    }
}

/// How the cat looks (can change while it runs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
pub struct Look {
    pub coat: Coat,
    pub scale: u32,
    pub collar: u32,
}

struct Running {
    stop: Arc<AtomicBool>,
    look: Arc<Mutex<Look>>,
    thread: std::thread::JoinHandle<()>,
}

struct Control {
    settings: Option<DeskCat>,
    collar: u32,
    playing: bool,
    running: Option<Running>,
}

fn control() -> MutexGuard<'static, Control> {
    static CONTROL: Mutex<Control> = Mutex::new(Control {
        settings: None,
        collar: 0,
        playing: false,
        running: None,
    });
    CONTROL.lock().unwrap_or_else(|p| p.into_inner())
}

/// The cat's settings or collar colour changed.
pub fn configure(settings: DeskCat, collar: u32) {
    let mut c = control();
    c.settings = Some(settings);
    c.collar = collar;
    sync(&mut c);
}

/// A game started or ended.
pub fn set_playing(playing: bool) {
    let mut c = control();
    c.playing = playing;
    sync(&mut c);
}

/// Feline is closing: take the cat home.
pub fn shutdown() {
    let mut c = control();
    c.settings = None;
    sync(&mut c);
}

fn sync(c: &mut Control) {
    let wanted = supported()
        && c.settings
            .is_some_and(|s| s.on && (!s.only_playing || c.playing));
    match (wanted, c.running.is_some()) {
        (true, false) => {
            if let Some(s) = c.settings {
                c.running = start(Look {
                    coat: s.coat,
                    scale: s.scale,
                    collar: c.collar,
                });
            }
        }
        (false, true) => {
            if let Some(running) = c.running.take() {
                running.stop.store(true, Ordering::Relaxed);
                let _ = running.thread.join();
            }
        }
        (true, true) => {
            if let (Some(s), Some(running)) = (c.settings, &c.running) {
                *running.look.lock().unwrap_or_else(|p| p.into_inner()) = Look {
                    coat: s.coat,
                    scale: s.scale,
                    collar: c.collar,
                };
            }
        }
        (false, false) => {}
    }
}

#[cfg(any(windows, target_os = "linux"))]
fn start(look: Look) -> Option<Running> {
    let stop = Arc::new(AtomicBool::new(false));
    let shared_look = Arc::new(Mutex::new(look));
    let thread = {
        let stop = stop.clone();
        let shared_look = shared_look.clone();
        std::thread::Builder::new()
            .name("feline-desk-cat".into())
            .spawn(move || {
                #[cfg(windows)]
                live(&stop, &shared_look);
                #[cfg(target_os = "linux")]
                wayland::live(&stop, &shared_look);
            })
            .ok()?
    };
    Some(Running {
        stop,
        look: shared_look,
        thread,
    })
}

#[cfg(not(any(windows, target_os = "linux")))]
fn start(_look: Look) -> Option<Running> {
    None
}

/// The cat's thread: one step and one picture every tick until stopped.
#[cfg(windows)]
fn live(stop: &AtomicBool, shared_look: &Mutex<Look>) {
    use feline_core::cat::{Cat, FRAME, Pose, TICK, render};
    use feline_win::overlay::{Overlay, cursor_position, desktop_bounds};
    use std::collections::HashMap;

    let mut look = *shared_look.lock().unwrap_or_else(|p| p.into_inner());
    let Some(mut overlay) = Overlay::new(FRAME * look.scale) else {
        eprintln!("feline: the desk cat's window couldn't be made");
        return;
    };
    // It strolls in from a little below the pointer.
    let (px, py) = cursor_position().unwrap_or((400, 400));
    let mut cat = Cat::new(px as f32 - 160.0, py as f32 + 120.0);
    let mut frames: HashMap<Pose, feline_core::png::Rgba> = HashMap::new();
    let mut last: Option<(Pose, i32, i32)> = None;
    while !stop.load(Ordering::Relaxed) {
        overlay.pump();
        let now = *shared_look.lock().unwrap_or_else(|p| p.into_inner());
        if now != look {
            look = now;
            frames.clear();
            last = None;
        }
        let size = FRAME * look.scale;
        if let Some((x, y)) = cursor_position() {
            let pose = cat.step((x as f32, y as f32), size as f32);
            // Keep the cat on the desktop.
            let (left, top, width, height) = desktop_bounds();
            let half = size as f32 / 2.0;
            cat.x = cat.x.clamp(left as f32 + half, (left + width) as f32 - half);
            cat.y = cat.y.clamp(top as f32 + half, (top + height) as f32 - half);
            let at = ((cat.x - half) as i32, (cat.y - half) as i32);
            if last != Some((pose, at.0, at.1)) {
                let image = frames
                    .entry(pose)
                    .or_insert_with(|| render(pose, look.coat, look.collar, look.scale));
                overlay.draw(at.0, at.1, &image.pixels, size);
                last = Some((pose, at.0, at.1));
            }
        }
        std::thread::sleep(TICK);
    }
}

/// A picture of the sitting cat for the settings (any system).
pub fn preview(coat: Coat, collar: u32) -> slint::Image {
    let image = feline_core::cat::render(feline_core::cat::Pose::Sit, coat, collar, 4);
    let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
        &image.pixels,
        image.width,
        image.height,
    );
    slint::Image::from_rgba8(buffer)
}

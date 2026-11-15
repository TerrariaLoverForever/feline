// SPDX-License-Identifier: GPL-3.0-or-later
//! Background music (About → Music): the player, which lives on its own
//! thread, and the Music tab's options.
//!
//! The player is told what's wanted (tracks, volume, whether it should be
//! heard) and works towards it, fading in and out instead of cutting off.
//! "Only while Feline is focused" asks winit about the window a few times a
//! second (see [`watch_focus`]).

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use feline_core::music;
use feline_core::settings::{Music, Settings};
use slint::winit_030::WinitWindowAccessor as _;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::gui::{AppWindow, Extras, Shared, lock};

/// What the player should be doing.
#[derive(Debug, Clone, PartialEq)]
struct Wanted {
    /// Played in turn, looping; empty = silence.
    tracks: Vec<PathBuf>,
    /// 0.0–1.0.
    volume: f32,
}

enum Command {
    Want(Wanted),
    Audible(bool),
    Quit,
}

struct Control {
    sender: Option<Sender<Command>>,
    thread: Option<std::thread::JoinHandle<()>>,
    settings: Music,
    focused: bool,
}

fn control() -> MutexGuard<'static, Control> {
    static CONTROL: OnceLock<Mutex<Control>> = OnceLock::new();
    CONTROL
        .get_or_init(|| {
            Mutex::new(Control {
                sender: None,
                thread: None,
                settings: Music::default(),
                focused: true,
            })
        })
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn send(c: &mut Control, command: Command) {
    if c.sender.is_none() {
        let (tx, rx) = mpsc::channel();
        c.thread = std::thread::Builder::new()
            .name("feline-music".into())
            .spawn(move || player(rx))
            .ok();
        c.sender = Some(tx);
    }
    if let Some(tx) = &c.sender {
        let _ = tx.send(command);
    }
}

fn audible(c: &Control) -> bool {
    c.settings.on && (c.focused || !c.settings.only_focused)
}

/// Applies the Music settings (on/off, track, volume).
pub fn configure(settings: &Music) {
    let mut c = control();
    c.settings = settings.clone();
    if !settings.on && c.sender.is_none() {
        return; // Never started: nothing to stop.
    }
    let tracks = match music::dir() {
        Some(dir) if settings.on => {
            let names = if settings.track.is_empty() {
                music::list(&dir)
            } else {
                vec![settings.track.clone()]
            };
            names.into_iter().map(|n| dir.join(n)).filter(|p| p.is_file()).collect()
        }
        _ => Vec::new(),
    };
    let want = Wanted {
        tracks,
        volume: settings.volume.min(100) as f32 / 100.0,
    };
    let audible = audible(&c);
    send(&mut c, Command::Want(want));
    send(&mut c, Command::Audible(audible));
}

/// Feline's window gained or lost the focus.
fn set_focused(focused: bool) {
    let mut c = control();
    if c.focused == focused {
        return;
    }
    c.focused = focused;
    if c.sender.is_some() {
        let audible = audible(&c);
        send(&mut c, Command::Audible(audible));
    }
}

/// Feline is closing: the music stops (with a short fade).
pub fn shutdown() {
    let (sender, thread) = {
        let mut c = control();
        (c.sender.take(), c.thread.take())
    };
    if let Some(tx) = sender {
        let _ = tx.send(Command::Quit);
        if let Some(thread) = thread {
            let _ = thread.join();
        }
    }
}

/// Keeps the player told whether Feline's window has the focus. The timer
/// must live as long as the window (keep what this returns).
pub fn watch_focus(ui: &AppWindow) -> slint::Timer {
    let timer = slint::Timer::default();
    let weak = ui.as_weak();
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(300), move || {
        let Some(ui) = weak.upgrade() else { return };
        // Not a winit window (shouldn't happen): count it as focused, so
        // the music isn't silent forever.
        let focused = ui.window().with_winit_window(|w| w.has_focus()).unwrap_or(true);
        let minimized = ui.window().is_minimized();
        set_focused(focused && !minimized);
    });
    timer
}

/// The player thread: owns the audio device while music is wanted.
fn player(rx: Receiver<Command>) {
    use rodio::{Decoder, DeviceSinkBuilder, Player};

    const TICK: Duration = Duration::from_millis(40);
    // Full volume in about half a second.
    const FADE_STEP: f32 = 0.08;

    let mut want = Wanted {
        tracks: Vec::new(),
        volume: 0.0,
    };
    let mut audible = false;
    // The device and what's playing on it; opened when first needed.
    let mut sink: Option<(rodio::MixerDeviceSink, Player)> = None;
    let mut next = 0usize;
    // How loud it is now (0..1 of the wanted volume), for fading.
    let mut level = 0.0f32;
    let mut quitting = false;
    let mut failures = 0usize;
    // Silent with nothing to fade or play: wait for the next command
    // instead of waking 25 times a second.
    let mut resting = false;

    loop {
        let received = if resting {
            rx.recv().map_err(|_| RecvTimeoutError::Disconnected)
        } else {
            rx.recv_timeout(TICK)
        };
        resting = false;
        match received {
            Ok(Command::Want(w)) => {
                if w.tracks != want.tracks {
                    if let Some((_, p)) = &sink {
                        p.stop();
                    }
                    next = 0;
                    failures = 0;
                }
                want = w;
            }
            Ok(Command::Audible(a)) => audible = a,
            Ok(Command::Quit) | Err(RecvTimeoutError::Disconnected) => {
                quitting = true;
                audible = false;
            }
            Err(RecvTimeoutError::Timeout) => {}
        }

        let should_play = audible && !want.tracks.is_empty();
        level = if should_play {
            (level + FADE_STEP).min(1.0)
        } else {
            (level - FADE_STEP).max(0.0)
        };

        if level == 0.0 && !should_play {
            if quitting || want.tracks.is_empty() {
                // Silent and nothing to play: let go of the audio device.
                sink = None;
                if quitting {
                    return;
                }
            } else if let Some((_, p)) = &sink {
                p.pause();
            }
            resting = true;
            continue;
        }

        if sink.is_none() {
            match DeviceSinkBuilder::open_default_sink() {
                Ok(mut device) => {
                    device.log_on_drop(false);
                    let p = Player::connect_new(device.mixer());
                    sink = Some((device, p));
                }
                Err(e) => {
                    eprintln!("feline: no audio output for the music ({e})");
                    // Try again in a while rather than every tick.
                    std::thread::sleep(Duration::from_secs(5));
                    continue;
                }
            }
        }
        let Some((_, p)) = &sink else { continue };

        if p.empty() && should_play {
            if failures >= want.tracks.len() {
                // Nothing here will play; wait for new settings.
                continue;
            }
            let path = &want.tracks[next % want.tracks.len()];
            next = (next + 1) % want.tracks.len();
            let decoded = std::fs::File::open(path)
                .map_err(|e| e.to_string())
                .and_then(|f| Decoder::try_from(f).map_err(|e| e.to_string()));
            match decoded {
                Ok(source) => {
                    failures = 0;
                    p.append(source);
                }
                Err(e) => {
                    failures += 1;
                    eprintln!("feline: can't play {} ({e})", path.display());
                }
            }
        }
        // A gentle curve: equal steps sound equally loud.
        p.set_volume(want.volume * want.volume * level);
        if p.is_paused() {
            p.play();
        }
    }
}

// ---------------------------------------------------------------------------
// The Music tab
// ---------------------------------------------------------------------------

/// Shows the Music options and the track list, and applies them.
pub fn refresh(ui: &AppWindow, settings: &Settings) {
    let m = &settings.music;
    let extras = ui.global::<Extras>();
    let names = music::dir().map(|d| music::list(&d)).unwrap_or_default();
    let mut choices: Vec<SharedString> = vec!["All my tracks, one after another".into()];
    choices.extend(names.iter().map(|n| SharedString::from(music::title(n))));
    let index = if m.track.is_empty() {
        0
    } else {
        names.iter().position(|n| *n == m.track).map_or(0, |i| i + 1)
    };
    extras.set_music_on(m.on);
    extras.set_music_tracks(ModelRc::new(VecModel::from(choices)));
    extras.set_music_track_count(names.len() as i32);
    extras.set_music_track_index(index as i32);
    extras.set_music_volume(m.volume.min(100) as f32);
    extras.set_music_when_index(i32::from(!m.only_focused));
    configure(m);
}

/// Changes one option ("music", "music-track", "music-volume",
/// "music-when"). Returns false for anything else.
pub fn set(shared: &Shared, which: &str, value: i32) -> bool {
    let mut state = lock(shared);
    let m = &mut state.settings.music;
    match which {
        "music" => m.on = value != 0,
        "music-volume" => m.volume = value.clamp(0, 100) as u32,
        "music-when" => m.only_focused = value == 0,
        "music-track" => {
            let names = music::dir().map(|d| music::list(&d)).unwrap_or_default();
            m.track = match value {
                i if i <= 0 => String::new(),
                i => names.get(i as usize - 1).cloned().unwrap_or_default(),
            };
        }
        _ => return false,
    }
    true
}

/// "Add a file…": copies an audio file into the music folder.
pub fn add_file(ui: &AppWindow, shared: &Shared) {
    let patterns: Vec<String> = music::EXTENSIONS.iter().map(|e| format!("*.{e}")).collect();
    let patterns: Vec<&str> = patterns.iter().map(String::as_str).collect();
    let Some(path) = crate::picker::pick_file("Add music", "Audio files", &patterns) else {
        return;
    };
    let message = match music::import_file(&path) {
        Ok(name) => {
            choose(shared, &name);
            format!("Added \"{}\".", music::title(&name))
        }
        Err(e) => format!("Couldn't add it: {e}"),
    };
    finish(ui, shared, message);
}

/// "Fetch": downloads the song at a link on a background thread.
pub fn add_link(ui: &AppWindow, shared: &Shared, link: &str) {
    if let Err(e) = music::classify(link) {
        ui.global::<Extras>().set_music_message(e.into());
        return;
    }
    let extras = ui.global::<Extras>();
    extras.set_music_busy(true);
    extras.set_music_message("Fetching… a song usually takes under a minute.".into());
    let link = link.to_owned();
    let weak = ui.as_weak();
    let shared = shared.clone();
    let _ = std::thread::Builder::new().name("feline-music-fetch".into()).spawn(move || {
        let result = music::import_link(&link);
        let _ = weak.upgrade_in_event_loop(move |ui| {
            ui.global::<Extras>().set_music_busy(false);
            let message = match result {
                Ok(name) => {
                    choose(&shared, &name);
                    ui.global::<Extras>().set_music_link(SharedString::new());
                    format!("Added \"{}\".", music::title(&name))
                }
                Err(e) => e,
            };
            finish(&ui, &shared, message);
        });
    });
}

/// "Remove": deletes the chosen track from the music folder.
pub fn remove(ui: &AppWindow, shared: &Shared) {
    let name = std::mem::take(&mut lock(shared).settings.music.track);
    if name.is_empty() {
        return;
    }
    let message = match music::remove(&name) {
        Ok(()) => format!("Removed \"{}\".", music::title(&name)),
        Err(e) => format!("Couldn't remove it ({e})."),
    };
    finish(ui, shared, message);
}

/// Opens the music folder (to drop files in by hand).
pub fn open_folder() {
    if let Some(dir) = music::dir() {
        let _ = std::fs::create_dir_all(&dir);
        let _ = crate::picker::open_with_default_app(&dir);
    }
}

/// A new track becomes the one playing, and the music comes on.
fn choose(shared: &Shared, name: &str) {
    let mut state = lock(shared);
    state.settings.music.track = name.to_owned();
    state.settings.music.on = true;
}

fn finish(ui: &AppWindow, shared: &Shared, message: String) {
    ui.global::<Extras>().set_music_message(message.into());
    let settings = lock(shared).settings.clone();
    if let Some(path) = Settings::path()
        && let Err(e) = settings.save_to(&path)
    {
        eprintln!("feline: couldn't save the settings: {e}");
    }
    refresh(ui, &settings);
}

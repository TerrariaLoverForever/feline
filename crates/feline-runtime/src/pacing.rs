// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! Frame pacing at presentation.
//!
//! With an FPS limit and `framePacing` on, Feline holds each frame until
//! its turn, just before it's presented (Vulkan's vkQueuePresentKHR,
//! OpenGL's eglSwapBuffers), and the engine's own limiter is opened up.
//! Frames then leave at an even rhythm instead of the engine's coarser
//! sleeps, which feels smoother at the same frame rate. A sleep to just
//! before the frame's turn and a very short spin get the timing exact
//! without keeping a CPU core busy.
//!
//! `throttleUnfocused`: 30 FPS while the window isn't focused, to save
//! power (Roblox's own Android code would do that for a phone app in the
//! background, which is why Feline doesn't pass focus loss on to it).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static FOCUSED: AtomicBool = AtomicBool::new(true);

/// The window gained or lost the focus.
pub fn set_focused(focused: bool) {
    FOCUSED.store(focused, Ordering::Relaxed);
}

/// Whether the window has the focus.
pub fn focused() -> bool {
    FOCUSED.load(Ordering::Relaxed)
}

/// Frames per second while unfocused, with `throttleUnfocused`.
const UNFOCUSED_FPS: u32 = 30;

/// Whether Feline paces frames itself (and the engine's limit is opened).
/// `FELINE_PACING=0` turns it off whatever the setting (for comparing).
pub fn paces(prefs: &crate::prefs::Prefs) -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let off = *OFF.get_or_init(|| std::env::var("FELINE_PACING").is_ok_and(|v| v == "0"));
    !off && prefs.frame_pacing && prefs.fps_limit > 0 && prefs.fps_limit < crate::prefs::MAX_FPS_LIMIT
}

/// The frame rate to hold right now, if any.
fn target(prefs: &crate::prefs::Prefs) -> Option<u32> {
    let paced = paces(prefs).then_some(prefs.fps_limit);
    if prefs.throttle_unfocused && !FOCUSED.load(Ordering::Relaxed) {
        return Some(paced.map_or(UNFOCUSED_FPS, |f| f.min(UNFOCUSED_FPS)));
    }
    paced
}

/// When the next frame may be presented.
static NEXT: Mutex<Option<Instant>> = Mutex::new(None);

/// Called just before presenting a frame: waits for its turn.
pub fn before_present() {
    let prefs = crate::prefs::get();
    let mut next = NEXT.lock().unwrap_or_else(|p| p.into_inner());
    let Some(fps) = target(prefs) else {
        *next = None;
        return;
    };
    let interval = Duration::from_secs_f64(1.0 / f64::from(fps.max(1)));
    if let Some(at) = *next {
        wait_until(at);
    }
    let now = Instant::now();
    // On time: the next slot follows this one. Late by more than a frame
    // (a hitch): start over from now instead of rushing to catch up.
    *next = Some(match *next {
        Some(at) if now.saturating_duration_since(at) < interval => at + interval,
        _ => now + interval,
    });
}

/// How long before a frame's turn the sleep ends and the spin begins.
/// Linux wakes a sleeping thread within about 60 microseconds (its default
/// timer slack is 50); spinning for a whole millisecond, as this did, kept
/// a core a quarter busy at 240 FPS for no gain in accuracy.
const SPIN: Duration = Duration::from_micros(150);

fn wait_until(at: Instant) {
    loop {
        let now = Instant::now();
        if now >= at {
            return;
        }
        let left = at - now;
        if left > SPIN {
            std::thread::sleep(left - SPIN);
        } else {
            std::hint::spin_loop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waiting_ends_on_time() {
        // A 4 ms wait (a frame at 240 FPS) ends at its time, not early, and
        // not late by more than a busy machine's scheduling.
        let at = Instant::now() + Duration::from_millis(4);
        wait_until(at);
        let late = Instant::now().duration_since(at);
        assert!(late < Duration::from_millis(3), "woke {late:?} late");
        wait_until(Instant::now() - Duration::from_millis(1)); // already past: returns at once
    }

    #[test]
    fn pacing_needs_a_limit() {
        let mut p = crate::prefs::Prefs { frame_pacing: true, ..Default::default() };
        assert!(!paces(&p), "no limit: nothing to pace");
        p.fps_limit = 144;
        assert!(paces(&p));
        p.fps_limit = crate::prefs::MAX_FPS_LIMIT;
        assert!(!paces(&p), "\"no limit\" isn't a pace");
    }

    #[test]
    fn unfocused_throttle() {
        let mut p = crate::prefs::Prefs { throttle_unfocused: true, ..Default::default() };
        set_focused(false);
        assert_eq!(target(&p), Some(UNFOCUSED_FPS));
        p.frame_pacing = true;
        p.fps_limit = 20;
        assert_eq!(target(&p), Some(20), "a lower limit stays");
        set_focused(true);
        assert_eq!(target(&p), Some(20));
        p.throttle_unfocused = false;
        p.fps_limit = 0;
        assert_eq!(target(&p), None);
    }
}

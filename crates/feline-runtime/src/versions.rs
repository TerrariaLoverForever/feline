// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! Marks for the launcher's automatic Roblox updates (feline-core's
//! `engine` module): each logged-in start of a version Feline installed
//! adds to its `starts` file, and reaching Home writes `ok` (a version that
//! shows Home and then logs out first writes nothing). A new version that
//! was started twice without an `ok` is replaced by the last one that
//! worked. Logged-out starts aren't counted (they never reach Home), and
//! engines copied by hand (not in a `versions/` folder) get no marks.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static DIR: OnceLock<PathBuf> = OnceLock::new();

/// The engine `--play` runs: records the start, if it's a Feline-installed
/// version (…/engine/versions/<version>/libroblox.so).
pub fn started(engine: &Path) {
    let Ok(real) = engine.canonicalize() else {
        return;
    };
    let Some(dir) = real.parent() else {
        return;
    };
    if dir.parent().and_then(Path::file_name).is_none_or(|n| n != "versions") {
        return;
    }
    let _ = DIR.set(dir.to_path_buf());
}

/// The saved login was accepted: this start counts.
pub fn logged_in() {
    if let Some(dir) = DIR.get()
        && let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("starts"))
    {
        let _ = file.write_all(b"x");
    }
}

static HOME_AT: OnceLock<std::time::Instant> = OnceLock::new();

/// Roblox's app showed `screen`. Home, still there 20 s later (a version
/// that loses the login drops to Landing within seconds): it works.
pub fn app_shown(screen: &str) {
    if screen == "Home" {
        let _ = HOME_AT.set(std::time::Instant::now());
    }
    if screen == "Landing" {
        // Logged out after Home: not a working version.
        LOGGED_OUT.store(true, std::sync::atomic::Ordering::Relaxed);
        // FELINE_EXIT_ON_LOGOUT=1 (for testing new versions): stop at once,
        // before the app ends the saved session on Roblox's servers.
        if HOME_AT.get().is_some() && std::env::var_os("FELINE_EXIT_ON_LOGOUT").is_some() {
            eprintln!("feline-runtime: the app lost the login; exiting before it logs out");
            crate::init::exit_now(3);
        }
    }
}

static LOGGED_OUT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// From the window loop: marks the version good once Home has stayed up.
pub fn tick() {
    let Some(at) = HOME_AT.get() else {
        return;
    };
    static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if DONE.load(std::sync::atomic::Ordering::Relaxed)
        || LOGGED_OUT.load(std::sync::atomic::Ordering::Relaxed)
        || at.elapsed().as_secs() < 20
    {
        return;
    }
    DONE.store(true, std::sync::atomic::Ordering::Relaxed);
    if let Some(dir) = DIR.get() {
        let ok = dir.join("ok");
        if !ok.exists() {
            let _ = std::fs::write(&ok, "");
            eprintln!("feline-runtime: this Roblox version works; marked as good");
        }
    }
}

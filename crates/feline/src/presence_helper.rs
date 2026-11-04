// SPDX-License-Identifier: GPL-3.0-or-later
//! Starting Feline's Discord status helper (`feline --presence`) next to a
//! game. It runs as its own process, so the status keeps going after the
//! Feline window closes, and it exits by itself when the game ends.

use std::process::{Command, Stdio};

use feline_core::presence::service;

/// Starts the helper, unless one is already running.
pub fn start() -> Result<(), String> {
    if service::running_service().is_some() {
        return Ok(());
    }
    let exe =
        std::env::current_exe().map_err(|e| format!("couldn't find the Feline program: {e}"))?;
    let mut helper = Command::new(exe);
    helper.arg("--presence");
    // Its own session, like the game, so closing the Feline window or the
    // terminal it was started from doesn't take the helper with it.
    let mut command = feline_core::launch::detached(helper);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
        .spawn()
        .map(drop)
        .map_err(|e| format!("couldn't start Feline's Discord status helper: {e}"))
}

/// The helper's current status line, if it's running.
pub fn status() -> Option<String> {
    service::running_service()?;
    let text = std::fs::read_to_string(service::status_path()?).ok()?;
    Some(text.trim().to_owned())
}

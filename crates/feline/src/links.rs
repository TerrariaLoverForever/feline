// SPDX-License-Identifier: GPL-3.0-or-later
//! Opening `roblox://` links from the Roblox website with Feline.
//!
//! Off by default. Switching it on (Settings page) makes Feline the default
//! handler for `roblox://` and `roblox-player:` links through `xdg-mime`,
//! after remembering which app handled them before; switching it off gives
//! them back to that app. A link then arrives as `feline <link>`: Feline
//! starts the chosen runtime with it (and the Discord status helper, if the
//! status is on) and exits, without opening its window.

use std::path::Path;
use std::process::{Command, ExitCode, Stdio};

use feline_core::launch;
use feline_core::link;
use feline_core::paths;
use feline_core::runtime::{DetectContext, LaunchTarget, detect_all};
use feline_core::settings::Settings;

use crate::desktop::{self, LINK_ENTRY};

const SCHEMES: [&str; 2] = ["x-scheme-handler/roblox", "x-scheme-handler/roblox-player"];

/// True for the arguments a browser passes when opening a Roblox link.
pub fn is_link(arg: &str) -> bool {
    let lower = arg.to_ascii_lowercase();
    lower.starts_with("roblox://") || lower.starts_with("roblox-player:")
}

/// The desktop entry that currently handles `roblox://` links.
fn current_handler() -> Option<String> {
    let out = Command::new("xdg-mime")
        .args(["query", "default", SCHEMES[0]])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let name = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (!name.is_empty()).then_some(name)
}

fn set_default(entry: &str) -> Result<(), String> {
    for scheme in SCHEMES {
        let status = Command::new("xdg-mime")
            .args(["default", entry, scheme])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|e| format!("couldn't run xdg-mime ({e}); is xdg-utils installed?"))?;
        if !status.success() {
            return Err(format!("xdg-mime couldn't set {entry} for {scheme}"));
        }
    }
    Ok(())
}

/// Makes Feline the handler, remembering the previous one in `settings`.
pub fn enable(settings: &mut Settings) -> Result<(), String> {
    let previous = current_handler().filter(|h| h != LINK_ENTRY);
    desktop::install_link_entry()?;
    set_default(LINK_ENTRY)?;
    if previous.is_some() {
        settings.previous_link_handler = previous;
    }
    Ok(())
}

/// Gives the links back to the previous handler (if there was one).
pub fn disable(settings: &mut Settings) -> Result<(), String> {
    if let Some(previous) = settings.previous_link_handler.take() {
        set_default(&previous)?;
    }
    desktop::uninstall_link_entry()
}

/// `feline <roblox-link>`: starts the chosen runtime with the link. Opens
/// Feline's window instead if no runtime has been chosen yet.
pub fn open(arg: &str) -> ExitCode {
    let settings = Settings::path()
        .and_then(|p| Settings::load_from(&p).ok())
        .unwrap_or_default();
    let found = detect_all(&DetectContext::host());
    let chosen = settings
        .runtime
        .as_deref()
        .and_then(|key| found.iter().find(|i| i.key() == key));
    let Some(installation) = chosen else {
        eprintln!("feline: no runtime chosen yet; opening Feline so you can pick one");
        return match crate::gui::run() {
            Ok(()) => ExitCode::SUCCESS,
            Err(_) => ExitCode::FAILURE,
        };
    };
    let game = match link::parse(arg) {
        Ok(game) => game,
        Err(e) => {
            eprintln!("feline: {e}");
            return ExitCode::from(2);
        }
    };

    let ctx = DetectContext::host();
    if let Err(e) = installation.apply_overrides(&ctx, &settings.overrides) {
        eprintln!("feline: Feline's settings weren't applied: {e}");
    }
    let plan = match installation.plan_launch(&LaunchTarget::Game(game)) {
        Ok(plan) => plan,
        Err(e) => {
            eprintln!(
                "feline: {} couldn't be prepared: {e}",
                installation.backend.name()
            );
            return ExitCode::FAILURE;
        }
    };
    if settings.overrides.discord_presence
        && let Err(e) = crate::presence_helper::start()
    {
        eprintln!("feline: {e}");
    }
    let log_dir = paths::log_dir().unwrap_or_else(|| Path::new("/tmp").to_owned());
    // The runtime runs in its own process group; Feline exits right away and
    // the game keeps going.
    match launch::spawn(plan.command, &log_dir, installation.backend.id()) {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!(
                "feline: {} couldn't be started: {e}",
                installation.backend.name()
            );
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_browser_links() {
        assert!(is_link("roblox://experiences/start?placeId=1"));
        assert!(is_link("roblox-player:1+launchmode:play"));
        assert!(is_link("ROBLOX://x"));
        assert!(!is_link("--info"));
        assert!(!is_link("https://www.roblox.com/games/1"));
    }
}

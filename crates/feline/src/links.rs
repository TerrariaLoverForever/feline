// SPDX-License-Identifier: GPL-3.0-or-later
//! Opening `roblox://` links from the Roblox website with Feline.
//!
//! Off by default. Switching it on (Settings page) makes Feline the default
//! handler for `roblox://` and `roblox-player:` links through `xdg-mime`,
//! after remembering which app handled them before; switching it off gives
//! them back to that app. A link then arrives as `feline <link>`: Feline
//! starts the runtime picked for links (Settings; by default the one in
//! use) with it, and the Discord status helper if the status is on, and
//! exits without opening its window.

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
    if cfg!(windows) {
        return Err("opening Roblox links with Feline isn't available on Windows yet".to_owned());
    }
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
    if cfg!(windows) {
        return Ok(());
    }
    if let Some(previous) = settings.previous_link_handler.take() {
        set_default(&previous)?;
    }
    desktop::uninstall_link_entry()
}

/// The card shown while Roblox starts from a link or shortcut (Feline has no
/// window then), so problems aren't lost in a hidden console.
struct Splash {
    on: bool,
    shown: Option<std::time::Instant>,
}

impl Splash {
    const SECONDS: u64 = 4;

    fn show(&mut self, title: &str, text: &str) {
        if !self.on {
            return;
        }
        #[cfg(windows)]
        {
            feline_win::overlay::card(title, text, Self::SECONDS);
            self.shown = Some(std::time::Instant::now());
        }
        #[cfg(not(windows))]
        launch::notify(title, text);
    }

    /// A problem: printed, and on the card for longer.
    fn fail(&mut self, text: &str) -> ExitCode {
        eprintln!("feline: {text}");
        if self.on {
            self.wait();
            self.show("Roblox didn't start", text);
            self.wait();
        }
        ExitCode::FAILURE
    }

    /// Keeps Feline (and so the card) around until the card is done.
    fn wait(&mut self) {
        if let Some(shown) = self.shown.take() {
            let total = std::time::Duration::from_millis(Self::SECONDS * 1000 + 700);
            std::thread::sleep(total.saturating_sub(shown.elapsed()));
        }
    }
}

/// `feline <roblox-link>`: starts the chosen runtime with the link. Opens
/// Feline's window instead if no runtime has been chosen yet.
pub fn open(arg: &str) -> ExitCode {
    let settings = Settings::path()
        .and_then(|p| Settings::load_from(&p).ok())
        .unwrap_or_default();
    let found = detect_all(&DetectContext::host());
    let find = |key: Option<&str>| key.and_then(|key| found.iter().find(|i| i.key() == key));
    // The runtime picked for links, else (or if it's gone) the one in use.
    let chosen = find(settings.link_runtime.as_deref()).or_else(|| find(settings.runtime.as_deref()));
    let Some(installation) = chosen else {
        eprintln!("feline: no runtime chosen yet; opening Feline so you can pick one");
        return match crate::gui::run() {
            Ok(()) => ExitCode::SUCCESS,
            Err(_) => ExitCode::FAILURE,
        };
    };
    let mut splash = Splash { on: settings.launch_splash, shown: None };
    let game = match link::parse(arg) {
        Ok(game) => game,
        Err(e) => {
            let _ = splash.fail(&format!("That link didn't make sense to Feline: {e}"));
            return ExitCode::from(2);
        }
    };
    let name = game
        .place_id()
        .and_then(|place| {
            let history = feline_core::history::History::load();
            let games = history.games_played();
            games.iter().find(|g| g.any_place_in(&[place])).map(|g| history.name_of(g))
        })
        .unwrap_or_else(|| "the game".to_owned());
    splash.show("Getting Roblox ready", &format!("Pouncing into {name}…"));

    let ctx = DetectContext::host();
    // The kept Roblox version (if you stay on one); room for more Robloxes.
    let (installation, lines) = crate::companion::before(installation, &settings);
    let installation = &installation;
    lines.iter().for_each(|l| eprintln!("feline: {l}"));
    let hide_cursor = crate::companion::hide_cursor(installation, &settings);
    if let Err(e) = installation.apply_overrides(&ctx, &settings.overrides) {
        eprintln!("feline: Feline's settings weren't applied: {e}");
    }
    // Roblox's own client (Windows): is it really Roblox's? Which GPU?
    match feline_core::runtime::roblox::prepare(installation, &settings) {
        Ok(warnings) => warnings.iter().for_each(|w| eprintln!("feline: {w}")),
        Err(why) => return splash.fail(&why),
    }
    for line in feline_core::mods::apply_for_launch(installation, &ctx, &settings.player_mods(), hide_cursor) {
        eprintln!("feline: {line}");
    }
    // Tweaks (FastFlags), as for Play: this game's profile, or the everyday ones.
    for line in feline_core::fflags::apply_for_launch(installation, &ctx, &settings, game.place_id()) {
        eprintln!("feline: {line}");
    }
    if let Some(line) =
        feline_core::roblox_tools::apply_game_profile(installation, &ctx, &settings, game.place_id())
    {
        eprintln!("feline: {line}");
    }
    let plan = match installation.plan_launch(&LaunchTarget::Game(game)) {
        Ok(plan) => plan,
        Err(e) => return splash.fail(&format!("{} couldn't be prepared: {e}", installation.backend.name())),
    };
    if settings.overrides.discord_presence
        && let Err(e) = crate::presence_helper::start()
    {
        eprintln!("feline: {e}");
    }
    let log_dir = paths::log_dir().unwrap_or_else(|| Path::new("/tmp").to_owned());
    // The runtime runs in its own process group; Feline exits right away and
    // the game keeps going.
    let command = if cfg!(windows) { plan.command } else { launch::wrap(plan.command, &settings.runtime_wrapper) };
    match launch::spawn(command, &log_dir, installation.backend.id()) {
        Ok(game) => {
            if let Some(w) = feline_core::runtime::roblox::after_start(game.pid(), &settings) {
                eprintln!("feline: {w}");
            }
            if let Some(w) = crate::companion::after(game.pid(), hide_cursor) {
                eprintln!("feline: {w}");
            }
            splash.wait();
            ExitCode::SUCCESS
        }
        Err(e) => splash.fail(&format!("{} couldn't be started: {e}", installation.backend.name())),
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

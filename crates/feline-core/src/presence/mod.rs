// SPDX-License-Identifier: GPL-3.0-or-later
//! Feline's own Discord status ("Rich Presence").
//!
//! While a game Feline started is running, a background thread:
//! 1. follows the Roblox client's log ([`log`]) to see whether you're in the
//!    menus or in a game, and which one;
//! 2. looks up the game's name, creator and icon (and, if you allow it, your
//!    username and avatar) from Roblox's public web API ([`roblox`]);
//! 3. shows that as your Discord status ([`discord`]).
//!
//! It works the same for every runtime, because it only needs the log. Feline
//! has to stay open for the status to stay up; Discord clears it when Feline
//! exits.

pub mod discord;
pub mod log;
pub mod roblox;
pub mod service;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Value, json};

use crate::settings::{DiscordExtras, PresenceOptions};
use discord::Discord;
use log::{LogEvent, LogFollower};
use roblox::{GameInfo, UserInfo};

/// How often the log is read.
const POLL: Duration = Duration::from_secs(1);
/// How long to wait before trying to reach Discord again.
const RECONNECT: Duration = Duration::from_secs(15);

/// What the player is doing, as far as the log tells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Activity {
    /// Nothing seen yet (the client is starting).
    Starting,
    Menu,
    Game {
        place_id: u64,
        job_id: String,
        universe_id: Option<u64>,
        user_id: Option<u64>,
        /// Unix time the player joined, for Discord's "elapsed" timer.
        since: Option<u64>,
        /// The game server's address (for its region).
        server: Option<String>,
    },
}

impl Activity {
    /// The activity after `event`.
    pub fn apply(&self, event: LogEvent) -> Self {
        match event {
            LogEvent::Menu | LogEvent::Left => Self::Menu,
            LogEvent::GameMessage(_) | LogEvent::Disconnected(_) | LogEvent::ServerStarted(_) => self.clone(),
            // The server's public address, for its region.
            LogEvent::Server(ip) => match self {
                Self::Game { .. } => {
                    let mut next = self.clone();
                    if let Self::Game { server, .. } = &mut next {
                        *server = Some(ip);
                    }
                    next
                }
                other => other.clone(),
            },
            LogEvent::Joining {
                place_id,
                job_id,
                at,
                server,
            } => Self::Game {
                place_id,
                job_id,
                universe_id: None,
                user_id: None,
                since: at,
                server,
            },
            LogEvent::Joined {
                place_id,
                universe_id,
                user_id,
            } => match self {
                // Keep the server and the start time from "Joining".
                Self::Game {
                    place_id: p,
                    job_id,
                    since,
                    server,
                    ..
                } if *p == place_id => Self::Game {
                    place_id,
                    job_id: job_id.clone(),
                    universe_id: Some(universe_id),
                    user_id: Some(user_id),
                    since: *since,
                    server: server.clone(),
                },
                _ => Self::Game {
                    place_id,
                    job_id: String::new(),
                    universe_id: Some(universe_id),
                    user_id: Some(user_id),
                    since: None,
                    server: None,
                },
            },
        }
    }
}

/// How Feline names itself in the status (image hover texts, menus line).
pub const FELINE_LABEL: &str = "🐱 Feline 🐾";

/// Extra facts for the status, worked out while the game runs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Extra {
    /// The user's extras settings.
    pub settings: DiscordExtras,
    /// Where the server is, e.g. "Frankfurt am Main, Germany".
    pub region: Option<String>,
    /// The runtime's name, shown on Linux ("on Sober").
    pub runtime: Option<String>,
    /// Playtime today, e.g. "1 h 20 min".
    pub today: Option<String>,
    /// The games (universes) of the hidden places, so a hidden game stays
    /// secret in its other places too (sub-places, private servers).
    pub hidden_universes: Vec<u64>,
    /// Games may set the status themselves (Bloxstrap's messages).
    pub game_status_allowed: bool,
    /// What the game set, if anything (reset when you change games).
    pub game_status: GameStatus,
}

/// A game's own Discord status, from Bloxstrap's `SetRichPresence` message
/// (`{"command": "SetRichPresence", "data": {"details": …, "state": …,
/// "timeStart": …, "timeEnd": …, "largeImage": {"assetId": …, "hoverText":
/// …, "clear": …, "reset": …}, "smallImage": …}}`). Each message changes
/// only what it names; `"<reset>"` (text) or `"reset": true` (images) puts
/// Feline's own back, `0` clears a time, `"clear": true` removes an image.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GameStatus {
    pub details: Option<String>,
    pub state: Option<String>,
    pub start: Option<u64>,
    pub end: Option<u64>,
    pub large: Option<GameImage>,
    pub small: Option<GameImage>,
}

/// An image the game chose: a picture (its URL, looked up from the Roblox
/// asset), or none at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameImage {
    Picture { url: String, text: Option<String> },
    Cleared,
}

impl GameStatus {
    /// Applies one message. `asset_url` turns a Roblox asset ID into a
    /// picture URL. Returns whether anything changed.
    pub fn apply(&mut self, message: &Value, asset_url: &dyn Fn(u64) -> Option<String>) -> bool {
        if message.get("command").and_then(Value::as_str) != Some("SetRichPresence") {
            return false;
        }
        let Some(data) = message.get("data") else { return false };
        let before = self.clone();
        let text = |key: &str, slot: &mut Option<String>| match data.get(key) {
            Some(Value::String(t)) if t == "<reset>" => *slot = None,
            Some(Value::String(t)) => *slot = Some(clamp(t)),
            _ => {}
        };
        text("details", &mut self.details);
        text("state", &mut self.state);
        let time = |key: &str, slot: &mut Option<u64>| {
            if let Some(n) = data.get(key).and_then(Value::as_u64) {
                *slot = (n > 0).then_some(n);
            }
        };
        time("timeStart", &mut self.start);
        time("timeEnd", &mut self.end);
        let image = |key: &str, slot: &mut Option<GameImage>| {
            let Some(i) = data.get(key).filter(|v| v.is_object()) else { return };
            if i.get("reset").and_then(Value::as_bool) == Some(true) {
                *slot = None;
            } else if i.get("clear").and_then(Value::as_bool) == Some(true) {
                *slot = Some(GameImage::Cleared);
            } else if let Some(url) = i.get("assetId").and_then(Value::as_u64).and_then(asset_url) {
                let text = i.get("hoverText").and_then(Value::as_str).map(clamp);
                *slot = Some(GameImage::Picture { url, text });
            }
        };
        image("largeImage", &mut self.large);
        image("smallImage", &mut self.small);
        *self != before
    }

    /// Puts the game's choices over Feline's activity.
    fn over(&self, value: &mut Value) {
        if let Some(d) = &self.details {
            value["details"] = Value::from(d.as_str());
        }
        if let Some(s) = &self.state {
            value["state"] = Value::from(s.as_str());
        }
        if self.start.is_some() || self.end.is_some() {
            let mut t = json!({});
            if let Some(start) = self.start {
                t["start"] = Value::from(start);
            }
            if let Some(end) = self.end {
                t["end"] = Value::from(end);
            }
            value["timestamps"] = t;
        }
        for (image, key, text_key) in [(&self.large, "large_image", "large_text"), (&self.small, "small_image", "small_text")] {
            match image {
                Some(GameImage::Picture { url, text }) => {
                    value["assets"][key] = Value::from(url.as_str());
                    if let Some(text) = text {
                        value["assets"][text_key] = Value::from(text.as_str());
                    }
                }
                Some(GameImage::Cleared) => {
                    if let Some(assets) = value.get_mut("assets").and_then(Value::as_object_mut) {
                        assets.remove(key);
                        assets.remove(text_key);
                    }
                }
                None => {}
            }
        }
    }
}

impl Extra {
    /// Whether the user hid this game from their status.
    fn is_hidden(&self, place_id: u64, universe_id: Option<u64>) -> bool {
        self.settings.hidden.contains(&place_id)
            || universe_id.is_some_and(|u| self.hidden_universes.contains(&u))
    }
}

impl Extra {
    /// The status's second line: the user's own text with its placeholders
    /// filled in, or Feline's line (creator, region, runtime, today).
    fn state(&self, game: &GameInfo) -> String {
        let region = self.region.as_deref().filter(|_| self.settings.region);
        let today = self.today.as_deref().filter(|_| self.settings.today);
        if !self.settings.text.trim().is_empty() {
            return self
                .settings
                .text
                .replace("{game}", &game.name)
                .replace("{creator}", &game.creator)
                .replace("{region}", region.unwrap_or("somewhere"))
                .replace("{runtime}", self.runtime.as_deref().unwrap_or("Roblox"))
                .replace("{today}", today.or(self.today.as_deref()).unwrap_or("a bit"));
        }
        // Kept short: creator, city and today's playtime. The runtime's
        // name is in the game picture's hover text instead.
        let mut parts = Vec::new();
        if !game.creator.is_empty() {
            parts.push(format!("by {}", game.creator));
        }
        if let Some(region) = region {
            // "Frankfurt am Main, Germany" → "Frankfurt am Main".
            let city = region.split(',').next().unwrap_or(region).trim();
            parts.push(format!("📍 {city}"));
        }
        if let Some(today) = today {
            parts.push(format!("{today} today"));
        }
        parts.join(" · ")
    }
}

/// Discord limits text fields to 128 characters (and wants at least 2).
fn clamp(text: &str) -> String {
    let mut out: String = text.chars().take(128).collect();
    while out.chars().count() < 2 {
        out.push(' ');
    }
    out
}

/// Builds the Discord activity for what the player is doing. `None` means
/// "show no status".
pub fn build_activity(
    activity: &Activity,
    game: Option<&GameInfo>,
    user: Option<&UserInfo>,
    options: &PresenceOptions,
    extra: &Extra,
) -> Option<Value> {
    // A hidden game: no name, picture, server or buttons.
    if let Activity::Game {
        place_id,
        universe_id,
        since,
        ..
    } = activity
        && extra.is_hidden(*place_id, *universe_id)
    {
        let mut value = json!({
            "type": 0,
            "details": "Playing something secret 🤫",
            "assets": { "large_image": "feline", "large_text": FELINE_LABEL },
        });
        if let Some(since) = since {
            value["timestamps"] = json!({ "start": since });
        }
        return Some(value);
    }
    match activity {
        Activity::Starting => None,
        Activity::Menu if !options.in_menus => None,
        Activity::Menu => Some(json!({
            "type": 0,
            "details": "In the Roblox menus 🐾",
            "assets": { "large_image": "feline", "large_text": FELINE_LABEL },
        })),
        Activity::Game {
            place_id,
            job_id,
            since,
            ..
        } => {
            let mut value = json!({ "type": 0 });
            match game {
                Some(game) => {
                    value["details"] = Value::from(clamp(&game.name));
                    let state = extra.state(game);
                    if !state.is_empty() {
                        value["state"] = Value::from(clamp(&state));
                    }
                    let hover = match &extra.runtime {
                        Some(runtime) => format!("{} · on {runtime}", game.name),
                        None => game.name.clone(),
                    };
                    value["assets"] = json!({
                        "large_image": game.icon_url.as_deref().unwrap_or("feline"),
                        "large_text": clamp(&hover),
                    });
                }
                None => {
                    value["details"] = Value::from("Joining a game");
                    value["assets"] =
                        json!({ "large_image": "feline", "large_text": FELINE_LABEL });
                }
            }
            // Small corner image: the player's avatar if allowed, else Feline.
            match (options.show_user, user) {
                (true, Some(user)) => {
                    value["assets"]["small_image"] =
                        Value::from(user.avatar_url.as_deref().unwrap_or("feline"));
                    value["assets"]["small_text"] = Value::from(clamp(&format!(
                        "Playing as {} (@{})",
                        user.display_name, user.name
                    )));
                }
                _ if game.is_some() => {
                    value["assets"]["small_image"] = Value::from("feline");
                    value["assets"]["small_text"] = Value::from(FELINE_LABEL);
                }
                _ => {}
            }
            if let Some(since) = since {
                value["timestamps"] = json!({ "start": since });
            }
            let mut buttons = Vec::new();
            if options.game_button {
                buttons.push(json!({
                    "label": "See this game",
                    "url": format!("https://www.roblox.com/games/{place_id}"),
                }));
            }
            if options.join_button && !job_id.is_empty() {
                buttons.push(json!({
                    "label": "Join server",
                    "url": format!(
                        "https://www.roblox.com/games/start?placeId={place_id}&gameInstanceId={job_id}"
                    ),
                }));
            }
            if !buttons.is_empty() {
                value["buttons"] = Value::from(buttons);
            }
            if extra.game_status_allowed {
                extra.game_status.over(&mut value);
            }
            Some(value)
        }
    }
}

/// A running Discord status for one game session. Stopping it (or dropping
/// it) clears the status.
pub struct Session {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Session {
    /// Starts following the logs under `log_roots` that change after `since`.
    /// `on_status` is called with a short, user-facing status line whenever
    /// it changes (e.g. "Connected to Discord").
    pub fn start(
        log_roots: Vec<PathBuf>,
        since: SystemTime,
        options: PresenceOptions,
        extra: Extra,
        on_status: impl Fn(String) + Send + 'static,
    ) -> std::io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = stop.clone();
        let thread = thread::Builder::new()
            .name("feline-presence".into())
            .spawn(move || run(log_roots, since, options, extra, &stop_flag, &on_status))?;
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }

    /// Clears the status and waits for the thread to finish.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The presence thread.
fn run(
    log_roots: Vec<PathBuf>,
    since: SystemTime,
    options: PresenceOptions,
    mut extra: Extra,
    stop: &AtomicBool,
    on_status: &dyn Fn(String),
) {
    let api = roblox::Api::new();
    let mut regions: HashMap<String, Option<String>> = HashMap::new();
    let mut last_today: Option<Instant> = None;
    let mut follower = LogFollower::new(log_roots.clone(), since);
    let mut activity = Activity::Starting;
    let mut games: HashMap<u64, Option<GameInfo>> = HashMap::new();
    let mut users: HashMap<u64, Option<UserInfo>> = HashMap::new();
    let mut discord: Option<Discord> = None;
    let mut last_attempt: Option<Instant> = None;
    let mut last_status = String::new();
    let mut dirty = true;

    let mut status = |text: String| {
        if text != last_status {
            on_status(text.clone());
            last_status = text;
        }
    };

    let mut asset_urls: HashMap<u64, Option<String>> = HashMap::new();
    while !stop.load(Ordering::Relaxed) {
        for event in follower.poll() {
            if let LogEvent::GameMessage(message) = &event {
                if extra.game_status_allowed && matches!(activity, Activity::Game { .. }) {
                    // The pictures it names, looked up once per asset.
                    for key in ["largeImage", "smallImage"] {
                        if let Some(id) = message.pointer(&format!("/data/{key}/assetId")).and_then(Value::as_u64) {
                            asset_urls.entry(id).or_insert_with(|| api.asset_image(id).ok());
                        }
                    }
                    let urls = &asset_urls;
                    if extra.game_status.apply(message, &|id| urls.get(&id).cloned().flatten()) {
                        dirty = true;
                    }
                }
                continue;
            }
            let next = activity.apply(event);
            if next != activity {
                // A different game (or the menus): the old game's own status
                // goes.
                let same_game = matches!((&activity, &next),
                    (Activity::Game { place_id: a, job_id: j, .. }, Activity::Game { place_id: b, job_id: k, .. }) if a == b && j == k);
                if !same_game {
                    extra.game_status = GameStatus::default();
                }
                activity = next;
                dirty = true;
            }
        }

        // Look up names once per game and player (cached for the session).
        if let Activity::Game {
            universe_id,
            user_id,
            ..
        } = &activity
        {
            if let Some(id) = universe_id
                && !games.contains_key(id)
            {
                games.insert(*id, api.game(*id).ok());
                dirty = true;
            }
            if options.show_user
                && let Some(id) = user_id
                && !users.contains_key(id)
            {
                users.insert(*id, api.user(*id).ok());
                dirty = true;
            }
        }

        // The server's region (once per server), and today's playtime
        // (every minute).
        if let Activity::Game { server, .. } = &activity {
            let region = match server {
                Some(ip) if extra.settings.region => regions
                    .entry(ip.clone())
                    .or_insert_with(|| api.server_region(ip).ok())
                    .clone(),
                _ => None,
            };
            if region != extra.region {
                extra.region = region;
                dirty = true;
            }
        }
        if extra.settings.today && last_today.is_none_or(|t| t.elapsed() >= Duration::from_secs(60)) {
            last_today = Some(Instant::now());
            // The game in progress counts too: its log may not have been
            // written out yet (Feline's runtime writes it in chunks).
            let playing_since = match &activity {
                Activity::Game { since, .. } => *since,
                _ => None,
            };
            let today = Some(crate::history::duration_label(crate::history::played_today_live(
                &log_roots,
                playing_since,
            )));
            if today != extra.today {
                extra.today = today;
                dirty = true;
            }
        }

        if discord.is_none() && last_attempt.is_none_or(|t| t.elapsed() >= RECONNECT) {
            last_attempt = Some(Instant::now());
            match Discord::connect() {
                Ok(d) => {
                    discord = Some(d);
                    dirty = true;
                    status("Connected to Discord".to_owned());
                }
                Err(e) => status(format!("{e}; trying again every 15 seconds")),
            }
        }

        if dirty && let Some(d) = discord.as_mut() {
            let (game, user) = match &activity {
                Activity::Game {
                    universe_id,
                    user_id,
                    ..
                } => (
                    universe_id
                        .and_then(|id| games.get(&id))
                        .and_then(Option::as_ref),
                    user_id
                        .and_then(|id| users.get(&id))
                        .and_then(Option::as_ref),
                ),
                _ => (None, None),
            };
            match d.set_activity(build_activity(&activity, game, user, &options, &extra)) {
                Ok(()) => {
                    dirty = false;
                    let what = match &activity {
                        Activity::Starting => "waiting for Roblox".to_owned(),
                        Activity::Menu => "in the menus".to_owned(),
                        Activity::Game { .. } => match game {
                            Some(g) => format!("showing {}", g.name),
                            None => "joining a game".to_owned(),
                        },
                    };
                    status(format!("Connected to Discord · {what}"));
                }
                Err(e) => {
                    discord = None;
                    status(format!("{e}; trying again every 15 seconds"));
                }
            }
        }

        // Sleep in small steps so stopping is quick.
        let wake = Instant::now() + POLL;
        while Instant::now() < wake && !stop.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(100));
        }
    }

    if let Some(mut d) = discord {
        let _ = d.set_activity(None);
    }
    status("Not showing a status (no game running)".to_owned());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game() -> GameInfo {
        GameInfo {
            name: "Fisch".into(),
            creator: "Fisching".into(),
            icon_url: Some("https://tr.rbxcdn.com/icon.png".into()),
        }
    }

    fn in_game() -> Activity {
        Activity::Starting
            .apply(LogEvent::Joining {
                place_id: 16732694052,
                job_id: "6b1f".into(),
                at: Some(1_793_780_695),
                server: None,
            })
            .apply(LogEvent::Joined {
                place_id: 16732694052,
                universe_id: 5750914919,
                user_id: 123,
            })
    }

    #[test]
    fn activity_follows_the_log() {
        let a = in_game();
        assert_eq!(
            a,
            Activity::Game {
                place_id: 16732694052,
                job_id: "6b1f".into(),
                universe_id: Some(5750914919),
                user_id: Some(123),
                since: Some(1_793_780_695),
                server: None,
            }
        );
        assert_eq!(a.apply(LogEvent::Left), Activity::Menu);
    }

    #[test]
    fn games_can_set_their_own_status_like_with_bloxstrap() {
        let line = r#"2026-11-12T10:00:00.000Z,1.0,abc,6 [FLog::CreatorOutput] [BloxstrapRPC] {"command":"SetRichPresence","data":{"details":"Fishing at the lake","state":"Level 12","timeStart":1700000000,"largeImage":{"assetId":42,"hoverText":"The lake"},"smallImage":{"clear":true}}}"#;
        let Some(LogEvent::GameMessage(message)) = log::parse_line(line) else { panic!("not read") };
        let urls = |id: u64| (id == 42).then(|| "https://tr.rbxcdn.com/lake.png".to_owned());
        let mut status = GameStatus::default();
        assert!(status.apply(&message, &urls));
        let mut extra = Extra { game_status_allowed: true, game_status: status.clone(), ..Default::default() };
        let v = build_activity(&in_game(), Some(&game()), None, &PresenceOptions::default(), &extra).unwrap();
        assert_eq!(v["details"], "Fishing at the lake");
        assert_eq!(v["state"], "Level 12");
        assert_eq!(v["timestamps"]["start"], 1_700_000_000);
        assert_eq!(v["assets"]["large_image"], "https://tr.rbxcdn.com/lake.png");
        assert_eq!(v["assets"]["large_text"], "The lake");
        assert!(v["assets"].get("small_image").is_none());
        // Only what a message names changes; "<reset>" brings Feline's back.
        let reset: Value = serde_json::from_str(r#"{"command":"SetRichPresence","data":{"state":"<reset>"}}"#).unwrap();
        assert!(extra.game_status.apply(&reset, &urls));
        let v = build_activity(&in_game(), Some(&game()), None, &PresenceOptions::default(), &extra).unwrap();
        assert_eq!(v["details"], "Fishing at the lake");
        assert_ne!(v["state"], "Level 12");
        // Switched off: the game can't change anything.
        extra.game_status_allowed = false;
        let v = build_activity(&in_game(), Some(&game()), None, &PresenceOptions::default(), &extra).unwrap();
        assert_ne!(v["details"], "Fishing at the lake");
        // Other commands and junk are ignored.
        let other: Value = serde_json::from_str(r#"{"command":"SetLaunchData","data":"x"}"#).unwrap();
        assert!(!status.apply(&other, &urls));
        assert!(log::parse_line("x [BloxstrapRPC] not json").is_none());
        assert_eq!(
            log::parse_line("z [FLog::Output] Server Prefix: 2b3c_20261112T160445Z_RCC_8f2e"),
            Some(LogEvent::ServerStarted(1_794_499_485))
        );
        assert_eq!(
            log::parse_line("y [FLog::Network] Client has been disconnected with reason: You were kicked for being idle."),
            Some(LogEvent::Disconnected("You were kicked for being idle".into()))
        );
    }

    #[test]
    fn extras_fill_the_second_line_and_hide_secret_games() {
        let options = PresenceOptions::default();
        let mut extra = Extra {
            region: Some("Frankfurt am Main, Germany".into()),
            runtime: Some("Sober".into()),
            today: Some("1 h 5 min".into()),
            ..Default::default()
        };
        extra.settings.today = true;
        let v = build_activity(&in_game(), Some(&game()), None, &options, &extra).unwrap();
        assert_eq!(
            v["state"],
            "by Fisching · 📍 Frankfurt am Main · 1 h 5 min today"
        );

        extra.settings.text = "Fishing in {region} with {runtime}".into();
        let v = build_activity(&in_game(), Some(&game()), None, &options, &extra).unwrap();
        assert_eq!(v["state"], "Fishing in Frankfurt am Main, Germany with Sober");

        extra.settings.hidden = vec![16732694052];
        let v = build_activity(&in_game(), Some(&game()), None, &options, &extra).unwrap();
        assert_eq!(v["details"], "Playing something secret 🤫");
        assert!(v.get("buttons").is_none() && v.get("state").is_none());

        // Hidden from another place of the same game (a sub-place).
        extra.settings.hidden = vec![1];
        extra.hidden_universes = vec![5750914919];
        let v = build_activity(&in_game(), Some(&game()), None, &options, &extra).unwrap();
        assert_eq!(v["details"], "Playing something secret 🤫");
    }

    #[test]
    fn defaults_show_the_game_but_not_the_user_or_join_button() {
        let options = PresenceOptions::default();
        let user = UserInfo {
            name: "someone".into(),
            display_name: "Someone".into(),
            avatar_url: None,
        };
        let v = build_activity(&in_game(), Some(&game()), Some(&user), &options, &Extra::default()).unwrap();
        assert_eq!(v["details"], "Fisch");
        assert_eq!(v["state"], "by Fisching");
        assert_eq!(v["assets"]["large_image"], "https://tr.rbxcdn.com/icon.png");
        assert_eq!(
            v["assets"]["small_text"], FELINE_LABEL,
            "no username by default"
        );
        assert_eq!(v["timestamps"]["start"], 1_793_780_695);
        let buttons = v["buttons"].as_array().unwrap();
        assert_eq!(buttons.len(), 1, "only the game page by default");
        assert_eq!(
            buttons[0]["url"],
            "https://www.roblox.com/games/16732694052"
        );
    }

    #[test]
    fn opt_ins_add_user_and_join_button() {
        let options = PresenceOptions {
            show_user: true,
            join_button: true,
            game_button: false,
            in_menus: false,
        };
        let user = UserInfo {
            name: "someone".into(),
            display_name: "Someone".into(),
            avatar_url: Some("https://tr.rbxcdn.com/me.png".into()),
        };
        let v = build_activity(&in_game(), Some(&game()), Some(&user), &options, &Extra::default()).unwrap();
        assert_eq!(v["assets"]["small_image"], "https://tr.rbxcdn.com/me.png");
        assert_eq!(v["assets"]["small_text"], "Playing as Someone (@someone)");
        assert_eq!(
            v["buttons"][0]["url"],
            "https://www.roblox.com/games/start?placeId=16732694052&gameInstanceId=6b1f"
        );
        // Menus hidden when asked.
        assert_eq!(build_activity(&Activity::Menu, None, None, &options, &Extra::default()), None);
        assert_eq!(
            build_activity(&Activity::Starting, None, None, &options, &Extra::default()),
            None
        );
    }

    #[test]
    fn clamps_text_for_discord() {
        assert_eq!(clamp("x"), "x ");
        assert_eq!(clamp(&"a".repeat(300)).chars().count(), 128);
    }
}

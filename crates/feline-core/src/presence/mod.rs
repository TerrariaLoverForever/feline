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

use crate::settings::PresenceOptions;
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
    },
}

impl Activity {
    /// The activity after `event`.
    pub fn apply(&self, event: LogEvent) -> Self {
        match event {
            LogEvent::Menu | LogEvent::Left => Self::Menu,
            LogEvent::Joining {
                place_id,
                job_id,
                at,
            } => Self::Game {
                place_id,
                job_id,
                universe_id: None,
                user_id: None,
                since: at,
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
                    ..
                } if *p == place_id => Self::Game {
                    place_id,
                    job_id: job_id.clone(),
                    universe_id: Some(universe_id),
                    user_id: Some(user_id),
                    since: *since,
                },
                _ => Self::Game {
                    place_id,
                    job_id: String::new(),
                    universe_id: Some(universe_id),
                    user_id: Some(user_id),
                    since: None,
                },
            },
        }
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
) -> Option<Value> {
    match activity {
        Activity::Starting => None,
        Activity::Menu if !options.in_menus => None,
        Activity::Menu => Some(json!({
            "type": 0,
            "details": "In the Roblox menus",
            "assets": { "large_image": "feline", "large_text": "Feline" },
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
                    if !game.creator.is_empty() {
                        value["state"] = Value::from(clamp(&format!("by {}", game.creator)));
                    }
                    value["assets"] = json!({
                        "large_image": game.icon_url.as_deref().unwrap_or("feline"),
                        "large_text": clamp(&game.name),
                    });
                }
                None => {
                    value["details"] = Value::from("Joining a game");
                    value["assets"] = json!({ "large_image": "feline", "large_text": "Feline" });
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
                    value["assets"]["small_text"] = Value::from("Feline");
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
        on_status: impl Fn(String) + Send + 'static,
    ) -> std::io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = stop.clone();
        let thread = thread::Builder::new()
            .name("feline-presence".into())
            .spawn(move || run(log_roots, since, options, &stop_flag, &on_status))?;
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
    stop: &AtomicBool,
    on_status: &dyn Fn(String),
) {
    let api = roblox::Api::new();
    let mut follower = LogFollower::new(log_roots, since);
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

    while !stop.load(Ordering::Relaxed) {
        for event in follower.poll() {
            let next = activity.apply(event);
            if next != activity {
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
            match d.set_activity(build_activity(&activity, game, user, &options)) {
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
            }
        );
        assert_eq!(a.apply(LogEvent::Left), Activity::Menu);
    }

    #[test]
    fn defaults_show_the_game_but_not_the_user_or_join_button() {
        let options = PresenceOptions::default();
        let user = UserInfo {
            name: "someone".into(),
            display_name: "Someone".into(),
            avatar_url: None,
        };
        let v = build_activity(&in_game(), Some(&game()), Some(&user), &options).unwrap();
        assert_eq!(v["details"], "Fisch");
        assert_eq!(v["state"], "by Fisching");
        assert_eq!(v["assets"]["large_image"], "https://tr.rbxcdn.com/icon.png");
        assert_eq!(
            v["assets"]["small_text"], "Feline",
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
        let v = build_activity(&in_game(), Some(&game()), Some(&user), &options).unwrap();
        assert_eq!(v["assets"]["small_image"], "https://tr.rbxcdn.com/me.png");
        assert_eq!(v["assets"]["small_text"], "Playing as Someone (@someone)");
        assert_eq!(
            v["buttons"][0]["url"],
            "https://www.roblox.com/games/start?placeId=16732694052&gameInstanceId=6b1f"
        );
        // Menus hidden when asked.
        assert_eq!(build_activity(&Activity::Menu, None, None, &options), None);
        assert_eq!(
            build_activity(&Activity::Starting, None, None, &options),
            None
        );
    }

    #[test]
    fn clamps_text_for_discord() {
        assert_eq!(clamp("x"), "x ");
        assert_eq!(clamp(&"a".repeat(300)).chars().count(), 128);
    }
}

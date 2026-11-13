// SPDX-License-Identifier: GPL-3.0-or-later
//! What you played, and for how long: read from Roblox's own logs.
//!
//! Roblox writes one log per session. Each "Joining game … place P at <ip>"
//! line starts a game, "game_join_loadtime … universeid:U" names its game,
//! and leaving (or going back to the menus, or joining another place) ends
//! it. Feline reads every player log it hasn't read yet, keeps what it
//! found in `history.json` in its state folder, and adds the games' names
//! and icons (Roblox's public web API) and the servers' regions.
//!
//! Everything stays on this computer. "Clear history" deletes the file.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::presence::log::{LogEvent, line_time, parse_line};

/// The version of `history.json`'s format. 2: public server addresses.
const FORMAT: u64 = 2;

/// One stay in one game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub place_id: u64,
    pub universe_id: Option<u64>,
    /// The game server's address, if the log said.
    pub server: Option<String>,
    /// Unix times.
    pub start: u64,
    pub end: u64,
}

impl Session {
    pub fn seconds(&self) -> u64 {
        self.end.saturating_sub(self.start)
    }
}

/// The sessions in one log file's text.
pub fn sessions_in(text: &str) -> Vec<Session> {
    let mut done = Vec::new();
    let mut open: Option<Session> = None;
    let mut last_time = None;
    for line in text.lines() {
        let time = line_time(line);
        if time.is_some() {
            last_time = time;
        }
        let Some(event) = parse_line(line) else {
            continue;
        };
        let now = time.or(last_time).unwrap_or(0);
        match event {
            LogEvent::Joining {
                place_id, server, ..
            } => {
                if let Some(mut s) = open.take() {
                    s.end = now;
                    done.push(s);
                }
                open = Some(Session {
                    place_id,
                    universe_id: None,
                    server,
                    start: now,
                    end: now,
                });
            }
            LogEvent::Joined {
                place_id,
                universe_id,
                ..
            } => {
                if let Some(s) = open.as_mut().filter(|s| s.place_id == place_id) {
                    s.universe_id = Some(universe_id);
                }
            }
            LogEvent::Server(ip) => {
                if let Some(s) = open.as_mut() {
                    s.server = Some(ip);
                }
            }
            LogEvent::Left | LogEvent::Menu => {
                if let Some(mut s) = open.take() {
                    s.end = now;
                    done.push(s);
                }
            }
            LogEvent::GameMessage(_) | LogEvent::Disconnected(_) | LogEvent::ServerStarted(_) => {}
        }
    }
    // The log ended mid-game (Roblox closed, or is still running).
    if let Some(mut s) = open {
        s.end = last_time.unwrap_or(s.start);
        done.push(s);
    }
    // A join that never got anywhere isn't a session.
    done.retain(|s| s.start > 0);
    done
}

/// What Feline knows about a game.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GameInfo {
    pub name: String,
    pub creator: String,
    pub icon_url: Option<String>,
}

/// Everything in `history.json`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct History {
    /// Sessions by the log file they came from.
    pub logs: BTreeMap<String, (u64, Vec<Session>)>,
    /// Game names and icons by universe ID.
    pub games: BTreeMap<u64, GameInfo>,
    /// Server regions by address.
    pub regions: BTreeMap<String, String>,
    /// Pinned games (place IDs), in the order they were pinned.
    pub favourites: Vec<u64>,
}

/// One game on the Home page: what it is and how much it was played.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameSummary {
    pub place_id: u64,
    pub universe_id: Option<u64>,
    pub seconds: u64,
    pub last_played: u64,
    pub sessions: usize,
    /// Every place of this game that was played (`place_id` is one of them).
    pub places: Vec<u64>,
}

impl GameSummary {
    /// Whether any of this game's places is in `list` (favourites, hidden).
    pub fn any_place_in(&self, list: &[u64]) -> bool {
        self.places.iter().any(|p| list.contains(p)) || list.contains(&self.place_id)
    }
}

pub fn history_path() -> Option<PathBuf> {
    crate::paths::state_dir().map(|d| d.join("history.json"))
}

/// Game icons are kept here as `<universe>.png`.
pub fn icon_dir() -> Option<PathBuf> {
    crate::paths::state_dir().map(|d| d.join("icons"))
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl History {
    pub fn load() -> Self {
        history_path()
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .map(|v| Self::from_json(&v))
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = history_path().ok_or_else(|| std::io::Error::other("no state folder"))?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let temp = path.with_extension("tmp");
        fs::write(&temp, serde_json::to_string(&self.to_json())?)?;
        fs::rename(temp, path)
    }

    /// Reads the player logs under `roots` that are new or grew since last
    /// time. Returns whether anything changed.
    pub fn scan(&mut self, roots: &[PathBuf]) -> bool {
        let mut changed = false;
        for root in roots {
            let Ok(entries) = fs::read_dir(root) else {
                continue;
            };
            for entry in entries.filter_map(Result::ok) {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !name.contains("_Player_") || !name.ends_with(".log") || name.contains("CrashHandler") {
                    continue;
                }
                let Ok(size) = entry.metadata().map(|m| m.len()) else {
                    continue;
                };
                if self.logs.get(&name).is_some_and(|(s, _)| *s == size) {
                    continue;
                }
                let Ok(bytes) = fs::read(entry.path()) else {
                    continue;
                };
                let sessions = sessions_in(&String::from_utf8_lossy(&bytes));
                self.logs.insert(name, (size, sessions));
                changed = true;
            }
        }
        changed
    }

    pub fn sessions(&self) -> impl Iterator<Item = &Session> {
        self.logs.values().flat_map(|(_, s)| s.iter())
    }

    /// Seconds played since `since` (Unix time).
    /// The newest `n` sessions, newest first.
    pub fn recent_sessions(&self, n: usize) -> Vec<&Session> {
        let mut all: Vec<&Session> = self.sessions().collect();
        all.sort_by_key(|s| std::cmp::Reverse(s.start));
        all.truncate(n);
        all
    }

    pub fn played_since(&self, since: u64) -> u64 {
        self.sessions()
            .map(|s| s.end.saturating_sub(s.start.max(since)))
            .sum()
    }

    /// The game (universe) a place belongs to, if a log ever said.
    pub fn universe_of(&self, place_id: u64) -> Option<u64> {
        self.sessions()
            .filter(|s| s.place_id == place_id)
            .find_map(|s| s.universe_id)
    }

    /// Every place Feline has seen of the same game as `place_id` (at least
    /// `place_id` itself).
    pub fn places_of_game(&self, place_id: u64) -> Vec<u64> {
        let mut places = vec![place_id];
        if let Some(universe) = self.universe_of(place_id) {
            places.extend(
                self.sessions()
                    .filter(|s| s.universe_id == Some(universe))
                    .map(|s| s.place_id),
            );
        }
        places.sort_unstable();
        places.dedup();
        places
    }

    /// Every game played, most recently played first. A game is one
    /// universe: its lobby and the places it teleports you to (sub-places,
    /// private and reserved servers) are counted together, so a game shows
    /// up once however you got into it.
    pub fn games_played(&self) -> Vec<GameSummary> {
        // Which universe each place belongs to (a session may not say, e.g.
        // when the log was cut short, but another session of it did).
        let mut universe_of: BTreeMap<u64, u64> = BTreeMap::new();
        for s in self.sessions() {
            if let Some(u) = s.universe_id {
                universe_of.insert(s.place_id, u);
            }
        }
        // Grouped by universe (or by place while the universe is unknown).
        #[derive(Default)]
        struct Group {
            summary: Option<GameSummary>,
            /// Sessions and last time played, per place.
            per_place: BTreeMap<u64, (usize, u64)>,
        }
        let mut groups: BTreeMap<(bool, u64), Group> = BTreeMap::new();
        for s in self.sessions() {
            let universe = universe_of.get(&s.place_id).copied();
            let key = universe.map_or((false, s.place_id), |u| (true, u));
            let group = groups.entry(key).or_default();
            let summary = group.summary.get_or_insert(GameSummary {
                place_id: s.place_id,
                universe_id: universe,
                seconds: 0,
                last_played: 0,
                sessions: 0,
                places: Vec::new(),
            });
            summary.seconds += s.seconds();
            summary.last_played = summary.last_played.max(s.end);
            summary.sessions += 1;
            let place = group.per_place.entry(s.place_id).or_default();
            place.0 += 1;
            place.1 = place.1.max(s.end);
        }
        let mut list: Vec<GameSummary> = groups
            .into_values()
            .filter_map(|group| {
                let mut summary = group.summary?;
                // The place to start: the one visited most often (the
                // lobby, usually: sub-places often can't be joined directly),
                // then the most recent.
                summary.place_id = group
                    .per_place
                    .iter()
                    .max_by_key(|(_, (count, last))| (*count, *last))
                    .map_or(summary.place_id, |(place, _)| *place);
                summary.places = group.per_place.into_keys().collect();
                Some(summary)
            })
            .collect();
        list.sort_by_key(|g| std::cmp::Reverse(g.last_played));
        list
    }

    /// The name of a game, or "Place <id>" until Roblox's API told us.
    pub fn name_of(&self, game: &GameSummary) -> String {
        game.universe_id
            .and_then(|u| self.games.get(&u))
            .map(|g| g.name.clone())
            .unwrap_or_else(|| format!("Place {}", game.place_id))
    }

    /// Universes whose names Feline doesn't know yet.
    pub fn unnamed_universes(&self) -> Vec<u64> {
        let mut list: Vec<u64> = self
            .sessions()
            .filter_map(|s| s.universe_id)
            .filter(|u| !self.games.contains_key(u))
            .collect();
        list.sort_unstable();
        list.dedup();
        list
    }

    /// Pins or unpins a game. Unpinning removes every place of that game
    /// (so a game pinned from two of its places comes off in one click).
    pub fn toggle_favourite(&mut self, place_id: u64) -> bool {
        let places = self.places_of_game(place_id);
        if self.favourites.iter().any(|p| places.contains(p)) {
            self.favourites.retain(|p| !places.contains(p));
            false
        } else {
            self.favourites.push(place_id);
            true
        }
    }

    /// Forgets every session (keeps favourites and the name cache).
    pub fn clear_sessions(&mut self) {
        for (_, sessions) in self.logs.values_mut() {
            sessions.clear();
        }
    }

    fn to_json(&self) -> Value {
        let session = |s: &Session| {
            json!([s.place_id, s.universe_id, s.server, s.start, s.end])
        };
        json!({
            "version": FORMAT,
            "logs": self.logs.iter().map(|(name, (size, list))| {
                (name.clone(), json!({ "size": size, "sessions": list.iter().map(session).collect::<Vec<_>>() }))
            }).collect::<serde_json::Map<_, _>>(),
            "games": self.games.iter().map(|(u, g)| {
                (u.to_string(), json!({ "name": g.name, "creator": g.creator, "icon": g.icon_url }))
            }).collect::<serde_json::Map<_, _>>(),
            "regions": self.regions,
            "favourites": self.favourites,
        })
    }

    fn from_json(v: &Value) -> Self {
        let mut h = Self::default();
        // Older formats are read again from the logs (only the log list is
        // dropped; names, regions and favourites stay).
        let current = v.get("version").and_then(Value::as_u64) == Some(FORMAT);
        if let Some(logs) = v.get("logs").and_then(Value::as_object).filter(|_| current) {
            for (name, entry) in logs {
                let size = entry.get("size").and_then(Value::as_u64).unwrap_or(0);
                let sessions = entry
                    .get("sessions")
                    .and_then(Value::as_array)
                    .map(|list| {
                        list.iter()
                            .filter_map(|s| {
                                Some(Session {
                                    place_id: s.get(0)?.as_u64()?,
                                    universe_id: s.get(1).and_then(Value::as_u64),
                                    server: s.get(2).and_then(Value::as_str).map(str::to_owned),
                                    start: s.get(3)?.as_u64()?,
                                    end: s.get(4)?.as_u64()?,
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                h.logs.insert(name.clone(), (size, sessions));
            }
        }
        if let Some(games) = v.get("games").and_then(Value::as_object) {
            for (u, g) in games {
                let Ok(u) = u.parse() else { continue };
                h.games.insert(
                    u,
                    GameInfo {
                        name: g.get("name").and_then(Value::as_str).unwrap_or_default().to_owned(),
                        creator: g.get("creator").and_then(Value::as_str).unwrap_or_default().to_owned(),
                        icon_url: g.get("icon").and_then(Value::as_str).map(str::to_owned),
                    },
                );
            }
        }
        if let Some(regions) = v.get("regions").and_then(Value::as_object) {
            for (ip, r) in regions {
                if let Some(r) = r.as_str() {
                    h.regions.insert(ip.clone(), r.to_owned());
                }
            }
        }
        h.favourites = v
            .get("favourites")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_u64).collect())
            .unwrap_or_default();
        h
    }
}

/// Fills in game names/icons and server regions Feline doesn't know yet,
/// from the network (call it off the UI thread). Returns whether anything
/// was added. Icons are saved as `<icon dir>/<universe>.png`.
pub fn fetch_missing(history: &mut History, regions: bool) -> bool {
    let api = crate::presence::roblox::Api::new();
    let mut changed = false;
    for universe in history.unnamed_universes().into_iter().take(20) {
        if let Ok(game) = api.game(universe) {
            if let (Some(url), Some(dir)) = (&game.icon_url, icon_dir()) {
                let file = dir.join(format!("{universe}.png"));
                if !file.exists()
                    && let Ok(bytes) = api.download(url)
                {
                    let _ = fs::create_dir_all(&dir);
                    let _ = fs::write(file, bytes);
                }
            }
            history.games.insert(
                universe,
                GameInfo {
                    name: game.name,
                    creator: game.creator,
                    icon_url: game.icon_url,
                },
            );
            changed = true;
        }
    }
    if regions {
        let mut ips: Vec<String> = history
            .sessions()
            .filter_map(|s| s.server.clone())
            .filter(|ip| !history.regions.contains_key(ip))
            .collect();
        ips.sort();
        ips.dedup();
        for ip in ips.into_iter().rev().take(10) {
            if let Ok(region) = api.server_region(&ip) {
                history.regions.insert(ip, region);
                changed = true;
            }
        }
    }
    changed
}

/// The icon file of a game, if it was downloaded.
pub fn icon_file(universe: Option<u64>) -> Option<PathBuf> {
    let file = icon_dir()?.join(format!("{}.png", universe?));
    file.is_file().then_some(file)
}

/// This computer's offset from UTC in seconds (east positive), for "today".
pub fn local_offset_seconds() -> i64 {
    #[cfg(windows)]
    return feline_win::utc_offset_seconds();
    #[cfg(not(windows))]
    {
        // `date +%z` prints e.g. "+0200"; no date library needed for this.
        let out = std::process::Command::new("date").arg("+%z").output().ok();
        let text = out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned()).unwrap_or_default();
        parse_offset(&text).unwrap_or(0)
    }
}

/// "+0200" → 7200, "-0530" → -19800.
pub fn parse_offset(text: &str) -> Option<i64> {
    let (sign, digits) = match text.as_bytes().first()? {
        b'+' => (1, &text[1..]),
        b'-' => (-1, &text[1..]),
        _ => return None,
    };
    if digits.len() != 4 {
        return None;
    }
    let hours: i64 = digits[..2].parse().ok()?;
    let minutes: i64 = digits[2..].parse().ok()?;
    Some(sign * (hours * 3600 + minutes * 60))
}

/// Seconds played today (local time), from every log under `roots`.
pub fn played_today(roots: &[PathBuf]) -> u64 {
    let mut history = History::load();
    history.scan(roots);
    let (today, _) = day_and_week_start(now(), local_offset_seconds());
    history.played_since(today)
}

/// Like [`played_today`], with the game being played now counted up to
/// this moment: `playing_since` is when it started. Its log may lag behind
/// (or not have its session yet), so its sessions are replaced by the live
/// time.
pub fn played_today_live(roots: &[PathBuf], playing_since: Option<u64>) -> u64 {
    let mut history = History::load();
    history.scan(roots);
    let now = now();
    let (today, _) = day_and_week_start(now, local_offset_seconds());
    live_total(&history, today, now, playing_since)
}

fn live_total(history: &History, today: u64, now: u64, playing_since: Option<u64>) -> u64 {
    let Some(since) = playing_since.filter(|s| *s <= now) else {
        return history.played_since(today);
    };
    let finished: u64 = history
        .sessions()
        .filter(|s| s.start < since)
        .map(|s| s.end.min(since).saturating_sub(s.start.max(today)))
        .sum();
    finished + now.saturating_sub(since.max(today))
}

/// "2 h 5 min", "12 min", "40 s".
pub fn duration_label(seconds: u64) -> String {
    match seconds {
        s if s < 60 => format!("{s} s"),
        s if s < 3600 => format!("{} min", s / 60),
        s => format!("{} h {} min", s / 3600, (s % 3600) / 60),
    }
}

/// Local midnight today and the start of the week (Monday), as Unix times.
/// Uses the computer's UTC offset from `offset_seconds` (east positive).
pub fn day_and_week_start(now: u64, offset_seconds: i64) -> (u64, u64) {
    let local = now as i64 + offset_seconds;
    let day = local.div_euclid(86_400);
    let today = day * 86_400 - offset_seconds;
    // 1970-01-01 was a Thursday: Monday is 3 days before.
    let weekday = (day + 3).rem_euclid(7);
    let week = (day - weekday) * 86_400 - offset_seconds;
    (today.max(0) as u64, week.max(0) as u64)
}


#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "\
2026-11-09T19:50:00.000Z,1.0,1,6 [FLog::SingleSurfaceApp] setStage: (stage:LuaApp)
2026-11-09T19:51:00.000Z,1.0,1,6 [FLog::Output] ! Joining game 'abc' place 111 at 128.116.1.2
2026-11-09T19:51:04.000Z,1.0,1,6 [FLog::Network] UDMUX Address = 128.116.50.3, Port = 53969 | RCC Server Address = 10.34.3.13, Port = 53969
2026-11-09T19:51:05.000Z,1.0,1,6 [FLog::GameJoinLoadTime] Report game_join_loadtime: placeid:111, userid:9, universeid:222,
2026-11-09T20:01:00.000Z,1.0,1,6 [FLog::SingleSurfaceApp] leaveUGCGameInternal
2026-11-09T20:02:00.000Z,1.0,1,6 [FLog::Output] ! Joining game 'def' place 333 at 128.116.9.9
2026-11-09T20:05:00.000Z,1.0,1,6 something else
";

    #[test]
    fn sessions_come_from_join_and_leave_lines() {
        let s = sessions_in(LOG);
        assert_eq!(s.len(), 2);
        assert_eq!((s[0].place_id, s[0].universe_id, s[0].seconds()), (111, Some(222), 600));
        // The public address (UDMUX), not Roblox's internal one.
        assert_eq!(s[0].server.as_deref(), Some("128.116.50.3"));
        // The log ended mid-game: the session lasts to its last line.
        assert_eq!((s[1].place_id, s[1].seconds()), (333, 180));
    }

    #[test]
    fn summaries_and_json_round_trip() {
        let mut h = History::default();
        h.logs.insert("a_Player_1_last.log".into(), (10, sessions_in(LOG)));
        h.games.insert(222, GameInfo { name: "Cat Game".into(), ..Default::default() });
        h.regions.insert("128.116.1.2".into(), "Frankfurt, Germany".into());
        h.toggle_favourite(111);
        let games = h.games_played();
        assert_eq!(games[0].place_id, 333, "most recent first");
        assert_eq!(h.name_of(&games[1]), "Cat Game");
        assert_eq!(h.name_of(&games[0]), "Place 333");
        assert_eq!(History::from_json(&h.to_json()), h);
    }

    #[test]
    fn places_of_one_game_count_as_one_game() {
        let s = |place, universe, start| Session {
            place_id: place,
            universe_id: universe,
            server: None,
            start,
            end: start + 60,
        };
        let mut h = History::default();
        // Lobby (10) three times, a sub-place (11) twice, all universe 1; the
        // sub-place's second visit was cut short before the universe line.
        h.logs.insert(
            "x_Player_1.log".into(),
            (
                1,
                vec![
                    s(10, Some(1), 100),
                    s(11, Some(1), 200),
                    s(10, Some(1), 300),
                    s(11, None, 400),
                    s(10, Some(1), 500),
                    s(20, Some(2), 50),
                ],
            ),
        );
        let games = h.games_played();
        assert_eq!(games.len(), 2, "{games:?}");
        // Started from the lobby, the place visited most.
        assert_eq!((games[0].place_id, games[0].sessions, games[0].seconds), (10, 5, 300));
        assert_eq!(games[0].places, vec![10, 11]);
        assert_eq!(h.places_of_game(11), vec![10, 11]);
        // Pinned from the sub-place, unpinned from the lobby.
        assert!(h.toggle_favourite(11));
        assert!(games[0].any_place_in(&h.favourites));
        assert!(!h.toggle_favourite(10));
        assert!(h.favourites.is_empty());
    }

    #[test]
    fn days_and_weeks_start_at_local_midnight() {
        // 2026-11-11 (a Wednesday) 10:00 UTC, in UTC+2.
        let now = 1_794_391_200;
        let (today, week) = day_and_week_start(now, 7200);
        // Local midnight is 22:00 UTC the day before.
        assert_eq!(today, now - 12 * 3600);
        // Monday is two days before Wednesday.
        assert_eq!(week, today - 2 * 86_400);
    }

    #[test]
    fn utc_offsets_parse() {
        assert_eq!(parse_offset("+0200"), Some(7200));
        assert_eq!(parse_offset("-0530"), Some(-19800));
        assert_eq!(parse_offset("UTC"), None);
    }

    #[test]
    fn regions_read_from_ipinfo() {
        let answer = json!({"ip": "128.116.1.2", "city": "Frankfurt am Main", "country": "DE"});
        assert_eq!(
            crate::presence::roblox::parse_region(&answer).as_deref(),
            Some("Frankfurt am Main, Germany")
        );
    }
    #[test]
    fn the_game_in_progress_counts_today() {
        let mut history = History::default();
        let s = |start, end| Session { place_id: 1, universe_id: None, server: None, start, end };
        // Earlier today: 10 min; yesterday's game doesn't count; the game
        // now started at 1000 and its log only reached 1060.
        history.logs.insert("a".into(), (1, vec![s(100, 700), s(1000, 1060)]));
        history.logs.insert("b".into(), (1, vec![s(0, 50)]));
        assert_eq!(live_total(&history, 60, 1600, Some(1000)), 600 + 600);
        // Not playing: the log's sessions only.
        assert_eq!(live_total(&history, 60, 1600, None), 600 + 60);
    }

}

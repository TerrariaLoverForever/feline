// SPDX-License-Identifier: GPL-3.0-or-later
//! More about game servers: where they are and how long they've been up
//! (RoValra's public server API, the one Fishstrap and Froststrap use), the
//! servers you were on (server history), and invite links.
//!
//! RoValra (<https://www.rovalra.com>) watches Roblox's public servers and
//! answers with their location and when it first saw them. Only the game
//! and server IDs are sent; nothing about the player.

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};

const API: &str = "https://apis.rovalra.com/v1";

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .user_agent(format!("Feline/{}", crate::VERSION))
        .build()
        .into()
}

fn get(url: &str) -> Result<Value, String> {
    let text = agent()
        .get(url)
        .call()
        .map_err(|e| format!("RoValra couldn't be reached ({e})"))?
        .body_mut()
        .with_config()
        .limit(4 * 1024 * 1024)
        .read_to_string()
        .map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|_| "RoValra answered with something unexpected".to_owned())
}

/// A public server, as RoValra knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerSpot {
    pub id: String,
    pub city: String,
    pub country: String,
    /// Unix time RoValra first saw it (about when it started).
    pub started: Option<u64>,
    /// The game's version on that server.
    pub version: Option<u64>,
}

impl ServerSpot {
    /// "Frankfurt am Main, Germany".
    pub fn place(&self) -> String {
        match (self.city.is_empty(), self.country.is_empty()) {
            (false, false) => format!("{}, {}", self.city, self.country),
            (false, true) => self.city.clone(),
            _ => self.country.clone(),
        }
    }
}

/// `2026-11-12T16:04:45.944000Z` → Unix time.
pub fn iso_time(text: &str) -> Option<u64> {
    let digits = |r: std::ops::Range<usize>| text.get(r)?.parse::<i64>().ok();
    let (y, m, d) = (digits(0..4)?, digits(5..7)?, digits(8..10)?);
    let (hh, mm, ss) = (digits(11..13)?, digits(14..16)?, digits(17..19)?);
    // Days from civil (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + hh * 3600 + mm * 60 + ss).ok()
}

pub fn parse_spots(v: &Value) -> Vec<ServerSpot> {
    v.get("servers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|s| {
            Some(ServerSpot {
                id: s.get("server_id")?.as_str()?.to_owned(),
                city: s.get("city").and_then(Value::as_str).unwrap_or_default().to_owned(),
                country: s.get("country").and_then(Value::as_str).unwrap_or_default().to_owned(),
                started: s.get("first_seen").and_then(Value::as_str).and_then(iso_time),
                version: s.get("place_version").and_then(Value::as_u64),
            })
        })
        .collect()
}

/// Up to 100 public servers of `place_id` in a country (two-letter code).
pub fn servers_in(place_id: u64, country: &str) -> Result<Vec<ServerSpot>, String> {
    let country: String = country.chars().filter(char::is_ascii_alphabetic).take(2).collect();
    let v = get(&format!("{API}/servers/region?place_id={place_id}&country={country}&limit=100"))?;
    Ok(parse_spots(&v))
}

/// One server's location and start time, if RoValra knows it.
pub fn server_details(place_id: u64, server_id: &str) -> Result<Option<ServerSpot>, String> {
    let id: String = server_id.chars().filter(|c| c.is_ascii_hexdigit() || *c == '-').collect();
    let v = get(&format!("{API}/servers/details?place_id={place_id}&server_ids={id}"))?;
    Ok(parse_spots(&v).into_iter().next())
}

/// The countries with Roblox servers: (code, name), sorted by name.
pub fn countries() -> Result<Vec<(String, String)>, String> {
    let v = get(&format!("{API}/datacenters/list"))?;
    let mut list: Vec<(String, String)> = v
        .as_array()
        .into_iter()
        .flatten()
        .filter(|d| d.get("inactive").and_then(Value::as_bool) != Some(true))
        .filter_map(|d| {
            let l = d.get("location")?;
            Some((l.get("country")?.as_str()?.to_owned(), l.get("country_name")?.as_str()?.to_owned()))
        })
        .collect();
    list.sort_by(|a, b| a.1.cmp(&b.1));
    list.dedup();
    Ok(list)
}

/// "2 h 13 min", "45 min", "a few minutes".
pub fn uptime_label(started: u64, now: u64) -> String {
    let minutes = now.saturating_sub(started) / 60;
    match minutes {
        0..=4 => "a few minutes".to_owned(),
        5..=59 => format!("{minutes} min"),
        _ if minutes < 48 * 60 => format!("{} h {} min", minutes / 60, minutes % 60),
        _ => format!("{} days", minutes / (24 * 60)),
    }
}

/// A link that drops anyone with Roblox into this exact server (Roblox's
/// own web link: it opens Roblox and joins).
pub fn invite_link(place_id: u64, server_id: &str) -> String {
    format!("https://www.roblox.com/games/start?placeId={place_id}&gameInstanceId={server_id}")
}

// ---------------------------------------------------------------------------
// Server history
// ---------------------------------------------------------------------------

/// A server you were on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Visit {
    pub place_id: u64,
    pub job_id: String,
    /// The game's name, if known.
    pub game: String,
    /// Where the server is, if known.
    pub region: String,
    /// Unix time you joined.
    pub at: u64,
}

const KEEP: usize = 30;

fn history_path() -> Option<PathBuf> {
    crate::paths::state_dir().map(|d| d.join("server-history.json"))
}

/// The servers you were on, newest first.
pub fn visits() -> Vec<Visit> {
    let Some(text) = history_path().and_then(|p| fs::read_to_string(p).ok()) else { return Vec::new() };
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(&text) else { return Vec::new() };
    items
        .iter()
        .filter_map(|v| {
            Some(Visit {
                place_id: v.get("place")?.as_u64()?,
                job_id: v.get("job")?.as_str()?.to_owned(),
                game: v.get("game").and_then(Value::as_str).unwrap_or_default().to_owned(),
                region: v.get("region").and_then(Value::as_str).unwrap_or_default().to_owned(),
                at: v.get("at").and_then(Value::as_u64).unwrap_or(0),
            })
        })
        .collect()
}

/// Remembers a server (or fills in its game or region, if it's the newest
/// one already). Keeps the last 30.
pub fn record(visit: Visit) -> std::io::Result<()> {
    let mut all = visits();
    match all.first_mut() {
        Some(top) if top.job_id == visit.job_id => {
            if !visit.game.is_empty() {
                top.game = visit.game;
            }
            if !visit.region.is_empty() {
                top.region = visit.region;
            }
        }
        _ => {
            all.retain(|v| v.job_id != visit.job_id);
            all.insert(0, visit);
            all.truncate(KEEP);
        }
    }
    let items: Vec<Value> = all
        .iter()
        .map(|v| json!({"place": v.place_id, "job": v.job_id, "game": v.game, "region": v.region, "at": v.at}))
        .collect();
    let path = history_path().ok_or_else(|| std::io::Error::other("no state folder"))?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, Value::Array(items).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rovalra_answers_are_read() {
        let v: Value = serde_json::from_str(
            r#"{"servers":[{"city":"Frankfurt am Main","country":"Germany","first_seen":"2026-11-12T16:04:45.944000Z",
                "place_version":1821,"server_id":"4e959569-16de-4a9f-9948-70f5d434881c"},{"bad":1}]}"#,
        )
        .unwrap();
        let spots = parse_spots(&v);
        assert_eq!(spots.len(), 1);
        assert_eq!(spots[0].place(), "Frankfurt am Main, Germany");
        assert_eq!(spots[0].started, Some(1_794_499_485));
        assert_eq!(spots[0].version, Some(1821));
    }

    #[test]
    fn times_and_labels() {
        assert_eq!(iso_time("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(iso_time("2000-03-01T12:30:15Z"), Some(951_913_815));
        assert_eq!(iso_time("nope"), None);
        assert_eq!(uptime_label(0, 120), "a few minutes");
        assert_eq!(uptime_label(0, 45 * 60), "45 min");
        assert_eq!(uptime_label(0, 133 * 60), "2 h 13 min");
        assert_eq!(uptime_label(0, 3 * 86_400), "3 days");
        assert_eq!(invite_link(1, "a-b"), "https://www.roblox.com/games/start?placeId=1&gameInstanceId=a-b");
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! Tools around Roblox itself: is Roblox up ([`status`]), which versions you
//! had ([`VersionLog`]), saved sets of Roblox's settings ([`Profile`], also
//! applied per game), and its public server list ([`servers`]).

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde_json::{Value, json};

use crate::runtime::config::ConfigError;
use crate::runtime::{DetectContext, Installation};

// ---------------------------------------------------------------------------
// Is Roblox down?
// ---------------------------------------------------------------------------

/// What status.roblox.com says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RobloxStatus {
    /// e.g. "All Systems Operational".
    pub headline: String,
    /// Components that aren't operational: (name, state), e.g.
    /// ("Player Games", "Degraded Performance").
    pub problems: Vec<(String, String)>,
}

impl RobloxStatus {
    pub fn all_good(&self) -> bool {
        self.problems.is_empty() && self.headline.to_lowercase().contains("operational")
    }
}

const STATES: &[&str] = &[
    "Operational",
    "Degraded Performance",
    "Partial Outage",
    "Major Outage",
    "Under Maintenance",
    "Maintenance",
    "Degraded",
    "Outage",
];

/// Fetches and reads Roblox's status page (it has no JSON feed; the page's
/// status bar and component list are read as text).
pub fn status() -> Result<RobloxStatus, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(10)))
        .user_agent(format!("Feline/{}", crate::VERSION))
        .build()
        .into();
    let html = agent
        .get("https://status.roblox.com/")
        .call()
        .map_err(|e| format!("status.roblox.com couldn't be reached ({e})"))?
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    parse_status(&html).ok_or_else(|| "status.roblox.com's page changed; Feline can't read it".to_owned())
}

/// The text of `html` with tags turned into `|` separators.
fn text_tokens(html: &str) -> Vec<String> {
    let mut out = String::with_capacity(html.len() / 2);
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => {
                in_tag = true;
                out.push('|');
            }
            '>' => {
                in_tag = false;
                out.push('|');
            }
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.split('|')
        .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|t| !t.is_empty())
        .collect()
}

pub fn parse_status(html: &str) -> Option<RobloxStatus> {
    let bar = html.find("data-section-type=\"statusbar\"")?;
    let headline = text_tokens(&html[bar..(bar + 2000).min(html.len())])
        .into_iter()
        .find(|t| !t.starts_with("data-") && !t.contains('='))?;
    let mut problems = Vec::new();
    if let Some(start) = html.find("data-section-type=\"components\"") {
        let end = html[start..]
            .find("data-section-type=\"history\"")
            .map_or(html.len(), |e| start + e);
        let tokens = text_tokens(&html[start..end]);
        // "User", "Website", "Operational", "Mobile App", "Operational", …:
        // a name right before a state; a name not followed by one is a group.
        let mut group = String::new();
        for pair in tokens.windows(2) {
            let (name, next) = (&pair[0], &pair[1]);
            if STATES.contains(&name.as_str()) {
                continue;
            }
            if STATES.contains(&next.as_str()) {
                if next != "Operational" {
                    let full = if group.is_empty() {
                        name.clone()
                    } else {
                        format!("{group} {name}")
                    };
                    problems.push((full, next.clone()));
                }
            } else if name.len() < 40 && !name.contains('=') {
                group = name.clone();
            }
        }
    }
    Some(RobloxStatus { headline, problems })
}

// ---------------------------------------------------------------------------
// Version history
// ---------------------------------------------------------------------------

/// Which versions of each runtime Feline has seen, with the time it first saw
/// each one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VersionLog {
    /// backend ID → [(version, first seen)], oldest first.
    pub runtimes: BTreeMap<String, Vec<(String, u64)>>,
}

fn state_file(name: &str) -> Option<PathBuf> {
    crate::paths::state_dir().map(|d| d.join(name))
}

impl VersionLog {
    pub fn load() -> Self {
        let mut log = Self::default();
        let Some(v) = state_file("versions.json")
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        else {
            return log;
        };
        if let Some(map) = v.as_object() {
            for (id, list) in map {
                let entries = list
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|e| Some((e.get(0)?.as_str()?.to_owned(), e.get(1)?.as_u64()?)))
                            .collect()
                    })
                    .unwrap_or_default();
                log.runtimes.insert(id.clone(), entries);
            }
        }
        log
    }

    /// Notes `version` for `backend`; true if it's new (an update).
    pub fn record(&mut self, backend: &str, version: &str, now: u64) -> bool {
        let list = self.runtimes.entry(backend.to_owned()).or_default();
        if list.last().is_some_and(|(v, _)| v == version) {
            return false;
        }
        list.push((version.to_owned(), now));
        // Keep the last 30.
        let extra = list.len().saturating_sub(30);
        list.drain(..extra);
        true
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = state_file("versions.json").ok_or_else(|| std::io::Error::other("no state folder"))?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let v: serde_json::Map<String, Value> = self
            .runtimes
            .iter()
            .map(|(id, list)| (id.clone(), json!(list.iter().map(|(v, t)| json!([v, t])).collect::<Vec<_>>())))
            .collect();
        fs::write(path, serde_json::to_string_pretty(&Value::Object(v))?)
    }
}

// ---------------------------------------------------------------------------
// Setting profiles
// ---------------------------------------------------------------------------

/// A saved set of a runtime's settings (for Roblox on Windows: its own
/// graphics, sound and camera settings), e.g. "Competitive".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    /// Which runtime's settings these are.
    pub backend: String,
    pub values: BTreeMap<String, Value>,
}

fn profiles_dir() -> Option<PathBuf> {
    crate::paths::config_dir().map(|d| d.join("profiles"))
}

/// A file name that's safe on every system for a profile name.
fn file_name(backend: &str, name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' { c } else { '_' })
        .collect();
    format!("{backend}--{}.json", safe.trim())
}

/// The profiles saved for `backend`, by name.
pub fn profiles(backend: &str) -> Vec<Profile> {
    let Some(dir) = profiles_dir() else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut list: Vec<Profile> = entries
        .filter_map(Result::ok)
        .filter_map(|e| serde_json::from_str::<Value>(&fs::read_to_string(e.path()).ok()?).ok())
        .filter_map(|v| {
            Some(Profile {
                name: v.get("name")?.as_str()?.to_owned(),
                backend: v.get("backend")?.as_str()?.to_owned(),
                values: v
                    .get("values")?
                    .as_object()?
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            })
        })
        .filter(|p| p.backend == backend)
        .collect();
    list.sort_by_key(|p| p.name.to_lowercase());
    list
}

/// Saves the installation's current settings as profile `name` (replacing
/// one of the same name).
pub fn save_profile(installation: &Installation, ctx: &DetectContext, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("give the profile a name".to_owned());
    }
    let file = installation.load_config(ctx).map_err(|e| e.to_string())?;
    let values: serde_json::Map<String, Value> = installation
        .backend
        .settings()
        .iter()
        .map(|def| (def.key.to_owned(), file.get(def)))
        .collect();
    let dir = profiles_dir().ok_or("no settings folder")?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let backend = installation.backend.id();
    let v = json!({ "name": name, "backend": backend, "values": values });
    fs::write(dir.join(file_name(backend, name)), serde_json::to_string_pretty(&v).unwrap_or_default())
        .map_err(|e| e.to_string())
}

pub fn delete_profile(backend: &str, name: &str) {
    if let Some(dir) = profiles_dir() {
        let _ = fs::remove_file(dir.join(file_name(backend, name)));
    }
}

/// Writes a profile's values into the installation's settings file (only
/// settings it still has, with values it accepts). Returns how many changed.
pub fn apply_profile(installation: &Installation, ctx: &DetectContext, profile: &Profile) -> Result<usize, String> {
    if installation.is_running() {
        return Err(ConfigError::Running(installation.backend.name()).to_string());
    }
    let mut file = installation.load_config(ctx).map_err(|e| e.to_string())?;
    let mut changed = 0;
    for def in installation.backend.settings() {
        if let Some(value) = profile.values.get(def.key)
            && def.accepts(value)
            && file.set(def, value.clone()).unwrap_or(false)
        {
            changed += 1;
        }
    }
    if changed > 0 {
        file.save().map_err(|e| e.to_string())?;
    }
    Ok(changed)
}

/// Before a game starts: applies the profile chosen for `place` (or for
/// every other game). Returns a line worth showing, if any.
pub fn apply_game_profile(
    installation: &Installation,
    ctx: &DetectContext,
    settings: &crate::settings::Settings,
    place: Option<u64>,
) -> Option<String> {
    let name = place
        .and_then(|p| settings.game_profiles.get(&p))
        .or_else(|| settings.game_profiles.get(&0))?;
    let Some(profile) = profiles(installation.backend.id()).into_iter().find(|p| &p.name == name) else {
        return Some(format!("The profile \"{name}\" doesn't exist any more."));
    };
    match apply_profile(installation, ctx, &profile) {
        Ok(0) => None,
        Ok(_) => Some(format!("Using your \"{name}\" settings.")),
        Err(e) => Some(format!("The \"{name}\" settings weren't applied: {e}")),
    }
}

// ---------------------------------------------------------------------------
// Servers
// ---------------------------------------------------------------------------

/// One public server of a game.
#[derive(Debug, Clone, PartialEq)]
pub struct Server {
    pub id: String,
    pub playing: u32,
    pub max: u32,
    /// Roblox's own measurement, if it has one.
    pub ping: Option<u32>,
    pub fps: Option<f64>,
}

/// Up to 100 public servers of `place_id` (Roblox's public games API).
pub fn servers(place_id: u64) -> Result<Vec<Server>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(10)))
        .user_agent(format!("Feline/{}", crate::VERSION))
        .build()
        .into();
    let text = agent
        .get(&format!(
            "https://games.roblox.com/v1/games/{place_id}/servers/Public?sortOrder=Desc&excludeFullGames=true&limit=100"
        ))
        .call()
        .map_err(|e| format!("Roblox's server list couldn't be loaded ({e})"))?
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(&text).map_err(|_| "unexpected answer from Roblox".to_owned())?;
    Ok(parse_servers(&v))
}

pub fn parse_servers(v: &Value) -> Vec<Server> {
    v.get("data")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|s| {
                    Some(Server {
                        id: s.get("id")?.as_str()?.to_owned(),
                        playing: s.get("playing").and_then(Value::as_u64).unwrap_or(0) as u32,
                        max: s.get("maxPlayers").and_then(Value::as_u64).unwrap_or(0) as u32,
                        ping: s.get("ping").and_then(Value::as_u64).map(|p| p as u32),
                        fps: s.get("fps").and_then(Value::as_f64),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The link that joins one particular server.
pub fn server_link(place_id: u64, server_id: &str) -> String {
    format!("roblox://experiences/start?placeId={place_id}&gameInstanceId={server_id}")
}

/// Which server a quick join picks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    /// The one with the fewest players (preferring one with somebody in it).
    Emptiest,
    /// The one with the lowest ping Roblox reports.
    BestPing,
}

/// Picks a server from `list`, never `avoid` (the one you're leaving, for
/// a server hop) and never a full one.
pub fn pick_server<'a>(list: &'a [Server], rule: Pick, avoid: Option<&str>) -> Option<&'a Server> {
    let open = list
        .iter()
        .filter(|s| Some(s.id.as_str()) != avoid)
        .filter(|s| s.max == 0 || s.playing < s.max);
    match rule {
        // An empty server is often a dead one: a quiet one with people in
        // it first, then an empty one.
        Pick::Emptiest => open.min_by_key(|s| (s.playing == 0, s.playing)),
        Pick::BestPing => open.min_by_key(|s| (s.ping.is_none(), s.ping.unwrap_or(u32::MAX))),
    }
}

/// The last game server you were on (from Roblox's log while you played),
/// for "Rejoin" and "Server hop".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastServer {
    pub place_id: u64,
    pub job_id: String,
    /// Unix time it was seen.
    pub at: u64,
}

fn last_server_path() -> Option<PathBuf> {
    crate::paths::state_dir().map(|d| d.join("last-server.json"))
}

impl LastServer {
    pub fn load() -> Option<Self> {
        let text = fs::read_to_string(last_server_path()?).ok()?;
        let v: Value = serde_json::from_str(&text).ok()?;
        Some(Self {
            place_id: v.get("place")?.as_u64()?,
            job_id: v.get("job")?.as_str()?.to_owned(),
            at: v.get("at").and_then(Value::as_u64).unwrap_or(0),
        })
        .filter(|s| !s.job_id.is_empty())
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = last_server_path().ok_or_else(|| std::io::Error::other("no state folder"))?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(
            path,
            json!({ "place": self.place_id, "job": self.job_id, "at": self.at }).to_string(),
        )
    }

    /// The link back into this exact server.
    pub fn link(&self) -> String {
        server_link(self.place_id, &self.job_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quick_joins_pick_sensible_servers() {
        let s = |id: &str, playing, max, ping| Server {
            id: id.into(),
            playing,
            max,
            ping,
            fps: None,
        };
        let list = [
            s("dead", 0, 10, Some(20)),
            s("full", 10, 10, Some(10)),
            s("quiet", 2, 10, Some(90)),
            s("busy", 8, 10, Some(40)),
        ];
        assert_eq!(pick_server(&list, Pick::Emptiest, None).unwrap().id, "quiet");
        assert_eq!(pick_server(&list, Pick::Emptiest, Some("quiet")).unwrap().id, "busy");
        assert_eq!(pick_server(&list, Pick::BestPing, None).unwrap().id, "dead");
        assert!(pick_server(&list[1..2], Pick::Emptiest, None).is_none());
    }

    #[test]
    fn status_page_is_read() {
        let html = r#"<div data-section-type="statusbar" style="x"> All Systems Operational <small>Updated a few seconds ago</small></div>
<div data-section-type="components"><b>User</b><span>Website</span><p>Operational</p><span>Mobile App</span><p>Operational</p>
<b>Player</b><span>Games</span><p>Degraded Performance</p><b>Creator</b><span>Studio</span><p>Operational</p></div>
<div data-section-type="history"></div>"#;
        let s = parse_status(html).unwrap();
        assert_eq!(s.headline, "All Systems Operational");
        assert_eq!(s.problems, [("Player Games".to_owned(), "Degraded Performance".to_owned())]);
        assert!(!s.all_good());
    }

    #[test]
    fn versions_are_recorded_once_each() {
        let mut log = VersionLog::default();
        assert!(log.record("roblox", "0.746", 1));
        assert!(!log.record("roblox", "0.746", 2));
        assert!(log.record("roblox", "0.747", 3));
        assert_eq!(log.runtimes["roblox"], [("0.746".to_owned(), 1), ("0.747".to_owned(), 3)]);
    }

    #[test]
    fn servers_parse() {
        let v = json!({"data": [{"id": "abc", "playing": 3, "maxPlayers": 12, "ping": 80, "fps": 59.9}]});
        let s = parse_servers(&v);
        assert_eq!((s[0].id.as_str(), s[0].playing, s[0].max, s[0].ping), ("abc", 3, 12, Some(80)));
        assert_eq!(server_link(1, "abc"), "roblox://experiences/start?placeId=1&gameInstanceId=abc");
    }

    #[test]
    fn profile_file_names_are_safe() {
        assert_eq!(file_name("roblox", "Comp/etitive?"), "roblox--Comp_etitive_.json");
    }
}

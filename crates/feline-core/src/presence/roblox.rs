// SPDX-License-Identifier: GPL-3.0-or-later
//! Looking up game and player names from Roblox's public web API.
//!
//! These endpoints need no login and receive only the public IDs from the log:
//!
//! ```text
//! games.roblox.com/v1/games?universeIds=Z                  name, creator
//! thumbnails.roblox.com/v1/games/icons?universeIds=Z&…      game icon URL
//! users.roblox.com/v1/users/U                               username   (opt-in)
//! thumbnails.roblox.com/v1/users/avatar-headshot?userIds=U  avatar URL (opt-in)
//! ```
//!
//! Every request has a timeout, so a slow network only delays the status.

use std::fmt;
use std::time::Duration;

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameInfo {
    pub name: String,
    /// The user or group that made it; empty if unknown.
    pub creator: String,
    pub icon_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserInfo {
    pub name: String,
    pub display_name: String,
    pub avatar_url: Option<String>,
}

#[derive(Debug)]
pub enum ApiError {
    Http(Box<ureq::Error>),
    /// The answer didn't have the expected shape.
    Unexpected(&'static str),
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http(e) => write!(f, "Roblox's web API couldn't be reached: {e}"),
            Self::Unexpected(what) => write!(f, "unexpected answer from Roblox's web API ({what})"),
        }
    }
}

impl std::error::Error for ApiError {}

impl From<ureq::Error> for ApiError {
    fn from(e: ureq::Error) -> Self {
        // Boxed because ureq's error type is large.
        Self::Http(Box::new(e))
    }
}

pub struct Api {
    agent: ureq::Agent,
}

impl Default for Api {
    fn default() -> Self {
        Self::new()
    }
}

impl Api {
    pub fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(10)))
            .user_agent(format!(
                "Feline/{} ({})",
                crate::VERSION,
                if cfg!(windows) { "Windows" } else { "Linux" }
            ))
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
        }
    }

    fn get_json(&self, url: &str) -> Result<Value, ApiError> {
        let text = self.agent.get(url).call()?.body_mut().read_to_string()?;
        serde_json::from_str(&text).map_err(|_| ApiError::Unexpected("not JSON"))
    }

    /// Name, creator and icon of a universe (a game).
    pub fn game(&self, universe_id: u64) -> Result<GameInfo, ApiError> {
        let games = self.get_json(&format!(
            "https://games.roblox.com/v1/games?universeIds={universe_id}"
        ))?;
        // The icon is nice to have; the status still works without it.
        let icon = self
            .get_json(&format!(
                "https://thumbnails.roblox.com/v1/games/icons?universeIds={universe_id}\
                 &size=512x512&format=Png&isCircular=false"
            ))
            .ok();
        parse_game(&games, icon.as_ref())
    }

    /// Where a game server is, e.g. "Frankfurt, Germany", from its address
    /// (ipinfo.io's free lookup, the service Bloxstrap uses too). Only the
    /// server's address is sent, never anything about the player.
    pub fn server_region(&self, ip: &str) -> Result<String, ApiError> {
        let answer = self.get_json(&format!("https://ipinfo.io/{ip}/json"))?;
        parse_region(&answer).ok_or(ApiError::Unexpected("no location"))
    }

    /// A picture URL for a Roblox image asset (what a game names in its own
    /// Discord status), from Roblox's thumbnail service.
    pub fn asset_image(&self, asset_id: u64) -> Result<String, ApiError> {
        let answer = self.get_json(&format!(
            "https://thumbnails.roblox.com/v1/assets?assetIds={asset_id}&returnPolicy=PlaceHolder&size=512x512&format=Png"
        ))?;
        answer
            .pointer("/data/0/imageUrl")
            .and_then(Value::as_str)
            .filter(|u| u.starts_with("https://"))
            .map(str::to_owned)
            .ok_or(ApiError::Unexpected("no picture"))
    }

    /// The image bytes at `url` (a game icon from Roblox's CDN), at most 4 MB.
    pub fn download(&self, url: &str) -> Result<Vec<u8>, ApiError> {
        Ok(self
            .agent
            .get(url)
            .call()?
            .body_mut()
            .with_config()
            .limit(4 * 1024 * 1024)
            .read_to_vec()?)
    }

    /// Username, display name and avatar of a player.
    pub fn user(&self, user_id: u64) -> Result<UserInfo, ApiError> {
        let user = self.get_json(&format!("https://users.roblox.com/v1/users/{user_id}"))?;
        let avatar = self
            .get_json(&format!(
                "https://thumbnails.roblox.com/v1/users/avatar-headshot?userIds={user_id}\
                 &size=150x150&format=Png&isCircular=false"
            ))
            .ok();
        parse_user(&user, avatar.as_ref())
    }
}

/// "City, Country" from an ipinfo.io answer (the country as its code, e.g.
/// "Frankfurt am Main, DE"; ipinfo's free answer has no country names).
pub fn parse_region(answer: &Value) -> Option<String> {
    let get = |k: &str| answer.get(k)?.as_str().filter(|s| !s.is_empty());
    match (get("city"), get("country")) {
        (Some(city), Some(country)) => Some(format!("{city}, {}", country_name(country))),
        (None, Some(country)) => Some(country_name(country).to_owned()),
        _ => None,
    }
}

/// English names for the countries Roblox has servers in; other codes are
/// shown as they are.
fn country_name(code: &str) -> &str {
    match code {
        "US" => "United States",
        "GB" => "United Kingdom",
        "DE" => "Germany",
        "NL" => "Netherlands",
        "FR" => "France",
        "PL" => "Poland",
        "SG" => "Singapore",
        "JP" => "Japan",
        "IN" => "India",
        "AU" => "Australia",
        "BR" => "Brazil",
        "HK" => "Hong Kong",
        "KR" => "South Korea",
        "CA" => "Canada",
        "MX" => "Mexico",
        "ES" => "Spain",
        "IT" => "Italy",
        "SE" => "Sweden",
        "ZA" => "South Africa",
        other => other,
    }
}

/// The image URL from a thumbnails answer, if Roblox finished making it.
fn thumbnail_url(answer: Option<&Value>) -> Option<String> {
    let entry = answer?.get("data")?.get(0)?;
    if entry.get("state")?.as_str()? != "Completed" {
        return None;
    }
    entry.get("imageUrl")?.as_str().map(str::to_owned)
}

pub fn parse_game(games: &Value, icon: Option<&Value>) -> Result<GameInfo, ApiError> {
    let game = games
        .get("data")
        .and_then(|d| d.get(0))
        .ok_or(ApiError::Unexpected("no game in the answer"))?;
    let name = game
        .get("name")
        .and_then(Value::as_str)
        .ok_or(ApiError::Unexpected("game without a name"))?;
    let creator = game
        .pointer("/creator/name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    Ok(GameInfo {
        name: name.to_owned(),
        creator: creator.to_owned(),
        icon_url: thumbnail_url(icon),
    })
}

pub fn parse_user(user: &Value, avatar: Option<&Value>) -> Result<UserInfo, ApiError> {
    let text = |key| user.get(key).and_then(Value::as_str).map(str::to_owned);
    Ok(UserInfo {
        name: text("name").ok_or(ApiError::Unexpected("user without a name"))?,
        display_name: text("displayName").unwrap_or_default(),
        avatar_url: thumbnail_url(avatar),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_real_answer_shapes() {
        // Shapes as returned by the API (checked with curl), shortened.
        let games = json!({"data": [{"id": 5750914919u64, "rootPlaceId": 16732694052u64,
            "name": "Fisch", "creator": {"id": 7381705, "name": "Fisching", "type": "Group"}}]});
        let icon = json!({"data": [{"targetId": 5750914919u64, "state": "Completed",
            "imageUrl": "https://tr.rbxcdn.com/icon.png"}]});
        let game = parse_game(&games, Some(&icon)).unwrap();
        assert_eq!(game.name, "Fisch");
        assert_eq!(game.creator, "Fisching");
        assert_eq!(
            game.icon_url.as_deref(),
            Some("https://tr.rbxcdn.com/icon.png")
        );

        let pending = json!({"data": [{"state": "Pending", "imageUrl": ""}]});
        assert_eq!(parse_game(&games, Some(&pending)).unwrap().icon_url, None);
        assert!(parse_game(&json!({"data": []}), None).is_err());

        let user = parse_user(&json!({"name": "Roblox", "displayName": "Roblox"}), None).unwrap();
        assert_eq!(user.name, "Roblox");
        assert_eq!(user.avatar_url, None);
    }
}

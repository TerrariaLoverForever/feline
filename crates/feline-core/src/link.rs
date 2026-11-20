// SPDX-License-Identifier: GPL-3.0-or-later
//! Checking the game links users paste into Feline.
//!
//! Accepted:
//! - `roblox://…` and `roblox-player:…` links, passed on unchanged
//! - `robloxmobile://…` links (what the Roblox website opens on Linux,
//!   which it treats like a phone), turned into the same `roblox://…` link:
//!   the two schemes take the same `placeId=…&…` forms
//! - experience pages on the Roblox website, `https://www.roblox.com/games/<id>/…`,
//!   turned into `roblox://experiences/start?placeId=<id>`
//!
//! Anything else is rejected with a message saying why. Private-server links
//! are rejected explicitly instead of silently dropping the server code,
//! which would send the user to a public server instead.
//!
//! The link later becomes a single command-line argument (no shell is
//! involved), and it always starts with `roblox`, so it can never be mistaken
//! for an option like `--something`.

use std::fmt;

/// The Roblox mobile app's link scheme.
const MOBILE_SCHEME: &str = "robloxmobile://";

/// Longest link we accept. Real links are far shorter.
const MAX_LEN: usize = 2048;

/// Roblox website hosts that serve experience pages.
const WEB_HOSTS: &[&str] = &[
    "www.roblox.com",
    "roblox.com",
    "web.roblox.com",
    "m.roblox.com",
];

/// A checked link to a Roblox experience.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameLink {
    /// From a website link: we only know the place ID.
    Experience { place_id: u64 },
    /// A `roblox://` or `roblox-player:` link, kept as-is.
    Uri { uri: String, place_id: Option<u64> },
}

impl GameLink {
    /// The link to hand to a runtime.
    pub fn launch_uri(&self) -> String {
        match self {
            Self::Experience { place_id } => {
                format!("roblox://experiences/start?placeId={place_id}")
            }
            Self::Uri { uri, .. } => uri.clone(),
        }
    }

    pub fn place_id(&self) -> Option<u64> {
        match self {
            Self::Experience { place_id } => Some(*place_id),
            Self::Uri { place_id, .. } => *place_id,
        }
    }
}

/// Why a link was rejected. The Display text is shown to users.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkError {
    Empty,
    TooLong,
    /// Contains spaces or control characters.
    InvalidCharacters,
    /// A Roblox website page that isn't an experience (e.g. a profile).
    NotAnExperience,
    PrivateServerUnsupported,
    NotRoblox,
}

impl fmt::Display for LinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Empty => "Paste a link to get started.",
            Self::TooLong => "This link is too long to be a Roblox link.",
            Self::InvalidCharacters => "Links can't contain spaces or control characters.",
            Self::NotAnExperience => {
                "This Roblox page isn't an experience. Open the experience's page and copy its link."
            }
            Self::PrivateServerUnsupported => "Private server links aren't supported yet.",
            Self::NotRoblox => {
                "This doesn't look like a Roblox link. Use a roblox.com/games/… or roblox:// link."
            }
        })
    }
}

impl std::error::Error for LinkError {}

/// Checks a pasted link.
pub fn parse(input: &str) -> Result<GameLink, LinkError> {
    let link = input.trim();
    if link.is_empty() {
        return Err(LinkError::Empty);
    }
    if link.len() > MAX_LEN {
        return Err(LinkError::TooLong);
    }
    if link.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(LinkError::InvalidCharacters);
    }

    let lower = link.to_ascii_lowercase();
    if lower.contains("privateserverlinkcode=") || lower.contains("type=server") {
        return Err(LinkError::PrivateServerUnsupported);
    }

    if lower.starts_with("roblox://") || lower.starts_with("roblox-player:") {
        return Ok(GameLink::Uri {
            uri: link.to_owned(),
            place_id: query_number(&lower, "placeid"),
        });
    }
    if lower.starts_with(MOBILE_SCHEME) {
        let uri = format!("roblox://{}", &link[MOBILE_SCHEME.len()..]);
        let place_id = query_number(&uri.to_ascii_lowercase(), "placeid");
        return Ok(GameLink::Uri { uri, place_id });
    }

    // Website link, with or without the scheme.
    let rest = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
        .unwrap_or(&lower);
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    if !WEB_HOSTS.contains(&host) {
        return Err(LinkError::NotRoblox);
    }

    // Some links have a language prefix, e.g. roblox.com/de/games/…
    let path = match path.split_once('/') {
        Some((lang, after)) if is_language_code(lang) => after,
        _ => path,
    };
    let id = path
        .strip_prefix("games/")
        .map(|after| after.split(['/', '?', '#']).next().unwrap_or(""))
        .and_then(|digits| digits.parse::<u64>().ok())
        .filter(|id| *id > 0);
    match id {
        Some(place_id) => Ok(GameLink::Experience { place_id }),
        None => Err(LinkError::NotAnExperience),
    }
}

/// Finds `key=<digits>` in a lowercased query string.
fn query_number(lower: &str, key: &str) -> Option<u64> {
    // `scheme://path?key=…` or `scheme://key=…&…` (no path).
    let query = lower.split_once('?').map_or_else(|| lower.split_once("://").map(|(_, q)| q), |(_, q)| Some(q))?;
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then(|| v.parse().ok())?
    })
}

/// "de", "pt-br", … (two letters, optionally a region).
fn is_language_code(segment: &str) -> bool {
    let (lang, region) = segment.split_once('-').unwrap_or((segment, "ok"));
    lang.len() == 2
        && lang.chars().all(|c| c.is_ascii_lowercase())
        && region.len() == 2
        && region.chars().all(|c| c.is_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mobile_links_become_roblox_links() {
        let game = parse("robloxmobile://placeID=920587237&gameInstanceId=abc-123").unwrap();
        assert_eq!(game.launch_uri(), "roblox://placeID=920587237&gameInstanceId=abc-123");
        assert_eq!(game.place_id(), Some(920587237));
        let game = parse("robloxmobile://experiences/start?placeId=1818").unwrap();
        assert_eq!(game.launch_uri(), "roblox://experiences/start?placeId=1818");
        assert_eq!(game.place_id(), Some(1818));
        // The scheme's case doesn't matter; the rest is kept as it was.
        let game = parse("RobloxMobile://placeId=5").unwrap();
        assert_eq!(game.launch_uri(), "roblox://placeId=5");
    }

    #[test]
    fn website_links_become_experience_links() {
        for link in [
            "https://www.roblox.com/games/920587237/Adopt-Me",
            "  https://www.roblox.com/games/920587237  ",
            "http://roblox.com/games/920587237?gameSearchSessionInfo=abc",
            "www.roblox.com/games/920587237",
            "https://web.roblox.com/games/920587237#!/about",
            "https://www.roblox.com/de/games/920587237/Adopt-Me",
        ] {
            let parsed = parse(link).unwrap_or_else(|e| panic!("{link}: {e}"));
            assert_eq!(
                parsed,
                GameLink::Experience {
                    place_id: 920587237
                },
                "{link}"
            );
            assert_eq!(
                parsed.launch_uri(),
                "roblox://experiences/start?placeId=920587237"
            );
        }
    }

    #[test]
    fn roblox_uris_pass_through() {
        let uri = "roblox://experiences/start?placeId=123&launchData=x";
        let parsed = parse(uri).unwrap();
        assert_eq!(parsed.launch_uri(), uri);
        assert_eq!(parsed.place_id(), Some(123));

        let player = "roblox-player:1+launchmode:play";
        assert_eq!(parse(player).unwrap().launch_uri(), player);
        assert_eq!(parse(player).unwrap().place_id(), None);
    }

    #[test]
    fn rejections() {
        assert_eq!(parse("   "), Err(LinkError::Empty));
        assert_eq!(
            parse("https://www.roblox.com/users/1/profile"),
            Err(LinkError::NotAnExperience)
        );
        assert_eq!(
            parse("https://www.roblox.com/games/abc"),
            Err(LinkError::NotAnExperience)
        );
        assert_eq!(
            parse("https://www.roblox.com/games/0"),
            Err(LinkError::NotAnExperience)
        );
        assert_eq!(
            parse("https://example.com/games/123"),
            Err(LinkError::NotRoblox)
        );
        assert_eq!(
            parse("https://roblox.com.evil.example/games/1"),
            Err(LinkError::NotRoblox)
        );
        assert_eq!(parse("--doctor"), Err(LinkError::NotRoblox));
        assert_eq!(parse("roblox://x y"), Err(LinkError::InvalidCharacters));
        assert_eq!(
            parse("https://www.roblox.com/games/1?privateServerLinkCode=abc"),
            Err(LinkError::PrivateServerUnsupported)
        );
        assert_eq!(parse(&"r".repeat(3000)), Err(LinkError::TooLong));
    }
}

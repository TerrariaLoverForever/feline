// SPDX-License-Identifier: GPL-3.0-or-later
//! Optional achievements: little milestones worked out from your play
//! history on this computer (nothing is sent anywhere, and nothing changes
//! in Roblox). Off unless you turn them on.
//!
//! Unlocked ones are remembered in `state/achievements.json` with the time
//! they were unlocked, so clearing the play history doesn't take them away.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde_json::{Map, Value};

use crate::history::{History, now};

/// What the achievements are worked out from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Facts {
    pub total_seconds: u64,
    pub sessions: usize,
    pub games: usize,
    pub longest_session: u64,
    /// The most sessions in one game.
    pub most_sessions_one_game: usize,
    pub favourites: usize,
    /// Played at some point between 2 and 5 at night (local time).
    pub night_owl: bool,
    /// Started a game between 5 and 7 in the morning.
    pub early_bird: bool,
    /// Different days played on.
    pub days: usize,
    /// The longest run of days in a row with play.
    pub streak: usize,
    /// Played on a Saturday and the Sunday after it.
    pub weekend: bool,
    pub mods_on: bool,
    pub desk_cat: bool,
    pub tidied: bool,
    pub own_colour: bool,
}

impl Facts {
    /// The facts from `history`, with local times via `offset_seconds`.
    pub fn from_history(history: &History, offset_seconds: i64) -> Self {
        let mut facts = Self {
            favourites: history.favourites.len(),
            ..Self::default()
        };
        let mut days = BTreeSet::new();
        for s in history.sessions() {
            facts.total_seconds += s.seconds();
            facts.sessions += 1;
            facts.longest_session = facts.longest_session.max(s.seconds());
            let local = |t: u64| t as i64 + offset_seconds;
            let start_hour = local(s.start).rem_euclid(86_400) / 3600;
            if (5..7).contains(&start_hour) {
                facts.early_bird = true;
            }
            // Any minute of the session between 2:00 and 5:00?
            let mut t = local(s.start);
            while t <= local(s.end) {
                let hour = t.rem_euclid(86_400) / 3600;
                if (2..5).contains(&hour) {
                    facts.night_owl = true;
                    break;
                }
                t += 1800;
            }
            days.insert(local(s.start).div_euclid(86_400));
        }
        let games = history.games_played();
        facts.games = games.len();
        facts.most_sessions_one_game = games.iter().map(|g| g.sessions).max().unwrap_or(0);
        facts.days = days.len();
        let mut run = 0;
        let mut previous = None;
        for &day in &days {
            run = if previous == Some(day - 1) { run + 1 } else { 1 };
            facts.streak = facts.streak.max(run);
            // 1970-01-01 was a Thursday, so day 2 was a Saturday.
            if (day - 2).rem_euclid(7) == 0 && days.contains(&(day + 1)) {
                facts.weekend = true;
            }
            previous = Some(day);
        }
        facts
    }
}

/// One achievement.
pub struct Achievement {
    pub id: &'static str,
    pub title: &'static str,
    pub text: &'static str,
    unlocked: fn(&Facts) -> bool,
}

const HOUR: u64 = 3600;

/// Every achievement, roughly from easiest to hardest.
pub const ALL: &[Achievement] = &[
    Achievement {
        id: "first-pounce",
        title: "First pounce",
        text: "Play your first game with Feline.",
        unlocked: |f| f.sessions >= 1,
    },
    Achievement {
        id: "favourite-toy",
        title: "Favourite toy",
        text: "Pin a game to your favourites.",
        unlocked: |f| f.favourites >= 1,
    },
    Achievement {
        id: "catnap",
        title: "Catnap",
        text: "Play for an hour in total.",
        unlocked: |f| f.total_seconds >= HOUR,
    },
    Achievement {
        id: "dressed-up",
        title: "Dressed up",
        text: "Turn on a mod.",
        unlocked: |f| f.mods_on,
    },
    Achievement {
        id: "desk-buddy",
        title: "Desk buddy",
        text: "Let the desk cat out.",
        unlocked: |f| f.desk_cat,
    },
    Achievement {
        id: "own-colours",
        title: "Own colours",
        text: "Pick an accent colour of your own.",
        unlocked: |f| f.own_colour,
    },
    Achievement {
        id: "tidy-whiskers",
        title: "Tidy whiskers",
        text: "Let Feline tidy up on its own.",
        unlocked: |f| f.tidied,
    },
    Achievement {
        id: "nine-lives",
        title: "Nine lives",
        text: "Play 9 different games.",
        unlocked: |f| f.games >= 9,
    },
    Achievement {
        id: "weekend-cat",
        title: "Weekend cat",
        text: "Play on a Saturday and the Sunday after.",
        unlocked: |f| f.weekend,
    },
    Achievement {
        id: "early-bird",
        title: "Early bird",
        text: "Start a game between 5 and 7 in the morning.",
        unlocked: |f| f.early_bird,
    },
    Achievement {
        id: "night-owl",
        title: "Night owl",
        text: "Still playing between 2 and 5 at night. Cats approve.",
        unlocked: |f| f.night_owl,
    },
    Achievement {
        id: "regular",
        title: "Regular",
        text: "Play on 7 different days.",
        unlocked: |f| f.days >= 7,
    },
    Achievement {
        id: "marathon",
        title: "Marathon",
        text: "Stay in one game for 3 hours.",
        unlocked: |f| f.longest_session >= 3 * HOUR,
    },
    Achievement {
        id: "cat-about-town",
        title: "Cat about town",
        text: "Play for 10 hours in total.",
        unlocked: |f| f.total_seconds >= 10 * HOUR,
    },
    Achievement {
        id: "same-spot",
        title: "Same sunny spot",
        text: "Join one game 25 times.",
        unlocked: |f| f.most_sessions_one_game >= 25,
    },
    Achievement {
        id: "explorer",
        title: "Curious cat",
        text: "Play 30 different games.",
        unlocked: |f| f.games >= 30,
    },
    Achievement {
        id: "streak",
        title: "Every day a cat day",
        text: "Play 7 days in a row.",
        unlocked: |f| f.streak >= 7,
    },
    Achievement {
        id: "hundred-hours",
        title: "Old cat",
        text: "Play for 100 hours in total.",
        unlocked: |f| f.total_seconds >= 100 * HOUR,
    },
];

fn store_path() -> Option<PathBuf> {
    crate::paths::state_dir().map(|d| d.join("achievements.json"))
}

/// Unlocked achievements: id → Unix time unlocked.
pub fn load() -> BTreeMap<String, u64> {
    let Some(text) = store_path().and_then(|p| std::fs::read_to_string(p).ok()) else {
        return BTreeMap::new();
    };
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&text) else {
        return BTreeMap::new();
    };
    map.into_iter()
        .filter_map(|(k, v)| Some((k, v.as_u64()?)))
        .collect()
}

fn save(unlocked: &BTreeMap<String, u64>) -> std::io::Result<()> {
    let path = store_path().ok_or_else(|| std::io::Error::other("no state folder"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let map: Map<String, Value> = unlocked
        .iter()
        .map(|(k, v)| (k.clone(), Value::from(*v)))
        .collect();
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, serde_json::to_string_pretty(&Value::Object(map))?)?;
    std::fs::rename(temp, path)
}

/// The achievements `facts` unlock that `unlocked` doesn't have yet.
pub fn newly_unlocked(facts: &Facts, unlocked: &BTreeMap<String, u64>) -> Vec<&'static Achievement> {
    ALL.iter()
        .filter(|a| (a.unlocked)(facts) && !unlocked.contains_key(a.id))
        .collect()
}

/// Works out the achievements from `facts`, remembers new ones, and returns
/// them (to announce) along with everything unlocked so far.
pub fn update(facts: &Facts) -> (Vec<&'static Achievement>, BTreeMap<String, u64>) {
    let mut unlocked = load();
    let new = newly_unlocked(facts, &unlocked);
    if !new.is_empty() {
        let when = now();
        for a in &new {
            unlocked.insert(a.id.to_owned(), when);
        }
        if let Err(e) = save(&unlocked) {
            eprintln!("feline: couldn't save achievements: {e}");
        }
    }
    (new, unlocked)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::Session;

    fn session(place: u64, start: u64, minutes: u64) -> Session {
        Session {
            place_id: place,
            universe_id: None,
            server: None,
            start,
            end: start + minutes * 60,
        }
    }

    #[test]
    fn ids_are_unique() {
        let ids: BTreeSet<_> = ALL.iter().map(|a| a.id).collect();
        assert_eq!(ids.len(), ALL.len());
    }

    #[test]
    fn facts_from_sessions() {
        let mut history = History::default();
        // Saturday 2026-11-07 03:00 UTC for 4 h, Sunday 06:00 for 30 min,
        // Monday 12:00 in another game.
        let saturday = 1_794_020_400;
        history.logs.insert(
            "a".into(),
            (
                0,
                vec![
                    session(1, saturday, 240),
                    session(1, saturday + 86_400 + 3 * 3600, 30),
                    session(2, saturday + 2 * 86_400 + 9 * 3600, 10),
                ],
            ),
        );
        let f = Facts::from_history(&history, 0);
        assert_eq!(f.sessions, 3);
        assert_eq!(f.games, 2);
        assert_eq!(f.days, 3);
        assert_eq!(f.streak, 3);
        assert!(f.weekend);
        assert!(f.night_owl);
        assert!(f.early_bird);
        assert_eq!(f.longest_session, 4 * HOUR);
        assert_eq!(f.most_sessions_one_game, 2);
        let new: Vec<_> = newly_unlocked(&f, &BTreeMap::new()).iter().map(|a| a.id).collect();
        assert!(new.contains(&"first-pounce"));
        assert!(new.contains(&"marathon"));
        assert!(new.contains(&"night-owl"));
        assert!(!new.contains(&"nine-lives"));
        let mut known = BTreeMap::new();
        known.insert("first-pounce".to_owned(), 1);
        assert!(!newly_unlocked(&f, &known).iter().any(|a| a.id == "first-pounce"));
    }
}

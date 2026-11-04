// SPDX-License-Identifier: GPL-3.0-or-later
//! Reading what the Roblox client is doing from its own log file.
//!
//! The Roblox client writes a log (`…_Player_…_last.log`) in its `appData/logs`
//! folder. Feline only *reads* it, the same way Bloxstrap does; nothing about
//! the client is changed. The lines used here were checked in real logs of the
//! Android client running under Sober and Cordial:
//!
//! ```text
//! … [FLog::SingleSurfaceApp] setStage: (stage:LuaApp)                 in the menus
//! … [FLog::Output] ! Joining game '<server id>' place <place id> at … joining
//! … [FLog::GameJoinLoadTime] Report game_join_loadtime: … placeid:P,
//!     userid:U, universeid:Z, …                                          joined
//! … [FLog::SingleSurfaceApp] leaveUGCGameInternal                        left
//! ```
//!
//! Every line starts with an ISO 8601 UTC timestamp, used as the start time.

use std::fs;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Something the client did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogEvent {
    /// Back in the Roblox app's menus (also after leaving a game).
    Menu,
    /// Connecting to a server. `at` is a Unix time in seconds.
    Joining {
        place_id: u64,
        job_id: String,
        at: Option<u64>,
    },
    /// Fully joined; now the universe and the player are known.
    Joined {
        place_id: u64,
        universe_id: u64,
        user_id: u64,
    },
    /// Left the game.
    Left,
}

/// Parses one log line. Most lines aren't interesting and give `None`.
pub fn parse_line(line: &str) -> Option<LogEvent> {
    if line.contains("[FLog::SingleSurfaceApp] setStage: (stage:LuaApp)") {
        return Some(LogEvent::Menu);
    }
    if line.contains("[FLog::SingleSurfaceApp] leaveUGCGameInternal") {
        return Some(LogEvent::Left);
    }
    if let Some(rest) = line.split("! Joining game '").nth(1) {
        let (job_id, rest) = rest.split_once('\'')?;
        let place = rest.trim_start().strip_prefix("place ")?;
        let place_id = place.split_whitespace().next()?.parse().ok()?;
        return Some(LogEvent::Joining {
            place_id,
            job_id: job_id.to_owned(),
            at: line_time(line),
        });
    }
    if line.contains("Report game_join_loadtime:") {
        return Some(LogEvent::Joined {
            place_id: field(line, "placeid:")?,
            universe_id: field(line, "universeid:")?,
            user_id: field(line, "userid:")?,
        });
    }
    None
}

/// The number after `name` in `…, name123, …`.
fn field(line: &str, name: &str) -> Option<u64> {
    let after = line.split(name).nth(1)?;
    let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// The Unix time of a line starting with `2026-11-04T08:24:55.466Z,…`.
pub fn line_time(line: &str) -> Option<u64> {
    let stamp = line.get(..19)?; // "2026-11-04T08:24:55"
    let num = |range: std::ops::Range<usize>| stamp.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hour, minute, second) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let separators_ok = &stamp[4..5] == "-" && &stamp[10..11] == "T" && &stamp[13..14] == ":";
    if !separators_ok || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let secs = days * 86_400 + hour * 3600 + minute * 60 + second;
    u64::try_from(secs).ok()
}

/// Days since 1970-01-01 for a Gregorian date (Howard Hinnant's algorithm),
/// so we don't need a date library for one conversion.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

// ---------------------------------------------------------------------------
// Following the newest log file
// ---------------------------------------------------------------------------

/// Follows the Roblox log of the current session, like `tail -f`: each call to
/// [`LogFollower::poll`] returns the events written since the last call.
pub struct LogFollower {
    roots: Vec<PathBuf>,
    /// Only logs changed after this belong to the session Feline started.
    since: SystemTime,
    current: Option<PathBuf>,
    offset: u64,
    /// The end of a line that wasn't complete yet at the last read.
    partial: String,
}

impl LogFollower {
    pub fn new(roots: Vec<PathBuf>, since: SystemTime) -> Self {
        Self {
            roots,
            since,
            current: None,
            offset: 0,
            partial: String::new(),
        }
    }

    /// The log being followed, once one was found.
    pub fn current(&self) -> Option<&Path> {
        self.current.as_deref()
    }

    /// New events since the last call. Switches to a newer log file when the
    /// client starts one (Roblox writes one per session).
    pub fn poll(&mut self) -> Vec<LogEvent> {
        if let Some(newest) = newest_log(&self.roots, self.since)
            && self.current.as_ref() != Some(&newest)
        {
            self.current = Some(newest);
            self.offset = 0;
            self.partial.clear();
        }
        let Some(path) = &self.current else {
            return Vec::new();
        };
        let Ok(mut file) = fs::File::open(path) else {
            return Vec::new();
        };
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);
        if len < self.offset {
            // The file was truncated or replaced: start over.
            self.offset = 0;
            self.partial.clear();
        }
        if file.seek(SeekFrom::Start(self.offset)).is_err() {
            return Vec::new();
        }
        let mut bytes = Vec::new();
        if file.read_to_end(&mut bytes).is_err() {
            return Vec::new();
        }
        self.offset += bytes.len() as u64;

        // Log lines may contain bytes that aren't valid UTF-8; keep the rest.
        self.partial.push_str(&String::from_utf8_lossy(&bytes));
        let mut events = Vec::new();
        while let Some(end) = self.partial.find('\n') {
            let line: String = self.partial.drain(..=end).collect();
            if let Some(event) = parse_line(&line) {
                events.push(event);
            }
        }
        events
    }
}

/// The most recently changed Roblox player log under `roots` (searched a few
/// levels deep, since runtimes keep them in different places), if it was
/// changed after `since`.
pub fn newest_log(roots: &[PathBuf], since: SystemTime) -> Option<PathBuf> {
    let mut best: Option<(SystemTime, PathBuf)> = None;
    for root in roots {
        visit(root, 6, &mut |path| {
            let is_log = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.contains("_Player_") && n.ends_with(".log"));
            if !is_log {
                return;
            }
            let Ok(modified) = path.metadata().and_then(|m| m.modified()) else {
                return;
            };
            if modified >= since && best.as_ref().is_none_or(|(t, _)| modified > *t) {
                best = Some((modified, path.to_owned()));
            }
        });
    }
    best.map(|(_, path)| path)
}

/// Calls `f` for every file under `dir`, at most `depth` folders deep.
/// Symbolic links to folders aren't followed, so there are no loops.
fn visit(dir: &Path, depth: u32, f: &mut impl FnMut(&Path)) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() {
            if depth > 0 {
                visit(&path, depth - 1, f);
            }
        } else if kind.is_file() {
            f(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    // Real lines from a Sober session (user ID and server address replaced).
    const MENU: &str = "2026-11-04T08:22:16.037Z,2.037541,e93d36c0,6 [FLog::SingleSurfaceApp] setStage: (stage:LuaApp)\n";
    const JOINING: &str = "2026-11-04T08:24:55.466Z,161.466614,7c8016c0,6 [FLog::Output] ! Joining game '6b1f335e-3cda-496e-9049-366bc1c79cf2' place 16732694052 at 10.0.0.1\n";
    const JOINED: &str = "2026-11-04T08:24:55.466Z,161.4,7c8016c0,6 [FLog::GameJoinLoadTime] Report game_join_loadtime: sid:939f, clienttime:1793780695.56, join_time:0.96, referral_page:, placeid:16732694052, userid:123, universeid:5750914919, \n";
    const LEFT: &str = "2026-11-04T08:50:19.210Z,1685.2,e93d36c0,6 [FLog::SingleSurfaceApp] leaveUGCGameInternal\n";

    #[test]
    fn parses_real_lines() {
        assert_eq!(parse_line(MENU), Some(LogEvent::Menu));
        assert_eq!(
            parse_line(JOINING),
            Some(LogEvent::Joining {
                place_id: 16732694052,
                job_id: "6b1f335e-3cda-496e-9049-366bc1c79cf2".into(),
                at: line_time(JOINING),
            })
        );
        assert_eq!(
            parse_line(JOINED),
            Some(LogEvent::Joined {
                place_id: 16732694052,
                universe_id: 5750914919,
                user_id: 123,
            })
        );
        assert_eq!(parse_line(LEFT), Some(LogEvent::Left));
        assert_eq!(
            parse_line("2026-11-04T08:22:15Z [FLog::Graphics] Vulkan"),
            None
        );
        // Other stages aren't the menus.
        assert_eq!(
            parse_line("[FLog::SingleSurfaceApp] setStage: (stage:UGCGame)"),
            None
        );
    }

    #[test]
    fn timestamps() {
        // 2026-11-04T08:24:55Z, checked with `date -u -d … +%s`.
        assert_eq!(line_time(JOINING), Some(1_793_780_695));
        assert_eq!(line_time("1970-01-01T00:00:00.000Z,"), Some(0));
        assert_eq!(line_time("2024-02-29T12:00:00Z"), Some(1_709_208_000));
        assert_eq!(line_time("not a timestamp at all"), None);
    }

    #[test]
    fn follows_appended_lines_and_partial_writes() {
        let dir = std::env::temp_dir().join(format!("feline-test-log-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let logs = dir.join("appData/logs");
        fs::create_dir_all(&logs).unwrap();
        let since = SystemTime::now() - std::time::Duration::from_secs(5);
        let mut follower = LogFollower::new(vec![dir.clone()], since);
        let no_log_yet = follower.poll();

        let path = logs.join("2.741.0.1061_20261104T082214Z_Player_c3926_last.log");
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(MENU.as_bytes()).unwrap();
        // Half a line: must wait for the rest.
        file.write_all(&JOINING.as_bytes()[..40]).unwrap();
        let first = follower.poll();
        file.write_all(&JOINING.as_bytes()[40..]).unwrap();
        file.write_all(LEFT.as_bytes()).unwrap();
        let second = follower.poll();
        let found = follower.current().map(Path::to_owned);
        let _ = fs::remove_dir_all(&dir);

        assert!(no_log_yet.is_empty());
        assert_eq!(first, [LogEvent::Menu]);
        assert!(matches!(
            second[0],
            LogEvent::Joining {
                place_id: 16732694052,
                ..
            }
        ));
        assert_eq!(second[1], LogEvent::Left);
        assert_eq!(found, Some(path));
    }
}

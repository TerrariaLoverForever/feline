// SPDX-License-Identifier: GPL-3.0-or-later
//! The crash detective: reads Roblox's own logs and says how each recent
//! play session ended (closed normally, disconnected and why, or closed
//! unexpectedly), then looks for patterns and says what to try.
//!
//! What Roblox writes (2025–2026 clients):
//! - `[FLog::Network] Client has been disconnected with reason: <text>`
//! - older clients: `Disconnection Notification. Reason: 277` and
//!   `Sending disconnect with reason: 277`
//! - `[FLog::SingleSurfaceApp] shutDown:` when the app closes normally.
//! - `HttpError: SslConnectFail url:{ "https://host/…" }` when something
//!   blocks or intercepts a Roblox address.
//!
//! A log without `shutDown:` whose Roblox isn't running any more means the
//! app ended without closing: a crash, a forced close or a power cut. On
//! Windows, a crash report in `logs\crashes` from that time confirms a crash.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::presence::log::line_time;
use crate::system::{Check, CheckLevel};

/// How a session ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ending {
    /// Roblox closed normally.
    Normal,
    /// Roblox is still running (the newest log).
    Running,
    /// Roblox ended without closing. `true` if a crash report confirms it.
    Unexpected { confirmed: bool },
    /// Feline closed it after the player left the game ("Close Roblox when
    /// you leave a game"): not a crash.
    ClosedByFeline,
}

/// Where Feline notes the logs of the Roblox runs it closed itself.
fn closed_list_path() -> Option<PathBuf> {
    crate::paths::state_dir().map(|d| d.join("closed-by-feline.txt"))
}

/// The file names of the logs whose Roblox Feline closed.
fn closed_by_feline() -> Vec<String> {
    closed_list_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| t.lines().map(str::to_owned).collect())
        .unwrap_or_default()
}

/// Notes that Feline closed the Roblox writing `log`, so the detective
/// doesn't call it a crash. Keeps the last 100.
pub fn remember_closed(log: &Path) {
    let Some(name) = log.file_name().map(|n| n.to_string_lossy().into_owned()) else {
        return;
    };
    let Some(path) = closed_list_path() else { return };
    let mut list = closed_by_feline();
    if list.contains(&name) {
        return;
    }
    list.push(name);
    let skip = list.len().saturating_sub(100);
    let _ = std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")));
    let _ = std::fs::write(&path, list[skip..].join("\n") + "\n");
}

/// One Roblox run, from one log file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Case {
    pub log: PathBuf,
    /// Unix times of the first and last line.
    pub start: u64,
    pub end: u64,
    /// The last game joined, if any.
    pub place_id: Option<u64>,
    pub universe_id: Option<u64>,
    /// Why the game server disconnected (readable text), in order.
    pub disconnects: Vec<String>,
    pub ending: Ending,
    /// Hosts a request to failed, with the error ("ecsv2.roblox.com",
    /// "SslConnectFail").
    pub blocked: Vec<(String, String)>,
    pub out_of_memory: bool,
}

/// Roblox's numeric disconnect codes, in words.
pub fn code_meaning(code: u32) -> Option<&'static str> {
    Some(match code {
        264 | 273 => "Your account joined from another device",
        267 => "Kicked by the game",
        268 => "Unexpected client behaviour (anti-cheat)",
        277 => "Lost connection to the game server",
        278 => "Idle for 20 minutes",
        279 => "Couldn't connect to the game server",
        524 => "Not allowed to join this server",
        529 => "Roblox's servers had a problem (HTTP error)",
        _ => return None,
    })
}

/// What to do about a disconnect reason.
pub fn advice_for(reason: &str) -> &'static str {
    let r = reason.to_lowercase();
    if r.contains("lost connection") || r.contains("couldn't connect") || r.contains("failed to connect") {
        "Your connection to the game dropped. Run the network test; Wi-Fi drops, VPNs and busy downloads are the usual causes."
    } else if r.contains("another device") || r.contains("same account") {
        "Your account was used somewhere else. If that wasn't you, change your Roblox password."
    } else if r.contains("kick") {
        "The game (or one of its moderators) removed you. Not a problem with your PC."
    } else if r.contains("idle") {
        "Roblox disconnects players who don't move for 20 minutes."
    } else if r.contains("shut") || r.contains("restart") || r.contains("update") {
        "The game's server closed, usually for an update. Join again."
    } else if r.contains("unexpected client") {
        "Roblox's anti-cheat didn't like something. Turn off mods and FastFlag tweaks, then try again."
    } else if r.contains("http") || r.contains("roblox's servers") {
        "Roblox itself had trouble. Check Roblox's status on the Roblox page."
    } else {
        ""
    }
}

/// The disconnect reason on a line, if it has one.
fn disconnect_on(line: &str) -> Option<String> {
    if let Some(at) = line.find("disconnected with reason:") {
        let text = line[at + "disconnected with reason:".len()..].trim();
        return Some(text.trim_end_matches('.').to_owned());
    }
    for marker in ["Disconnection Notification. Reason:", "Sending disconnect with reason:"] {
        if let Some(at) = line.find(marker) {
            let code: String = line[at + marker.len()..]
                .trim()
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            let code: u32 = code.parse().ok()?;
            return Some(match code_meaning(code) {
                Some(text) => format!("{text} (code {code})"),
                None => format!("Disconnected (code {code})"),
            });
        }
    }
    None
}

/// `HttpError: Kind url:{ "https://host/…" }` → (host, kind).
fn blocked_on(line: &str) -> Option<(String, String)> {
    let at = line.find("message:HttpError: ")? + "message:HttpError: ".len();
    let kind: String = line[at..].chars().take_while(char::is_ascii_alphanumeric).collect();
    if !kind.contains("Connect") && !kind.contains("Dns") && !kind.contains("Resolve") {
        return None;
    }
    let url = line.find("https://")? + "https://".len();
    let host: String = line[url..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-')
        .collect();
    (!host.is_empty()).then_some((host, kind))
}

/// Reads one log's text.
pub fn read_case(log: PathBuf, text: &str) -> Case {
    let mut case = Case {
        log,
        start: 0,
        end: 0,
        place_id: None,
        universe_id: None,
        disconnects: Vec::new(),
        ending: Ending::Unexpected { confirmed: false },
        blocked: Vec::new(),
        out_of_memory: false,
    };
    let mut closed = false;
    for line in text.lines() {
        if let Some(t) = line_time(line) {
            if case.start == 0 {
                case.start = t;
            }
            case.end = t;
        }
        if line.contains("shutDown:") {
            closed = true;
        }
        if let Some(reason) = disconnect_on(line) {
            case.disconnects.push(reason);
        }
        if let Some(found) = blocked_on(line)
            && !case.blocked.contains(&found)
        {
            case.blocked.push(found);
        }
        if line.contains("Out of memory") || line.contains("OutOfMemory") {
            case.out_of_memory = true;
        }
        if let Some(crate::presence::log::LogEvent::Joining { place_id, .. }) =
            crate::presence::log::parse_line(line)
        {
            case.place_id = Some(place_id);
            case.universe_id = None;
        }
        if let Some(crate::presence::log::LogEvent::Joined { universe_id, .. }) =
            crate::presence::log::parse_line(line)
        {
            case.universe_id = Some(universe_id);
        }
    }
    if closed {
        case.ending = Ending::Normal;
    }
    case
}

fn modified(path: &Path) -> u64 {
    path.metadata()
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs())
}

/// Times of Roblox's crash reports (Crashpad's `*.dmp` files).
fn crash_report_times(dirs: &[PathBuf]) -> Vec<u64> {
    fn walk(dir: &Path, depth: u32, out: &mut Vec<u64>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() && depth > 0 {
                walk(&path, depth - 1, out);
            } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("dmp")) {
                out.push(modified(&path));
            }
        }
    }
    let mut out = Vec::new();
    for dir in dirs {
        walk(&dir.join("crashes"), 3, &mut out);
    }
    out
}

/// Whether a log is a Roblox player's (not Studio's, not the crash handler's).
fn is_player_log(path: &Path) -> bool {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let lower = name.to_lowercase();
    (lower.ends_with(".log") || lower.ends_with(".txt"))
        && !lower.contains("crashhandler")
        && !lower.contains("studio")
        && !lower.contains("installer")
        && !lower.contains("bootstrapper")
}

/// Looks at the newest `limit` Roblox logs in `dirs`. `running`: Roblox
/// is running right now (its newest log isn't finished yet).
pub fn investigate(dirs: &[PathBuf], running: bool, limit: usize) -> Vec<Case> {
    let mut logs: Vec<(u64, PathBuf)> = dirs
        .iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flat_map(|entries| entries.filter_map(Result::ok).map(|e| e.path()))
        .filter(|p| p.is_file() && is_player_log(p))
        .map(|p| (modified(&p), p))
        .collect();
    logs.sort_by_key(|l| std::cmp::Reverse(l.0));
    logs.truncate(limit);
    let reports = crash_report_times(dirs);
    let closed = closed_by_feline();
    let mut cases: Vec<Case> = logs
        .into_iter()
        .filter_map(|(_, path)| {
            let bytes = std::fs::read(&path).ok()?;
            Some(read_case(path, &String::from_utf8_lossy(&bytes)))
        })
        .filter(|c| c.start > 0)
        // A start that ended within seconds without a game is Roblox
        // handing over to its updater (or being closed on purpose).
        .filter(|c| c.place_id.is_some() || c.end.saturating_sub(c.start) >= 15)
        .collect();
    for (i, case) in cases.iter_mut().enumerate() {
        if case.ending == Ending::Normal {
            continue;
        }
        if i == 0 && running {
            case.ending = Ending::Running;
            continue;
        }
        if case
            .log
            .file_name()
            .is_some_and(|n| closed.iter().any(|c| *c == n.to_string_lossy()))
        {
            case.ending = Ending::ClosedByFeline;
            continue;
        }
        let confirmed = reports
            .iter()
            .any(|&t| t + 120 >= case.start && t <= case.end + 600);
        case.ending = Ending::Unexpected { confirmed };
    }
    cases
}

/// Addresses that only collect telemetry: blocking them is harmless.
fn is_telemetry(host: &str) -> bool {
    ["ecsv2.", "ephemeralcounters.", "client-telemetry.", "metrics.", "analytics.", "lms.", "tracing."]
        .iter()
        .any(|p| host.starts_with(p))
}

/// The patterns across all cases, most serious first.
pub fn conclusions(cases: &[Case]) -> Vec<Check> {
    let mut out = Vec::new();
    if cases.is_empty() {
        out.push(Check::new(
            CheckLevel::Ok,
            "No Roblox logs yet",
            "Play a game, then come back: the detective reads Roblox's own logs.",
        ));
        return out;
    }
    let crashes: Vec<&Case> = cases
        .iter()
        .filter(|c| matches!(c.ending, Ending::Unexpected { .. }))
        .collect();
    if !crashes.is_empty() {
        let confirmed = crashes
            .iter()
            .any(|c| matches!(c.ending, Ending::Unexpected { confirmed: true }));
        let mut detail = String::from(
            "Things to try, easiest first: update the graphics driver (System page), clear the shader cache (Settings → Storage), turn off mods, lower Graphics quality, then Repair Roblox (Roblox page).",
        );
        if crashes.iter().any(|c| c.out_of_memory) {
            detail.insert_str(0, "Roblox ran out of memory: close other apps while you play (browsers are the usual memory hogs). ");
        }
        if !confirmed {
            detail.push_str(" (No crash report was saved, so it may also have been closed by force or a shutdown.)");
        }
        out.push(Check::new(
            if crashes.len() >= 2 { CheckLevel::Problem } else { CheckLevel::Warning },
            if crashes.len() == 1 {
                "Roblox closed unexpectedly once".to_owned()
            } else {
                format!("Roblox closed unexpectedly {} times", crashes.len())
            },
            detail,
        ));
    }

    let lost = cases
        .iter()
        .flat_map(|c| &c.disconnects)
        .filter(|r| advice_for(r).starts_with("Your connection"))
        .count();
    if lost > 0 {
        out.push(Check::new(
            if lost >= 2 { CheckLevel::Warning } else { CheckLevel::Ok },
            format!("Lost the game's connection {lost} time{}", if lost == 1 { "" } else { "s" }),
            advice_for("lost connection"),
        ));
    }

    let mut hosts: Vec<&str> = cases
        .iter()
        .flat_map(|c| &c.blocked)
        .map(|(h, _)| h.as_str())
        .collect();
    hosts.sort_unstable();
    hosts.dedup();
    let (telemetry, important): (Vec<&str>, Vec<&str>) = hosts.into_iter().partition(|h| is_telemetry(h));
    if !important.is_empty() {
        out.push(Check::new(
            CheckLevel::Warning,
            "Some Roblox addresses are blocked",
            format!(
                "Roblox couldn't reach {}. An ad-blocker, DNS filter, VPN or antivirus may block them; parts of Roblox can break. Run the network test.",
                important.join(", ")
            ),
        ));
    }
    if !telemetry.is_empty() {
        out.push(Check::new(
            CheckLevel::Ok,
            "Roblox's telemetry is blocked",
            format!(
                "Something on this PC or network (an ad-blocker, DNS filter or antivirus) blocks {}. Those only collect usage statistics, so blocking them is harmless.",
                telemetry.join(", ")
            ),
        ));
    }

    if crashes.is_empty() && lost == 0 {
        out.insert(
            0,
            Check::new(
                CheckLevel::Ok,
                format!("No crashes in your last {} Roblox sessions", cases.len()),
                "Purr. Roblox has been closing normally.",
            ),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const CRASHED: &str = "\
2026-11-09T22:22:01.899Z,1.899,0d90,6 [FLog::Output] start
2026-11-09T22:36:29.018Z,869.0,39f0,6 [FLog::Output] ! Joining game 'abc' place 129279692364812 at 10.9.1.100
2026-11-09T22:36:29.019Z,869.0,39f0,6 [FLog::GameJoinLoadTime] Report game_join_loadtime: placeid:129279692364812, join_time:1.8, universeid:7623259179, referral_page:, userid:1
2026-11-09T22:40:00.000Z,900.0,1a44,12 [DFLog::HttpTraceError] HttpResponse(#11) time:1.0ms error:10 message:HttpError: SslConnectFail url:{ \"https://ecsv2.roblox.com/pe?t=x\" }
2026-11-09T23:02:29.096Z,2429.0,43ac,7 [FLog::Network] Client has been disconnected with reason: Lost connection to the game server, please reconnect
";

    #[test]
    fn a_log_tells_its_story() {
        let case = read_case(PathBuf::from("x.log"), CRASHED);
        assert_eq!(case.place_id, Some(129279692364812));
        assert_eq!(case.universe_id, Some(7623259179));
        assert_eq!(case.disconnects, ["Lost connection to the game server, please reconnect"]);
        assert_eq!(case.blocked, [("ecsv2.roblox.com".to_owned(), "SslConnectFail".to_owned())]);
        assert_eq!(case.ending, Ending::Unexpected { confirmed: false });
        assert!(case.end > case.start);

        let closed = format!("{CRASHED}2026-11-09T23:17:40.875Z,3340.8,0d90,6 [FLog::SingleSurfaceApp] shutDown: (stage:LuaApp).\n");
        assert_eq!(read_case(PathBuf::from("x.log"), &closed).ending, Ending::Normal);
    }

    #[test]
    fn old_codes_are_explained() {
        assert_eq!(
            disconnect_on("[FLog::Network] Disconnection Notification. Reason: 277").as_deref(),
            Some("Lost connection to the game server (code 277)")
        );
        assert_eq!(
            disconnect_on("Sending disconnect with reason: 9999").as_deref(),
            Some("Disconnected (code 9999)")
        );
        assert!(advice_for("You were kicked from this experience").starts_with("The game"));
    }

    #[test]
    fn patterns_become_advice() {
        let case = read_case(PathBuf::from("x.log"), CRASHED);
        let found = conclusions(&[case.clone(), case]);
        assert_eq!(found[0].level, CheckLevel::Problem);
        assert!(found[0].title.contains("2 times"));
        assert!(found.iter().any(|c| c.title == "Roblox's telemetry is blocked"));
        assert!(found.iter().any(|c| c.title.starts_with("Lost the game's connection 2")));
    }

    #[test]
    fn only_player_logs_count() {
        assert!(is_player_log(Path::new("0.746_Player_2628E_last.log")));
        assert!(!is_player_log(Path::new("0.746_Player_7DB29_CrashHandler_last.log")));
        assert!(!is_player_log(Path::new("0.746_Studio_1_last.log")));
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! Joining a game from a link: `feline-runtime --play <libroblox.so> --join <link>`.
//!
//! On Android, a `roblox://` link opens Roblox's app, and its Java code
//! starts the game with `NativeGLInterface.nativeAppBridgeV2StartGameWithParam`
//! and a `StartGameParams` object. Feline does the same once the app has
//! reached its Home screen (so the login is in place): it answers the
//! engine's questions about `StartGameParams` with the link's values (see
//! `jni::set_known_number` / `set_known_string`) and calls the native once,
//! from the window loop (the main thread).
//!
//! Which links carry which values, the names of `StartGameParams`' fields
//! and the join request types follow Mocktail
//! (<https://github.com/komaruworld/mocktail>, Apache-2.0,
//! src/runtime/roblox_launch_uri.cc, roblox_fresh_game_launch_controller.cc
//! and roblox_experience_composition.cc's `JoinRequestTypeFor`); the code is
//! Feline's own.
//!
//! Unsafe: calling the engine's native through its address.
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::elf::Elf;
use crate::load::Image;

const GL: &str = "com/roblox/engine/jni/NativeGLInterface";
/// The abstract class the engine reads, and the class Java builds.
const PARAMS: [&str; 2] = [
    "com/roblox/engine/jni/autovalue/StartGameParams",
    "com/roblox/engine/jni/autovalue/AutoValue_StartGameParams",
];
/// Longest link (and value in it) accepted. Real ones are far shorter.
const MAX_LEN: usize = 16 * 1024;
/// How long Home has to be up before the game starts (the app settles).
const SETTLE: Duration = Duration::from_millis(1500);

/// A game to join, from a link.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Request {
    pub place_id: u64,
    /// A friend to follow (join request type 1).
    pub user_id: u64,
    pub conversation_id: u64,
    pub referred_by_player_id: u64,
    /// `StartGameParams` string fields (field name, value), non-empty only.
    pub strings: Vec<(&'static str, String)>,
}

impl Request {
    fn string(&self, field: &str) -> &str {
        self.strings
            .iter()
            .find(|(f, _)| *f == field)
            .map_or("", |(_, v)| v.as_str())
    }

    /// The kind of join, as Roblox's Java code tells the engine.
    pub fn join_request_type(&self) -> i32 {
        if self.conversation_id > 0 {
            6
        } else if self.user_id > 0 {
            1
        } else if !self.string("accessCode").is_empty() || !self.string("linkCode").is_empty() {
            2
        } else if !self.string("gameId").is_empty() {
            3
        } else if !self.string("reservedServerAccessCode").is_empty() {
            8
        } else {
            0
        }
    }
}

/// A link's parameter name (lowercase) → the `StartGameParams` field it
/// fills. Numbers are handled separately.
fn string_field(key: &str) -> Option<&'static str> {
    Some(match key {
        "gameid" | "gameinstanceid" | "instanceid" => "gameId",
        "reservedserveraccesscode" => "reservedServerAccessCode",
        "callid" => "callId",
        "referralpage" => "referralPage",
        "accesscode" => "accessCode",
        "linkcode" | "privateserverlinkcode" => "linkCode",
        "launchdata" => "launchData",
        "eventid" => "eventId",
        "gamejoincontext" => "gameJoinContext",
        "joinattemptid" => "joinAttemptId",
        "joinattemptorigin" => "joinAttemptOrigin",
        "isocontext" => "isoContext",
        _ => return None,
    })
}

/// Reads a game link:
///
/// - `roblox://experiences/start?placeId=…&gameInstanceId=…` (the website
///   and Feline's own links), also `roblox://placeId=…&…`;
/// - `roblox-player:1+launchmode:play+…+placelauncherurl:<PlaceLauncher URL>+…`
///   (the website's Play button for the desktop client).
pub fn parse(link: &str) -> Result<Request, String> {
    if link.len() > MAX_LEN || link.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("the link is too long or has spaces in it".into());
    }
    let lower = link.to_ascii_lowercase();
    let mut request = Request::default();
    if lower.starts_with("roblox://") {
        let rest = &link["roblox://".len()..];
        let query = if rest.to_ascii_lowercase().starts_with("experiences/start?") {
            &rest["experiences/start?".len()..]
        } else if rest.to_ascii_lowercase().starts_with("placeid=") {
            rest
        } else {
            return Err("this roblox:// link doesn't start a game".into());
        };
        read_query(query, true, &mut request)?;
    } else if lower.starts_with("roblox-player:") {
        let mut launcher = None;
        for part in link["roblox-player:".len()..].split('+') {
            let (key, value) = part.split_once(':').unwrap_or((part, ""));
            match key.to_ascii_lowercase().as_str() {
                "launchmode" if !value.eq_ignore_ascii_case("play") => {
                    return Err(format!("the link asks for launch mode \"{value}\", not a game"));
                }
                "placelauncherurl" => launcher = Some(decode(value, false)?),
                _ => {}
            }
        }
        let url = launcher.ok_or("the link has no game in it")?;
        let lower = url.to_ascii_lowercase();
        let rest = lower.strip_prefix("https://").ok_or("the link's game address isn't https")?;
        let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
        let path = path.split('?').next().unwrap_or("");
        if !matches!(host, "www.roblox.com" | "roblox.com" | "assetgame.roblox.com")
            || path != "game/placelauncher.ashx"
        {
            return Err("the link's game address isn't Roblox's".into());
        }
        let query = url.split_once('?').map_or("", |(_, q)| q);
        read_query(query, false, &mut request)?;
    } else {
        return Err("not a Roblox link".into());
    }
    if request.place_id == 0 {
        return Err(if request.user_id > 0 {
            "following a friend from a link isn't supported yet".into()
        } else {
            "the link doesn't say which game".into()
        });
    }
    Ok(request)
}

/// `key=value&…`: fills `request`. Unknown keys are ignored.
fn read_query(query: &str, modern: bool, request: &mut Request) -> Result<(), String> {
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let key = decode(key, true)?.to_ascii_lowercase();
        let mut value = decode(value, true)?;
        let number = |name: &str| {
            value.parse::<u64>().map_err(|_| format!("the link's {name} isn't a number"))
        };
        match key.as_str() {
            "placeid" => request.place_id = number("placeId")?,
            "userid" => request.user_id = number("userId")?,
            "conversationid" => request.conversation_id = number("conversationId")?,
            "referredbyplayerid" => request.referred_by_player_id = number("referredByPlayerId")?,
            _ => {
                let Some(field) = string_field(&key) else {
                    continue;
                };
                // Newer links encode launchData twice.
                if modern && field == "launchData" && value.contains('%') {
                    value = decode(&value, false)?;
                }
                if !value.is_empty() {
                    request.strings.retain(|(f, _)| *f != field);
                    request.strings.push((field, value));
                }
            }
        }
    }
    Ok(())
}

/// Percent-decoding (and `+` as a space in query values).
fn decode(text: &str, plus_is_space: bool) -> Result<String, String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = match bytes[i] {
            b'%' => {
                let hex = text.get(i + 1..i + 3).ok_or("bad %-escape in the link")?;
                i += 2;
                u8::from_str_radix(hex, 16).map_err(|_| "bad %-escape in the link")?
            }
            b'+' if plus_is_space => b' ',
            b => b,
        };
        if b < 0x20 || b == 0x7f {
            return Err("the link has control characters in it".into());
        }
        out.push(b);
        i += 1;
    }
    String::from_utf8(out).map_err(|_| "the link isn't valid text".into())
}

struct State {
    request: Option<Request>,
    /// When the app first showed Home.
    home_at: Option<Instant>,
    username: String,
}

static STATE: Mutex<State> = Mutex::new(State {
    request: None,
    home_at: None,
    username: String::new(),
});
/// The engine's `nativeAppBridgeV2StartGameWithParam` (0 until found).
static START_GAME: AtomicUsize = AtomicUsize::new(0);

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|p| p.into_inner())
}

/// The game to join once the app is ready.
pub fn set(request: Request) {
    eprintln!(
        "feline-runtime: will join place {} once the app is ready (join type {})",
        request.place_id,
        request.join_request_type()
    );
    state().request = Some(request);
}

/// The logged-in user's name (StartGameParams carries it).
pub fn logged_in(username: &str) {
    state().username = username.to_owned();
}

/// Finds the native. Call once the app has started.
pub fn remember_native(elf: &Elf, image: &Image) -> Result<(), String> {
    if state().request.is_none() {
        return Ok(());
    }
    let name = crate::init::jni_symbol(GL, "nativeAppBridgeV2StartGameWithParam");
    let symbols = elf.symbols().map_err(|e| e.to_string())?;
    let entry = symbols
        .iter()
        .find(|s| s.defined && s.name == name)
        .ok_or_else(|| format!("no {name}"))?;
    START_GAME.store((image.bias() + entry.value) as usize, Ordering::Release);
    Ok(())
}

/// Roblox's app showed `screen` (from `gameActivity_onAppReady`).
pub fn app_shown(screen: &str) {
    let mut s = state();
    if s.request.is_none() {
        return;
    }
    match screen {
        "Home" if s.home_at.is_none() => s.home_at = Some(Instant::now()),
        // Logged out: the game can't be joined; the app's login comes first.
        "Landing" | "Login" if s.home_at.is_none() => {
            eprintln!("feline-runtime: not logged in yet; the game from the link starts after you sign in");
        }
        _ => {}
    }
}

/// From the window loop: starts the game once Home has been up a moment.
pub fn tick(env: *mut c_void) {
    let address = START_GAME.load(Ordering::Acquire);
    if address == 0 {
        return;
    }
    let (request, username) = {
        let mut s = state();
        if s.home_at.is_none_or(|at| at.elapsed() < SETTLE) {
            return;
        }
        let Some(request) = s.request.take() else {
            return;
        };
        (request, s.username.clone())
    };
    START_GAME.store(0, Ordering::Release);
    use crate::jni::{new_class, new_instance, set_known_number, set_known_string};
    for class in PARAMS {
        set_known_number(class, "placeId", request.place_id as f64);
        set_known_number(class, "userId", request.user_id as f64);
        set_known_number(class, "conversationId", request.conversation_id as f64);
        set_known_number(class, "referredByPlayerId", request.referred_by_player_id as f64);
        set_known_number(class, "joinRequestType", f64::from(request.join_request_type()));
        set_known_number(class, "isUnder13", 0.0);
        if !username.is_empty() {
            set_known_string(class, "username", &username);
        }
        for (field, value) in &request.strings {
            set_known_string(class, field, value);
        }
    }
    eprintln!("feline-runtime: joining place {} from the link", request.place_id);
    crate::stubs::set_phase("NativeGLInterface.nativeAppBridgeV2StartGameWithParam".to_owned());
    let params = new_instance(PARAMS[1]);
    // SAFETY: the engine's static JNI function, declared as
    // (Lcom/roblox/engine/jni/autovalue/StartGameParams;)I.
    let start: extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> i32 =
        unsafe { std::mem::transmute(address) };
    let result = start(env, new_class(GL), params);
    crate::stubs::set_phase(String::new());
    eprintln!("feline-runtime: nativeAppBridgeV2StartGameWithParam returned {result}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn experience_links() {
        let r = parse("roblox://experiences/start?placeId=920587237").unwrap();
        assert_eq!(r.place_id, 920587237);
        assert_eq!(r.join_request_type(), 0);

        let r = parse("roblox://experiences/start?placeId=1&gameInstanceId=abc-def").unwrap();
        assert_eq!(r.string("gameId"), "abc-def");
        assert_eq!(r.join_request_type(), 3);

        let r = parse("roblox://placeId=5&linkCode=xyz&launchData=a%2520b").unwrap();
        assert_eq!((r.place_id, r.string("linkCode")), (5, "xyz"));
        assert_eq!(r.string("launchData"), "a b");
        assert_eq!(r.join_request_type(), 2);
    }

    #[test]
    fn website_play_button() {
        let link = "roblox-player:1+launchmode:play+gameinfo:TICKET+launchtime:1700000000000\
            +placelauncherurl:https%3A%2F%2Fwww.roblox.com%2FGame%2FPlaceLauncher.ashx%3Frequest%3DRequestGameJob\
            %26browserTrackerId%3D1%26placeId%3D606849621%26gameId%3D0f1e-22%26isPlayTogetherGame%3Dfalse\
            +browsertrackerid:1+robloxLocale:en_us+gameLocale:en_us";
        let r = parse(link).unwrap();
        assert_eq!(r.place_id, 606849621);
        assert_eq!(r.string("gameId"), "0f1e-22");
        assert_eq!(r.join_request_type(), 3);
    }

    #[test]
    fn rejections() {
        assert!(parse("roblox-player:1+launchmode:edit+placelauncherurl:x").is_err());
        assert!(parse("roblox://navigation/home").is_err());
        assert!(parse("roblox://experiences/start?userId=12").is_err());
        assert!(parse("roblox://experiences/start?placeId=abc").is_err());
        assert!(parse("roblox-player:1+launchmode:play+placelauncherurl:https%3A%2F%2Fevil.example%2FGame%2FPlaceLauncher.ashx%3FplaceId%3D1").is_err());
        assert!(parse("roblox://experiences/start?placeId=1&launchData=%0a").is_err());
        assert!(parse("https://www.roblox.com/games/1").is_err());
    }
}

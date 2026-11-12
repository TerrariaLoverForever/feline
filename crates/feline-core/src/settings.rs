// SPDX-License-Identifier: GPL-3.0-or-later
//! Feline's saved settings, in `~/.config/feline/settings.conf`.
//!
//! The format is plain `key = value` lines, readable and editable by hand:
//!
//! ```text
//! # Feline settings
//! runtime = cordial:flatpak-system
//! account_notice = 1
//! renderer = opengl
//! gamemode = on
//! discord = on
//! discord_join_button = on
//! ```
//!
//! Stability rules:
//! - Unknown keys and malformed lines are ignored, so an old Feline can read a
//!   newer file (and a hand-edit can't stop Feline from starting).
//! - Saving is atomic: write a temporary file, then rename it over the old
//!   one. A crash mid-save leaves the old file intact, never half a file.

use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use crate::paths;

/// The current version of the account-risk notice shown on first start.
/// Bump it when the notice's content changes meaningfully, so everyone who
/// accepted an older version sees the new one once.
pub const ACCOUNT_NOTICE_VERSION: u32 = 1;

/// Feline's renderer override for all runtimes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Renderer {
    /// Don't override: each runtime uses its own setting.
    #[default]
    Runtime,
    Vulkan,
    OpenGl,
}

/// An on/off override, or "let each runtime decide".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Toggle {
    #[default]
    Runtime,
    On,
    Off,
}

/// How much CPU time Windows gives the game compared with other programs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GamePriority {
    #[default]
    Normal,
    AboveNormal,
    High,
}

impl GamePriority {
    fn parse(value: &str) -> Self {
        match value {
            "above-normal" => Self::AboveNormal,
            "high" => Self::High,
            _ => Self::Normal,
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::AboveNormal => "above-normal",
            Self::High => "high",
        }
    }
}

/// What Feline's window does once the game has started.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AfterLaunch {
    /// Stay open, showing the game's stats.
    #[default]
    Stay,
    Minimize,
    /// Close Feline (the game, and the Discord status helper, keep running).
    Close,
}

impl AfterLaunch {
    fn parse(value: &str) -> Self {
        match value {
            "minimize" => Self::Minimize,
            "close" => Self::Close,
            _ => Self::Stay,
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Self::Stay => "stay",
            Self::Minimize => "minimize",
            Self::Close => "close",
        }
    }
}

/// Feline-wide settings that override the matching setting of every runtime
/// (see `runtime::config`). The default overrides nothing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Overrides {
    pub renderer: Renderer,
    pub gamemode: Toggle,
    pub close_on_leave: Toggle,
    /// Feline shows the Discord status itself, so the runtimes' own Discord
    /// status is turned off (two programs would fight over one status).
    pub discord_presence: bool,
}

/// What Feline's Discord status shows (when `Overrides::discord_presence` is on).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresenceOptions {
    /// Your Roblox username and avatar. Off by default: it's personal.
    pub show_user: bool,
    /// A "Join server" button. Off by default: anyone who sees your profile
    /// could join the server you're on.
    pub join_button: bool,
    /// A "See this game" button linking to the game's Roblox page.
    pub game_button: bool,
    /// Show "In the Roblox menus" between games (otherwise no status).
    pub in_menus: bool,
}

impl Default for PresenceOptions {
    fn default() -> Self {
        Self {
            show_user: false,
            join_button: false,
            game_button: true,
            in_menus: true,
        }
    }
}

impl Renderer {
    fn parse(value: &str) -> Self {
        match value {
            "vulkan" => Self::Vulkan,
            "opengl" => Self::OpenGl,
            _ => Self::Runtime,
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
            Self::Vulkan => "vulkan",
            Self::OpenGl => "opengl",
        }
    }
}

impl Toggle {
    fn parse(value: &str) -> Self {
        match value {
            "on" => Self::On,
            "off" => Self::Off,
            _ => Self::Runtime,
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
            Self::On => "on",
            Self::Off => "off",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// [`Installation::key`](crate::runtime::Installation::key) of the runtime
    /// the user chose. `None` until they choose one: Feline never picks.
    pub runtime: Option<String>,
    /// The version of the account-risk notice the user accepted; 0 = never.
    pub account_notice: u32,
    pub overrides: Overrides,
    pub presence: PresenceOptions,
    /// Feline opens `roblox://` links from the website (it's their default
    /// handler).
    pub link_handler: bool,
    /// The desktop entry that handled `roblox://` links before Feline took
    /// over, e.g. `froststrap.desktop`, so it can be given back.
    pub previous_link_handler: Option<String>,
    /// Keep Roblox's app up to date for Feline's runtime (on by default).
    pub roblox_updates: bool,
    /// Delete Roblox's and Feline's logs older than this many days (0 =
    /// keep them).
    pub log_days: u32,
    /// What Feline's window does once the game has started.
    pub after_launch: AfterLaunch,
    /// Windows: the game's process priority.
    pub game_priority: GamePriority,
    /// Windows: ask for the high-performance GPU (laptops with two GPUs).
    pub fast_gpu: bool,
    /// Windows: only start Roblox's client if it's signed by Roblox
    /// Corporation and unchanged (on by default).
    pub verify_roblox: bool,
    /// The enabled mods (folder names in Feline's mods folder).
    pub mods: Vec<String>,
    /// Windows: end a Roblox that's still running in the background before
    /// starting a new one.
    pub close_stuck_roblox: bool,
    /// Windows: end the Roblox Feline started once you leave the game (back
    /// at Roblox's menus, or its window gone), instead of it lingering in
    /// the taskbar and background.
    pub close_roblox_on_leave: bool,
    /// Remind you to take a break every this many minutes of play (0 = never).
    pub break_minutes: u32,
    /// Tidy up Roblox's leftovers every this many days (0 = never).
    pub tidy_days: u32,
    /// The Discord status's extras (see [`DiscordExtras`]).
    pub discord: DiscordExtras,
    /// Windows: give Roblox's window and taskbar button Feline's cat icon.
    pub roblox_icon: bool,
    /// Windows 11: Roblox's title bar colour.
    pub title_bar: TitleBar,
    /// Setting profiles applied before a game starts: place ID → profile
    /// name. Place 0 means "every other game" (and the Roblox app).
    pub game_profiles: std::collections::BTreeMap<u64, String>,
    /// Everyday FastFlags (Tweaks page), allowlisted ones only.
    pub fflags: crate::fflags::FlagSet,
    /// A FastFlag profile per game (place ID → profile name).
    pub fflag_games: std::collections::BTreeMap<u64, String>,
    /// What to delete when Feline closes.
    pub clear_on_exit: ClearOnExit,
    /// The desk cat (see [`crate::cat`]).
    pub cat: DeskCat,
    /// Optional achievements (off by default).
    pub achievements: bool,
    /// Background music (see [`crate::music`]).
    pub music: Music,
    /// Seasonal themes.
    pub season: crate::look::SeasonChoice,
    /// The accent colour: a preset name or `#rrggbb` (see [`crate::look`]).
    pub accent: String,
    /// Dark (the default), light, or whatever the system uses.
    pub appearance: crate::look::Appearance,
    /// Windows: let more than one Roblox run at a time.
    pub multi_roblox: bool,
    /// A desktop notification saying where the server is, when you join.
    pub server_popup: bool,
    /// Programs started with Roblox (and maybe closed with it).
    pub programs: Vec<Program>,
    /// Windows: the Roblox version to stay on (a `version-…` folder name
    /// Feline keeps a copy of), instead of the newest.
    pub roblox_pin: Option<String>,
    /// Ask before starting Roblox when one is already open (on by default).
    pub confirm_launch: bool,
    /// Games may set your Discord status themselves (Bloxstrap's
    /// "BloxstrapRPC" messages; on by default).
    pub game_status: bool,
    /// Rejoin the same server after Roblox disconnects you for being idle.
    pub auto_rejoin: bool,
    /// Windows: a Feline icon by the clock while a game runs (on by default).
    pub tray: bool,
    /// Look for a new Feline when it opens (on by default).
    pub check_updates: bool,
    /// The font for Feline's own window (a font family installed on this
    /// computer); empty: Feline's default.
    pub app_font: String,
    /// Behind Feline's pages: empty (plain), `gradient`, or a picture file.
    pub app_background: String,
    /// Windows 11's glass behind Feline's window: 0 off, 1 Mica, 2 Acrylic.
    pub app_glass: u32,
    /// A Feline card while Roblox starts from a link or shortcut (on by
    /// default), and what went wrong if it didn't.
    pub launch_splash: bool,
    /// Windows: put your mods into Roblox Studio too, when Feline opens it.
    pub studio_mods: bool,
    /// Windows: a Discord status while you build in Studio.
    pub studio_discord: bool,
    /// The mod library shows Froststrap's community mods too.
    pub library_froststrap: bool,
    /// Windows: end Roblox's crash reporter (`RobloxCrashHandler.exe`)
    /// that runs next to every game.
    pub no_crash_handler: bool,
    /// Windows: what opened `.rbxl` place files before Feline did (to give
    /// them back); `Some` while Feline opens them.
    pub rbxl_previous: Option<String>,
    /// Linux: a command the runtime is started through, e.g. `gamescope -f
    /// --` or `distrobox enter games --`. Empty: started directly.
    pub runtime_wrapper: String,
    /// Windows: closing Feline's window keeps it running by the clock
    /// (on by default); its tray icon brings it back or quits.
    pub keep_running: bool,
}

/// A program started with Roblox (Bloxstrap's "custom integrations").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    /// The program file.
    pub path: String,
    /// Its command-line arguments (split like a shell would, quotes kept
    /// together).
    pub args: String,
    /// Close it again when Roblox closes.
    pub close_after: bool,
}

impl Program {
    fn parse(value: &str) -> Option<Self> {
        let mut parts = value.splitn(3, '|').map(str::trim);
        let close_after = match parts.next()? {
            "close" => true,
            "keep" => false,
            _ => return None,
        };
        let path = parts.next().filter(|p| !p.is_empty())?.to_owned();
        let args = parts.next().unwrap_or_default().to_owned();
        Some(Self { path, args, close_after })
    }

    fn to_text(&self) -> String {
        let when = if self.close_after { "close" } else { "keep" };
        let clean = |t: &str| t.replace(['\n', '|'], " ");
        format!("{when} | {} | {}", clean(&self.path), clean(&self.args)).trim_end().trim_end_matches('|').trim_end().to_owned()
    }

    /// The program's file name, e.g. `obs64.exe`.
    pub fn name(&self) -> String {
        std::path::Path::new(&self.path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.clone())
    }

    /// Its arguments, one by one: spaces separate them, except inside
    /// double quotes.
    pub fn arg_list(&self) -> Vec<String> {
        let mut out = Vec::new();
        let (mut current, mut quoted, mut any) = (String::new(), false, false);
        for c in self.args.chars() {
            match c {
                '"' => {
                    quoted = !quoted;
                    any = true;
                }
                c if c.is_whitespace() && !quoted => {
                    if any || !current.is_empty() {
                        out.push(std::mem::take(&mut current));
                    }
                    any = false;
                }
                c => current.push(c),
            }
        }
        if any || !current.is_empty() {
            out.push(current);
        }
        out
    }
}

/// What Feline deletes each time it closes (all off by default).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClearOnExit {
    pub roblox_logs: bool,
    pub feline_logs: bool,
    pub roblox_cache: bool,
    /// The play history (playtime, recent games); favourites stay.
    pub history: bool,
}

impl ClearOnExit {
    pub fn any(self) -> bool {
        self.roblox_logs || self.feline_logs || self.roblox_cache || self.history
    }

    fn parse(value: &str) -> Self {
        let mut c = Self::default();
        for item in value.split(',').map(str::trim) {
            match item {
                "roblox-logs" => c.roblox_logs = true,
                "feline-logs" => c.feline_logs = true,
                "cache" => c.roblox_cache = true,
                "history" => c.history = true,
                _ => {}
            }
        }
        c
    }

    fn to_text(self) -> String {
        let mut items = Vec::new();
        for (on, name) in [
            (self.roblox_logs, "roblox-logs"),
            (self.feline_logs, "feline-logs"),
            (self.roblox_cache, "cache"),
            (self.history, "history"),
        ] {
            if on {
                items.push(name);
            }
        }
        items.join(", ")
    }
}

/// The desk cat's settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeskCat {
    pub on: bool,
    pub coat: crate::cat::Coat,
    /// How many screen pixels per sprite pixel (2–4).
    pub scale: u32,
    /// Only while a game runs (instead of whenever Feline is open).
    pub only_playing: bool,
}

impl Default for DeskCat {
    fn default() -> Self {
        Self {
            on: false,
            coat: crate::cat::Coat::White,
            scale: 2,
            only_playing: false,
        }
    }
}

/// Background music in Feline's window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Music {
    pub on: bool,
    /// The track to loop (a file name in the music folder); empty: every
    /// track, one after another.
    pub track: String,
    /// 0–100.
    pub volume: u32,
    /// Only while Feline's window has the focus (on by default): it goes
    /// quiet when you switch to the game or anything else.
    pub only_focused: bool,
}

impl Default for Music {
    fn default() -> Self {
        Self {
            on: false,
            track: String::new(),
            volume: 40,
            only_focused: true,
        }
    }
}

/// The colour of Roblox's title bar (Windows 11).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TitleBar {
    /// Windows' own colours.
    #[default]
    Normal,
    /// Feline's purple, with white text.
    Purple,
    /// Feline's deep night purple, with lavender text.
    Night,
}

/// More for the Discord status, beyond [`PresenceOptions`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscordExtras {
    /// Your own second line, with `{game}`, `{creator}`, `{region}`,
    /// `{runtime}` and `{today}` filled in. Empty: Feline's own line.
    pub text: String,
    /// Show where the game server is (on by default).
    pub region: bool,
    /// Show how long you've played today.
    pub today: bool,
    /// Games (place IDs) shown only as "Playing something secret".
    pub hidden: Vec<u64>,
}

impl Default for DiscordExtras {
    fn default() -> Self {
        Self {
            text: String::new(),
            region: true,
            today: false,
            hidden: Vec::new(),
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            runtime: None,
            account_notice: 0,
            overrides: Overrides::default(),
            presence: PresenceOptions::default(),
            link_handler: false,
            previous_link_handler: None,
            roblox_updates: true,
            log_days: 0,
            after_launch: AfterLaunch::Stay,
            game_priority: GamePriority::Normal,
            fast_gpu: false,
            verify_roblox: true,
            mods: Vec::new(),
            close_stuck_roblox: false,
            close_roblox_on_leave: false,
            break_minutes: 0,
            tidy_days: 0,
            discord: DiscordExtras::default(),
            game_profiles: std::collections::BTreeMap::new(),
            fflags: crate::fflags::FlagSet::new(),
            fflag_games: std::collections::BTreeMap::new(),
            roblox_icon: false,
            title_bar: TitleBar::Normal,
            clear_on_exit: ClearOnExit::default(),
            cat: DeskCat::default(),
            achievements: false,
            music: Music::default(),
            season: crate::look::SeasonChoice::Auto,
            accent: "violet".to_owned(),
            appearance: crate::look::Appearance::Dark,
            multi_roblox: false,
            server_popup: false,
            programs: Vec::new(),
            roblox_pin: None,
            confirm_launch: true,
            game_status: true,
            auto_rejoin: false,
            tray: true,
            check_updates: true,
            app_font: String::new(),
            app_background: String::new(),
            app_glass: 0,
            launch_splash: true,
            studio_mods: false,
            studio_discord: false,
            library_froststrap: false,
            no_crash_handler: false,
            rbxl_previous: None,
            runtime_wrapper: String::new(),
            keep_running: true,
        }
    }
}

impl Settings {
    /// True if the account-risk notice should be shown (never accepted, or
    /// only an older version of it was).
    pub fn needs_account_notice(&self) -> bool {
        self.account_notice < ACCOUNT_NOTICE_VERSION
    }

    /// The settings file path, `~/.config/feline/settings.conf`.
    pub fn path() -> Option<PathBuf> {
        paths::config_dir().map(|d| d.join("settings.conf"))
    }

    pub fn parse(text: &str) -> Self {
        let mut settings = Self::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "runtime" if !value.is_empty() => settings.runtime = Some(value.to_owned()),
                // A value that isn't a number counts as "never accepted", so
                // the notice is shown again rather than skipped.
                "account_notice" => settings.account_notice = value.parse().unwrap_or(0),
                // Unknown values mean "don't override", the safe choice.
                "renderer" => settings.overrides.renderer = Renderer::parse(value),
                "gamemode" => settings.overrides.gamemode = Toggle::parse(value),
                "close_on_leave" => settings.overrides.close_on_leave = Toggle::parse(value),
                "discord" => settings.overrides.discord_presence = value == "on",
                "discord_show_user" => settings.presence.show_user = value == "on",
                "discord_join_button" => settings.presence.join_button = value == "on",
                "link_handler" => settings.link_handler = value == "on",
                "previous_link_handler" if !value.is_empty() => {
                    settings.previous_link_handler = Some(value.to_owned())
                }
                // These two default to on, so only "off" changes them.
                "discord_game_button" => settings.presence.game_button = value != "off",
                "discord_in_menus" => settings.presence.in_menus = value != "off",
                "roblox_updates" => settings.roblox_updates = value != "off",
                "log_days" => settings.log_days = value.parse().unwrap_or(0),
                "after_launch" => settings.after_launch = AfterLaunch::parse(value),
                "game_priority" => settings.game_priority = GamePriority::parse(value),
                "fast_gpu" => settings.fast_gpu = value == "on",
                // On by default: only "off" switches the check off.
                "verify_roblox" => settings.verify_roblox = value != "off",
                // Folder names can't contain `|` on Windows; on Linux one
                // that does just can't be enabled.
                "close_stuck_roblox" => settings.close_stuck_roblox = value == "on",
                "close_roblox_on_leave" => settings.close_roblox_on_leave = value == "on",
                "roblox_icon" => settings.roblox_icon = value == "on",
                "title_bar" => {
                    settings.title_bar = match value {
                        "purple" => TitleBar::Purple,
                        "night" => TitleBar::Night,
                        _ => TitleBar::Normal,
                    }
                }
                key if key.starts_with("fflag.") => {
                    if let Some(v) = crate::fflags::checked(&key["fflag.".len()..], value) {
                        settings.fflags.insert(key["fflag.".len()..].to_owned(), v);
                    }
                }
                key if key.starts_with("fflag_game.") => {
                    if let Ok(place) = key["fflag_game.".len()..].parse()
                        && !value.is_empty()
                    {
                        settings.fflag_games.insert(place, value.to_owned());
                    }
                }
                key if key.starts_with("game_profile.") => {
                    if let Ok(place) = key["game_profile.".len()..].parse()
                        && !value.is_empty()
                    {
                        settings.game_profiles.insert(place, value.to_owned());
                    }
                }
                "discord_text" => settings.discord.text = value.chars().take(128).collect(),
                "discord_region" => settings.discord.region = value != "off",
                "discord_today" => settings.discord.today = value == "on",
                "discord_hidden" => {
                    settings.discord.hidden =
                        value.split(',').filter_map(|v| v.trim().parse().ok()).collect();
                }
                "break_minutes" => settings.break_minutes = value.parse().unwrap_or(0),
                "tidy_days" => settings.tidy_days = value.parse().unwrap_or(0),
                "clear_on_exit" => settings.clear_on_exit = ClearOnExit::parse(value),
                "desk_cat" => settings.cat.on = value == "on",
                "desk_cat_coat" => settings.cat.coat = crate::cat::Coat::parse(value),
                "desk_cat_size" => settings.cat.scale = value.parse().unwrap_or(2).clamp(2, 4),
                "desk_cat_when" => settings.cat.only_playing = value == "playing",
                "achievements" => settings.achievements = value == "on",
                "music" => settings.music.on = value == "on",
                "music_track" => settings.music.track = value.to_owned(),
                "music_volume" => settings.music.volume = value.parse().unwrap_or(40).min(100),
                "music_focused_only" => settings.music.only_focused = value != "off",
                "season" => settings.season = crate::look::SeasonChoice::parse(value),
                "appearance" => settings.appearance = crate::look::Appearance::parse(value),
                "accent" => {
                    let preset = crate::look::ACCENTS.iter().any(|(n, _, _)| *n == value);
                    let own = value.starts_with('#') && crate::look::parse_hex(value).is_some();
                    if preset || own {
                        settings.accent = value.to_owned();
                    }
                }
                "multi_roblox" => settings.multi_roblox = value == "on",
                // On by default: only "off" changes them.
                "confirm_launch" => settings.confirm_launch = value != "off",
                "game_status" => settings.game_status = value != "off",
                "tray" => settings.tray = value != "off",
                "check_updates" => settings.check_updates = value != "off",
                "launch_splash" => settings.launch_splash = value != "off",
                "keep_running" => settings.keep_running = value != "off",
                "studio_mods" => settings.studio_mods = value == "on",
                "studio_discord" => settings.studio_discord = value == "on",
                "library_froststrap" => settings.library_froststrap = value == "on",
                "no_crash_handler" => settings.no_crash_handler = value == "on",
                "rbxl_previous" => settings.rbxl_previous = Some(value.to_owned()),
                "runtime_wrapper" => settings.runtime_wrapper = value.chars().take(300).collect(),
                "auto_rejoin" => settings.auto_rejoin = value == "on",
                "app_font" => settings.app_font = value.chars().take(64).collect(),
                "app_background" => settings.app_background = value.to_owned(),
                "app_glass" => settings.app_glass = value.parse().unwrap_or(0).min(2),
                "server_popup" => settings.server_popup = value == "on",
                "with_roblox" => settings.programs.extend(Program::parse(value)),
                "roblox_pin" if value.starts_with("version-") && !value.contains(['/', '\\']) => {
                    settings.roblox_pin = Some(value.to_owned())
                }
                "mods" => {
                    settings.mods = value
                        .split('|')
                        .map(str::trim)
                        .filter(|m| !m.is_empty())
                        .map(str::to_owned)
                        .collect();
                }
                _ => {}
            }
        }
        settings
    }

    pub fn to_text(&self) -> String {
        let mut text = String::from("# Feline settings. Safe to edit while Feline is closed.\n");
        if let Some(runtime) = &self.runtime {
            text.push_str(&format!("runtime = {runtime}\n"));
        }
        if self.account_notice > 0 {
            text.push_str(&format!("account_notice = {}\n", self.account_notice));
        }
        let o = &self.overrides;
        if o.renderer != Renderer::Runtime {
            text.push_str(&format!("renderer = {}\n", o.renderer.as_str()));
        }
        if o.gamemode != Toggle::Runtime {
            text.push_str(&format!("gamemode = {}\n", o.gamemode.as_str()));
        }
        if o.close_on_leave != Toggle::Runtime {
            text.push_str(&format!("close_on_leave = {}\n", o.close_on_leave.as_str()));
        }
        let on_off = |b: bool| if b { "on" } else { "off" };
        if o.discord_presence {
            text.push_str("discord = on\n");
        }
        if self.link_handler {
            text.push_str("link_handler = on\n");
        }
        if let Some(previous) = &self.previous_link_handler {
            text.push_str(&format!("previous_link_handler = {previous}\n"));
        }
        if !self.roblox_updates {
            text.push_str("roblox_updates = off\n");
        }
        if self.log_days > 0 {
            text.push_str(&format!("log_days = {}\n", self.log_days));
        }
        if self.after_launch != AfterLaunch::Stay {
            text.push_str(&format!("after_launch = {}\n", self.after_launch.as_str()));
        }
        if self.game_priority != GamePriority::Normal {
            text.push_str(&format!("game_priority = {}\n", self.game_priority.as_str()));
        }
        if self.fast_gpu {
            text.push_str("fast_gpu = on\n");
        }
        if !self.verify_roblox {
            text.push_str("verify_roblox = off\n");
        }
        if self.roblox_icon {
            text.push_str("roblox_icon = on\n");
        }
        match self.title_bar {
            TitleBar::Normal => {}
            TitleBar::Purple => text.push_str("title_bar = purple\n"),
            TitleBar::Night => text.push_str("title_bar = night\n"),
        }
        for (place, profile) in &self.game_profiles {
            text.push_str(&format!("game_profile.{place} = {profile}\n"));
        }
        for (name, value) in crate::fflags::sanitize(&self.fflags) {
            text.push_str(&format!("fflag.{name} = {value}\n"));
        }
        for (place, profile) in &self.fflag_games {
            text.push_str(&format!("fflag_game.{place} = {profile}\n"));
        }
        let d = &self.discord;
        if !d.text.is_empty() {
            text.push_str(&format!("discord_text = {}\n", d.text.replace('\n', " ")));
        }
        if !d.region {
            text.push_str("discord_region = off\n");
        }
        if d.today {
            text.push_str("discord_today = on\n");
        }
        if !d.hidden.is_empty() {
            let ids: Vec<String> = d.hidden.iter().map(u64::to_string).collect();
            text.push_str(&format!("discord_hidden = {}\n", ids.join(", ")));
        }
        if self.close_stuck_roblox {
            text.push_str("close_stuck_roblox = on\n");
        }
        if self.close_roblox_on_leave {
            text.push_str("close_roblox_on_leave = on\n");
        }
        if self.break_minutes > 0 {
            text.push_str(&format!("break_minutes = {}\n", self.break_minutes));
        }
        if self.tidy_days > 0 {
            text.push_str(&format!("tidy_days = {}\n", self.tidy_days));
        }
        if !self.mods.is_empty() {
            text.push_str(&format!("mods = {}\n", self.mods.join(" | ")));
        }
        if self.clear_on_exit.any() {
            text.push_str(&format!("clear_on_exit = {}\n", self.clear_on_exit.to_text()));
        }
        let cat = &self.cat;
        let cat_default = DeskCat::default();
        if cat.on {
            text.push_str("desk_cat = on\n");
        }
        if cat.coat != cat_default.coat {
            text.push_str(&format!("desk_cat_coat = {}\n", cat.coat.as_str()));
        }
        if cat.scale != cat_default.scale {
            text.push_str(&format!("desk_cat_size = {}\n", cat.scale));
        }
        if cat.only_playing {
            text.push_str("desk_cat_when = playing\n");
        }
        if self.achievements {
            text.push_str("achievements = on\n");
        }
        let music = &self.music;
        let music_default = Music::default();
        if music.on {
            text.push_str("music = on\n");
        }
        if !music.track.is_empty() {
            text.push_str(&format!("music_track = {}\n", music.track.replace('\n', " ")));
        }
        if music.volume != music_default.volume {
            text.push_str(&format!("music_volume = {}\n", music.volume));
        }
        if !music.only_focused {
            text.push_str("music_focused_only = off\n");
        }
        if self.season != crate::look::SeasonChoice::Auto {
            text.push_str(&format!("season = {}\n", self.season.as_str()));
        }
        if self.accent != "violet" {
            text.push_str(&format!("accent = {}\n", self.accent));
        }
        if self.appearance != crate::look::Appearance::Dark {
            text.push_str(&format!("appearance = {}\n", self.appearance.as_str()));
        }
        if self.multi_roblox {
            text.push_str("multi_roblox = on\n");
        }
        if self.server_popup {
            text.push_str("server_popup = on\n");
        }
        for program in &self.programs {
            text.push_str(&format!("with_roblox = {}\n", program.to_text()));
        }
        if let Some(pin) = &self.roblox_pin {
            text.push_str(&format!("roblox_pin = {pin}\n"));
        }
        for (key, on) in [
            ("confirm_launch", self.confirm_launch),
            ("game_status", self.game_status),
            ("tray", self.tray),
            ("check_updates", self.check_updates),
            ("launch_splash", self.launch_splash),
            ("keep_running", self.keep_running),
        ] {
            if !on {
                text.push_str(&format!("{key} = off\n"));
            }
        }
        if self.auto_rejoin {
            text.push_str("auto_rejoin = on\n");
        }
        if !self.app_font.is_empty() {
            text.push_str(&format!("app_font = {}\n", self.app_font.replace('\n', " ")));
        }
        if !self.app_background.is_empty() {
            text.push_str(&format!("app_background = {}\n", self.app_background.replace('\n', " ")));
        }
        if self.studio_mods {
            text.push_str("studio_mods = on\n");
        }
        if self.studio_discord {
            text.push_str("studio_discord = on\n");
        }
        if self.library_froststrap {
            text.push_str("library_froststrap = on\n");
        }
        if self.no_crash_handler {
            text.push_str("no_crash_handler = on\n");
        }
        if let Some(previous) = &self.rbxl_previous {
            text.push_str(&format!("rbxl_previous = {}\n", previous.replace('\n', " ")));
        }
        if !self.runtime_wrapper.is_empty() {
            text.push_str(&format!("runtime_wrapper = {}\n", self.runtime_wrapper.replace('\n', " ")));
        }
        if self.app_glass > 0 {
            text.push_str(&format!("app_glass = {}\n", self.app_glass));
        }
        let p = &self.presence;
        let defaults = PresenceOptions::default();
        for (key, value, default) in [
            ("discord_show_user", p.show_user, defaults.show_user),
            ("discord_join_button", p.join_button, defaults.join_button),
            ("discord_game_button", p.game_button, defaults.game_button),
            ("discord_in_menus", p.in_menus, defaults.in_menus),
        ] {
            if value != default {
                text.push_str(&format!("{key} = {}\n", on_off(value)));
            }
        }
        text
    }

    /// Loads from `path`. A missing file means default settings.
    pub fn load_from(path: &Path) -> io::Result<Self> {
        match fs::read_to_string(path) {
            Ok(text) => Ok(Self::parse(&text)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }

    /// Saves to `path` atomically, creating the folder if needed.
    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let temp = path.with_extension("conf.tmp");
        {
            let mut file = fs::File::create(&temp)?;
            file.write_all(self.to_text().as_bytes())?;
            // Make sure the data is on disk before the rename makes it live.
            file.sync_all()?;
        }
        fs::rename(&temp, path)
    }
}

#[cfg(test)]
mod new_option_tests {
    use super::*;

    #[test]
    fn new_options_round_trip_and_default_safely() {
        let defaults = Settings::default();
        assert!(defaults.verify_roblox, "the signature check is on by default");
        assert_eq!(Settings::parse(&defaults.to_text()), defaults);

        let changed = Settings {
            after_launch: AfterLaunch::Minimize,
            game_priority: GamePriority::High,
            fast_gpu: true,
            verify_roblox: false,
            mods: vec!["Old oof".into(), "Cat cursors".into()],
            close_stuck_roblox: true,
            close_roblox_on_leave: true,
            break_minutes: 60,
            tidy_days: 7,
            clear_on_exit: ClearOnExit {
                roblox_logs: true,
                history: true,
                ..ClearOnExit::default()
            },
            cat: DeskCat {
                on: true,
                coat: crate::cat::Coat::Black,
                scale: 3,
                only_playing: true,
            },
            achievements: true,
            music: Music {
                on: true,
                track: "Purr song.m4a".into(),
                volume: 0,
                only_focused: false,
            },
            season: crate::look::SeasonChoice::Off,
            accent: "#12ab34".to_owned(),
            appearance: crate::look::Appearance::System,
            game_profiles: [(0, "Everyday".into()), (606849621, "Competitive".into())].into(),
            fflags: [("FIntDebugForceMSAASamples".to_owned(), "4".to_owned())].into(),
            fflag_games: [(606849621, "Potato".into())].into(),
            roblox_icon: true,
            title_bar: TitleBar::Night,
            discord: DiscordExtras {
                text: "Purring in {game} · {region}".into(),
                region: false,
                today: true,
                hidden: vec![1, 2],
            },
            multi_roblox: true,
            server_popup: true,
            programs: vec![
                Program { path: r"C:\Tools\macro.exe".into(), args: r#"--profile "Cat run" -q"#.into(), close_after: true },
                Program { path: "/usr/bin/obs".into(), args: String::new(), close_after: false },
            ],
            roblox_pin: Some("version-63eb6055af4e4688".into()),
            confirm_launch: false,
            game_status: false,
            auto_rejoin: true,
            tray: false,
            check_updates: false,
            app_font: "Comic Sans MS".into(),
            app_background: r"C:\Pics\cat.png".into(),
            app_glass: 2,
            launch_splash: false,
            studio_mods: true,
            studio_discord: true,
            library_froststrap: true,
            no_crash_handler: true,
            rbxl_previous: Some("Roblox.Place".into()),
            runtime_wrapper: "gamescope -f --".into(),
            keep_running: false,
            ..Settings::default()
        };
        assert_eq!(Settings::parse(&changed.to_text()), changed);
        assert_eq!(changed.programs[0].arg_list(), ["--profile", "Cat run", "-q"]);
        assert_eq!(changed.programs[0].name(), if cfg!(windows) { "macro.exe" } else { r"C:\Tools\macro.exe" });
        // A pin that isn't a plain version folder is ignored.
        assert_eq!(Settings::parse("roblox_pin = ../evil\n").roblox_pin, None);
        // Unknown values fall back to the safe default.
        let odd = Settings::parse("game_priority = realtime\nafter_launch = explode\n");
        assert_eq!(odd.game_priority, GamePriority::Normal);
        assert_eq!(odd.after_launch, AfterLaunch::Stay);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_is_lenient() {
        let text = "# comment\n\nruntime = cordial:flatpak-system\nfuture_key = 1\nnonsense\n";
        assert_eq!(
            Settings::parse(text).runtime.as_deref(),
            Some("cordial:flatpak-system")
        );
        assert_eq!(Settings::parse("runtime =   \n").runtime, None);
    }

    #[test]
    fn overrides_parse_and_default_to_runtime() {
        let s = Settings::parse("renderer = vulkan\ngamemode = on\nclose_on_leave = maybe\n");
        assert_eq!(s.overrides.renderer, Renderer::Vulkan);
        assert_eq!(s.overrides.gamemode, Toggle::On);
        assert_eq!(s.overrides.close_on_leave, Toggle::Runtime);
        // Nothing overridden → nothing written.
        assert!(!Settings::default().to_text().contains("renderer"));
    }

    #[test]
    fn account_notice_versions() {
        assert!(Settings::default().needs_account_notice());
        assert!(Settings::parse("account_notice = banana\n").needs_account_notice());

        let accepted = Settings::parse(&format!("account_notice = {ACCOUNT_NOTICE_VERSION}\n"));
        assert!(!accepted.needs_account_notice());
        // A newer notice than the one accepted must be shown again.
        let older = Settings {
            account_notice: ACCOUNT_NOTICE_VERSION - 1,
            ..Settings::default()
        };
        assert!(older.needs_account_notice());
    }

    #[test]
    fn round_trip_and_missing_file() {
        let dir = std::env::temp_dir().join(format!("feline-test-settings-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("nested/settings.conf");

        assert_eq!(Settings::load_from(&path).unwrap(), Settings::default());

        let settings = Settings {
            runtime: Some("cordial:appimage:/home/me/My Apps/Cordial.AppImage".into()),
            account_notice: ACCOUNT_NOTICE_VERSION,
            overrides: Overrides {
                renderer: Renderer::OpenGl,
                gamemode: Toggle::Off,
                close_on_leave: Toggle::On,
                discord_presence: true,
            },
            presence: PresenceOptions {
                show_user: true,
                join_button: true,
                game_button: false,
                in_menus: false,
            },
            link_handler: true,
            previous_link_handler: Some("froststrap.desktop".into()),
            roblox_updates: false,
            log_days: 14,
            ..Settings::default()
        };
        settings.save_to(&path).unwrap();
        let loaded = Settings::load_from(&path).unwrap();
        let leftover_temp = path.with_extension("conf.tmp").exists();
        let _ = fs::remove_dir_all(&dir);

        assert_eq!(loaded, settings);
        assert!(!leftover_temp);
    }
}

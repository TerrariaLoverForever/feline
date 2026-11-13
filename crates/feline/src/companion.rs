// SPDX-License-Identifier: GPL-3.0-or-later
//! What Feline does around a Roblox session, for both Play and roblox.com
//! links:
//!
//! - **Staying on a version**: start the kept copy instead of the newest.
//! - **More than one Roblox** (Windows): a small helper process
//!   (`feline --multi-roblox`) holds Roblox's "only one of me" markers for
//!   as long as any Roblox runs, so a second one doesn't close the first.
//! - **The game helper** (`feline --with-roblox <pid>`): follows the game
//!   in Roblox's log for the tray menu (Windows), the server pop-up, the
//!   server history and rejoining after an idle kick; starts the programs
//!   that go with Roblox and closes them afterwards; plays an animated
//!   cursor over the game (Windows). Its own process, so all of that keeps
//!   going if the Feline window closes, and ends with the game.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use feline_core::runtime::{DetectContext, InstallKind, Installation};
use feline_core::settings::Settings;

/// Roblox's own client on Windows (what multi-Roblox, cursors and pinning
/// are for).
fn official(installation: &Installation) -> bool {
    cfg!(windows) && matches!(installation.kind, InstallKind::Official(_))
}

/// Before starting Roblox: the installation to start (the kept version, if
/// one is chosen and there), and lines for the user.
pub fn before(installation: &Installation, settings: &Settings) -> (Installation, Vec<String>) {
    let mut lines = Vec::new();
    let mut chosen = installation.clone();
    if let Some(pin) = &settings.roblox_pin
        && official(installation)
    {
        match feline_core::runtime::roblox::pinned(installation, pin) {
            Some(kept) => chosen = kept,
            None => lines.push(format!(
                "The Roblox version you chose to stay on ({pin}) isn't there any more; starting the newest."
            )),
        }
    }
    if settings.multi_roblox && official(installation) {
        lines.extend(make_room_for_more());
    }
    (chosen, lines)
}

/// Whether Roblox should get a see-through cursor this time: an animated
/// cursor is picked and on, and Feline can draw it (Windows).
pub fn hide_cursor(installation: &Installation, settings: &Settings) -> bool {
    official(installation)
        && settings.mods.iter().any(|m| m == feline_core::mods::FELINE_PICKS)
        && feline_core::mods::mods_dir().and_then(|d| feline_core::mods::animated_cursor(&d)).is_some()
}

/// After Roblox started: the game helper (tray, server pop-up and history,
/// auto-rejoin, programs, animated cursor). It reads the settings itself.
pub fn after(pid: u32, hide_cursor: bool) -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let mut helper = Command::new(exe);
    helper.args(["--with-roblox", &pid.to_string()]);
    if hide_cursor {
        helper.arg("--cursor");
    }
    let mut command = feline_core::launch::detached(helper);
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    command.spawn().err().map(|e| format!("Feline's game helper didn't start: {e}"))
}

/// Starts the multi-Roblox helper unless it runs already; waits a moment
/// for it to hold Roblox's markers. A line for the user if it couldn't.
fn make_room_for_more() -> Option<String> {
    #[cfg(windows)]
    {
        if feline_win::game::OnlyOne::held("MultiRoblox") {
            return None;
        }
        let exe = std::env::current_exe().ok()?;
        let mut helper = Command::new(exe);
        helper.arg("--multi-roblox");
        let mut command = feline_core::launch::detached(helper);
        command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        if let Err(e) = command.spawn() {
            return Some(format!("More than one Roblox won't work this time: {e}"));
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if feline_win::game::OnlyOne::held("MultiRoblox") {
                return None;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Some("More than one Roblox won't work this time: a Roblox was already running. Close every Roblox, then press Play.".to_owned())
    }
    #[cfg(not(windows))]
    None
}

/// The Roblox player's process IDs.
#[cfg(windows)]
fn roblox_pids() -> Vec<u32> {
    feline_win::processes()
        .into_iter()
        .filter(|p| p.exe_name.eq_ignore_ascii_case(feline_core::runtime::roblox::PLAYER_EXE))
        .map(|p| p.pid)
        .collect()
}

/// `feline --multi-roblox`: holds Roblox's markers while any Roblox runs.
pub fn run_multi_holder() -> std::process::ExitCode {
    #[cfg(windows)]
    {
        let held = match feline_win::game::RobloxSingleton::hold() {
            Ok(held) => held,
            Err(e) => {
                eprintln!("feline: {e}");
                return std::process::ExitCode::FAILURE;
            }
        };
        // Only now say "ready" (see make_room_for_more).
        let Some(only) = feline_win::game::OnlyOne::take("MultiRoblox") else {
            return std::process::ExitCode::SUCCESS;
        };
        // A minute for the first Roblox to show up, then until the last
        // one has been gone for a few seconds.
        let started = Instant::now();
        let mut last_seen = None::<Instant>;
        loop {
            std::thread::sleep(Duration::from_secs(2));
            if !roblox_pids().is_empty() {
                last_seen = Some(Instant::now());
            }
            let quiet = match last_seen {
                Some(t) => t.elapsed() > Duration::from_secs(10),
                None => started.elapsed() > Duration::from_secs(60),
            };
            if quiet {
                break;
            }
        }
        drop(only);
        drop(held);
        std::process::ExitCode::SUCCESS
    }
    #[cfg(not(windows))]
    {
        eprintln!("feline: more than one Roblox is a Windows thing");
        std::process::ExitCode::FAILURE
    }
}


// ---------------------------------------------------------------------------
// The game helper
// ---------------------------------------------------------------------------

/// What the network look-ups found (they run on their own threads and
/// report back to the helper's loop).
enum Found {
    GameName(u64, String),
    Region(String, String),
    Spot(String, feline_core::serverinfo::ServerSpot),
}

/// The game you're in right now, as far as the helper knows.
#[derive(Default)]
struct Now {
    place_id: u64,
    job_id: String,
    universe_id: Option<u64>,
    game: String,
    region: String,
    started: Option<u64>,
}

impl Now {
    /// "Server in Frankfurt am Main, Germany · up 2 h 13 min".
    #[cfg_attr(not(windows), allow(dead_code))]
    fn summary(&self) -> String {
        let mut parts = Vec::new();
        if !self.region.is_empty() {
            parts.push(format!("Server in {}", self.region));
        }
        if let Some(started) = self.started {
            parts.push(format!("up {}", feline_core::serverinfo::uptime_label(started, feline_core::history::now())));
        }
        parts.join(" · ")
    }

    fn remember(&self) {
        let _ = feline_core::serverinfo::record(feline_core::serverinfo::Visit {
            place_id: self.place_id,
            job_id: self.job_id.clone(),
            game: self.game.clone(),
            region: self.region.clone(),
            at: feline_core::history::now(),
        });
    }
}

/// `feline --with-roblox <pid> [--cursor]`: the game helper (see the top).
pub fn run_game_helper(pid: u32, cursor: bool) -> std::process::ExitCode {
    use feline_core::presence::{Activity, log::LogEvent};
    let settings = Settings::path().and_then(|p| Settings::load_from(&p).ok()).unwrap_or_default();
    let ctx = DetectContext::host();
    let found = feline_core::runtime::detect_all(&ctx);
    let installation = settings.runtime.as_deref().and_then(|key| found.iter().find(|i| i.key() == key)).cloned();
    let roots = installation.as_ref().map(|i| i.roblox_log_roots(&ctx)).unwrap_or_default();

    let (companions, problems) = feline_core::launch::start_programs(&settings.programs);
    if !problems.is_empty() {
        feline_core::launch::notify_quiet("A program didn't start with Roblox", &problems.join("\n"));
    }

    #[cfg(windows)]
    let mut painter = if cursor { CursorPainter::new() } else { None };
    #[cfg(windows)]
    let mut tray = settings
        .tray
        .then(|| feline_win::tray::Tray::new(feline_core::runtime::roblox::feline_icon_file().as_deref(), "Feline"))
        .flatten();

    let mut follower =
        feline_core::presence::log::LogFollower::new(roots, std::time::SystemTime::now() - Duration::from_secs(60));
    let (send, receive) = std::sync::mpsc::channel::<Found>();
    let mut activity = Activity::Starting;
    let mut now = Now::default();
    let mut told = String::new();
    let mut rejoin_after: Option<Instant> = None;
    let mut started_in_log: Option<u64> = None;
    let begun = Instant::now();
    #[cfg(windows)]
    let mut hidden: Vec<isize> = Vec::new();
    let mut polled = Instant::now() - Duration::from_secs(1);
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut robloxes_seen = Instant::now();

    loop {
        // About twice a second: the log, the look-ups, the end of the game.
        if polled.elapsed() >= Duration::from_millis(500) {
            polled = Instant::now();
            for event in follower.poll() {
                if let LogEvent::ServerStarted(at) = &event {
                    started_in_log = Some(*at);
                }
                if let LogEvent::Disconnected(reason) = &event
                    && settings.auto_rejoin
                    && reason.to_ascii_lowercase().contains("idle")
                    && !now.job_id.is_empty()
                {
                    rejoin_after = Some(Instant::now() + Duration::from_secs(3));
                }
                activity = activity.apply(event);
            }
            if let Activity::Game { place_id, job_id, server, .. } = &activity
                && !job_id.is_empty()
                && *job_id != now.job_id
            {
                // A new server: remember it, and find out where it is.
                now = Now { place_id: *place_id, job_id: job_id.clone(), started: started_in_log.take(), ..Now::default() };
                now.remember();
                look_up_spot(&send, *place_id, job_id.clone(), server.clone());
            }
            if let Activity::Game { universe_id: Some(u), .. } = &activity
                && now.universe_id != Some(*u)
            {
                now.universe_id = Some(*u);
                look_up_name(&send, *u);
            }
            let mut learned = false;
            while let Ok(found) = receive.try_recv() {
                match found {
                    Found::GameName(u, name) if now.universe_id == Some(u) => now.game = name,
                    Found::Region(job, region) if job == now.job_id && now.region.is_empty() => now.region = region,
                    Found::Spot(job, spot) if job == now.job_id => {
                        now.region = spot.place();
                        now.started = now.started.or(spot.started);
                    }
                    _ => continue,
                }
                learned = true;
            }
            if learned {
                now.remember();
            }
            // The server pop-up, once per server, when its place is known.
            if settings.server_popup && !now.region.is_empty() && told != now.job_id {
                told.clone_from(&now.job_id);
                let title = if now.game.is_empty() { "You're in!".to_owned() } else { format!("You're in {}", now.game) };
                feline_core::launch::notify_quiet(&title, &format!("The server is in {}.", now.region));
            }
            #[cfg(windows)]
            if !roblox_pids().is_empty() {
                robloxes_seen = Instant::now();
            }
            // Roblox's crash reporter starts with the game; it's ended
            // while it may still turn up.
            #[cfg(windows)]
            if settings.no_crash_handler && begun.elapsed() < Duration::from_secs(90) {
                feline_win::game::end_processes("RobloxCrashHandler.exe");
            }
            // Done when the game has closed (and, while Feline draws the
            // cursor, every other Roblox too).
            if !feline_core::launch::alive(pid) && (!cursor || robloxes_seen.elapsed() > Duration::from_secs(2)) {
                break;
            }
        }

        // Rejoining after an idle kick: close this Roblox, join the same
        // server again (that starts a new helper), and stop.
        if rejoin_after.is_some_and(|t| Instant::now() >= t) {
            feline_core::runtime::roblox::end_game(pid);
            std::thread::sleep(Duration::from_secs(2));
            open_with_feline(Some(&feline_core::roblox_tools::server_link(now.place_id, &now.job_id)));
            break;
        }

        #[cfg(windows)]
        if let Some(t) = tray.as_mut() {
            let tip = if now.game.is_empty() { "Feline".to_owned() } else { format!("Feline · {}", now.game) };
            t.set_tip(&tip);
            if t.clicked() && tray_menu(t, &now, pid, &mut hidden) {
                break;
            }
        }

        #[cfg(windows)]
        match painter.as_mut() {
            Some(p) => p.tick(),
            None => std::thread::sleep(Duration::from_millis(50)),
        }
        #[cfg(not(windows))]
        std::thread::sleep(Duration::from_millis(100));
    }

    #[cfg(windows)]
    {
        // Never leave a hidden Roblox window behind.
        for window in hidden {
            feline_win::window::set_shown(window, true);
        }
        drop(tray);
    }
    #[cfg(not(windows))]
    let _ = begun;
    feline_core::launch::close_programs(companions);
    if cursor {
        // Roblox's cursor back to the picked one's first frame, so a Roblox
        // started some other way doesn't get an invisible cursor.
        restore_cursor(&settings);
    }
    std::process::ExitCode::SUCCESS
}

fn look_up_name(send: &std::sync::mpsc::Sender<Found>, universe: u64) {
    let send = send.clone();
    std::thread::spawn(move || {
        if let Ok(game) = feline_core::presence::roblox::Api::new().game(universe) {
            let _ = send.send(Found::GameName(universe, game.name));
        }
    });
}

/// Where the server is and since when (RoValra), or at least where (from
/// its address, as for the Discord status).
fn look_up_spot(send: &std::sync::mpsc::Sender<Found>, place: u64, job: String, server: Option<String>) {
    let send = send.clone();
    std::thread::spawn(move || {
        if let Ok(Some(spot)) = feline_core::serverinfo::server_details(place, &job) {
            let _ = send.send(Found::Spot(job, spot));
            return;
        }
        if let Some(ip) = server
            && let Ok(region) = feline_core::presence::roblox::Api::new().server_region(&ip)
        {
            let _ = send.send(Found::Region(job, region));
        }
    });
}

/// Starts Feline itself: its window, or with a `roblox://` link.
fn open_with_feline(link: Option<&str>) {
    let Ok(exe) = std::env::current_exe() else { return };
    let mut command = Command::new(exe);
    if let Some(link) = link {
        command.arg(link);
    }
    let mut command = feline_core::launch::detached(command);
    let _ = command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
}

/// The tray menu. True if the helper should stop (Roblox is being left).
#[cfg(windows)]
fn tray_menu(tray: &feline_win::tray::Tray, now: &Now, pid: u32, hidden: &mut Vec<isize>) -> bool {
    use feline_win::tray::Item;
    let in_game = !now.job_id.is_empty();
    let title = if now.game.is_empty() { "Roblox".to_owned() } else { now.game.clone() };
    let summary = now.summary();
    let mut items = vec![Item::Info(&title)];
    if !summary.is_empty() {
        items.push(Item::Info(&summary));
    }
    items.push(Item::Separator);
    if in_game {
        items.push(Item::Action(1, "Copy invite link"));
        items.push(Item::Action(2, "Rejoin this server"));
    }
    items.push(Item::Action(5, if hidden.is_empty() { "Hide Roblox" } else { "Show Roblox" }));
    items.push(Item::Action(3, "Open Feline"));
    items.push(Item::Separator);
    items.push(Item::Action(4, "Close Roblox"));
    match tray.menu(&items) {
        Some(1) => {
            let link = feline_core::serverinfo::invite_link(now.place_id, &now.job_id);
            if feline_win::tray::copy_text(&link) {
                feline_core::launch::notify_quiet("Invite link copied", "Send it to a friend: it drops them into your server.");
            }
            false
        }
        Some(2) => {
            feline_core::runtime::roblox::end_game(pid);
            std::thread::sleep(Duration::from_secs(2));
            open_with_feline(Some(&feline_core::roblox_tools::server_link(now.place_id, &now.job_id)));
            true
        }
        Some(3) => {
            open_with_feline(None);
            false
        }
        Some(4) => {
            feline_core::runtime::roblox::end_game(pid);
            false
        }
        Some(5) => {
            if hidden.is_empty() {
                // Off the screen and the taskbar; the tray brings it back.
                *hidden = feline_win::window::windows_of(pid);
                for &w in hidden.iter() {
                    feline_win::window::set_shown(w, false);
                }
            } else {
                for w in hidden.drain(..) {
                    feline_win::window::set_shown(w, true);
                }
            }
            false
        }
        _ => false,
    }
}

/// Puts the mods back in without the see-through cursor (once no Roblox
/// runs any more).
fn restore_cursor(settings: &Settings) {
    let ctx = DetectContext::host();
    let found = feline_core::runtime::detect_all(&ctx);
    let Some(installation) = settings.runtime.as_deref().and_then(|key| found.iter().find(|i| i.key() == key)) else {
        return;
    };
    let (installation, _) = before(installation, &Settings { multi_roblox: false, ..settings.clone() });
    let _ = feline_core::mods::apply_for_launch(&installation, &ctx, &settings.mods, false);
}

/// Plays the animated cursor over Roblox's windows, a frame at a time.
#[cfg(windows)]
struct CursorPainter {
    window: feline_win::overlay::Overlay,
    frames: Vec<(Vec<u8>, Duration)>,
    size: u32,
    hotspot: (i32, i32),
    robloxes: Vec<u32>,
    checked: Instant,
    frame: usize,
    frame_since: Instant,
    last: Option<(i32, i32, usize)>,
}

#[cfg(windows)]
impl CursorPainter {
    /// `None` if there's no animated cursor or no window for it.
    fn new() -> Option<Self> {
        use feline_win::overlay;
        let file = feline_core::mods::mods_dir().and_then(|d| feline_core::mods::animated_cursor(&d))?;
        let animation = feline_core::cursor::load(&std::fs::read(&file).ok()?).ok()?;
        // Real screen pixels, and a cursor as big as Windows' own at this scale.
        let dpi = overlay::dpi_aware();
        let factor = ((dpi as f32 / 96.0).round() as u32).max(1);
        let animation =
            if animation.width.max(animation.height) * factor <= 256 { animation.scaled(factor) } else { animation };
        let size = animation.width.max(animation.height);
        // Each frame on the overlay's square canvas.
        let frames = animation
            .frames
            .iter()
            .map(|f| {
                let mut canvas = vec![0u8; (size * size * 4) as usize];
                for y in 0..f.image.height {
                    let from = (y * f.image.width * 4) as usize;
                    let to = (y * size * 4) as usize;
                    let n = (f.image.width * 4) as usize;
                    canvas[to..to + n].copy_from_slice(&f.image.pixels[from..from + n]);
                }
                (canvas, Duration::from_millis(u64::from(f.ms.max(10))))
            })
            .collect();
        Some(Self {
            window: overlay::Overlay::new(size)?,
            frames,
            size,
            hotspot: (animation.hotspot.0 as i32, animation.hotspot.1 as i32),
            robloxes: roblox_pids(),
            checked: Instant::now(),
            frame: 0,
            frame_since: Instant::now(),
            last: None,
        })
    }

    /// Draws (or hides) the cursor, then waits for the next screen refresh.
    fn tick(&mut self) {
        use feline_win::overlay;
        self.window.pump();
        if self.checked.elapsed() > Duration::from_millis(700) {
            self.robloxes = roblox_pids();
            self.checked = Instant::now();
        }
        // Only over a Roblox window that has the focus, and only while
        // Roblox shows a pointer (not while it holds the mouse).
        let show = overlay::pointer()
            .filter(|p| p.visible && self.robloxes.contains(&p.foreground_pid) && self.robloxes.contains(&p.over_pid));
        match show {
            Some(p) => {
                if self.frame_since.elapsed() >= self.frames[self.frame].1 {
                    self.frame = (self.frame + 1) % self.frames.len();
                    self.frame_since = Instant::now();
                }
                let at = (p.x - self.hotspot.0, p.y - self.hotspot.1, self.frame);
                if self.last != Some(at) {
                    self.window.draw(at.0, at.1, &self.frames[self.frame].0, self.size);
                    self.last = Some(at);
                }
            }
            None => {
                self.window.hide();
                self.last = None;
            }
        }
        if !overlay::wait_for_frame() {
            std::thread::sleep(Duration::from_millis(4));
        }
    }
}

/// `feline --with-studio <pid>`: the Discord status while Roblox Studio
/// runs, from the place in its window title.
pub fn run_studio_helper(pid: u32) -> std::process::ExitCode {
    let since = feline_core::history::now();
    let mut discord: Option<feline_core::presence::discord::Discord> = None;
    let mut shown: Option<serde_json::Value> = None;
    let mut tried = Instant::now() - Duration::from_secs(60);
    while feline_core::launch::alive(pid) {
        #[cfg(windows)]
        let place = feline_win::window::windows_of(pid)
            .into_iter()
            .find_map(|w| feline_core::studio::place_from_title(&feline_win::window::title(w)));
        #[cfg(not(windows))]
        let place: Option<String> = None;
        let activity = feline_core::studio::activity(place.as_deref(), since);
        if discord.is_none() && tried.elapsed() > Duration::from_secs(15) {
            tried = Instant::now();
            discord = feline_core::presence::discord::Discord::connect().ok();
            shown = None;
        }
        if let Some(d) = discord.as_mut()
            && shown.as_ref() != Some(&activity)
        {
            match d.set_activity(Some(activity.clone())) {
                Ok(()) => shown = Some(activity),
                Err(_) => discord = None,
            }
        }
        std::thread::sleep(Duration::from_secs(5));
    }
    if let Some(mut d) = discord {
        let _ = d.set_activity(None);
    }
    std::process::ExitCode::SUCCESS
}

/// Opens Roblox Studio (with a place file, if given): Windows with
/// Studio's flags, your mods (if asked) and its Discord status helper (if
/// asked); Linux through Vinegar. Lines for the user, or why not.
pub fn launch_studio(settings: &Settings, place: Option<&std::path::Path>) -> Result<Vec<String>, String> {
    if !cfg!(windows) {
        let vinegar = feline_core::studio::vinegar().ok_or("Studio on Linux needs Vinegar (vinegarhq.org)")?;
        let mut lines = Vec::new();
        // Your Studio icons (and mods, if asked) into the Studio Vinegar
        // installed. Its FastFlags and Discord status are Vinegar's own
        // settings.
        if let Some(dir) = feline_core::studio::find(None).and_then(|e| e.parent().map(std::path::Path::to_path_buf)) {
            let packs: Vec<feline_core::mods::ModPack> = feline_core::mods::mods_dir()
                .as_deref()
                .map(feline_core::mods::list_packs)
                .unwrap_or_default()
                .into_iter()
                .filter(|p| p.name == feline_core::studio::ICONS_MOD || (settings.studio_mods && settings.mods.contains(&p.name)))
                .collect();
            match feline_core::mods::apply(&feline_core::mods::ModTarget { dir, overlay: false }, &packs) {
                Ok(applied) if applied.files > 0 => lines.push(format!("{} mod files are in Studio.", applied.files)),
                Ok(_) => {}
                Err(e) => lines.push(format!("Mods didn't go into Studio: {e}.")),
            }
        }
        let command = feline_core::studio::vinegar_command(&vinegar, place).ok_or("Vinegar couldn't be started")?;
        feline_core::launch::detached(command)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Vinegar didn't start: {e}"))?;
        lines.insert(0, "Studio is opening in Vinegar.".to_owned());
        return Ok(lines);
    }
    let exe = feline_core::studio::find(std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from).as_deref())
        .ok_or("Roblox Studio isn't installed")?;
    let dir = exe.parent().map(std::path::Path::to_path_buf).unwrap_or_default();
    let mut lines = Vec::new();
    if let Err(e) = feline_core::studio::write_flags(&dir, &feline_core::studio::flags()) {
        lines.push(format!("Studio's flags weren't written: {e}."));
    }
    let mods_dir = feline_core::mods::mods_dir();
    let packs: Vec<feline_core::mods::ModPack> = mods_dir
        .as_deref()
        .map(feline_core::mods::list_packs)
        .unwrap_or_default()
        .into_iter()
        .filter(|p| {
            // Your Studio icons always; the rest only if asked.
            p.name == feline_core::studio::ICONS_MOD || (settings.studio_mods && settings.mods.contains(&p.name))
        })
        .collect();
    match feline_core::mods::apply(&feline_core::mods::ModTarget { dir: dir.clone(), overlay: false }, &packs) {
        Ok(applied) if applied.files > 0 => lines.push(format!("{} mod files are in Studio.", applied.files)),
        Ok(_) => {}
        Err(e) => lines.push(format!("Mods didn't go into Studio: {e}.")),
    }
    let mut command = Command::new(&exe);
    command.current_dir(&dir);
    if let Some(place) = place {
        command.arg(place);
    }
    let child = feline_core::launch::detached(command)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Studio didn't start: {e}"))?;
    lines.insert(0, "Studio is opening.".to_owned());
    if settings.studio_discord
        && let Ok(me) = std::env::current_exe()
    {
        let mut helper = Command::new(me);
        helper.args(["--with-studio", &child.id().to_string()]);
        let _ = feline_core::launch::detached(helper)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
    Ok(lines)
}

/// `feline --studio [file]`: Studio from a double-clicked place file (or
/// a shortcut), with a card saying what happened.
pub fn run_studio_command(place: Option<&str>) -> std::process::ExitCode {
    let settings = Settings::path().and_then(|p| Settings::load_from(&p).ok()).unwrap_or_default();
    let result = launch_studio(&settings, place.map(std::path::Path::new));
    let (title, text) = match &result {
        Ok(lines) => ("Opening Roblox Studio", lines.join(" ")),
        Err(e) => ("Studio didn't open", e.clone()),
    };
    #[cfg(windows)]
    {
        feline_win::overlay::card(title, &text, 4);
        std::thread::sleep(Duration::from_millis(4700));
    }
    #[cfg(not(windows))]
    feline_core::launch::notify(title, &text);
    if result.is_ok() { std::process::ExitCode::SUCCESS } else { std::process::ExitCode::FAILURE }
}

/// Place files (`.rbxl`, `.rbxlx`) open with Feline (`on`), or with what
/// opened them before. Returns what opened them before (to keep).
#[cfg(windows)]
pub fn set_place_files(on: bool, previous: Option<&str>) -> Result<Option<String>, String> {
    use feline_win::game::{associations_changed, delete_user_tree, set_user_string, user_string};
    const CLASSES: &str = r"Software\Classes";
    const PROG_ID: &str = "Feline.Place";
    let mut before = None;
    for ext in [".rbxl", ".rbxlx"] {
        let key = format!(r"{CLASSES}\{ext}");
        if on {
            let current = user_string(&key, None).filter(|v| v != PROG_ID);
            before = before.or(current);
            set_user_string(&key, None, PROG_ID);
        } else {
            match previous {
                Some(p) if !p.is_empty() => {
                    set_user_string(&key, None, p);
                }
                _ => feline_win::game::delete_user_value(&key, ""),
            }
        }
    }
    if on {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let exe = exe.display();
        set_user_string(&format!(r"{CLASSES}\{PROG_ID}"), None, "Roblox place (opens with Feline)");
        set_user_string(&format!(r"{CLASSES}\{PROG_ID}\shell\open\command"), None, &format!("\"{exe}\" --studio \"%1\""));
        if let Some(studio) = feline_core::studio::find(std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from).as_deref()) {
            set_user_string(&format!(r"{CLASSES}\{PROG_ID}\DefaultIcon"), None, &format!("{},0", studio.display()));
        }
    } else {
        delete_user_tree(&format!(r"{CLASSES}\{PROG_ID}"));
    }
    associations_changed();
    Ok(before)
}

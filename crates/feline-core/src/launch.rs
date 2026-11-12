// SPDX-License-Identifier: GPL-3.0-or-later
//! Starting a runtime for play, and watching it.
//!
//! Stability rules:
//! - The runtime gets **its own session** (and so its own process group), so
//!   it's cut off from the terminal Feline may have been started from: Ctrl+C,
//!   the terminal closing (SIGHUP) or job control never reach the game, and
//!   closing or crashing Feline never ends a game session. See [`detached`].
//!   On Windows: its own process group, with no console.
//! - All runtime output goes to a log file in Feline's log folder,
//!   never to a pipe Feline must keep reading. A pipe would stall the game if
//!   Feline stopped reading it (for example after Feline exits).
//! - Old logs are pruned so the folder doesn't grow forever.

use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::process::RunError;

/// How many runtime logs to keep.
const KEEP_LOGS: usize = 20;

/// A runtime process that Feline started.
#[derive(Debug)]
pub struct RunningGame {
    child: Child,
    started: Instant,
    pub log_path: PathBuf,
}

/// How a runtime process ended.
#[derive(Debug)]
pub struct GameExit {
    pub status: ExitStatus,
    pub ran_for: Duration,
    pub log_path: PathBuf,
}

#[derive(Debug)]
pub enum LaunchError {
    /// The log file couldn't be created.
    LogFile { path: PathBuf, source: io::Error },
    /// The runtime couldn't be started.
    Start(RunError),
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LogFile { path, source } => {
                write!(
                    f,
                    "couldn't create the log file {}: {source}",
                    path.display()
                )
            }
            Self::Start(RunError::NotFound { program }) if program == "flatpak" => f.write_str(
                "the `flatpak` command was not found, so the Flatpak runtime can't be started",
            ),
            Self::Start(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for LaunchError {}

/// Makes `command` run in a new session, detached from Feline's terminal.
///
/// Rust's own `CommandExt::setsid` isn't stable yet, and calling `setsid()`
/// ourselves would need `unsafe` code, so the standard `setsid` program
/// (util-linux, on practically every distribution) does it: `setsid <program>
/// <args>`. Because the child isn't a process-group leader, `setsid` replaces
/// itself with the program under the **same process ID**, so waiting for it,
/// monitoring it and its exit status work unchanged.
///
/// Falls back to just a new process group (still safe from Ctrl+C) if
/// `setsid` isn't installed, or if the program itself can't be found: then
/// starting it directly gives the clear "not found" error.
#[cfg(unix)]
pub fn detached(command: Command) -> Command {
    use std::os::unix::process::CommandExt;

    let program_exists = {
        let program = Path::new(command.get_program());
        if program.components().count() > 1 {
            program.is_file()
        } else {
            which(&program.to_string_lossy()).is_some()
        }
    };
    let setsid = which("setsid").filter(|_| program_exists);
    let Some(setsid) = setsid else {
        let mut command = command;
        command.process_group(0);
        return command;
    };
    let mut wrapped = Command::new(setsid);
    wrapped.arg(command.get_program()).args(command.get_args());
    for (key, value) in command.get_envs() {
        match value {
            Some(value) => wrapped.env(key, value),
            None => wrapped.env_remove(key),
        };
    }
    if let Some(dir) = command.get_current_dir() {
        wrapped.current_dir(dir);
    }
    wrapped
}

/// Makes `command` run in its own process group, without a console window,
/// so Ctrl+C or closing the terminal Feline was started from never reaches
/// it, and closing Feline doesn't end it (Windows doesn't end child
/// processes with their parent).
#[cfg(windows)]
pub fn detached(mut command: Command) -> Command {
    use std::os::windows::process::CommandExt;
    // From the Windows API's process creation flags.
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    command
}

/// The first executable called `name` on `PATH`.
#[cfg(unix)]
fn which(name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|p| {
            p.metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

/// Starts `command` in its own session (see [`detached`]) with output going
/// to a new log file in `log_dir`, named after `label` and the current time.
pub fn spawn(command: Command, log_dir: &Path, label: &str) -> Result<RunningGame, LaunchError> {
    let mut command = detached(command);
    let log_path = log_dir.join(format!("{label}-{}.log", unix_seconds()));
    let log = fs::create_dir_all(log_dir)
        .and_then(|()| fs::File::create(&log_path))
        .map_err(|source| LaunchError::LogFile {
            path: log_path.clone(),
            source,
        })?;
    let log_for_stderr = log.try_clone().map_err(|source| LaunchError::LogFile {
        path: log_path.clone(),
        source,
    })?;

    command
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(log_for_stderr);

    let program = command.get_program().to_string_lossy().into_owned();
    let child = command.spawn().map_err(|source| {
        LaunchError::Start(match source.kind() {
            io::ErrorKind::NotFound => RunError::NotFound { program },
            _ => RunError::Spawn { program, source },
        })
    })?;

    prune_logs(log_dir, KEEP_LOGS);
    Ok(RunningGame {
        child,
        started: Instant::now(),
        log_path,
    })
}

impl RunningGame {
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Blocks until the runtime exits. Call it on a background thread.
    pub fn wait(mut self) -> io::Result<GameExit> {
        let status = self.child.wait()?;
        Ok(GameExit {
            status,
            ran_for: self.started.elapsed(),
            log_path: self.log_path,
        })
    }
}

/// Shows a small notification on the desktop (best effort; never blocks).
/// Windows: a message box on top of other windows. Linux: `notify-send`.
pub fn notify(title: &str, text: &str) {
    #[cfg(windows)]
    feline_win::game::notify(title, text);
    #[cfg(not(windows))]
    {
        let _ = Command::new("notify-send")
            .args(["--app-name=Feline", title, text])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}

/// A quiet notification that doesn't take the focus (for while a game
/// runs). Windows: Feline's own little card in the corner of the game's
/// screen (Windows holds its own notifications back during full-screen
/// games). Linux: `notify-send`, which is quiet anyway.
pub fn notify_quiet(title: &str, text: &str) {
    #[cfg(windows)]
    feline_win::overlay::card(title, text, 5);
    #[cfg(not(windows))]
    notify(title, text);
}

/// `command` started through `wrapper` (e.g. `gamescope -f --` or
/// `distrobox enter games --`): the wrapper's words, then the command.
/// Words are split on spaces, with double quotes keeping words together.
/// An empty wrapper leaves the command as it is.
pub fn wrap(command: Command, wrapper: &str) -> Command {
    let words = crate::settings::Program { path: String::new(), args: wrapper.to_owned(), close_after: false }.arg_list();
    let Some((program, args)) = words.split_first() else { return command };
    let mut wrapped = Command::new(program);
    wrapped.args(args).arg(command.get_program()).args(command.get_args());
    for (key, value) in command.get_envs() {
        match value {
            Some(value) => wrapped.env(key, value),
            None => wrapped.env_remove(key),
        };
    }
    if let Some(dir) = command.get_current_dir() {
        wrapped.current_dir(dir);
    }
    wrapped
}

/// Puts `text` on the clipboard. Windows: directly. Linux: `wl-copy`
/// (Wayland) or `xclip` (X11), whichever is there. False if neither worked.
pub fn copy_text(text: &str) -> bool {
    #[cfg(windows)]
    {
        feline_win::tray::copy_text(text)
    }
    #[cfg(not(windows))]
    {
        use std::io::Write as _;
        let tries: [&[&str]; 2] = [&["wl-copy"], &["xclip", "-selection", "clipboard"]];
        tries.iter().any(|args| {
            let Ok(mut child) = Command::new(args[0])
                .args(&args[1..])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            else {
                return false;
            };
            let wrote = child.stdin.take().is_some_and(|mut i| i.write_all(text.as_bytes()).is_ok());
            wrote && child.wait().is_ok_and(|s| s.success())
        })
    }
}

/// A program Feline started next to Roblox.
pub struct Companion {
    pub name: String,
    child: Child,
    close_after: bool,
}

/// Starts the programs that go with Roblox. Returns them, and a line for
/// each one that couldn't start.
pub fn start_programs(programs: &[crate::settings::Program]) -> (Vec<Companion>, Vec<String>) {
    let mut started = Vec::new();
    let mut problems = Vec::new();
    for program in programs {
        let path = Path::new(&program.path);
        let mut command = Command::new(path);
        command.args(program.arg_list());
        if let Some(dir) = path.parent().filter(|d| d.is_dir()) {
            command.current_dir(dir);
        }
        command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // Its own process group, so it's independent of Feline.
            command.creation_flags(0x0000_0200);
        }
        match command.spawn() {
            Ok(child) => started.push(Companion { name: program.name(), child, close_after: program.close_after }),
            Err(e) => problems.push(format!("{} didn't start: {e}", program.name())),
        }
    }
    (started, problems)
}

/// Closes the programs that were set to close with Roblox: first asks
/// them nicely (their windows' close button, or SIGTERM), and only ends
/// them if they're still there a few seconds later.
pub fn close_programs(companions: Vec<Companion>) {
    let mut closing: Vec<Companion> = companions.into_iter().filter(|c| c.close_after).collect();
    for c in &mut closing {
        if matches!(c.child.try_wait(), Ok(Some(_))) {
            continue;
        }
        #[cfg(windows)]
        for window in feline_win::window::windows_of(c.child.id()) {
            feline_win::window::ask_to_close(window);
        }
        #[cfg(not(windows))]
        let _ = Command::new("kill")
            .args(["-TERM", &c.child.id().to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && closing.iter_mut().any(|c| matches!(c.child.try_wait(), Ok(None))) {
        std::thread::sleep(Duration::from_millis(200));
    }
    for mut c in closing {
        if matches!(c.child.try_wait(), Ok(None)) {
            let _ = c.child.kill();
        }
        let _ = c.child.wait();
    }
}

/// Whether the process `pid` is still running.
pub fn alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        feline_win::game::alive(pid)
    }
    #[cfg(not(windows))]
    {
        // A finished child of another process can linger as a "zombie"
        // until it's collected; that's not running.
        fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|s| s.rsplit_once(')').map(|(_, rest)| rest.trim_start().to_owned()))
            .is_some_and(|rest| !rest.starts_with('Z') && !rest.starts_with('X'))
    }
}

/// The text of the break reminder after `minutes` of play.
pub fn break_reminder(minutes: u64) -> (String, String) {
    let played = match minutes {
        m if m < 60 => format!("{m} minutes"),
        60 => "an hour".to_owned(),
        m if m % 60 == 0 => format!("{} hours", m / 60),
        m => format!("{} h {} min", m / 60, m % 60),
    };
    (
        "Time for a cat nap?".to_owned(),
        format!(
            "You've been playing for {played}. Stretch your paws, drink some water, \
             rest your eyes. Your game is still there when you're back."
        ),
    )
}

/// The last `max_lines` lines of a log file (reads at most the last 64 KiB).
pub fn log_tail(path: &Path, max_lines: usize) -> String {
    const WINDOW: u64 = 64 * 1024;
    let Ok(mut file) = fs::File::open(path) else {
        return String::new();
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    if file
        .seek(SeekFrom::Start(len.saturating_sub(WINDOW)))
        .is_err()
    {
        return String::new();
    }
    let mut bytes = Vec::new();
    let _ = file.read_to_end(&mut bytes);
    let text = String::from_utf8_lossy(&bytes);
    crate::runtime::tail_lines(&text, max_lines)
}

/// Deletes all but the newest `keep` `.log` files in `dir`. Best effort:
/// failing to delete an old log must never stop a game from starting.
fn prune_logs(dir: &Path, keep: usize) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut logs: Vec<(SystemTime, PathBuf)> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "log"))
        .filter_map(|p| Some((p.metadata().ok()?.modified().ok()?, p)))
        .collect();
    logs.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified)); // newest first
    for (_, path) in logs.into_iter().skip(keep) {
        let _ = fs::remove_file(path);
    }
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("feline-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[cfg(unix)]
    #[test]
    fn output_goes_to_log_and_exit_status_is_reported() {
        let dir = temp_dir("launch-log");
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "echo hello; echo oops >&2; exit 3"]);
        let game = spawn(cmd, &dir, "test").expect("starts");
        let exit = game.wait().expect("waits");
        let log = fs::read_to_string(&exit.log_path).unwrap();
        let tail = log_tail(&exit.log_path, 1);
        let _ = fs::remove_dir_all(&dir);

        assert_eq!(exit.status.code(), Some(3));
        assert!(log.contains("hello") && log.contains("oops"), "{log}");
        assert_eq!(tail, "oops");
    }

    #[cfg(unix)]
    #[test]
    fn runtime_gets_its_own_session_with_the_same_pid() {
        let dir = temp_dir("launch-pgid");
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "sleep 1"]);
        let game = spawn(cmd, &dir, "test").expect("starts");
        let pid = game.pid();
        // `setsid` hands over to the program a moment after starting, so
        // look until the process is the program (or give up after 2 s).
        // Fields of /proc/<pid>/stat after the name in parentheses:
        // [0] state, [1] ppid, [2] pgrp, [3] session.
        let read = || {
            let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
            let after_name = stat.rsplit_once(')').unwrap().1.to_owned();
            let field = |n: usize| -> u32 {
                after_name
                    .split_whitespace()
                    .nth(n)
                    .unwrap()
                    .parse()
                    .unwrap()
            };
            let comm = fs::read_to_string(format!("/proc/{pid}/comm")).unwrap();
            (field(2), field(3), comm.trim().to_owned())
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        let (mut pgid, mut sid, mut comm) = read();
        while comm == "setsid" && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
            (pgid, sid, comm) = read();
        }
        let _ = game.wait();
        let _ = fs::remove_dir_all(&dir);

        assert_eq!(pgid, pid, "the game leads its own process group");
        if which("setsid").is_some() {
            assert_eq!(sid, pid, "and its own session, away from Feline's terminal");
            // `setsid` replaced itself with the program: same PID.
            assert_eq!(comm, "sh");
        }
    }

    #[test]
    fn missing_program_is_a_clear_error() {
        let dir = temp_dir("launch-missing");
        let err = spawn(Command::new("feline-no-such-runtime"), &dir, "test").unwrap_err();
        let _ = fs::remove_dir_all(&dir);
        assert!(
            matches!(err, LaunchError::Start(RunError::NotFound { .. })),
            "{err}"
        );
    }

    #[test]
    fn wrappers_go_in_front() {
        let mut game = Command::new("flatpak");
        game.args(["run", "org.vinegarhq.Sober"]).env("A", "1");
        let wrapped = wrap(game, r#"gamescope -W 1920 -H 1080 -f --"#);
        assert_eq!(wrapped.get_program(), "gamescope");
        let args: Vec<_> = wrapped.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(args, ["-W", "1920", "-H", "1080", "-f", "--", "flatpak", "run", "org.vinegarhq.Sober"]);
        assert_eq!(wrapped.get_envs().count(), 1);
        assert_eq!(wrap(Command::new("x"), "  ").get_program(), "x");
    }

    #[test]
    fn programs_start_and_close_with_the_game() {
        use crate::settings::Program;
        let (program, args) = if cfg!(windows) {
            ("C:/Windows/System32/PING.EXE", "-n 30 127.0.0.1")
        } else {
            ("/bin/sleep", "30")
        };
        if !Path::new(program).is_file() {
            return;
        }
        let programs = [
            Program { path: program.into(), args: args.into(), close_after: true },
            Program { path: "/no/such/program".into(), args: String::new(), close_after: true },
        ];
        let (started, problems) = start_programs(&programs);
        assert_eq!((started.len(), problems.len()), (1, 1), "{problems:?}");
        let pid = started[0].child.id();
        assert!(alive(pid));
        let begin = Instant::now();
        close_programs(started);
        assert!(!alive(pid));
        assert!(begin.elapsed() < Duration::from_secs(8));
    }

    #[test]
    fn old_logs_are_pruned() {
        let dir = temp_dir("launch-prune");
        fs::create_dir_all(&dir).unwrap();
        for i in 0..5 {
            fs::write(dir.join(format!("{i}.log")), "x").unwrap();
        }
        fs::write(dir.join("keep.txt"), "not a log").unwrap();
        prune_logs(&dir, 2);
        let remaining = fs::read_dir(&dir).unwrap().count();
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(remaining, 3, "2 logs + the non-log file");
    }
}

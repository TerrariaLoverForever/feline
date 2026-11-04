// SPDX-License-Identifier: GPL-3.0-or-later
//! Starting a runtime for play, and watching it.
//!
//! Stability rules:
//! - The runtime gets **its own session** (and so its own process group), so
//!   it's cut off from the terminal Feline may have been started from: Ctrl+C,
//!   the terminal closing (SIGHUP) or job control never reach the game, and
//!   closing or crashing Feline never ends a game session. See [`detached`].
//! - All runtime output goes to a log file in `~/.local/state/feline/logs/`,
//!   never to a pipe Feline must keep reading. A pipe would stall the game if
//!   Feline stopped reading it (for example after Feline exits).
//! - Old logs are pruned so the folder doesn't grow forever.

use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::process::CommandExt;
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
pub fn detached(command: Command) -> Command {
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

/// The first executable called `name` on `PATH`.
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

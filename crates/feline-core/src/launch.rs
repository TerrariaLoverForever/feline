// SPDX-License-Identifier: GPL-3.0-or-later
//! Starting a runtime for play, and watching it.
//!
//! Stability rules:
//! - The runtime gets **its own process group**. Signals aimed at Feline
//!   (Ctrl+C in the terminal, the terminal closing) don't reach the game, and
//!   closing or crashing Feline never ends a game session.
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

/// Starts `command` in its own process group with output going to a new log
/// file in `log_dir`, named after `label` and the current time.
pub fn spawn(
    mut command: Command,
    log_dir: &Path,
    label: &str,
) -> Result<RunningGame, LaunchError> {
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
        .stderr(log_for_stderr)
        // 0 = "make a new group whose ID is the child's own PID".
        .process_group(0);

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
    fn runtime_gets_its_own_process_group() {
        let dir = temp_dir("launch-pgid");
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "sleep 1"]);
        let game = spawn(cmd, &dir, "test").expect("starts");
        let pid = game.pid();
        // Field 5 of /proc/<pid>/stat is the process group ID. The command
        // name (field 2) is in parentheses, so split after the last ')'.
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        let after_name = stat.rsplit_once(')').unwrap().1;
        let pgid: u32 = after_name
            .split_whitespace()
            .nth(2)
            .unwrap()
            .parse()
            .unwrap();
        let _ = game.wait();
        let _ = fs::remove_dir_all(&dir);

        assert_eq!(pgid, pid, "the game leads its own process group");
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

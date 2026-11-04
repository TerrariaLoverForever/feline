// SPDX-License-Identifier: GPL-3.0-or-later
//! Running short-lived helper commands safely.
//!
//! Used for things like `cordial --doctor`: start a program, capture its
//! output, and give up after a timeout instead of hanging the launcher.
//!
//! Stability details worth knowing:
//! - stdout and stderr are read on separate threads. Reading them one after
//!   the other can deadlock: if the child fills the stderr pipe while we wait
//!   on stdout, both sides wait forever.
//! - Output is capped ([`MAX_OUTPUT`]) so a runaway program can't eat memory.
//! - On timeout the child is killed. We don't wait for the reader threads in
//!   that case, because a grandchild process may still hold the pipes open.

use std::fmt;
use std::io::{self, Read};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// At most this many bytes are kept from each of stdout and stderr.
pub const MAX_OUTPUT: u64 = 1024 * 1024;

/// How often we check whether the child has exited.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// The result of a command that ran to completion.
#[derive(Debug)]
pub struct Captured {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

/// Why a command couldn't be run to completion.
#[derive(Debug)]
pub enum RunError {
    /// The program doesn't exist (or isn't on `PATH`).
    NotFound { program: String },
    /// The program exists but couldn't be started (permissions, …).
    Spawn { program: String, source: io::Error },
    /// The program didn't finish in time and was killed.
    TimedOut { program: String, after: Duration },
    /// Waiting for the program failed (very unusual).
    Wait { program: String, source: io::Error },
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { program } => write!(f, "`{program}` was not found"),
            Self::Spawn { program, source } => write!(f, "couldn't start `{program}`: {source}"),
            Self::TimedOut { program, after } => write!(
                f,
                "`{program}` didn't finish within {} seconds and was stopped",
                after.as_secs()
            ),
            Self::Wait { program, source } => {
                write!(
                    f,
                    "lost track of `{program}` while it was running: {source}"
                )
            }
        }
    }
}

impl std::error::Error for RunError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Spawn { source, .. } | Self::Wait { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Runs `cmd` to completion, capturing stdout and stderr, with a timeout.
pub fn run_captured(mut cmd: Command, timeout: Duration) -> Result<Captured, RunError> {
    let program = cmd.get_program().to_string_lossy().into_owned();

    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|source| match source.kind() {
        io::ErrorKind::NotFound => RunError::NotFound {
            program: program.clone(),
        },
        _ => RunError::Spawn {
            program: program.clone(),
            source,
        },
    })?;

    // `take()` moves the pipe handles out of `child`, so each thread owns one.
    let stdout = child.stdout.take().map(spawn_reader);
    let stderr = child.stderr.take().map(spawn_reader);

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                // Best effort: the child may have exited in the meantime.
                let _ = child.kill();
                let _ = child.wait();
                return Err(RunError::TimedOut {
                    program,
                    after: timeout,
                });
            }
            Ok(None) => thread::sleep(POLL_INTERVAL),
            Err(source) => return Err(RunError::Wait { program, source }),
        }
    };

    // The child has exited, so its pipes close and the readers finish.
    let collect = |reader: Option<thread::JoinHandle<String>>| {
        reader
            .and_then(|handle| handle.join().ok())
            .unwrap_or_default()
    };
    Ok(Captured {
        status,
        stdout: collect(stdout),
        stderr: collect(stderr),
    })
}

/// Reads a pipe to the end on a background thread (up to [`MAX_OUTPUT`]).
fn spawn_reader(pipe: impl Read + Send + 'static) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut limited = pipe.take(MAX_OUTPUT);
        // A read error just ends the output early; what we have is still useful.
        let _ = limited.read_to_end(&mut bytes);
        // Drain (and discard) anything past the limit so the child never
        // blocks on a full pipe.
        let _ = io::copy(&mut limited.into_inner(), &mut io::sink());
        String::from_utf8_lossy(&bytes).into_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str) -> Command {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", script]);
        cmd
    }

    #[test]
    fn captures_both_streams_and_status() {
        let out = run_captured(sh("echo out; echo err >&2; exit 3"), Duration::from_secs(5))
            .expect("runs");
        assert_eq!(out.stdout, "out\n");
        assert_eq!(out.stderr, "err\n");
        assert_eq!(out.status.code(), Some(3));
    }

    #[test]
    fn large_stderr_does_not_deadlock() {
        // 256 KiB on stderr is far more than a pipe buffer (64 KiB).
        let script = "head -c 262144 /dev/zero >&2; echo done";
        let out = run_captured(sh(script), Duration::from_secs(10)).expect("runs");
        assert_eq!(out.stdout, "done\n");
        assert_eq!(out.stderr.len(), 262_144);
    }

    #[test]
    fn times_out_and_kills() {
        let started = Instant::now();
        let err = run_captured(sh("exec sleep 10"), Duration::from_millis(200)).unwrap_err();
        assert!(matches!(err, RunError::TimedOut { .. }), "{err}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn missing_program_is_reported() {
        let err = run_captured(
            Command::new("feline-definitely-missing-program"),
            Duration::from_secs(1),
        )
        .unwrap_err();
        assert!(matches!(err, RunError::NotFound { .. }), "{err}");
    }
}

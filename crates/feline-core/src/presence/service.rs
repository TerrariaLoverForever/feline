// SPDX-License-Identifier: GPL-3.0-or-later
//! Feline's Discord status helper (`feline --presence`): a small process that
//! lives exactly as long as one game, like the status built into Sober's or
//! Stud's own game process. Feline starts it next to the game, so closing
//! Feline's window doesn't end the status; when the game ends, it exits.
//! Nothing starts it at login.
//!
//! It checks every two seconds which runtime is running (by process name,
//! `/proc/<pid>/comm`), follows that runtime's Roblox log with a [`Session`],
//! and exits once the game has ended (or if no game appears within a minute
//! and a half).
//!
//! **Only one status at a time.** The helper holds a lock on
//! `~/.local/state/feline/presence.lock` (with its process ID inside). A
//! second helper sees the lock and exits at once. The kernel releases the
//! lock when the process ends, even after a crash, so it can't go stale.

use std::fs::{self, File, TryLockError};
use std::io::{self, Write as _};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, SystemTime};

use super::Session;
use crate::paths;
use crate::runtime::config::any_process_named;
use crate::runtime::{DetectContext, RuntimeBackend, backends, detect_all};
use crate::settings::Settings;

const CHECK_EVERY: Duration = Duration::from_secs(2);
/// How long to wait for the game to appear after the helper starts.
const STARTUP_GRACE: Duration = Duration::from_secs(90);

fn lock_path() -> Option<PathBuf> {
    paths::state_dir().map(|d| d.join("presence.lock"))
}

/// The service's last status line, for the Settings page.
pub fn status_path() -> Option<PathBuf> {
    paths::state_dir().map(|d| d.join("presence.status"))
}

/// Proof that this process is the one showing the status. Dropping it (or
/// exiting) releases the lock.
pub struct ServiceLock {
    _file: File,
}

/// Takes the lock, or returns `None` if another Feline already holds it.
pub fn try_lock() -> io::Result<Option<ServiceLock>> {
    let path = lock_path().ok_or_else(|| io::Error::other("no home folder"))?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    // Opened without truncating: another process may hold it.
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)?;
    match file.try_lock() {
        Ok(()) => {
            file.set_len(0)?;
            writeln!(file, "{}", std::process::id())?;
            Ok(Some(ServiceLock { _file: file }))
        }
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(e)) => Err(e),
    }
}

/// The process ID of the running helper, if there is one.
pub fn running_service() -> Option<u32> {
    let path = lock_path()?;
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .ok()?;
    match file.try_lock() {
        // We got the lock, so nobody holds it. Dropping `file` releases it.
        Ok(()) => None,
        Err(TryLockError::WouldBlock) => fs::read_to_string(&path).ok()?.trim().parse().ok(),
        Err(TryLockError::Error(_)) => None,
    }
}

/// The running runtime, if any (the first one found).
fn running_backend() -> Option<&'static dyn RuntimeBackend> {
    backends()
        .iter()
        .copied()
        .find(|b| any_process_named(b.process_names()))
}

/// Shows the status for the game that is starting or running, and returns
/// when it has ended. Returns at once if another helper is already running.
pub fn run_for_game(on_status: impl Fn(String) + Send + Sync + Clone + 'static) -> io::Result<()> {
    let Some(_lock) = try_lock()? else {
        return Ok(()); // another helper already shows the status
    };
    let write_status = move |text: String| {
        if let Some(path) = status_path() {
            let _ = fs::write(path, format!("{text}\n"));
        }
        on_status(text);
    };
    let settings = Settings::path()
        .and_then(|p| Settings::load_from(&p).ok())
        .unwrap_or_default();
    if !settings.overrides.discord_presence {
        return Ok(());
    }
    write_status("Waiting for the game".to_owned());

    let started = std::time::Instant::now();
    let mut session: Option<(&'static str, Session)> = None;
    loop {
        match (&session, running_backend()) {
            (Some((id, _)), Some(backend)) if *id == backend.id() => {}
            // The game ended (or the player switched runtimes, which ends
            // this game too).
            (Some(_), _) => break,
            (None, Some(backend)) => {
                session =
                    start_for(backend, &settings, write_status.clone()).map(|s| (backend.id(), s));
            }
            (None, None) if started.elapsed() > STARTUP_GRACE => break,
            (None, None) => {}
        }
        thread::sleep(CHECK_EVERY);
    }
    if let Some((_, s)) = session {
        s.stop();
    }
    write_status("Not showing a status (no game running)".to_owned());
    Ok(())
}

fn start_for(
    backend: &'static dyn RuntimeBackend,
    settings: &Settings,
    on_status: impl Fn(String) + Send + 'static,
) -> Option<Session> {
    let ctx = DetectContext::host();
    // Every install of this runtime: it isn't known which one is running.
    let mut roots: Vec<PathBuf> = detect_all(&ctx)
        .iter()
        .filter(|i| i.backend.id() == backend.id())
        .flat_map(|i| i.roblox_log_roots(&ctx))
        .collect();
    roots.sort();
    roots.dedup();
    // The game may have started a little before we noticed; a log written
    // in the last minute belongs to it. The follower reads the whole log, so
    // a game that's already running is picked up in its current state.
    let since = SystemTime::now() - Duration::from_secs(60);
    let name = backend.name();
    // A hidden game counts in every place of it Feline has seen.
    let history = crate::history::History::load();
    let mut hidden_universes: Vec<u64> = settings
        .discord
        .hidden
        .iter()
        .filter_map(|p| history.universe_of(*p))
        .collect();
    hidden_universes.sort_unstable();
    hidden_universes.dedup();
    let extra = super::Extra {
        settings: settings.discord.clone(),
        hidden_universes,
        // On Windows there's only Roblox's own client; on Linux the
        // runtime is worth showing.
        runtime: (!cfg!(windows)).then(|| name.to_owned()),
        game_status_allowed: settings.game_status,
        ..Default::default()
    };
    Session::start(roots, since, settings.presence, extra, move |status| {
        on_status(format!("{name}: {status}"));
    })
    .ok()
}

#[cfg(test)]
mod tests {
    #[test]
    fn file_locks_exclude_a_second_holder() {
        // The mechanism the service relies on: a second open of the same file
        // can't take the lock while the first holds it, and can afterwards.
        let path = std::env::temp_dir().join(format!("feline-test-lock-{}", std::process::id()));
        let first = std::fs::File::create(&path).unwrap();
        first.try_lock().unwrap();
        let second = std::fs::File::open(&path).unwrap();
        assert!(matches!(
            second.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        drop(first);
        assert!(second.try_lock().is_ok());
        let _ = std::fs::remove_file(&path);
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! Keeping Roblox's client settings ("FastFlags") current.
//!
//! Roblox's Android app downloads its settings
//! (`clientsettingscdn.roblox.com/v2/settings/application/AndroidApp`)
//! every time it starts, and hands them to the engine with
//! `nativeInitClientSettings`. New features are turned on through these
//! settings: the chat's emoji (expression) tray and friends tab came that
//! way. Feline used to read a copy saved once, at setup, so the app never
//! got anything Roblox turned on after that day.
//!
//! Now the runtime downloads them in the background as it starts (while the
//! engine's constructors run) and saves them over `ClientSettings.json` once
//! they look right. The engine's start waits for the download a few seconds
//! at most; without a connection the saved copy is used, as before.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

const URL: &str = "https://clientsettingscdn.roblox.com/v2/settings/application/AndroidApp";
/// The download's own time limit.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15);
/// Fewer settings than this isn't a real answer (there are about 25 000).
const MIN_SETTINGS: usize = 1000;

/// Signals the end of the download started by [`start_refresh`].
static DONE: Mutex<Option<Receiver<()>>> = Mutex::new(None);

/// Starts downloading the current settings into `<runtime>/ClientSettings.json`.
/// Does nothing when `FELINE_SETTINGS` or `FELINE_SETTINGS_FILE` chooses the
/// settings.
pub fn start_refresh(runtime: PathBuf) {
    if std::env::var_os("FELINE_SETTINGS").is_some() || std::env::var_os("FELINE_SETTINGS_FILE").is_some() {
        return;
    }
    let (tx, rx) = channel();
    *DONE.lock().unwrap_or_else(|p| p.into_inner()) = Some(rx);
    let spawned = std::thread::Builder::new()
        .name("feline-settings".into())
        .spawn(move || {
            match download().and_then(|json| save(&runtime.join("ClientSettings.json"), &json)) {
                Ok(count) => eprintln!("feline-runtime: client settings updated from Roblox ({count} settings)"),
                Err(e) => eprintln!("feline-runtime: couldn't update the client settings ({e}); using the saved ones"),
            }
            let _ = tx.send(());
        });
    if spawned.is_err() {
        *DONE.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }
}

/// Waits up to `limit` for the download to finish (it usually has by the
/// time the engine asks for the settings).
pub fn wait(limit: Duration) {
    let Some(rx) = DONE.lock().unwrap_or_else(|p| p.into_inner()).take() else {
        return;
    };
    if rx.recv_timeout(limit).is_err() {
        eprintln!("feline-runtime: the client settings download is slow; using the saved ones this time");
    }
}

fn download() -> Result<String, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(DOWNLOAD_TIMEOUT))
        .build()
        .into();
    agent
        .get(URL)
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .with_config()
        .limit(16 << 20)
        .read_to_string()
        .map_err(|e| e.to_string())
}

/// The number of settings in a downloaded answer, if it looks right.
fn check(json: &str) -> Result<usize, String> {
    let value: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("not JSON: {e}"))?;
    let count = value
        .get("applicationSettings")
        .and_then(serde_json::Value::as_object)
        .ok_or("no applicationSettings")?
        .len();
    if count < MIN_SETTINGS {
        return Err(format!("only {count} settings"));
    }
    Ok(count)
}

/// Replaces `file` with `json` (written next to it, then renamed, so a
/// half-written file is never read).
fn save(file: &Path, json: &str) -> Result<usize, String> {
    let count = check(json)?;
    let temporary = file.with_extension("json.new");
    std::fs::write(&temporary, json).map_err(|e| e.to_string())?;
    std::fs::rename(&temporary, file).map_err(|e| e.to_string())?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_answers() {
        let many: serde_json::Map<String, serde_json::Value> =
            (0..MIN_SETTINGS).map(|i| (format!("FFlag{i}"), "True".into())).collect();
        let good = serde_json::json!({ "applicationSettings": many }).to_string();
        assert_eq!(check(&good), Ok(MIN_SETTINGS));
        assert!(check(r#"{"applicationSettings":{"FFlagA":"True"}}"#).is_err());
        assert!(check(r#"{"errors":[{"code":0}]}"#).is_err());
        assert!(check("<html>").is_err());
    }
}

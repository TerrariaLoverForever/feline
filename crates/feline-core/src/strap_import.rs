// SPDX-License-Identifier: GPL-3.0-or-later
//! Taking over settings from Bloxstrap and its forks (Fishstrap, Froststrap,
//! Voidstrap): their `Settings.json` and their FastFlags. Their mods are
//! imported separately (see [`crate::mods::find_bootstrappers`]).
//!
//! Only what Feline has an equivalent for is taken; field names are
//! Bloxstrap's (`Bloxstrap/Models/Persistable/Settings.cs`), which the
//! forks keep. Unknown or odd values are skipped, never guessed.

use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::settings::{Program, Settings};

/// Copies what Feline understands from the bootstrapper folder `dir` (e.g.
/// `%LOCALAPPDATA%\Bloxstrap`) into `settings`. Returns a line per thing
/// taken over, for the user.
pub fn import_settings(dir: &Path, settings: &mut Settings) -> Result<Vec<String>, String> {
    let mut done = Vec::new();
    let file = dir.join("Settings.json");
    let text = fs::read_to_string(&file).ok();
    if let Some(text) = &text {
        let v: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
            .map_err(|e| format!("its Settings.json couldn't be read ({e})"))?;
        let flag = |key: &str| v.get(key).and_then(Value::as_bool);
        if let Some(on) = flag("UseDiscordRichPresence") {
            settings.overrides.discord_presence = on;
            done.push(format!("Discord status {}", if on { "on" } else { "off" }));
        }
        if let Some(on) = flag("ShowAccountOnRichPresence") {
            settings.presence.show_user = on;
        }
        if let Some(hide) = flag("HideRPCButtons") {
            settings.presence.join_button = !hide;
            settings.presence.game_button = !hide;
        }
        if let Some(on) = flag("ShowServerDetails") {
            settings.server_popup = on;
            if on {
                done.push("the server location pop-up".to_owned());
            }
        }
        if let Some(on) = flag("ConfirmLaunches") {
            settings.confirm_launch = on;
        }
        // The forks' "more than one Roblox" switch (named a little
        // differently in each).
        if let Some(on) = v
            .as_object()
            .into_iter()
            .flatten()
            .find(|(k, _)| k.to_ascii_lowercase().contains("multiinstance"))
            .and_then(|(_, v)| v.as_bool())
        {
            settings.multi_roblox = on;
            if on {
                done.push("more than one Roblox at once".to_owned());
            }
        }
        let mut added = 0;
        for item in v.get("CustomIntegrations").and_then(Value::as_array).into_iter().flatten() {
            let Some(path) = item.get("Location").and_then(Value::as_str).filter(|p| !p.trim().is_empty()) else {
                continue;
            };
            if settings.programs.iter().any(|p| p.path == path) {
                continue;
            }
            settings.programs.push(Program {
                path: path.to_owned(),
                args: item.get("LaunchArgs").and_then(Value::as_str).unwrap_or_default().replace(['\n', '|'], " "),
                close_after: item.get("AutoClose").and_then(Value::as_bool).unwrap_or(true),
            });
            added += 1;
        }
        if added > 0 {
            done.push(format!("{added} program{} that start with Roblox", if added == 1 { "" } else { "s" }));
        }
    }
    // FastFlags: kept with their mods, in Roblox's own file format.
    let flags_file = dir.join("Modifications").join("ClientSettings").join("ClientAppSettings.json");
    if let Ok(flags) = fs::read_to_string(&flags_file)
        && let Ok((set, ignored)) = crate::fflags::from_json(&flags)
    {
        let count = set.len();
        settings.fflags.extend(set);
        if count > 0 {
            done.push(format!("{count} FastFlag{}", if count == 1 { "" } else { "s" }));
        }
        if !ignored.is_empty() {
            done.push(format!(
                "(left out {} FastFlag{} Roblox no longer reads)",
                ignored.len(),
                if ignored.len() == 1 { "" } else { "s" }
            ));
        }
    }
    if text.is_none() && done.is_empty() {
        return Err("there are no settings in it".to_owned());
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bloxstrap_settings_come_over() {
        let dir = std::env::temp_dir().join(format!("feline-strap-import-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("Modifications/ClientSettings")).unwrap();
        fs::write(
            dir.join("Settings.json"),
            r#"{"UseDiscordRichPresence": true, "HideRPCButtons": false, "ShowServerDetails": true,
                "ConfirmLaunches": false, "MultiInstanceLaunching": true, "Theme": "Dark",
                "CustomIntegrations": [{"Name": "OBS", "Location": "C:\\OBS\\obs64.exe",
                    "LaunchArgs": "--minimize-to-tray", "AutoClose": false}, {"Location": ""}]}"#,
        )
        .unwrap();
        fs::write(
            dir.join("Modifications/ClientSettings/ClientAppSettings.json"),
            r#"{"FIntDebugForceMSAASamples": "4", "DFIntTaskSchedulerTargetFps": "240"}"#,
        )
        .unwrap();
        let mut s = Settings::default();
        let lines = import_settings(&dir, &mut s).unwrap();
        let _ = fs::remove_dir_all(&dir);
        assert!(s.overrides.discord_presence && s.presence.join_button && s.server_popup && s.multi_roblox);
        assert!(!s.confirm_launch);
        assert_eq!(s.programs.len(), 1);
        assert_eq!((s.programs[0].args.as_str(), s.programs[0].close_after), ("--minimize-to-tray", false));
        assert_eq!(s.fflags.get("FIntDebugForceMSAASamples").map(String::as_str), Some("4"));
        assert!(!s.fflags.contains_key("DFIntTaskSchedulerTargetFps"));
        assert!(lines.iter().any(|l| l.contains("left out 1")), "{lines:?}");
    }
}

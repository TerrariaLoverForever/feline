// SPDX-License-Identifier: GPL-3.0-or-later
//! Optional adapter for Sober, an **external third-party runtime**.
//!
//! Sober (<https://sober.vinegarhq.org>) is a closed-source project by the
//! Vinegar team, distributed only as a Flatpak on Flathub. Feline contains
//! none of its code, never downloads it, and only starts an installed copy the
//! same way its own menu entry does:
//!
//! ```text
//! flatpak run org.vinegarhq.Sober                 start Roblox
//! flatpak run org.vinegarhq.Sober <roblox-link>   join the experience in the link
//! ```
//!
//! Sober has no self-check or diagnostics command, so the trait's defaults
//! ("not offered") apply.
//!
//! Settings: `~/.var/app/org.vinegarhq.Sober/config/sober/config.json`, keys
//! and defaults from Sober's configuration documentation
//! (<https://vinegarhq.org/Sober/Configuration/index.html>). The file starts
//! with a `//` comment header, which Feline keeps.

use std::path::{Path, PathBuf};

use super::config::{Choice, Kind, Link, SettingDef, Val};
use super::{
    DetectContext, InstallKind, Installation, LaunchPlan, LaunchTarget, Origin, RuntimeBackend,
    RuntimeError, flatpak_installs, metainfo_version,
};

const fn toggle(
    key: &'static str,
    label: &'static str,
    help: &'static str,
    default: bool,
) -> SettingDef {
    SettingDef {
        key,
        label,
        help,
        kind: Kind::Toggle,
        default: Val::Bool(default),
        link: None,
    }
}

static SETTINGS: &[SettingDef] = &[
    SettingDef {
        link: Some(Link::Renderer {
            vulkan: Val::Bool(false),
            opengl: Some(Val::Bool(true)),
        }),
        ..toggle(
            "use_opengl",
            "Use OpenGL instead of Vulkan",
            "Can work around \"OutOfMemory\" crashes.",
            false,
        )
    },
    SettingDef {
        key: "graphics_optimization_mode",
        label: "Graphics optimization",
        help: "Performance lowers detail and texture quality for more speed.",
        kind: Kind::Choice(&[
            Choice {
                value: "quality",
                label: "Quality",
            },
            Choice {
                value: "balanced",
                label: "Balanced",
            },
            Choice {
                value: "performance",
                label: "Performance",
            },
        ]),
        default: Val::Str("balanced"),
        link: None,
    },
    SettingDef {
        link: Some(Link::GameMode),
        ..toggle(
            "enable_gamemode",
            "GameMode",
            "Asks Feral GameMode to boost performance while you play.",
            true,
        )
    },
    toggle(
        "enable_hidpi",
        "HiDPI",
        "Scales the window to your screen's pixel density.",
        false,
    ),
    SettingDef {
        link: Some(Link::CloseOnLeave),
        ..toggle("close_on_leave", "Close when leaving a game", "", true)
    },
    toggle(
        "server_location_indicator_enabled",
        "Show server location",
        "Shows where the game server you joined is.",
        false,
    ),
    toggle(
        "allow_gamepad_permission",
        "Allow gamepads",
        "Lets Roblox use controllers.",
        false,
    ),
    SettingDef {
        link: Some(Link::DiscordPresence),
        ..toggle(
            "discord_rpc_enabled",
            "Discord status",
            "Sober's own Discord Rich Presence.",
            false,
        )
    },
    SettingDef {
        link: Some(Link::DiscordPresence),
        ..toggle(
            "discord_rpc_show_join_button",
            "Discord Join button",
            "Lets people join your server from your Discord profile.",
            false,
        )
    },
];

pub const FLATPAK_ID: &str = "org.vinegarhq.Sober";

/// The one instance of the backend (it has no state).
pub static SOBER: Sober = Sober;

pub struct Sober;

impl RuntimeBackend for Sober {
    fn id(&self) -> &'static str {
        "sober"
    }

    fn name(&self) -> &'static str {
        "Sober"
    }

    fn origin(&self) -> Origin {
        Origin::External {
            project_url: "https://sober.vinegarhq.org",
        }
    }

    fn detect(&'static self, ctx: &DetectContext) -> Vec<Installation> {
        flatpak_installs(ctx, FLATPAK_ID)
            .into_iter()
            .map(|(scope, active)| Installation {
                backend: self,
                kind: InstallKind::Flatpak {
                    app_id: FLATPAK_ID,
                    scope,
                },
                known_version: flatpak_version(&active),
            })
            .collect()
    }

    fn settings(&self) -> &'static [SettingDef] {
        SETTINGS
    }

    fn config_path(&self, installation: &Installation, ctx: &DetectContext) -> Option<PathBuf> {
        // Sober only exists as a Flatpak.
        matches!(installation.kind, InstallKind::Flatpak { .. })
            .then(|| ctx.flatpak_config_dir(FLATPAK_ID))
            .flatten()
            .map(|dir| dir.join("sober/config.json"))
    }

    fn process_names(&self) -> &'static [&'static str] {
        &["sober"]
    }

    fn roblox_log_roots(&self, _installation: &Installation, ctx: &DetectContext) -> Vec<PathBuf> {
        // Checked on a real install.
        ctx.flatpak_data_dir(FLATPAK_ID)
            .map(|dir| dir.join("sober/appData/logs"))
            .into_iter()
            .collect()
    }

    fn version(&self, _installation: &Installation) -> Result<String, RuntimeError> {
        // The version comes from the Flatpak's metainfo during detection.
        // Sober has no version option we could ask instead.
        Err(RuntimeError::Unsupported {
            what: "a version query",
        })
    }

    fn plan_launch(
        &self,
        installation: &Installation,
        target: &LaunchTarget,
    ) -> Result<LaunchPlan, RuntimeError> {
        let mut command = installation.command();
        if let LaunchTarget::Game(link) = target {
            // Sober is registered for roblox:// and roblox-player: links.
            command.arg(link.launch_uri());
        }
        Ok(LaunchPlan {
            command,
            note: None,
        })
    }
}

fn flatpak_version(active: &Path) -> Option<String> {
    metainfo_version(
        &active
            .join("files/share/metainfo")
            .join(format!("{FLATPAK_ID}.metainfo.xml")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link::GameLink;
    use crate::runtime::FlatpakScope;
    use std::fs;
    use std::path::PathBuf;

    fn args(plan: &LaunchPlan) -> Vec<String> {
        plan.command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn detects_user_flatpak_and_plans_launches() {
        let root = std::env::temp_dir().join(format!("feline-test-sober-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let home = root.join("home");
        let active = home
            .join(".local/share/flatpak/app")
            .join(FLATPAK_ID)
            .join("current/active/files/share/metainfo");
        fs::create_dir_all(&active).unwrap();
        fs::write(
            active.join(format!("{FLATPAK_ID}.metainfo.xml")),
            r#"<release version="1.8.0" date="2026-10-02"/>"#,
        )
        .unwrap();

        let ctx = DetectContext {
            home: Some(home),
            path: None,
            flatpak_system_dir: PathBuf::from("/nonexistent"),
            config_home: None,
        };
        let found = SOBER.detect(&ctx);
        let _ = fs::remove_dir_all(&root);

        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].kind,
            InstallKind::Flatpak {
                app_id: FLATPAK_ID,
                scope: FlatpakScope::User
            }
        );
        assert_eq!(found[0].known_version.as_deref(), Some("1.8.0"));
        assert!(!found[0].backend.has_doctor());

        let home_plan = found[0].plan_launch(&LaunchTarget::Home).unwrap();
        let game = LaunchTarget::Game(GameLink::Experience { place_id: 42 });
        let game_plan = found[0].plan_launch(&game).unwrap();
        assert_eq!(args(&home_plan), ["run", "--user", FLATPAK_ID]);
        assert_eq!(
            args(&game_plan),
            [
                "run",
                "--user",
                FLATPAK_ID,
                "roblox://experiences/start?placeId=42"
            ]
        );
    }
}

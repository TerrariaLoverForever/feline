// SPDX-License-Identifier: GPL-3.0-or-later
//! Optional adapter for Stud, an **external third-party runtime**.
//!
//! Stud (<https://github.com/CatPieLeaf/Stud>, AGPL-3.0) is a separate project.
//! Feline contains none of its code and only starts the `stud` launcher the
//! way Stud's own menu entry does (`Exec=stud %u`):
//!
//! ```text
//! stud                  start Roblox
//! stud <roblox-link>    join the experience in the link
//! ```
//!
//! It's found as a Flatpak (`io.github.catpieleaf.Stud`), a `stud` program on
//! `PATH`, or an AppImage (`Stud-x86_64.AppImage`). Stud has no self-check or
//! diagnostics command, so the trait's defaults ("not offered") apply.
//!
//! Settings: `config.json` in Stud's config folder. Keys, types and defaults
//! were checked against Stud's settings loader. Stud refuses to start if a key
//! has the wrong JSON type, so every value is written with exactly the type
//! listed here. The upscaler names aren't documented, so that choice is left
//! to Stud's own settings window.

use std::path::{Path, PathBuf};

use super::config::{Choice, Kind, Link, SettingDef, Val};
use super::{
    DetectContext, InstallKind, Installation, LaunchPlan, LaunchTarget, Origin, RuntimeBackend,
    RuntimeError, find_appimages, find_on_path, flatpak_installs, metainfo_version,
};

pub const APP_ID: &str = "io.github.catpieleaf.Stud";

/// The one instance of the backend (it has no state).
pub static STUD: Stud = Stud;

pub struct Stud;

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
        key: "graphicsMode",
        label: "Renderer",
        help: "",
        kind: Kind::Choice(&[
            Choice {
                value: "vulkan",
                label: "Vulkan",
            },
            Choice {
                value: "opengl",
                label: "OpenGL",
            },
        ]),
        default: Val::Str("vulkan"),
        link: Some(Link::Renderer {
            vulkan: Val::Str("vulkan"),
            opengl: Some(Val::Str("opengl")),
        }),
    },
    toggle(
        "upscaling",
        "Upscaling",
        "Renders at a lower resolution and upscales the image.",
        true,
    ),
    toggle(
        "hidpi",
        "HiDPI",
        "Renders at your screen's full pixel density.",
        false,
    ),
    SettingDef {
        key: "backgroundFps",
        label: "Background FPS limit",
        help: "Frame cap while the window is minimised or covered. 0 = no limit.",
        kind: Kind::Number { min: 0, max: 240 },
        default: Val::Int(30),
        link: None,
    },
    SettingDef {
        link: Some(Link::GameMode),
        ..toggle(
            "gamemode",
            "GameMode",
            "Asks Feral GameMode to boost performance while you play.",
            false,
        )
    },
    toggle(
        "mangohud",
        "MangoHud overlay",
        "Shows FPS and frame times over the game.",
        false,
    ),
    SettingDef {
        link: Some(Link::CloseOnLeave),
        ..toggle("closeOnLeave", "Close when leaving a game", "", false)
    },
    toggle(
        "serverRegionNotification",
        "Show server region",
        "Tells you where the game server you joined is.",
        false,
    ),
    toggle("systemTray", "System tray icon", "", true),
    SettingDef {
        link: Some(Link::DiscordPresence),
        ..toggle(
            "discordRichPresence",
            "Discord status",
            "Stud's own Discord Rich Presence.",
            true,
        )
    },
    SettingDef {
        link: Some(Link::DiscordPresence),
        ..toggle(
            "discordJoinButton",
            "Discord Join button",
            "Lets people join your server from your Discord profile.",
            false,
        )
    },
];

/// Grouped as in Stud's settings window.
static TABS: &[super::SettingTab] = &[super::SettingTab {
    name: "Settings",
    sections: &[
        super::SettingSection {
            title: "Graphics",
            keys: &["graphicsMode", "upscaling", "hidpi"],
        },
        super::SettingSection {
            title: "Performance",
            keys: &["backgroundFps", "gamemode", "mangohud"],
        },
        super::SettingSection {
            title: "Game",
            keys: &["closeOnLeave", "serverRegionNotification", "systemTray"],
        },
        super::SettingSection {
            title: "Discord",
            keys: &["discordRichPresence", "discordJoinButton"],
        },
    ],
}];

impl RuntimeBackend for Stud {
    fn id(&self) -> &'static str {
        "stud"
    }

    fn name(&self) -> &'static str {
        "Stud"
    }

    fn origin(&self) -> Origin {
        Origin::External {
            project_url: "https://github.com/CatPieLeaf/Stud",
        }
    }

    fn detect(&'static self, ctx: &DetectContext) -> Vec<Installation> {
        let mut found = Vec::new();

        // 1. Flatpak installs (user and system).
        for (scope, active) in flatpak_installs(ctx, APP_ID) {
            found.push(Installation {
                backend: self,
                kind: InstallKind::Flatpak {
                    app_id: APP_ID,
                    scope,
                },
                known_version: metainfo_version(&metainfo_in(&active.join("files"))),
            });
        }

        // 2. A `stud` program on PATH. That name is also used by an unrelated
        //    old program (a TLS proxy), so it only counts if Stud's metainfo
        //    was installed next to it: <prefix>/bin/stud and
        //    <prefix>/share/metainfo/io.github.catpieleaf.Stud.metainfo.xml.
        if let Some(path) = find_on_path(ctx, "stud") {
            let metainfo = path
                .parent()
                .and_then(Path::parent)
                .map(metainfo_in)
                .filter(|m| m.is_file());
            if let Some(metainfo) = metainfo {
                found.push(Installation {
                    backend: self,
                    kind: InstallKind::Native(path),
                    known_version: metainfo_version(&metainfo),
                });
            }
        }

        // 3. AppImages in the usual folders. Their version can't be read
        //    without starting them, and starting Stud with an option it
        //    doesn't know might open its window, so it stays unknown.
        for path in find_appimages(ctx, "stud") {
            found.push(Installation {
                backend: self,
                kind: InstallKind::AppImage(path),
                known_version: None,
            });
        }
        found
    }

    fn settings(&self) -> &'static [SettingDef] {
        SETTINGS
    }

    fn setting_tabs(&self) -> &'static [super::SettingTab] {
        TABS
    }

    fn config_path(&self, installation: &Installation, ctx: &DetectContext) -> Option<PathBuf> {
        let dir = match &installation.kind {
            InstallKind::Flatpak { app_id, .. } => ctx.flatpak_config_dir(app_id)?.join("stud"),
            InstallKind::AppImage(_) | InstallKind::Native(_) | InstallKind::Official(_) => ctx.config_dir("stud")?,
        };
        Some(dir.join("config.json"))
    }

    fn roblox_log_roots(&self, installation: &Installation, ctx: &DetectContext) -> Vec<PathBuf> {
        // Not verified on a real install yet: Stud keeps "engine data" in its
        // data folder, so the Roblox log is searched for under it.
        match &installation.kind {
            InstallKind::Flatpak { app_id, .. } => {
                ctx.flatpak_data_dir(app_id).map(|d| d.join("stud"))
            }
            InstallKind::AppImage(_) | InstallKind::Native(_) | InstallKind::Official(_) => ctx.data_dir("stud"),
        }
        .into_iter()
        .collect()
    }

    fn process_names(&self) -> &'static [&'static str] {
        // `/proc/<pid>/comm` cuts names to 15 characters.
        &["stud", "stud-runtime-bi", "stud-render-hos"]
    }

    fn version(&self, _installation: &Installation) -> Result<String, RuntimeError> {
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
            // Stud is registered for roblox:// and roblox-player: links.
            command.arg(link.launch_uri());
        }
        Ok(LaunchPlan {
            command,
            note: None,
        })
    }
}

/// Where Stud's metainfo file lives under an install prefix.
fn metainfo_in(prefix: &Path) -> std::path::PathBuf {
    prefix
        .join("share/metainfo")
        .join(format!("{APP_ID}.metainfo.xml"))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    fn write_executable(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "#!/bin/sh\n").unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("feline-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        root
    }

    #[test]
    fn native_stud_needs_its_metainfo() {
        let root = temp_root("stud-native");
        // Real Stud: /usr/bin/stud + /usr/share/metainfo/…
        write_executable(&root.join("usr/bin/stud"));
        let metainfo = metainfo_in(&root.join("usr"));
        fs::create_dir_all(metainfo.parent().unwrap()).unwrap();
        fs::write(&metainfo, r#"<release version="1.2.9"/>"#).unwrap();
        // Unrelated program with the same name, earlier on PATH.
        write_executable(&root.join("other/bin/stud"));

        let ctx = |path: &str| DetectContext {
            home: None,
            path: Some(root.join(path).into_os_string()),
            flatpak_system_dir: PathBuf::from("/nonexistent"),
            config_home: None,
            local_app_data: None,
        };
        let real = STUD.detect(&ctx("usr/bin"));
        let impostor = STUD.detect(&ctx("other/bin"));
        let _ = fs::remove_dir_all(&root);

        assert_eq!(real.len(), 1);
        assert_eq!(real[0].known_version.as_deref(), Some("1.2.9"));
        assert!(impostor.is_empty(), "an unrelated `stud` must be ignored");
    }

    #[test]
    fn finds_appimage_and_plans_a_join() {
        let root = temp_root("stud-appimage");
        let appimage = root.join("home/Applications/Stud-x86_64.AppImage");
        write_executable(&appimage);
        let ctx = DetectContext {
            home: Some(root.join("home")),
            path: None,
            flatpak_system_dir: PathBuf::from("/nonexistent"),
            config_home: None,
            local_app_data: None,
        };
        let found = STUD.detect(&ctx);
        let _ = fs::remove_dir_all(&root);

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, InstallKind::AppImage(appimage.clone()));
        let game = LaunchTarget::Game(crate::link::GameLink::Experience { place_id: 7 });
        let plan = found[0].plan_launch(&game).unwrap();
        assert_eq!(plan.command.get_program(), appimage.as_os_str());
        let args: Vec<_> = plan.command.get_args().collect();
        assert_eq!(args, ["roblox://experiences/start?placeId=7"]);
    }
}

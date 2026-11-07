// SPDX-License-Identifier: GPL-3.0-or-later
//! Optional adapter for Cordial, an **external third-party runtime**.
//!
//! Cordial (<https://github.com/luohoa97/cordial>) is a separate project that
//! users install themselves. Feline contains none of its code, never depends
//! on it and never selects it automatically. This adapter only starts the
//! `cordial` program through its **public command line**:
//!
//! ```text
//! cordial [ROBLOX-LINK]   open Cordial's window, or join the experience in the link
//! cordial --play          (newer versions) start Roblox directly, no Cordial window
//! cordial --place-id N    (newer versions) join experience N directly
//! cordial --doctor        check the machine; exits 1 if something blocks play
//! cordial --diagnostics   versions and install info for bug reports
//! cordial --help          first line is "cordial <version>", then the options
//! ```
//!
//! Which options exist is read from `--help` at launch time, so Feline works
//! with old and new Cordial versions without hard-coding version numbers.
//!
//! We deliberately don't touch `cordial-run`, an internal that can change in
//! any release. Feline does edit a few keys of Cordial's settings file,
//! `shell.json`, whose allowed values were checked against the way Cordial
//! loads that file. That matters: one unknown value makes Cordial ignore the
//! whole file and fall back to defaults. Cordial's renderer key (`graphics`)
//! has no documented value list, so Feline leaves it alone.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::config::{Choice, Kind, Link, SettingDef, Val};
use super::{
    DetectContext, DoctorReport, InstallKind, Installation, LaunchPlan, LaunchTarget, Origin,
    RuntimeBackend, RuntimeError, find_appimages, find_on_path, flatpak_installs, metainfo_version,
    tail_lines,
};
use crate::link::GameLink;
use crate::process::run_captured;

pub const FLATPAK_ID: &str = "io.github.luohoa97.Cordial";

/// The one instance of the backend (it has no state).
pub static CORDIAL: Cordial = Cordial;

// Timeouts. Generous, because a Flatpak's first start can be slow (cold
// caches), but bounded so a stuck runtime never hangs Feline.
const VERSION_TIMEOUT: Duration = Duration::from_secs(15);
const DIAGNOSTICS_TIMEOUT: Duration = Duration::from_secs(30);
const DOCTOR_TIMEOUT: Duration = Duration::from_secs(60);

pub struct Cordial;

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

const fn choice(value: &'static str, label: &'static str) -> Choice {
    Choice { value, label }
}

static SETTINGS: &[SettingDef] = &[
    SettingDef {
        key: "frame_rate_limit",
        label: "FPS limit",
        help: "Cordial's own frame cap. Roblox's in-game Maximum Frame Rate also applies.",
        kind: Kind::Choice(&[
            choice("display", "Match the display"),
            choice("cap90", "90 FPS"),
            choice("cap120", "120 FPS"),
            choice("cap144", "144 FPS"),
            choice("cap165", "165 FPS"),
            choice("cap240", "240 FPS"),
            choice("cap360", "360 FPS"),
            choice("cap480", "480 FPS"),
        ]),
        default: Val::Str("display"),
        link: None,
    },
    SettingDef {
        key: "present_mode",
        label: "Present mode",
        help: "How frames reach the screen. Mailbox is smooth without tearing; Immediate has the lowest latency but can tear.",
        kind: Kind::Choice(&[
            choice("automatic", "Automatic"),
            choice("fifo", "VSync (FIFO)"),
            choice("mailbox", "Mailbox"),
            choice("immediate", "Immediate"),
        ]),
        default: Val::Str("mailbox"),
        link: None,
    },
    SettingDef {
        key: "throttle",
        label: "Lower the frame rate",
        help: "Saves power while you aren't looking at the game.",
        kind: Kind::Choice(&[
            choice("visible", "When the window is hidden"),
            choice("unfocused", "When the window isn't focused"),
            choice("off", "Never"),
        ]),
        default: Val::Str("visible"),
        link: None,
    },
    SettingDef {
        link: Some(Link::GameMode),
        ..toggle(
            "gamemode",
            "GameMode",
            "Asks Feral GameMode to boost performance while you play.",
            true,
        )
    },
    toggle(
        "mangohud",
        "MangoHud overlay",
        "Shows FPS and frame times over the game.",
        false,
    ),
    toggle(
        "vkbasalt",
        "vkBasalt",
        "Post-processing effects (sharpening and more).",
        false,
    ),
    SettingDef {
        link: Some(Link::CloseOnLeave),
        ..toggle("close_on_leave", "Close when leaving a game", "", false)
    },
];

/// Grouped as in Cordial's own settings window (General, Graphics).
static TABS: &[super::SettingTab] = &[
    super::SettingTab {
        name: "General",
        sections: &[
            super::SettingSection {
                title: "Session",
                keys: &["close_on_leave"],
            },
            super::SettingSection {
                title: "Performance",
                keys: &["gamemode", "throttle", "mangohud", "vkbasalt"],
            },
        ],
    },
    super::SettingTab {
        name: "Graphics",
        sections: &[
            super::SettingSection {
                title: "Frame pacing",
                keys: &["present_mode"],
            },
            super::SettingSection {
                title: "Frame rate",
                keys: &["frame_rate_limit"],
            },
        ],
    },
];

impl RuntimeBackend for Cordial {
    fn id(&self) -> &'static str {
        "cordial"
    }

    fn name(&self) -> &'static str {
        "Cordial"
    }

    fn origin(&self) -> Origin {
        Origin::External {
            project_url: "https://github.com/luohoa97/cordial",
        }
    }

    fn detect(&'static self, ctx: &DetectContext) -> Vec<Installation> {
        let mut found = Vec::new();

        // 1. Flatpak installs (user and system).
        for (scope, active) in flatpak_installs(ctx, FLATPAK_ID) {
            found.push(Installation {
                backend: self,
                kind: InstallKind::Flatpak {
                    app_id: FLATPAK_ID,
                    scope,
                },
                known_version: flatpak_version(&active),
            });
        }
        // 2. A `cordial` program on PATH (distro package, AUR, …).
        if let Some(path) = find_on_path(ctx, "cordial") {
            found.push(self.installation(InstallKind::Native(path)));
        }
        // 3. AppImages in the usual folders.
        for path in find_appimages(ctx, "cordial") {
            found.push(self.installation(InstallKind::AppImage(path)));
        }
        found
    }

    fn version(&self, installation: &Installation) -> Result<String, RuntimeError> {
        parse_help_version(&help_text(installation)?).ok_or(RuntimeError::UnexpectedOutput {
            what: "`cordial --help`",
        })
    }

    fn plan_launch(
        &self,
        installation: &Installation,
        target: &LaunchTarget,
    ) -> Result<LaunchPlan, RuntimeError> {
        let help = help_text(installation)?;
        let mut command = installation.command();
        let mut note = None;

        match target {
            LaunchTarget::Home if has_option(&help, "--play") => {
                command.arg("--play");
            }
            LaunchTarget::Home => {
                // Older Cordial: the only way in is its own window.
                note = Some(
                    "This Cordial version can't start Roblox directly, so Cordial's own window \
                     opens instead. Updating Cordial adds direct start."
                        .to_owned(),
                );
            }
            LaunchTarget::Game(GameLink::Experience { place_id })
                if has_option(&help, "--place-id") =>
            {
                command.arg("--place-id").arg(place_id.to_string());
            }
            LaunchTarget::Game(link) => {
                // Documented: a roblox:/roblox-player: link joins that experience.
                command.arg(link.launch_uri());
            }
        }
        Ok(LaunchPlan { command, note })
    }

    fn settings(&self) -> &'static [SettingDef] {
        SETTINGS
    }

    fn setting_tabs(&self) -> &'static [super::SettingTab] {
        TABS
    }

    fn config_path(&self, installation: &Installation, ctx: &DetectContext) -> Option<PathBuf> {
        let dir = match &installation.kind {
            InstallKind::Flatpak { app_id, .. } => ctx.flatpak_config_dir(app_id)?.join("cordial"),
            InstallKind::AppImage(_) | InstallKind::Native(_) => ctx.config_dir("cordial")?,
        };
        Some(dir.join("shell.json"))
    }

    fn process_names(&self) -> &'static [&'static str] {
        &["cordial", "cordial-run"]
    }

    fn roblox_log_roots(&self, installation: &Installation, ctx: &DetectContext) -> Vec<PathBuf> {
        // Checked on real installs: …/cordial/profiles/<profile>/data/files/appData/logs.
        let profiles = match &installation.kind {
            InstallKind::Flatpak { app_id, .. } => ctx
                .flatpak_data_dir(app_id)
                .map(|data| data.join("cordial/profiles")),
            InstallKind::AppImage(_) | InstallKind::Native(_) => {
                ctx.data_dir("cordial").map(|dir| dir.join("profiles"))
            }
        };
        profiles.into_iter().collect()
    }

    fn has_doctor(&self) -> bool {
        true
    }

    fn has_diagnostics(&self) -> bool {
        true
    }

    fn doctor(&self, installation: &Installation) -> Result<DoctorReport, RuntimeError> {
        let mut cmd = installation.command();
        cmd.arg("--doctor");
        let out = run_captured(cmd, DOCTOR_TIMEOUT)?;
        // Documented: exit 0 = fine, exit 1 = something will stop Roblox
        // starting. Anything else means the doctor itself broke.
        let ok = match out.status.code() {
            Some(0) => true,
            Some(1) => false,
            code => {
                return Err(RuntimeError::Failed {
                    what: "Cordial's doctor",
                    code,
                    detail: tail_lines(&out.stderr, 10),
                });
            }
        };
        Ok(DoctorReport {
            ok,
            output: combine(&out.stdout, &out.stderr),
        })
    }

    fn diagnostics(&self, installation: &Installation) -> Result<String, RuntimeError> {
        let mut cmd = installation.command();
        cmd.arg("--diagnostics");
        let out = run_captured(cmd, DIAGNOSTICS_TIMEOUT)?;
        if !out.status.success() {
            return Err(RuntimeError::Failed {
                what: "Cordial's diagnostics",
                code: out.status.code(),
                detail: tail_lines(&out.stderr, 10),
            });
        }
        Ok(out.stdout.trim_end().to_owned())
    }
}

/// Runs `cordial --help` and returns its output.
fn help_text(installation: &Installation) -> Result<String, RuntimeError> {
    let mut cmd = installation.command();
    cmd.arg("--help");
    let out = run_captured(cmd, VERSION_TIMEOUT)?;
    Ok(out.stdout)
}

/// True if the help text lists `option` as an option of its own (a line
/// starting with it), not just mentions it inside another option's text.
pub fn has_option(help: &str, option: &str) -> bool {
    help.lines().any(|line| {
        line.trim_start()
            .strip_prefix(option)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '=', ',']))
    })
}

impl Cordial {
    fn installation(&'static self, kind: InstallKind) -> Installation {
        Installation {
            backend: self,
            kind,
            known_version: None,
        }
    }
}

/// Reads the version from the Flatpak's AppStream metainfo.
fn flatpak_version(active: &Path) -> Option<String> {
    metainfo_version(
        &active
            .join("files/share/metainfo")
            .join(format!("{FLATPAK_ID}.metainfo.xml")),
    )
}

/// `cordial --help` starts with "cordial 0.27.0".
pub fn parse_help_version(help: &str) -> Option<String> {
    let first = help.lines().next()?.trim();
    let version = first.strip_prefix("cordial ")?.trim();
    version
        .starts_with(|c: char| c.is_ascii_digit())
        .then(|| version.to_owned())
}

/// Joins stdout and stderr for display, skipping an empty stderr.
fn combine(stdout: &str, stderr: &str) -> String {
    let (out, err) = (stdout.trim_end(), stderr.trim_end());
    if err.is_empty() {
        out.to_owned()
    } else if out.is_empty() {
        err.to_owned()
    } else {
        format!("{out}\n\n{err}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::FlatpakScope;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("feline-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn write_executable(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn help_version() {
        assert_eq!(
            parse_help_version("cordial 0.27.0\n\nUsage: …").as_deref(),
            Some("0.27.0")
        );
        assert_eq!(parse_help_version("Usage: cordial"), None);
    }

    #[test]
    fn detects_flatpak_path_and_appimages() {
        let root = temp_root("detect");
        let home = root.join("home");
        let system = root.join("var/lib/flatpak");

        let active = system.join("app").join(FLATPAK_ID).join("current/active");
        fs::create_dir_all(active.join("files/share/metainfo")).unwrap();
        fs::write(
            active.join(format!("files/share/metainfo/{FLATPAK_ID}.metainfo.xml")),
            r#"<release version="0.27.0"/>"#,
        )
        .unwrap();

        let bin = root.join("bin");
        write_executable(&bin.join("cordial"), "#!/bin/sh\n");
        write_executable(&home.join(".local/bin/Cordial.AppImage"), "");
        write_executable(&home.join("Applications/cordial-0.20.AppImage"), "");
        // Not executable: must be ignored.
        fs::write(home.join("Applications/cordial-old.AppImage"), "").unwrap();
        // Different app: must be ignored.
        write_executable(&home.join("Applications/Other.AppImage"), "");

        let ctx = DetectContext {
            home: Some(home.clone()),
            path: Some(bin.clone().into_os_string()),
            flatpak_system_dir: system,
            config_home: None,
        };
        let found = CORDIAL.detect(&ctx);
        let _ = fs::remove_dir_all(&root);

        let kinds: Vec<&InstallKind> = found.iter().map(|i| &i.kind).collect();
        assert_eq!(
            kinds,
            vec![
                &InstallKind::Flatpak {
                    app_id: FLATPAK_ID,
                    scope: FlatpakScope::System
                },
                &InstallKind::Native(bin.join("cordial")),
                &InstallKind::AppImage(home.join("Applications/cordial-0.20.AppImage")),
                &InstallKind::AppImage(home.join(".local/bin/Cordial.AppImage")),
            ]
        );
        assert_eq!(found[0].known_version.as_deref(), Some("0.27.0"));
    }

    /// Runs the real command-line handling against a fake `cordial` script.
    #[test]
    fn doctor_diagnostics_and_version_with_fake_cordial() {
        let root = temp_root("fake-cordial");
        let script = root.join("cordial");
        write_executable(
            &script,
            r#"#!/bin/sh
case "$1" in
  --help) echo "cordial 9.9.9"; echo; echo "Usage: cordial" ;;
  --doctor) echo "Display: ok"; echo "GPU: problem" ; exit 1 ;;
  --diagnostics) echo "Cordial 9.9.9 (test)" ;;
  *) exit 7 ;;
esac
"#,
        );
        let inst = CORDIAL.installation(InstallKind::Native(script));

        let version = inst.version();
        let doctor = inst.doctor();
        let diagnostics = inst.diagnostics();
        let _ = fs::remove_dir_all(&root);

        assert_eq!(version.unwrap(), "9.9.9");
        let doctor = doctor.unwrap();
        assert!(!doctor.ok, "exit 1 means a blocking problem");
        assert_eq!(doctor.output, "Display: ok\nGPU: problem");
        assert_eq!(diagnostics.unwrap(), "Cordial 9.9.9 (test)");
    }

    #[test]
    fn option_detection_ignores_mentions_inside_other_options() {
        // Real excerpt from Cordial 0.27.0's --help.
        let old = "cordial 0.27.0\n\nOptions:\n  --doctor       Check this machine\n  \
                   --import-flags FILE|-|--sober\n                 Merge a list into a profile \
                   (--profile NAME, --replace).\n";
        assert!(has_option(old, "--doctor"));
        assert!(!has_option(old, "--profile"));
        assert!(!has_option(old, "--play"));
        let new = "  --play         Start Roblox at once\n  --place-id N   Join N\n";
        assert!(has_option(new, "--play"));
        assert!(has_option(new, "--place-id"));
    }

    fn args(plan: &LaunchPlan) -> Vec<String> {
        plan.command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn launch_plans_follow_supported_options() {
        let root = temp_root("plan");
        let old = root.join("old/cordial");
        let new = root.join("new/cordial");
        write_executable(
            &old,
            "#!/bin/sh\necho 'cordial 0.27.0'\necho '  --doctor  x'\n",
        );
        write_executable(
            &new,
            "#!/bin/sh\necho 'cordial 1.0.0'\necho '  --play  x'\necho '  --place-id N  x'\n",
        );
        let old = CORDIAL.installation(InstallKind::Native(old));
        let new = CORDIAL.installation(InstallKind::Native(new));
        let game = LaunchTarget::Game(GameLink::Experience { place_id: 42 });

        let old_home = old.plan_launch(&LaunchTarget::Home).unwrap();
        let old_game = old.plan_launch(&game).unwrap();
        let new_home = new.plan_launch(&LaunchTarget::Home).unwrap();
        let new_game = new.plan_launch(&game).unwrap();
        let _ = fs::remove_dir_all(&root);

        assert!(args(&old_home).is_empty());
        assert!(old_home.note.is_some(), "the user is told why");
        assert_eq!(args(&old_game), ["roblox://experiences/start?placeId=42"]);
        assert_eq!(args(&new_home), ["--play"]);
        assert!(new_home.note.is_none());
        assert_eq!(args(&new_game), ["--place-id", "42"]);
    }
}

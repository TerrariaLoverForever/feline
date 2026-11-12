// SPDX-License-Identifier: GPL-3.0-or-later
//! Optional adapter for Tipsy, an **external third-party runtime**.
//!
//! Tipsy (<https://github.com/32bitx64bit/Tipsy>, GPL-3.0-or-later) is a
//! separate project. Feline contains none of its code and only starts it the
//! way Tipsy's own "Tipsy - Play" menu entry does
//! (`Exec=tipsy-gui --play %u`):
//!
//! ```text
//! tipsy-gui --play                 start Roblox, skipping Tipsy's menu
//! tipsy-gui --play <roblox-link>   join the experience in the link
//! tipsy version                    print the version
//! ```
//!
//! It's found as a Flatpak (`io.github.tipsy_linux.Tipsy`, whose command is
//! `tipsy-gui`), a `tipsy-gui` program on `PATH`, or an AppImage
//! (`Tipsy-<version>-x86_64.AppImage`, whose AppRun takes the same
//! `--play [link]`). Settings aren't edited yet: Tipsy has its own window.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use super::{
    DetectContext, InstallKind, Installation, LaunchPlan, LaunchTarget, Origin, RuntimeBackend,
    RuntimeError, find_appimages, find_on_path, flatpak_installs, tail_lines,
};
use crate::process::run_captured;

pub const APP_ID: &str = "io.github.tipsy_linux.Tipsy";

const VERSION_TIMEOUT: Duration = Duration::from_secs(5);

/// The one instance of the backend (it has no state).
pub static TIPSY: Tipsy = Tipsy;

pub struct Tipsy;

impl RuntimeBackend for Tipsy {
    fn id(&self) -> &'static str {
        "tipsy"
    }

    fn name(&self) -> &'static str {
        "Tipsy"
    }

    fn origin(&self) -> Origin {
        Origin::External {
            project_url: "https://github.com/32bitx64bit/Tipsy",
        }
    }

    fn detect(&'static self, ctx: &DetectContext) -> Vec<Installation> {
        let mut found = Vec::new();
        // Tipsy's metainfo lists no releases, so versions come from
        // `tipsy version` when asked.
        for (scope, _) in flatpak_installs(ctx, APP_ID) {
            found.push(Installation {
                backend: self,
                kind: InstallKind::Flatpak {
                    app_id: APP_ID,
                    scope,
                },
                known_version: None,
            });
        }
        // `tipsy-gui` is a distinctive name; distro packages install it with
        // `tipsy` next to it.
        if let Some(path) = find_on_path(ctx, "tipsy-gui") {
            found.push(Installation {
                backend: self,
                kind: InstallKind::Native(path),
                known_version: None,
            });
        }
        for path in find_appimages(ctx, "tipsy") {
            found.push(Installation {
                backend: self,
                kind: InstallKind::AppImage(path),
                known_version: None,
            });
        }
        found
    }

    fn roblox_log_roots(&self, installation: &Installation, ctx: &DetectContext) -> Vec<PathBuf> {
        // Not verified on a real install yet: the Roblox client is installed
        // in Tipsy's data folder (`~/.local/share/tipsy`), where it writes
        // its logs.
        match &installation.kind {
            InstallKind::Flatpak { app_id, .. } => {
                ctx.flatpak_data_dir(app_id).map(|d| d.join("tipsy"))
            }
            InstallKind::AppImage(_) | InstallKind::Native(_) | InstallKind::Official(_) => ctx.data_dir("tipsy"),
        }
        .into_iter()
        .collect()
    }

    fn process_names(&self) -> &'static [&'static str] {
        &["tipsy", "tipsy-gui"]
    }

    fn version(&self, installation: &Installation) -> Result<String, RuntimeError> {
        // `tipsy version` prints only the version. The AppImage has no way
        // to reach `tipsy` without possibly starting the game.
        let cmd = match &installation.kind {
            InstallKind::Native(gui) => {
                let cli = gui.with_file_name("tipsy");
                if !super::is_executable(&cli) {
                    return Err(RuntimeError::Unsupported {
                        what: "a version query",
                    });
                }
                let mut cmd = Command::new(cli);
                cmd.arg("version");
                cmd
            }
            InstallKind::Flatpak { app_id, scope } => {
                let mut cmd = Command::new("flatpak");
                cmd.arg("run").arg(match scope {
                    super::FlatpakScope::User => "--user",
                    super::FlatpakScope::System => "--system",
                });
                cmd.arg("--command=tipsy").arg(app_id).arg("version");
                cmd
            }
            InstallKind::AppImage(_) | InstallKind::Official(_) => {
                return Err(RuntimeError::Unsupported {
                    what: "a version query",
                });
            }
        };
        let out = run_captured(cmd, VERSION_TIMEOUT)?;
        if !out.status.success() {
            return Err(RuntimeError::Failed {
                what: "`tipsy version`",
                code: out.status.code(),
                detail: tail_lines(&out.stderr, 5),
            });
        }
        parse_version(&out.stdout).ok_or(RuntimeError::UnexpectedOutput {
            what: "`tipsy version`",
        })
    }

    fn plan_launch(
        &self,
        installation: &Installation,
        target: &LaunchTarget,
    ) -> Result<LaunchPlan, RuntimeError> {
        let mut command = installation.command();
        command.arg("--play");
        if let LaunchTarget::Game(link) = target {
            command.arg(link.launch_uri());
        }
        Ok(LaunchPlan {
            command,
            note: None,
        })
    }
}

/// The version from `tipsy version`'s output: one line, e.g. `1.4.0`.
fn parse_version(stdout: &str) -> Option<String> {
    let line = stdout.lines().map(str::trim).find(|l| !l.is_empty())?;
    (!line.contains(char::is_whitespace)).then(|| line.to_owned())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    fn write_executable(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("feline-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        root
    }

    #[test]
    fn version_output() {
        assert_eq!(parse_version("1.4.0\n").as_deref(), Some("1.4.0"));
        assert_eq!(parse_version("unknown command: version\n"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn native_tipsy_plays_with_the_link_and_reports_its_version() {
        let root = temp_root("tipsy-native");
        let gui = root.join("bin/tipsy-gui");
        write_executable(&gui, "#!/bin/sh\n");
        write_executable(
            &root.join("bin/tipsy"),
            "#!/bin/sh\n[ \"$1\" = version ] && echo 1.4.0\n",
        );
        let ctx = DetectContext {
            home: None,
            path: Some(root.join("bin").into_os_string()),
            flatpak_system_dir: PathBuf::from("/nonexistent"),
            config_home: None,
            local_app_data: None,
        };
        let found = TIPSY.detect(&ctx);
        assert_eq!(found.len(), 1);
        let version = found[0].version();
        let game = LaunchTarget::Game(crate::link::GameLink::Experience { place_id: 7 });
        let plan = found[0].plan_launch(&game).unwrap();
        let home = found[0].plan_launch(&LaunchTarget::Home).unwrap();
        let _ = fs::remove_dir_all(&root);

        assert_eq!(version.unwrap(), "1.4.0");
        assert_eq!(plan.command.get_program(), gui.as_os_str());
        let args: Vec<_> = plan.command.get_args().collect();
        assert_eq!(args, ["--play", "roblox://experiences/start?placeId=7"]);
        let args: Vec<_> = home.command.get_args().collect();
        assert_eq!(args, ["--play"]);
    }
}

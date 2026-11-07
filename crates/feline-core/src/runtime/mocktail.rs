// SPDX-License-Identifier: GPL-3.0-or-later
//! Optional adapters for Mocktail and Nightcap, **external third-party
//! runtimes**.
//!
//! Mocktail (<https://github.com/komaruworld/mocktail>, Apache-2.0) and its
//! fork Nightcap (<https://github.com/CoderDayton/nightcap>, Apache-2.0) are
//! separate projects. Feline contains none of their code and only starts
//! their launcher the way their own menu entries do (`Exec=mocktail %u`):
//!
//! ```text
//! mocktail                 start Roblox
//! mocktail <roblox-link>   join the experience in the link
//! ```
//!
//! Both install a program called `mocktail` (Nightcap keeps the name so
//! upstream fixes apply), so a `mocktail` on `PATH` is told apart by the
//! AppStream metainfo installed next to it. Their Flatpaks have different
//! app IDs and can be installed side by side. AppImages are found by name
//! (`Mocktail-*.AppImage`, `Nightcap-x86_64.AppImage`).
//!
//! Neither has a version query that's safe to run (unknown options start
//! the game), so the version comes from the metainfo when there is one.
//! Settings aren't edited yet: both use their own settings window.

use std::path::{Path, PathBuf};

use super::{
    DetectContext, InstallKind, Installation, LaunchPlan, LaunchTarget, Origin, RuntimeBackend,
    RuntimeError, find_appimages, find_on_path, flatpak_installs, metainfo_version,
};

/// One project of the Mocktail family.
pub struct Mocktail {
    id: &'static str,
    name: &'static str,
    project_url: &'static str,
    /// The Flatpak app ID, also the name of its metainfo file.
    app_id: &'static str,
    /// The start of its AppImage file names (any case).
    appimage_prefix: &'static str,
}

pub static MOCKTAIL: Mocktail = Mocktail {
    id: "mocktail",
    name: "Mocktail",
    project_url: "https://github.com/komaruworld/mocktail",
    app_id: "space.bigrat.mocktail",
    appimage_prefix: "mocktail",
};

pub static NIGHTCAP: Mocktail = Mocktail {
    id: "nightcap",
    name: "Nightcap",
    project_url: "https://github.com/CoderDayton/nightcap",
    app_id: "io.github.CoderDayton.nightcap",
    appimage_prefix: "nightcap",
};

impl Mocktail {
    /// Where this project's metainfo file lives under an install prefix.
    fn metainfo_in(&self, prefix: &Path) -> PathBuf {
        prefix
            .join("share/metainfo")
            .join(format!("{}.metainfo.xml", self.app_id))
    }
}

impl RuntimeBackend for Mocktail {
    fn id(&self) -> &'static str {
        self.id
    }

    fn name(&self) -> &'static str {
        self.name
    }

    fn origin(&self) -> Origin {
        Origin::External {
            project_url: self.project_url,
        }
    }

    fn detect(&'static self, ctx: &DetectContext) -> Vec<Installation> {
        let mut found = Vec::new();

        // 1. Flatpak installs (user and system).
        for (scope, active) in flatpak_installs(ctx, self.app_id) {
            found.push(Installation {
                backend: self,
                kind: InstallKind::Flatpak {
                    app_id: self.app_id,
                    scope,
                },
                known_version: metainfo_version(&self.metainfo_in(&active.join("files"))),
            });
        }

        // 2. A `mocktail` program on PATH, if this project's metainfo was
        //    installed next to it (<prefix>/bin/mocktail and
        //    <prefix>/share/metainfo/<app id>.metainfo.xml).
        if let Some(path) = find_on_path(ctx, "mocktail") {
            let metainfo = path
                .parent()
                .and_then(Path::parent)
                .map(|prefix| self.metainfo_in(prefix))
                .filter(|m| m.is_file());
            if let Some(metainfo) = metainfo {
                found.push(Installation {
                    backend: self,
                    kind: InstallKind::Native(path),
                    known_version: metainfo_version(&metainfo),
                });
            }
        }

        // 3. AppImages in the usual folders.
        for path in find_appimages(ctx, self.appimage_prefix) {
            found.push(Installation {
                backend: self,
                kind: InstallKind::AppImage(path),
                known_version: None,
            });
        }
        found
    }

    fn roblox_log_roots(&self, installation: &Installation, ctx: &DetectContext) -> Vec<PathBuf> {
        // Not verified on a real install yet: both keep the Roblox client's
        // files in their data folder, `mocktail` for either project.
        match &installation.kind {
            InstallKind::Flatpak { app_id, .. } => {
                ctx.flatpak_data_dir(app_id).map(|d| d.join("mocktail"))
            }
            InstallKind::AppImage(_) | InstallKind::Native(_) => ctx.data_dir("mocktail"),
        }
        .into_iter()
        .collect()
    }

    fn process_names(&self) -> &'static [&'static str] {
        // Same program name for both projects.
        &["mocktail"]
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
            // `mocktail [options] [roblox-uri]`, as its menu entry passes it.
            command.arg(link.launch_uri());
        }
        Ok(LaunchPlan {
            command,
            note: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

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
    fn a_native_mocktail_belongs_to_the_project_whose_metainfo_is_installed() {
        let root = temp_root("mocktail-native");
        write_executable(&root.join("usr/bin/mocktail"));
        let metainfo = NIGHTCAP.metainfo_in(&root.join("usr"));
        fs::create_dir_all(metainfo.parent().unwrap()).unwrap();
        fs::write(&metainfo, r#"<release version="0.5.1"/>"#).unwrap();
        let ctx = DetectContext {
            home: None,
            path: Some(root.join("usr/bin").into_os_string()),
            flatpak_system_dir: PathBuf::from("/nonexistent"),
            config_home: None,
        };
        let nightcap = NIGHTCAP.detect(&ctx);
        let mocktail = MOCKTAIL.detect(&ctx);
        let _ = fs::remove_dir_all(&root);

        assert_eq!(nightcap.len(), 1);
        assert_eq!(nightcap[0].known_version.as_deref(), Some("0.5.1"));
        assert!(mocktail.is_empty(), "Nightcap's `mocktail` isn't Mocktail");
    }

    #[test]
    fn appimages_are_told_apart_by_name_and_get_the_link() {
        let root = temp_root("mocktail-appimage");
        let mocktail = root.join("home/Applications/Mocktail-1.0.5-x86_64.AppImage");
        let nightcap = root.join("home/Applications/Nightcap-x86_64.AppImage");
        write_executable(&mocktail);
        write_executable(&nightcap);
        let ctx = DetectContext {
            home: Some(root.join("home")),
            path: None,
            flatpak_system_dir: PathBuf::from("/nonexistent"),
            config_home: None,
        };
        let found_mocktail = MOCKTAIL.detect(&ctx);
        let found_nightcap = NIGHTCAP.detect(&ctx);
        let _ = fs::remove_dir_all(&root);

        assert_eq!(found_mocktail.len(), 1);
        assert_eq!(found_mocktail[0].kind, InstallKind::AppImage(mocktail));
        assert_eq!(found_nightcap.len(), 1);
        assert_eq!(found_nightcap[0].kind, InstallKind::AppImage(nightcap.clone()));
        let game = LaunchTarget::Game(crate::link::GameLink::Experience { place_id: 7 });
        let plan = found_nightcap[0].plan_launch(&game).unwrap();
        assert_eq!(plan.command.get_program(), nightcap.as_os_str());
        let args: Vec<_> = plan.command.get_args().collect();
        assert_eq!(args, ["roblox://experiences/start?placeId=7"]);
    }
}

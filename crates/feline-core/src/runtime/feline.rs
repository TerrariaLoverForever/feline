// SPDX-License-Identifier: GPL-3.0-or-later
//! Feline's own runtime, `feline-runtime` (in development).
//!
//! It runs Roblox's Android engine (`libroblox.so`, with the app's
//! `base.apk` next to it), which the user copies from their own install;
//! Feline never ships or downloads it. It counts as installed when both are
//! there:
//!
//! - the program: `feline-runtime` next to the launcher, or on `PATH`;
//! - the engine: `~/.local/share/feline/engine/` or `~/feline-engine/`.
//!
//! ```text
//! feline-runtime --play <libroblox.so>    Roblox's app, until the window closes
//! feline-runtime --check <libroblox.so>   self-check: can the engine be loaded?
//! feline-runtime --version
//! ```
//!
//! It can't join an experience from a link yet: it opens the app, where you
//! pick the game.

use std::path::PathBuf;
use std::time::Duration;

use super::config::{Choice, Kind, SettingDef, Val};
use super::{
    DetectContext, DoctorReport, InstallKind, Installation, LaunchPlan, LaunchTarget, Origin,
    RuntimeBackend, RuntimeError, find_on_path, tail_lines,
};
use crate::process::run_captured;

/// The one instance of the backend (it has no state).
pub static FELINE: FelineRuntime = FelineRuntime;

pub struct FelineRuntime;

const PROGRAM: &str = "feline-runtime";
const CHECK_TIMEOUT: Duration = Duration::from_secs(60);
const VERSION_TIMEOUT: Duration = Duration::from_secs(5);

/// The engine library, if it's in one of the folders Feline looks in, with
/// the app's `base.apk` next to it.
pub fn engine(ctx: &DetectContext) -> Option<PathBuf> {
    engine_in([
        ctx.data_dir("feline").map(|d| d.join("engine")),
        ctx.home.as_ref().map(|h| h.join("feline-engine")),
    ])
}

/// The first of `folders` with both `libroblox.so` and `base.apk`.
fn engine_in<const N: usize>(folders: [Option<PathBuf>; N]) -> Option<PathBuf> {
    folders
        .into_iter()
        .flatten()
        .map(|d| d.join("libroblox.so"))
        .find(|lib| lib.is_file() && lib.with_file_name("base.apk").is_file())
}

/// The runtime program: next to the running launcher first (they're
/// installed together), then on `PATH`.
fn program(ctx: &DetectContext) -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .map(|exe| exe.with_file_name(PROGRAM))
        .filter(|p| p.is_file())
        .or_else(|| find_on_path(ctx, PROGRAM))
}

/// The runtime's settings (`~/.config/feline/runtime.json`, created by the
/// runtime on its first start; see feline-runtime's prefs.rs).
static SETTINGS: &[SettingDef] = &[
    SettingDef {
        key: "theme",
        label: "App theme",
        help: "",
        kind: Kind::Choice(&[
            Choice {
                value: "dark",
                label: "Dark",
            },
            Choice {
                value: "light",
                label: "Light",
            },
        ]),
        default: Val::Str("dark"),
        link: None,
    },
    SettingDef {
        key: "fpsLimit",
        label: "FPS limit",
        help: "The highest frame rate, e.g. 600 or 999 (9999 = no limit); it can go past Roblox's usual 240. Set each time the game starts. 0 = use Roblox's in-game Maximum Frame Rate.",
        kind: Kind::Number { min: 0, max: 9999 },
        default: Val::Int(0),
        link: None,
    },
    SettingDef {
        key: "startFullscreen",
        label: "Start in fullscreen",
        help: "F11 switches fullscreen on and off while playing.",
        kind: Kind::Toggle,
        default: Val::Bool(false),
        link: None,
    },
];

fn no_engine() -> RuntimeError {
    RuntimeError::Failed {
        what: "Finding Roblox's engine",
        code: None,
        detail: "copy libroblox.so and base.apk into ~/.local/share/feline/engine/".into(),
    }
}

impl RuntimeBackend for FelineRuntime {
    fn id(&self) -> &'static str {
        "feline"
    }

    fn name(&self) -> &'static str {
        "Feline Runtime"
    }

    fn origin(&self) -> Origin {
        Origin::Feline
    }

    fn detect(&'static self, ctx: &DetectContext) -> Vec<Installation> {
        match (program(ctx), engine(ctx)) {
            (Some(path), Some(_)) => vec![Installation {
                backend: self,
                kind: InstallKind::Native(path),
                known_version: None,
            }],
            _ => Vec::new(),
        }
    }

    fn version(&self, installation: &Installation) -> Result<String, RuntimeError> {
        let mut cmd = installation.command();
        cmd.arg("--version");
        let out = run_captured(cmd, VERSION_TIMEOUT)?;
        // "feline-runtime 0.1.0"
        out.stdout
            .split_whitespace()
            .nth(1)
            .filter(|_| out.status.success())
            .map(str::to_owned)
            .ok_or(RuntimeError::UnexpectedOutput {
                what: "feline-runtime --version",
            })
    }

    fn has_doctor(&self) -> bool {
        true
    }

    /// `--check`: reads the engine and checks everything loading it needs.
    fn doctor(&self, installation: &Installation) -> Result<DoctorReport, RuntimeError> {
        let engine = engine(&DetectContext::host()).ok_or_else(no_engine)?;
        let mut cmd = installation.command();
        cmd.arg("--check").arg(&engine);
        let out = run_captured(cmd, CHECK_TIMEOUT)?;
        let mut output = out.stdout;
        if !out.stderr.trim().is_empty() {
            output.push('\n');
            output.push_str(&tail_lines(&out.stderr, 20));
        }
        Ok(DoctorReport {
            ok: out.status.success(),
            output,
        })
    }

    fn settings(&self) -> &'static [SettingDef] {
        SETTINGS
    }

    fn config_path(&self, _installation: &Installation, ctx: &DetectContext) -> Option<PathBuf> {
        Some(ctx.config_dir("feline")?.join("runtime.json"))
    }

    fn roblox_log_roots(&self, _installation: &Installation, ctx: &DetectContext) -> Vec<PathBuf> {
        // The engine writes to <runtime data>/files/appData/logs.
        ctx.data_dir("feline")
            .map(|d| d.join("runtime/files/appData/logs"))
            .into_iter()
            .collect()
    }

    fn process_names(&self) -> &'static [&'static str] {
        &[PROGRAM]
    }

    fn plan_launch(
        &self,
        installation: &Installation,
        target: &LaunchTarget,
    ) -> Result<LaunchPlan, RuntimeError> {
        // A Roblox update that never reached the app twice: back to the
        // last version that worked (see `crate::engine`).
        let rolled_back = crate::engine::roll_back_if_broken();
        let engine = engine(&DetectContext::host()).ok_or_else(no_engine)?;
        let mut command = installation.command();
        command.arg("--play").arg(engine);
        let link_note = matches!(target, LaunchTarget::Game(_)).then(|| {
            "Feline's runtime can't join from a link yet, so it opens the Roblox app: \
             find the experience there."
                .to_owned()
        });
        let note = match (rolled_back, link_note) {
            (Some(a), Some(b)) => Some(format!("{a} {b}")),
            (a, b) => a.or(b),
        };
        Ok(LaunchPlan { command, note })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn the_runtimes_settings_file_is_readable() {
        // As feline-runtime writes it (prefs.rs), with non-default values.
        let text = "// Feline runtime settings. Feline's launcher edits this file.\n{\n  \"theme\": \"light\",\n  \"fpsLimit\": 600,\n  \"startFullscreen\": true\n}\n";
        let file = super::super::config::ConfigFile::parse(std::path::Path::new("/x"), text)
            .expect("parses");
        let expected = [
            serde_json::json!("light"),
            serde_json::json!(600),
            serde_json::json!(true),
        ];
        for (def, value) in SETTINGS.iter().zip(expected) {
            assert!(def.accepts(&def.default.to_json()), "{}", def.key);
            assert_eq!(file.get(def), value, "{}", def.key);
        }
    }

    #[test]
    fn engine_needs_the_apk_next_to_it() {
        let root =
            std::env::temp_dir().join(format!("feline-test-engine-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let first = root.join("engine");
        let second = root.join("feline-engine");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        let folders = || [Some(first.clone()), Some(second.clone())];
        fs::write(first.join("libroblox.so"), b"").unwrap();
        assert_eq!(engine_in(folders()), None);
        fs::write(second.join("libroblox.so"), b"").unwrap();
        fs::write(second.join("base.apk"), b"").unwrap();
        assert_eq!(engine_in(folders()), Some(second.join("libroblox.so")));
        fs::write(first.join("base.apk"), b"").unwrap();
        assert_eq!(engine_in(folders()), Some(first.join("libroblox.so")));
        let _ = fs::remove_dir_all(&root);
    }
}

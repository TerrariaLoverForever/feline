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

use super::config::{Choice, Kind, SettingDef, TextCheck, Val};
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
        key: "presentMode",
        label: "Present mode",
        help: "How frames reach the screen. Mailbox is smooth without tearing; Immediate has the lowest latency but can tear; VSync waits for your monitor (no tearing, frame rate stops at its refresh rate, input lags a little).",
        kind: Kind::Choice(&[
            Choice {
                value: "mailbox",
                label: "Mailbox",
            },
            Choice {
                value: "immediate",
                label: "Immediate",
            },
            Choice {
                value: "fifo",
                label: "VSync (FIFO)",
            },
        ]),
        default: Val::Str("mailbox"),
        link: None,
    },
    SettingDef {
        key: "framePacing",
        label: "Frame pacing",
        help: "With an FPS limit, Feline holds each frame until its turn just before it's shown, so frames come at an even rhythm (smoother than Roblox's own limiter at the same frame rate).",
        kind: Kind::Toggle,
        default: Val::Bool(true),
        link: None,
    },
    SettingDef {
        key: "throttleUnfocused",
        label: "Lower the frame rate in the background",
        help: "30 FPS while the game window isn't focused, to save power.",
        kind: Kind::Toggle,
        default: Val::Bool(false),
        link: None,
    },
    SettingDef {
        key: "gamemode",
        label: "GameMode",
        help: "Asks Feral GameMode to boost performance while you play (needs gamemode installed).",
        kind: Kind::Toggle,
        default: Val::Bool(false),
        link: Some(super::config::Link::GameMode),
    },
    SettingDef {
        key: "mangohud",
        label: "MangoHud overlay",
        help: "Shows FPS and frame times over the game (needs MangoHud installed).",
        kind: Kind::Toggle,
        default: Val::Bool(false),
        link: None,
    },
    SettingDef {
        key: "vkbasalt",
        label: "vkBasalt",
        help: "Post-processing effects such as sharpening, set up in vkBasalt's own config (needs vkBasalt installed; Vulkan only).",
        kind: Kind::Toggle,
        default: Val::Bool(false),
        link: None,
    },
    SettingDef {
        key: "renderer",
        label: "Graphics API",
        help: "How Roblox draws. Vulkan is usually faster; OpenGL (OpenGL ES) is the older way, worth trying if the picture looks glitched. Used from the next start.",
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
        // Feline's own Graphics setting (Settings page) decides when set.
        link: Some(super::config::Link::Renderer {
            vulkan: Val::Str("vulkan"),
            opengl: Some(Val::Str("opengl")),
        }),
    },
    SettingDef {
        key: "graphicsOptimization",
        label: "Graphics optimisation",
        help: "What Feline tells Roblox this is, and how many CPU cores the engine may use. Windows PC: games see a Windows player (PC layouts and hints instead of mobile ones). More cores: lets the engine spread its work over your CPU's cores (Roblox's Android settings are sized for phones). Fewer cores: only small scheduler tweaks. Measured on an 8-core Ryzen 7 5700X with no FPS limit: about 160–185 FPS as Roblox app, 220–240 with more cores; other PCs may differ.",
        kind: Kind::Choice(&[
            Choice {
                value: "roblox-app",
                label: "Roblox app",
            },
            Choice {
                value: "windows-pc",
                label: "Windows PC",
            },
            Choice {
                value: "windows-pc-more-cores",
                label: "Windows PC - more cores",
            },
            Choice {
                value: "windows-pc-fewer-cores",
                label: "Windows PC - fewer cores",
            },
        ]),
        default: Val::Str("roblox-app"),
        link: None,
    },
    SettingDef {
        key: "graphicsQuality",
        label: "Graphics quality",
        help: "Roblox's own graphics quality, 1 (fastest) to 10 (best looking), set each time the game starts. 0 = Roblox decides (automatic, or what you set in the game).",
        kind: Kind::Number { min: 0, max: 10 },
        default: Val::Int(0),
        link: None,
    },
    SettingDef {
        key: "microphone",
        label: "Microphone for voice chat",
        help: "Off: voice chat still works for listening, but your microphone is never opened and others hear silence.",
        kind: Kind::Toggle,
        default: Val::Bool(true),
        link: None,
    },
    SettingDef {
        key: "controllers",
        label: "Controllers",
        help: "Passes gamepads (Xbox, PlayStation and others) to Roblox.",
        kind: Kind::Toggle,
        default: Val::Bool(true),
        link: None,
    },
    SettingDef {
        key: "blockTracking",
        label: "Block Roblox tracking",
        help: "Roblox's analytics and telemetry servers (usage events, performance metrics, crash and log reports) look unreachable to the app. Games, login, chat and voice don't use them.",
        kind: Kind::Toggle,
        default: Val::Bool(true),
        link: None,
    },
    SettingDef {
        key: "proxy",
        label: "Proxy",
        help: "Off when empty. For Roblox's web traffic (login, website and API requests), e.g. socks5://127.0.0.1:9050 or http://proxy:8080. The game connection itself doesn't go through a proxy.",
        kind: Kind::Text {
            placeholder: "socks5://host:port",
            check: TextCheck::Proxy,
        },
        default: Val::Str(""),
        link: None,
    },
    SettingDef {
        key: "cpuThreads",
        label: "CPU threads",
        help: "How many of your CPU's threads Roblox may run on. 0 = all of them. Fewer leaves room for a recording, a stream or another game, at the cost of frame rate.",
        kind: Kind::Number { min: 0, max: 256 },
        default: Val::Int(0),
        link: None,
    },
    SettingDef {
        key: "priority",
        label: "Game priority",
        help: "Low lets everything else go first (handy next to a stream or a second game). High asks the system to put Roblox first: most Linux systems only allow that with a raised nice limit or GameMode, and Feline's log says when it was refused.",
        kind: Kind::Choice(&[
            Choice {
                value: "low",
                label: "Low",
            },
            Choice {
                value: "normal",
                label: "Normal",
            },
            Choice {
                value: "high",
                label: "High",
            },
        ]),
        default: Val::Str("normal"),
        link: None,
    },
    SettingDef {
        key: "trimMemory",
        label: "Trim memory in the background",
        help: "While the game window isn't focused, memory Roblox isn't touching is handed back to the system (to swap, or dropped if it's an unchanged file), every two minutes. It comes back by itself when Roblox needs it, so switching back can hitch for a moment.",
        kind: Kind::Toggle,
        default: Val::Bool(false),
        link: None,
    },
    SettingDef {
        key: "blockCaptures",
        label: "Turn off Roblox's screenshots and recordings",
        help: "Roblox's own capture feature (the Captures tab and its screenshot key) can't save anything. Your desktop's screenshot and recording tools keep working. Used from the next start.",
        kind: Kind::Toggle,
        default: Val::Bool(false),
        link: None,
    },
    SettingDef {
        key: "detailedLogs",
        label: "Detailed logs",
        help: "Logs every call between Roblox and Feline's runtime. Only turn on when a developer asks for a log: the logs get much bigger.",
        kind: Kind::Toggle,
        default: Val::Bool(false),
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

/// GameMode, MangoHud and vkBasalt, as the runtime's settings ask.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Tools {
    gamemode: bool,
    mangohud: bool,
    vkbasalt: bool,
}

impl FelineRuntime {
    fn tool_settings(&self, installation: &Installation) -> Tools {
        let ctx = DetectContext::host();
        let file = self
            .config_path(installation, &ctx)
            .and_then(|p| super::config::ConfigFile::load(&p).ok());
        let on = |key: &str| {
            SETTINGS.iter().find(|d| d.key == key).is_some_and(|def| match &file {
                Some(f) => f.get(def) == serde_json::Value::Bool(true),
                None => def.default.to_json() == serde_json::Value::Bool(true),
            })
        };
        Tools {
            gamemode: on("gamemode"),
            mangohud: on("mangohud"),
            vkbasalt: on("vkbasalt"),
        }
    }
}

/// `command` with the tools: MangoHud and vkBasalt are Vulkan layers
/// switched on by environment variables (MangoHud's `mangohud` wrapper
/// also covers OpenGL); GameMode wraps the command in `gamemoderun`.
/// A tool that isn't installed is skipped.
fn with_tools(command: std::process::Command, tools: &Tools) -> std::process::Command {
    let ctx = DetectContext::host();
    let mut wrappers = Vec::new();
    if tools.gamemode
        && let Some(gm) = find_on_path(&ctx, "gamemoderun")
    {
        wrappers.push(gm);
    }
    if tools.mangohud
        && let Some(mh) = find_on_path(&ctx, "mangohud")
    {
        wrappers.push(mh);
    }
    let mut out = match wrappers.first() {
        None => command,
        Some(first) => {
            let mut wrapped = std::process::Command::new(first);
            wrapped.args(wrappers.iter().skip(1));
            wrapped.arg(command.get_program()).args(command.get_args());
            for (key, value) in command.get_envs() {
                match value {
                    Some(v) => wrapped.env(key, v),
                    None => wrapped.env_remove(key),
                };
            }
            if let Some(dir) = command.get_current_dir() {
                wrapped.current_dir(dir);
            }
            wrapped
        }
    };
    if tools.mangohud {
        out.env("MANGOHUD", "1");
    }
    if tools.vkbasalt {
        out.env("ENABLE_VKBASALT", "1");
    }
    out
}

fn no_engine() -> RuntimeError {
    RuntimeError::Failed {
        what: "Finding Roblox's engine",
        code: None,
        detail: "copy libroblox.so and base.apk into ~/.local/share/feline/engine/".into(),
    }
}

/// Grouped as in sections of one page.
static TABS: &[super::SettingTab] = &[
    super::SettingTab {
        name: "Graphics",
        sections: &[
            super::SettingSection {
                title: "Frame pacing",
                keys: &["presentMode", "framePacing"],
            },
            super::SettingSection {
                title: "Frame rate",
                keys: &["fpsLimit", "throttleUnfocused"],
            },
            super::SettingSection {
                title: "Picture",
                keys: &["renderer", "graphicsOptimization", "graphicsQuality", "startFullscreen"],
            },
        ],
    },
    super::SettingTab {
        name: "Performance",
        sections: &[
            super::SettingSection {
                title: "Processor and memory",
                keys: &["cpuThreads", "priority", "trimMemory"],
            },
            super::SettingSection {
                title: "Tools",
                keys: &["gamemode", "mangohud", "vkbasalt"],
            },
        ],
    },
    super::SettingTab {
        name: "Audio & controls",
        sections: &[
            super::SettingSection {
                title: "Voice chat",
                keys: &["microphone"],
            },
            super::SettingSection {
                title: "Controls",
                keys: &["controllers"],
            },
        ],
    },
    super::SettingTab {
        name: "Privacy & network",
        sections: &[
            super::SettingSection {
                title: "Privacy",
                keys: &["blockTracking", "blockCaptures"],
            },
            super::SettingSection {
                title: "Network",
                keys: &["proxy"],
            },
        ],
    },
    super::SettingTab {
        name: "Advanced",
        sections: &[
            super::SettingSection {
                title: "Appearance",
                keys: &["theme"],
            },
            super::SettingSection {
                title: "Troubleshooting",
                keys: &["detailedLogs"],
            },
        ],
    },
];

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

    fn setting_tabs(&self) -> &'static [super::SettingTab] {
        TABS
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

    fn mod_target(
        &self,
        _installation: &Installation,
        ctx: &DetectContext,
    ) -> Option<crate::mods::ModTarget> {
        // The engine reads Roblox's assets as plain files from the folder
        // the runtime unpacks them to (`PlatformParams.assetFolderPath`).
        Some(crate::mods::ModTarget {
            dir: ctx.data_dir("feline")?.join("runtime/assets"),
            overlay: false,
        })
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
        let command = with_tools(command, &self.tool_settings(installation));
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
        let text = "// Feline runtime settings. Feline's launcher edits this file.\n{\n  \"theme\": \"light\",\n  \"fpsLimit\": 600,\n  \"startFullscreen\": true,\n  \"proxy\": \"socks5://127.0.0.1:9050\",\n  \"blockTracking\": false\n}\n";
        let file = super::super::config::ConfigFile::parse(std::path::Path::new("/x"), text)
            .expect("parses");
        let expected = [
            ("theme", serde_json::json!("light")),
            ("fpsLimit", serde_json::json!(600)),
            ("startFullscreen", serde_json::json!(true)),
            ("proxy", serde_json::json!("socks5://127.0.0.1:9050")),
            ("blockTracking", serde_json::json!(false)),
            // Not in the file: the default.
            ("microphone", serde_json::json!(true)),
        ];
        for def in SETTINGS {
            assert!(def.accepts(&def.default.to_json()), "{}", def.key);
        }
        for (key, value) in expected {
            let def = SETTINGS.iter().find(|d| d.key == key).expect(key);
            assert_eq!(file.get(def), value, "{key}");
        }
        let proxy = SETTINGS.iter().find(|d| d.key == "proxy").unwrap();
        assert!(!proxy.accepts(&serde_json::json!("not a proxy")));
    }

    #[test]
    fn engine_needs_the_apk_next_to_it() {
        let root = std::env::temp_dir().join(format!("feline-test-engine-{}", std::process::id()));
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

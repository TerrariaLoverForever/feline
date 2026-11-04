// SPDX-License-Identifier: GPL-3.0-or-later
//! Reading and changing an external runtime's **own** settings file.
//!
//! Each backend lists the settings Feline may show and change as
//! [`SettingDef`]s. Only settings whose keys, types and allowed values were
//! checked against the runtime's documentation or its file loader are listed:
//! one wrong value can make a runtime ignore its whole file (Cordial) or refuse
//! to start (Stud).
//!
//! Safety rules for editing another program's file ([`ConfigFile`]):
//! - Never create the file. A runtime writes a complete file on its first
//!   start; a partial one written by Feline could confuse it.
//! - Never write a file that couldn't be parsed. Show the error instead.
//! - Keep everything Feline doesn't touch: unknown keys, key order, the file's
//!   indentation, and a leading `//` comment header (Sober has one).
//! - Make a one-time backup (`<file>.feline-backup`) before the first change.
//! - Write atomically (temporary file + rename).
//! - Never write while the runtime is running: it may save its own copy on
//!   exit and undo the change, or read a half-applied state.

use std::fmt;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::settings::{Overrides, Renderer, Toggle};

// ---------------------------------------------------------------------------
// Describing settings
// ---------------------------------------------------------------------------

/// A value as it's stored in the file. `&'static str` so setting lists can be
/// plain `static` tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Val {
    Bool(bool),
    Str(&'static str),
    Int(i64),
}

impl Val {
    pub fn to_json(self) -> Value {
        match self {
            Self::Bool(b) => Value::Bool(b),
            Self::Str(s) => Value::String(s.to_owned()),
            Self::Int(i) => Value::from(i),
        }
    }
}

/// One entry of a dropdown: the value written to the file, and what the user sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Choice {
    pub value: &'static str,
    pub label: &'static str,
}

/// What kind of control a setting needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A JSON `true`/`false`.
    Toggle,
    /// A JSON string, one of these values.
    Choice(&'static [Choice]),
    /// A JSON integer in `min..=max`.
    Number { min: i64, max: i64 },
}

/// Which Feline-wide setting, if any, overrides this runtime setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    /// Feline's renderer choice. `opengl` is `None` if this runtime's
    /// value for OpenGL isn't known; Feline then leaves it alone.
    Renderer { vulkan: Val, opengl: Option<Val> },
    /// Feline's GameMode switch (the setting must be a `Toggle`).
    GameMode,
    /// Feline's "close the runtime when leaving a game" switch (a `Toggle`).
    CloseOnLeave,
    /// The runtime's own Discord status (a `Toggle`), turned off while
    /// Feline shows the status itself.
    DiscordPresence,
}

/// One setting of a runtime that Feline can show and change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingDef {
    /// The key in the runtime's JSON file.
    pub key: &'static str,
    pub label: &'static str,
    /// One sentence for the user. Empty if the label says it all.
    pub help: &'static str,
    pub kind: Kind,
    /// The runtime's own default, used when the key isn't in the file.
    pub default: Val,
    pub link: Option<Link>,
}

impl SettingDef {
    /// True if `value` is something the runtime accepts for this setting.
    pub fn accepts(&self, value: &Value) -> bool {
        match (self.kind, value) {
            (Kind::Toggle, Value::Bool(_)) => true,
            (Kind::Choice(choices), Value::String(s)) => choices.iter().any(|c| c.value == s),
            (Kind::Number { min, max }, Value::Number(n)) => {
                n.as_i64().is_some_and(|n| (min..=max).contains(&n))
            }
            _ => false,
        }
    }
}

/// What a Feline-wide setting forces a runtime setting to, with the reason
/// shown next to the locked control.
pub fn managed_value(link: Link, overrides: &Overrides) -> Option<(Value, &'static str)> {
    let toggle = |t: Toggle, on: &'static str, off: &'static str| match t {
        Toggle::Runtime => None,
        Toggle::On => Some((Value::Bool(true), on)),
        Toggle::Off => Some((Value::Bool(false), off)),
    };
    match link {
        Link::Renderer { vulkan, opengl } => match overrides.renderer {
            Renderer::Runtime => None,
            Renderer::Vulkan => Some((vulkan.to_json(), "Feline's renderer is set to Vulkan")),
            Renderer::OpenGl => opengl.map(|v| (v.to_json(), "Feline's renderer is set to OpenGL")),
        },
        Link::GameMode => toggle(
            overrides.gamemode,
            "Feline turns GameMode on",
            "Feline turns GameMode off",
        ),
        Link::CloseOnLeave => toggle(
            overrides.close_on_leave,
            "Feline closes the runtime when you leave a game",
            "Feline keeps the runtime open when you leave a game",
        ),
        Link::DiscordPresence => overrides.discord_presence.then(|| {
            (
                Value::Bool(false),
                "Feline shows your Discord status instead",
            )
        }),
    }
}

// ---------------------------------------------------------------------------
// The file
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum ConfigError {
    /// The runtime hasn't created its settings file yet.
    Missing(PathBuf),
    Read(PathBuf, io::Error),
    /// The file isn't valid JSON (or not a JSON object). Feline won't touch it.
    Invalid(PathBuf, String),
    /// The runtime is running, so its file must not be changed now.
    Running(&'static str),
    /// A value the runtime doesn't accept for this setting.
    BadValue {
        key: &'static str,
    },
    Write(PathBuf, io::Error),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(path) => write!(
                f,
                "{} doesn't exist yet. Start the runtime once so it creates its settings.",
                path.display()
            ),
            Self::Read(path, e) => write!(f, "couldn't read {}: {e}", path.display()),
            Self::Invalid(path, e) => write!(
                f,
                "{} isn't valid JSON ({e}). Feline won't change it; fix or delete it first.",
                path.display()
            ),
            Self::Running(name) => write!(
                f,
                "{name} is running. Close it first, so it doesn't overwrite the change."
            ),
            Self::BadValue { key } => write!(f, "that value isn't allowed for `{key}`"),
            Self::Write(path, e) => write!(f, "couldn't save {}: {e}", path.display()),
        }
    }
}

impl std::error::Error for ConfigError {}

/// A runtime's settings file, loaded for editing.
#[derive(Debug, Clone)]
pub struct ConfigFile {
    path: PathBuf,
    /// Leading `//` comment lines, kept verbatim.
    header: String,
    map: Map<String, Value>,
    /// The file's indentation unit, e.g. four spaces.
    indent: String,
    trailing_newline: bool,
}

impl ConfigFile {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        match fs::read_to_string(path) {
            Ok(text) => Self::parse(path, &text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                Err(ConfigError::Missing(path.to_owned()))
            }
            Err(e) => Err(ConfigError::Read(path.to_owned(), e)),
        }
    }

    /// Parses file contents. Separate from `load` so tests can use strings.
    pub fn parse(path: &Path, text: &str) -> Result<Self, ConfigError> {
        // Split off the `//` header: every line before the JSON starts.
        let mut header_len = 0;
        for line in text.split_inclusive('\n') {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") || trimmed.trim().is_empty() {
                header_len += line.len();
            } else {
                break;
            }
        }
        let (header, json) = text.split_at(header_len);

        let map = match serde_json::from_str::<Value>(json) {
            Ok(Value::Object(map)) => map,
            Ok(_) => {
                return Err(ConfigError::Invalid(
                    path.to_owned(),
                    "the top level isn't an object".to_owned(),
                ));
            }
            Err(e) => return Err(ConfigError::Invalid(path.to_owned(), e.to_string())),
        };

        // The indentation of the first indented line, or four spaces.
        let indent = json
            .lines()
            .map(|l| &l[..l.len() - l.trim_start().len()])
            .find(|ws| !ws.is_empty())
            .unwrap_or("    ")
            .to_owned();

        Ok(Self {
            path: path.to_owned(),
            header: header.to_owned(),
            map,
            indent,
            trailing_newline: text.ends_with('\n'),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The current value, or the runtime's default if the key is missing or
    /// holds something the runtime wouldn't accept.
    pub fn get(&self, def: &SettingDef) -> Value {
        match self.map.get(def.key) {
            Some(value) if def.accepts(value) => value.clone(),
            _ => def.default.to_json(),
        }
    }

    /// Sets a value in memory (call [`ConfigFile::save`] to write it).
    /// Returns whether anything changed.
    pub fn set(&mut self, def: &SettingDef, value: Value) -> Result<bool, ConfigError> {
        if !def.accepts(&value) {
            return Err(ConfigError::BadValue { key: def.key });
        }
        if self.map.get(def.key) == Some(&value) {
            return Ok(false);
        }
        self.map.insert(def.key.to_owned(), value);
        Ok(true)
    }

    /// Applies Feline's overrides to every linked setting in `defs`.
    /// Returns the keys that changed.
    pub fn apply_overrides(
        &mut self,
        defs: &[SettingDef],
        overrides: &Overrides,
    ) -> Result<Vec<&'static str>, ConfigError> {
        let mut changed = Vec::new();
        for def in defs {
            let Some(link) = def.link else { continue };
            if let Some((value, _)) = managed_value(link, overrides)
                && self.set(def, value)?
            {
                changed.push(def.key);
            }
        }
        Ok(changed)
    }

    /// The file contents as they'd be written.
    pub fn to_text(&self) -> String {
        // serde_json pretty-prints with two-space indentation. Re-indent each
        // line with the file's own unit. Safe line by line, because JSON
        // strings can't contain raw newlines.
        let pretty = serde_json::to_string_pretty(&Value::Object(self.map.clone()))
            .unwrap_or_else(|_| "{}".to_owned());
        let mut out = self.header.clone();
        for (i, line) in pretty.lines().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            let content = line.trim_start_matches(' ');
            let depth = (line.len() - content.len()) / 2;
            out.push_str(&self.indent.repeat(depth));
            out.push_str(content);
        }
        if self.trailing_newline {
            out.push('\n');
        }
        out
    }

    /// Writes the file: a one-time backup first, then an atomic replace.
    pub fn save(&self) -> Result<(), ConfigError> {
        let write_err = |e| ConfigError::Write(self.path.clone(), e);
        let name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "config".to_owned());

        let backup = self.path.with_file_name(format!("{name}.feline-backup"));
        if !backup.exists() {
            fs::copy(&self.path, &backup).map_err(write_err)?;
        }

        let temp = self.path.with_file_name(format!("{name}.feline-tmp"));
        {
            let mut file = fs::File::create(&temp).map_err(write_err)?;
            file.write_all(self.to_text().as_bytes())
                .map_err(write_err)?;
            file.sync_all().map_err(write_err)?;
        }
        // Keep the original file's permissions.
        if let Ok(meta) = fs::metadata(&self.path) {
            let _ = fs::set_permissions(&temp, meta.permissions());
        }
        fs::rename(&temp, &self.path).map_err(write_err)
    }
}

// ---------------------------------------------------------------------------
// Is the runtime running?
// ---------------------------------------------------------------------------

/// True if a process with one of these names runs (for this user or any
/// other). Reads `/proc/<pid>/comm`, which the kernel cuts to 15 characters,
/// so `names` must be given in that cut form. Works for Flatpak apps too:
/// their processes are visible from outside the sandbox.
pub fn any_process_named(names: &[&str]) -> bool {
    any_process_named_in(Path::new("/proc"), names)
}

fn any_process_named_in(proc_dir: &Path, names: &[&str]) -> bool {
    if names.is_empty() {
        return false;
    }
    let Ok(entries) = fs::read_dir(proc_dir) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        let is_pid = entry
            .file_name()
            .to_str()
            .is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit()));
        is_pid
            && fs::read_to_string(entry.path().join("comm"))
                .is_ok_and(|comm| names.contains(&comm.trim_end()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHOICES: &[Choice] = &[
        Choice {
            value: "vulkan",
            label: "Vulkan",
        },
        Choice {
            value: "opengl",
            label: "OpenGL",
        },
    ];
    const TOGGLE: SettingDef = SettingDef {
        key: "gamemode",
        label: "GameMode",
        help: "",
        kind: Kind::Toggle,
        default: Val::Bool(true),
        link: Some(Link::GameMode),
    };
    const RENDERER: SettingDef = SettingDef {
        key: "graphicsMode",
        label: "Renderer",
        help: "",
        kind: Kind::Choice(CHOICES),
        default: Val::Str("vulkan"),
        link: Some(Link::Renderer {
            vulkan: Val::Str("vulkan"),
            opengl: Some(Val::Str("opengl")),
        }),
    };
    const FPS: SettingDef = SettingDef {
        key: "backgroundFps",
        label: "Background FPS",
        help: "",
        kind: Kind::Number { min: 0, max: 240 },
        default: Val::Int(30),
        link: None,
    };

    const SOBER_LIKE: &str = "// !!! STOP !!!\n// Don't edit by hand.\n{\n    \"enable_gamemode\": true,\n    \"fflags\": {\n        \"FFlagExample\": true\n    },\n    \"unknown_key\": [1, 2]\n}\n";

    #[test]
    fn keeps_header_unknown_keys_order_and_indent() {
        let path = Path::new("/x/config.json");
        let mut file = ConfigFile::parse(path, SOBER_LIKE).unwrap();
        assert_eq!(
            file.to_text(),
            SOBER_LIKE.replace("[1, 2]", "[\n        1,\n        2\n    ]")
        );

        let def = SettingDef {
            key: "enable_gamemode",
            ..TOGGLE
        };
        assert!(file.set(&def, Value::Bool(false)).unwrap());
        let text = file.to_text();
        assert!(text.starts_with(
            "// !!! STOP !!!\n// Don't edit by hand.\n{\n    \"enable_gamemode\": false,"
        ));
        assert!(text.contains("\"FFlagExample\": true"));
        assert!(text.ends_with("}\n"));
    }

    #[test]
    fn two_space_files_stay_two_space() {
        let file =
            ConfigFile::parse(Path::new("/x"), "{\n  \"a\": {\n    \"b\": 1\n  }\n}").unwrap();
        assert_eq!(file.to_text(), "{\n  \"a\": {\n    \"b\": 1\n  }\n}");
    }

    #[test]
    fn invalid_files_are_refused() {
        let path = Path::new("/x");
        assert!(matches!(
            ConfigFile::parse(path, "{ broken"),
            Err(ConfigError::Invalid(..))
        ));
        assert!(matches!(
            ConfigFile::parse(path, "[1]"),
            Err(ConfigError::Invalid(..))
        ));
    }

    #[test]
    fn values_are_validated_and_defaults_used() {
        let mut file = ConfigFile::parse(
            Path::new("/x"),
            r#"{"graphicsMode": "metal", "backgroundFps": 60}"#,
        )
        .unwrap();
        // An invalid stored value reads as the default.
        assert_eq!(file.get(&RENDERER), Value::from("vulkan"));
        assert_eq!(file.get(&FPS), Value::from(60));
        assert_eq!(file.get(&TOGGLE), Value::Bool(true), "missing → default");

        assert!(matches!(
            file.set(&RENDERER, Value::from("directx")),
            Err(ConfigError::BadValue { .. })
        ));
        assert!(matches!(
            file.set(&FPS, Value::from(999)),
            Err(ConfigError::BadValue { .. })
        ));
        assert!(matches!(
            file.set(&TOGGLE, Value::from("yes")),
            Err(ConfigError::BadValue { .. })
        ));
        assert!(!file.set(&FPS, Value::from(60)).unwrap(), "unchanged");
    }

    #[test]
    fn overrides_change_only_linked_settings() {
        let mut file = ConfigFile::parse(
            Path::new("/x"),
            r#"{"gamemode": true, "graphicsMode": "vulkan", "backgroundFps": 30}"#,
        )
        .unwrap();
        let defs = [TOGGLE, RENDERER, FPS];

        let none = Overrides::default();
        assert!(file.apply_overrides(&defs, &none).unwrap().is_empty());

        let overrides = Overrides {
            renderer: Renderer::OpenGl,
            gamemode: Toggle::Off,
            close_on_leave: Toggle::On, // no linked setting here
            discord_presence: false,
        };
        let changed = file.apply_overrides(&defs, &overrides).unwrap();
        assert_eq!(changed, ["gamemode", "graphicsMode"]);
        assert_eq!(file.get(&TOGGLE), Value::Bool(false));
        assert_eq!(file.get(&RENDERER), Value::from("opengl"));

        // OpenGL value unknown for this runtime: left alone.
        let unknown_gl = SettingDef {
            link: Some(Link::Renderer {
                vulkan: Val::Str("vulkan"),
                opengl: None,
            }),
            ..RENDERER
        };
        assert_eq!(managed_value(unknown_gl.link.unwrap(), &overrides), None);
    }

    #[test]
    fn save_backs_up_once_and_replaces_atomically() {
        let dir = std::env::temp_dir().join(format!("feline-test-config-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        fs::write(&path, "{\n    \"gamemode\": true\n}\n").unwrap();

        let mut file = ConfigFile::load(&path).unwrap();
        file.set(&TOGGLE, Value::Bool(false)).unwrap();
        file.save().unwrap();
        file.set(&TOGGLE, Value::Bool(true)).unwrap();
        file.save().unwrap();

        let saved = fs::read_to_string(&path).unwrap();
        let backup = fs::read_to_string(dir.join("config.json.feline-backup")).unwrap();
        let temp_left = dir.join("config.json.feline-tmp").exists();
        let missing = ConfigFile::load(&dir.join("nope.json"));
        let _ = fs::remove_dir_all(&dir);

        assert_eq!(saved, "{\n    \"gamemode\": true\n}\n");
        assert_eq!(
            backup, "{\n    \"gamemode\": true\n}\n",
            "the original, not the first edit"
        );
        assert!(!temp_left);
        assert!(matches!(missing, Err(ConfigError::Missing(_))));
    }

    #[test]
    fn finds_running_processes_by_name() {
        let dir = std::env::temp_dir().join(format!("feline-test-proc-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        for (pid, comm) in [("1", "runit\n"), ("4242", "sober\n"), ("self", "sober\n")] {
            fs::create_dir_all(dir.join(pid)).unwrap();
            fs::write(dir.join(pid).join("comm"), comm).unwrap();
        }
        let sober = any_process_named_in(&dir, &["sober"]);
        let stud = any_process_named_in(&dir, &["stud"]);
        let _ = fs::remove_dir_all(&dir);
        assert!(sober);
        assert!(!stud);
    }
}

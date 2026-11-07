// SPDX-License-Identifier: GPL-3.0-or-later
//! Runtime backends.
//!
//! A *runtime* is a separate program that runs the Roblox client: Feline's
//! own, `feline-runtime` (in development, see [`feline`]), or **external,
//! third-party** runtimes the user installed themselves, which Feline drives
//! through their public command line only.
//!
//! The pieces:
//! - [`RuntimeBackend`]: one implementation per runtime project. It knows how
//!   to find that runtime and which command-line options to use.
//! - [`Installation`]: one concrete install that was found (a Flatpak, an
//!   AppImage, …). It knows *how to start* the program, independent of which
//!   runtime it is.
//! - [`detect_all`]: asks every backend for its installations.
//!
//! Policy: no installation is ever chosen automatically. The user picks one.
//! Nothing in this file is specific to any external runtime.

pub mod config;
pub mod cordial;
pub mod feline;
pub mod mocktail;
pub mod sober;
pub mod stud;
pub mod tipsy;

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::link::GameLink;
use crate::process::RunError;
use crate::settings::Overrides;
use config::{ConfigError, ConfigFile, SettingDef};

/// Everything a backend may look at while detecting. Kept explicit (instead of
/// reading `$HOME` etc. directly) so tests can use a fake environment.
#[derive(Debug, Clone)]
pub struct DetectContext {
    pub home: Option<PathBuf>,
    /// The `PATH` variable, searched for native binaries.
    pub path: Option<OsString>,
    /// System-wide Flatpak installation, normally `/var/lib/flatpak`.
    pub flatpak_system_dir: PathBuf,
    /// `$XDG_CONFIG_HOME` if set (and absolute), for non-Flatpak runtimes'
    /// settings files. `None` means `~/.config`.
    pub config_home: Option<PathBuf>,
}

impl DetectContext {
    /// The real environment of this process.
    pub fn host() -> Self {
        Self {
            home: std::env::var_os("HOME").map(PathBuf::from),
            path: std::env::var_os("PATH"),
            // Flatpak itself honours this override, so we do too.
            flatpak_system_dir: std::env::var_os("FLATPAK_SYSTEM_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/var/lib/flatpak")),
            config_home: std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute()),
        }
    }

    /// The settings folder of a non-Flatpak program, e.g. `~/.config/stud`.
    pub fn config_dir(&self, program: &str) -> Option<PathBuf> {
        let base = match &self.config_home {
            Some(dir) => dir.clone(),
            None => self.home.as_ref()?.join(".config"),
        };
        Some(base.join(program))
    }

    /// The data folder of a non-Flatpak program, e.g. `~/.local/share/stud`.
    pub fn data_dir(&self, program: &str) -> Option<PathBuf> {
        Some(crate::paths::data_home()?.join(program))
    }

    /// The data folder a Flatpak app sees as its data home, e.g.
    /// `~/.var/app/org.vinegarhq.Sober/data`.
    pub fn flatpak_data_dir(&self, app_id: &str) -> Option<PathBuf> {
        Some(
            self.home
                .as_ref()?
                .join(".var/app")
                .join(app_id)
                .join("data"),
        )
    }

    /// The settings folder a Flatpak app sees as its config home, e.g.
    /// `~/.var/app/org.vinegarhq.Sober/config`.
    pub fn flatpak_config_dir(&self, app_id: &str) -> Option<PathBuf> {
        Some(
            self.home
                .as_ref()?
                .join(".var/app")
                .join(app_id)
                .join("config"),
        )
    }

    /// The per-user Flatpak installation, `~/.local/share/flatpak`.
    pub fn flatpak_user_dir(&self) -> Option<PathBuf> {
        self.home.as_ref().map(|h| h.join(".local/share/flatpak"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlatpakScope {
    User,
    System,
}

/// How an installation is packaged, which decides how it's started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallKind {
    /// Started with `flatpak run --user|--system <app_id>`.
    Flatpak {
        app_id: &'static str,
        scope: FlatpakScope,
    },
    /// A self-contained AppImage file, started directly.
    AppImage(PathBuf),
    /// A regular executable (distro package, `cargo install`, …).
    Native(PathBuf),
}

impl fmt::Display for InstallKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Flatpak {
                scope: FlatpakScope::User,
                ..
            } => f.write_str("Flatpak (user)"),
            Self::Flatpak {
                scope: FlatpakScope::System,
                ..
            } => f.write_str("Flatpak (system)"),
            Self::AppImage(path) => write!(f, "AppImage ({})", path.display()),
            Self::Native(path) => write!(f, "installed program ({})", path.display()),
        }
    }
}

/// One runtime installation found on this machine.
#[derive(Clone)]
pub struct Installation {
    /// The backend that found it, used for runtime-specific operations.
    pub backend: &'static dyn RuntimeBackend,
    pub kind: InstallKind,
    /// Version, if it could be read cheaply during detection (no process
    /// started). Otherwise use [`Installation::version`].
    pub known_version: Option<String>,
}

// `dyn RuntimeBackend` has no Debug, so we write Debug by hand.
impl fmt::Debug for Installation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Installation")
            .field("backend", &self.backend.id())
            .field("kind", &self.kind)
            .field("known_version", &self.known_version)
            .finish()
    }
}

impl fmt::Display for Installation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.backend.name())?;
        if let Some(v) = &self.known_version {
            write!(f, " {v}")?;
        }
        write!(f, " · {} · {}", self.backend.origin(), self.kind)
    }
}

/// Who develops a runtime. Shown to users wherever a runtime is listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Developed by the Feline project: `feline-runtime`.
    Feline,
    /// A separate, third-party project. Feline only starts it through its
    /// public interface and contains none of its code.
    External { project_url: &'static str },
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Feline => f.write_str("Feline runtime"),
            Self::External { .. } => f.write_str("external third-party runtime"),
        }
    }
}

impl Installation {
    /// A `Command` that starts this installation. The caller adds arguments.
    pub fn command(&self) -> Command {
        match &self.kind {
            InstallKind::Flatpak { app_id, scope } => {
                let mut cmd = Command::new("flatpak");
                cmd.arg("run");
                cmd.arg(match scope {
                    FlatpakScope::User => "--user",
                    FlatpakScope::System => "--system",
                });
                cmd.arg(app_id);
                cmd
            }
            InstallKind::AppImage(path) | InstallKind::Native(path) => Command::new(path),
        }
    }

    /// The version, starting the program if it wasn't known from detection.
    pub fn version(&self) -> Result<String, RuntimeError> {
        match &self.known_version {
            Some(v) => Ok(v.clone()),
            None => self.backend.version(self),
        }
    }

    pub fn doctor(&self) -> Result<DoctorReport, RuntimeError> {
        self.backend.doctor(self)
    }

    pub fn diagnostics(&self) -> Result<String, RuntimeError> {
        self.backend.diagnostics(self)
    }

    pub fn plan_launch(&self, target: &LaunchTarget) -> Result<LaunchPlan, RuntimeError> {
        self.backend.plan_launch(self, target)
    }

    /// Where this installation keeps its own settings file, if Feline can edit it.
    pub fn config_path(&self, ctx: &DetectContext) -> Option<PathBuf> {
        self.backend.config_path(self, ctx)
    }

    /// Where this installation's Roblox client writes its logs.
    pub fn roblox_log_roots(&self, ctx: &DetectContext) -> Vec<PathBuf> {
        self.backend.roblox_log_roots(self, ctx)
    }

    /// True if this runtime is running right now (any installation of it).
    pub fn is_running(&self) -> bool {
        config::any_process_named(self.backend.process_names())
    }

    /// Opens this installation's settings file for editing.
    pub fn load_config(&self, ctx: &DetectContext) -> Result<ConfigFile, ConfigError> {
        let path = self
            .config_path(ctx)
            .ok_or(ConfigError::Missing(PathBuf::new()))?;
        ConfigFile::load(&path)
    }

    /// Changes one setting and saves the file, unless the runtime is running.
    pub fn set_config_value(
        &self,
        ctx: &DetectContext,
        def: &SettingDef,
        value: serde_json::Value,
    ) -> Result<(), ConfigError> {
        if self.is_running() {
            return Err(ConfigError::Running(self.backend.name()));
        }
        let mut file = self.load_config(ctx)?;
        if file.set(def, value)? {
            file.save()?;
        }
        Ok(())
    }

    /// Writes Feline's overrides into this installation's settings file.
    pub fn apply_overrides(
        &self,
        ctx: &DetectContext,
        overrides: &Overrides,
    ) -> Result<ApplyOutcome, ConfigError> {
        let defs = self.backend.settings();
        if defs.is_empty() || self.config_path(ctx).is_none() {
            return Ok(ApplyOutcome::NotEditable);
        }
        let mut file = match self.load_config(ctx) {
            Ok(file) => file,
            // Nothing to override yet: the runtime creates its file on first start.
            Err(ConfigError::Missing(_)) => return Ok(ApplyOutcome::NoFile),
            Err(e) => return Err(e),
        };
        let changed = file.apply_overrides(defs, overrides)?;
        if changed.is_empty() {
            return Ok(ApplyOutcome::Unchanged);
        }
        if self.is_running() {
            return Err(ConfigError::Running(self.backend.name()));
        }
        file.save()?;
        Ok(ApplyOutcome::Changed(changed))
    }

    /// A stable string identifying this installation, used to remember the
    /// user's choice in the settings file, e.g. `cordial:flatpak-system` or
    /// `cordial:appimage:/home/me/Applications/Cordial.AppImage`.
    pub fn key(&self) -> String {
        let id = self.backend.id();
        match &self.kind {
            InstallKind::Flatpak {
                scope: FlatpakScope::User,
                ..
            } => format!("{id}:flatpak-user"),
            InstallKind::Flatpak {
                scope: FlatpakScope::System,
                ..
            } => format!("{id}:flatpak-system"),
            InstallKind::AppImage(path) => format!("{id}:appimage:{}", path.display()),
            InstallKind::Native(path) => format!("{id}:native:{}", path.display()),
        }
    }
}

/// What [`Installation::apply_overrides`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// This runtime has no settings Feline edits.
    NotEditable,
    /// The runtime hasn't created its settings file yet.
    NoFile,
    /// Everything already matched.
    Unchanged,
    /// These keys were changed.
    Changed(Vec<&'static str>),
}

/// What the user wants to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchTarget {
    /// The Roblox app itself (its home screen).
    Home,
    /// A specific experience.
    Game(GameLink),
}

/// How to start a runtime for a [`LaunchTarget`], decided by its backend.
#[derive(Debug)]
pub struct LaunchPlan {
    pub command: Command,
    /// Shown to the user when the runtime can't do exactly what was asked,
    /// e.g. an older version that can only open its own window.
    pub note: Option<String>,
}

/// Result of a runtime's self-check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorReport {
    /// False if the runtime reported something that will stop it starting.
    pub ok: bool,
    /// The runtime's own human-readable output, shown as-is.
    pub output: String,
}

/// One tab of a runtime's settings page (see [`RuntimeBackend::setting_tabs`]).
#[derive(Debug, Clone, Copy)]
pub struct SettingTab {
    pub name: &'static str,
    pub sections: &'static [SettingSection],
}

/// A titled group of settings inside a tab ("" for no title).
#[derive(Debug, Clone, Copy)]
pub struct SettingSection {
    pub title: &'static str,
    pub keys: &'static [&'static str],
}

/// A runtime project Feline can work with.
///
/// `Send + Sync` lets backends be used from background threads (the GUI runs
/// slow operations off the UI thread).
pub trait RuntimeBackend: Send + Sync {
    /// Stable identifier, e.g. "cordial".
    fn id(&self) -> &'static str;
    /// Human-readable name, e.g. "Cordial".
    fn name(&self) -> &'static str;
    /// Whether this is Feline's own runtime or an external project.
    fn origin(&self) -> Origin;
    /// Finds installations. Must be cheap: file checks only, no processes.
    fn detect(&'static self, ctx: &DetectContext) -> Vec<Installation>;
    /// Asks the program for its version.
    fn version(&self, installation: &Installation) -> Result<String, RuntimeError>;
    /// True if the runtime has a self-check ([`RuntimeBackend::doctor`]).
    fn has_doctor(&self) -> bool {
        false
    }
    /// True if the runtime can describe itself for bug reports
    /// ([`RuntimeBackend::diagnostics`]).
    fn has_diagnostics(&self) -> bool {
        false
    }
    /// Runs the runtime's self-check. The default is for runtimes without one.
    fn doctor(&self, _installation: &Installation) -> Result<DoctorReport, RuntimeError> {
        Err(RuntimeError::Unsupported {
            what: "a self-check",
        })
    }
    /// A block of text describing the install, meant for bug reports.
    fn diagnostics(&self, _installation: &Installation) -> Result<String, RuntimeError> {
        Err(RuntimeError::Unsupported {
            what: "a bug-report summary",
        })
    }
    /// The settings of this runtime that Feline shows and may change. Empty
    /// for runtimes whose settings file Feline doesn't edit.
    fn settings(&self) -> &'static [SettingDef] {
        &[]
    }
    /// How `settings` are grouped, like the runtime's own launcher: tabs of
    /// titled sections of setting keys. Empty: one tab with everything.
    fn setting_tabs(&self) -> &'static [SettingTab] {
        &[]
    }
    /// Where `installation` keeps the settings file described by `settings`.
    fn config_path(&self, _installation: &Installation, _ctx: &DetectContext) -> Option<PathBuf> {
        None
    }
    /// Folders under which the Roblox client of `installation` writes its
    /// logs (searched a few levels deep), for the Discord status.
    fn roblox_log_roots(&self, _installation: &Installation, _ctx: &DetectContext) -> Vec<PathBuf> {
        Vec::new()
    }
    /// Process names (as in `/proc/<pid>/comm`, cut to 15 characters) that
    /// mean this runtime is running.
    fn process_names(&self) -> &'static [&'static str] {
        &[]
    }
    /// Builds the command that starts `target`. May start the program briefly
    /// to find out which options it supports, so call it off the UI thread.
    fn plan_launch(
        &self,
        installation: &Installation,
        target: &LaunchTarget,
    ) -> Result<LaunchPlan, RuntimeError>;
}

/// Every backend Feline knows about. The order is not a preference: Feline
/// never picks a runtime on the user's behalf. A `static` lives for the whole
/// program, so we can hand out `'static` references.
static BACKENDS: &[&dyn RuntimeBackend] = &[
    &feline::FELINE,
    &cordial::CORDIAL,
    &sober::SOBER,
    &stud::STUD,
    &tipsy::TIPSY,
    &mocktail::MOCKTAIL,
    &mocktail::NIGHTCAP,
];

pub fn backends() -> &'static [&'static dyn RuntimeBackend] {
    BACKENDS
}

/// All installations of all backends, in detection order (not a ranking).
pub fn detect_all(ctx: &DetectContext) -> Vec<Installation> {
    backends().iter().flat_map(|b| b.detect(ctx)).collect()
}

/// Errors from runtime operations, written to be shown to users.
#[derive(Debug)]
pub enum RuntimeError {
    /// The program couldn't be run to completion.
    Run(RunError),
    /// The program ran but reported failure.
    Failed {
        what: &'static str,
        code: Option<i32>,
        /// The tail of the program's error output, for context.
        detail: String,
    },
    /// The program's output wasn't in the expected format.
    UnexpectedOutput { what: &'static str },
    /// The runtime doesn't offer this, e.g. Sober has no self-check.
    Unsupported { what: &'static str },
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Run(RunError::NotFound { program }) if program == "flatpak" => f.write_str(
                "the `flatpak` command was not found. Install Flatpak, or use a non-Flatpak install of the runtime.",
            ),
            Self::Run(e) => e.fmt(f),
            Self::Failed { what, code, detail } => {
                match code {
                    Some(c) => write!(f, "{what} failed (exit code {c})")?,
                    None => write!(f, "{what} was stopped by a signal")?,
                }
                if !detail.is_empty() {
                    write!(f, ":\n{detail}")?;
                }
                Ok(())
            }
            Self::UnexpectedOutput { what } => {
                write!(f, "{what} gave output Feline doesn't understand")
            }
            Self::Unsupported { what } => write!(f, "this runtime doesn't offer {what}"),
        }
    }
}

impl std::error::Error for RuntimeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Run(e) => Some(e),
            _ => None,
        }
    }
}

// `From` lets us use `?` on a `RunError` inside functions returning `RuntimeError`.
impl From<RunError> for RuntimeError {
    fn from(e: RunError) -> Self {
        Self::Run(e)
    }
}

/// Returns the last `max_lines` lines of `text`, for short error summaries.
pub fn tail_lines(text: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = text.trim_end().lines().collect();
    lines[lines.len().saturating_sub(max_lines)..].join("\n")
}

// ---------------------------------------------------------------------------
// Detection helpers shared by the backends
// ---------------------------------------------------------------------------

/// Folders where people commonly keep AppImages (searched non-recursively).
const APPIMAGE_DIRS: &[&str] = &["Applications", "AppImages", ".local/bin", ".local/opt"];

/// The user and system installs of Flatpak `app_id`, as (scope, path of the
/// active deployment). Only checks folders; starts nothing.
pub(crate) fn flatpak_installs(ctx: &DetectContext, app_id: &str) -> Vec<(FlatpakScope, PathBuf)> {
    let roots = [
        (ctx.flatpak_user_dir(), FlatpakScope::User),
        (Some(ctx.flatpak_system_dir.clone()), FlatpakScope::System),
    ];
    roots
        .into_iter()
        .filter_map(|(dir, scope)| {
            let active = dir?.join("app").join(app_id).join("current/active");
            active.is_dir().then_some((scope, active))
        })
        .collect()
}

/// The first executable called `name` on `PATH`, like the shell would find.
pub(crate) fn find_on_path(ctx: &DetectContext, name: &str) -> Option<PathBuf> {
    let path_var = ctx.path.as_ref()?;
    std::env::split_paths(path_var)
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

/// Executable files named `<prefix>*.AppImage` (any case) in the usual
/// AppImage folders, sorted within each folder.
pub(crate) fn find_appimages(ctx: &DetectContext, prefix: &str) -> Vec<PathBuf> {
    let Some(home) = &ctx.home else {
        return Vec::new();
    };
    let prefix = prefix.to_ascii_lowercase();
    let mut found = Vec::new();
    for dir in APPIMAGE_DIRS {
        let Ok(entries) = fs::read_dir(home.join(dir)) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(str::to_ascii_lowercase)
                    .is_some_and(|n| n.starts_with(&prefix) && n.ends_with(".appimage"))
            })
            .filter(|p| is_executable(p))
            .collect();
        paths.sort();
        found.extend(paths);
    }
    found
}

/// Reads the newest release version from an AppStream metainfo file.
pub(crate) fn metainfo_version(file: &Path) -> Option<String> {
    parse_metainfo_version(&fs::read_to_string(file).ok()?)
}

/// Extracts the first `<release version="…">` from AppStream XML. Releases
/// are listed newest first. A full XML parser would be a big dependency for
/// one attribute; this handles the standard form and returns None otherwise.
pub fn parse_metainfo_version(xml: &str) -> Option<String> {
    let after = xml.split("<release ").nth(1)?;
    let tag = after.split('>').next()?;
    let value = tag.split("version=").nth(1)?;
    let quote = value.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    let version = value[1..].split(quote).next()?;
    (!version.is_empty()).then(|| version.to_owned())
}

/// True if `path` is a regular file with an execute bit set.
pub(crate) fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metainfo_version_parsing() {
        let xml = r#"<component><releases>
            <release version="0.27.0" date="2026-10-08"><description/></release>
            <release version="0.26.0" date="2026-10-08"/>
        </releases></component>"#;
        assert_eq!(parse_metainfo_version(xml).as_deref(), Some("0.27.0"));
        assert_eq!(
            parse_metainfo_version("<release date='x' version='1.2'/>").as_deref(),
            Some("1.2")
        );
        assert_eq!(parse_metainfo_version("<component/>"), None);
    }

    #[test]
    fn every_setting_is_in_exactly_one_tab() {
        for backend in backends() {
            let tabs = backend.setting_tabs();
            if tabs.is_empty() {
                continue;
            }
            let mut listed: Vec<&str> = tabs
                .iter()
                .flat_map(|t| t.sections.iter())
                .flat_map(|s| s.keys.iter().copied())
                .collect();
            let mut defined: Vec<&str> = backend.settings().iter().map(|d| d.key).collect();
            listed.sort_unstable();
            defined.sort_unstable();
            assert_eq!(
                listed,
                defined,
                "{}: tabs and settings differ",
                backend.id()
            );
        }
    }

    #[test]
    fn tail_keeps_last_lines() {
        assert_eq!(tail_lines("a\nb\nc\n", 2), "b\nc");
        assert_eq!(tail_lines("a", 5), "a");
        assert_eq!(tail_lines("", 3), "");
    }
}

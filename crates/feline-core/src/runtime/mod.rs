// SPDX-License-Identifier: GPL-3.0-or-later
//! Runtime backends.
//!
//! A *runtime* is a separate program that runs the Roblox client. Feline's
//! own runtime is a long-term goal and doesn't exist yet. Until then, Feline
//! can optionally drive **external, third-party** runtimes the user installed
//! themselves, through their public command line only.
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

pub mod cordial;

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::link::GameLink;
use crate::process::RunError;

/// Everything a backend may look at while detecting. Kept explicit (instead of
/// reading `$HOME` etc. directly) so tests can use a fake environment.
#[derive(Debug, Clone)]
pub struct DetectContext {
    pub home: Option<PathBuf>,
    /// The `PATH` variable, searched for native binaries.
    pub path: Option<OsString>,
    /// System-wide Flatpak installation, normally `/var/lib/flatpak`.
    pub flatpak_system_dir: PathBuf,
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
        }
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
    /// Developed by the Feline project (none exists yet).
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
    /// Runs the runtime's self-check.
    fn doctor(&self, installation: &Installation) -> Result<DoctorReport, RuntimeError>;
    /// A block of text describing the install, meant for bug reports.
    fn diagnostics(&self, installation: &Installation) -> Result<String, RuntimeError>;
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
static BACKENDS: &[&dyn RuntimeBackend] = &[&cordial::CORDIAL];

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
    fn tail_keeps_last_lines() {
        assert_eq!(tail_lines("a\nb\nc\n", 2), "b\nc");
        assert_eq!(tail_lines("a", 5), "a");
        assert_eq!(tail_lines("", 3), "");
    }
}

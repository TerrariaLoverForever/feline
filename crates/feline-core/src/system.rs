// SPDX-License-Identifier: GPL-3.0-or-later
//! Host system information.
//!
//! On Linux, everything is read directly from `/proc`, `/sys`, `/etc` and
//! environment variables: no external commands, no extra crates. On Windows,
//! from the registry and one memory query (see [`windows_info`]). Every field is
//! best-effort. A missing file gives `None`/`Unknown` rather than an error,
//! because a missing value is information to show the user, not a failure.
//!
//! Design note (for testing): the parsers are plain functions that take file
//! *contents* (`&str`), so tests can feed them sample text. The [`Probe`] type
//! does the actual file reading, and it can be pointed at a fake root
//! directory so the sysfs/flatpak scanning can be tested too.

use std::fmt::{self, Write as _};
use std::fs;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// A snapshot of the machine Feline is running on.
#[derive(Debug, Clone, PartialEq)]
pub struct SystemInfo {
    /// Distribution name, e.g. "Void Linux" (from `os-release`).
    pub os: Option<String>,
    /// Kernel release, e.g. "6.18.58_1".
    pub kernel: Option<String>,
    /// CPU model name, e.g. "AMD Ryzen 7 5700X 8-Core Processor".
    pub cpu: Option<String>,
    /// Logical CPU count (threads).
    pub cpu_threads: usize,
    /// Total RAM in KiB.
    pub memory_total_kib: Option<u64>,
    /// GPUs found under `/sys/class/drm`.
    pub gpus: Vec<Gpu>,
    /// Version of the loaded NVIDIA kernel driver, if any.
    pub nvidia_driver: Option<String>,
    /// Whether Flatpak apps can use the NVIDIA driver (see [`FlatpakNvidia`]).
    pub flatpak_nvidia: FlatpakNvidia,
    /// Wayland, X11, …
    pub session: SessionType,
    /// Desktop environment, e.g. "KDE" (from `XDG_CURRENT_DESKTOP`).
    pub desktop: Option<String>,
    /// The init system (PID 1).
    pub init: InitSystem,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuVendor {
    Nvidia,
    Amd,
    Intel,
    Other(u16),
}

impl GpuVendor {
    /// Maps a PCI vendor ID to a vendor.
    pub fn from_pci_id(id: u16) -> Self {
        match id {
            0x10de => Self::Nvidia,
            0x1002 => Self::Amd,
            0x8086 => Self::Intel,
            other => Self::Other(other),
        }
    }
}

impl fmt::Display for GpuVendor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Nvidia => f.write_str("NVIDIA"),
            Self::Amd => f.write_str("AMD"),
            Self::Intel => f.write_str("Intel"),
            Self::Other(id) => write!(f, "vendor {id:04x}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gpu {
    pub vendor: GpuVendor,
    pub vendor_id: u16,
    pub device_id: u16,
    /// Marketing name from the system's `pci.ids` database, if installed.
    pub name: Option<String>,
    /// Kernel driver in use, e.g. "nvidia", "amdgpu", "i915".
    pub driver: Option<String>,
    /// True for the GPU the firmware used at boot (usually the primary one).
    pub boot_vga: bool,
}

impl fmt::Display for Gpu {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.name {
            // Windows' names already start with the vendor ("Intel(R) UHD …").
            Some(name)
                if name
                    .to_ascii_lowercase()
                    .starts_with(&self.vendor.to_string().to_ascii_lowercase()) =>
            {
                f.write_str(name)?
            }
            Some(name) => write!(f, "{} {name}", self.vendor)?,
            None => write!(
                f,
                "{} device {:04x}:{:04x}",
                self.vendor, self.vendor_id, self.device_id
            )?,
        }
        if let Some(driver) = &self.driver {
            write!(f, " (driver: {driver})")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionType {
    Wayland,
    X11,
    Tty,
    Unknown,
}

impl fmt::Display for SessionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Wayland => "Wayland",
            Self::X11 => "X11",
            Self::Tty => "TTY (no graphical session)",
            Self::Unknown => "Unknown",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InitSystem {
    Systemd,
    Runit,
    OpenRc,
    S6,
    Dinit,
    /// PID 1 has a name we don't recognise.
    Other(String),
    Unknown,
}

impl fmt::Display for InitSystem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Systemd => f.write_str("systemd"),
            Self::Runit => f.write_str("runit"),
            Self::OpenRc => f.write_str("OpenRC"),
            Self::S6 => f.write_str("s6"),
            Self::Dinit => f.write_str("dinit"),
            Self::Other(name) => f.write_str(name),
            Self::Unknown => f.write_str("Unknown"),
        }
    }
}

/// Flatpak apps can't use the host's NVIDIA driver directly. They need the
/// `org.freedesktop.Platform.GL.nvidia-<version>` extension, and its version
/// must match the host driver *exactly*, otherwise the app silently falls back
/// to software/Mesa drivers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlatpakNvidia {
    /// No NVIDIA driver loaded, so this check doesn't apply.
    NotApplicable,
    /// An extension matching the host driver is installed.
    Match { version: String },
    /// NVIDIA driver present, but no NVIDIA GL extension installed at all.
    Missing { host: String },
    /// Extensions are installed, but none matches the host driver.
    Mismatch {
        host: String,
        installed: Vec<String>,
    },
}

impl fmt::Display for FlatpakNvidia {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotApplicable => f.write_str("not applicable (no NVIDIA driver)"),
            Self::Match { version } => write!(f, "OK (GL.nvidia-{version} installed)"),
            Self::Missing { host } => write!(
                f,
                "missing: no GL.nvidia extension for driver {host}. Flatpak apps can't use the NVIDIA GPU."
            ),
            Self::Mismatch { host, installed } => write!(
                f,
                "MISMATCH: host driver is {host}, installed extensions are {}. Run `flatpak update`.",
                installed.join(", ")
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Collecting
// ---------------------------------------------------------------------------

/// The environment variables that describe the graphical session.
/// Kept separate so tests don't depend on the real environment.
#[derive(Debug, Clone, Default)]
pub struct SessionEnv {
    pub xdg_session_type: Option<String>,
    pub wayland_display: Option<String>,
    pub display: Option<String>,
    pub xdg_current_desktop: Option<String>,
}

impl SessionEnv {
    /// Reads the variables from the current process environment.
    pub fn from_process() -> Self {
        // `.ok()` turns "not set / not valid Unicode" into None.
        let var = |name| std::env::var(name).ok().filter(|v: &String| !v.is_empty());
        Self {
            xdg_session_type: var("XDG_SESSION_TYPE"),
            wayland_display: var("WAYLAND_DISPLAY"),
            display: var("DISPLAY"),
            xdg_current_desktop: var("XDG_CURRENT_DESKTOP"),
        }
    }
}

/// Reads system information from a filesystem root (normally `/`).
pub struct Probe {
    root: PathBuf,
    home: Option<PathBuf>,
    env: SessionEnv,
}

impl Probe {
    /// A probe for the real machine.
    pub fn host() -> Self {
        Self {
            root: PathBuf::from("/"),
            home: std::env::var_os("HOME").map(PathBuf::from),
            env: SessionEnv::from_process(),
        }
    }

    /// A probe for a fake filesystem tree (used by tests).
    pub fn with_root(root: impl Into<PathBuf>, home: Option<PathBuf>, env: SessionEnv) -> Self {
        Self {
            root: root.into(),
            home,
            env,
        }
    }

    /// Turns an absolute path like "/proc/meminfo" into one under `root`.
    fn path(&self, absolute: &str) -> PathBuf {
        self.root.join(absolute.trim_start_matches('/'))
    }

    fn read(&self, absolute: &str) -> Option<String> {
        fs::read_to_string(self.path(absolute)).ok()
    }

    pub fn collect(&self) -> SystemInfo {
        let os = self
            .read("/etc/os-release")
            .or_else(|| self.read("/usr/lib/os-release"))
            .and_then(|s| parse_os_release(&s));

        let kernel = self
            .read("/proc/sys/kernel/osrelease")
            .map(|s| s.trim().to_owned());

        let (cpu, cpu_threads) = self
            .read("/proc/cpuinfo")
            .map(|s| parse_cpuinfo(&s))
            .unwrap_or((None, 0));

        let memory_total_kib = self
            .read("/proc/meminfo")
            .and_then(|s| parse_meminfo_total_kib(&s));

        let nvidia_driver = self
            .read("/proc/driver/nvidia/version")
            .and_then(|s| parse_nvidia_version(&s));

        let flatpak_nvidia = match &nvidia_driver {
            None => FlatpakNvidia::NotApplicable,
            Some(host) => check_flatpak_nvidia(host, &self.flatpak_nvidia_extensions()),
        };

        SystemInfo {
            os,
            kernel,
            cpu,
            cpu_threads,
            memory_total_kib,
            gpus: self.gpus(),
            nvidia_driver,
            flatpak_nvidia,
            session: session_type(&self.env),
            desktop: self.env.xdg_current_desktop.clone(),
            init: self.init_system(),
        }
    }

    /// Scans `/sys/class/drm/cardN` entries (not `cardN-DP-1` connectors or
    /// `renderD*` nodes) for GPUs.
    fn gpus(&self) -> Vec<Gpu> {
        let Ok(entries) = fs::read_dir(self.path("/sys/class/drm")) else {
            return Vec::new();
        };
        let pci_ids = self
            .read("/usr/share/hwdata/pci.ids")
            .or_else(|| self.read("/usr/share/misc/pci.ids"));

        let mut cards: Vec<(String, PathBuf)> = entries
            .filter_map(Result::ok)
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                let digits = name.strip_prefix("card")?;
                let is_card = !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit());
                is_card.then(|| (name, e.path().join("device")))
            })
            .collect();
        cards.sort(); // read_dir order is arbitrary; keep card0, card1, … stable

        cards
            .into_iter()
            .filter_map(|(_, device)| {
                let read_hex = |file: &str| {
                    let text = fs::read_to_string(device.join(file)).ok()?;
                    u16::from_str_radix(text.trim().trim_start_matches("0x"), 16).ok()
                };
                let vendor_id = read_hex("vendor")?;
                let device_id = read_hex("device")?;
                let driver = fs::read_link(device.join("driver"))
                    .ok()
                    .and_then(|p| p.file_name()?.to_str().map(str::to_owned));
                let boot_vga =
                    fs::read_to_string(device.join("boot_vga")).is_ok_and(|s| s.trim() == "1");
                Some(Gpu {
                    vendor: GpuVendor::from_pci_id(vendor_id),
                    vendor_id,
                    device_id,
                    name: pci_ids
                        .as_deref()
                        .and_then(|db| lookup_pci_device_name(db, vendor_id, device_id)),
                    driver,
                    boot_vga,
                })
            })
            .collect()
    }

    /// Versions of installed `org.freedesktop.Platform.GL.nvidia-*` extensions,
    /// from both the per-user and the system-wide Flatpak installation.
    /// Directory names use dashes (`nvidia-595-104-02`), returned as-is.
    fn flatpak_nvidia_extensions(&self) -> Vec<String> {
        const PREFIX: &str = "org.freedesktop.Platform.GL.nvidia-";
        let mut dirs = vec![self.path("/var/lib/flatpak/runtime")];
        if let Some(home) = &self.home {
            dirs.push(home.join(".local/share/flatpak/runtime"));
        }
        let mut versions: Vec<String> = dirs
            .iter()
            .filter_map(|d| fs::read_dir(d).ok())
            .flatten()
            .filter_map(Result::ok)
            .filter_map(|e| Some(e.file_name().to_str()?.strip_prefix(PREFIX)?.to_owned()))
            .collect();
        versions.sort();
        versions.dedup();
        versions
    }

    fn init_system(&self) -> InitSystem {
        // Marker directories are more reliable than PID 1's name: OpenRC, for
        // example, usually runs under sysvinit's `init`.
        let exists = |p: &str| self.path(p).exists();
        if exists("/run/systemd/system") {
            return InitSystem::Systemd;
        }
        if exists("/run/openrc") {
            return InitSystem::OpenRc;
        }
        if exists("/run/runit") || exists("/etc/runit/runsvdir") {
            return InitSystem::Runit;
        }
        if exists("/run/s6") || exists("/run/s6-rc") {
            return InitSystem::S6;
        }
        match self.read("/proc/1/comm").as_deref().map(str::trim) {
            Some("systemd") => InitSystem::Systemd,
            Some("runit") => InitSystem::Runit,
            Some("openrc-init") => InitSystem::OpenRc,
            Some("s6-svscan") => InitSystem::S6,
            Some("dinit") => InitSystem::Dinit,
            Some(other) if !other.is_empty() => InitSystem::Other(other.to_owned()),
            _ => InitSystem::Unknown,
        }
    }
}

impl SystemInfo {
    /// Collects information about the real machine.
    pub fn collect() -> Self {
        #[cfg(windows)]
        return windows_info::collect();
        #[cfg(not(windows))]
        Probe::host().collect()
    }

    /// A plain-text report, used by `feline --info` and the GUI's copy button.
    pub fn report(&self) -> String {
        let unknown = "Unknown";
        let mut out = String::new();
        // `writeln!` into a String can't fail, so ignoring the Result is fine.
        let mut line = |label: &str, value: &dyn fmt::Display| {
            let _ = writeln!(out, "{label:<16}{value}");
        };
        line("OS", &self.os.as_deref().unwrap_or(unknown));
        // On Windows, `kernel` holds the Windows build number.
        line(
            if cfg!(windows) { "Build" } else { "Kernel" },
            &self.kernel.as_deref().unwrap_or(unknown),
        );
        let cpu = match &self.cpu {
            Some(cpu) => format!("{cpu} ({} threads)", self.cpu_threads),
            None => unknown.to_owned(),
        };
        line("CPU", &cpu);
        let memory = self
            .memory_total_kib
            .map(|kib| format!("{:.1} GiB", kib as f64 / (1024.0 * 1024.0)))
            .unwrap_or_else(|| unknown.to_owned());
        line("Memory", &memory);
        if self.gpus.is_empty() {
            line("GPU", &"none found");
        }
        for gpu in &self.gpus {
            let primary = if self.gpus.len() > 1 && gpu.boot_vga {
                " [primary]"
            } else {
                ""
            };
            line("GPU", &format!("{gpu}{primary}"));
        }
        if cfg!(windows) {
            // No Flatpak, display server or init system to speak of.
            return out;
        }
        if let Some(version) = &self.nvidia_driver {
            line("NVIDIA driver", version);
        }
        line("Flatpak NVIDIA", &self.flatpak_nvidia);
        line("Session", &self.session);
        line("Desktop", &self.desktop.as_deref().unwrap_or(unknown));
        line("Init system", &self.init);
        out
    }
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

/// Reads the same facts on Windows: the version from the registry (the
/// same values `winver` shows), the CPU's name, total memory, and the display
/// adapters from the registry's device class for them.
#[cfg(windows)]
pub mod windows_info {
    use super::{FlatpakNvidia, Gpu, GpuVendor, InitSystem, SessionType, SystemInfo};
    use feline_win::{machine_number, machine_string, total_memory};

    const VERSION_KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    const CPU_KEY: &str = r"HARDWARE\DESCRIPTION\System\CentralProcessor\0";
    /// Windows' device class for display adapters.
    const DISPLAY_CLASS: &str =
        r"SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}";

    pub fn collect() -> SystemInfo {
        let build = machine_string(VERSION_KEY, "CurrentBuild");
        let os = machine_string(VERSION_KEY, "ProductName").map(|name| {
            let name = windows_name(&name, build.as_deref());
            match machine_string(VERSION_KEY, "DisplayVersion") {
                Some(release) => format!("{name} {release}"),
                None => name,
            }
        });
        let kernel = build.map(|b| match machine_number(VERSION_KEY, "UBR") {
            Some(revision) => format!("{b}.{revision}"),
            None => b,
        });
        SystemInfo {
            os,
            kernel,
            cpu: machine_string(CPU_KEY, "ProcessorNameString"),
            cpu_threads: std::thread::available_parallelism().map_or(1, |n| n.get()),
            memory_total_kib: total_memory().map(|bytes| bytes / 1024),
            gpus: gpus(),
            nvidia_driver: None,
            flatpak_nvidia: FlatpakNvidia::NotApplicable,
            session: SessionType::Unknown,
            desktop: None,
            init: InitSystem::Other("Windows".to_owned()),
        }
    }

    /// Windows 11 still calls itself "Windows 10" in `ProductName`; its build
    /// numbers (22000 and up) tell them apart.
    pub fn windows_name(product: &str, build: Option<&str>) -> String {
        let is_11 = build
            .and_then(|b| b.parse::<u32>().ok())
            .is_some_and(|b| b >= 22000);
        if is_11 {
            product.replacen("Windows 10", "Windows 11", 1)
        } else {
            product.to_owned()
        }
    }

    /// The display adapters Windows has drivers for (`0000`, `0001`, …).
    fn gpus() -> Vec<Gpu> {
        (0..16)
            .map(|n| format!(r"{DISPLAY_CLASS}\{n:04}"))
            .filter_map(|key| {
                let name = machine_string(&key, "DriverDesc")?;
                // e.g. PCI\VEN_8086&DEV_46B3&SUBSYS_13E31043
                let ids = machine_string(&key, "MatchingDeviceId").unwrap_or_default();
                let vendor_id = pci_id(&ids, "VEN_").unwrap_or(0);
                Some(Gpu {
                    vendor: GpuVendor::from_pci_id(vendor_id),
                    vendor_id,
                    device_id: pci_id(&ids, "DEV_").unwrap_or(0),
                    name: Some(name),
                    driver: machine_string(&key, "DriverVersion"),
                    boot_vga: false,
                })
            })
            .collect()
    }

    /// The four hex digits after `tag` in a device ID such as `PCI\VEN_8086&…`.
    pub fn pci_id(ids: &str, tag: &str) -> Option<u16> {
        let upper = ids.to_ascii_uppercase();
        let start = upper.find(tag)? + tag.len();
        u16::from_str_radix(upper.get(start..start + 4)?, 16).ok()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn windows_11_is_named_by_its_build() {
            assert_eq!(windows_name("Windows 10 Home", Some("26200")), "Windows 11 Home");
            assert_eq!(windows_name("Windows 10 Pro", Some("19045")), "Windows 10 Pro");
            assert_eq!(windows_name("Windows 10 Pro", None), "Windows 10 Pro");
        }

        #[test]
        fn pci_ids_from_device_ids() {
            let ids = r"PCI\VEN_8086&DEV_46B3&SUBSYS_13E31043";
            assert_eq!(pci_id(ids, "VEN_"), Some(0x8086));
            assert_eq!(pci_id(ids, "DEV_"), Some(0x46b3));
            assert_eq!(pci_id("ROOT\\BasicDisplay", "VEN_"), None);
        }

        #[test]
        fn this_computer_is_described() {
            let info = collect();
            assert!(info.os.is_some_and(|os| os.starts_with("Windows")));
            assert!(info.memory_total_kib.is_some());
        }
    }
}

// ---------------------------------------------------------------------------
// Parsers: pure functions, easy to unit-test
// ---------------------------------------------------------------------------

/// Returns `PRETTY_NAME` (or `NAME` as fallback) from an os-release file.
pub fn parse_os_release(contents: &str) -> Option<String> {
    let value = |key: &str| {
        contents.lines().find_map(|line| {
            let v = line.strip_prefix(key)?.strip_prefix('=')?;
            let v = v.trim().trim_matches('"').trim_matches('\'');
            (!v.is_empty()).then(|| v.to_owned())
        })
    };
    value("PRETTY_NAME").or_else(|| value("NAME"))
}

/// Returns the CPU model name and the number of logical CPUs.
pub fn parse_cpuinfo(contents: &str) -> (Option<String>, usize) {
    let field = |line: &str, key: &str| {
        let (k, v) = line.split_once(':')?;
        (k.trim() == key).then(|| v.trim().to_owned())
    };
    let model = contents.lines().find_map(|l| field(l, "model name"));
    let threads = contents
        .lines()
        .filter(|l| field(l, "processor").is_some())
        .count();
    (model, threads)
}

/// Returns `MemTotal` in KiB.
pub fn parse_meminfo_total_kib(contents: &str) -> Option<u64> {
    contents
        .lines()
        .find_map(|l| l.strip_prefix("MemTotal:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|n| n.parse().ok())
}

/// Extracts the version from `/proc/driver/nvidia/version`, e.g.
/// `NVRM version: NVIDIA UNIX Open Kernel Module for x86_64  595.104.02  Release Build ...`
/// gives "595.104.02". The first token shaped like `digits.digits[.digits]` wins.
pub fn parse_nvidia_version(contents: &str) -> Option<String> {
    let first_line = contents.lines().next()?;
    first_line
        .split_whitespace()
        .find(|token| {
            let parts: Vec<&str> = token.split('.').collect();
            parts.len() >= 2
                && parts
                    .iter()
                    .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
        })
        .map(str::to_owned)
}

/// Looks up a device's name in the `pci.ids` database format:
///
/// ```text
/// 10de  NVIDIA Corporation
/// \t2582  GA107 [GeForce RTX 3050 8GB]
/// ```
///
/// If the name has a `[bracketed]` marketing name, that part is returned
/// ("GeForce RTX 3050 8GB"), since it's what users recognise.
pub fn lookup_pci_device_name(db: &str, vendor: u16, device: u16) -> Option<String> {
    let vendor_hex = format!("{vendor:04x}");
    let device_hex = format!("{device:04x}");
    let mut in_vendor = false;
    for line in db.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if !line.starts_with('\t') {
            // A new vendor line. Once we've passed our vendor, stop.
            if in_vendor {
                return None;
            }
            in_vendor = line.starts_with(&vendor_hex);
            continue;
        }
        // Device lines have exactly one tab; subsystem lines have two.
        if in_vendor && !line.starts_with("\t\t") {
            let rest = line.trim_start_matches('\t');
            if let Some(name) = rest.strip_prefix(&device_hex) {
                let name = name.trim();
                let marketing = name
                    .split_once('[')
                    .and_then(|(_, r)| r.split_once(']'))
                    .map(|(inner, _)| inner.trim());
                return Some(marketing.unwrap_or(name).to_owned());
            }
        }
    }
    None
}

/// Compares the host driver ("595.104.02") with installed extension versions
/// ("595-104-02", Flatpak's dash form).
pub fn check_flatpak_nvidia(host: &str, installed: &[String]) -> FlatpakNvidia {
    let wanted = host.replace('.', "-");
    if installed.contains(&wanted) {
        FlatpakNvidia::Match { version: wanted }
    } else if installed.is_empty() {
        FlatpakNvidia::Missing {
            host: host.to_owned(),
        }
    } else {
        FlatpakNvidia::Mismatch {
            host: host.to_owned(),
            installed: installed.to_vec(),
        }
    }
}

/// Works out the session type, preferring the explicit `XDG_SESSION_TYPE`.
pub fn session_type(env: &SessionEnv) -> SessionType {
    match env.xdg_session_type.as_deref() {
        Some("wayland") => SessionType::Wayland,
        Some("x11") => SessionType::X11,
        Some("tty") => SessionType::Tty,
        _ if env.wayland_display.is_some() => SessionType::Wayland,
        _ if env.display.is_some() => SessionType::X11,
        _ => SessionType::Unknown,
    }
}

// ---------------------------------------------------------------------------
// Quick checks: plain-language findings derived from a SystemInfo
// ---------------------------------------------------------------------------

/// How serious a [`Check`] result is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckLevel {
    Ok,
    /// Likely to cause problems, but may still work.
    Warning,
    /// Will stop the game from working.
    Problem,
}

/// One finding about the system, shown on the Diagnostics page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub level: CheckLevel,
    pub title: String,
    /// What it means and what to do about it. Empty when there's nothing to add.
    pub detail: String,
}

impl Check {
    pub(crate) fn new(level: CheckLevel, title: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            level,
            title: title.into(),
            detail: detail.into(),
        }
    }
}

impl SystemInfo {
    /// Checks that need nothing but the information already collected. They
    /// only report facts Feline can actually see; anything deeper is the
    /// runtime's own self-check.
    pub fn checks(&self) -> Vec<Check> {
        let mut checks = Vec::new();

        if cfg!(windows) {
            checks.push(match self.gpus.first() {
                None => Check::new(
                    CheckLevel::Warning,
                    "No graphics card found",
                    "Windows lists no display adapter Feline can read. Roblox needs a \
                     GPU with DirectX 10 or newer.",
                ),
                Some(gpu) => Check::new(CheckLevel::Ok, format!("GPU: {gpu}"), ""),
            });
            return checks;
        }

        checks.push(match self.session {
            SessionType::Wayland | SessionType::X11 => Check::new(
                CheckLevel::Ok,
                format!("Graphical session: {}", self.session),
                "",
            ),
            SessionType::Tty => Check::new(
                CheckLevel::Problem,
                "No graphical session",
                "Roblox needs a Wayland or X11 desktop session to open its window.",
            ),
            SessionType::Unknown => Check::new(
                CheckLevel::Warning,
                "Session type unknown",
                "Neither XDG_SESSION_TYPE, WAYLAND_DISPLAY nor DISPLAY is set, so Feline \
                 can't tell whether a desktop session is running.",
            ),
        });

        // With several GPUs, the boot GPU is usually the one driving the screen.
        let primary = self
            .gpus
            .iter()
            .find(|g| g.boot_vga)
            .or_else(|| self.gpus.first());
        checks.push(match primary {
            None => Check::new(
                CheckLevel::Warning,
                "No GPU found",
                "Feline found no graphics card under /sys/class/drm. Roblox needs a \
                 GPU with Vulkan or OpenGL ES support.",
            ),
            Some(gpu) if gpu.driver.is_none() => Check::new(
                CheckLevel::Warning,
                format!("GPU without a driver: {gpu}"),
                "No kernel driver is bound to this GPU, so it can't be used for 3D.",
            ),
            Some(gpu) => Check::new(CheckLevel::Ok, format!("GPU: {gpu}"), ""),
        });

        match &self.flatpak_nvidia {
            FlatpakNvidia::NotApplicable => {}
            FlatpakNvidia::Match { version } => checks.push(Check::new(
                CheckLevel::Ok,
                "Flatpak apps can use the NVIDIA driver",
                format!("GL.nvidia-{version} matches the host driver."),
            )),
            FlatpakNvidia::Missing { host } => checks.push(Check::new(
                CheckLevel::Warning,
                "Flatpak NVIDIA driver missing",
                format!(
                    "No org.freedesktop.Platform.GL.nvidia extension is installed for driver \
                     {host}. Runtimes installed as a Flatpak will fall back to software \
                     rendering. Run `flatpak update` to install it."
                ),
            )),
            FlatpakNvidia::Mismatch { host, installed } => checks.push(Check::new(
                CheckLevel::Warning,
                "Flatpak NVIDIA driver doesn't match",
                format!(
                    "The host driver is {host}, but the installed extensions are for {}. \
                     Runtimes installed as a Flatpak will fall back to software rendering \
                     until `flatpak update` installs the matching one (Flathub can lag a \
                     day behind a driver update).",
                    installed.join(", ")
                ),
            )),
        }

        checks
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Writes a file in a fake tree, creating parent directories.
    fn write_file(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().expect("path has a parent")).expect("create dirs");
        fs::write(path, contents).expect("write file");
    }

    #[test]
    fn os_release_prefers_pretty_name_and_strips_quotes() {
        let text = "NAME=\"Void\"\nID=\"void\"\nPRETTY_NAME=\"Void Linux\"\n";
        assert_eq!(parse_os_release(text).as_deref(), Some("Void Linux"));
        assert_eq!(parse_os_release("NAME=Gentoo\n").as_deref(), Some("Gentoo"));
        assert_eq!(parse_os_release("ID=x\n"), None);
    }

    #[test]
    fn cpuinfo_counts_threads() {
        let text = "processor\t: 0\nmodel name\t: AMD Ryzen 7 5700X 8-Core Processor\n\n\
                    processor\t: 1\nmodel name\t: AMD Ryzen 7 5700X 8-Core Processor\n";
        let (model, threads) = parse_cpuinfo(text);
        assert_eq!(model.as_deref(), Some("AMD Ryzen 7 5700X 8-Core Processor"));
        assert_eq!(threads, 2);
    }

    #[test]
    fn meminfo_total() {
        let text = "MemTotal:       16298012 kB\nMemFree:  1 kB\n";
        assert_eq!(parse_meminfo_total_kib(text), Some(16_298_012));
        assert_eq!(parse_meminfo_total_kib("MemFree: 1 kB\n"), None);
    }

    #[test]
    fn nvidia_version_open_and_proprietary_formats() {
        let open = "NVRM version: NVIDIA UNIX Open Kernel Module for x86_64  595.104.02  Release Build  (dvs-builder@U22)  Thu Sep 17 2026\nGCC version: 14.2.1\n";
        assert_eq!(parse_nvidia_version(open).as_deref(), Some("595.104.02"));
        let proprietary =
            "NVRM version: NVIDIA UNIX x86_64 Kernel Module  550.54.14  Thu Feb 22 2024\n";
        assert_eq!(
            parse_nvidia_version(proprietary).as_deref(),
            Some("550.54.14")
        );
        assert_eq!(parse_nvidia_version("garbage"), None);
    }

    const PCI_IDS: &str = "# comment\n\
        1002  Advanced Micro Devices, Inc. [AMD/ATI]\n\
        \t73bf  Navi 21 [Radeon RX 6800/6800 XT / 6900 XT]\n\
        10de  NVIDIA Corporation\n\
        \t2582  GA107 [GeForce RTX 3050 8GB]\n\
        \t\t1043 8899  some subsystem\n\
        \t2583  NoBrackets Device\n\
        8086  Intel Corporation\n\
        \t2582  Not this one\n";

    #[test]
    fn pci_name_lookup() {
        assert_eq!(
            lookup_pci_device_name(PCI_IDS, 0x10de, 0x2582).as_deref(),
            Some("GeForce RTX 3050 8GB")
        );
        assert_eq!(
            lookup_pci_device_name(PCI_IDS, 0x10de, 0x2583).as_deref(),
            Some("NoBrackets Device")
        );
        assert_eq!(lookup_pci_device_name(PCI_IDS, 0x10de, 0xffff), None);
    }

    #[test]
    fn flatpak_nvidia_states() {
        let installed = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            check_flatpak_nvidia("595.104.02", &installed(&["580-1-1", "595-104-02"])),
            FlatpakNvidia::Match {
                version: "595-104-02".into()
            }
        );
        assert!(matches!(
            check_flatpak_nvidia("595.104.02", &[]),
            FlatpakNvidia::Missing { .. }
        ));
        assert!(matches!(
            check_flatpak_nvidia("595.104.02", &installed(&["580-1-1"])),
            FlatpakNvidia::Mismatch { .. }
        ));
    }

    #[test]
    fn session_type_detection() {
        let env = |t: Option<&str>, w: Option<&str>, d: Option<&str>| SessionEnv {
            xdg_session_type: t.map(Into::into),
            wayland_display: w.map(Into::into),
            display: d.map(Into::into),
            xdg_current_desktop: None,
        };
        assert_eq!(
            session_type(&env(Some("wayland"), None, None)),
            SessionType::Wayland
        );
        assert_eq!(session_type(&env(None, None, Some(":0"))), SessionType::X11);
        assert_eq!(
            session_type(&env(None, Some("wayland-0"), Some(":0"))),
            SessionType::Wayland
        );
        assert_eq!(session_type(&env(None, None, None)), SessionType::Unknown);
    }

    #[cfg(not(windows))]
    fn bare_info() -> SystemInfo {
        SystemInfo {
            os: None,
            kernel: None,
            cpu: None,
            cpu_threads: 0,
            memory_total_kib: None,
            gpus: Vec::new(),
            nvidia_driver: None,
            flatpak_nvidia: FlatpakNvidia::NotApplicable,
            session: SessionType::Wayland,
            desktop: None,
            init: InitSystem::Unknown,
        }
    }

    #[cfg(not(windows))]
    fn levels(info: &SystemInfo) -> Vec<CheckLevel> {
        info.checks().iter().map(|c| c.level).collect()
    }

    #[cfg(not(windows))]
    #[test]
    fn checks_report_session_gpu_and_flatpak_nvidia() {
        let mut info = bare_info();
        // Wayland is fine, no GPU is a warning, and no NVIDIA means no NVIDIA check.
        assert_eq!(levels(&info), [CheckLevel::Ok, CheckLevel::Warning]);

        info.session = SessionType::Tty;
        info.gpus.push(Gpu {
            vendor: GpuVendor::Nvidia,
            vendor_id: 0x10de,
            device_id: 0x2582,
            name: None,
            driver: Some("nvidia".into()),
            boot_vga: true,
        });
        info.flatpak_nvidia = FlatpakNvidia::Mismatch {
            host: "595.104.02".into(),
            installed: vec!["580-1-1".into()],
        };
        assert_eq!(
            levels(&info),
            [CheckLevel::Problem, CheckLevel::Ok, CheckLevel::Warning]
        );

        info.gpus[0].driver = None;
        info.flatpak_nvidia = FlatpakNvidia::Match {
            version: "595-104-02".into(),
        };
        assert_eq!(
            levels(&info),
            [CheckLevel::Problem, CheckLevel::Warning, CheckLevel::Ok]
        );
    }

    /// Builds a fake filesystem tree and checks the whole probe end to end.
    #[test]
    fn probe_reads_fake_tree() {
        let root = std::env::temp_dir().join(format!("feline-test-probe-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let home = root.join("home/user");

        write_file(&root.join("etc/os-release"), "PRETTY_NAME=\"Test Linux\"\n");
        write_file(&root.join("proc/sys/kernel/osrelease"), "6.1.0\n");
        write_file(&root.join("proc/meminfo"), "MemTotal: 2097152 kB\n");
        write_file(&root.join("proc/1/comm"), "runit\n");
        write_file(
            &root.join("proc/driver/nvidia/version"),
            "NVRM version: NVIDIA UNIX Open Kernel Module for x86_64  595.104.02  Release\n",
        );
        write_file(&root.join("usr/share/hwdata/pci.ids"), PCI_IDS);
        let card = root.join("sys/class/drm/card0/device");
        write_file(&card.join("vendor"), "0x10de\n");
        write_file(&card.join("device"), "0x2582\n");
        write_file(&card.join("boot_vga"), "1\n");
        // A connector entry that must be ignored.
        write_file(&root.join("sys/class/drm/card0-DP-1/status"), "connected\n");
        fs::create_dir_all(
            home.join(".local/share/flatpak/runtime/org.freedesktop.Platform.GL.nvidia-595-104-02"),
        )
        .unwrap();

        let env = SessionEnv {
            xdg_session_type: Some("wayland".into()),
            xdg_current_desktop: Some("KDE".into()),
            ..SessionEnv::default()
        };
        let info = Probe::with_root(&root, Some(home), env).collect();
        let _ = fs::remove_dir_all(&root);

        assert_eq!(info.os.as_deref(), Some("Test Linux"));
        assert_eq!(info.kernel.as_deref(), Some("6.1.0"));
        assert_eq!(info.memory_total_kib, Some(2_097_152));
        assert_eq!(info.init, InitSystem::Runit);
        assert_eq!(info.gpus.len(), 1);
        assert_eq!(info.gpus[0].vendor, GpuVendor::Nvidia);
        assert_eq!(info.gpus[0].name.as_deref(), Some("GeForce RTX 3050 8GB"));
        assert!(info.gpus[0].boot_vga);
        assert!(matches!(info.flatpak_nvidia, FlatpakNvidia::Match { .. }));
        assert_eq!(info.session, SessionType::Wayland);
        assert!(info.report().contains("NVIDIA GeForce RTX 3050 8GB"));
    }
}

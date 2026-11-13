// SPDX-License-Identifier: GPL-3.0-or-later
//! The System page's extras:
//!
//! - **PC check** ([`pc_check`]): can this computer run Roblox well? Memory,
//!   processor, graphics, free space and the system version, in plain words.
//! - **Driver check** ([`drivers`]): each graphics card's driver, how old it
//!   is (Windows says when it was made), and where to get a newer one.
//! - **Live usage** ([`UsageMeter`]): the whole computer's CPU and memory
//!   use, and Roblox's share of it, once a second for the graph.
//!
//! Temperatures aren't here: reading them needs a driver Feline won't
//! install.

use std::path::{Path, PathBuf};

use crate::system::{Check, CheckLevel, GpuVendor, SystemInfo};

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

// ---------------------------------------------------------------------------
// PC check
// ---------------------------------------------------------------------------

/// The overall answer of the PC check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub level: CheckLevel,
    pub title: String,
    pub detail: String,
}

/// Where Roblox lives on this computer (for its drive's free space).
pub fn roblox_home() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        crate::paths::data_home()
    }
}

/// `(free, total)` bytes on the drive holding `path`.
pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
    #[cfg(windows)]
    return feline_win::stats::disk_space(path);
    #[cfg(not(windows))]
    {
        // `df` is in every distribution; statvfs would need `unsafe` code.
        let out = std::process::Command::new("df")
            .args(["-Pk", "--"])
            .arg(path)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        parse_df(&String::from_utf8_lossy(&out.stdout))
    }
}

/// `df -Pk`'s second line: filesystem, 1K-blocks, used, available, …
pub fn parse_df(text: &str) -> Option<(u64, u64)> {
    let fields: Vec<&str> = text.lines().nth(1)?.split_whitespace().collect();
    let total: u64 = fields.get(1)?.parse().ok()?;
    let free: u64 = fields.get(3)?.parse().ok()?;
    Some((free * 1024, total * 1024))
}

/// Windows' build number, from the "Build" field (`26200.7171`).
fn windows_build(info: &SystemInfo) -> Option<u32> {
    info.kernel.as_deref()?.split('.').next()?.parse().ok()
}

fn is_basic_display(name: &str) -> bool {
    name.contains("Microsoft Basic Display") || name.contains("Microsoft Basic Render")
}

/// Can this PC run Roblox well? `free_disk` is the free space on Roblox's
/// drive, if known.
pub fn pc_check(info: &SystemInfo, free_disk: Option<u64>) -> (Verdict, Vec<Check>) {
    let mut checks = Vec::new();

    if cfg!(windows) {
        checks.push(match windows_build(info) {
            Some(build) if build < 10240 => Check::new(
                CheckLevel::Problem,
                "Windows is too old",
                "Roblox needs Windows 10 or newer.",
            ),
            _ => Check::new(
                CheckLevel::Ok,
                info.os.clone().unwrap_or_else(|| "Windows".to_owned()),
                "Roblox supports this Windows.",
            ),
        });
    }

    if let Some(kib) = info.memory_total_kib {
        let gib = kib as f64 * 1024.0 / GIB;
        checks.push(if gib < 3.5 {
            Check::new(
                CheckLevel::Warning,
                format!("{gib:.0} GB of memory"),
                "Roblox starts, but busy games may stutter or close. Close other apps (browsers above all) while playing.",
            )
        } else if gib < 7.5 {
            Check::new(
                CheckLevel::Ok,
                format!("{gib:.0} GB of memory"),
                "Enough for most games. Very big games like 8 GB or more.",
            )
        } else {
            Check::new(CheckLevel::Ok, format!("{gib:.0} GB of memory"), "Plenty for Roblox.")
        });
    }

    checks.push(if info.cpu_threads < 4 {
        Check::new(
            CheckLevel::Warning,
            format!("Processor with {} threads", info.cpu_threads),
            "Roblox runs, but crowded servers can feel slow. Lower Graphics quality helps a little.",
        )
    } else {
        Check::new(
            CheckLevel::Ok,
            info.cpu.clone().unwrap_or_else(|| "Processor".to_owned()),
            format!("{} threads: fine for Roblox.", info.cpu_threads),
        )
    });

    let named = |g: &crate::system::Gpu| g.name.clone().unwrap_or_else(|| g.to_string());
    let real: Vec<_> = info
        .gpus
        .iter()
        .filter(|g| !g.name.as_deref().is_some_and(is_basic_display))
        .collect();
    checks.push(if info.gpus.is_empty() {
        Check::new(
            CheckLevel::Problem,
            "No graphics card found",
            "Roblox needs a graphics card with DirectX 10 / Vulkan or newer.",
        )
    } else if real.is_empty() {
        Check::new(
            CheckLevel::Problem,
            "No graphics driver",
            "Windows is using its basic display driver, which can't run games. Install the driver from your graphics card's maker (see Graphics drivers below).",
        )
    } else if let Some(gpu) = real
        .iter()
        .find(|g| matches!(g.vendor, GpuVendor::Nvidia | GpuVendor::Amd))
    {
        Check::new(CheckLevel::Ok, named(gpu), "A gaming graphics card: Roblox runs well.")
    } else {
        Check::new(
            CheckLevel::Ok,
            named(real[0]),
            "Built-in graphics: fine for most games. In busy ones, a lower Graphics quality keeps it smooth.",
        )
    });

    if let Some(free) = free_disk {
        let gib = free as f64 / GIB;
        checks.push(if gib < 2.0 {
            Check::new(
                CheckLevel::Problem,
                format!("Only {gib:.1} GB free"),
                "Roblox needs room to update and cache games. Free some space (Settings → Storage helps).",
            )
        } else if gib < 10.0 {
            Check::new(
                CheckLevel::Warning,
                format!("{gib:.1} GB free"),
                "Enough for now, but updates and the game cache grow. Settings → Storage can tidy up.",
            )
        } else {
            Check::new(CheckLevel::Ok, format!("{gib:.0} GB free"), "Plenty of room for Roblox.")
        });
    }

    let problems = checks.iter().filter(|c| c.level == CheckLevel::Problem).count();
    let warnings = checks.iter().filter(|c| c.level == CheckLevel::Warning).count();
    let verdict = if problems > 0 {
        Verdict {
            level: CheckLevel::Problem,
            title: "Roblox may struggle here".to_owned(),
            detail: "Something below stops Roblox from running well. Each line says what to do.".to_owned(),
        }
    } else if warnings > 0 {
        Verdict {
            level: CheckLevel::Warning,
            title: "Good enough".to_owned(),
            detail: "Most games run fine. Busy ones may need lower graphics.".to_owned(),
        }
    } else {
        Verdict {
            level: CheckLevel::Ok,
            title: "Purr-fect".to_owned(),
            detail: "This PC runs Roblox well.".to_owned(),
        }
    };
    (verdict, checks)
}

// ---------------------------------------------------------------------------
// Driver check
// ---------------------------------------------------------------------------

/// One graphics card's driver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Driver {
    pub gpu: String,
    /// The version as its maker writes it (NVIDIA's "560.94"), else as
    /// Windows or Linux reports it.
    pub version: String,
    /// "March 2024", if known.
    pub date: Option<String>,
    pub level: CheckLevel,
    pub advice: String,
    /// The maker's driver download page.
    pub link: Option<&'static str>,
}

/// NVIDIA's own version from Windows' driver version: the last five
/// digits, "32.0.15.6094" → "560.94".
pub fn nvidia_version(windows: &str) -> Option<String> {
    let digits: String = windows.chars().filter(char::is_ascii_digit).collect();
    let tail = digits.get(digits.len().checked_sub(5)?..)?;
    let major = tail[..3].trim_start_matches('0');
    Some(format!("{}.{}", if major.is_empty() { "0" } else { major }, &tail[3..]))
}

/// Windows' driver date, "3-26-2024" (month-day-year) → (year, month, day).
pub fn parse_driver_date(text: &str) -> Option<(i64, i64, i64)> {
    let mut parts = text.trim().split('-').map(|p| p.parse::<i64>().ok());
    let (month, day, year) = (parts.next()??, parts.next()??, parts.next()??);
    ((1..=12).contains(&month) && (1..=31).contains(&day) && year > 1990).then_some((year, month, day))
}

#[cfg_attr(not(windows), allow(dead_code))]
const MONTHS: [&str; 12] = [
    "January", "February", "March", "April", "May", "June", "July", "August", "September",
    "October", "November", "December",
];

#[cfg_attr(not(windows), allow(dead_code))]
fn vendor_link(vendor: GpuVendor) -> Option<&'static str> {
    match vendor {
        GpuVendor::Nvidia => Some("https://www.nvidia.com/en-us/drivers/"),
        GpuVendor::Amd => Some("https://www.amd.com/en/support/download/drivers.html"),
        GpuVendor::Intel => Some("https://www.intel.com/content/www/us/en/support/detect.html"),
        GpuVendor::Other(_) => None,
    }
}

/// How old a driver from `date` is on day `today` (days since 1970), as a
/// level and advice.
pub fn judge_age(date: (i64, i64, i64), today: i64) -> (CheckLevel, String) {
    let days = today - crate::presence::log::days_from_civil(date.0, date.1, date.2);
    let months = (days / 30).max(0);
    if days > 730 {
        (
            CheckLevel::Warning,
            format!("About {} years old. A newer driver often fixes crashes and adds speed in games.", days / 365),
        )
    } else if days > 365 {
        (
            CheckLevel::Warning,
            "Over a year old. Worth updating if games crash or stutter.".to_owned(),
        )
    } else if months <= 1 {
        (CheckLevel::Ok, "Fresh. Nothing to do.".to_owned())
    } else {
        (CheckLevel::Ok, format!("{months} months old. Up to date enough."))
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
fn today() -> i64 {
    (crate::history::now() as i64 + crate::history::local_offset_seconds()) / 86_400
}

/// Each graphics card's driver, with advice.
pub fn drivers(info: &SystemInfo) -> Vec<Driver> {
    #[cfg(windows)]
    {
        let _ = info;
        windows_drivers()
    }
    #[cfg(not(windows))]
    {
        info.gpus
            .iter()
            .map(|gpu| {
                let name = gpu.name.clone().unwrap_or_else(|| gpu.to_string());
                match (&gpu.driver, gpu.vendor) {
                    (None, _) => Driver {
                        gpu: name,
                        version: "no driver".to_owned(),
                        date: None,
                        level: CheckLevel::Warning,
                        advice: "No kernel driver is bound to this card, so games can't use it.".to_owned(),
                        link: None,
                    },
                    (Some(_), GpuVendor::Nvidia) if info.nvidia_driver.is_some() => Driver {
                        gpu: name,
                        version: format!("NVIDIA {}", info.nvidia_driver.clone().unwrap_or_default()),
                        date: None,
                        level: CheckLevel::Ok,
                        advice: "Updates come through your distribution's packages.".to_owned(),
                        link: None,
                    },
                    (Some(driver), _) => Driver {
                        gpu: name,
                        version: driver.clone(),
                        date: None,
                        level: CheckLevel::Ok,
                        advice: "Part of your system (Mesa / the kernel): updates come with system updates.".to_owned(),
                        link: None,
                    },
                }
            })
            .collect()
    }
}

#[cfg(windows)]
fn windows_drivers() -> Vec<Driver> {
    use feline_win::machine_string;
    const DISPLAY_CLASS: &str =
        r"SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}";
    let today = today();
    (0..16)
        .filter_map(|n| {
            let key = format!(r"{DISPLAY_CLASS}\{n:04}");
            let name = machine_string(&key, "DriverDesc")?;
            let ids = machine_string(&key, "MatchingDeviceId").unwrap_or_default();
            let vendor = GpuVendor::from_pci_id(
                crate::system::windows_info::pci_id(&ids, "VEN_").unwrap_or(0),
            );
            let raw = machine_string(&key, "DriverVersion").unwrap_or_default();
            let date = machine_string(&key, "DriverDate").and_then(|d| parse_driver_date(&d));
            if is_basic_display(&name) {
                return Some(Driver {
                    gpu: name,
                    version: raw,
                    date: None,
                    level: CheckLevel::Problem,
                    advice: "Windows' basic fallback, not a real driver: games can't run well. Install the driver for your graphics card.".to_owned(),
                    link: None,
                });
            }
            let version = match vendor {
                GpuVendor::Nvidia => nvidia_version(&raw).map_or(raw.clone(), |v| format!("{v} ({raw})")),
                _ => raw,
            };
            let (level, advice) = match date {
                Some(date) => judge_age(date, today),
                None => (CheckLevel::Ok, String::new()),
            };
            Some(Driver {
                gpu: name,
                version,
                date: date.map(|(y, m, _)| format!("{} {y}", MONTHS[(m - 1) as usize])),
                level,
                advice,
                link: vendor_link(vendor),
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Live usage
// ---------------------------------------------------------------------------

/// One reading for the live graph.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Usage {
    /// The whole computer, 0–100.
    pub cpu: f32,
    pub memory_used: u64,
    pub memory_total: u64,
    /// Roblox's share, 0–100 of the whole computer.
    pub roblox_cpu: f32,
    pub roblox_memory: u64,
    /// Whether Roblox is running.
    pub roblox: bool,
}

/// Measures the computer and Roblox. The first [`UsageMeter::sample`] only
/// sets the starting point for CPU.
pub struct UsageMeter {
    last_cpu: Option<(u64, u64)>,
    last_roblox: std::collections::HashMap<u32, u64>,
    last_at: Option<std::time::Instant>,
    threads: f64,
}

impl Default for UsageMeter {
    fn default() -> Self {
        Self::new()
    }
}

/// Roblox's program names, for the live graph.
#[cfg(not(windows))]
const ROBLOX_NAMES: &[&str] = &["feline-runtime", "sober", "RobloxPlayerBet"];

impl UsageMeter {
    pub fn new() -> Self {
        Self {
            last_cpu: None,
            last_roblox: Default::default(),
            last_at: None,
            threads: std::thread::available_parallelism().map_or(1, |n| n.get()) as f64,
        }
    }

    /// `(idle, total)` CPU time of the whole computer.
    fn cpu_times() -> Option<(u64, u64)> {
        #[cfg(windows)]
        return feline_win::stats::cpu_times();
        #[cfg(not(windows))]
        parse_proc_stat(&std::fs::read_to_string("/proc/stat").ok()?)
    }

    /// `(total, available)` memory.
    fn memory() -> Option<(u64, u64)> {
        #[cfg(windows)]
        return feline_win::stats::memory();
        #[cfg(not(windows))]
        {
            let text = std::fs::read_to_string("/proc/meminfo").ok()?;
            let total = crate::monitor::parse_kib_field(&text, "MemTotal:")?;
            let available = crate::monitor::parse_kib_field(&text, "MemAvailable:")?;
            // Already in bytes.
            Some((total, available))
        }
    }

    /// Roblox's processes: (pid, CPU time in the OS's units, memory bytes).
    fn roblox_processes() -> Vec<(u32, u64, u64)> {
        #[cfg(windows)]
        {
            feline_win::processes()
                .into_iter()
                .filter(|p| p.exe_name.eq_ignore_ascii_case("RobloxPlayerBeta.exe"))
                .filter_map(|p| feline_win::game::usage(p.pid).map(|u| (p.pid, u.cpu_time, u.memory)))
                .collect()
        }
        #[cfg(not(windows))]
        {
            let Ok(entries) = std::fs::read_dir("/proc") else {
                return Vec::new();
            };
            entries
                .filter_map(Result::ok)
                .filter_map(|e| {
                    let pid: u32 = e.file_name().to_str()?.parse().ok()?;
                    let comm = std::fs::read_to_string(e.path().join("comm")).ok()?;
                    if !ROBLOX_NAMES.contains(&comm.trim()) {
                        return None;
                    }
                    let stat = crate::monitor::parse_stat(&std::fs::read_to_string(e.path().join("stat")).ok()?)?;
                    let rss = std::fs::read_to_string(e.path().join("status"))
                        .ok()
                        .and_then(|s| crate::monitor::parse_kib_field(&s, "VmRSS:"))
                        .unwrap_or(0);
                    Some((pid, stat.cpu_ticks, rss * 1024))
                })
                .collect()
        }
    }

    pub fn sample(&mut self) -> Option<Usage> {
        const TICKS_PER_SECOND: f64 = if cfg!(windows) { 10_000_000.0 } else { 100.0 };
        let now = std::time::Instant::now();
        let times = Self::cpu_times();
        let (memory_total, available) = Self::memory().unwrap_or((0, 0));
        let roblox = Self::roblox_processes();

        let cpu = match (self.last_cpu, times) {
            (Some((idle0, total0)), Some((idle1, total1))) if total1 > total0 => {
                let busy = (total1 - total0).saturating_sub(idle1.saturating_sub(idle0));
                busy as f32 / (total1 - total0) as f32 * 100.0
            }
            _ => -1.0,
        };
        let roblox_cpu = match self.last_at {
            Some(at) => {
                let used: u64 = roblox
                    .iter()
                    .map(|(pid, t, _)| t.saturating_sub(self.last_roblox.get(pid).copied().unwrap_or(*t)))
                    .sum();
                let seconds = now.duration_since(at).as_secs_f64().max(0.001);
                (used as f64 / TICKS_PER_SECOND / seconds / self.threads * 100.0).clamp(0.0, 100.0) as f32
            }
            None => 0.0,
        };
        self.last_cpu = times;
        self.last_at = Some(now);
        self.last_roblox = roblox.iter().map(|(pid, t, _)| (*pid, *t)).collect();
        (cpu >= 0.0).then(|| Usage {
            cpu: cpu.clamp(0.0, 100.0),
            memory_used: memory_total.saturating_sub(available),
            memory_total,
            roblox_cpu,
            roblox_memory: roblox.iter().map(|(_, _, m)| m).sum(),
            roblox: !roblox.is_empty(),
        })
    }
}

/// `/proc/stat`'s first line: `(idle, total)` in clock ticks.
pub fn parse_proc_stat(text: &str) -> Option<(u64, u64)> {
    let line = text.lines().next()?.strip_prefix("cpu ")?;
    let values: Vec<u64> = line.split_whitespace().filter_map(|v| v.parse().ok()).collect();
    // user nice system idle iowait irq softirq steal (guests are inside user).
    let total: u64 = values.iter().take(8).sum();
    let idle = values.get(3)? + values.get(4).copied().unwrap_or(0);
    Some((idle, total))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nvidia_versions_are_shown_like_nvidia_does() {
        assert_eq!(nvidia_version("32.0.15.6094").as_deref(), Some("560.94"));
        assert_eq!(nvidia_version("31.0.15.5222").as_deref(), Some("552.22"));
        assert_eq!(nvidia_version("12"), None);
    }

    #[test]
    fn driver_dates_and_ages() {
        assert_eq!(parse_driver_date("3-26-2024"), Some((2024, 3, 26)));
        assert_eq!(parse_driver_date("13-1-2024"), None);
        assert_eq!(parse_driver_date("junk"), None);
        let day = |y, m, d| crate::presence::log::days_from_civil(y, m, d);
        assert_eq!(judge_age((2024, 3, 26), day(2024, 4, 1)).0, CheckLevel::Ok);
        assert_eq!(judge_age((2023, 1, 1), day(2024, 6, 1)).0, CheckLevel::Warning);
        assert!(judge_age((2020, 1, 1), day(2024, 6, 1)).1.starts_with("About 4 years"));
    }

    #[test]
    fn linux_numbers_parse() {
        let df = "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/sda2 100 40 60 40% /\n";
        assert_eq!(parse_df(df), Some((60 * 1024, 100 * 1024)));
        let stat = "cpu  10 0 5 80 5 0 0 0 0 0\ncpu0 1 2 3\n";
        assert_eq!(parse_proc_stat(stat), Some((85, 100)));
    }

    #[test]
    fn a_weak_pc_gets_honest_advice() {
        let info = SystemInfo {
            os: Some("Windows 11 Home".into()),
            kernel: Some("26200.1".into()),
            cpu: Some("Tiny CPU".into()),
            cpu_threads: 2,
            memory_total_kib: Some(2 * 1024 * 1024),
            gpus: Vec::new(),
            nvidia_driver: None,
            flatpak_nvidia: crate::system::FlatpakNvidia::NotApplicable,
            session: crate::system::SessionType::Unknown,
            desktop: None,
            init: crate::system::InitSystem::Unknown,
        };
        let (verdict, checks) = pc_check(&info, Some(1024 * 1024 * 1024));
        assert_eq!(verdict.level, CheckLevel::Problem);
        assert!(checks.iter().any(|c| c.title == "No graphics card found"));
        assert!(checks.iter().any(|c| c.title.starts_with("Only 1.0 GB")));
    }

    #[cfg(windows)]
    #[test]
    fn this_pc_can_be_measured() {
        let mut meter = UsageMeter::new();
        assert!(meter.sample().is_none());
        std::thread::sleep(std::time::Duration::from_millis(200));
        let usage = meter.sample().unwrap();
        assert!(usage.memory_total > usage.memory_used && usage.memory_used > 0);
    }
}

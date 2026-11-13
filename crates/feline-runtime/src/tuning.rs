// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! How much of the PC Roblox gets (runtime settings, see prefs.rs):
//!
//! - `cpuThreads`: Roblox runs on that many CPU threads (the process's CPU
//!   affinity, set before any thread starts so all of them inherit it).
//! - `priority`: the process's nice value. Raising it needs the system's
//!   permission (`RLIMIT_NICE`, off on most desktops); lowering always works.
//! - `trimMemory`: while the window isn't focused, memory Roblox isn't
//!   touching is handed back to the system (`MADV_PAGEOUT`: unchanged file
//!   pages are dropped, the rest goes to swap). What Roblox touches again
//!   comes back by itself. This is what "RAM trimming" in Windows
//!   launchers does with `EmptyWorkingSet`.
//! - `blockCaptures`: Roblox's own screenshots and recordings have nowhere
//!   to be saved (its capture folder is made read-only), the way Windows
//!   launchers block Roblox's Pictures and Videos folders.

// libc calls: CPU affinity, priority, madvise.
#![allow(unsafe_code)]

use std::path::Path;
use std::time::{Duration, Instant};

use crate::prefs::Prefs;

/// The first `count` of `allowed` (CPU numbers the process may use now).
/// `count` 0, or as many as there are, changes nothing (None).
fn pick_cpus(allowed: &[usize], count: u32) -> Option<Vec<usize>> {
    let count = count as usize;
    (count > 0 && count < allowed.len()).then(|| allowed[..count].to_vec())
}

/// The nice value for a `priority` setting (None: leave it alone).
fn nice_for(priority: &str) -> Option<i32> {
    match priority {
        "low" => Some(10),
        "high" => Some(-5),
        _ => None,
    }
}

/// CPU threads and priority. Call before any other thread starts.
pub fn apply_at_start(prefs: &Prefs) {
    if prefs.cpu_threads > 0 {
        // SAFETY: plain libc calls on a zeroed, correctly sized CPU set.
        unsafe {
            let mut set: libc::cpu_set_t = std::mem::zeroed();
            if libc::sched_getaffinity(0, size_of::<libc::cpu_set_t>(), &mut set) == 0 {
                let allowed: Vec<usize> =
                    (0..libc::CPU_SETSIZE as usize).filter(|&cpu| libc::CPU_ISSET(cpu, &set)).collect();
                if let Some(cpus) = pick_cpus(&allowed, prefs.cpu_threads) {
                    let mut wanted: libc::cpu_set_t = std::mem::zeroed();
                    for &cpu in &cpus {
                        libc::CPU_SET(cpu, &mut wanted);
                    }
                    if libc::sched_setaffinity(0, size_of::<libc::cpu_set_t>(), &wanted) == 0 {
                        eprintln!(
                            "feline-runtime: Roblox runs on {} of {} CPU threads",
                            cpus.len(),
                            allowed.len()
                        );
                    } else {
                        eprintln!(
                            "feline-runtime: couldn't limit the CPU threads: {}",
                            std::io::Error::last_os_error()
                        );
                    }
                }
            }
        }
    }
    if let Some(nice) = nice_for(&prefs.priority) {
        // SAFETY: a plain libc call.
        let result = unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, nice) };
        if result == 0 {
            eprintln!("feline-runtime: priority {} (nice {nice})", prefs.priority);
        } else {
            eprintln!(
                "feline-runtime: the system didn't allow {} priority ({}); \
                 a higher priority needs a nice limit (ulimit -e) or GameMode",
                prefs.priority,
                std::io::Error::last_os_error()
            );
        }
    }
}

/// One line of `/proc/self/maps` as (start, end) if it's memory worth
/// paging out: private and readable, and not the kernel's own pages.
fn trimmable(line: &str) -> Option<(usize, usize)> {
    let mut parts = line.split_whitespace();
    let range = parts.next()?;
    let perms = parts.next()?;
    let name = parts.nth(3).unwrap_or("");
    if !perms.starts_with('r') || !perms.ends_with('p') {
        return None;
    }
    if matches!(name, "[vdso]" | "[vvar]" | "[vsyscall]" | "[vvar_vclock]") || name.starts_with("/dev/") {
        return None;
    }
    let (start, end) = range.split_once('-')?;
    let start = usize::from_str_radix(start, 16).ok()?;
    let end = usize::from_str_radix(end, 16).ok()?;
    (end > start).then_some((start, end))
}

/// This process's resident memory in KiB.
fn resident_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmRSS:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

/// Hands memory that isn't being touched back to the system. Returns how
/// many KiB of resident memory went away.
pub fn trim_now() -> u64 {
    let before = resident_kib().unwrap_or(0);
    let Ok(maps) = std::fs::read_to_string("/proc/self/maps") else {
        return 0;
    };
    for (start, end) in maps.lines().filter_map(trimmable) {
        // SAFETY: MADV_PAGEOUT never changes what the memory holds; pages
        // that can't be paged out (locked, device memory) are refused or
        // skipped by the kernel, and a mapping that went away since the
        // list was read just fails.
        unsafe {
            libc::madvise(start as *mut libc::c_void, end - start, libc::MADV_PAGEOUT);
        }
    }
    before.saturating_sub(resident_kib().unwrap_or(before))
}

/// Unfocused this long: trim.
const TRIM_AFTER: Duration = Duration::from_secs(10);
/// And again this often while it stays unfocused.
const TRIM_EVERY: Duration = Duration::from_secs(120);

/// Starts the memory trimmer, if `trimMemory` is on.
pub fn start_trimmer(prefs: &Prefs) {
    if !prefs.trim_memory {
        return;
    }
    let _ = std::thread::Builder::new().name("feline-trim".into()).spawn(|| {
        let mut unfocused_since: Option<Instant> = None;
        let mut trimmed: Option<Instant> = None;
        loop {
            std::thread::sleep(Duration::from_secs(1));
            if crate::pacing::focused() {
                unfocused_since = None;
                trimmed = None;
                continue;
            }
            let since = *unfocused_since.get_or_insert_with(Instant::now);
            let due = match trimmed {
                None => since.elapsed() >= TRIM_AFTER,
                Some(at) => at.elapsed() >= TRIM_EVERY,
            };
            if due {
                let freed = trim_now();
                trimmed = Some(Instant::now());
                eprintln!("feline-runtime: trimmed memory in the background: {} MB handed back", freed / 1024);
            }
        }
    });
}

/// Roblox's own captures land in `<app data>/tmp-capture-storage` first.
/// Blocked: that folder is read-only, so a capture can't be saved.
pub fn block_captures(app_data: &Path, block: bool) {
    use std::os::unix::fs::PermissionsExt;
    let dir = app_data.join("tmp-capture-storage");
    if block {
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
    } else if !dir.is_dir() {
        return;
    }
    let mode = if block { 0o500 } else { 0o700 };
    match std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(mode)) {
        Ok(()) if block => eprintln!("feline-runtime: Roblox's screenshots and recordings are blocked"),
        Ok(()) => {}
        Err(e) => eprintln!("feline-runtime: couldn't change {}: {e}", dir.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_choice() {
        let allowed: Vec<usize> = (0..16).collect();
        assert_eq!(pick_cpus(&allowed, 0), None);
        assert_eq!(pick_cpus(&allowed, 4), Some(vec![0, 1, 2, 3]));
        assert_eq!(pick_cpus(&allowed, 16), None);
        assert_eq!(pick_cpus(&allowed, 99), None);
        // Already limited from outside (taskset): stays inside that.
        assert_eq!(pick_cpus(&[2, 3, 6, 7], 2), Some(vec![2, 3]));
    }

    #[test]
    fn priorities() {
        assert_eq!(nice_for("low"), Some(10));
        assert_eq!(nice_for("normal"), None);
        assert_eq!(nice_for("high"), Some(-5));
        assert_eq!(nice_for("realtime"), None);
    }

    #[test]
    fn which_memory_is_trimmed() {
        assert_eq!(
            trimmable("7f0000000000-7f0000100000 rw-p 00000000 00:00 0"),
            Some((0x7f00_0000_0000, 0x7f00_0010_0000))
        );
        assert_eq!(
            trimmable("7f0000000000-7f0000100000 r-xp 00001000 08:01 123 /usr/lib/libc.so.6"),
            Some((0x7f00_0000_0000, 0x7f00_0010_0000))
        );
        // Shared, unreadable, the kernel's and devices': left alone.
        assert_eq!(trimmable("7f0000000000-7f0000100000 rw-s 00000000 00:01 5 /dev/shm/x"), None);
        assert_eq!(trimmable("7f0000000000-7f0000100000 ---p 00000000 00:00 0"), None);
        assert_eq!(trimmable("7ffc00000000-7ffc00002000 r-xp 00000000 00:00 0 [vdso]"), None);
        assert_eq!(trimmable("7f0000000000-7f0000100000 rw-p 00000000 00:05 9 /dev/nvidiactl"), None);
        assert_eq!(trimmable("garbage"), None);
    }

    #[test]
    fn trimming_keeps_memory_intact() {
        let data: Vec<u8> = (0..4 << 20).map(|i| (i % 251) as u8).collect();
        trim_now();
        assert!(data.iter().enumerate().all(|(i, &b)| b == (i % 251) as u8));
    }

    #[test]
    fn captures_folder_is_locked_and_unlocked() {
        use std::os::unix::fs::PermissionsExt;
        let app = std::env::temp_dir().join(format!("feline-tuning-test-{}", std::process::id()));
        let dir = app.join("tmp-capture-storage");
        // Off, and Roblox never made the folder: nothing appears.
        block_captures(&app, false);
        assert!(!dir.exists());
        block_captures(&app, true);
        assert_eq!(std::fs::metadata(&dir).expect("made").permissions().mode() & 0o777, 0o500);
        assert!(std::fs::write(dir.join("shot.png"), b"x").is_err());
        block_captures(&app, false);
        assert!(std::fs::write(dir.join("shot.png"), b"x").is_ok());
        let _ = std::fs::remove_dir_all(&app);
    }
}

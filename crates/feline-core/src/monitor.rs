// SPDX-License-Identifier: GPL-3.0-or-later
//! Basic monitoring of a running game: CPU and memory use.
//!
//! The game isn't one process. Feline starts e.g. `flatpak run …`, which starts
//! a sandbox, which starts the runtime, which may start helpers. So Feline adds
//! up **every descendant** of the process it started (found through the parent
//! IDs in `/proc/<pid>/stat`). Flatpak sandboxes use their own process IDs
//! inside, but from outside they're still ordinary descendants.
//!
//! - **CPU:** the change in user + system time (`/proc/<pid>/stat`, fields 14
//!   and 15) between two samples, divided by the time between them.
//! - **Memory:** PSS ("proportional set size", `/proc/<pid>/smaps_rollup`).
//!   Shared memory is divided between the processes sharing it, so the sum
//!   isn't inflated the way adding up RSS would be. Falls back to RSS if PSS
//!   can't be read.
//!
//! FPS isn't measured here: that needs a graphics layer (such as MangoHud).

use std::collections::{HashMap, HashSet};
#[cfg_attr(windows, allow(unused_imports))]
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// CPU time in `/proc/<pid>/stat` is in "clock ticks". Linux reports them in
/// USER_HZ, which is 100 on every architecture Feline supports. (Reading it
/// properly needs `sysconf`, a C call; not worth `unsafe` code for a constant.)
#[cfg(not(windows))]
const TICKS_PER_SECOND: f64 = 100.0;
/// On Windows, CPU time comes in 100-nanosecond units.
#[cfg(windows)]
const TICKS_PER_SECOND: f64 = 10_000_000.0;

/// One measurement of the whole game.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    /// CPU use as a share of the whole machine, 0–100.
    pub cpu_percent: f64,
    /// Memory in bytes (PSS, see the module docs).
    pub memory_bytes: u64,
    /// How many processes make up the game right now.
    pub processes: usize,
}

/// One line of `/proc/<pid>/stat`, the parts we need.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcStat {
    pub ppid: u32,
    /// User + system CPU time, in clock ticks.
    pub cpu_ticks: u64,
}

/// Parses `/proc/<pid>/stat`. The second field is the program name in
/// parentheses, and it may itself contain spaces or parentheses, so parsing
/// starts after the **last** `)`.
pub fn parse_stat(contents: &str) -> Option<ProcStat> {
    let after_name = &contents[contents.rfind(')')? + 1..];
    let fields: Vec<&str> = after_name.split_whitespace().collect();
    // After the name: [0] state, [1] ppid, …, [11] utime, [12] stime
    // (fields 3, 4, 14 and 15 in `man proc`).
    let ppid = fields.get(1)?.parse().ok()?;
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    Some(ProcStat {
        ppid,
        cpu_ticks: utime + stime,
    })
}

/// The `Pss:` value of `/proc/<pid>/smaps_rollup`, or `VmRSS:` of
/// `/proc/<pid>/status`, in bytes (both files use kB).
pub fn parse_kib_field(contents: &str, field: &str) -> Option<u64> {
    let line = contents.lines().find(|l| l.starts_with(field))?;
    let kib: u64 = line[field.len()..]
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    Some(kib * 1024)
}

/// `root` and all its descendants, given each process's parent.
pub fn descendants(root: u32, parents: &HashMap<u32, u32>) -> HashSet<u32> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (&pid, &ppid) in parents {
        children.entry(ppid).or_default().push(pid);
    }
    let mut found = HashSet::new();
    let mut stack = vec![root];
    while let Some(pid) = stack.pop() {
        // `insert` returns false for a PID already seen, which also guards
        // against a parent loop in a racy snapshot.
        if parents.contains_key(&pid) && found.insert(pid) {
            stack.extend(children.get(&pid).into_iter().flatten().copied());
        }
    }
    found
}

/// Measures a running game. Call [`Sampler::sample`] regularly; the first
/// call only sets the starting point for CPU and returns `None`.
pub struct Sampler {
    // Linux only: Windows asks the system directly.
    #[cfg_attr(windows, allow(dead_code))]
    proc_dir: PathBuf,
    root: u32,
    threads: usize,
    /// CPU ticks per process at the last sample.
    last: HashMap<u32, u64>,
    last_at: Option<Instant>,
}

impl Sampler {
    /// Watches the process `root` (the one Feline started) and its descendants.
    pub fn new(root: u32) -> Self {
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
        Self::with_proc_dir(Path::new("/proc"), root, threads)
    }

    /// For tests: a fake `/proc` and thread count.
    pub fn with_proc_dir(proc_dir: &Path, root: u32, threads: usize) -> Self {
        Self {
            proc_dir: proc_dir.to_owned(),
            root,
            threads: threads.max(1),
            last: HashMap::new(),
            last_at: None,
        }
    }

    /// Every process's parent. CPU time is read later, only for the game's
    /// processes (opening every process on Windows would be wasteful).
    #[cfg(windows)]
    fn snapshot(&self) -> HashMap<u32, ProcStat> {
        feline_win::processes()
            .into_iter()
            .map(|p| {
                (
                    p.pid,
                    ProcStat {
                        ppid: p.parent_pid,
                        cpu_ticks: 0,
                    },
                )
            })
            .collect()
    }

    #[cfg(windows)]
    fn cpu_ticks(&self, pid: u32, _stat: &ProcStat) -> u64 {
        feline_win::game::usage(pid).map_or(0, |u| u.cpu_time)
    }

    #[cfg(windows)]
    fn memory_of(&self, pid: u32) -> u64 {
        feline_win::game::usage(pid).map_or(0, |u| u.memory)
    }

    #[cfg(not(windows))]
    fn cpu_ticks(&self, _pid: u32, stat: &ProcStat) -> u64 {
        stat.cpu_ticks
    }

    /// Reads every process's parent and CPU time.
    #[cfg(not(windows))]
    fn snapshot(&self) -> HashMap<u32, ProcStat> {
        let Ok(entries) = fs::read_dir(&self.proc_dir) else {
            return HashMap::new();
        };
        entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
                // A process can exit between listing and reading; skip it.
                let stat = parse_stat(&fs::read_to_string(entry.path().join("stat")).ok()?)?;
                Some((pid, stat))
            })
            .collect()
    }

    #[cfg(not(windows))]
    fn memory_of(&self, pid: u32) -> u64 {
        let dir = self.proc_dir.join(pid.to_string());
        fs::read_to_string(dir.join("smaps_rollup"))
            .ok()
            .and_then(|s| parse_kib_field(&s, "Pss:"))
            .or_else(|| {
                fs::read_to_string(dir.join("status"))
                    .ok()
                    .and_then(|s| parse_kib_field(&s, "VmRSS:"))
            })
            .unwrap_or(0)
    }

    pub fn sample(&mut self) -> Option<Sample> {
        self.sample_at(Instant::now())
    }

    fn sample_at(&mut self, now: Instant) -> Option<Sample> {
        let all = self.snapshot();
        let parents: HashMap<u32, u32> = all.iter().map(|(&pid, s)| (pid, s.ppid)).collect();
        let game = descendants(self.root, &parents);

        let ticks: HashMap<u32, u64> = game
            .iter()
            .map(|pid| (*pid, self.cpu_ticks(*pid, &all[pid])))
            .collect();
        // A process that started since the last sample counts from zero; one
        // that exited simply drops out.
        let used: u64 = ticks
            .iter()
            .map(|(pid, &t)| t.saturating_sub(self.last.get(pid).copied().unwrap_or(0)))
            .sum();
        let elapsed = self.last_at.map(|at| now.duration_since(at));
        self.last = ticks;
        self.last_at = Some(now);

        let elapsed = elapsed.filter(|d| *d > Duration::ZERO)?;
        let busy_seconds = used as f64 / TICKS_PER_SECOND;
        let cpu_percent =
            (busy_seconds / elapsed.as_secs_f64() / self.threads as f64 * 100.0).clamp(0.0, 100.0);
        Some(Sample {
            cpu_percent,
            memory_bytes: game.iter().map(|&pid| self.memory_of(pid)).sum(),
            processes: game.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stat_line(pid: u32, name: &str, ppid: u32, utime: u64, stime: u64) -> String {
        // Real layout: pid (comm) state ppid pgrp session tty tpgid flags
        // minflt cminflt majflt cmajflt utime stime …
        format!("{pid} ({name}) S {ppid} 1 1 0 -1 4194560 100 0 0 0 {utime} {stime} 0 0 20 0 8 0\n")
    }

    #[test]
    fn stat_parsing_handles_odd_names() {
        let s = parse_stat(&stat_line(42, "Web Content (x)", 7, 150, 50)).unwrap();
        assert_eq!(
            s,
            ProcStat {
                ppid: 7,
                cpu_ticks: 200
            }
        );
        assert_eq!(parse_stat("garbage"), None);
    }

    #[test]
    fn kib_fields() {
        let rollup = "55d0-7ffd ---p 00000000 00:00 0 [rollup]\nRss:  2048 kB\nPss:  1024 kB\n";
        assert_eq!(parse_kib_field(rollup, "Pss:"), Some(1024 * 1024));
        assert_eq!(
            parse_kib_field("VmRSS:\t  300 kB\n", "VmRSS:"),
            Some(300 * 1024)
        );
        assert_eq!(parse_kib_field("Rss: 1 kB\n", "Pss:"), None);
    }

    #[test]
    fn finds_all_descendants_only() {
        // 1 → 10 (flatpak) → 11 (bwrap) → 12 (sober) → 13 (helper); 20 is unrelated.
        let parents: HashMap<u32, u32> =
            [(1, 0), (10, 1), (11, 10), (12, 11), (13, 12), (20, 1)].into();
        let found = descendants(10, &parents);
        assert_eq!(found, HashSet::from([10, 11, 12, 13]));
        assert!(descendants(99, &parents).is_empty(), "exited root");
    }

    #[cfg(not(windows))]
    #[test]
    fn samples_cpu_and_memory_of_the_tree() {
        let dir = std::env::temp_dir().join(format!("feline-test-monitor-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let write = |pid: u32, ppid: u32, ticks: u64, pss_kib: u64| {
            let p = dir.join(pid.to_string());
            fs::create_dir_all(&p).unwrap();
            fs::write(p.join("stat"), stat_line(pid, "x", ppid, ticks, 0)).unwrap();
            fs::write(p.join("smaps_rollup"), format!("Pss: {pss_kib} kB\n")).unwrap();
        };
        write(1, 0, 0, 0);
        write(10, 1, 100, 1000);
        write(11, 10, 1000, 3000);
        write(20, 1, 5000, 9999); // not part of the game

        let mut sampler = Sampler::with_proc_dir(&dir, 10, 4);
        let t0 = Instant::now();
        assert_eq!(sampler.sample_at(t0), None, "first sample is the baseline");

        // In 2 s the game used 200 ticks = 2 CPU-seconds = one core of four.
        write(10, 1, 150, 1000);
        write(11, 10, 1150, 3000);
        let s = sampler.sample_at(t0 + Duration::from_secs(2)).unwrap();
        let _ = fs::remove_dir_all(&dir);

        assert!((s.cpu_percent - 25.0).abs() < 1e-9, "{}", s.cpu_percent);
        assert_eq!(s.memory_bytes, 4000 * 1024);
        assert_eq!(s.processes, 2);
    }
}

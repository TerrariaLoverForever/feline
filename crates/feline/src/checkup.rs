// SPDX-License-Identifier: GPL-3.0-or-later
//! The System page's PC check, driver check and live usage graph, and the
//! Diagnostics page's crash detective, network test and bug report zip.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::PathBuf;

use feline_core::history::{self, History};
use feline_core::housekeeping::human;
use feline_core::pc::{self, UsageMeter};
use feline_core::system::{CheckLevel, SystemInfo};
use feline_core::{detective, nettest};
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::gui::{AppWindow, CaseRow, CheckRow, DriverRow, Level, ProbeRow, Shared};

fn level(level: CheckLevel) -> Level {
    match level {
        CheckLevel::Ok => Level::Ok,
        CheckLevel::Warning => Level::Warning,
        CheckLevel::Problem => Level::Problem,
    }
}

fn rows(checks: Vec<feline_core::system::Check>) -> ModelRc<CheckRow> {
    let rows: Vec<CheckRow> = checks
        .into_iter()
        .map(|c| CheckRow {
            level: level(c.level),
            title: c.title.into(),
            detail: c.detail.into(),
        })
        .collect();
    ModelRc::new(VecModel::from(rows))
}

/// Opens a web page in the default browser.
pub fn open_link(url: &str) {
    if !url.starts_with("https://") {
        return;
    }
    let opener = if cfg!(windows) { "explorer" } else { "xdg-open" };
    let _ = std::process::Command::new(opener)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

// ---------------------------------------------------------------------------
// System page
// ---------------------------------------------------------------------------

/// The PC check and the drivers, after a system scan.
pub fn show_pc(ui: &AppWindow, info: &SystemInfo) {
    let free = pc::roblox_home().and_then(|p| pc::disk_space(&p)).map(|(free, _)| free);
    let (verdict, checks) = pc::pc_check(info, free);
    ui.set_verdict_level(level(verdict.level));
    ui.set_verdict_title(verdict.title.into());
    ui.set_verdict_detail(verdict.detail.into());
    ui.set_pc_checks(rows(checks));
    let drivers: Vec<DriverRow> = pc::drivers(info)
        .into_iter()
        .map(|d| DriverRow {
            gpu: d.gpu.into(),
            version: d.version.into(),
            date: d.date.unwrap_or_default().into(),
            level: level(d.level),
            advice: d.advice.into(),
            link: d.link.unwrap_or_default().into(),
        })
        .collect();
    ui.set_drivers(ModelRc::new(VecModel::from(drivers)));
}

/// How many seconds the live graph shows.
const SLOTS: usize = 60;

#[derive(Default)]
struct Live {
    meter: Option<UsageMeter>,
    cpu: VecDeque<f32>,
    roblox_cpu: VecDeque<f32>,
    memory: VecDeque<f32>,
    roblox_memory: VecDeque<f32>,
}

thread_local! {
    static LIVE: RefCell<Live> = RefCell::new(Live::default());
}

fn push(list: &mut VecDeque<f32>, value: f32) {
    list.push_back(value);
    while list.len() > SLOTS {
        list.pop_front();
    }
}

fn model(list: &VecDeque<f32>) -> ModelRc<f32> {
    ModelRc::new(VecModel::from(list.iter().copied().collect::<Vec<_>>()))
}

/// One live sample (the System page's timer, once a second).
pub fn tick(ui: &AppWindow) {
    LIVE.with(|live| {
        let mut live = live.borrow_mut();
        let Some(usage) = live.meter.get_or_insert_with(UsageMeter::new).sample() else {
            return;
        };
        let total = usage.memory_total.max(1) as f32;
        push(&mut live.cpu, usage.cpu / 100.0);
        push(&mut live.roblox_cpu, usage.roblox_cpu / 100.0);
        push(&mut live.memory, usage.memory_used as f32 / total);
        push(&mut live.roblox_memory, usage.roblox_memory as f32 / total);
        ui.set_cpu_history(model(&live.cpu));
        ui.set_roblox_cpu_history(model(&live.roblox_cpu));
        ui.set_memory_history(model(&live.memory));
        ui.set_roblox_memory_history(model(&live.roblox_memory));
        ui.set_cpu_now(format!("{:.0}%", usage.cpu).into());
        ui.set_memory_now(
            format!("{} of {}", human(usage.memory_used), human(usage.memory_total)).into(),
        );
        ui.set_roblox_now(
            if usage.roblox {
                format!("Roblox: {:.0}% CPU, {}", usage.roblox_cpu, human(usage.roblox_memory))
            } else {
                "Roblox (not running)".to_owned()
            }
            .into(),
        );
    });
}

// ---------------------------------------------------------------------------
// Crash detective
// ---------------------------------------------------------------------------

fn roblox_running() -> bool {
    let names: &[&str] = if cfg!(windows) {
        &["RobloxPlayerBeta.exe"]
    } else {
        &["feline-runtime", "sober"]
    };
    feline_core::runtime::config::any_process_named(names)
}

/// "today 23:17", "yesterday 08:02", "3 Nov 14:30".
fn when(time: u64) -> String {
    let offset = history::local_offset_seconds();
    let local = time as i64 + offset;
    let (today, _) = history::day_and_week_start(history::now(), offset);
    let clock = format!("{:02}:{:02}", local.rem_euclid(86_400) / 3600, local.rem_euclid(3600) / 60);
    if time >= today {
        format!("today {clock}")
    } else if time + 86_400 >= today {
        format!("yesterday {clock}")
    } else {
        format!("{} days ago {clock}", (today - time) / 86_400 + 1)
    }
}

fn case_row(case: &detective::Case, h: &History) -> CaseRow {
    let game = match (case.place_id, case.universe_id.and_then(|u| h.games.get(&u))) {
        (_, Some(info)) if !info.name.is_empty() => info.name.clone(),
        (Some(place), _) => format!("Game {place}"),
        (None, _) => "Roblox's menus (no game)".to_owned(),
    };
    let length = history::duration_label(case.end.saturating_sub(case.start));
    let last_reason = case.disconnects.last();
    let (ending, lvl, advice) = match (&case.ending, last_reason) {
        (detective::Ending::Running, _) => ("Still running".to_owned(), Level::Ok, String::new()),
        (detective::Ending::ClosedByFeline, _) => (
            "Closed by Feline after you left".to_owned(),
            Level::Ok,
            String::new(),
        ),
        (detective::Ending::Unexpected { confirmed }, reason) => {
            let mut text = if *confirmed {
                "Crashed (Roblox saved a crash report)".to_owned()
            } else {
                "Closed unexpectedly: a crash, or ended by force".to_owned()
            };
            if let Some(r) = reason {
                text.push_str(&format!(", after: {r}"));
            }
            (text, Level::Problem, String::new())
        }
        (detective::Ending::Normal, Some(reason)) => (
            format!("Disconnected: {reason}"),
            Level::Warning,
            detective::advice_for(reason).to_owned(),
        ),
        (detective::Ending::Normal, None) => ("Closed normally".to_owned(), Level::Ok, String::new()),
    };
    CaseRow {
        when: format!("{} · {length}", when(case.start)).into(),
        game: game.into(),
        ending: ending.into(),
        level: lvl,
        advice: advice.into(),
    }
}

/// Runs the crash detective in the background.
pub fn investigate(ui: &AppWindow, shared: &Shared) {
    if ui.get_investigating() {
        return;
    }
    ui.set_investigating(true);
    let dirs = crate::home_page::log_roots(shared);
    let weak = ui.as_weak();
    let _ = std::thread::Builder::new()
        .name("feline-detective".into())
        .spawn(move || {
            let cases = detective::investigate(&dirs, roblox_running(), 12);
            let clues = detective::conclusions(&cases);
            let h = History::load();
            let case_rows: Vec<CaseRow> = cases.iter().map(|c| case_row(c, &h)).collect();
            let clue_rows: Vec<(CheckLevel, String, String)> =
                clues.into_iter().map(|c| (c.level, c.title, c.detail)).collect();
            let _ = weak.upgrade_in_event_loop(move |ui| {
                ui.set_investigating(false);
                ui.set_cases(ModelRc::new(VecModel::from(case_rows)));
                let clues: Vec<CheckRow> = clue_rows
                    .into_iter()
                    .map(|(l, t, d)| CheckRow {
                        level: level(l),
                        title: t.into(),
                        detail: d.into(),
                    })
                    .collect();
                ui.set_clues(ModelRc::new(VecModel::from(clues)));
            });
        });
}

/// The detective's findings as text, for the bug report.
pub fn detective_text(dirs: &[PathBuf]) -> String {
    let cases = detective::investigate(dirs, roblox_running(), 12);
    let mut out = String::from("Crash detective\n");
    for c in detective::conclusions(&cases) {
        out.push_str(&format!("  [{:?}] {}\n", c.level, c.title));
    }
    for c in &cases {
        let ending = match &c.ending {
            detective::Ending::Normal => "closed normally".to_owned(),
            detective::Ending::Running => "running".to_owned(),
            detective::Ending::ClosedByFeline => "closed by Feline after leaving".to_owned(),
            detective::Ending::Unexpected { confirmed } => {
                format!("unexpected end (crash report: {})", if *confirmed { "yes" } else { "no" })
            }
        };
        out.push_str(&format!(
            "  {} · {} · place {} · {}{}\n",
            when(c.start),
            history::duration_label(c.end.saturating_sub(c.start)),
            c.place_id.map_or("-".to_owned(), |p| p.to_string()),
            ending,
            c.disconnects
                .last()
                .map_or(String::new(), |r| format!(" · disconnected: {r}")),
        ));
    }
    out
}

// ---------------------------------------------------------------------------
// Network test
// ---------------------------------------------------------------------------

/// The last game server's address and region, from the play history.
fn last_server() -> Option<(std::net::IpAddr, String)> {
    let h = History::load();
    let session = h
        .sessions()
        .filter(|s| s.server.is_some())
        .max_by_key(|s| s.start)?;
    let ip = session.server.as_deref()?;
    let region = h.regions.get(ip).cloned().unwrap_or_default();
    Some((ip.parse().ok()?, region))
}

/// Runs the network test in the background.
pub fn network_test(ui: &AppWindow) {
    if ui.get_net_running() {
        return;
    }
    ui.set_net_running(true);
    ui.set_net_title("".into());
    let weak = ui.as_weak();
    let _ = std::thread::Builder::new()
        .name("feline-network-test".into())
        .spawn(move || {
            let probes = nettest::run(last_server());
            let verdict = nettest::verdict(&probes);
            let rows: Vec<(String, String, String, CheckLevel)> = probes
                .iter()
                .map(|p| {
                    let host = match p.dns_ms {
                        Some(ms) => format!("{} · name lookup {ms:.0} ms", p.host),
                        None => p.host.clone(),
                    };
                    (p.label.clone(), host, p.summary(), p.level())
                })
                .collect();
            let _ = weak.upgrade_in_event_loop(move |ui| {
                ui.set_net_running(false);
                ui.set_net_level(level(verdict.level));
                ui.set_net_title(verdict.title.into());
                ui.set_net_detail(verdict.detail.into());
                let rows: Vec<ProbeRow> = rows
                    .into_iter()
                    .map(|(label, host, result, l)| ProbeRow {
                        label: label.into(),
                        host: host.into(),
                        result: result.into(),
                        level: level(l),
                    })
                    .collect();
                ui.set_probes(ModelRc::new(VecModel::from(rows)));
            });
        });
}

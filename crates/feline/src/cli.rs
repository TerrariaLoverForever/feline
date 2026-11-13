// SPDX-License-Identifier: GPL-3.0-or-later
//! Feline's text commands (`feline --info` etc.).
//!
//! Arguments are parsed by hand: there are only a few, so a parsing crate
//! would add more than it saves.

use std::process::ExitCode;

use feline_core::runtime::{DetectContext, Installation, detect_all};
use feline_core::system::SystemInfo;

const HELP: &str = "\
Feline: a launcher for playing Roblox on Linux

Usage: feline [OPTION]

Without an option, Feline opens its window.

Options:
  --info         Print information about this system (for bug reports)
  --runtimes     List the runtimes found on this system, numbered
  --doctor <N>   Run the self-check of runtime number N from --runtimes
  --update-roblox
                 Download the newest Roblox app for Feline's runtime, if
                 there is one (checked against Roblox's signature)
  --presence     Show the Discord status for the game that is starting or
                 running, then exit when it ends (Feline starts this itself)
  --install-desktop-entry
                 Add Feline to your app menu (and give its window an icon)
  --uninstall-desktop-entry
                 Remove what --install-desktop-entry added
  -V, --version  Print Feline's version
  -h, --help     Print this help

Listed runtimes are Feline's own or external third-party
programs you installed yourself; Feline never picks one for you.
";

/// Runs a text command. `args` excludes the program name and is never empty
/// (no arguments means "open the window", handled in `main`).
pub fn run(args: &[&str]) -> ExitCode {
    match args {
        ["--info"] => {
            println!("Feline {}\n", feline_core::VERSION);
            print!("{}", SystemInfo::collect().report());
            ExitCode::SUCCESS
        }
        ["--runtimes"] => list_runtimes(),
        ["--update-roblox"] => update_roblox(),
        ["--doctor", number] => doctor(number),
        ["--doctor"] => {
            eprintln!(
                "feline: --doctor needs a runtime number. Run `feline --runtimes` to see them."
            );
            ExitCode::from(2)
        }
        ["--presence"] => {
            match feline_core::presence::service::run_for_game(|status| {
                eprintln!("feline: {status}")
            }) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("feline: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        // Feline starts these itself (see companion.rs).
        ["--with-roblox", pid, rest @ ..] => match pid.parse() {
            Ok(pid) => crate::companion::run_game_helper(pid, rest.contains(&"--cursor")),
            Err(_) => ExitCode::from(2),
        },
        ["--multi-roblox"] => crate::companion::run_multi_holder(),
        ["--studio"] => crate::companion::run_studio_command(None),
        ["--studio", file] => crate::companion::run_studio_command(Some(file)),
        ["--with-studio", pid] => match pid.parse() {
            Ok(pid) => crate::companion::run_studio_helper(pid),
            Err(_) => ExitCode::from(2),
        },
        ["--install-desktop-entry"] => install_desktop_entry(),
        ["--uninstall-desktop-entry"] => uninstall_desktop_entry(),
        ["-V" | "--version"] => {
            println!("Feline {}", feline_core::VERSION);
            ExitCode::SUCCESS
        }
        ["-h" | "--help"] => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        other => {
            eprintln!(
                "feline: unrecognised arguments: {}\n\n{HELP}",
                other.join(" ")
            );
            // Exit code 2 is the usual convention for "wrong usage".
            ExitCode::from(2)
        }
    }
}

const NO_RUNTIME: &str = "No runtime found. Feline's own runtime needs feline-runtime and \
                          Roblox's engine (libroblox.so and base.apk) in \
                          ~/.local/share/feline/engine/. Feline can also start supported \
                          external runtimes you install yourself (currently: Sober, Cordial, Stud).";

fn list_runtimes() -> ExitCode {
    let found = detect_all(&DetectContext::host());
    if found.is_empty() {
        eprintln!("{NO_RUNTIME}");
        return ExitCode::FAILURE;
    }
    for (i, installation) in found.iter().enumerate() {
        println!("{:>2}. {}", i + 1, describe(installation));
    }
    ExitCode::SUCCESS
}

/// Like `Installation`'s Display, but asks the program for its version when
/// detection couldn't read it from a file.
fn describe(installation: &Installation) -> String {
    if installation.known_version.is_some() {
        return installation.to_string();
    }
    let version = installation
        .version()
        .unwrap_or_else(|e| format!("(version unknown: {e})"));
    format!(
        "{} {version} · {} · {}",
        installation.backend.name(),
        installation.backend.origin(),
        installation.kind
    )
}

fn doctor(number: &str) -> ExitCode {
    let found = detect_all(&DetectContext::host());
    // Numbers shown to users start at 1; vector indexes start at 0.
    let chosen = number
        .parse::<usize>()
        .ok()
        .and_then(|n| n.checked_sub(1))
        .and_then(|i| found.get(i));
    let Some(installation) = chosen else {
        eprintln!(
            "feline: there is no runtime number {number}. Run `feline --runtimes` to see them."
        );
        return ExitCode::from(2);
    };

    println!("Running the self-check of {installation}…\n");
    match installation.doctor() {
        Ok(report) => {
            println!("{}", report.output);
            if report.ok {
                println!("\nResult: no blocking problems found.");
                ExitCode::SUCCESS
            } else {
                println!("\nResult: something will stop Roblox from starting (see above).");
                ExitCode::FAILURE
            }
        }
        Err(e) => {
            eprintln!("Couldn't run the self-check: {e}");
            ExitCode::FAILURE
        }
    }
}

fn install_desktop_entry() -> ExitCode {
    match crate::desktop::install() {
        Ok(report) => {
            println!("Installed for your user account:");
            for file in &report.files {
                println!("  {}", file.display());
            }
            for warning in &report.warnings {
                println!("Note: {warning}");
            }
            println!(
                "\nThe menu entry starts this Feline binary. Run this command again if you move it."
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("feline: {e}");
            ExitCode::FAILURE
        }
    }
}

fn uninstall_desktop_entry() -> ExitCode {
    match crate::desktop::uninstall() {
        Ok(removed) if removed.is_empty() => {
            println!("Nothing to remove: Feline's menu entry isn't installed.");
            ExitCode::SUCCESS
        }
        Ok(removed) => {
            println!("Removed:");
            for file in &removed {
                println!("  {}", file.display());
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("feline: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `--update-roblox`: installs the newest Roblox version for Feline's
/// runtime (see `feline_core::engine`).
fn update_roblox() -> ExitCode {
    use feline_core::engine::{Outcome, Progress};
    let mut last_percent = None;
    let result = feline_core::engine::update(&mut |p| match p {
        Progress::Checking => eprintln!("Checking for a new Roblox version…"),
        Progress::Downloading(done, total) => {
            let percent = total.map(|t| done * 100 / t.max(1));
            if percent != last_percent {
                last_percent = percent;
                match percent {
                    Some(p) => eprint!("\rDownloading… {p}%  "),
                    None => eprint!("\rDownloading… {} MB  ", done >> 20),
                }
            }
        }
        Progress::Verifying => eprintln!("\nChecking it's signed by Roblox…"),
        Progress::Installing => eprintln!("Installing…"),
    });
    match result {
        Ok(Outcome::UpToDate(v)) => {
            println!("Roblox {v} is the newest version.");
            ExitCode::SUCCESS
        }
        Ok(Outcome::Installed { version, previous }) => {
            match previous {
                Some(p) => println!("Roblox updated from {p} to {version}."),
                None => println!("Roblox {version} installed."),
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("feline: Roblox update failed: {e}");
            ExitCode::FAILURE
        }
    }
}

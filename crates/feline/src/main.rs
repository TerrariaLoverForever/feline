// SPDX-License-Identifier: GPL-3.0-or-later
//! Feline application entry point: no arguments opens the window, anything
//! else is a text command (see `cli.rs`).

// On Windows, Feline is a windowed program: opening it from the Start menu,
// a shortcut or a link shows no console window. Text commands attach to the
// terminal they were typed in instead (see `main`).
#![cfg_attr(windows, windows_subsystem = "windows")]

mod accounts_page;
mod checkup;
mod background;
mod cli;
mod companion;
mod desk_cat;
mod desktop;
mod extras;
mod tweaks;
mod gui;
mod home_page;
mod links;
mod looks;
mod mods_page;
mod more;
mod music;
mod picker;
mod play_extras;
mod presence_helper;
mod roblox_page;
mod settings_page;

use std::process::ExitCode;

fn main() -> ExitCode {
    // skip(1) drops the program name itself.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();

    // A `roblox://` link from the browser (when Feline handles those links).
    if let [arg] = args.as_slice()
        && links::is_link(arg)
    {
        return links::open(arg);
    }
    // Started with Windows: the window opens minimized.
    if args == ["--minimized"] {
        gui::START_MINIMIZED.store(true, std::sync::atomic::Ordering::Relaxed);
        return match gui::run() {
            Ok(()) => ExitCode::SUCCESS,
            Err(_) => ExitCode::FAILURE,
        };
    }
    if !args.is_empty() {
        // Print in the terminal the command was typed in, if there is one.
        // The Discord status helper has none, and stays invisible.
        #[cfg(windows)]
        feline_win::attach_parent_console();
        return cli::run(&args);
    }

    match gui::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!(
                "feline: couldn't open the Feline window: {e}\n{}",
                if cfg!(windows) {
                    "Text commands still work: try `feline --help`."
                } else {
                    "Feline needs a graphical session (Wayland or X11). \
                     Text commands still work: try `feline --help`."
                }
            );
            ExitCode::FAILURE
        }
    }
}

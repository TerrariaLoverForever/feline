// SPDX-License-Identifier: GPL-3.0-or-later
//! Feline application entry point: no arguments opens the window, anything
//! else is a text command (see `cli.rs`).

mod cli;
mod desktop;
mod gui;
mod links;
mod presence_helper;
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
    if !args.is_empty() {
        return cli::run(&args);
    }

    match gui::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!(
                "feline: couldn't open the Feline window: {e}\n\
                 Feline needs a graphical session (Wayland or X11). \
                 Text commands still work: try `feline --help`."
            );
            ExitCode::FAILURE
        }
    }
}

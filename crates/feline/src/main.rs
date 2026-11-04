// SPDX-License-Identifier: GPL-3.0-or-later
//! Feline application entry point: no arguments opens the window, anything
//! else is a text command (see `cli.rs`).

mod cli;
mod desktop;
mod gui;

use std::process::ExitCode;

fn main() -> ExitCode {
    // skip(1) drops the program name itself.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();

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

// SPDX-License-Identifier: GPL-3.0-or-later
//! `feline-login`: Feline's login window for its own runtime. The program
//! itself is in `app.rs`; it runs on Linux only (WebKitGTK).

#[cfg(target_os = "linux")]
mod app;

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    app::main()
}

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eprintln!(
        "feline-login: this login window is part of Feline's own runtime, \
         which only runs on Linux."
    );
    std::process::ExitCode::FAILURE
}

// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! `feline-runtime`: Feline's own runtime (in development). The program
//! itself is in `app.rs`; it runs on Linux only.

#[cfg(target_os = "linux")]
mod app;

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    app::main()
}

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eprintln!(
        "feline-runtime: Feline's own runtime only runs on Linux. \
         On Windows, Feline starts Roblox's official client instead."
    );
    std::process::ExitCode::FAILURE
}

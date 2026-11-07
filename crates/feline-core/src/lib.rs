// SPDX-License-Identifier: GPL-3.0-or-later
//! Feline core library.
//!
//! Everything that does not need a window lives here, so it can be unit-tested
//! without starting the GUI. Modules are added stage by stage as they get real
//! code (see `docs/ARCHITECTURE.md`).

// No `unsafe` anywhere in the core library, and nothing may override that.
#![forbid(unsafe_code)]

pub mod engine;
pub mod housekeeping;
pub mod launch;
pub mod link;
pub mod monitor;
pub mod paths;
pub mod presence;
pub mod process;
pub mod runtime;
pub mod settings;
pub mod system;

/// Feline's version, taken from `Cargo.toml` at compile time.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

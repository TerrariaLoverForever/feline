// SPDX-License-Identifier: GPL-3.0-or-later
//! Feline core library.
//!
//! Everything that does not need a window lives here, so it can be unit-tested
//! without starting the GUI. Modules are added stage by stage as they get real
//! code (see `docs/ARCHITECTURE.md`).

// No `unsafe` anywhere in the core library, and nothing may override that.
#![forbid(unsafe_code)]

pub mod achievements;
pub mod bugreport;
pub mod cat;
pub mod detective;
pub mod fflags;
pub mod engine;
pub mod fontpatch;
pub mod history;
pub mod housekeeping;
pub mod launch;
pub mod link;
pub mod look;
pub mod mods;
pub mod nettest;
pub mod pc;
pub mod monitor;
pub mod cursor;
pub mod modlibrary;
pub mod music;
pub mod selfupdate;
pub mod serverinfo;
pub mod shortcut;
pub mod sky;
pub mod strap_import;
pub mod studio;
pub mod paths;
pub mod png;
pub mod presence;
pub mod process;
pub mod roblox_tools;
pub mod runtime;
pub mod settings;
pub mod system;

/// Feline's version, taken from `Cargo.toml` at compile time.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

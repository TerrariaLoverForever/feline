// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! Feline's own runtime for Roblox's Android client.
//!
//! **Milestone M3, step 1.** This crate can so far only *read* the engine
//! library (`libroblox.so`): its ELF structure, the functions it needs, the
//! functions it exports, and its relocations, including Android's packed
//! format. Nothing is loaded or run yet. See `docs/runtime/M2-RESEARCH.md`.
//!
//! Clean-room rule: no code from Sober, Cordial, Stud or other Roblox
//! runtimes is used (see CONTRIBUTING.md). Roblox's library is only inspected
//! through its ELF interface, never disassembled.

// Reading files needs no `unsafe`. Mapping and running the engine (next
// steps) will, in clearly marked places only.
#![forbid(unsafe_code)]

pub mod check;
pub mod elf;
pub mod packed;

// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! Feline's own runtime for Roblox's Android client.
//!
//! This crate reads the engine library (`libroblox.so`), puts it into
//! memory, and gives it what it expects from Android: a Java world, a
//! window, graphics, input, sound and files. See `docs/ARCHITECTURE.md`,
//! and `docs/runtime/M2-RESEARCH.md` for the research it started from.
//!
//! Rules for this crate (see CONTRIBUTING.md): no code from Sober, Cordial,
//! Stud or any other Roblox runtime is used; Mocktail and Nightcap were read
//! as a reference, and each place that follows them says so. Roblox's
//! library is inspected through its ELF interface, not disassembled; the one
//! exception is the allocator address in `alloc.rs`.

// `unsafe` is denied everywhere (workspace lint) except where a module opts
// in explicitly: `load` (mapping the engine), `host` (dlopen/dlsym),
// `stubs` (generated stub code), `init` (running the engine's constructors)
// `bionic` and `dl` (Feline's implementations calling glibc) and `jni`
// (the fake Java world the engine calls into). Each use says why it's sound.

// The runtime is built on Linux's own interfaces (ELF, glibc, Wayland/X11).
// Elsewhere the crate is empty, so the workspace still builds there.
#![cfg(target_os = "linux")]

pub mod audio;
pub mod alloc;
pub mod assets;
pub mod bionic;
pub mod desktop;
pub mod check;
pub mod dex;
pub mod dl;
pub mod egl;
pub mod elf;
pub mod etc2;
pub mod files;
pub mod gamepad;
pub mod host;
pub mod imports;
pub mod init;
pub mod jni;
pub mod join;
pub mod jni_names;
pub mod keys;
pub mod liblog;
pub mod load;
pub mod login;
pub mod messagebus;
pub mod looper;
pub mod manifest;
pub mod net;
pub mod overlay;
pub mod packed;
pub mod pacing;
pub mod prefs;
pub mod sem;
pub mod sig;
pub mod stdio;
pub mod stubs;
pub mod sysconf_table;
pub mod text;
pub mod trace;
pub mod tuning;
pub mod versions;
pub mod voice;
pub mod vkoverlay;
pub mod vktex;
pub mod vulkan;
pub mod window;

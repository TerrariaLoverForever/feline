// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! Feline's own runtime for Roblox's Android client.
//!
//! **Milestone M3, step 2.** This crate can read the engine library
//! (`libroblox.so`): its ELF structure, imports, exports and relocations
//! (including Android's packed format), and put it into memory with every
//! relocation applied that doesn't need an import. None of its code is run
//! yet. See `docs/runtime/M2-RESEARCH.md`.
//!
//! Clean-room rule: no code from Sober, Cordial, Stud or other Roblox
//! runtimes is used (see CONTRIBUTING.md). Roblox's library is only inspected
//! through its ELF interface, never disassembled.

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

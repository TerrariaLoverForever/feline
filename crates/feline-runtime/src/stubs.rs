// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! Stubs: placeholder functions for imports Feline doesn't provide yet.
//!
//! The engine calls imports through plain function pointers, so each missing
//! function needs its own address, and when one is called we must know
//! *which* it was. Each stub is 32 bytes of generated x86-64 code:
//!
//! ```text
//! bf <index:u32>            mov  edi, index      ; first argument = which stub
//! 48 b8 <handler:u64>       movabs rax, handler
//! ff e0                     jmp  rax             ; tail call: same stack as the original call
//! cc …                      int3 padding
//! ```
//!
//! The handler prints the function's name and stops the process: a missing
//! function must be visible, never silently "succeed" (M2 §4). Later steps
//! replace stubs with real implementations one by one, so the list of stubs
//! hit is the to-do list.
//!
//! # `unsafe` in this module
//!
//! Mapping memory, writing the code while it's writable, then switching it
//! to read + execute (never both writable and executable), and unmapping it.

#![allow(unsafe_code)]

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use rustix::mm::{self, MapFlags, MprotectFlags, ProtFlags};

const STUB_SIZE: usize = 32;

/// Names of every stub ever made, by global index (stubs from several
/// `Stubs` sets, e.g. in tests, get distinct indexes).
static NAMES: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Tests can make a stub return 0 instead of stopping the process.
static RETURN_INSTEAD_OF_ABORT: AtomicBool = AtomicBool::new(false);
static LAST_CALLED: Mutex<Option<String>> = Mutex::new(None);

/// Called by every stub, with the stub's index.
extern "C" fn stub_called(index: u32) -> u64 {
    let name = NAMES
        .lock()
        .ok()
        .and_then(|names| names.get(index as usize).cloned())
        .unwrap_or_else(|| format!("stub #{index}"));
    if RETURN_INSTEAD_OF_ABORT.load(Ordering::Relaxed) {
        if let Ok(mut last) = LAST_CALLED.lock() {
            *last = Some(name);
        }
        return 0;
    }
    eprintln!(
        "feline-runtime: the engine called `{name}`, which Feline doesn't provide yet. Stopping."
    );
    std::process::abort();
}

/// A set of stubs in one block of executable memory.
pub struct Stubs {
    code: *mut c_void,
    len: usize,
    entries: Vec<(String, u64)>,
}

impl Stubs {
    pub fn new(names: Vec<String>) -> Result<Self, String> {
        if names.is_empty() {
            return Ok(Self {
                code: std::ptr::null_mut(),
                len: 0,
                entries: Vec::new(),
            });
        }
        let first_index = {
            let mut all = NAMES.lock().map_err(|_| "stub table poisoned")?;
            let first = all.len();
            all.extend(names.iter().cloned());
            first
        };
        let page = rustix::param::page_size();
        let len = (names.len() * STUB_SIZE).div_ceil(page) * page;

        // SAFETY: a fresh anonymous mapping can't overlap anything we own.
        let code = unsafe {
            mm::mmap_anonymous(
                std::ptr::null_mut(),
                len,
                ProtFlags::READ | ProtFlags::WRITE,
                MapFlags::PRIVATE,
            )
        }
        .map_err(|e| format!("couldn't map memory for stubs: {e}"))?;

        let handler = stub_called as extern "C" fn(u32) -> u64 as usize as u64;
        let mut entries = Vec::with_capacity(names.len());
        {
            // SAFETY: the mapping is `len` bytes and writable; only this
            // slice refers to it until the protection change below.
            let memory = unsafe { std::slice::from_raw_parts_mut(code.cast::<u8>(), len) };
            memory.fill(0xcc); // int3: a jump into padding traps
            for (i, name) in names.into_iter().enumerate() {
                let index = u32::try_from(first_index + i).map_err(|_| "too many stubs")?;
                let at = i * STUB_SIZE;
                let stub = &mut memory[at..at + STUB_SIZE];
                stub[0] = 0xbf; // mov edi, imm32
                stub[1..5].copy_from_slice(&index.to_le_bytes());
                stub[5..7].copy_from_slice(&[0x48, 0xb8]); // movabs rax, imm64
                stub[7..15].copy_from_slice(&handler.to_le_bytes());
                stub[15..17].copy_from_slice(&[0xff, 0xe0]); // jmp rax
                entries.push((name, code as u64 + at as u64));
            }
        }
        // SAFETY: our own mapping, page aligned, no references to it remain.
        unsafe { mm::mprotect(code, len, MprotectFlags::READ | MprotectFlags::EXEC) }
            .map_err(|e| format!("couldn't make the stubs executable: {e}"))?;

        Ok(Self { code, len, entries })
    }

    /// (name, address) of every stub in this set.
    pub fn addresses(&self) -> impl Iterator<Item = (&str, u64)> {
        self.entries.iter().map(|(n, a)| (n.as_str(), *a))
    }
}

impl Drop for Stubs {
    fn drop(&mut self) {
        if !self.code.is_null() {
            // SAFETY: exactly the mapping made in `new`.
            let _ = unsafe { mm::munmap(self.code, self.len) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stub_reports_which_function_was_called() {
        let stubs = Stubs::new(vec!["first".into(), "pthread_mutex_lock".into()]).unwrap();
        let (name, address) = stubs.addresses().nth(1).unwrap();
        assert_eq!(name, "pthread_mutex_lock");

        RETURN_INSTEAD_OF_ABORT.store(true, Ordering::Relaxed);
        // SAFETY (test): `address` is the start of a generated stub in live,
        // executable memory; it follows the C calling convention and, in
        // test mode, returns 0.
        let call: extern "C" fn(u64, u64) -> u64 = unsafe { std::mem::transmute(address) };
        let result = call(11, 22);
        RETURN_INSTEAD_OF_ABORT.store(false, Ordering::Relaxed);

        assert_eq!(result, 0);
        assert_eq!(
            LAST_CALLED.lock().unwrap().as_deref(),
            Some("pthread_mutex_lock")
        );
    }
}

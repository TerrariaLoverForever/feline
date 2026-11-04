// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! File access diagnostics (`FELINE_FILE_LOG=1`): logs each path the engine
//! fails to open, stat or access (once per path), to find files it expects
//! on Android (certificate stores, system folders, …).
//!
//! Without the variable these functions aren't used at all: `open`, `fopen`,
//! `stat`, `access` and `opendir` are forwarded to glibc as before (see
//! `imports.rs` for why each is compatible). With it, they are the same
//! glibc calls plus the log.
//!
//! # `unsafe` in this module
//!
//! Passing the engine's C strings and buffers to glibc.

#![allow(unsafe_code)]

use std::collections::HashSet;
use std::ffi::{CStr, c_char, c_int, c_uint, c_void};
use std::sync::{Mutex, OnceLock};

fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("FELINE_FILE_LOG").is_some())
}

/// Logs a failed call on `path` (once per function and path).
fn failed(function: &str, path: *const c_char) {
    let errno = std::io::Error::last_os_error();
    if path.is_null() {
        return;
    }
    // SAFETY: the engine's NUL-terminated path.
    let text = unsafe { CStr::from_ptr(path) }
        .to_string_lossy()
        .into_owned();
    static SEEN: Mutex<Option<HashSet<String>>> = Mutex::new(None);
    let mut seen = SEEN.lock().unwrap_or_else(|p| p.into_inner());
    if seen
        .get_or_insert_with(HashSet::new)
        .insert(format!("{function} {text}"))
    {
        eprintln!("feline-runtime: {function}(\"{text}\") failed: {errno}");
    }
    // Keep errno as glibc left it for the engine.
    // SAFETY: this thread's errno.
    unsafe { *libc::__errno_location() = errno.raw_os_error().unwrap_or(0) };
}

/// `int open(path, flags, mode)`: the mode is only read by the kernel when
/// O_CREAT/O_TMPFILE is set, as with C's variadic `open`.
///
/// # Safety
/// As C's.
pub unsafe extern "C" fn open(path: *const c_char, flags: c_int, mode: c_uint) -> c_int {
    // SAFETY: as C's.
    let fd = unsafe { libc::open(path, flags, mode) };
    if fd < 0 {
        failed("open", path);
    }
    fd
}

/// # Safety
/// As C's.
pub unsafe extern "C" fn fopen(path: *const c_char, mode: *const c_char) -> *mut libc::FILE {
    // SAFETY: as C's.
    let file = unsafe { libc::fopen(path, mode) };
    if file.is_null() {
        failed("fopen", path);
    }
    file
}

/// # Safety
/// As C's (`struct stat` has the same layout in both, see imports.rs).
pub unsafe extern "C" fn stat(path: *const c_char, buf: *mut libc::stat) -> c_int {
    // SAFETY: as C's.
    let result = unsafe { libc::stat(path, buf) };
    if result != 0 {
        failed("stat", path);
    }
    result
}

/// # Safety
/// As C's.
pub unsafe extern "C" fn access(path: *const c_char, mode: c_int) -> c_int {
    // SAFETY: as C's.
    let result = unsafe { libc::access(path, mode) };
    if result != 0 {
        failed("access", path);
    }
    result
}

/// # Safety
/// As C's.
pub unsafe extern "C" fn opendir(path: *const c_char) -> *mut c_void {
    // SAFETY: as C's.
    let dir = unsafe { libc::opendir(path) }.cast::<c_void>();
    if dir.is_null() {
        failed("opendir", path);
    }
    dir
}

pub fn implementation(name: &str) -> Option<u64> {
    if !enabled() {
        return None;
    }
    let address = match name {
        "open" => open as unsafe extern "C" fn(*const c_char, c_int, c_uint) -> c_int as usize,
        "fopen" => {
            fopen as unsafe extern "C" fn(*const c_char, *const c_char) -> *mut libc::FILE as usize
        }
        "stat" => stat as unsafe extern "C" fn(*const c_char, *mut libc::stat) -> c_int as usize,
        "access" => access as unsafe extern "C" fn(*const c_char, c_int) -> c_int as usize,
        "opendir" => opendir as unsafe extern "C" fn(*const c_char) -> *mut c_void as usize,
        _ => return None,
    };
    Some(address as u64)
}

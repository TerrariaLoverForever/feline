// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! Memory the engine frees: `realpath`, `getcwd` and `vasprintf` results.
//!
//! The engine has its own allocator built in (mimalloc v3, statically
//! linked); it imports no `malloc` or `free`. A few libc functions return a
//! buffer that the caller frees, and the engine frees those with its own
//! `free`, which looks the pointer up in mimalloc's page map. A buffer from
//! glibc's `malloc` isn't in that map: if the map has no entry for that part
//! of the address space, the lookup reads through a null table and crashes.
//! This was the `HttpClient` thread's crash: `realpath(path, NULL)` then
//! `free` (engine offsets 0x232919c and 0x23291c5). On Android it survives
//! when bionic's heap happens to share address ranges with mimalloc's.
//!
//! So these functions call glibc, then copy the result into a buffer from
//! the engine's own `malloc`. The engine doesn't export that function. For
//! engine 2.742.0.1059 it's at offset 0x1e15298: `operator new` (0x1e15256)
//! calls it and throws `bad_alloc` when it returns NULL. Before using it,
//! Feline checks its first bytes and that call. If either doesn't match (a
//! different engine), the old glibc behaviour stays and a warning says so.
//!
//! # `unsafe` in this module
//!
//! Reading the engine's code, calling its `malloc`, and copying the
//! engine's C strings and `va_list`s to glibc.

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)] // the engine's contract: valid C arguments

use std::ffi::{VaList, c_char, c_int, c_void};
use std::sync::OnceLock;

/// Engine 2.742.0.1059: `malloc`, and `operator new`'s call to it.
const MALLOC_OFFSET: u64 = 0x1e15298;
const NEW_CALL_OFFSET: u64 = 0x1e1525c;
/// `push rbp; mov rbp,rsp; push r15..r12; push rbx; sub rsp,0x58; mov rbx,rdi`.
const MALLOC_PROLOGUE: [u8; 20] = [
    0x55, 0x48, 0x89, 0xe5, 0x41, 0x57, 0x41, 0x56, 0x41, 0x55, 0x41, 0x54, 0x53, 0x48, 0x83, 0xec,
    0x58, 0x48, 0x89, 0xfb,
];

type Malloc = unsafe extern "C" fn(usize) -> *mut c_void;

unsafe extern "C" {
    fn vasprintf(strp: *mut *mut c_char, format: *const c_char, args: VaList) -> c_int;
}

/// The engine's `malloc`, if this engine has it where Feline expects.
fn engine_malloc() -> Option<Malloc> {
    static FOUND: OnceLock<Option<Malloc>> = OnceLock::new();
    *FOUND.get_or_init(|| {
        let engine = crate::dl::engine()?;
        let found = find(engine.bias, engine.end - engine.start);
        if found.is_none() {
            eprintln!(
                "feline-runtime: warning: the engine's malloc isn't where Feline expects \
                 (a different engine version?); realpath/getcwd/vasprintf results may \
                 crash when the engine frees them"
            );
        }
        found
    })
}

/// Checks the code at `bias + MALLOC_OFFSET` in an image `len` bytes long.
fn find(bias: u64, len: u64) -> Option<Malloc> {
    if MALLOC_OFFSET + MALLOC_PROLOGUE.len() as u64 > len || NEW_CALL_OFFSET + 5 > len {
        return None;
    }
    // SAFETY: both ranges are inside the loaded engine image (checked
    // above), whose code segment is mapped readable.
    let (prologue, call) = unsafe {
        (
            std::slice::from_raw_parts((bias + MALLOC_OFFSET) as *const u8, MALLOC_PROLOGUE.len()),
            std::slice::from_raw_parts((bias + NEW_CALL_OFFSET) as *const u8, 5),
        )
    };
    let relative = i32::from_le_bytes([call[1], call[2], call[3], call[4]]);
    let target = (NEW_CALL_OFFSET + 5).wrapping_add_signed(i64::from(relative));
    if prologue != MALLOC_PROLOGUE || call[0] != 0xe8 || target != MALLOC_OFFSET {
        return None;
    }
    // SAFETY: the engine's malloc(size_t) -> void*, checked above.
    Some(unsafe { std::mem::transmute::<u64, Malloc>(bias + MALLOC_OFFSET) })
}

/// Moves a glibc-allocated buffer of `size` bytes into engine memory.
/// Returns the glibc pointer unchanged when the engine's malloc is unknown.
unsafe fn into_engine(glibc: *mut c_char, size: usize) -> *mut c_char {
    if glibc.is_null() {
        return glibc;
    }
    let Some(malloc) = engine_malloc() else {
        return glibc;
    };
    // SAFETY: `glibc` holds `size` bytes; the new buffer is at least as big.
    unsafe {
        let copy = malloc(size).cast::<c_char>();
        if !copy.is_null() {
            std::ptr::copy_nonoverlapping(glibc, copy, size);
        }
        libc::free(glibc.cast());
        copy
    }
}

/// `char *realpath(const char *path, char *resolved)`.
pub unsafe extern "C" fn realpath(path: *const c_char, resolved: *mut c_char) -> *mut c_char {
    // SAFETY: the engine's path, and its buffer (PATH_MAX bytes) or NULL.
    unsafe {
        let result = libc::realpath(path, resolved);
        if !resolved.is_null() || result.is_null() {
            return result;
        }
        into_engine(result, libc::strlen(result) + 1)
    }
}

/// `char *getcwd(char *buf, size_t size)`: with `buf` NULL, the engine frees
/// the result, so it comes from the engine's malloc (`size` bytes, or just
/// enough when 0, as both C libraries do).
pub unsafe extern "C" fn getcwd(buf: *mut c_char, size: usize) -> *mut c_char {
    // SAFETY: the engine's buffer of `size` bytes, or NULL.
    unsafe {
        if !buf.is_null() {
            return libc::getcwd(buf, size);
        }
        let result = libc::getcwd(std::ptr::null_mut(), 0);
        if result.is_null() {
            return result;
        }
        let len = libc::strlen(result) + 1;
        if size != 0 && len > size {
            libc::free(result.cast());
            *libc::__errno_location() = libc::ERANGE;
            return std::ptr::null_mut();
        }
        let Some(malloc) = engine_malloc() else {
            return result;
        };
        let copy = malloc(size.max(len)).cast::<c_char>();
        if !copy.is_null() {
            std::ptr::copy_nonoverlapping(result, copy, len);
        }
        libc::free(result.cast());
        copy
    }
}

/// `int vasprintf(char **strp, const char *format, va_list args)`.
pub unsafe extern "C" fn feline_vasprintf(
    strp: *mut *mut c_char,
    format: *const c_char,
    args: VaList,
) -> c_int {
    // SAFETY: the engine's out-pointer, format and arguments.
    unsafe {
        let mut text = std::ptr::null_mut();
        let written = vasprintf(&mut text, format, args);
        if written < 0 {
            return written;
        }
        let copy = into_engine(text, written as usize + 1);
        if copy.is_null() {
            return -1;
        }
        *strp = copy;
        written
    }
}

pub fn implementation(name: &str) -> Option<u64> {
    let address = match name {
        "realpath" => {
            realpath as unsafe extern "C" fn(*const c_char, *mut c_char) -> *mut c_char as usize
        }
        "getcwd" => getcwd as unsafe extern "C" fn(*mut c_char, usize) -> *mut c_char as usize,
        "vasprintf" => {
            feline_vasprintf
                as unsafe extern "C" fn(*mut *mut c_char, *const c_char, VaList) -> c_int
                as usize
        }
        _ => return None,
    };
    Some(address as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_different_engine_is_not_trusted() {
        let image = vec![0u8; (MALLOC_OFFSET + 64) as usize];
        assert!(find(image.as_ptr() as u64, image.len() as u64).is_none());
        assert!(find(image.as_ptr() as u64, 16).is_none());
    }

    #[test]
    fn the_expected_code_is_recognised() {
        let mut image = vec![0u8; (MALLOC_OFFSET + 64) as usize];
        let m = MALLOC_OFFSET as usize;
        image[m..m + MALLOC_PROLOGUE.len()].copy_from_slice(&MALLOC_PROLOGUE);
        let n = NEW_CALL_OFFSET as usize;
        let relative = (MALLOC_OFFSET as i64 - (NEW_CALL_OFFSET as i64 + 5)) as i32;
        image[n] = 0xe8;
        image[n + 1..n + 5].copy_from_slice(&relative.to_le_bytes());
        assert!(find(image.as_ptr() as u64, image.len() as u64).is_some());
    }
}

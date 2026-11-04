// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! stdio for the engine: glibc's, plus bionic's legacy `__sF` streams.
//!
//! `FILE` is opaque to Android apps, so a `FILE*` from `fopen` is simply a
//! glibc `FILE*`. But apps built with older NDKs reach the standard streams
//! as `&__sF[0]`, `&__sF[1]`, `&__sF[2]`: an array of bionic's own `FILE`
//! structures (152 bytes each on 64-bit; AOSP `libc/stdio/local.h`, which
//! notes that NDK apps index into this array directly). glibc has no such
//! array, so Feline owns one (see `imports.rs`) and every stdio function
//! taking a `FILE*` first turns a pointer into it into glibc's
//! `stdin`/`stdout`/`stderr`, then calls glibc's function.
//!
//! # `unsafe` in this module
//!
//! The functions take the engine's C pointers and call glibc's.

#![allow(unsafe_code)]
#![allow(clippy::missing_safety_doc)] // see the note above the wrappers

use std::ffi::{VaList, c_char, c_int, c_long, c_void};
use std::sync::atomic::{AtomicUsize, Ordering};

use libc::FILE;

/// `wint_t`: 32-bit unsigned in both libraries on x86-64.
type WInt = u32;

/// Size of bionic's `struct __sFILE` on 64-bit.
pub const BIONIC_FILE_SIZE: usize = 152;

/// Where Feline's `__sF` array is (0 until the engine is bound).
static SF_BASE: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" {
    static mut stdin: *mut FILE;
    static mut stdout: *mut FILE;
    static mut stderr: *mut FILE;
    fn vfscanf(stream: *mut FILE, format: *const c_char, args: VaList) -> c_int;
    fn vfprintf(stream: *mut FILE, format: *const c_char, args: VaList) -> c_int;
    #[link_name = "getc"]
    fn glibc_getc(stream: *mut FILE) -> c_int;
    #[link_name = "fgetwc"]
    fn glibc_fgetwc(stream: *mut FILE) -> WInt;
    #[link_name = "ungetwc"]
    fn glibc_ungetwc(c: WInt, stream: *mut FILE) -> WInt;
    #[link_name = "fputwc"]
    fn glibc_fputwc(c: libc::wchar_t, stream: *mut FILE) -> WInt;
    fn __fread_chk(ptr: *mut c_void, ptr_len: usize, size: usize, n: usize, f: *mut FILE) -> usize;
}

/// Fills in Feline's `__sF` array (`3 * BIONIC_FILE_SIZE` bytes, as words)
/// as bionic does, `_flags` (offset 16, non-zero = in use) and `_file`
/// (offset 20: fd 0, 1, 2), for code that reads them; then records where
/// it is. The array must stay at this address (it's a leaked/owned block).
pub fn set_sf(array: &mut [u64]) {
    const WORDS: usize = BIONIC_FILE_SIZE / 8;
    for i in 0..3 {
        // Little-endian: the low half is `_flags`, the high half `_file`.
        array[i * WORDS + 2] = 1 | ((i as u64) << 32);
    }
    SF_BASE.store(array.as_ptr() as usize, Ordering::Release);
}

/// The glibc stream for an engine `FILE*`.
pub fn stream(file: *mut FILE) -> *mut FILE {
    let base = SF_BASE.load(Ordering::Acquire);
    let at = file as usize;
    if base != 0 && at >= base && at < base + 3 * BIONIC_FILE_SIZE {
        // SAFETY: glibc's standard stream variables, read only.
        unsafe {
            match (at - base) / BIONIC_FILE_SIZE {
                0 => stdin,
                1 => stdout,
                _ => stderr,
            }
        }
    } else {
        file
    }
}

// One wrapper per stdio function the engine imports that takes a FILE*.
// Safety (for all, hence the allow below): the same contract as the C
// function; the engine's own call with its own arguments, passed to glibc
// unchanged except for the stream.

pub unsafe extern "C" fn fflush(f: *mut FILE) -> c_int {
    unsafe { libc::fflush(if f.is_null() { f } else { stream(f) }) }
}
pub unsafe extern "C" fn fclose(f: *mut FILE) -> c_int {
    unsafe { libc::fclose(stream(f)) }
}
pub unsafe extern "C" fn fread(p: *mut c_void, size: usize, n: usize, f: *mut FILE) -> usize {
    unsafe { libc::fread(p, size, n, stream(f)) }
}
pub unsafe extern "C" fn fwrite(p: *const c_void, size: usize, n: usize, f: *mut FILE) -> usize {
    unsafe { libc::fwrite(p, size, n, stream(f)) }
}
pub unsafe extern "C" fn fseek(f: *mut FILE, offset: c_long, whence: c_int) -> c_int {
    unsafe { libc::fseek(stream(f), offset, whence) }
}
pub unsafe extern "C" fn fseeko(f: *mut FILE, offset: libc::off_t, whence: c_int) -> c_int {
    unsafe { libc::fseeko(stream(f), offset, whence) }
}
pub unsafe extern "C" fn ftell(f: *mut FILE) -> c_long {
    unsafe { libc::ftell(stream(f)) }
}
pub unsafe extern "C" fn ftello(f: *mut FILE) -> libc::off_t {
    unsafe { libc::ftello(stream(f)) }
}
pub unsafe extern "C" fn fgets(s: *mut c_char, n: c_int, f: *mut FILE) -> *mut c_char {
    unsafe { libc::fgets(s, n, stream(f)) }
}
pub unsafe extern "C" fn fputs(s: *const c_char, f: *mut FILE) -> c_int {
    unsafe { libc::fputs(s, stream(f)) }
}
pub unsafe extern "C" fn fputc(c: c_int, f: *mut FILE) -> c_int {
    unsafe { libc::fputc(c, stream(f)) }
}
pub unsafe extern "C" fn getc(f: *mut FILE) -> c_int {
    unsafe { glibc_getc(stream(f)) }
}
pub unsafe extern "C" fn ungetc(c: c_int, f: *mut FILE) -> c_int {
    unsafe { libc::ungetc(c, stream(f)) }
}
pub unsafe extern "C" fn feof(f: *mut FILE) -> c_int {
    unsafe { libc::feof(stream(f)) }
}
pub unsafe extern "C" fn ferror(f: *mut FILE) -> c_int {
    unsafe { libc::ferror(stream(f)) }
}
pub unsafe extern "C" fn clearerr(f: *mut FILE) {
    unsafe { libc::clearerr(stream(f)) }
}
pub unsafe extern "C" fn fileno(f: *mut FILE) -> c_int {
    unsafe { libc::fileno(stream(f)) }
}
pub unsafe extern "C" fn setvbuf(
    f: *mut FILE,
    buf: *mut c_char,
    mode: c_int,
    size: usize,
) -> c_int {
    unsafe { libc::setvbuf(stream(f), buf, mode, size) }
}
pub unsafe extern "C" fn fprintf(f: *mut FILE, format: *const c_char, args: ...) -> c_int {
    unsafe { vfprintf(stream(f), format, args) }
}
pub unsafe extern "C" fn feline_vfprintf(
    f: *mut FILE,
    format: *const c_char,
    args: VaList,
) -> c_int {
    unsafe { vfprintf(stream(f), format, args) }
}
pub unsafe extern "C" fn fscanf(f: *mut FILE, format: *const c_char, args: ...) -> c_int {
    unsafe { vfscanf(stream(f), format, args) }
}
pub unsafe extern "C" fn getwc(f: *mut FILE) -> WInt {
    unsafe { glibc_fgetwc(stream(f)) }
}
pub unsafe extern "C" fn ungetwc(c: WInt, f: *mut FILE) -> WInt {
    unsafe { glibc_ungetwc(c, stream(f)) }
}
pub unsafe extern "C" fn fputwc(c: libc::wchar_t, f: *mut FILE) -> WInt {
    unsafe { glibc_fputwc(c, stream(f)) }
}
pub unsafe extern "C" fn feline_fread_chk(
    p: *mut c_void,
    p_len: usize,
    size: usize,
    n: usize,
    f: *mut FILE,
) -> usize {
    unsafe { __fread_chk(p, p_len, size, n, stream(f)) }
}

pub fn implementation(name: &str) -> Option<u64> {
    type F1 = unsafe extern "C" fn(*mut FILE) -> c_int;
    let address = match name {
        "fflush" => fflush as F1 as usize,
        "fclose" => fclose as F1 as usize,
        "fread" => {
            fread as unsafe extern "C" fn(*mut c_void, usize, usize, *mut FILE) -> usize as usize
        }
        "fwrite" => {
            fwrite as unsafe extern "C" fn(*const c_void, usize, usize, *mut FILE) -> usize as usize
        }
        "fseek" => fseek as unsafe extern "C" fn(*mut FILE, c_long, c_int) -> c_int as usize,
        "fseeko" => fseeko as unsafe extern "C" fn(*mut FILE, libc::off_t, c_int) -> c_int as usize,
        "ftell" => ftell as unsafe extern "C" fn(*mut FILE) -> c_long as usize,
        "ftello" => ftello as unsafe extern "C" fn(*mut FILE) -> libc::off_t as usize,
        "fgets" => {
            fgets as unsafe extern "C" fn(*mut c_char, c_int, *mut FILE) -> *mut c_char as usize
        }
        "fputs" => fputs as unsafe extern "C" fn(*const c_char, *mut FILE) -> c_int as usize,
        "fputc" => fputc as unsafe extern "C" fn(c_int, *mut FILE) -> c_int as usize,
        "getc" => getc as F1 as usize,
        "ungetc" => ungetc as unsafe extern "C" fn(c_int, *mut FILE) -> c_int as usize,
        "feof" => feof as F1 as usize,
        "ferror" => ferror as F1 as usize,
        "clearerr" => clearerr as unsafe extern "C" fn(*mut FILE) as usize,
        "fileno" => fileno as F1 as usize,
        "setvbuf" => {
            setvbuf as unsafe extern "C" fn(*mut FILE, *mut c_char, c_int, usize) -> c_int as usize
        }
        "fprintf" => {
            fprintf as unsafe extern "C" fn(*mut FILE, *const c_char, ...) -> c_int as usize
        }
        "vfprintf" => {
            feline_vfprintf as unsafe extern "C" fn(*mut FILE, *const c_char, VaList) -> c_int
                as usize
        }
        "fscanf" => fscanf as unsafe extern "C" fn(*mut FILE, *const c_char, ...) -> c_int as usize,
        "getwc" => getwc as unsafe extern "C" fn(*mut FILE) -> WInt as usize,
        "ungetwc" => ungetwc as unsafe extern "C" fn(WInt, *mut FILE) -> WInt as usize,
        "fputwc" => fputwc as unsafe extern "C" fn(libc::wchar_t, *mut FILE) -> WInt as usize,
        "__fread_chk" => {
            feline_fread_chk
                as unsafe extern "C" fn(*mut c_void, usize, usize, usize, *mut FILE) -> usize
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
    fn sf_entries_map_to_the_standard_streams() {
        let array: &'static mut [u64] =
            Box::leak(vec![0u64; 3 * BIONIC_FILE_SIZE / 8].into_boxed_slice());
        let base = array.as_ptr() as usize;
        set_sf(array);
        // SAFETY (test): reading glibc's stream variables.
        let (i, o, e) = unsafe { (stdin, stdout, stderr) };
        assert_eq!(stream(base as *mut FILE), i);
        assert_eq!(stream((base + BIONIC_FILE_SIZE) as *mut FILE), o);
        assert_eq!(stream((base + 2 * BIONIC_FILE_SIZE) as *mut FILE), e);
        let other = 0x1234 as *mut FILE;
        assert_eq!(stream(other), other);
        // SAFETY (test): flushing glibc's stdout through its bionic name.
        assert_eq!(unsafe { fflush((base + BIONIC_FILE_SIZE) as *mut FILE) }, 0);
    }
}

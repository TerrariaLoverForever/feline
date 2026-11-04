// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! Feline's own implementations of bionic functions that can't simply be
//! forwarded to glibc, because the two libraries differ (constants, struct
//! layouts, argument order). Each one translates between them, written from
//! the public headers' definitions, not from any other runtime's code.
//!
//! [`implementation`] maps an import name to the function's address; names
//! not listed here are forwarded or become stubs (see `imports.rs`).
//!
//! # `unsafe` in this module
//!
//! Calling glibc (C functions) and setting the thread's `errno`.

#![allow(unsafe_code)]

use std::ffi::{c_char, c_int, c_long, c_void};

use crate::sysconf_table::SYSCONF;

fn set_errno(value: c_int) {
    // SAFETY: __errno_location returns this thread's errno, always valid.
    unsafe { *libc::__errno_location() = value };
}

/// `long sysconf(int name)` with bionic's `_SC_*` numbers, translated to
/// glibc's. Unknown names fail with `EINVAL`, as POSIX says.
pub extern "C" fn sysconf(name: c_int) -> c_long {
    match SYSCONF.iter().find(|(bionic, _)| *bionic == name) {
        // SAFETY: sysconf only reads system information.
        Some((_, glibc)) => unsafe { libc::sysconf(*glibc) },
        None => {
            set_errno(libc::EINVAL);
            -1
        }
    }
}

// ---------------------------------------------------------------------------
// bionic's fortified functions ("_chk") that glibc doesn't have. The compiler
// passes the size of the destination it knows about; like bionic, these stop
// the process if an operation would overflow it, and otherwise do the
// normal operation (with glibc). They're `unsafe extern "C" fn` because
// their callers (the engine) promise valid pointers, as in C.
// ---------------------------------------------------------------------------

fn fortify_failure(function: &str, what: &str) -> ! {
    eprintln!("feline-runtime: {function}: prevented {what} (fortify check)");
    std::process::abort();
}

/// `size_t __fwrite_chk(const void* buf, size_t size, size_t count, FILE*
/// stream, size_t buf_size)`.
///
/// # Safety
/// As for `fwrite`; `buf_size` is the size the compiler knew for `buf`.
pub unsafe extern "C" fn __fwrite_chk(
    buf: *const c_void,
    size: usize,
    count: usize,
    stream: *mut libc::FILE,
    buf_size: usize,
) -> usize {
    if size.checked_mul(count).is_none_or(|total| total > buf_size) {
        fortify_failure("fwrite", "a read past the end of a buffer");
    }
    // SAFETY: the caller's buffer holds size * count bytes (checked), and
    // `stream` is a glibc FILE* (all stdio is forwarded to glibc).
    unsafe { libc::fwrite(buf, size, count, stream) }
}

/// `size_t __strlen_chk(const char* s, size_t s_len)`.
///
/// # Safety
/// `s` must be a valid C string.
pub unsafe extern "C" fn __strlen_chk(s: *const c_char, s_len: usize) -> usize {
    // SAFETY: the caller passes a valid C string.
    let len = unsafe { libc::strlen(s) };
    if len >= s_len {
        fortify_failure("strlen", "a read past the end of a buffer");
    }
    len
}

/// `char* __strchr_chk(const char* s, int c, size_t s_len)`.
///
/// # Safety
/// `s` must be a valid C string.
pub unsafe extern "C" fn __strchr_chk(s: *const c_char, c: c_int, s_len: usize) -> *mut c_char {
    // SAFETY: as for strchr; the result is checked against the known size.
    let found = unsafe { libc::strchr(s, c) };
    let end = if found.is_null() {
        // Not found: strchr read the whole string, including its NUL.
        // SAFETY: valid C string.
        unsafe { libc::strlen(s) }
    } else {
        found as usize - s as usize
    };
    if end >= s_len {
        fortify_failure("strchr", "a read past the end of a buffer");
    }
    found
}

/// `char* __strncpy_chk2(char* dst, const char* src, size_t n, size_t
/// dst_len, size_t src_len)`.
///
/// # Safety
/// As for `strncpy`.
pub unsafe extern "C" fn __strncpy_chk2(
    dst: *mut c_char,
    src: *const c_char,
    n: usize,
    dst_len: usize,
    src_len: usize,
) -> *mut c_char {
    if n > dst_len {
        fortify_failure("strncpy", "a write past the end of a buffer");
    }
    // strncpy reads up to n bytes of src, stopping at its NUL.
    // SAFETY: the caller passes valid pointers; `strnlen` reads at most n.
    if n > src_len && unsafe { libc::strnlen(src, n) } >= src_len {
        fortify_failure("strncpy", "a read past the end of a buffer");
    }
    // SAFETY: bounds checked above.
    unsafe { libc::strncpy(dst, src, n) }
}

/// `ssize_t __write_chk(int fd, const void* buf, size_t count, size_t buf_size)`.
///
/// # Safety
/// As for `write`.
pub unsafe extern "C" fn __write_chk(
    fd: c_int,
    buf: *const c_void,
    count: usize,
    buf_size: usize,
) -> isize {
    if count > buf_size {
        fortify_failure("write", "a read past the end of a buffer");
    }
    // SAFETY: the buffer holds `count` bytes (checked).
    unsafe { libc::write(fd, buf, count) }
}

/// `ssize_t __sendto_chk(int fd, const void* buf, size_t len, size_t
/// buf_size, int flags, const struct sockaddr* addr, socklen_t addr_len)`.
///
/// # Safety
/// As for `sendto` (sockaddr is the kernel's layout in both libraries).
pub unsafe extern "C" fn __sendto_chk(
    fd: c_int,
    buf: *const c_void,
    len: usize,
    buf_size: usize,
    flags: c_int,
    addr: *const libc::sockaddr,
    addr_len: libc::socklen_t,
) -> isize {
    if len > buf_size {
        fortify_failure("sendto", "a read past the end of a buffer");
    }
    // SAFETY: the buffer holds `len` bytes (checked).
    unsafe { libc::sendto(fd, buf, len, flags, addr, addr_len) }
}

/// `fd_set` operations: `void __FD_SET_chk(int fd, fd_set* set, size_t
/// set_size)` and friends. `fd_set` is the same 1024-bit array in both.
fn fd_check(function: &str, fd: c_int, set_size: usize) -> (usize, u64) {
    const FD_SETSIZE: c_int = 1024;
    if !(0..FD_SETSIZE).contains(&fd) {
        fortify_failure(
            function,
            "an fd_set access with a file descriptor out of range",
        );
    }
    if set_size < std::mem::size_of::<libc::fd_set>() {
        fortify_failure(function, "an access past the end of an fd_set");
    }
    let fd = fd as usize;
    (fd / 64, 1u64 << (fd % 64))
}

/// # Safety
/// `set` must point to a valid `fd_set` of `set_size` bytes.
#[allow(non_snake_case)] // bionic's exact name
pub unsafe extern "C" fn __FD_SET_chk(fd: c_int, set: *mut u64, set_size: usize) {
    let (word, bit) = fd_check("FD_SET", fd, set_size);
    // SAFETY: word < 16 (fd < 1024), inside the checked fd_set.
    unsafe { *set.add(word) |= bit };
}

/// # Safety
/// `set` must point to a valid `fd_set` of `set_size` bytes.
#[allow(non_snake_case)] // bionic's exact name
pub unsafe extern "C" fn __FD_CLR_chk(fd: c_int, set: *mut u64, set_size: usize) {
    let (word, bit) = fd_check("FD_CLR", fd, set_size);
    // SAFETY: as above.
    unsafe { *set.add(word) &= !bit };
}

/// # Safety
/// `set` must point to a valid `fd_set` of `set_size` bytes.
#[allow(non_snake_case)] // bionic's exact name
pub unsafe extern "C" fn __FD_ISSET_chk(fd: c_int, set: *const u64, set_size: usize) -> c_int {
    let (word, bit) = fd_check("FD_ISSET", fd, set_size);
    // SAFETY: as above.
    c_int::from(unsafe { *set.add(word) } & bit != 0)
}

/// `sighandler_t signal(int signum, sighandler_t handler)`: forwarded to
/// glibc (same signal numbers and handler type), but logged, so it's visible
/// when the engine takes over a signal (e.g. installs its own crash handler).
pub extern "C" fn signal(signum: c_int, handler: libc::sighandler_t) -> libc::sighandler_t {
    eprintln!("feline-runtime: the engine set a handler for signal {signum}");
    // SAFETY: the same call the engine made, passed through unchanged.
    unsafe { libc::signal(signum, handler) }
}

// ---------------------------------------------------------------------------
// Android system properties (`__system_property_get`). Feline describes
// itself honestly: Android 13 (SDK 33) on x86-64, made by "Feline", model
// "Linux"; not a specific real phone. Unknown properties are empty, as on
// Android. Every property asked for is logged.
// ---------------------------------------------------------------------------

/// Android's limit for property values (PROP_VALUE_MAX), including the NUL.
const PROP_VALUE_MAX: usize = 92;

const PROPERTIES: &[(&str, &str)] = &[
    ("ro.build.version.sdk", "33"),
    ("ro.build.version.release", "13"),
    ("ro.product.manufacturer", "Feline"),
    ("ro.product.brand", "Feline"),
    ("ro.product.model", "Linux"),
    ("ro.product.device", "feline"),
    ("ro.product.name", "feline"),
    ("ro.hardware", "feline"),
    ("ro.product.cpu.abi", "x86_64"),
    ("ro.product.cpu.abilist", "x86_64"),
    ("ro.kernel.qemu", "0"),
];

/// `int __system_property_get(const char* name, char* value)`: writes the
/// value (at most 91 characters + NUL) and returns its length.
///
/// # Safety
/// `name` is a valid C string; `value` has room for PROP_VALUE_MAX bytes.
pub unsafe extern "C" fn __system_property_get(name: *const c_char, value: *mut c_char) -> c_int {
    // SAFETY: the engine passes a valid property name.
    let name = unsafe { std::ffi::CStr::from_ptr(name) }.to_string_lossy();
    let found = PROPERTIES
        .iter()
        .find(|(k, _)| *k == name)
        .map(|(_, v)| *v)
        .unwrap_or("");
    eprintln!("feline-runtime: property {name} -> \"{found}\"");
    let bytes = &found.as_bytes()[..found.len().min(PROP_VALUE_MAX - 1)];
    // SAFETY: `value` has room for PROP_VALUE_MAX bytes (Android's contract);
    // we write at most PROP_VALUE_MAX - 1 bytes plus the NUL.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr().cast(), value, bytes.len());
        *value.add(bytes.len()) = 0;
    }
    bytes.len() as c_int
}

// ---------------------------------------------------------------------------
// Thread-specific data, traced (FELINE_CALL_TRACE=1 only): forwarded to
// glibc unchanged, but the first calls log their keys and values.
// ---------------------------------------------------------------------------

fn tls_trace(line: std::fmt::Arguments) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    if CALLS.fetch_add(1, Ordering::Relaxed) < 60 {
        eprintln!("feline-runtime: tls: {line}");
    }
}

/// # Safety
/// As for `pthread_key_create`.
pub unsafe extern "C" fn traced_key_create(
    key: *mut libc::pthread_key_t,
    destructor: Option<unsafe extern "C" fn(*mut c_void)>,
) -> c_int {
    // SAFETY: the engine's own call, passed through.
    let result = unsafe { libc::pthread_key_create(key, destructor) };
    // SAFETY: on success, glibc wrote the key.
    let created = if result == 0 {
        (unsafe { *key }) as i64
    } else {
        -1
    };
    tls_trace(format_args!(
        "pthread_key_create -> {result}, key {created}"
    ));
    result
}

/// The last `pthread_getspecific` key and result, and per key how often it
/// returned NULL, for crash reports (atomics only: signal-safe to read).
pub static LAST_GET_KEY: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(u32::MAX);
pub static LAST_GET_VALUE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
pub static NULL_GETS: [std::sync::atomic::AtomicU64; 64] =
    [const { std::sync::atomic::AtomicU64::new(0) }; 64];

pub extern "C" fn traced_getspecific(key: libc::pthread_key_t) -> *mut c_void {
    use std::sync::atomic::Ordering;
    // SAFETY: the engine's own call, passed through.
    let value = unsafe { libc::pthread_getspecific(key) };
    LAST_GET_KEY.store(key, Ordering::Relaxed);
    LAST_GET_VALUE.store(value as usize, Ordering::Relaxed);
    if value.is_null() && (key as usize) < NULL_GETS.len() {
        NULL_GETS[key as usize].fetch_add(1, Ordering::Relaxed);
    }
    tls_trace(format_args!("pthread_getspecific(key {key}) -> {value:p}"));
    value
}

/// # Safety
/// As for `pthread_setspecific`.
pub unsafe extern "C" fn traced_setspecific(
    key: libc::pthread_key_t,
    value: *const c_void,
) -> c_int {
    // SAFETY: the engine's own call, passed through.
    let result = unsafe { libc::pthread_setspecific(key, value) };
    tls_trace(format_args!(
        "pthread_setspecific(key {key}, {value:p}) -> {result}"
    ));
    result
}

/// The address of Feline's implementation of `name`, if there is one.
pub fn implementation(name: &str) -> Option<u64> {
    if let Some(address) = crate::dl::implementation(name) {
        return Some(address);
    }
    if let Some(address) = crate::liblog::implementation(name) {
        return Some(address);
    }
    if let Some(address) = crate::looper::implementation(name) {
        return Some(address);
    }
    if let Some(address) = crate::assets::implementation(name) {
        return Some(address);
    }
    if let Some(address) = crate::window::implementation(name) {
        return Some(address);
    }
    if let Some(address) = crate::sem::implementation(name) {
        return Some(address);
    }
    if crate::trace::enabled() {
        type KeyCreate = unsafe extern "C" fn(
            *mut libc::pthread_key_t,
            Option<unsafe extern "C" fn(*mut c_void)>,
        ) -> c_int;
        let traced = match name {
            "pthread_key_create" => Some(traced_key_create as KeyCreate as usize),
            "pthread_getspecific" => Some(
                traced_getspecific as extern "C" fn(libc::pthread_key_t) -> *mut c_void as usize,
            ),
            "pthread_setspecific" => Some(
                traced_setspecific
                    as unsafe extern "C" fn(libc::pthread_key_t, *const c_void) -> c_int
                    as usize,
            ),
            _ => None,
        };
        if let Some(address) = traced {
            return Some(address as u64);
        }
    }
    type FdOp = unsafe extern "C" fn(c_int, *mut u64, usize);
    let address = match name {
        "sysconf" => sysconf as extern "C" fn(c_int) -> c_long as usize,
        "__system_property_get" => __system_property_get as *const () as usize,
        "signal" => {
            signal as extern "C" fn(c_int, libc::sighandler_t) -> libc::sighandler_t as usize
        }
        "__fwrite_chk" => {
            __fwrite_chk
                as unsafe extern "C" fn(
                    *const c_void,
                    usize,
                    usize,
                    *mut libc::FILE,
                    usize,
                ) -> usize as usize
        }
        "__strlen_chk" => {
            __strlen_chk as unsafe extern "C" fn(*const c_char, usize) -> usize as usize
        }
        "__strchr_chk" => {
            __strchr_chk as unsafe extern "C" fn(*const c_char, c_int, usize) -> *mut c_char
                as usize
        }
        "__strncpy_chk2" => {
            __strncpy_chk2
                as unsafe extern "C" fn(
                    *mut c_char,
                    *const c_char,
                    usize,
                    usize,
                    usize,
                ) -> *mut c_char as usize
        }
        "__write_chk" => {
            __write_chk as unsafe extern "C" fn(c_int, *const c_void, usize, usize) -> isize
                as usize
        }
        "__sendto_chk" => {
            __sendto_chk
                as unsafe extern "C" fn(
                    c_int,
                    *const c_void,
                    usize,
                    usize,
                    c_int,
                    *const libc::sockaddr,
                    libc::socklen_t,
                ) -> isize as usize
        }
        "__FD_SET_chk" => __FD_SET_chk as FdOp as usize,
        "__FD_CLR_chk" => __FD_CLR_chk as FdOp as usize,
        "__FD_ISSET_chk" => {
            __FD_ISSET_chk as unsafe extern "C" fn(c_int, *const u64, usize) -> c_int as usize
        }
        _ => return None,
    };
    Some(address as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// bionic's numbers, from AOSP's bits/sysconf.h.
    const BIONIC_SC_PAGESIZE: c_int = 0x27;
    const BIONIC_SC_NPROCESSORS_ONLN: c_int = 0x61;

    #[test]
    fn sysconf_translates_bionic_names() {
        // SAFETY (test): plain glibc queries.
        let (page, cpus) = unsafe {
            (
                libc::sysconf(libc::_SC_PAGESIZE),
                libc::sysconf(libc::_SC_NPROCESSORS_ONLN),
            )
        };
        assert_eq!(sysconf(BIONIC_SC_PAGESIZE), page);
        assert_eq!(sysconf(BIONIC_SC_NPROCESSORS_ONLN), cpus);
        assert_eq!(sysconf(0x7fff_0000), -1, "unknown name");
        // SAFETY (test): reading this thread's errno.
        assert_eq!(unsafe { *libc::__errno_location() }, libc::EINVAL);
    }

    #[test]
    fn fortified_functions_do_the_normal_operation_within_bounds() {
        let text = c"hello";
        // SAFETY (test): valid C strings and buffers with their real sizes.
        unsafe {
            assert_eq!(__strlen_chk(text.as_ptr(), 6), 5);
            assert_eq!(
                __strchr_chk(text.as_ptr(), c_int::from(b'l'), 6),
                text.as_ptr().add(2).cast_mut()
            );
            let mut set = [0u64; 16];
            __FD_SET_chk(70, set.as_mut_ptr(), 128);
            assert_eq!(set[1], 1 << 6);
            assert_eq!(__FD_ISSET_chk(70, set.as_ptr(), 128), 1);
            __FD_CLR_chk(70, set.as_mut_ptr(), 128);
            assert_eq!(set[1], 0);
            let mut dst = [0 as c_char; 8];
            __strncpy_chk2(dst.as_mut_ptr(), text.as_ptr(), 8, 8, 6);
            assert_eq!(std::ffi::CStr::from_ptr(dst.as_ptr()), text);
        }
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! Android's logging library (`liblog`): the engine's own log messages,
//! written to Feline's output as `android-log: <priority>/<tag>: <text>`.
//!
//! `__android_log_print` takes printf-style `...` arguments: stable Rust can
//! receive them and hand them to glibc's `vsnprintf` as a `va_list`, which
//! formats them (the format language is the same in both libraries).
//!
//! # `unsafe` in this module
//!
//! C strings and `va_list`s from the engine, which it promises are valid.

#![allow(unsafe_code)]

use std::ffi::{CStr, VaList, c_char, c_int};

unsafe extern "C" {
    fn vsnprintf(buf: *mut c_char, size: usize, format: *const c_char, args: VaList) -> c_int;
}

fn priority(p: c_int) -> char {
    match p {
        2 => 'V',
        3 => 'D',
        4 => 'I',
        5 => 'W',
        6 => 'E',
        7 => 'F',
        _ => '?',
    }
}

/// # Safety
/// NULL or a valid C string.
unsafe fn text(s: *const c_char) -> String {
    if s.is_null() {
        return String::new();
    }
    // SAFETY: the caller passes a valid C string.
    unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned()
}

fn write(prio: c_int, tag: &str, message: &str) {
    eprintln!(
        "android-log: {}/{tag}: {}",
        priority(prio),
        message.trim_end()
    );
}

/// Formats `format` with `args` (at most 4 KiB, like Android's log line limit).
unsafe fn format(format: *const c_char, args: VaList) -> String {
    let mut buf = [0 as c_char; 4096];
    // SAFETY: `buf` is writable for its length; `format` and `args` come
    // from the engine's printf-style call.
    unsafe { vsnprintf(buf.as_mut_ptr(), buf.len(), format, args) };
    // SAFETY: vsnprintf always NUL-terminates within `buf`.
    unsafe { CStr::from_ptr(buf.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

/// `int __android_log_print(int prio, const char* tag, const char* fmt, ...)`
///
/// # Safety
/// As for printf.
pub unsafe extern "C" fn __android_log_print(
    prio: c_int,
    tag: *const c_char,
    fmt: *const c_char,
    args: ...
) -> c_int {
    // SAFETY: the engine's arguments.
    let (tag, message) = unsafe { (text(tag), format(fmt, args)) };
    write(prio, &tag, &message);
    1
}

/// `int __android_log_write(int prio, const char* tag, const char* text)`
///
/// # Safety
/// Valid C strings.
pub unsafe extern "C" fn __android_log_write(
    prio: c_int,
    tag: *const c_char,
    message: *const c_char,
) -> c_int {
    // SAFETY: the engine's arguments.
    let (tag, message) = unsafe { (text(tag), text(message)) };
    write(prio, &tag, &message);
    1
}

/// `int __android_log_buf_write(int buf_id, int prio, const char* tag, const char* text)`
///
/// # Safety
/// Valid C strings.
pub unsafe extern "C" fn __android_log_buf_write(
    _buf_id: c_int,
    prio: c_int,
    tag: *const c_char,
    message: *const c_char,
) -> c_int {
    // SAFETY: the engine's arguments.
    unsafe { __android_log_write(prio, tag, message) }
}

/// `void __android_log_assert(const char* cond, const char* tag, const char* fmt, ...)`:
/// logs, then stops, as on Android.
///
/// # Safety
/// As for printf (`fmt` may be NULL).
pub unsafe extern "C" fn __android_log_assert(
    cond: *const c_char,
    tag: *const c_char,
    fmt: *const c_char,
    args: ...
) -> ! {
    // SAFETY: the engine's arguments.
    let (cond, tag) = unsafe { (text(cond), text(tag)) };
    let message = if fmt.is_null() {
        format!("assertion failed: {cond}")
    } else {
        // SAFETY: the engine's printf-style arguments.
        unsafe { format(fmt, args) }
    };
    write(7, &tag, &message);
    std::process::abort();
}

pub fn implementation(name: &str) -> Option<u64> {
    type Write = unsafe extern "C" fn(c_int, *const c_char, *const c_char) -> c_int;
    let address = match name {
        "__android_log_print" => __android_log_print as *const () as usize,
        "__android_log_write" => __android_log_write as Write as *const () as usize,
        "__android_log_buf_write" => __android_log_buf_write as *const () as usize,
        "__android_log_assert" => __android_log_assert as *const () as usize,
        _ => return None,
    };
    Some(address as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_printf_style_messages() {
        unsafe extern "C" fn render(out: *mut String, fmt: *const c_char, args: ...) {
            // SAFETY (test): a printf-style call with matching arguments, and
            // `out` points to a live String.
            unsafe { *out = format(fmt, args) };
        }
        let mut out = String::new();
        // SAFETY (test): the format matches the arguments.
        unsafe {
            render(
                &mut out,
                c"%s has %d lives, %.1f left".as_ptr(),
                c"a cat".as_ptr(),
                9,
                8.5f64,
            )
        };
        assert_eq!(out, "a cat has 9 lives, 8.5 left");
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! Finding the host's functions and variables that imports are forwarded to:
//! glibc (`libc.so.6`, `libm.so.6`) and the system's OpenGL ES library
//! (`libGLESv2.so.2`) and EGL (`libEGL.so.1`), usually glvnd, which
//! dispatches to the GPU driver.
//!
//! Each library is opened by name and searched on its own, never "the whole
//! process", so an import can't accidentally bind to a same-named symbol in
//! Feline itself or in another library.
//!
//! # `unsafe` in this module
//!
//! `dlopen` and `dlsym` are C functions. They're called with valid
//! NUL-terminated names; the handles they return live as long as the
//! process (libraries are never closed), so addresses stay valid.

#![allow(unsafe_code)]

use std::ffi::{CString, c_void};

pub struct Host {
    libc: *mut c_void,
    libm: *mut c_void,
    gles: *mut c_void,
    egl: *mut c_void,
}

fn open(name: &str) -> *mut c_void {
    let name = CString::new(name).expect("library names have no NUL");
    // SAFETY: a valid C string; RTLD_NOW resolves everything up front, and
    // RTLD_LOCAL keeps the library's symbols out of the global namespace.
    unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) }
}

fn lookup(handle: *mut c_void, name: &str) -> Option<u64> {
    if handle.is_null() {
        return None;
    }
    let name = CString::new(name).ok()?;
    // SAFETY: `handle` came from a successful dlopen and is never closed;
    // `name` is a valid C string.
    let address = unsafe { libc::dlsym(handle, name.as_ptr()) };
    (!address.is_null()).then_some(address as u64)
}

impl Host {
    /// Opens the host libraries. A missing one simply forwards nothing
    /// (those imports become stubs).
    pub fn open() -> Self {
        Self {
            libc: open("libc.so.6"),
            libm: open("libm.so.6"),
            gles: open("libGLESv2.so.2"),
            egl: open("libEGL.so.1"),
        }
    }

    /// A glibc function or variable (libc first, then libm: some math
    /// functions such as `frexp` live in libc).
    pub fn libc(&self, name: &str) -> Option<u64> {
        lookup(self.libc, name).or_else(|| lookup(self.libm, name))
    }

    /// An OpenGL ES function.
    pub fn gles(&self, name: &str) -> Option<u64> {
        lookup(self.gles, name)
    }

    /// An EGL function (the ones that differ by platform are Feline's own,
    /// in `egl.rs`).
    pub fn egl(&self, name: &str) -> Option<u64> {
        lookup(self.egl, name)
    }

    pub fn has_gles(&self) -> bool {
        !self.gles.is_null()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_glibc_functions_and_variables() {
        let host = Host::open();
        assert!(host.libc("strlen").is_some());
        assert!(host.libc("__errno_location").is_some());
        assert!(host.libc("environ").is_some(), "a variable");
        assert!(host.libc("cosf").is_some(), "libm");
        assert!(host.libc("feline_no_such_function").is_none());
    }
}

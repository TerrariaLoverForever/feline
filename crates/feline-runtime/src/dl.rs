// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! The engine's `dl*` imports: loading other libraries and finding code.
//!
//! - `dlopen`: Android's system libraries whose functions Feline provides
//!   (libc, libandroid, EGL, …) get a handle; anything else is **logged**
//!   (that list says which libraries the engine needs, e.g. Vulkan or
//!   audio) and answered honestly with "not available".
//! - `dlsym`: on that handle, the function an import of the same name is
//!   bound to; otherwise "not found". Every lookup is logged.
//! - `dlclose`, `dlerror`: the matching operations, with a per-thread error
//!   message as in any C library.
//! - `dladdr`: answers for addresses inside the engine; others go to glibc.
//!   `Dl_info` has the same four fields in both libraries.
//! - `dl_iterate_phdr`: reports the engine first (C++ exception unwinding
//!   uses it to find the engine's unwind tables), then glibc's libraries.
//!   The first four fields of `dl_phdr_info` are the same in both.
//!
//! # `unsafe` in this module
//!
//! These are C functions called by the engine with C pointers; the engine
//! promises they're valid, as in C. Writing results through them and
//! calling glibc's versions needs `unsafe`.

#![allow(unsafe_code)]

use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::sync::OnceLock;

/// Where the engine is in memory, set once before its code runs.
#[derive(Debug, Clone, Copy)]
pub struct Engine {
    pub bias: u64,
    pub start: u64,
    pub end: u64,
    /// Real address of the program headers, and how many there are.
    pub phdr: u64,
    pub phnum: u16,
}

static ENGINE: OnceLock<Engine> = OnceLock::new();
static ENGINE_NAME: &CStr = c"libroblox.so";

pub fn set_engine(engine: Engine) {
    let _ = ENGINE.set(engine);
}

/// Where the engine is, once it's loaded.
pub fn engine() -> Option<Engine> {
    ENGINE.get().copied()
}

thread_local! {
    /// The pending error for `dlerror`, and the last one handed out (kept
    /// alive, because the caller may still be reading it).
    static PENDING: RefCell<Option<CString>> = const { RefCell::new(None) };
    static SHOWN: RefCell<Option<CString>> = const { RefCell::new(None) };
}

fn fail(message: String) {
    PENDING.with(|p| *p.borrow_mut() = CString::new(message).ok());
}

/// Android system libraries whose functions Feline provides: `dlopen` of
/// these returns a handle, and `dlsym` on it answers with the same function
/// an import of that name is bound to (see `imports::resolve_function`).
const SYSTEM_LIBRARIES: &[&str] = &[
    "libc.so",
    "libm.so",
    "libdl.so",
    "liblog.so",
    "libandroid.so",
    "libEGL.so",
    "libGLESv2.so",
    "libGLESv3.so",
];

/// The handle for Feline's system libraries (any non-NULL value works; this
/// one points at a static so it can't collide with a real address).
static SYSTEM_HANDLE: u8 = 0;

fn system_handle() -> *mut c_void {
    std::ptr::from_ref(&SYSTEM_HANDLE).cast_mut().cast()
}

/// The handle for `libvulkan.so`.
static VULKAN_HANDLE: u8 = 0;

fn vulkan_handle() -> *mut c_void {
    std::ptr::from_ref(&VULKAN_HANDLE).cast_mut().cast()
}

/// # Safety
/// `name` is NULL or a valid C string.
pub unsafe extern "C" fn dlopen(name: *const c_char, _flags: c_int) -> *mut c_void {
    let name = if name.is_null() {
        "(the main program)".to_owned()
    } else {
        // SAFETY: the caller passes a valid C string.
        unsafe { CStr::from_ptr(name) }
            .to_string_lossy()
            .into_owned()
    };
    // "libc.so", "/system/lib64/libc.so", …
    let base = name.rsplit('/').next().unwrap_or(&name);
    if SYSTEM_LIBRARIES.contains(&base) {
        eprintln!("feline-runtime: the engine opened `{name}` (Feline's)");
        return system_handle();
    }
    // Vulkan, on the desktop's driver (see vulkan.rs; FELINE_VULKAN=1).
    if (base == "libvulkan.so" || base == "libvulkan.so.1") && crate::vulkan::enabled() {
        eprintln!("feline-runtime: the engine opened `{name}` (Feline's, on the desktop's Vulkan)");
        return vulkan_handle();
    }
    eprintln!(
        "feline-runtime: the engine asked for the library `{name}`, which Feline doesn't provide yet"
    );
    fail(format!("{name}: not available in Feline (yet)"));
    std::ptr::null_mut()
}

/// # Safety
/// `symbol` is a valid C string.
pub unsafe extern "C" fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void {
    // SAFETY: the caller passes a valid C string.
    let symbol = unsafe { CStr::from_ptr(symbol) }.to_string_lossy();
    if handle == vulkan_handle()
        && let Some(address) = crate::vulkan::symbol(&symbol)
    {
        return address as *mut c_void;
    }
    if handle == system_handle()
        && let Some(address) = crate::imports::resolve_function(&symbol, &crate::host::Host::open())
    {
        eprintln!("feline-runtime: the engine looked up `{symbol}` (found)");
        return address as *mut c_void;
    }
    eprintln!(
        "feline-runtime: the engine looked up `{symbol}` with dlsym, which Feline doesn't answer yet"
    );
    fail(format!("{symbol}: not found"));
    std::ptr::null_mut()
}

pub extern "C" fn dlclose(_handle: *mut c_void) -> c_int {
    0
}

pub extern "C" fn dlerror() -> *const c_char {
    let pending = PENDING.with(|p| p.borrow_mut().take());
    SHOWN.with(|s| {
        let mut shown = s.borrow_mut();
        *shown = pending;
        shown.as_ref().map_or(std::ptr::null(), |c| c.as_ptr())
    })
}

/// # Safety
/// `info` points to a writable `Dl_info`.
pub unsafe extern "C" fn dladdr(address: *const c_void, info: *mut libc::Dl_info) -> c_int {
    let a = address as u64;
    if let Some(engine) = ENGINE.get()
        && a >= engine.start
        && a < engine.end
    {
        // SAFETY: the caller passes a writable Dl_info.
        unsafe {
            (*info).dli_fname = ENGINE_NAME.as_ptr();
            (*info).dli_fbase = engine.bias as *mut c_void;
            (*info).dli_sname = std::ptr::null();
            (*info).dli_saddr = std::ptr::null_mut();
        }
        return 1;
    }
    // SAFETY: same contract as glibc's dladdr.
    unsafe { libc::dladdr(address, info) }
}

type PhdrCallback = unsafe extern "C" fn(*mut libc::dl_phdr_info, usize, *mut c_void) -> c_int;

/// # Safety
/// `callback` is a valid function; `data` is passed through unchanged.
pub unsafe extern "C" fn dl_iterate_phdr(
    callback: Option<PhdrCallback>,
    data: *mut c_void,
) -> c_int {
    let Some(callback) = callback else { return 0 };
    if let Some(engine) = ENGINE.get() {
        // SAFETY: an all-zero dl_phdr_info is valid (no TLS module).
        let mut info: libc::dl_phdr_info = unsafe { std::mem::zeroed() };
        info.dlpi_addr = engine.bias;
        info.dlpi_name = ENGINE_NAME.as_ptr();
        info.dlpi_phdr = engine.phdr as *const libc::Elf64_Phdr;
        info.dlpi_phnum = engine.phnum;
        // SAFETY: the engine's callback, with a valid info for the engine.
        let result =
            unsafe { callback(&mut info, std::mem::size_of::<libc::dl_phdr_info>(), data) };
        if result != 0 {
            return result;
        }
    }
    // SAFETY: glibc's dl_iterate_phdr with the same callback and data.
    unsafe { libc::dl_iterate_phdr(Some(callback), data) }
}

/// The address of Feline's implementation of `name`, if it's a `dl*` function.
pub fn implementation(name: &str) -> Option<u64> {
    let address = match name {
        "dlopen" => dlopen as unsafe extern "C" fn(*const c_char, c_int) -> *mut c_void as usize,
        "dlsym" => {
            dlsym as unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void as usize
        }
        "dlclose" => dlclose as extern "C" fn(*mut c_void) -> c_int as usize,
        "dlerror" => dlerror as extern "C" fn() -> *const c_char as usize,
        "dladdr" => {
            dladdr as unsafe extern "C" fn(*const c_void, *mut libc::Dl_info) -> c_int as usize
        }
        "dl_iterate_phdr" => {
            dl_iterate_phdr as unsafe extern "C" fn(Option<PhdrCallback>, *mut c_void) -> c_int
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
    fn dlerror_reports_once_then_clears() {
        // SAFETY (test): a valid C string.
        let handle = unsafe { dlopen(c"libcamera2ndk.so".as_ptr(), 0) };
        assert!(handle.is_null());
        let message = dlerror();
        assert!(!message.is_null());
        // SAFETY (test): dlerror returned a live C string.
        let text = unsafe { CStr::from_ptr(message) }
            .to_string_lossy()
            .into_owned();
        assert!(text.contains("libcamera2ndk.so"), "{text}");
        assert!(dlerror().is_null(), "cleared after being read");
    }
}

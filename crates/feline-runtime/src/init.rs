// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! Running the engine's constructors (M3 step 4): the first time any of the
//! library's code runs.
//!
//! A library's `DT_INIT_ARRAY` lists functions that must run once after it
//! is loaded and relocated (C++ global constructors and similar). Android's
//! loader calls each as `f(argc, argv, envp)` and skips entries that are 0
//! or -1; so do we.
//!
//! When one of them calls a function Feline doesn't provide yet, its stub
//! names the function and stops (see `stubs.rs`). If one crashes instead
//! (bad memory access, illegal instruction, abort), a signal handler reports
//! which constructor it was. Either way the result is the next item on the
//! to-do list.
//!
//! # `unsafe` in this module
//!
//! Calling the engine's functions (which we can't check: that's the point
//! of this step), installing signal handlers, and `_exit`. The signal handler
//! only uses async-signal-safe operations (an atomic load and `write(2)`).

#![allow(unsafe_code)]

use std::ffi::{CString, c_char, c_int};
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::elf::{DT_INIT_ARRAY, Elf};
use crate::load::Image;
use crate::stubs;

/// The constructor running right now (1-based; 0 = none), for the signal
/// handler.
static CURRENT: AtomicUsize = AtomicUsize::new(0);
static TOTAL: AtomicUsize = AtomicUsize::new(0);

type Constructor = extern "C" fn(c_int, *const *const c_char, *const *const c_char);

/// Writes a number in decimal into `buf`, returns the used part. No
/// allocation, so it's usable in a signal handler.
fn format_number(mut n: usize, buf: &mut [u8; 20]) -> &[u8] {
    let mut at = buf.len();
    loop {
        at -= 1;
        buf[at] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            return &buf[at..];
        }
    }
}

extern "C" fn on_fatal_signal(signal: c_int) {
    let name: &[u8] = match signal {
        libc::SIGSEGV => b"a bad memory access (SIGSEGV)",
        libc::SIGBUS => b"a bus error (SIGBUS)",
        libc::SIGILL => b"an illegal instruction (SIGILL)",
        libc::SIGFPE => b"an arithmetic error (SIGFPE)",
        libc::SIGABRT => b"abort (SIGABRT)",
        _ => b"a fatal signal",
    };
    let (mut a, mut b) = ([0u8; 20], [0u8; 20]);
    let current = format_number(CURRENT.load(Ordering::Relaxed), &mut a);
    let total = format_number(TOTAL.load(Ordering::Relaxed), &mut b);
    let parts: [&[u8]; 7] = [
        b"feline-runtime: stopped by ",
        name,
        b" in constructor ",
        current,
        b" of ",
        total,
        b"\n",
    ];
    for part in parts {
        // SAFETY: write(2) is async-signal-safe; the buffers are valid.
        unsafe {
            libc::write(2, part.as_ptr().cast(), part.len());
        }
    }
    // SA_RESETHAND restored the default action, so returning re-raises the
    // signal (or abort() raises it again) and the process ends normally for
    // that signal.
}

fn install_crash_reporting() {
    for signal in [
        libc::SIGSEGV,
        libc::SIGBUS,
        libc::SIGILL,
        libc::SIGFPE,
        libc::SIGABRT,
    ] {
        // SAFETY: a zeroed sigaction with a plain handler and SA_RESETHAND
        // is valid; the handler only does async-signal-safe work.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = on_fatal_signal as extern "C" fn(c_int) as usize;
            action.sa_flags = libc::SA_RESETHAND;
            libc::sigemptyset(&mut action.sa_mask);
            libc::sigaction(signal, &action, std::ptr::null_mut());
        }
    }
}

/// Runs every constructor of the loaded library, in order. Returns how many
/// ran (entries 0 and -1 are skipped, as Android does).
pub fn run_constructors(elf: &Elf, image: &Image) -> Result<usize, String> {
    // Tell dladdr/dl_iterate_phdr where the engine is, before its code runs.
    let phdr = elf
        .phdr_vaddr()
        .ok_or("the program headers aren't in a loadable segment")?;
    let phnum = u16::try_from(elf.segments.len()).map_err(|_| "too many program headers")?;
    crate::dl::set_engine(crate::dl::Engine {
        bias: image.bias(),
        start: image.bias(),
        end: image.bias() + image.len() as u64,
        phdr: image.bias() + phdr,
        phnum,
    });
    let count = elf.init_array_len() as usize;
    let Some(array) = elf.dyn_value(DT_INIT_ARRAY) else {
        return Ok(0);
    };

    // argv = ["libroblox.so"], envp = this process's environment. They live
    // until the process ends (the engine may keep the pointers).
    let arg0 = CString::new("libroblox.so").expect("no NUL");
    let argv: &'static [*const c_char] =
        Box::leak(Box::new([arg0.into_raw().cast_const(), std::ptr::null()]));
    let mut env: Vec<*const c_char> = std::env::vars_os()
        .filter_map(|(k, v)| {
            let mut entry = k.into_encoded_bytes();
            entry.push(b'=');
            entry.extend(v.into_encoded_bytes());
            CString::new(entry).ok().map(|c| c.into_raw().cast_const())
        })
        .collect();
    env.push(std::ptr::null());
    let envp: &'static [*const c_char] = Box::leak(env.into_boxed_slice());

    install_crash_reporting();
    TOTAL.store(count, Ordering::Relaxed);
    let mut ran = 0;
    for i in 0..count {
        let address = image
            .read_u64(array + i as u64 * 8)
            .ok_or_else(|| format!("constructor table entry {i} is outside the image"))?;
        if address == 0 || address == u64::MAX {
            continue;
        }
        CURRENT.store(i + 1, Ordering::Relaxed);
        stubs::set_phase(format!("constructor {} of {count}", i + 1));
        // SAFETY: this is the step whose purpose is to run the engine's own
        // code. The address came from its relocated constructor table, and
        // the signature is the one Android's loader uses. Nothing here can
        // check what the code does; stubs and the signal handler report it.
        let constructor: Constructor = unsafe { std::mem::transmute(address) };
        constructor(1, argv.as_ptr(), envp.as_ptr());
        ran += 1;
    }
    CURRENT.store(0, Ordering::Relaxed);
    stubs::set_phase(String::new());
    Ok(ran)
}

/// Calls the engine's `JNI_OnLoad(JavaVM*, void*)`, as Android does right
/// after loading a library that exports it. Returns the JNI version it asks
/// for.
pub fn call_jni_onload(elf: &Elf, image: &Image) -> Result<i32, String> {
    let symbols = elf.symbols().map_err(|e| e.to_string())?;
    let onload = symbols
        .iter()
        .find(|s| s.defined && s.name == "JNI_OnLoad")
        .ok_or("the library has no JNI_OnLoad")?;
    let vm = crate::jni::java_vm()?;
    stubs::set_phase("JNI_OnLoad".to_owned());
    // SAFETY: as for the constructors: the engine's own function at its
    // exported address, with the signature JNI defines.
    let function: extern "C" fn(*mut std::ffi::c_void, *mut std::ffi::c_void) -> c_int =
        unsafe { std::mem::transmute(image.bias() + onload.value) };
    let version = function(vm, std::ptr::null_mut());
    stubs::set_phase(String::new());
    Ok(version)
}

/// Ends the process right away, without running exit handlers: the engine
/// registered some with glibc during its constructors, and they would run
/// more of its code.
pub fn exit_now(code: i32) -> ! {
    use std::io::Write as _;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    // SAFETY: _exit just ends the process.
    unsafe { libc::_exit(code) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_safe_number_formatting() {
        let mut buf = [0u8; 20];
        assert_eq!(format_number(0, &mut buf), b"0");
        assert_eq!(format_number(3678, &mut buf), b"3678");
        assert_eq!(
            format_number(usize::MAX, &mut buf),
            usize::MAX.to_string().as_bytes()
        );
    }
}

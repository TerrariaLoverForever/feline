// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! Signal masks and `sigaction` for the engine, on glibc's.
//!
//! Signal numbers and `SA_*` flags are the kernel's in both libraries, but
//! the structures differ on x86-64 (AOSP `libc/include/bits/signal_types.h`,
//! glibc headers on the target system):
//!
//! - bionic's `sigset_t` is the kernel's 8 bytes (one bit per signal 1–64);
//!   glibc's is 128 bytes, whose first 8 bytes use the same bits.
//! - bionic's `struct sigaction` is `{ int sa_flags; handler; sa_mask;
//!   sa_restorer }` (32 bytes); glibc's is `{ handler; sa_mask (128);
//!   int sa_flags; sa_restorer }`.
//!
//! Handlers then run under glibc: the `siginfo_t` and the machine registers
//! in `ucontext_t` (`uc_mcontext`) are the kernel's layout in both, which is
//! what handlers read. The saved mask inside `ucontext_t` differs in size.
//!
//! # `unsafe` in this module
//!
//! The functions take the engine's C pointers and call glibc's.

#![allow(unsafe_code)]

use std::ffi::c_int;

/// The kernel's SA_RESTORER flag (x86-64), which the C library sets itself.
const SA_RESTORER: c_int = 0x0400_0000;

/// bionic's x86-64 `sigset_t`.
type BionicSigset = u64;

/// bionic's x86-64 `struct sigaction`.
#[repr(C)]
pub struct BionicSigaction {
    sa_flags: c_int,
    sa_handler: usize,
    sa_mask: BionicSigset,
    sa_restorer: usize,
}

fn glibc_set(mask: BionicSigset) -> libc::sigset_t {
    // SAFETY: an all-zero sigset_t is the empty set; its first 8 bytes hold
    // signals 1–64 as in bionic's.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        std::ptr::from_mut(&mut set).cast::<u64>().write(mask);
        set
    }
}

fn bionic_set(set: &libc::sigset_t) -> BionicSigset {
    // SAFETY: reading the first 8 of its 128 bytes.
    unsafe { std::ptr::from_ref(set).cast::<u64>().read() }
}

/// `int sigemptyset(sigset_t*)`
///
/// # Safety
/// `set` points to a bionic `sigset_t`.
pub unsafe extern "C" fn sigemptyset(set: *mut BionicSigset) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { set.write(0) };
    0
}

/// `int sigfillset(sigset_t*)`
///
/// # Safety
/// `set` points to a bionic `sigset_t`.
pub unsafe extern "C" fn sigfillset(set: *mut BionicSigset) -> c_int {
    // SAFETY: the caller's promise.
    unsafe { set.write(u64::MAX) };
    0
}

/// `int pthread_sigmask(how, set, old)` with bionic's 8-byte sets.
///
/// # Safety
/// `set`/`old` are NULL or point to bionic `sigset_t`s.
pub unsafe extern "C" fn pthread_sigmask(
    how: c_int,
    set: *const BionicSigset,
    old: *mut BionicSigset,
) -> c_int {
    // SAFETY: the caller's pointers.
    let new = (!set.is_null()).then(|| glibc_set(unsafe { *set }));
    // SAFETY: an empty set to be filled.
    let mut previous: libc::sigset_t = unsafe { std::mem::zeroed() };
    // SAFETY: valid glibc sets.
    let result = unsafe {
        libc::pthread_sigmask(
            how,
            new.as_ref().map_or(std::ptr::null(), |s| s),
            &mut previous,
        )
    };
    if result == 0 && !old.is_null() {
        // SAFETY: the caller's output.
        unsafe { *old = bionic_set(&previous) };
    }
    result
}

/// `int sigaction(signum, act, oldact)` with bionic's structure; logged, so
/// it's visible when the engine takes over a signal.
///
/// # Safety
/// `act`/`old` are NULL or point to bionic `struct sigaction`s.
pub unsafe extern "C" fn sigaction(
    signum: c_int,
    act: *const BionicSigaction,
    old: *mut BionicSigaction,
) -> c_int {
    let new = (!act.is_null()).then(|| {
        // SAFETY: the caller's structure.
        let a = unsafe { &*act };
        let (kind, what) = match a.sa_handler {
            libc::SIG_DFL => (0, "the default action"),
            libc::SIG_IGN => (1, "ignore"),
            _ => (2, "a handler"),
        };
        // Said once per signal and kind: the engine sets some of them again
        // on every connection (SIGPIPE to ignore filled the log).
        static SAID: [std::sync::atomic::AtomicU8; 65] = [const { std::sync::atomic::AtomicU8::new(0) }; 65];
        let said = usize::try_from(signum).ok().and_then(|n| SAID.get(n));
        if said.is_none_or(|s| s.fetch_or(1 << kind, std::sync::atomic::Ordering::Relaxed) & (1 << kind) == 0) {
            eprintln!("feline-runtime: the engine set signal {signum} to {what}");
        }
        // SAFETY: zeroed is a valid glibc sigaction to fill in.
        let mut g: libc::sigaction = unsafe { std::mem::zeroed() };
        g.sa_sigaction = a.sa_handler;
        g.sa_mask = glibc_set(a.sa_mask);
        // glibc supplies its own restorer; the engine's isn't needed.
        g.sa_flags = a.sa_flags & !SA_RESTORER;
        g
    });
    // SAFETY: zeroed output for glibc.
    let mut previous: libc::sigaction = unsafe { std::mem::zeroed() };
    // SAFETY: valid glibc structures.
    let result = unsafe {
        libc::sigaction(
            signum,
            new.as_ref().map_or(std::ptr::null(), |a| a),
            &mut previous,
        )
    };
    if result == 0 && !old.is_null() {
        // SAFETY: the caller's output.
        unsafe {
            old.write(BionicSigaction {
                sa_flags: previous.sa_flags & !SA_RESTORER,
                sa_handler: previous.sa_sigaction,
                sa_mask: bionic_set(&previous.sa_mask),
                sa_restorer: 0,
            });
        }
    }
    result
}

pub fn implementation(name: &str) -> Option<u64> {
    type Set = unsafe extern "C" fn(*mut BionicSigset) -> c_int;
    let address = match name {
        "sigemptyset" => sigemptyset as Set as usize,
        "sigfillset" => sigfillset as Set as usize,
        "pthread_sigmask" => {
            pthread_sigmask
                as unsafe extern "C" fn(c_int, *const BionicSigset, *mut BionicSigset) -> c_int
                as usize
        }
        "sigaction" => {
            sigaction
                as unsafe extern "C" fn(
                    c_int,
                    *const BionicSigaction,
                    *mut BionicSigaction,
                ) -> c_int as usize
        }
        _ => return None,
    };
    Some(address as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_matches_bionic() {
        assert_eq!(std::mem::size_of::<BionicSigaction>(), 32);
        assert_eq!(std::mem::offset_of!(BionicSigaction, sa_handler), 8);
        assert_eq!(std::mem::offset_of!(BionicSigaction, sa_mask), 16);
    }

    #[test]
    fn sigaction_round_trips_through_glibc() {
        // SIGUSR2 ignored, with SIGUSR1 (10) in the mask; then read back.
        let act = BionicSigaction {
            sa_flags: libc::SA_RESTART,
            sa_handler: libc::SIG_IGN,
            sa_mask: 1 << (libc::SIGUSR1 - 1),
            sa_restorer: 0,
        };
        let mut old = BionicSigaction {
            sa_flags: 0,
            sa_handler: 0,
            sa_mask: 0,
            sa_restorer: 0,
        };
        // SAFETY (test): valid structures.
        unsafe {
            assert_eq!(sigaction(libc::SIGUSR2, &act, std::ptr::null_mut()), 0);
            assert_eq!(sigaction(libc::SIGUSR2, std::ptr::null(), &mut old), 0);
            assert_eq!(sigaction(libc::SIGUSR2, &old, std::ptr::null_mut()), 0);
        }
        assert_eq!(old.sa_handler, libc::SIG_IGN);
        assert_eq!(old.sa_mask, 1 << (libc::SIGUSR1 - 1));
        assert_ne!(old.sa_flags & libc::SA_RESTART, 0);
        // Restore the default for other tests.
        let default = BionicSigaction {
            sa_handler: libc::SIG_DFL,
            ..old
        };
        // SAFETY (test): as above.
        unsafe { sigaction(libc::SIGUSR2, &default, std::ptr::null_mut()) };
    }

    #[test]
    fn masks_round_trip() {
        let mut set = 0;
        // SAFETY (test): a local set.
        unsafe {
            sigfillset(&mut set);
            assert_eq!(set, u64::MAX);
            sigemptyset(&mut set);
        }
        assert_eq!(bionic_set(&glibc_set(0b1010)), 0b1010);
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! POSIX semaphores (`sem_*`) for the engine, on Linux futexes.
//!
//! They can't be forwarded to glibc: bionic's `sem_t` is 16 bytes on x86-64
//! and glibc's is 32, so glibc would write past the engine's semaphore.
//! Feline keeps the count in the first 4 bytes (as bionic does) and sleeps
//! on that word with `futex(2)`.
//!
//! The engine imports `sem_init`, `sem_destroy`, `sem_post` and `sem_wait`.
//!
//! # `unsafe` in this module
//!
//! The functions take the engine's `sem_t*`, and call the futex system call.

#![allow(unsafe_code)]

use std::ffi::{c_int, c_uint};
use std::sync::atomic::{AtomicU32, Ordering};

/// bionic's `SEM_VALUE_MAX`.
const SEM_VALUE_MAX: c_uint = 0x3fff_ffff;

fn set_errno(value: c_int) {
    // SAFETY: __errno_location returns this thread's errno, always valid.
    unsafe { *libc::__errno_location() = value };
}

/// The count word at the start of the engine's 16-byte `sem_t`.
///
/// # Safety
/// `sem` points to a live, 4-byte aligned `sem_t`.
unsafe fn count<'a>(sem: *mut SemT) -> &'a AtomicU32 {
    // SAFETY: the caller's promise; AtomicU32 has u32's layout.
    unsafe { &*sem.cast::<AtomicU32>() }
}

/// The engine's opaque `sem_t` (16 bytes in bionic).
#[repr(C)]
pub struct SemT {
    _bytes: [u32; 4],
}

/// `int sem_init(sem_t*, int pshared, unsigned value)`
///
/// # Safety
/// `sem` points to a 16-byte `sem_t`.
pub unsafe extern "C" fn sem_init(sem: *mut SemT, _pshared: c_int, value: c_uint) -> c_int {
    if sem.is_null() || value > SEM_VALUE_MAX {
        set_errno(libc::EINVAL);
        return -1;
    }
    // SAFETY: as above. The futex calls below don't use FUTEX_PRIVATE_FLAG,
    // so a process-shared semaphore works too.
    unsafe { count(sem) }.store(value, Ordering::Release);
    0
}

/// `int sem_destroy(sem_t*)`: nothing to free.
pub extern "C" fn sem_destroy(_sem: *mut SemT) -> c_int {
    0
}

/// `int sem_post(sem_t*)`: adds one and wakes one waiter.
///
/// # Safety
/// `sem` was set up by `sem_init`.
pub unsafe extern "C" fn sem_post(sem: *mut SemT) -> c_int {
    // SAFETY: as above.
    let word = unsafe { count(sem) };
    let mut value = word.load(Ordering::Relaxed);
    loop {
        if value >= SEM_VALUE_MAX {
            set_errno(libc::EOVERFLOW);
            return -1;
        }
        match word.compare_exchange_weak(value, value + 1, Ordering::Release, Ordering::Relaxed) {
            Ok(_) => break,
            Err(now) => value = now,
        }
    }
    // Simple and correct: always wake one sleeper (a cheap system call when
    // nobody sleeps). Tracking waiters can come later if it shows up.
    // SAFETY: futex on a live word; FUTEX_WAKE only reads the address.
    unsafe { libc::syscall(libc::SYS_futex, word.as_ptr(), libc::FUTEX_WAKE, 1) };
    0
}

/// `int sem_wait(sem_t*)`: waits until the count is above zero, then takes one.
///
/// # Safety
/// `sem` was set up by `sem_init`.
pub unsafe extern "C" fn sem_wait(sem: *mut SemT) -> c_int {
    // SAFETY: as above.
    let word = unsafe { count(sem) };
    loop {
        let value = word.load(Ordering::Relaxed);
        if value > 0 {
            if word
                .compare_exchange_weak(value, value - 1, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return 0;
            }
            continue;
        }
        // Sleep while the word is still 0. EAGAIN (it changed) and EINTR (a
        // signal) both just retry, as bionic does for apps on API 24+.
        // SAFETY: futex on a live word, no timeout.
        unsafe {
            libc::syscall(
                libc::SYS_futex,
                word.as_ptr(),
                libc::FUTEX_WAIT,
                0,
                std::ptr::null::<libc::timespec>(),
            )
        };
    }
}

pub fn implementation(name: &str) -> Option<u64> {
    type Sem = unsafe extern "C" fn(*mut SemT) -> c_int;
    let address = match name {
        "sem_init" => sem_init as unsafe extern "C" fn(*mut SemT, c_int, c_uint) -> c_int as usize,
        "sem_destroy" => sem_destroy as extern "C" fn(*mut SemT) -> c_int as usize,
        "sem_post" => sem_post as Sem as usize,
        "sem_wait" => sem_wait as Sem as usize,
        _ => return None,
    };
    Some(address as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn post_wakes_a_waiting_thread() {
        let sem: &'static mut SemT = Box::leak(Box::new(SemT { _bytes: [0; 4] }));
        let address = sem as *mut SemT as usize;
        // SAFETY (test): a leaked, live 16-byte semaphore.
        unsafe {
            assert_eq!(sem_init(address as *mut _, 0, 1), 0);
            assert_eq!(sem_wait(address as *mut _), 0); // 1 -> 0, no sleep
        }
        let waiter = std::thread::spawn(move || {
            // SAFETY (test): as above.
            unsafe { sem_wait(address as *mut _) }
        });
        std::thread::sleep(std::time::Duration::from_millis(50));
        // SAFETY (test): as above.
        unsafe { assert_eq!(sem_post(address as *mut _), 0) };
        assert_eq!(waiter.join().unwrap(), 0);
        // SAFETY (test): as above.
        unsafe {
            assert_eq!(sem_init(address as *mut _, 0, SEM_VALUE_MAX + 1), -1);
        }
    }
}

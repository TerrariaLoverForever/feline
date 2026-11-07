// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! Android's `ALooper` (NDK `looper.h`): a per-thread event loop over file
//! descriptors, built on Linux's `epoll`.
//!
//! On Android the app's main thread always has a looper (Java's main
//! `Looper`), and AGDK's GameActivity fails without one, so the thread that
//! loads the engine gets one automatically. Other threads get one through
//! `ALooper_prepare`.
//!
//! Loopers live as long as the process (`acquire`/`release` keep a count
//! but never free one), which is simple and safe for the engine's few
//! loopers.
//!
//! # `unsafe` in this module
//!
//! epoll and read(2) calls, and calling the engine's callbacks.

#![allow(unsafe_code)]
#![allow(non_snake_case)] // the NDK's exact names

use std::cell::Cell;
use std::collections::HashMap;
use std::ffi::{c_int, c_void};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

const ALOOPER_POLL_WAKE: c_int = -1;
const ALOOPER_POLL_CALLBACK: c_int = -2;
const ALOOPER_POLL_TIMEOUT: c_int = -3;
const ALOOPER_POLL_ERROR: c_int = -4;

// ALOOPER_EVENT_* (NDK) and their epoll equivalents.
const EVENT_INPUT: c_int = 1;
const EVENT_OUTPUT: c_int = 2;
const EVENT_ERROR: c_int = 4;
const EVENT_HANGUP: c_int = 8;

type Callback = Option<unsafe extern "C" fn(c_int, c_int, *mut c_void) -> c_int>;

#[derive(Clone, Copy)]
struct Entry {
    ident: c_int,
    events: c_int,
    callback: Callback,
    data: *mut c_void,
}

pub struct Looper {
    epoll: c_int,
    /// An eventfd for waking the looper (`ALooper_wake`, not imported yet).
    wake: c_int,
    fds: Mutex<HashMap<c_int, Entry>>,
    refs: AtomicUsize,
}

// The raw pointers in `Entry` are the engine's callback data, only handed
// back to the engine.
unsafe impl Send for Looper {}
unsafe impl Sync for Looper {}

thread_local! {
    static CURRENT: Cell<*const Looper> = const { Cell::new(std::ptr::null()) };
}

/// The thread that loads the engine: it gets a looper automatically.
static MAIN: Mutex<Option<std::thread::ThreadId>> = Mutex::new(None);

pub fn set_main_thread() {
    if let Ok(mut main) = MAIN.lock() {
        *main = Some(std::thread::current().id());
    }
}

fn to_epoll(events: c_int) -> u32 {
    let mut e = 0;
    if events & EVENT_INPUT != 0 {
        e |= libc::EPOLLIN as u32;
    }
    if events & EVENT_OUTPUT != 0 {
        e |= libc::EPOLLOUT as u32;
    }
    e
}

fn from_epoll(e: u32) -> c_int {
    let mut events = 0;
    if e & libc::EPOLLIN as u32 != 0 {
        events |= EVENT_INPUT;
    }
    if e & libc::EPOLLOUT as u32 != 0 {
        events |= EVENT_OUTPUT;
    }
    if e & libc::EPOLLERR as u32 != 0 {
        events |= EVENT_ERROR;
    }
    if e & libc::EPOLLHUP as u32 != 0 {
        events |= EVENT_HANGUP;
    }
    events
}

impl Looper {
    fn new() -> Option<&'static Looper> {
        // SAFETY: plain syscalls; both descriptors are checked.
        let (epoll, wake) = unsafe {
            (
                libc::epoll_create1(libc::EPOLL_CLOEXEC),
                libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK),
            )
        };
        if epoll < 0 || wake < 0 {
            return None;
        }
        let mut event = libc::epoll_event {
            events: libc::EPOLLIN as u32,
            u64: wake as u64,
        };
        // SAFETY: valid descriptors and event.
        unsafe { libc::epoll_ctl(epoll, libc::EPOLL_CTL_ADD, wake, &mut event) };
        Some(Box::leak(Box::new(Looper {
            epoll,
            wake,
            fds: Mutex::new(HashMap::new()),
            refs: AtomicUsize::new(1),
        })))
    }
}

fn current_or_main() -> *const Looper {
    CURRENT.with(|current| {
        if current.get().is_null() {
            let is_main = MAIN.lock().ok().and_then(|m| *m) == Some(std::thread::current().id());
            if is_main && let Some(looper) = Looper::new() {
                current.set(looper);
            }
        }
        current.get()
    })
}

/// `ALooper* ALooper_forThread(void)`
pub extern "C" fn ALooper_forThread() -> *const Looper {
    current_or_main()
}

/// `ALooper* ALooper_prepare(int opts)`: this thread's looper, created if needed.
pub extern "C" fn ALooper_prepare(_opts: c_int) -> *const Looper {
    let existing = current_or_main();
    if !existing.is_null() {
        return existing;
    }
    let created: *const Looper = Looper::new().map_or(std::ptr::null(), |l| l as *const Looper);
    CURRENT.with(|current| current.set(created));
    created
}

/// # Safety
/// `looper` came from ALooper_forThread/prepare.
pub unsafe extern "C" fn ALooper_acquire(looper: *const Looper) {
    // SAFETY: a live looper (loopers are never freed).
    unsafe { (*looper).refs.fetch_add(1, Ordering::Relaxed) };
}

/// # Safety
/// `looper` came from ALooper_forThread/prepare.
pub unsafe extern "C" fn ALooper_release(looper: *const Looper) {
    // SAFETY: as above.
    unsafe { (*looper).refs.fetch_sub(1, Ordering::Relaxed) };
}

/// `int ALooper_addFd(ALooper*, int fd, int ident, int events, callback, void* data)`
///
/// # Safety
/// `looper` came from ALooper_forThread/prepare.
pub unsafe extern "C" fn ALooper_addFd(
    looper: *const Looper,
    fd: c_int,
    ident: c_int,
    events: c_int,
    callback: Callback,
    data: *mut c_void,
) -> c_int {
    // SAFETY: a live looper.
    let looper = unsafe { &*looper };
    let mut event = libc::epoll_event {
        events: to_epoll(events),
        u64: fd as u64,
    };
    let mut fds = looper.fds.lock().unwrap_or_else(|p| p.into_inner());
    let op = if fds.contains_key(&fd) {
        libc::EPOLL_CTL_MOD
    } else {
        libc::EPOLL_CTL_ADD
    };
    // SAFETY: valid epoll descriptor and event.
    if unsafe { libc::epoll_ctl(looper.epoll, op, fd, &mut event) } != 0 {
        return -1;
    }
    let ident = if callback.is_some() {
        ALOOPER_POLL_CALLBACK
    } else {
        ident
    };
    fds.insert(
        fd,
        Entry {
            ident,
            events,
            callback,
            data,
        },
    );
    1
}

/// `int ALooper_removeFd(ALooper*, int fd)`
///
/// # Safety
/// `looper` came from ALooper_forThread/prepare.
pub unsafe extern "C" fn ALooper_removeFd(looper: *const Looper, fd: c_int) -> c_int {
    // SAFETY: a live looper.
    let looper = unsafe { &*looper };
    let mut fds = looper.fds.lock().unwrap_or_else(|p| p.into_inner());
    if fds.remove(&fd).is_none() {
        return 0;
    }
    // SAFETY: valid epoll descriptor.
    unsafe { libc::epoll_ctl(looper.epoll, libc::EPOLL_CTL_DEL, fd, std::ptr::null_mut()) };
    1
}

/// `int ALooper_pollOnce(int timeoutMillis, int* outFd, int* outEvents, void** outData)`
///
/// # Safety
/// The out pointers are NULL or writable.
pub unsafe extern "C" fn ALooper_pollOnce(
    timeout: c_int,
    out_fd: *mut c_int,
    out_events: *mut c_int,
    out_data: *mut *mut c_void,
) -> c_int {
    let looper = current_or_main();
    if looper.is_null() {
        return ALOOPER_POLL_ERROR;
    }
    // SAFETY: a live looper.
    let looper = unsafe { &*looper };
    let mut event = libc::epoll_event { events: 0, u64: 0 };
    // SAFETY: valid epoll descriptor and one-event buffer.
    let n = unsafe { libc::epoll_wait(looper.epoll, &mut event, 1, timeout) };
    stats(timeout, if n > 0 { event.u64 as c_int } else { n });
    if n < 0 {
        // EINTR counts as a wake-up, as on Android.
        return ALOOPER_POLL_WAKE;
    }
    if n == 0 {
        // The engine's main loop polls without waiting, millions of times a
        // second (a full CPU core) with nothing to do; on Android its frame
        // waits for vsync. A short pause on empty polls frees the core and
        // doesn't change the frame rate (measured: 60 per second either
        // way). FELINE_POLL_NAP=microseconds (0 = no pause), default 500.
        if timeout == 0 {
            use std::sync::OnceLock;
            static NAP: OnceLock<u64> = OnceLock::new();
            let nap = *NAP.get_or_init(|| {
                std::env::var("FELINE_POLL_NAP")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(500)
            });
            if nap > 0 {
                std::thread::sleep(std::time::Duration::from_micros(nap));
            }
        }
        return ALOOPER_POLL_TIMEOUT;
    }
    let fd = event.u64 as c_int;
    if fd == looper.wake {
        let mut buf = 0u64;
        // SAFETY: draining the eventfd into an 8-byte buffer.
        unsafe { libc::read(fd, (&mut buf as *mut u64).cast(), 8) };
        return ALOOPER_POLL_WAKE;
    }
    let events = from_epoll(event.events);
    let entry = looper
        .fds
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&fd)
        .copied();
    let Some(entry) = entry else {
        return ALOOPER_POLL_WAKE;
    };
    if let Some(callback) = entry.callback {
        // SAFETY: the engine's own callback with the data it registered.
        if unsafe { callback(fd, events, entry.data) } == 0 {
            // SAFETY: the looper pointer is still live.
            unsafe { ALooper_removeFd(looper, fd) };
        }
        return ALOOPER_POLL_CALLBACK;
    }
    // SAFETY: the engine passes NULL or writable out pointers.
    unsafe {
        if !out_fd.is_null() {
            *out_fd = fd;
        }
        if !out_events.is_null() {
            *out_events = events;
        }
        if !out_data.is_null() {
            *out_data = entry.data;
        }
    }
    let _ = entry.events;
    entry.ident
}

/// FELINE_LOOPER_LOG=1: every 2 s, how often pollOnce ran, with which
/// timeouts and results (an fd, 0 = timeout, -1 = error).
fn stats(timeout: c_int, result: c_int) {
    use std::sync::OnceLock;
    use std::time::Instant;
    static ON: OnceLock<bool> = OnceLock::new();
    if !*ON.get_or_init(|| std::env::var_os("FELINE_LOOPER_LOG").is_some()) {
        return;
    }
    type Counts = HashMap<(c_int, c_int), u64>;
    static STATE: Mutex<Option<(Instant, Counts)>> = Mutex::new(None);
    let mut state = STATE.lock().unwrap_or_else(|p| p.into_inner());
    let (since, counts) = state.get_or_insert_with(|| (Instant::now(), HashMap::new()));
    *counts.entry((timeout, result)).or_insert(0) += 1;
    if since.elapsed().as_secs() >= 2 {
        let mut list: Vec<_> = counts.drain().collect();
        list.sort_by_key(|entry| std::cmp::Reverse(entry.1));
        let text: Vec<String> = list
            .iter()
            .take(6)
            .map(|((t, r), n)| format!("timeout {t} -> {r}: {n}"))
            .collect();
        eprintln!("feline-runtime: looper, 2 s: {}", text.join(", "));
        *since = Instant::now();
    }
}

pub fn implementation(name: &str) -> Option<u64> {
    let address = match name {
        "ALooper_forThread" => ALooper_forThread as *const () as usize,
        "ALooper_prepare" => ALooper_prepare as *const () as usize,
        "ALooper_acquire" => ALooper_acquire as *const () as usize,
        "ALooper_release" => ALooper_release as *const () as usize,
        "ALooper_addFd" => ALooper_addFd as *const () as usize,
        "ALooper_removeFd" => ALooper_removeFd as *const () as usize,
        "ALooper_pollOnce" => ALooper_pollOnce as *const () as usize,
        _ => return None,
    };
    Some(address as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delivers_fd_events_with_and_without_callbacks() {
        let looper = ALooper_prepare(0);
        assert!(!looper.is_null());
        let mut fds = [0 as c_int; 2];
        // SAFETY (test): plain pipe creation and writes.
        unsafe { libc::pipe(fds.as_mut_ptr()) };

        // Without a callback: pollOnce returns the ident.
        // SAFETY (test): a live looper; no callback.
        assert_eq!(
            unsafe { ALooper_addFd(looper, fds[0], 7, EVENT_INPUT, None, 0x1234 as *mut c_void) },
            1
        );
        // SAFETY (test): writing one byte into our pipe.
        unsafe { libc::write(fds[1], b"x".as_ptr().cast(), 1) };
        let (mut fd, mut events, mut data) = (0, 0, std::ptr::null_mut());
        // SAFETY (test): writable out pointers.
        let r = unsafe { ALooper_pollOnce(1000, &mut fd, &mut events, &mut data) };
        assert_eq!(
            (r, fd, events, data as usize),
            (7, fds[0], EVENT_INPUT, 0x1234)
        );

        // Nothing pending: timeout.
        let mut byte = 0u8;
        // SAFETY (test): draining the pipe.
        unsafe { libc::read(fds[0], (&mut byte as *mut u8).cast(), 1) };
        // SAFETY (test): NULL out pointers are allowed.
        let r = unsafe {
            ALooper_pollOnce(
                10,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(r, ALOOPER_POLL_TIMEOUT);
        // SAFETY (test): a live looper.
        assert_eq!(unsafe { ALooper_removeFd(looper, fds[0]) }, 1);
        assert_eq!(unsafe { ALooper_removeFd(looper, fds[0]) }, 0);
    }
}

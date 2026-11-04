// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! Call tracing (`FELINE_CALL_TRACE=1`): which imported functions the engine
//! calls, in order, so a crash report can show what it was doing.
//!
//! Each traced import gets a trampoline (78 bytes, in 96-byte slots) in
//! front of its real function. It records the import's number and the
//! calling thread in a ring of the last 4,096 calls and jumps on. It uses no
//! stack and only `r10`/`r11`, which the x86-64 calling convention leaves
//! free at every call, so the call itself is unchanged:
//!
//! ```text
//! 49 bb <&POSITION:u64>     movabs r11, &POSITION
//! 41 ba 01 00 00 00         mov    r10d, 1
//! f0 4d 0f c1 13            lock xadd [r11], r10      ; r10 = this call's number
//! 41 81 e2 ff 0f 00 00      and    r10d, 4095         ; wrap around the ring
//! 49 c1 e2 03               shl    r10, 3             ; 8-byte entries
//! 49 bb <&RING:u64>         movabs r11, &RING
//! 4d 01 da                  add    r10, r11           ; r10 = &RING[i]
//! 64 4c 8b 1c 25 00000000   mov    r11, fs:[0]        ; the thread's control block
//! 45 89 5a 04               mov    [r10 + 4], r11d    ; its low half: the thread tag
//! 41 c7 02 <id:u32>         mov    dword [r10], id
//! 49 bb <target:u64>        movabs r11, target
//! 41 ff e3                  jmp    r11
//! ```
//!
//! `fs:[0]` is the thread control block's own address on x86-64 Linux
//! (glibc's `tcbhead_t`), different for every thread; the crash report uses
//! it to show the crashing thread's own last calls.
//!
//! Only interface activity is recorded: which imports are called, never
//! anything about the engine's own code.
//!
//! # `unsafe` in this module
//!
//! Mapping, writing and protecting the trampoline memory (written, then
//! read + execute, never both), and reading the ring in a signal handler.

#![allow(unsafe_code)]

use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use rustix::mm::{self, MapFlags, MprotectFlags, ProtFlags};

const RING_SIZE: usize = 4096;
const STUB_SIZE: usize = 96;

static POSITION: AtomicU64 = AtomicU64::new(0);
/// Low half: the import's id; high half: the calling thread's tag.
static RING: [AtomicU64; RING_SIZE] = [const { AtomicU64::new(0) }; RING_SIZE];
/// Names by trace id (id 0 = unused). Set once; read-only afterwards, so the
/// signal handler can read it without locks.
static NAMES: OnceLock<Vec<String>> = OnceLock::new();

pub fn enabled() -> bool {
    std::env::var_os("FELINE_CALL_TRACE").is_some()
}

/// Trampolines for a set of functions. Lives as long as the engine (leaked).
pub struct Tracer {
    /// Never unmapped: the engine may call through it until the end.
    _code: *mut c_void,
    /// (name, trampoline address) per traced function.
    pub entries: Vec<(String, u64)>,
}

/// One trampoline (padded to its slot).
fn encode(id: u32, target: u64) -> [u8; STUB_SIZE] {
    let mut code = [0xccu8; STUB_SIZE]; // int3 padding
    let mut at = 0;
    let mut put = |bytes: &[u8]| {
        code[at..at + bytes.len()].copy_from_slice(bytes);
        at += bytes.len();
    };
    put(&[0x49, 0xbb]);
    put(&(&POSITION as *const AtomicU64 as u64).to_le_bytes());
    put(&[0x41, 0xba, 0x01, 0x00, 0x00, 0x00]);
    put(&[0xf0, 0x4d, 0x0f, 0xc1, 0x13]);
    put(&[0x41, 0x81, 0xe2]);
    put(&(RING_SIZE as u32 - 1).to_le_bytes());
    put(&[0x49, 0xc1, 0xe2, 0x03]);
    put(&[0x49, 0xbb]);
    put(&(RING.as_ptr() as u64).to_le_bytes());
    put(&[0x4d, 0x01, 0xda]);
    put(&[0x64, 0x4c, 0x8b, 0x1c, 0x25, 0, 0, 0, 0]);
    put(&[0x45, 0x89, 0x5a, 0x04]);
    put(&[0x41, 0xc7, 0x02]);
    put(&id.to_le_bytes());
    put(&[0x49, 0xbb]);
    put(&target.to_le_bytes());
    put(&[0x41, 0xff, 0xe3]);
    code
}

impl Tracer {
    /// Builds trampolines for `functions` (name, real address). Can be
    /// called once per process (ids index one global name table).
    pub fn new(functions: Vec<(String, u64)>) -> Result<Self, String> {
        let mut names = vec![String::new()];
        names.extend(functions.iter().map(|(n, _)| n.clone()));
        NAMES
            .set(names)
            .map_err(|_| "the call tracer can only be set up once")?;

        let page = rustix::param::page_size();
        let len = (functions.len().max(1) * STUB_SIZE).div_ceil(page) * page;
        // SAFETY: a fresh anonymous mapping can't overlap anything we own.
        let code = unsafe {
            mm::mmap_anonymous(
                std::ptr::null_mut(),
                len,
                ProtFlags::READ | ProtFlags::WRITE,
                MapFlags::PRIVATE,
            )
        }
        .map_err(|e| format!("couldn't map memory for the call tracer: {e}"))?;
        let mut entries = Vec::with_capacity(functions.len());
        {
            // SAFETY: writable mapping of `len` bytes; only this slice uses it
            // until the protection change below.
            let memory = unsafe { std::slice::from_raw_parts_mut(code.cast::<u8>(), len) };
            for (i, (name, target)) in functions.into_iter().enumerate() {
                let id = u32::try_from(i + 1).map_err(|_| "too many traced functions")?;
                memory[i * STUB_SIZE..(i + 1) * STUB_SIZE].copy_from_slice(&encode(id, target));
                entries.push((name, code as u64 + (i * STUB_SIZE) as u64));
            }
        }
        // SAFETY: our own page-aligned mapping, no references remain.
        unsafe { mm::mprotect(code, len, MprotectFlags::READ | MprotectFlags::EXEC) }
            .map_err(|e| format!("couldn't make the call tracer executable: {e}"))?;
        Ok(Self {
            _code: code,
            entries,
        })
    }
}

/// This thread's tag, as the trampolines record it (the low half of the
/// thread control block's address, `fs:[0]`).
fn thread_tag() -> u32 {
    let tcb: u64;
    // SAFETY: reading fs:[0], the control block's self pointer, which every
    // thread created by glibc (all of them here) has.
    unsafe {
        std::arch::asm!("mov {}, fs:[0]", out(reg) tcb, options(nostack, readonly, preserves_flags))
    };
    tcb as u32
}

fn entry(i: usize) -> (usize, u32) {
    let value = RING[i % RING_SIZE].load(Ordering::Relaxed);
    ((value & 0xffff_ffff) as usize, (value >> 32) as u32)
}

fn write_bytes(bytes: &[u8]) {
    // SAFETY: write(2) is async-signal-safe; the buffer is valid.
    unsafe {
        libc::write(2, bytes.as_ptr().cast(), bytes.len());
    }
}

fn write_number(mut n: u64) {
    let mut buf = [0u8; 20];
    let mut at = buf.len();
    loop {
        at -= 1;
        buf[at] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    write_bytes(&buf[at..]);
}

/// Prints the recorded calls: the most frequent functions in the ring, then
/// the last 40 calls, newest last. Async-signal-safe (atomics, write(2),
/// fixed-size buffers), for the crash handler.
pub fn report() {
    let Some(names) = NAMES.get() else { return };
    let total = POSITION.load(Ordering::Relaxed);
    write_bytes(b"feline-runtime: calls to imports: ");
    write_number(total);
    write_bytes(b" in total\n");
    let recorded = (total as usize).min(RING_SIZE);
    if recorded == 0 {
        return;
    }

    // Most frequent among the last calls.
    let mut counts = [0u32; 1024];
    for i in 0..recorded {
        let (id, _) = entry(total as usize - 1 - i);
        if id < counts.len() {
            counts[id] += 1;
        }
    }
    write_bytes(b"feline-runtime: most called among the last ");
    write_number(recorded as u64);
    write_bytes(b":");
    for _ in 0..10 {
        let (best, n) = counts
            .iter()
            .enumerate()
            .max_by_key(|(_, n)| **n)
            .map(|(i, n)| (i, *n))
            .unwrap_or((0, 0));
        if n == 0 {
            break;
        }
        write_bytes(b" ");
        write_bytes(names.get(best).map_or(b"?".as_slice(), |s| s.as_bytes()));
        write_bytes(b" x");
        write_number(u64::from(n));
        counts[best] = 0;
    }
    {
        use std::sync::atomic::Ordering::Relaxed;
        write_bytes(b"\nfeline-runtime: last pthread_getspecific: key ");
        write_number(u64::from(crate::bionic::LAST_GET_KEY.load(Relaxed)));
        write_bytes(b" -> ");
        write_number(crate::bionic::LAST_GET_VALUE.load(Relaxed) as u64);
        write_bytes(b"; NULL results per key:");
        for (key, n) in crate::bionic::NULL_GETS.iter().enumerate() {
            let n = n.load(Relaxed);
            if n > 0 {
                write_bytes(b" key ");
                write_number(key as u64);
                write_bytes(b": ");
                write_number(n);
            }
        }
    }
    write_bytes(b"\nfeline-runtime: last calls, oldest first:");
    for i in (0..recorded.min(40)).rev() {
        let (id, _) = entry(total as usize - 1 - i);
        write_bytes(b" ");
        write_bytes(names.get(id).map_or(b"?".as_slice(), |s| s.as_bytes()));
    }
    // The calling (crashing) thread's own last calls, repeats folded
    // ("memcpy x12"), so the report reaches further back.
    let mine = thread_tag();
    let mut own = [(0usize, 0u32); 40];
    let mut found = 0;
    for i in 0..recorded {
        let (id, tag) = entry(total as usize - 1 - i);
        if tag != mine {
            continue;
        }
        if found > 0 && own[found - 1].0 == id {
            own[found - 1].1 += 1;
        } else if found < own.len() {
            own[found] = (id, 1);
            found += 1;
        } else {
            break;
        }
    }
    write_bytes(b"\nfeline-runtime: this thread's last calls, oldest first:");
    for &(id, n) in own[..found].iter().rev() {
        write_bytes(b" ");
        write_bytes(names.get(id).map_or(b"?".as_slice(), |s| s.as_bytes()));
        if n > 1 {
            write_bytes(b" x");
            write_number(u64::from(n));
        }
    }
    write_bytes(b"\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    extern "C" fn add(a: u64, b: u64, c: u64, d: u64, e: u64, f: u64) -> u64 {
        a + b * 10 + c * 100 + d * 1000 + e * 10_000 + f * 100_000
    }

    #[test]
    fn trampolines_record_and_pass_every_argument_through() {
        let target = add as extern "C" fn(u64, u64, u64, u64, u64, u64) -> u64 as usize as u64;
        let tracer = Tracer::new(vec![("add".into(), target), ("other".into(), target)]).unwrap();
        let (_, address) = tracer.entries[1].clone();
        // SAFETY (test): `address` is a trampoline that jumps to `add`,
        // which has exactly this signature.
        let call: extern "C" fn(u64, u64, u64, u64, u64, u64) -> u64 =
            unsafe { std::mem::transmute(address) };
        let before = POSITION.load(Ordering::Relaxed);
        assert_eq!(
            call(1, 2, 3, 4, 5, 6),
            654_321,
            "all six argument registers survive"
        );
        assert_eq!(call(0, 0, 0, 0, 0, 7), 700_000);
        let after = POSITION.load(Ordering::Relaxed);
        assert_eq!(after - before, 2);
        // Both calls recorded id 2 ("other"), tagged with this thread.
        assert_eq!(entry(after as usize - 1), (2, thread_tag()));
        assert_eq!(entry(after as usize - 2), (2, thread_tag()));
        assert_eq!(NAMES.get().unwrap()[2], "other");
    }
}

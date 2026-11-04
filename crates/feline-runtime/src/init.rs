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

use std::ffi::{CString, c_char, c_int, c_void};
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

/// The engine's load bias, for reporting crash addresses as offsets.
static BIAS: AtomicUsize = AtomicUsize::new(0);
static IMAGE_LEN: AtomicUsize = AtomicUsize::new(0);

/// Writes `n` as 0x-prefixed hex into `buf` (no allocation: signal-safe).
fn format_hex(mut n: usize, buf: &mut [u8; 18]) -> &[u8] {
    let mut at = buf.len();
    loop {
        at -= 1;
        buf[at] = b"0123456789abcdef"[n % 16];
        n /= 16;
        if n == 0 {
            break;
        }
    }
    at -= 2;
    buf[at] = b'0';
    buf[at + 1] = b'x';
    &buf[at..]
}

extern "C" fn on_fatal_signal(signal: c_int, info: *mut libc::siginfo_t, context: *mut c_void) {
    let name: &[u8] = match signal {
        libc::SIGSEGV => b"a bad memory access (SIGSEGV)",
        libc::SIGBUS => b"a bus error (SIGBUS)",
        libc::SIGILL => b"an illegal instruction (SIGILL)",
        libc::SIGFPE => b"an arithmetic error (SIGFPE)",
        libc::SIGABRT => b"abort (SIGABRT)",
        _ => b"a fatal signal",
    };
    let current = CURRENT.load(Ordering::Relaxed);
    let (mut a, mut b) = ([0u8; 20], [0u8; 20]);
    let where_: [&[u8]; 4] = if current == 0 {
        [b" after the constructors", b"", b"", b""]
    } else {
        [
            b" in constructor ",
            format_number(current, &mut a),
            b" of ",
            format_number(TOTAL.load(Ordering::Relaxed), &mut b),
        ]
    };
    // The instruction that failed (RIP) and, for memory errors, the address.
    // SAFETY: with SA_SIGINFO the kernel passes a valid siginfo_t and
    // ucontext_t; reading their fields is async-signal-safe.
    let (rip, fault) = unsafe {
        let uc = context.cast::<libc::ucontext_t>();
        let rip = (*uc).uc_mcontext.gregs[libc::REG_RIP as usize] as usize;
        (rip, (*info).si_addr() as usize)
    };
    let (bias, len) = (
        BIAS.load(Ordering::Relaxed),
        IMAGE_LEN.load(Ordering::Relaxed),
    );
    let (mut h1, mut h2) = ([0u8; 18], [0u8; 18]);
    let at: [&[u8]; 2] = if bias != 0 && rip >= bias && rip < bias + len {
        [b" at engine offset ", format_hex(rip - bias, &mut h1)]
    } else {
        [b" at address ", format_hex(rip, &mut h1)]
    };
    let parts: [&[u8]; 11] = [
        b"feline-runtime: stopped by ",
        name,
        where_[0],
        where_[1],
        where_[2],
        where_[3],
        at[0],
        at[1],
        b" (accessing ",
        format_hex(fault, &mut h2),
        b")\n",
    ];
    for part in parts {
        // SAFETY: write(2) is async-signal-safe; the buffers are valid.
        unsafe {
            libc::write(2, part.as_ptr().cast(), part.len());
        }
    }
    // A rough backtrace: values on the stack just above the stack pointer
    // that point into the engine are mostly return addresses. Runaway
    // recursion shows up as the same few repeating.
    // SAFETY: as above, plus: the words read lie between the stack pointer
    // and the stack's top, which is mapped (the thread was running there).
    unsafe {
        let uc = context.cast::<libc::ucontext_t>();
        let rsp = (*uc).uc_mcontext.gregs[libc::REG_RSP as usize] as usize;
        write_bytes(b"feline-runtime: engine addresses on the stack:");
        let mut shown = 0;
        for i in 0..4096 {
            let value = *((rsp + i * 8) as *const usize);
            if bias != 0 && value > bias && value < bias + len {
                let mut h = [0u8; 18];
                write_bytes(b" ");
                write_bytes(format_hex(value - bias, &mut h));
                shown += 1;
                if shown == 24 {
                    break;
                }
            }
        }
        write_bytes(b"\n");
        crate::trace::report();
        let len = SNAPSHOT_LEN.load(Ordering::Acquire);
        let snapshot = SNAPSHOT.load(Ordering::Acquire);
        if !snapshot.is_null() && len > 0 {
            libc::write(2, snapshot.cast(), len);
        }
    }
    // SA_RESETHAND restored the default action, so returning re-raises the
    // signal (or abort() raises it again) and the process ends normally for
    // that signal.
}

/// The latest thread snapshot (see [`start_thread_watch`]), for the crash
/// handler: a leaked buffer, published with an atomic pointer so the handler
/// never takes a lock.
static SNAPSHOT: std::sync::atomic::AtomicPtr<u8> =
    std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());
static SNAPSHOT_LEN: AtomicUsize = AtomicUsize::new(0);

/// `FELINE_THREAD_WATCH=1`: a diagnostic thread that records, every 10 ms,
/// each thread's name, state and where in the kernel it waits
/// (`/proc/self/task/*`), so a crash report shows what the other threads
/// were doing. Diagnostics only: each snapshot is leaked.
pub fn start_thread_watch() {
    if std::env::var_os("FELINE_THREAD_WATCH").is_none() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("feline-watch".into())
        .spawn(|| {
            loop {
                let mut text = String::from("feline-runtime: threads (name, state, waiting in):\n");
                if let Ok(tasks) = std::fs::read_dir("/proc/self/task") {
                    for task in tasks.filter_map(Result::ok) {
                        let read = |f: &str| {
                            std::fs::read_to_string(task.path().join(f)).unwrap_or_default()
                        };
                        let stat = read("stat");
                        let state = stat
                            .rsplit_once(')')
                            .map(|(_, r)| r.trim().chars().next().unwrap_or('?'));
                        text.push_str(&format!(
                            "  {:<16} {} {}\n",
                            read("comm").trim(),
                            state.unwrap_or('?'),
                            read("wchan").trim()
                        ));
                    }
                }
                // Which signals the process catches, blocks or has pending.
                if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
                    for line in status
                        .lines()
                        .filter(|l| l.starts_with("Sig") || l.starts_with("ShdPnd"))
                    {
                        text.push_str(&format!("  {line}\n"));
                    }
                }
                let bytes: &'static mut [u8] = Box::leak(text.into_bytes().into_boxed_slice());
                SNAPSHOT_LEN.store(0, Ordering::Release);
                SNAPSHOT.store(bytes.as_mut_ptr(), Ordering::Release);
                SNAPSHOT_LEN.store(bytes.len(), Ordering::Release);
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        });
}

fn write_bytes(bytes: &[u8]) {
    // SAFETY: write(2) is async-signal-safe; the buffer is valid.
    unsafe {
        libc::write(2, bytes.as_ptr().cast(), bytes.len());
    }
}

/// Gives this thread an alternate signal stack, so the crash report can be
/// written even when the crash is a stack overflow.
fn install_signal_stack() {
    const SIZE: usize = 256 * 1024;
    let memory: &'static mut [u8] = Box::leak(vec![0u8; SIZE].into_boxed_slice());
    let stack = libc::stack_t {
        ss_sp: memory.as_mut_ptr().cast(),
        ss_flags: 0,
        ss_size: SIZE,
    };
    // SAFETY: the memory is leaked, so it stays valid for the thread's life.
    unsafe { libc::sigaltstack(&stack, std::ptr::null_mut()) };
}

fn install_crash_reporting() {
    install_signal_stack();
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
            action.sa_sigaction =
                on_fatal_signal as extern "C" fn(c_int, *mut libc::siginfo_t, *mut c_void) as usize;
            action.sa_flags = libc::SA_RESETHAND | libc::SA_SIGINFO | libc::SA_ONSTACK;
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
    BIAS.store(image.bias() as usize, Ordering::Relaxed);
    IMAGE_LEN.store(image.len(), Ordering::Relaxed);
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
    start_thread_watch();
    // The engine may have installed its own crash handler meanwhile.
    install_crash_reporting();
    stubs::set_phase("JNI_OnLoad".to_owned());
    // SAFETY: as for the constructors: the engine's own function at its
    // exported address, with the signature JNI defines.
    let function: extern "C" fn(*mut std::ffi::c_void, *mut std::ffi::c_void) -> c_int =
        unsafe { std::mem::transmute(image.bias() + onload.value) };
    let version = function(vm, std::ptr::null_mut());
    stubs::set_phase(String::new());
    Ok(version)
}

/// Folders the engine gets as its app's data directories, under
/// `$XDG_DATA_HOME/feline/runtime/` (created if needed).
fn data_dirs() -> Result<[String; 3], String> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share"))
        })
        .ok_or("no home folder")?
        .join("feline/runtime");
    let dirs = ["files", "obb", "external"].map(|d| base.join(d));
    for dir in &dirs {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("couldn't create {}: {e}", dir.display()))?;
    }
    Ok(dirs.map(|d| d.to_string_lossy().into_owned()))
}

/// Starts the engine the way Android's GameActivity does (AGDK, Apache-2.0):
/// `initializeNativeCode(activity, internalDataDir, obbDir, externalDataDir,
/// assetManager, savedState, configuration)`. Returns the native handle the
/// later lifecycle calls (`onSurfaceCreatedNative`, …) take.
pub fn call_initialize_native_code(elf: &Elf, image: &Image) -> Result<i64, String> {
    const NAME: &str = "Java_com_google_androidgamesdk_GameActivity_initializeNativeCode";
    let symbols = elf.symbols().map_err(|e| e.to_string())?;
    let entry = symbols
        .iter()
        .find(|s| s.defined && s.name == NAME)
        .ok_or("the library has no GameActivity.initializeNativeCode")?;
    crate::jni::java_vm()?; // makes sure the JNI tables exist
    // Like Android's main thread, this one has a looper.
    crate::looper::set_main_thread();
    let env = crate::jni::current_env().ok_or("no JNIEnv for the main thread")?;
    let [files, obb, external] = data_dirs()?;
    let activity = crate::jni::new_instance("com/roblox/client/startup/MainGameActivity");
    let (files, obb, external) = (
        crate::jni::new_string(&files),
        crate::jni::new_string(&obb),
        crate::jni::new_string(&external),
    );
    let assets = crate::jni::new_instance("android/content/res/AssetManager");
    let config = crate::jni::new_instance("android/content/res/Configuration");
    type Init = extern "C" fn(
        *mut c_void,
        *mut c_void,
        *mut c_void,
        *mut c_void,
        *mut c_void,
        *mut c_void,
        *mut c_void,
        *mut c_void,
    ) -> i64;
    *ACTIVITY.lock().unwrap_or_else(|p| p.into_inner()) = activity as usize;
    stubs::set_phase("GameActivity.initializeNativeCode".to_owned());
    // SAFETY: the engine's exported JNI function, with the signature AGDK
    // declares for it.
    let function: Init = unsafe { std::mem::transmute(image.bias() + entry.value) };
    let handle = function(
        env,
        activity,
        files,
        obb,
        external,
        assets,
        std::ptr::null_mut(),
        config,
    );
    stubs::set_phase(String::new());
    Ok(handle)
}

/// The activity object handed to initializeNativeCode (later lifecycle
/// calls pass it as `this`).
static ACTIVITY: std::sync::Mutex<usize> = std::sync::Mutex::new(0);

/// Calls one of GameActivity's registered lifecycle natives that take only
/// the native handle: `void name(JNIEnv*, jobject thiz, jlong handle)`.
pub fn call_lifecycle(name: &str, handle: i64) -> Result<(), String> {
    let function =
        crate::jni::registered_native(name).ok_or_else(|| format!("{name} wasn't registered"))?;
    let env = crate::jni::current_env().ok_or("no JNIEnv for the main thread")?;
    let activity = *ACTIVITY.lock().unwrap_or_else(|p| p.into_inner()) as *mut c_void;
    stubs::set_phase(format!("GameActivity.{name}"));
    // SAFETY: a native the engine registered with signature "(J)V".
    let call: extern "C" fn(*mut c_void, *mut c_void, i64) =
        unsafe { std::mem::transmute(function) };
    call(env, activity, handle);
    stubs::set_phase(String::new());
    Ok(())
}

/// Runs this (main) thread's looper for `duration`, as Android's main
/// thread does between events. Returns how many callbacks ran.
pub fn run_main_loop(duration: std::time::Duration) -> usize {
    stubs::set_phase("the main looper".to_owned());
    let end = std::time::Instant::now() + duration;
    let mut callbacks = 0;
    while std::time::Instant::now() < end {
        let left = end
            .saturating_duration_since(std::time::Instant::now())
            .as_millis()
            .min(100) as c_int;
        // SAFETY: NULL out pointers are allowed.
        let r = unsafe {
            crate::looper::ALooper_pollOnce(
                left,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if r == -2 {
            callbacks += 1;
        }
    }
    stubs::set_phase(String::new());
    callbacks
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

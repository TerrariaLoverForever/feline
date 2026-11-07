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
    // Outside the engine: which host library (and nearest exported symbol).
    // dladdr isn't formally async-signal-safe, but the process is ending
    // and it only reads the loader's tables.
    if !(bias != 0 && rip >= bias && rip < bias + len) {
        // SAFETY: a zeroed Dl_info for dladdr to fill; the strings it
        // returns belong to the loader and stay valid.
        unsafe {
            let mut info: libc::Dl_info = std::mem::zeroed();
            if libc::dladdr(rip as *const c_void, &mut info) != 0 {
                write_bytes(b"feline-runtime: in ");
                if !info.dli_fname.is_null() {
                    write_bytes(std::ffi::CStr::from_ptr(info.dli_fname).to_bytes());
                }
                if !info.dli_sname.is_null() {
                    let mut h = [0u8; 18];
                    write_bytes(b" (");
                    write_bytes(std::ffi::CStr::from_ptr(info.dli_sname).to_bytes());
                    write_bytes(b" + ");
                    write_bytes(format_hex(rip - info.dli_saddr as usize, &mut h));
                    write_bytes(b")");
                }
                write_bytes(b"\n");
            }
        }
    }
    // The crashing thread's name: prctl is a plain system call, safe here.
    let mut thread = [0u8; 17];
    // SAFETY: PR_GET_NAME writes at most 16 bytes plus NUL into `thread`.
    unsafe { libc::prctl(libc::PR_GET_NAME, thread.as_mut_ptr()) };
    let end = thread.iter().position(|&b| b == 0).unwrap_or(16);
    write_bytes(b"feline-runtime: on thread \"");
    write_bytes(&thread[..end]);
    write_bytes(b"\"\n");
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
    // The engine keeps its logs and caches (`appData/`, `cache/`) relative
    // to the working directory, so run it inside its own data folder.
    std::env::set_current_dir(&files).map_err(|e| format!("couldn't enter {files}: {e}"))?;
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

/// Tells the engine about a window change, through the natives AGDK
/// registered: `onSurfaceCreatedNative(J, Surface)`, `onSurfaceChangedNative
/// (J, Surface, format, width, height)`, `onWindowFocusChangedNative(J, Z)`.
/// Roblox's own input functions. Its Java code passes mouse and keyboard
/// input through these, not through GameActivity's input queue (which the
/// engine never reads): mouse buttons are Roblox's MouseButton1/2/3 indices
/// (0, 1, 2), positions are surface pixels. Argument meanings follow
/// mocktail's input adapter (Apache-2.0, github.com/komaruworld/mocktail).
#[derive(Clone, Copy)]
struct InputNatives {
    input_class: usize,
    gl_class: usize,
    mouse_move: Option<usize>,
    mouse_button: Option<usize>,
    mouse_wheel: Option<usize>,
    key: Option<usize>,
    sync_text: Option<usize>,
    pass_text: Option<usize>,
    return_pressed: Option<usize>,
    release_focus: Option<usize>,
    text_box_info: Option<usize>,
}

static INPUT_NATIVES: std::sync::OnceLock<InputNatives> = std::sync::OnceLock::new();

/// Finds Roblox's input functions (once the engine is loaded).
pub fn prepare_input_natives(elf: &Elf, image: &Image) -> Result<(), String> {
    const INPUT: &str = "com/roblox/engine/jni/NativeInputInterface";
    const GL: &str = "com/roblox/engine/jni/NativeGLInterface";
    let symbols = elf.symbols().map_err(|e| e.to_string())?;
    let find = |class: &str, method: &str| {
        let name = jni_symbol(class, method);
        symbols
            .iter()
            .find(|s| s.defined && s.name == name)
            .map(|s| (image.bias() + s.value) as usize)
    };
    let natives = InputNatives {
        input_class: crate::jni::new_class(INPUT) as usize,
        gl_class: crate::jni::new_class(GL) as usize,
        mouse_move: find(INPUT, "nativePassMouseMove"),
        mouse_button: find(INPUT, "nativePassMouseButton"),
        mouse_wheel: find(INPUT, "nativePassMouseWheel"),
        key: find(GL, "nativePassKeyEvent"),
        sync_text: find(GL, "syncTextboxTextAndCursorPosition2"),
        pass_text: find(GL, "nativePassText"),
        return_pressed: find(GL, "nativeReturnPressedFromOnScreenKeyboard"),
        release_focus: find(GL, "nativeReleaseFocus"),
        text_box_info: find(GL, "nativeGetTextBoxInfo"),
    };
    let _ = INPUT_NATIVES.set(natives);
    Ok(())
}

/// Sends a pointer event through Roblox's mouse functions. False if they
/// aren't available (then GameActivity's touch path is used).
fn roblox_mouse(env: *mut c_void, touch: crate::window::Touch) -> bool {
    use crate::window::TouchAction;
    use std::sync::Mutex;
    static LAST: Mutex<Option<(f32, f32)>> = Mutex::new(None);
    let Some(natives) = INPUT_NATIVES.get() else {
        return false;
    };
    let class = natives.input_class as *mut c_void;
    let (x, y) = (touch.x.max(0.0), touch.y.max(0.0));
    match touch.action {
        TouchAction::Down | TouchAction::Up => {
            let Some(address) = natives.mouse_button else {
                return false;
            };
            // SAFETY: nativePassMouseButton (FFZI)V, a static JNI function.
            let button: extern "C" fn(*mut c_void, *mut c_void, f32, f32, u8, c_int) =
                unsafe { std::mem::transmute(address) };
            button(
                env,
                class,
                x,
                y,
                u8::from(touch.action == TouchAction::Down),
                0,
            );
        }
        TouchAction::Move | TouchAction::Hover => {
            let Some(address) = natives.mouse_move else {
                return false;
            };
            let mut last = LAST.lock().unwrap_or_else(|p| p.into_inner());
            let (dx, dy) = last.map_or((0.0, 0.0), |(lx, ly)| (x - lx, y - ly));
            *last = Some((x, y));
            // SAFETY: nativePassMouseMove (FFFF)V: position, then movement.
            let moved: extern "C" fn(*mut c_void, *mut c_void, f32, f32, f32, f32) =
                unsafe { std::mem::transmute(address) };
            moved(env, class, x, y, dx, dy);
        }
    }
    true
}

/// Sends a right (1) or middle (2) button press through Roblox's function.
fn roblox_mouse_button(env: *mut c_void, x: f32, y: f32, button: i32, down: bool) {
    let Some(natives) = INPUT_NATIVES.get() else {
        return;
    };
    if let Some(address) = natives.mouse_button {
        // SAFETY: nativePassMouseButton (FFZI)V, a static JNI function.
        let pass: extern "C" fn(*mut c_void, *mut c_void, f32, f32, u8, c_int) =
            unsafe { std::mem::transmute(address) };
        pass(
            env,
            natives.input_class as *mut c_void,
            x.max(0.0),
            y.max(0.0),
            u8::from(down),
            button,
        );
    }
}

/// Sends a mouse wheel step through Roblox's function (`dy` > 0 is up).
fn roblox_wheel(env: *mut c_void, x: f32, y: f32, dy: f32) {
    if let Some(address) = INPUT_NATIVES.get().and_then(|n| n.mouse_wheel) {
        let class = INPUT_NATIVES.get().map_or(0, |n| n.input_class) as *mut c_void;
        // SAFETY: nativePassMouseWheel (FFF)V.
        let wheel: extern "C" fn(*mut c_void, *mut c_void, f32, f32, f32) =
            unsafe { std::mem::transmute(address) };
        wheel(env, class, x, y, dy);
    }
}

/// Logs a key event for bug reports: the key code, scan code and where it
/// went ("text box" or "game"). Nothing typed into a text box can be read
/// back from it. Returns `to_text_box` unchanged.
fn log_key(key: crate::window::Key, to_text_box: bool) -> bool {
    // In a text box, a character key's code would give away what was
    // typed: only keys without a character (Enter, Esc, arrows) are logged.
    if !key.repeat && !(to_text_box && key.unicode != 0) {
        eprintln!(
            "feline-runtime: key {} code {} scan {} → {}",
            if key.down { "down" } else { "up" },
            key.code,
            key.scan_code,
            if to_text_box { "text box" } else { "game" }
        );
    }
    to_text_box
}

/// Types `key` into the focused text box, if there is one and the key is for
/// it (see `text.rs`). True if the key was used.
fn roblox_text(env: *mut c_void, key: crate::window::Key) -> bool {
    use crate::text::Edit;
    let Some(natives) = INPUT_NATIVES.get() else {
        return false;
    };
    let (Some(sync), Some(pass), Some(ret), Some(release)) = (
        natives.sync_text,
        natives.pass_text,
        natives.return_pressed,
        natives.release_focus,
    ) else {
        return false;
    };
    let Some(edit) = crate::text::key(key) else {
        return false;
    };
    let class = natives.gl_class as *mut c_void;
    // SAFETY: the DEX signatures: syncTextboxTextAndCursorPosition2
    // (Ljava/lang/String;I)V, nativePassText (JLjava/lang/String;ZI)V,
    // nativeReturnPressedFromOnScreenKeyboard (J)V, nativeReleaseFocus (J)V.
    type Sync = extern "C" fn(*mut c_void, *mut c_void, *mut c_void, c_int);
    type Pass = extern "C" fn(*mut c_void, *mut c_void, i64, *mut c_void, u8, c_int);
    type WithBox = extern "C" fn(*mut c_void, *mut c_void, i64);
    let (sync, pass, ret, release) = unsafe {
        (
            std::mem::transmute::<usize, Sync>(sync),
            std::mem::transmute::<usize, Pass>(pass),
            std::mem::transmute::<usize, WithBox>(ret),
            std::mem::transmute::<usize, WithBox>(release),
        )
    };
    match edit {
        Edit::Unchanged => {}
        Edit::Changed {
            text_box,
            text,
            cursor,
        } => {
            sync(env, class, crate::jni::new_string(&text), cursor);
            pass(
                env,
                class,
                text_box,
                crate::jni::new_string(&text),
                0,
                cursor,
            );
        }
        Edit::Enter {
            text_box,
            text,
            cursor,
        } => {
            sync(env, class, crate::jni::new_string(&text), cursor);
            ret(env, class, text_box);
            pass(
                env,
                class,
                text_box,
                crate::jni::new_string(&text),
                1,
                cursor,
            );
        }
        Edit::Release { text_box } => release(env, class, text_box),
    }
    true
}

/// Passes the text of a box the engine just left as finished (see
/// `text::blur`).
/// When Roblox reported that the focused box changed (position, size),
/// asks it for the box's current info (`nativeGetTextBoxInfo`, which builds
/// a NativeTextBoxInfo that `text::set_info` reads), outside the engine's
/// own callback, so typed text is drawn where the box now is.
fn refresh_text_box_info(env: *mut c_void) {
    if !crate::text::take_info_refresh() {
        return;
    }
    let Some(natives) = INPUT_NATIVES.get() else {
        return;
    };
    let Some(address) = natives.text_box_info else {
        return;
    };
    // SAFETY: nativeGetTextBoxInfo ()Lcom/roblox/engine/jni/model/NativeTextBoxInfo;
    let get: extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
        unsafe { std::mem::transmute(address) };
    get(env, natives.gl_class as *mut c_void);
    crate::text::update_position();
}

fn finish_left_text_box(env: *mut c_void) {
    let Some((text_box, text, cursor)) = crate::text::take_left() else {
        return;
    };
    let Some(natives) = INPUT_NATIVES.get() else {
        return;
    };
    let Some(pass) = natives.pass_text else {
        return;
    };
    // SAFETY: nativePassText (JLjava/lang/String;ZI)V.
    let pass: extern "C" fn(*mut c_void, *mut c_void, i64, *mut c_void, u8, c_int) =
        unsafe { std::mem::transmute(pass) };
    pass(
        env,
        natives.gl_class as *mut c_void,
        text_box,
        crate::jni::new_string(&text),
        1,
        cursor,
    );
}

/// Sends a key through Roblox's function. False if it isn't available.
fn roblox_key(env: *mut c_void, key: crate::window::Key) -> bool {
    let Some(natives) = INPUT_NATIVES.get() else {
        return false;
    };
    if !crate::text::game_key(key) {
        return true;
    }
    let Some(address) = natives.key else {
        return false;
    };
    // SAFETY: nativePassKeyEvent (ZIIZ)V: pressed, evdev scan code, Android
    // key code, repeat.
    let pass: extern "C" fn(*mut c_void, *mut c_void, u8, c_int, c_int, u8) =
        unsafe { std::mem::transmute(address) };
    pass(
        env,
        natives.gl_class as *mut c_void,
        u8::from(key.down),
        key.scan_code,
        key.code,
        u8::from(key.repeat),
    );
    true
}

pub fn window_changed(handle: i64, change: crate::window::WindowChange) -> Result<(), String> {
    use crate::window::WindowChange;
    /// Android's PixelFormat.RGBA_8888.
    const RGBA_8888: c_int = 1;
    let env = crate::jni::current_env().ok_or("no JNIEnv for the main thread")?;
    let activity = *ACTIVITY.lock().unwrap_or_else(|p| p.into_inner()) as *mut c_void;
    let surface = crate::window::surface_object();
    let native = |name: &str| {
        crate::jni::registered_native(name).ok_or_else(|| format!("{name} wasn't registered"))
    };
    match change {
        WindowChange::Created { width, height } => {
            eprintln!("feline-runtime: onSurfaceCreated, onSurfaceChanged {width}x{height}");
            stubs::set_phase("GameActivity.onSurfaceCreatedNative".to_owned());
            // SAFETY: AGDK's native with signature (JLandroid/view/Surface;)V.
            let created: extern "C" fn(*mut c_void, *mut c_void, i64, *mut c_void) =
                unsafe { std::mem::transmute(native("onSurfaceCreatedNative")?) };
            created(env, activity, handle, surface);
            window_changed(handle, WindowChange::Resized { width, height })?;
        }
        WindowChange::Resized { width, height } => {
            stubs::set_phase("GameActivity.onSurfaceChangedNative".to_owned());
            // SAFETY: AGDK's native with signature (JLandroid/view/Surface;III)V.
            let changed: extern "C" fn(
                *mut c_void,
                *mut c_void,
                i64,
                *mut c_void,
                c_int,
                c_int,
                c_int,
            ) = unsafe { std::mem::transmute(native("onSurfaceChangedNative")?) };
            changed(env, activity, handle, surface, RGBA_8888, width, height);
            // What Android also does when a window changes size: the area
            // the app may draw in, the new configuration (its size in dp),
            // and Roblox's app screens get the new surface size. Without
            // these the app screens kept their first size: black areas
            // after growing the window, glitches after shrinking it.
            stubs::set_phase("GameActivity.onContentRectChangedNative".to_owned());
            // SAFETY: AGDK's native with signature (JIIII)V.
            let content: extern "C" fn(*mut c_void, *mut c_void, i64, c_int, c_int, c_int, c_int) =
                unsafe { std::mem::transmute(native("onContentRectChangedNative")?) };
            content(env, activity, handle, 0, 0, width, height);
            stubs::set_phase("GameActivity.onConfigurationChangedNative".to_owned());
            // SAFETY: AGDK's native with signature (JLandroid/content/res/Configuration;)V.
            let configuration: extern "C" fn(*mut c_void, *mut c_void, i64, *mut c_void) =
                unsafe { std::mem::transmute(native("onConfigurationChangedNative")?) };
            configuration(
                env,
                activity,
                handle,
                crate::jni::new_instance("android/content/res/Configuration"),
            );
            update_app_surface(env);
            stubs::set_phase(String::new());
        }
        WindowChange::Focus(focused) => {
            stubs::set_phase("GameActivity.onWindowFocusChangedNative".to_owned());
            // SAFETY: AGDK's native with signature (JZ)V.
            let focus: extern "C" fn(*mut c_void, *mut c_void, i64, u8) =
                unsafe { std::mem::transmute(native("onWindowFocusChangedNative")?) };
            focus(env, activity, handle, u8::from(focused));
        }
        WindowChange::CloseRequested => {}
        WindowChange::Wheel { x, y, dy } => roblox_wheel(env, x, y, dy),
        WindowChange::MouseButton { x, y, button, down } => {
            roblox_mouse_button(env, x, y, button, down);
        }
        WindowChange::Touch(touch) if crate::window::mouse_mode() && roblox_mouse(env, touch) => {
            static LOGGED: AtomicUsize = AtomicUsize::new(0);
            if touch.action != crate::window::TouchAction::Hover
                && LOGGED.fetch_add(1, Ordering::Relaxed) < 12
            {
                eprintln!(
                    "feline-runtime: mouse {:?} at ({:.0}, {:.0}) through Roblox's input",
                    touch.action, touch.x, touch.y
                );
            }
        }
        WindowChange::Key(key) if log_key(key, roblox_text(env, key)) || roblox_key(env, key) => {}
        WindowChange::Touch(touch) => {
            use crate::window::TouchAction;
            /// AINPUT_SOURCE_TOUCHSCREEN, AINPUT_SOURCE_MOUSE.
            const SOURCE_TOUCHSCREEN: c_int = 0x1002;
            const SOURCE_MOUSE: c_int = 0x2002;
            /// MotionEvent.BUTTON_PRIMARY.
            const BUTTON_PRIMARY: c_int = 1;
            let mouse = crate::window::mouse_mode();
            let source = if mouse {
                SOURCE_MOUSE
            } else {
                SOURCE_TOUCHSCREEN
            };
            // The button held during the event, and the one that changed.
            let (button_state, action_button) = match (mouse, touch.action) {
                (false, _) | (true, TouchAction::Hover) => (0, 0),
                (true, TouchAction::Down) => (BUTTON_PRIMARY, BUTTON_PRIMARY),
                (true, TouchAction::Move) => (BUTTON_PRIMARY, 0),
                (true, TouchAction::Up) => (0, BUTTON_PRIMARY),
            };
            static DOWN_TIME: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
            let now = uptime_millis();
            if touch.action == TouchAction::Down {
                DOWN_TIME.store(now, std::sync::atomic::Ordering::Relaxed);
            }
            let down_time = DOWN_TIME.load(std::sync::atomic::Ordering::Relaxed);
            let event = crate::jni::motion_event(touch.x, touch.y);
            stubs::set_phase("GameActivity.onTouchEventNative".to_owned());
            // SAFETY: AGDK's native with signature
            // (JLandroid/view/MotionEvent;IIIIIJJIIIIIIFF)Z: pointer count,
            // history size, device, source, action, event time, down time,
            // flags, meta state, action button, button state,
            // classification, edge flags, x/y precision.
            let touch_native: extern "C" fn(
                *mut c_void,
                *mut c_void,
                i64,
                *mut c_void,
                c_int,
                c_int,
                c_int,
                c_int,
                c_int,
                i64,
                i64,
                c_int,
                c_int,
                c_int,
                c_int,
                c_int,
                c_int,
                f32,
                f32,
            ) -> u8 = unsafe { std::mem::transmute(native("onTouchEventNative")?) };
            let handled = touch_native(
                env,
                activity,
                handle,
                event,
                1,
                0,
                1,
                source,
                touch.action as c_int,
                now,
                down_time,
                0,
                0,
                action_button,
                button_state,
                0,
                0,
                1.0,
                1.0,
            );
            static LOGGED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            if touch.action != TouchAction::Hover
                && LOGGED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 12
            {
                eprintln!(
                    "feline-runtime: touch {:?} at ({:.0}, {:.0}) -> {}",
                    touch.action,
                    touch.x,
                    touch.y,
                    if handled != 0 { "taken" } else { "not taken" }
                );
            }
        }
        WindowChange::Key(key) => {
            static DOWN_TIME: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
            let now = uptime_millis();
            if key.down && !key.repeat {
                DOWN_TIME.store(now, std::sync::atomic::Ordering::Relaxed);
            }
            let event = crate::jni::key_event(
                key,
                now,
                DOWN_TIME.load(std::sync::atomic::Ordering::Relaxed),
            );
            let name = if key.down {
                "onKeyDownNative"
            } else {
                "onKeyUpNative"
            };
            stubs::set_phase(format!("GameActivity.{name}"));
            // SAFETY: AGDK's natives with signature (JLandroid/view/KeyEvent;)Z.
            let key_native: extern "C" fn(*mut c_void, *mut c_void, i64, *mut c_void) -> u8 =
                unsafe { std::mem::transmute(native(name)?) };
            key_native(env, activity, handle, event);
        }
    }
    stubs::set_phase(String::new());
    Ok(())
}

/// The FELINE_INPUT_TEST events: tap, drag a little, release; W down, up.
fn synthetic_input() -> Vec<crate::window::WindowChange> {
    use crate::window::{Key, Touch, TouchAction, WindowChange};
    let (w, h) = crate::window::native_window().map_or((1280, 720), |n| n.size());
    let (x, y) = (w as f32 / 2.0, h as f32 / 2.0);
    eprintln!("feline-runtime: input test: tap at {x}x{y}, then W");
    let touch = |action, x| WindowChange::Touch(Touch { action, x, y });
    let key = |down| {
        WindowChange::Key(Key {
            down,
            code: 51, // KEYCODE_W
            scan_code: 17,
            meta: 0,
            unicode: 'w' as i32,
            repeat: false,
        })
    };
    vec![
        touch(TouchAction::Down, x),
        touch(TouchAction::Move, x + 5.0),
        touch(TouchAction::Up, x + 5.0),
        key(true),
        key(false),
    ]
}

/// FELINE_INPUT_SCRIPT: timed test input, steps separated by `;`, each
/// `seconds:action`: `tap x,y`, `type text` (each character pressed and
/// released), `key code` (an Android key code, e.g. 66 for Enter), or
/// `shot path` (the next frame saved as a PPM image), or `size w,h` (resize
/// the window). For testing without
/// anyone at the window.
fn input_script() -> Vec<(f64, String)> {
    let Ok(script) = std::env::var("FELINE_INPUT_SCRIPT") else {
        return Vec::new();
    };
    let mut steps: Vec<(f64, String)> = script
        .split(';')
        .filter_map(|step| {
            let (at, action) = step.trim().split_once(':')?;
            Some((at.trim().parse().ok()?, action.trim().to_owned()))
        })
        .collect();
    steps.sort_by(|a, b| a.0.total_cmp(&b.0));
    steps
}

/// The window changes for one script action.
fn script_action(action: &str) -> Vec<crate::window::WindowChange> {
    use crate::window::{Key, Touch, TouchAction, WindowChange};
    eprintln!("feline-runtime: input script: {action}");
    let key = |down, code, unicode| {
        WindowChange::Key(Key {
            down,
            code,
            scan_code: 0,
            meta: 0,
            unicode,
            repeat: false,
        })
    };
    let (verb, rest) = action.split_once(' ').unwrap_or((action, ""));
    match verb {
        "tap" => {
            let mut xy = rest.split(',').filter_map(|v| v.trim().parse::<f32>().ok());
            let (Some(x), Some(y)) = (xy.next(), xy.next()) else {
                return Vec::new();
            };
            let touch = |action| WindowChange::Touch(Touch { action, x, y });
            vec![
                touch(TouchAction::Hover),
                touch(TouchAction::Down),
                touch(TouchAction::Up),
            ]
        }
        "down" | "up" => {
            let mut xy = rest.split(',').filter_map(|v| v.trim().parse::<f32>().ok());
            let (Some(x), Some(y)) = (xy.next(), xy.next()) else {
                return Vec::new();
            };
            let action = if verb == "down" {
                TouchAction::Down
            } else {
                TouchAction::Up
            };
            vec![WindowChange::Touch(Touch { action, x, y })]
        }
        "type" => rest
            .chars()
            .flat_map(|c| [key(true, 0, c as i32), key(false, 0, c as i32)])
            .collect(),
        // `key CODE [SCAN [CHAR]]`: an Android key code, optionally with the
        // evdev scan code and the character a real keyboard sends with it
        // (e.g. `key 111 1` for Esc, `key 40 38 l` for L).
        "key" | "keydown" | "keyup" => {
            let mut parts = rest.split_whitespace();
            let Some(code) = parts.next().and_then(|c| c.parse().ok()) else {
                return Vec::new();
            };
            let scan: i32 = parts.next().and_then(|c| c.parse().ok()).unwrap_or(0);
            let unicode = parts.next().and_then(|c| c.chars().next()).map_or(0, |c| c as i32);
            let with = |down| {
                WindowChange::Key(Key {
                    down,
                    code,
                    scan_code: scan,
                    meta: 0,
                    unicode,
                    repeat: false,
                })
            };
            match verb {
                "keydown" => vec![with(true)],
                "keyup" => vec![with(false)],
                _ => vec![with(true), with(false)],
            }
        }
        "shot" => {
            crate::egl::request_screenshot(rest.trim());
            Vec::new()
        }
        _ => Vec::new(),
    }
}

/// Milliseconds since boot, like Android's `SystemClock.uptimeMillis()`
/// (which event times use).
fn uptime_millis() -> i64 {
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: a valid timespec to fill.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) };
    now.tv_sec * 1000 + now.tv_nsec / 1_000_000
}

/// The main loop with a window: window events (forwarded to the engine)
/// and the main looper, until `duration` passes or the window is closed.
pub fn run_window_loop(
    window: &mut crate::window::DesktopWindow,
    handle: i64,
    duration: std::time::Duration,
) -> Result<&'static str, String> {
    let start = std::time::Instant::now();
    let end = start + duration;
    let mut first = true;
    // FELINE_INPUT_TEST=1: after 8 s, a tap in the middle of the window and
    // a press of W, to test input without anyone at the keyboard.
    let mut input_test = std::env::var_os("FELINE_INPUT_TEST").is_some();
    let mut script = input_script().into_iter().peekable();
    while std::time::Instant::now() < end {
        if let Some(env) = crate::jni::current_env() {
            finish_left_text_box(env);
            refresh_text_box_info(env);
            pass_refresh_rates(env);
            crate::gamepad::poll(env);
            crate::versions::tick();
        }
        let mut changes = window.pump(std::time::Duration::from_millis(1));
        while let Some((at, _)) = script.peek()
            && start.elapsed().as_secs_f64() >= *at
        {
            let (_, action) = script.next().expect("peeked");
            // `size W,H`: resize the window.
            if let Some((w, h)) = action
                .strip_prefix("size ")
                .and_then(|r| r.split_once(','))
                .and_then(|(w, h)| Some((w.trim().parse().ok()?, h.trim().parse().ok()?)))
            {
                eprintln!("feline-runtime: input script: {action}");
                window.request_size(w, h);
                continue;
            }
            changes.extend(script_action(&action));
        }
        if input_test && start.elapsed() >= std::time::Duration::from_secs(8) {
            input_test = false;
            changes.extend(synthetic_input());
        }
        if first {
            // The window already exists when the loop starts.
            if let Some(native) = crate::window::native_window() {
                let (w, h) = native.size();
                changes.insert(
                    0,
                    crate::window::WindowChange::Created {
                        width: w,
                        height: h,
                    },
                );
            }
            first = false;
        }
        for change in changes {
            if change == crate::window::WindowChange::CloseRequested {
                return Ok("the window was closed");
            }
            window_changed(handle, change)?;
        }
        stubs::set_phase("the main looper".to_owned());
        // SAFETY: NULL out pointers are allowed.
        unsafe {
            crate::looper::ALooper_pollOnce(
                15,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
        }
        stubs::set_phase(String::new());
    }
    Ok("time is up")
}

/// The exported symbol of a JNI native (the JNI specification's short
/// name): `Java_` + class and method with `/` as `_`, `_` as `_1` and `$`
/// as `_00024`.
pub fn jni_symbol(class: &str, method: &str) -> String {
    let mangle = |text: &str| {
        let mut out = String::new();
        for c in text.chars() {
            match c {
                '/' => out.push('_'),
                '_' => out.push_str("_1"),
                '$' => out.push_str("_00024"),
                c => out.push(c),
            }
        }
        out
    };
    format!("Java_{}_{}", mangle(class), mangle(method))
}

/// Calls one of the engine's exported natives as Roblox's Java code would:
/// `Java_<class>_<method>(JNIEnv*, jclass or this, args…)`. For an instance
/// method, `this` is a new (empty) instance of the class. Argument and
/// return types come from the method's DEX declaration (see dex.rs); all
/// are passed and returned as machine words here (objects, ints and longs
/// are all word sized in registers on x86-64).
pub fn call_java_native(
    elf: &Elf,
    image: &Image,
    class: &str,
    method: &str,
    is_static: bool,
    args: &[usize],
) -> Result<usize, String> {
    let name = jni_symbol(class, method);
    let symbols = elf.symbols().map_err(|e| e.to_string())?;
    let entry = symbols
        .iter()
        .find(|s| s.defined && s.name == name)
        .ok_or_else(|| format!("no {name}"))?;
    let env = crate::jni::current_env().ok_or("no JNIEnv for the main thread")? as usize;
    let receiver = if is_static {
        crate::jni::new_class(class)
    } else {
        crate::jni::new_instance(class)
    } as usize;
    let address = (image.bias() + entry.value) as usize;
    let short = class.rsplit('/').next().unwrap_or(class);
    eprintln!("feline-runtime: calling {short}.{method}");
    stubs::set_phase(format!("{short}.{method}"));
    // SAFETY: the engine's exported JNI function; the arguments follow its
    // DEX declaration (all word-sized).
    let result = unsafe {
        match args {
            [] => std::mem::transmute::<usize, extern "C" fn(usize, usize) -> usize>(address)(
                env, receiver,
            ),
            [a] => std::mem::transmute::<usize, extern "C" fn(usize, usize, usize) -> usize>(
                address,
            )(env, receiver, *a),
            [a, b] => std::mem::transmute::<
                usize,
                extern "C" fn(usize, usize, usize, usize) -> usize,
            >(address)(env, receiver, *a, *b),
            [a, b, c] => std::mem::transmute::<
                usize,
                extern "C" fn(usize, usize, usize, usize, usize) -> usize,
            >(address)(env, receiver, *a, *b, *c),
            _ => return Err("too many arguments".into()),
        }
    };
    stubs::set_phase(String::new());
    Ok(result)
}

/// The engine's `NativeGLInterface.nativeAppBridgeV2UpdateSurfaceAppWithPlatformParams`,
/// once the app has started (0 before).
static APP_SURFACE_UPDATE: AtomicUsize = AtomicUsize::new(0);

/// Remembers the app's surface update, to call it when the window changes
/// size (see [`window_changed`]). Call once the app has started.
pub fn remember_app_surface_update(elf: &Elf, image: &Image) -> Result<(), String> {
    let name = jni_symbol(
        "com/roblox/engine/jni/NativeGLInterface",
        "nativeAppBridgeV2UpdateSurfaceAppWithPlatformParams",
    );
    let symbols = elf.symbols().map_err(|e| e.to_string())?;
    let entry = symbols
        .iter()
        .find(|s| s.defined && s.name == name)
        .ok_or_else(|| format!("no {name}"))?;
    APP_SURFACE_UPDATE.store((image.bias() + entry.value) as usize, Ordering::Release);
    Ok(())
}

/// Gives Roblox's app screens the window's current surface and size.
fn update_app_surface(env: *mut c_void) {
    let address = APP_SURFACE_UPDATE.load(Ordering::Acquire);
    if address == 0 {
        return;
    }
    stubs::set_phase("NativeGLInterface.nativeAppBridgeV2UpdateSurfaceAppWithPlatformParams".to_owned());
    let class = crate::jni::new_class("com/roblox/engine/jni/NativeGLInterface");
    let platform = crate::jni::new_instance("com/roblox/engine/jni/model/PlatformParams");
    // SAFETY: the engine's static JNI function, signature
    // (Landroid/view/Surface;Lcom/roblox/engine/jni/model/PlatformParams;)V.
    let update: extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *mut c_void) =
        unsafe { std::mem::transmute(address) };
    update(env, class, crate::window::surface_object(), platform);
}

/// The engine's `nativePassSupportedRefreshRates([F)V` and
/// `nativePassCurrentDisplayRefreshRate(F)V`, once found (0 before).
static REFRESH_NATIVES: std::sync::Mutex<(usize, usize)> = std::sync::Mutex::new((0, 0));

/// Finds the functions that tell the engine the monitor's refresh rates,
/// as Android's Java side does for the phone's screen. Without them the
/// engine assumes 60 Hz and Roblox's in-game Maximum Frame Rate couldn't
/// go higher. They're called by [`pass_refresh_rates`] once the monitor is
/// known.
pub fn remember_refresh_natives(elf: &Elf, image: &Image) -> Result<(), String> {
    const GL: &str = "com/roblox/engine/jni/NativeGLInterface";
    let symbols = elf.symbols().map_err(|e| e.to_string())?;
    let find = |method: &str| {
        let name = jni_symbol(GL, method);
        symbols
            .iter()
            .find(|s| s.defined && s.name == name)
            .map(|s| (image.bias() + s.value) as usize)
            .ok_or_else(|| format!("no {name}"))
    };
    *REFRESH_NATIVES.lock().unwrap_or_else(|p| p.into_inner()) = (
        find("nativePassSupportedRefreshRates")?,
        find("nativePassCurrentDisplayRefreshRate")?,
    );
    Ok(())
}

/// Passes the monitor's refresh rates once (when they're known). From the
/// window loop.
fn pass_refresh_rates(env: *mut c_void) {
    static PASSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if PASSED.load(Ordering::Relaxed) {
        return;
    }
    let (supported_address, current_address) =
        *REFRESH_NATIVES.lock().unwrap_or_else(|p| p.into_inner());
    let Some((current, supported)) = crate::window::refresh_rates() else {
        return;
    };
    if supported_address == 0 || current_address == 0 {
        return;
    }
    PASSED.store(true, Ordering::Relaxed);
    eprintln!("feline-runtime: telling the engine about {current} Hz (supported {supported:?})");
    let class = crate::jni::new_class("com/roblox/engine/jni/NativeGLInterface");
    stubs::set_phase("NativeGLInterface.nativePassSupportedRefreshRates".to_owned());
    // SAFETY: static JNI functions with signatures ([F)V and (F)V; the
    // float goes in a vector register, as C passes it.
    let pass_supported: extern "C" fn(*mut c_void, *mut c_void, *mut c_void) =
        unsafe { std::mem::transmute(supported_address) };
    pass_supported(env, class, crate::jni::new_float_array(&supported));
    stubs::set_phase("NativeGLInterface.nativePassCurrentDisplayRefreshRate".to_owned());
    let pass_current: extern "C" fn(*mut c_void, *mut c_void, f32) =
        unsafe { std::mem::transmute(current_address) };
    pass_current(env, class, current);
    stubs::set_phase(String::new());
}

/// [`call_java_native`] for `com.roblox.engine.jni.NativeGLInterface`'s
/// static natives.
pub fn call_gl_native(
    elf: &Elf,
    image: &Image,
    method: &str,
    args: &[usize],
) -> Result<usize, String> {
    call_java_native(
        elf,
        image,
        "com/roblox/engine/jni/NativeGLInterface",
        method,
        true,
        args,
    )
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
    fn jni_symbols_are_mangled() {
        assert_eq!(
            jni_symbol(
                "com/roblox/engine/jni/NativeGLInterface",
                "nativeGameGlobalInit"
            ),
            "Java_com_roblox_engine_jni_NativeGLInterface_nativeGameGlobalInit"
        );
        assert_eq!(jni_symbol("a/B$C", "do_it"), "Java_a_B_00024C_do_1it");
    }

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

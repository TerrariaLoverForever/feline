// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! EGL for the engine, on the host's EGL (glvnd: NVIDIA or Mesa).
//!
//! EGL is the same Khronos C API on Android and on Linux: same types, same
//! constants, same calling convention, so most `egl*` imports are forwarded
//! unchanged (see `imports.rs`). Only what involves the *platform* differs:
//!
//! - `eglGetDisplay(EGL_DEFAULT_DISPLAY)` means "the screen" on Android. Here
//!   it becomes the EGL display of Feline's window (Wayland or X11).
//! - `eglCreateWindowSurface` gets an `ANativeWindow*`. Here that's Feline's
//!   [`NativeWindow`], turned into what the host's EGL expects: a
//!   `wl_egl_window` (libwayland-egl) on Wayland, the window ID on X11.
//! - `eglChooseConfig` may ask for Android-only attributes
//!   (`EGL_RECORDABLE_ANDROID`, …), which desktop drivers reject; they're
//!   removed from the list.
//! - `eglSwapBuffers` is the host's, counted (frames presented).
//! - `eglGetProcAddress` returns these versions for their names, so a lookup
//!   can't bypass them.
//!
//! # `unsafe` in this module
//!
//! Calling the host's EGL and libwayland-egl through `dlsym` pointers, and
//! reading the engine's attribute lists.

#![allow(unsafe_code)]
#![allow(non_snake_case)] // EGL's exact names

use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::window::{NativeHandles, NativeWindow, native_window};

type EglDisplay = *mut c_void;
type EglConfig = *mut c_void;
type EglSurface = *mut c_void;
type EglInt = i32;
type EglBoolean = u32;

const EGL_NONE: EglInt = 0x3038;
const EGL_PLATFORM_X11_KHR: u32 = 0x31d5;
const EGL_PLATFORM_WAYLAND_KHR: u32 = 0x31d8;
/// Android-only config attributes (AOSP `EGL/eglext.h`).
const ANDROID_ONLY: &[EglInt] = &[
    0x3142, // EGL_RECORDABLE_ANDROID
    0x3147, // EGL_FRAMEBUFFER_TARGET_ANDROID
    0x314c, // EGL_FRONT_BUFFER_AUTO_REFRESH_ANDROID
];

/// The host functions this module calls.
struct Host {
    get_display: unsafe extern "C" fn(usize) -> EglDisplay,
    get_platform_display:
        Option<unsafe extern "C" fn(u32, *mut c_void, *const isize) -> EglDisplay>,
    choose_config: unsafe extern "C" fn(
        EglDisplay,
        *const EglInt,
        *mut EglConfig,
        EglInt,
        *mut EglInt,
    ) -> EglBoolean,
    create_window_surface:
        unsafe extern "C" fn(EglDisplay, EglConfig, usize, *const EglInt) -> EglSurface,
    get_proc_address: unsafe extern "C" fn(*const c_char) -> *mut c_void,
    get_error: unsafe extern "C" fn() -> EglInt,
    swap_buffers: unsafe extern "C" fn(EglDisplay, EglSurface) -> EglBoolean,
    swap_interval: unsafe extern "C" fn(EglDisplay, EglInt) -> EglBoolean,
    wl_egl_window_create: Option<unsafe extern "C" fn(*mut c_void, i32, i32) -> *mut c_void>,
    wl_egl_window_resize: Option<unsafe extern "C" fn(*mut c_void, i32, i32, i32, i32)>,
}

fn open(name: &str) -> *mut c_void {
    let name = CString::new(name).expect("no NUL");
    // SAFETY: a valid C string; the library is never closed.
    unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) }
}

fn symbol(library: *mut c_void, name: &str) -> Option<*mut c_void> {
    if library.is_null() {
        return None;
    }
    let name = CString::new(name).ok()?;
    // SAFETY: a live library handle and a valid C string.
    let address = unsafe { libc::dlsym(library, name.as_ptr()) };
    (!address.is_null()).then_some(address)
}

/// Turns a `dlsym` result into a function pointer of type `F`.
///
/// # Safety
/// `F` is the C signature of the symbol.
unsafe fn function<F: Copy>(address: Option<*mut c_void>) -> Option<F> {
    // SAFETY: the caller's promise; F is a function pointer (pointer-sized).
    address.map(|a| unsafe { std::mem::transmute_copy::<*mut c_void, F>(&a) })
}

fn host() -> Option<&'static Host> {
    static HOST: OnceLock<Option<Host>> = OnceLock::new();
    HOST.get_or_init(|| {
        let egl = open("libEGL.so.1");
        let wayland = open("libwayland-egl.so.1");
        // SAFETY: each type is the function's C signature from the EGL 1.5
        // and wayland-egl headers.
        unsafe {
            Some(Host {
                get_display: function(symbol(egl, "eglGetDisplay"))?,
                get_platform_display: function(symbol(egl, "eglGetPlatformDisplay")),
                choose_config: function(symbol(egl, "eglChooseConfig"))?,
                create_window_surface: function(symbol(egl, "eglCreateWindowSurface"))?,
                get_proc_address: function(symbol(egl, "eglGetProcAddress"))?,
                get_error: function(symbol(egl, "eglGetError"))?,
                swap_buffers: function(symbol(egl, "eglSwapBuffers"))?,
                swap_interval: function(symbol(egl, "eglSwapInterval"))?,
                wl_egl_window_create: function(symbol(wayland, "wl_egl_window_create")),
                wl_egl_window_resize: function(symbol(wayland, "wl_egl_window_resize")),
            })
        }
    })
    .as_ref()
}

fn log(what: String) {
    eprintln!("feline-runtime: egl: {what}");
}

/// The window's `wl_egl_window`, once a surface was created (Wayland).
static WL_EGL_WINDOW: AtomicUsize = AtomicUsize::new(0);

/// `EGLDisplay eglGetDisplay(EGLNativeDisplayType)`
///
/// # Safety
/// Called by the engine like EGL's.
pub unsafe extern "C" fn eglGetDisplay(native: usize) -> EglDisplay {
    let Some(host) = host() else {
        log("no host EGL (libEGL.so.1)".into());
        return std::ptr::null_mut();
    };
    let platform = match (native, native_window().map(|w| w.handles)) {
        (0, Some(NativeHandles::Wayland { display, .. })) => {
            Some((EGL_PLATFORM_WAYLAND_KHR, display, "Wayland"))
        }
        (0, Some(NativeHandles::Xlib { display, .. })) => {
            Some((EGL_PLATFORM_X11_KHR, display, "X11"))
        }
        _ => None,
    };
    let display = match (platform, host.get_platform_display) {
        (Some((kind, native_display, name)), Some(get_platform)) => {
            // SAFETY: a live display connection of the named platform.
            let d = unsafe { get_platform(kind, native_display as *mut c_void, std::ptr::null()) };
            log(format!("display -> {name} {d:p}"));
            d
        }
        // No window (or XCB): the host's default display.
        // SAFETY: EGL accepts EGL_DEFAULT_DISPLAY.
        _ => unsafe { (host.get_display)(native) },
    };
    if display.is_null() {
        // SAFETY: a plain EGL call.
        log(format!("no display (error {:#x})", unsafe {
            (host.get_error)()
        }));
    }
    display
}

/// The engine's attribute list without Android-only attributes.
///
/// # Safety
/// `attributes` is NULL or an EGL_NONE-terminated list of pairs.
unsafe fn desktop_attributes(attributes: *const EglInt) -> Vec<EglInt> {
    let mut out = Vec::new();
    if !attributes.is_null() {
        let mut at = attributes;
        // SAFETY: pairs until EGL_NONE, as EGL requires of callers.
        unsafe {
            while *at != EGL_NONE {
                let (name, value) = (*at, *at.add(1));
                if ANDROID_ONLY.contains(&name) {
                    log(format!(
                        "leaving out Android-only attribute {name:#x}={value}"
                    ));
                } else {
                    out.extend([name, value]);
                }
                at = at.add(2);
            }
        }
    }
    out.push(EGL_NONE);
    out
}

/// `EGLBoolean eglChooseConfig(dpy, attribs, configs, size, count)`
///
/// # Safety
/// Called by the engine like EGL's.
pub unsafe extern "C" fn eglChooseConfig(
    display: EglDisplay,
    attributes: *const EglInt,
    configs: *mut EglConfig,
    size: EglInt,
    count: *mut EglInt,
) -> EglBoolean {
    let Some(host) = host() else { return 0 };
    // SAFETY: the engine's list, per EGL's rules.
    let list = unsafe { desktop_attributes(attributes) };
    // SAFETY: forwarding the engine's own output pointers.
    let ok = unsafe { (host.choose_config)(display, list.as_ptr(), configs, size, count) };
    let found = if count.is_null() {
        -1
    } else {
        // SAFETY: EGL wrote it.
        unsafe { *count }
    };
    log(format!("eglChooseConfig -> {ok}, {found} configs"));
    ok
}

/// `EGLSurface eglCreateWindowSurface(dpy, config, ANativeWindow*, attribs)`
///
/// # Safety
/// Called by the engine like EGL's.
pub unsafe extern "C" fn eglCreateWindowSurface(
    display: EglDisplay,
    config: EglConfig,
    window: *const NativeWindow,
    attributes: *const EglInt,
) -> EglSurface {
    let Some(host) = host() else {
        return std::ptr::null_mut();
    };
    let Some(ours) = native_window().filter(|w| std::ptr::eq(*w, window)) else {
        log(format!(
            "eglCreateWindowSurface for an unknown window {window:p}"
        ));
        return std::ptr::null_mut();
    };
    let native = match ours.handles {
        NativeHandles::Wayland { surface, .. } => {
            let mut wl = WL_EGL_WINDOW.load(Ordering::Acquire);
            if wl == 0 {
                let Some(create) = host.wl_egl_window_create else {
                    log("no libwayland-egl.so.1".into());
                    return std::ptr::null_mut();
                };
                let (w, h) = ours.size();
                // SAFETY: the window's live wl_surface.
                wl = unsafe { create(surface as *mut c_void, w, h) } as usize;
                WL_EGL_WINDOW.store(wl, Ordering::Release);
            }
            wl
        }
        NativeHandles::Xlib { window, .. } => window as usize,
        NativeHandles::Xcb { window, .. } => window as usize,
    };
    // SAFETY: the engine's list, per EGL's rules.
    let list = unsafe { desktop_attributes(attributes) };
    // SAFETY: a native window of the display's platform.
    let surface = unsafe { (host.create_window_surface)(display, config, native, list.as_ptr()) };
    if surface.is_null() {
        // SAFETY: a plain EGL call.
        log(format!(
            "eglCreateWindowSurface failed (error {:#x})",
            unsafe { (host.get_error)() }
        ));
    } else {
        let (w, h) = ours.size();
        log(format!("window surface {surface:p} ({w}x{h})"));
        // A new surface starts with an interval of 1, even at the address
        // of one destroyed before it (common: resizing and joining a game
        // reused it, so vsync stayed on and games capped at the monitor's
        // 240 Hz). Its first swap sets 0 again.
        NO_VSYNC_SURFACE.store(0, Ordering::Relaxed);
    }
    surface
}

/// `eglGetProcAddress(name)`: Feline's versions for the names above, the
/// host's for everything else (GL ES functions included).
///
/// # Safety
/// `name` is a valid C string.
pub unsafe extern "C" fn eglGetProcAddress(name: *const c_char) -> *mut c_void {
    if name.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: the caller's promise.
    let text = unsafe { CStr::from_ptr(name) }.to_string_lossy();
    if let Some(address) = implementation(&text) {
        return address as *mut c_void;
    }
    // SAFETY: as above.
    host().map_or(std::ptr::null_mut(), |h| unsafe {
        (h.get_proc_address)(name)
    })
}

/// Frames the engine presented.
static FRAMES: AtomicUsize = AtomicUsize::new(0);

/// How many frames the engine has presented so far.
pub fn frames() -> usize {
    FRAMES.load(Ordering::Relaxed)
}

/// `EGLBoolean eglSwapBuffers(dpy, surface)`: the host's, counted (the
/// first frame, and every power of ten, is logged).
///
/// # Safety
/// Called by the engine like EGL's.
pub unsafe extern "C" fn eglSwapBuffers(display: EglDisplay, surface: EglSurface) -> EglBoolean {
    let Some(host) = host() else { return 0 };
    // Before the first frame of each surface: no waiting for the
    // compositor (see `eglSwapInterval`), even if the engine never sets an
    // interval. The interval belongs to the surface, and the engine makes
    // new ones (e.g. joining a game): setting it only once left games
    // synced to the monitor, capped at its refresh rate.
    if NO_VSYNC_SURFACE.swap(surface as usize, Ordering::Relaxed) != surface as usize {
        // SAFETY: the engine's display, with its context current here.
        unsafe { (host.swap_interval)(display, 0) };
    }
    // The focused text box's text, which Android's keyboard would show.
    if let Some((w, h)) = crate::window::native_window().map(|n| n.size()) {
        crate::overlay::draw(w, h);
    }
    if let Some(path) = SCREENSHOT.lock().unwrap_or_else(|p| p.into_inner()).take() {
        save_screenshot(&path);
    }
    // SAFETY: forwarding the engine's own handles.
    let started = std::time::Instant::now();
    let ok = unsafe { (host.swap_buffers)(display, surface) };
    let spent = started.elapsed();
    let n = FRAMES.fetch_add(1, Ordering::Relaxed) + 1;
    swap_timing(spent);
    if n == 1 || (n.is_power_of_two() && n >= 64) {
        if ok == 0 {
            // SAFETY: a plain EGL call.
            log(format!("frame {n}: swap failed (error {:#x})", unsafe {
                (host.get_error)()
            }));
        } else {
            log(format!("frame {n} presented"));
        }
    }
    ok
}

/// FELINE_SWAP_LOG=1: every 64 frames, the time spent in the host's
/// eglSwapBuffers and between swaps (the engine's own frame time).
fn swap_timing(spent: std::time::Duration) {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static ON: OnceLock<bool> = OnceLock::new();
    if !*ON.get_or_init(|| std::env::var_os("FELINE_SWAP_LOG").is_some()) {
        return;
    }
    static STATE: Mutex<Option<(Instant, Duration, u32)>> = Mutex::new(None);
    let mut state = STATE.lock().unwrap_or_else(|p| p.into_inner());
    let (since, total, count) = state.get_or_insert((Instant::now(), Duration::ZERO, 0));
    *total += spent;
    *count += 1;
    if *count == 64 {
        let wall = since.elapsed();
        log(format!(
            "64 frames: {:.1} ms per frame, {:.1} ms of it in eglSwapBuffers",
            wall.as_secs_f64() * 1000.0 / 64.0,
            total.as_secs_f64() * 1000.0 / 64.0
        ));
        *state = Some((Instant::now(), Duration::ZERO, 0));
    }
}

/// The last surface the swap interval was set to 0 for.
static NO_VSYNC_SURFACE: AtomicUsize = AtomicUsize::new(0);

/// A pending screenshot (FELINE_INPUT_SCRIPT's `shot`): the path to save
/// the next presented frame to.
static SCREENSHOT: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

pub fn request_screenshot(path: &str) {
    *SCREENSHOT.lock().unwrap_or_else(|p| p.into_inner()) = Some(path.to_owned());
}

/// The pending screenshot's path, taken (for Vulkan's presents).
pub fn take_screenshot_request() -> Option<String> {
    SCREENSHOT.lock().unwrap_or_else(|p| p.into_inner()).take()
}

/// Saves the current back buffer (before it's presented) as a binary PPM.
fn save_screenshot(path: &str) {
    type ReadPixels = unsafe extern "C" fn(i32, i32, i32, i32, u32, u32, *mut c_void);
    const GL_RGBA: u32 = 0x1908;
    const GL_UNSIGNED_BYTE: u32 = 0x1401;
    let Some(read) = gl_function::<ReadPixels>("glReadPixels") else {
        return;
    };
    let (w, h) = crate::window::native_window().map_or((0, 0), |n| n.size());
    if w <= 0 || h <= 0 {
        return;
    }
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    // SAFETY: the engine's context is current on this thread; the buffer
    // holds w*h RGBA pixels.
    unsafe {
        read(
            0,
            0,
            w,
            h,
            GL_RGBA,
            GL_UNSIGNED_BYTE,
            rgba.as_mut_ptr().cast(),
        )
    };
    let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
    // GL's rows start at the bottom.
    for row in (0..h as usize).rev() {
        for px in rgba[row * w as usize * 4..(row + 1) * w as usize * 4].chunks(4) {
            ppm.extend_from_slice(&px[..3]);
        }
    }
    match std::fs::write(path, ppm) {
        Ok(()) => log(format!("screenshot saved to {path}")),
        Err(e) => log(format!("couldn't save the screenshot to {path}: {e}")),
    }
}

/// A GL ES function from the host, through eglGetProcAddress.
pub fn gl_function<F: Copy>(name: &str) -> Option<F> {
    let host = host()?;
    let name = CString::new(name).ok()?;
    // SAFETY: a valid C string.
    let address = unsafe { (host.get_proc_address)(name.as_ptr()) };
    // SAFETY: the caller names F as the function's C signature.
    unsafe {
        function(if address.is_null() {
            None
        } else {
            Some(address)
        })
    }
}

/// `EGLBoolean eglSwapInterval(dpy, interval)`: always 0 on the host. On
/// Wayland, with an interval of 1 `eglSwapBuffers` waits for the
/// compositor's frame callback, which KWin (and others) stop sending while
/// the window is hidden behind others or minimized: the game slowed to a few
/// frames per second when you switched windows. The engine limits its own
/// frame rate, so it doesn't need the driver to.
///
/// # Safety
/// Called by the engine like EGL's.
pub unsafe extern "C" fn eglSwapInterval(display: EglDisplay, interval: EglInt) -> EglBoolean {
    let Some(host) = host() else { return 0 };
    static LOGGED: AtomicBool = AtomicBool::new(false);
    if !LOGGED.swap(true, Ordering::Relaxed) {
        log(format!(
            "swap interval {interval} requested, using 0 (no compositor waits)"
        ));
    }
    // SAFETY: the engine's display.
    unsafe { (host.swap_interval)(display, 0) }
}

/// Called when the window is resized: the Wayland EGL window follows
/// (applied by the driver at the engine's next `eglSwapBuffers`).
pub fn window_resized(width: i32, height: i32) {
    let wl = WL_EGL_WINDOW.load(Ordering::Acquire);
    if let (true, Some(resize)) = (wl != 0, host().and_then(|h| h.wl_egl_window_resize)) {
        // SAFETY: a live wl_egl_window (never destroyed).
        unsafe { resize(wl as *mut c_void, width, height, 0, 0) };
    }
}

pub fn implementation(name: &str) -> Option<u64> {
    let address = match name {
        "eglGetDisplay" => eglGetDisplay as unsafe extern "C" fn(usize) -> EglDisplay as usize,
        "eglChooseConfig" => {
            eglChooseConfig
                as unsafe extern "C" fn(
                    EglDisplay,
                    *const EglInt,
                    *mut EglConfig,
                    EglInt,
                    *mut EglInt,
                ) -> EglBoolean as usize
        }
        "eglCreateWindowSurface" => {
            eglCreateWindowSurface
                as unsafe extern "C" fn(
                    EglDisplay,
                    EglConfig,
                    *const NativeWindow,
                    *const EglInt,
                ) -> EglSurface as usize
        }
        "eglSwapInterval" => {
            eglSwapInterval as unsafe extern "C" fn(EglDisplay, EglInt) -> EglBoolean as usize
        }
        "eglSwapBuffers" => {
            eglSwapBuffers as unsafe extern "C" fn(EglDisplay, EglSurface) -> EglBoolean as usize
        }
        "eglGetProcAddress" => {
            eglGetProcAddress as unsafe extern "C" fn(*const c_char) -> *mut c_void as usize
        }
        _ => return None,
    };
    Some(address as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn android_only_attributes_are_left_out() {
        let list = [0x3024, 8, 0x3142, 1, 0x3040, 4, EGL_NONE];
        // SAFETY (test): an EGL_NONE-terminated list.
        let out = unsafe { desktop_attributes(list.as_ptr()) };
        assert_eq!(out, [0x3024, 8, 0x3040, 4, EGL_NONE]);
        // SAFETY (test): NULL is allowed.
        assert_eq!(unsafe { desktop_attributes(std::ptr::null()) }, [EGL_NONE]);
    }
}

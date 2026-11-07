// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! The engine's window: a real desktop window (Wayland, X11 fallback) behind
//! Android's `ANativeWindow`.
//!
//! On Android, the Java side hands the engine a `Surface`, which native code
//! turns into an `ANativeWindow` with `ANativeWindow_fromSurface`. Feline
//! creates a desktop window with winit, gives the engine a `Surface` object
//! for it, and answers `ANativeWindow_*` from the window. The native window
//! handles (Wayland `wl_display`/`wl_surface` or X11 display/window) are kept
//! for the graphics step (EGL / Vulkan surfaces).
//!
//! winit's events are pumped from the main loop, so they share the main
//! thread with Android's `ALooper`.
//!
//! # `unsafe` in this module
//!
//! None needed: the `ANativeWindow_*` functions compare the engine's pointer
//! with Feline's window before using it.

#![allow(non_snake_case)] // the NDK's exact names

use std::ffi::{c_int, c_void};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
use std::time::Duration;

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::platform::pump_events::EventLoopExtPumpEvents;
use winit::raw_window_handle::{
    HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle,
};
use winit::window::{Window, WindowId};

/// Native handles of the desktop window, for EGL/Vulkan surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeHandles {
    Wayland { display: usize, surface: usize },
    Xlib { display: usize, window: u64 },
    Xcb { connection: usize, window: u32 },
}

/// What the engine's `ANativeWindow*` points to.
pub struct NativeWindow {
    width: AtomicI32,
    height: AtomicI32,
    refs: AtomicUsize,
    pub handles: NativeHandles,
}

impl NativeWindow {
    pub fn size(&self) -> (i32, i32) {
        (
            self.width.load(Ordering::Relaxed),
            self.height.load(Ordering::Relaxed),
        )
    }
}

static WINDOW: OnceLock<&'static NativeWindow> = OnceLock::new();
/// The fake `android.view.Surface` object handed to the engine.
static SURFACE: AtomicUsize = AtomicUsize::new(0);

/// The window, once created.
pub fn native_window() -> Option<&'static NativeWindow> {
    WINDOW.get().copied()
}

/// Something that happened to the window, for the lifecycle code.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WindowChange {
    Created {
        width: i32,
        height: i32,
    },
    Resized {
        width: i32,
        height: i32,
    },
    Focus(bool),
    CloseRequested,
    /// The left mouse button as one finger on a touch screen.
    Touch(Touch),
    Key(Key),
}

/// Android's MotionEvent actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchAction {
    Down = 0,
    Up = 1,
    Move = 2,
}

/// One touch event (window pixels, physical).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Touch {
    pub action: TouchAction,
    pub x: f32,
    pub y: f32,
}

/// One key event, in Android's terms (`KeyEvent`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key {
    pub down: bool,
    /// Android `KEYCODE_*`.
    pub code: i32,
    /// The evdev scan code (what Android reports too).
    pub scan_code: i32,
    /// Android `META_*` bits for Shift/Ctrl/Alt.
    pub meta: i32,
    /// The character the key types, or 0.
    pub unicode: i32,
    pub repeat: bool,
}

struct App {
    window: Option<Window>,
    changes: Vec<WindowChange>,
    error: Option<String>,
    cursor: (f32, f32),
    pressed: bool,
    modifiers: winit::keyboard::ModifiersState,
}

fn handles(window: &Window) -> Result<NativeHandles, String> {
    let display = window.display_handle().map_err(|e| e.to_string())?.as_raw();
    let raw = window.window_handle().map_err(|e| e.to_string())?.as_raw();
    Ok(match (display, raw) {
        (RawDisplayHandle::Wayland(d), RawWindowHandle::Wayland(w)) => NativeHandles::Wayland {
            display: d.display.as_ptr() as usize,
            surface: w.surface.as_ptr() as usize,
        },
        (RawDisplayHandle::Xlib(d), RawWindowHandle::Xlib(w)) => NativeHandles::Xlib {
            display: d.display.map_or(0, |p| p.as_ptr() as usize),
            window: w.window,
        },
        (RawDisplayHandle::Xcb(d), RawWindowHandle::Xcb(w)) => NativeHandles::Xcb {
            connection: d.connection.map_or(0, |p| p.as_ptr() as usize),
            window: w.window.get(),
        },
        other => return Err(format!("unsupported window system: {other:?}")),
    })
}

impl App {
    fn touch(&mut self, action: TouchAction) {
        let (x, y) = self.cursor;
        self.changes
            .push(WindowChange::Touch(Touch { action, x, y }));
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("Feline (runtime test)")
            .with_inner_size(LogicalSize::new(1280.0, 720.0));
        let window = match event_loop.create_window(attributes) {
            Ok(w) => w,
            Err(e) => {
                self.error = Some(format!("couldn't create a window: {e}"));
                return;
            }
        };
        let size = window.inner_size();
        match handles(&window) {
            Ok(handles) => {
                let native: &'static NativeWindow = Box::leak(Box::new(NativeWindow {
                    width: AtomicI32::new(size.width as i32),
                    height: AtomicI32::new(size.height as i32),
                    refs: AtomicUsize::new(1),
                    handles,
                }));
                let _ = WINDOW.set(native);
                eprintln!(
                    "feline-runtime: window {}x{} ({handles:?})",
                    size.width, size.height
                );
                self.changes.push(WindowChange::Created {
                    width: size.width as i32,
                    height: size.height as i32,
                });
            }
            Err(e) => self.error = Some(e),
        }
        self.window = Some(window);
    }

    fn window_event(&mut self, _event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::Resized(size) => {
                if let Some(native) = native_window() {
                    native.width.store(size.width as i32, Ordering::Relaxed);
                    native.height.store(size.height as i32, Ordering::Relaxed);
                    crate::egl::window_resized(size.width as i32, size.height as i32);
                }
                self.changes.push(WindowChange::Resized {
                    width: size.width as i32,
                    height: size.height as i32,
                });
            }
            WindowEvent::CloseRequested => self.changes.push(WindowChange::CloseRequested),
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x as f32, position.y as f32);
                if self.pressed {
                    self.touch(TouchAction::Move);
                }
            }
            WindowEvent::MouseInput {
                state,
                button: winit::event::MouseButton::Left,
                ..
            } => {
                let down = state == winit::event::ElementState::Pressed;
                if down != self.pressed {
                    self.pressed = down;
                    self.touch(if down {
                        TouchAction::Down
                    } else {
                        TouchAction::Up
                    });
                }
            }
            // Losing focus isn't passed on: Roblox's Android code lowers the
            // frame rate for an unfocused window (a phone app in the
            // background), which a desktop window shouldn't do. Only the
            // finger is lifted, so it isn't left "down".
            WindowEvent::Focused(false) => {
                if self.pressed {
                    self.pressed = false;
                    self.touch(TouchAction::Up);
                }
            }
            WindowEvent::Focused(true) => self.changes.push(WindowChange::Focus(true)),
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::KeyboardInput { event, .. } => {
                if let Some(key) = crate::keys::android_key(&event, self.modifiers) {
                    self.changes.push(WindowChange::Key(key));
                }
            }
            _ => {}
        }
    }
}

/// The desktop window and its event loop, driven from the main loop.
pub struct DesktopWindow {
    event_loop: EventLoop<()>,
    app: App,
}

impl DesktopWindow {
    pub fn open() -> Result<Self, String> {
        let event_loop = EventLoop::new().map_err(|e| format!("no window system: {e}"))?;
        let mut window = Self {
            event_loop,
            app: App {
                window: None,
                changes: Vec::new(),
                error: None,
                cursor: (0.0, 0.0),
                pressed: false,
                modifiers: Default::default(),
            },
        };
        // The window is created on the first `resumed` event.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while native_window().is_none()
            && window.app.error.is_none()
            && std::time::Instant::now() < deadline
        {
            window.pump(Duration::from_millis(10));
        }
        if let Some(e) = window.app.error.take() {
            return Err(e);
        }
        native_window().ok_or("the window didn't appear")?;
        Ok(window)
    }

    /// Handles pending window events (waiting at most `timeout`), and returns
    /// what changed.
    pub fn pump(&mut self, timeout: Duration) -> Vec<WindowChange> {
        let _ = self
            .event_loop
            .pump_app_events(Some(timeout), &mut self.app);
        std::mem::take(&mut self.app.changes)
    }
}

/// The `android.view.Surface` object for the window (created on first use).
pub fn surface_object() -> *mut c_void {
    let mut surface = SURFACE.load(Ordering::Relaxed);
    if surface == 0 {
        surface = crate::jni::new_instance("android/view/Surface") as usize;
        SURFACE.store(surface, Ordering::Relaxed);
    }
    surface as *mut c_void
}

// ---------------------------------------------------------------------------
// ANativeWindow
// ---------------------------------------------------------------------------

/// `ANativeWindow* ANativeWindow_fromSurface(JNIEnv*, jobject surface)`
pub extern "C" fn ANativeWindow_fromSurface(
    _env: *mut c_void,
    surface: *mut c_void,
) -> *const NativeWindow {
    match native_window() {
        Some(window) if surface as usize == SURFACE.load(Ordering::Relaxed) => {
            window.refs.fetch_add(1, Ordering::Relaxed);
            window
        }
        _ => {
            eprintln!(
                "feline-runtime: ANativeWindow_fromSurface for an unknown surface {surface:p}"
            );
            std::ptr::null()
        }
    }
}

/// The window behind an engine pointer: only Feline's own window is accepted
/// (NULL or anything else is logged and refused, so a bad pointer can't be
/// dereferenced).
fn ours(window: *const NativeWindow, function: &str) -> Option<&'static NativeWindow> {
    let found = native_window().filter(|w| std::ptr::eq(*w, window));
    if found.is_none() {
        eprintln!("feline-runtime: {function} for an unknown window {window:p}");
    }
    found
}

/// `void ANativeWindow_acquire(ANativeWindow*)`
pub extern "C" fn ANativeWindow_acquire(window: *const NativeWindow) {
    if let Some(w) = ours(window, "ANativeWindow_acquire") {
        w.refs.fetch_add(1, Ordering::Relaxed);
    }
}

/// `void ANativeWindow_release(ANativeWindow*)`
pub extern "C" fn ANativeWindow_release(window: *const NativeWindow) {
    if let Some(w) = ours(window, "ANativeWindow_release") {
        w.refs.fetch_sub(1, Ordering::Relaxed);
    }
}

/// `int32_t ANativeWindow_getWidth(ANativeWindow*)`: negative on error.
pub extern "C" fn ANativeWindow_getWidth(window: *const NativeWindow) -> c_int {
    ours(window, "ANativeWindow_getWidth").map_or(-libc::EINVAL, |w| w.size().0)
}

/// `int32_t ANativeWindow_getHeight(ANativeWindow*)`: negative on error.
pub extern "C" fn ANativeWindow_getHeight(window: *const NativeWindow) -> c_int {
    ours(window, "ANativeWindow_getHeight").map_or(-libc::EINVAL, |w| w.size().1)
}

pub fn implementation(name: &str) -> Option<u64> {
    let address = match name {
        "ANativeWindow_fromSurface" => ANativeWindow_fromSurface as *const () as usize,
        "ANativeWindow_acquire" => ANativeWindow_acquire as *const () as usize,
        "ANativeWindow_release" => ANativeWindow_release as *const () as usize,
        "ANativeWindow_getWidth" => ANativeWindow_getWidth as *const () as usize,
        "ANativeWindow_getHeight" => ANativeWindow_getHeight as *const () as usize,
        _ => return None,
    };
    Some(address as u64)
}

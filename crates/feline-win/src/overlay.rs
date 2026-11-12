// SPDX-License-Identifier: GPL-3.0-or-later
//! A tiny see-through window that floats above everything, for Feline's
//! desk cat. It's a "layered" window: Windows draws our picture with its
//! transparency straight onto the desktop, clicks go through it to whatever
//! is underneath, it never takes the keyboard focus, and it has no taskbar
//! button. It is our own window in our own process: nothing is drawn into
//! any game.
//!
//! A window belongs to the thread that made it, so an [`Overlay`] is made,
//! drawn and dropped on one thread (it isn't `Send`).

use std::ffi::c_void;
use std::marker::PhantomData;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
    CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, HBITMAP, HDC,
    HGDIOBJ, SelectObject,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos,
    GetSystemMetrics, HWND_TOPMOST, MSG, PM_REMOVE, PeekMessageW, RegisterClassExW,
    SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos, ShowWindow,
    TranslateMessage, ULW_ALPHA, UpdateLayeredWindow, WNDCLASSEXW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use crate::wide;

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // SAFETY: Windows calls this with valid arguments for its own window.
    unsafe { DefWindowProcW(hwnd, msg, w, l) }
}

/// The pointer's position on the desktop, in screen pixels.
pub fn cursor_position() -> Option<(i32, i32)> {
    let mut point = POINT { x: 0, y: 0 };
    // SAFETY: `point` is a valid, writable POINT.
    (unsafe { GetCursorPos(&mut point) } != 0).then_some((point.x, point.y))
}

/// Where the pointer is and what's under it (for the animated cursor).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pointer {
    pub x: i32,
    pub y: i32,
    /// A program shows a pointer (false while a game hides it, e.g. while
    /// it holds the mouse for the camera).
    pub visible: bool,
    /// The process whose window is under the pointer (0: none).
    pub over_pid: u32,
    /// The process whose window has the keyboard focus (0: none).
    pub foreground_pid: u32,
}

/// The pointer's state right now.
pub fn pointer() -> Option<Pointer> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CURSOR_SHOWING, CURSORINFO, GA_ROOT, GetAncestor, GetCursorInfo, GetForegroundWindow,
        GetWindowThreadProcessId, WindowFromPoint,
    };
    // SAFETY: CURSORINFO is plain data; cbSize is set as Windows requires.
    let mut info: CURSORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = size_of::<CURSORINFO>() as u32;
    // SAFETY: `info` is a valid, writable CURSORINFO with its size set.
    if unsafe { GetCursorInfo(&mut info) } == 0 {
        return None;
    }
    let pid_of = |hwnd: HWND| {
        if hwnd.is_null() {
            return 0;
        }
        let mut pid = 0u32;
        // SAFETY: any window handle (or a stale one) is fine to ask about.
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        pid
    };
    // SAFETY: plain calls; click-through windows (ours) aren't "under" the
    // pointer for WindowFromPoint.
    let (under, focus) =
        unsafe { (GetAncestor(WindowFromPoint(info.ptScreenPos), GA_ROOT), GetForegroundWindow()) };
    Some(Pointer {
        x: info.ptScreenPos.x,
        y: info.ptScreenPos.y,
        visible: info.flags & CURSOR_SHOWING != 0 && !info.hCursor.is_null(),
        over_pid: pid_of(under),
        foreground_pid: pid_of(focus),
    })
}

/// Makes this process see real screen pixels on high-DPI screens (instead
/// of Windows stretching its windows), and returns the screen's DPI (96 =
/// 100 %). For a helper without other windows.
pub fn dpi_aware() -> u32 {
    use windows_sys::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForSystem, SetProcessDpiAwarenessContext,
    };
    // SAFETY: plain calls; failing (already set) is harmless.
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        GetDpiForSystem()
    }
}

/// The whole desktop (every screen): left, top, width, height.
pub fn desktop_bounds() -> (i32, i32, i32, i32) {
    // SAFETY: plain calls with constant arguments.
    unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }
}

/// The see-through floating window.
pub struct Overlay {
    hwnd: HWND,
    /// A memory drawing surface holding the current picture.
    dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
    size: u32,
    draws: u32,
    /// Windows belong to their thread: not `Send`.
    _not_send: PhantomData<*mut c_void>,
}

impl Overlay {
    /// Makes the (still empty) window, `size` × `size` pixels.
    pub fn new(size: u32) -> Option<Self> {
        let class = wide("FelineDeskCat");
        // SAFETY: GetModuleHandleW(null) is this program; the class struct
        // is fully set up and `class` outlives the calls. Registering twice
        // fails harmlessly (the class already exists).
        let hwnd = unsafe {
            let instance = GetModuleHandleW(null());
            let mut wc: WNDCLASSEXW = std::mem::zeroed();
            wc.cbSize = size_of::<WNDCLASSEXW>() as u32;
            wc.lpfnWndProc = Some(window_proc);
            wc.hInstance = instance;
            wc.lpszClassName = class.as_ptr();
            RegisterClassExW(&wc);
            let title = wide("Feline desk cat");
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class.as_ptr(),
                title.as_ptr(),
                WS_POPUP,
                0,
                0,
                size as i32,
                size as i32,
                null_mut(),
                null_mut(),
                instance,
                null(),
            )
        };
        if hwnd.is_null() {
            return None;
        }
        let mut overlay = Self {
            hwnd,
            dc: null_mut(),
            bitmap: null_mut(),
            old: null_mut(),
            bits: null_mut(),
            size: 0,
            draws: 0,
            _not_send: PhantomData,
        };
        if !overlay.surface(size) {
            return None;
        }
        Some(overlay)
    }

    /// (Re)makes the drawing surface for `size` × `size` pixels.
    fn surface(&mut self, size: u32) -> bool {
        self.free_surface();
        // SAFETY: a memory DC compatible with the screen; the BITMAPINFO is
        // fully set up (top-down 32-bit BGRA); `bits` receives the pixel
        // memory, owned by the bitmap until it's deleted in free_surface.
        unsafe {
            let dc = CreateCompatibleDC(null_mut());
            if dc.is_null() {
                return false;
            }
            let mut info: BITMAPINFO = std::mem::zeroed();
            info.bmiHeader = BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size as i32,
                biHeight: -(size as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..std::mem::zeroed()
            };
            let mut bits: *mut c_void = null_mut();
            let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
            if bitmap.is_null() || bits.is_null() {
                DeleteDC(dc);
                return false;
            }
            self.old = SelectObject(dc, bitmap);
            self.dc = dc;
            self.bitmap = bitmap;
            self.bits = bits.cast();
            self.size = size;
        }
        true
    }

    fn free_surface(&mut self) {
        // SAFETY: only frees what `surface` made, once (fields are reset).
        unsafe {
            if !self.dc.is_null() {
                SelectObject(self.dc, self.old);
                DeleteDC(self.dc);
            }
            if !self.bitmap.is_null() {
                DeleteObject(self.bitmap);
            }
        }
        self.dc = null_mut();
        self.bitmap = null_mut();
        self.bits = null_mut();
        self.size = 0;
    }

    /// Shows `rgba` (`size` × `size` pixels, 8-bit RGBA, each pixel either
    /// fully clear or fully opaque, or premultiplied) with its top-left
    /// corner at (`x`, `y`) on the desktop.
    pub fn draw(&mut self, x: i32, y: i32, rgba: &[u8], size: u32) -> bool {
        let count = (size * size) as usize;
        if rgba.len() < count * 4 || (size != self.size && !self.surface(size)) {
            return false;
        }
        // SAFETY: `bits` points to size × size × 4 bytes of the DIB section
        // (made for this size above), which nothing else touches.
        let pixels = unsafe { std::slice::from_raw_parts_mut(self.bits, count * 4) };
        for (out, px) in pixels.as_chunks_mut::<4>().0.iter_mut().zip(rgba.as_chunks::<4>().0) {
            // Layered windows want premultiplied BGRA.
            let a = u32::from(px[3]);
            let pre = |c: u8| ((u32::from(c) * a + 127) / 255) as u8;
            out.copy_from_slice(&[pre(px[2]), pre(px[1]), pre(px[0]), px[3]]);
        }
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let at = POINT { x, y };
        let extent = SIZE {
            cx: size as i32,
            cy: size as i32,
        };
        let origin = POINT { x: 0, y: 0 };
        // SAFETY: our window and memory DC; every pointer is to a local
        // that lives through the call.
        let ok = unsafe {
            UpdateLayeredWindow(
                self.hwnd,
                null_mut(),
                &at,
                &extent,
                self.dc,
                &origin,
                0,
                &blend,
                ULW_ALPHA,
            ) != 0
        };
        if self.draws == 0 {
            // SAFETY: our own window.
            unsafe { ShowWindow(self.hwnd, SW_SHOWNOACTIVATE) };
        }
        // Stay above windows that became "always on top" after us (every
        // couple of seconds is plenty).
        if self.draws.is_multiple_of(20) {
            // SAFETY: our own window; no move, no resize, no focus.
            unsafe {
                SetWindowPos(
                    self.hwnd,
                    HWND_TOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                )
            };
        }
        self.draws = self.draws.wrapping_add(1);
        ok
    }

    /// Takes the window off the screen until the next [`Overlay::draw`].
    pub fn hide(&mut self) {
        if self.draws > 0 {
            // SAFETY: our own window.
            unsafe { ShowWindow(self.hwnd, windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE) };
            self.draws = 0;
        }
    }

    /// Handles the window's waiting messages (call it regularly).
    pub fn pump(&self) {
        // SAFETY: `msg` is a valid MSG; only this thread's messages.
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        self.free_surface();
        // SAFETY: our own window, destroyed on the thread that made it.
        unsafe { DestroyWindow(self.hwnd) };
    }
}

/// Waits for the next screen refresh (so a pointer-following window moves
/// once per frame). False if Windows' compositor didn't wait.
pub fn wait_for_frame() -> bool {
    // SAFETY: plain call.
    unsafe { windows_sys::Win32::Graphics::Dwm::DwmFlush() >= 0 }
}

/// A small Feline card in the top-right corner of the screen the game is on,
/// for a few seconds (the server pop-up). Like the desk cat, it's a
/// see-through window of Feline's own that never takes the focus and lets
/// clicks through, so it shows over a full-screen game where Windows'
/// own notifications are held back. Doesn't block the caller.
pub fn card(title: &str, text: &str, seconds: u64) {
    let (title, text) = (wide(title), wide(text));
    let _ = std::thread::Builder::new().name("feline-card".into()).spawn(move || {
        // SAFETY: everything below is made, used and freed on this thread;
        // every pointer is to a local that outlives the call using it.
        unsafe { show_card(&title, &text, seconds) };
    });
}

/// See [`card`].
unsafe fn show_card(title: &[u16], text: &[u16], seconds: u64) {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::{
        ANTIALIASED_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, DEFAULT_CHARSET, DEFAULT_PITCH, DT_END_ELLIPSIS,
        DT_NOPREFIX, DT_SINGLELINE, DT_WORDBREAK, DrawTextW, FW_NORMAL, FW_SEMIBOLD, GdiFlush, GetMonitorInfoW,
        MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromWindow, OUT_DEFAULT_PRECIS, SetBkMode, SetTextColor,
        TRANSPARENT,
    };
    use windows_sys::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI, SetThreadDpiAwarenessContext,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, SW_HIDE};

    // SAFETY (whole function): plain Windows calls on objects made here.
    unsafe {
        // Real pixels on this thread, whatever the rest of the program does,
        // so the screen's size and the card's agree.
        SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        // The screen the game (the focused window) is on, and its scale.
        let monitor = MonitorFromWindow(GetForegroundWindow(), MONITOR_DEFAULTTOPRIMARY);
        let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
        GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
        let scale = dpi_x.max(96) as f32 / 96.0;
        let px = |v: f32| (v * scale).round() as i32;
        let (w, h) = (px(340.0), px(78.0));
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(monitor, &mut info) == 0 {
            return;
        }
        let at = POINT { x: info.rcMonitor.right - w - px(24.0), y: info.rcMonitor.top + px(24.0) };

        let class = wide("FelineCard");
        let instance = GetModuleHandleW(null());
        let mut wc: WNDCLASSEXW = std::mem::zeroed();
        wc.cbSize = size_of::<WNDCLASSEXW>() as u32;
        wc.lpfnWndProc = Some(window_proc);
        wc.hInstance = instance;
        wc.lpszClassName = class.as_ptr();
        RegisterClassExW(&wc);
        let name = wide("Feline");
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class.as_ptr(),
            name.as_ptr(),
            WS_POPUP,
            at.x,
            at.y,
            w,
            h,
            null_mut(),
            null_mut(),
            instance,
            null(),
        );
        if hwnd.is_null() {
            return;
        }
        let dc = CreateCompatibleDC(null_mut());
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader = BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..std::mem::zeroed()
        };
        let mut bits: *mut c_void = null_mut();
        let bitmap = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        if dc.is_null() || bitmap.is_null() || bits.is_null() {
            DestroyWindow(hwnd);
            return;
        }
        let old = SelectObject(dc, bitmap);
        let count = (w * h) as usize;
        let pixels = std::slice::from_raw_parts_mut(bits.cast::<u8>(), count * 4);

        // The card: Feline's dark surface with rounded corners and a violet
        // edge on the left. `inside` also gives the corners' soft edge.
        let radius = px(14.0) as f32;
        let inside = |x: i32, y: i32| -> f32 {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let cx = fx.clamp(radius, w as f32 - radius);
            let cy = fy.clamp(radius, h as f32 - radius);
            let d = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
            (radius - d + 0.5).clamp(0.0, 1.0)
        };
        let edge = px(5.0);
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                let bgr: [u8; 3] = if x < edge { [0xff, 0x8c, 0x9d] } else { [0x29, 0x1f, 0x1d] };
                pixels[i..i + 3].copy_from_slice(&bgr);
                pixels[i + 3] = 0;
            }
        }
        let font = |size: f32, weight: u32| {
            let face = wide("Segoe UI");
            CreateFontW(
                -px(size),
                0,
                0,
                0,
                weight as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                OUT_DEFAULT_PRECIS as u32,
                CLIP_DEFAULT_PRECIS as u32,
                ANTIALIASED_QUALITY as u32,
                DEFAULT_PITCH as u32,
                face.as_ptr(),
            )
        };
        let (bold, normal) = (font(15.0, FW_SEMIBOLD), font(13.0, FW_NORMAL));
        SetBkMode(dc, TRANSPARENT as i32);
        let pad = px(18.0);
        let before = SelectObject(dc, bold);
        SetTextColor(dc, 0x00ff_ffff);
        let mut rect = RECT { left: pad, top: px(14.0), right: w - px(14.0), bottom: px(36.0) };
        DrawTextW(dc, title.as_ptr(), -1, &mut rect, DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX);
        SelectObject(dc, normal);
        SetTextColor(dc, 0x00d6_c9c8);
        let mut rect = RECT { left: pad, top: px(38.0), right: w - px(14.0), bottom: h - px(8.0) };
        DrawTextW(dc, text.as_ptr(), -1, &mut rect, DT_WORDBREAK | DT_END_ELLIPSIS | DT_NOPREFIX);
        SelectObject(dc, before);
        GdiFlush();
        // GDI leaves alpha alone: make the card opaque (soft at the corners)
        // and premultiplied, as layered windows want.
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                let a = inside(x, y) * 0.97;
                for c in 0..3 {
                    pixels[i + c] = (f32::from(pixels[i + c]) * a).round() as u8;
                }
                pixels[i + 3] = (a * 255.0).round() as u8;
            }
        }

        let size = SIZE { cx: w, cy: h };
        let origin = POINT { x: 0, y: 0 };
        let show = |alpha: u8| {
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: alpha,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            UpdateLayeredWindow(hwnd, null_mut(), &at, &size, dc, &origin, 0, &blend, ULW_ALPHA);
        };
        let pump_for = |ms: u64| {
            let end = std::time::Instant::now() + std::time::Duration::from_millis(ms);
            while std::time::Instant::now() < end {
                let mut msg: MSG = std::mem::zeroed();
                while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                std::thread::sleep(std::time::Duration::from_millis(15));
            }
        };
        show(0);
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        for step in 1..=10u8 {
            show(step * 25);
            pump_for(18);
        }
        pump_for(seconds * 1000);
        for step in (0..10u8).rev() {
            show(step * 25);
            pump_for(25);
        }
        ShowWindow(hwnd, SW_HIDE);
        SelectObject(dc, old);
        DeleteObject(bold);
        DeleteObject(normal);
        DeleteObject(bitmap);
        DeleteDC(dc);
        DestroyWindow(hwnd);
    }
}

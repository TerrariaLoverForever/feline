// SPDX-License-Identifier: GPL-3.0-or-later
//! A Feline icon by the clock while a game runs, with a small menu (server
//! details, invite link, rejoin…), and copying text to the clipboard.
//!
//! The icon belongs to a hidden window of our own; a click on the icon is
//! noted by the window procedure and the owner asks [`Tray::clicked`] in
//! its own loop, then shows the menu with [`Tray::menu`]. Everything is
//! made, used and dropped on one thread (not `Send`).

use std::cell::Cell;
use std::marker::PhantomData;
use std::path::Path;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetCursorPos, HICON, IDI_APPLICATION, IMAGE_ICON, LR_DEFAULTSIZE, LR_LOADFROMFILE,
    LoadIconW, LoadImageW, MF_GRAYED, MF_SEPARATOR, MF_STRING, MSG, PM_REMOVE, PeekMessageW, PostMessageW,
    RegisterClassExW, SetForegroundWindow, TPM_BOTTOMALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu,
    TranslateMessage, WM_APP, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP, WNDCLASSEXW, WS_POPUP,
};

use crate::wide;

const CALLBACK: u32 = WM_APP + 7;
/// Another copy of Feline asks this one to show its window.
const WAKE: u32 = WM_APP + 8;

thread_local! {
    static CLICKED: Cell<bool> = const { Cell::new(false) };
    static WOKEN: Cell<bool> = const { Cell::new(false) };
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == CALLBACK && matches!(l as u32, WM_LBUTTONUP | WM_RBUTTONUP) {
        CLICKED.with(|c| c.set(true));
        return 0;
    }
    if msg == WAKE {
        WOKEN.with(|c| c.set(true));
        return 0;
    }
    // SAFETY: Windows calls this with valid arguments for its own window.
    unsafe { DefWindowProcW(hwnd, msg, w, l) }
}

/// One line of the tray menu.
pub enum Item<'a> {
    /// Something to click, returning `id`.
    Action(u32, &'a str),
    /// Grey text (information).
    Info(&'a str),
    Separator,
}

/// The tray icon.
pub struct Tray {
    hwnd: HWND,
    icon: HICON,
    own_icon: bool,
    tip: String,
    _not_send: PhantomData<*mut ()>,
}

impl Tray {
    /// Adds the icon (`icon` an `.ico` file; Windows' default otherwise).
    pub fn new(icon: Option<&Path>, tip: &str) -> Option<Self> {
        Self::named(icon, tip, "")
    }

    /// Like [`Tray::new`], with a name other copies of Feline can find it
    /// by (to [`wake`] it).
    pub fn named(icon: Option<&Path>, tip: &str, name: &str) -> Option<Self> {
        let class = wide("FelineTray");
        let title = wide(name);
        // SAFETY: our own class and window; all pointers are to locals that
        // live through the calls. Registering twice fails harmlessly.
        unsafe {
            let instance = GetModuleHandleW(null());
            let mut wc: WNDCLASSEXW = std::mem::zeroed();
            wc.cbSize = size_of::<WNDCLASSEXW>() as u32;
            wc.lpfnWndProc = Some(window_proc);
            wc.hInstance = instance;
            wc.lpszClassName = class.as_ptr();
            RegisterClassExW(&wc);
            let hwnd =
                CreateWindowExW(0, class.as_ptr(), title.as_ptr(), WS_POPUP, 0, 0, 0, 0, null_mut(), null_mut(), instance, null());
            if hwnd.is_null() {
                return None;
            }
            let loaded = icon.map_or(null_mut(), |p| {
                let path = wide(p);
                LoadImageW(null_mut(), path.as_ptr(), IMAGE_ICON, 0, 0, LR_LOADFROMFILE | LR_DEFAULTSIZE)
            });
            let (icon, own_icon) = if loaded.is_null() { (LoadIconW(null_mut(), IDI_APPLICATION), false) } else { (loaded, true) };
            let tray = Self { hwnd, icon, own_icon, tip: tip.to_owned(), _not_send: PhantomData };
            if Shell_NotifyIconW(NIM_ADD, &tray.data()) == 0 {
                return None;
            }
            Some(tray)
        }
    }

    fn data(&self) -> NOTIFYICONDATAW {
        let mut data = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: 1,
            uFlags: NIF_ICON | NIF_TIP | NIF_MESSAGE,
            uCallbackMessage: CALLBACK,
            hIcon: self.icon,
            ..Default::default()
        };
        let units: Vec<u16> = self.tip.encode_utf16().take(data.szTip.len() - 1).collect();
        data.szTip[..units.len()].copy_from_slice(&units);
        data
    }

    /// The text shown when pointing at the icon.
    pub fn set_tip(&mut self, tip: &str) {
        if tip != self.tip {
            tip.clone_into(&mut self.tip);
            // SAFETY: a fully set-up structure for our own icon.
            unsafe { Shell_NotifyIconW(NIM_MODIFY, &self.data()) };
        }
    }

    /// Handles waiting messages; true if the icon was clicked since.
    pub fn clicked(&self) -> bool {
        // SAFETY: `msg` is a valid MSG; only this thread's messages.
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        CLICKED.with(|c| c.replace(false))
    }

    /// True if another copy of Feline asked to be shown since.
    pub fn woken(&self) -> bool {
        WOKEN.with(|c| c.replace(false))
    }

    /// Shows the menu at the pointer and waits for a choice (`None`: the
    /// menu was closed).
    pub fn menu(&self, items: &[Item]) -> Option<u32> {
        // SAFETY: a menu of our own, destroyed before returning; the window
        // is ours. Windows wants it in front so the menu closes on a click
        // elsewhere, and a null message after.
        unsafe {
            let menu = CreatePopupMenu();
            if menu.is_null() {
                return None;
            }
            for item in items {
                match item {
                    Item::Action(id, text) => {
                        let text = wide(text);
                        AppendMenuW(menu, MF_STRING, *id as usize, text.as_ptr());
                    }
                    Item::Info(text) => {
                        let text = wide(text);
                        AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, text.as_ptr());
                    }
                    Item::Separator => {
                        AppendMenuW(menu, MF_SEPARATOR, 0, null());
                    }
                }
            }
            let mut at = POINT { x: 0, y: 0 };
            GetCursorPos(&mut at);
            SetForegroundWindow(self.hwnd);
            let chosen = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, at.x, at.y, 0, self.hwnd, null());
            PostMessageW(self.hwnd, WM_NULL, 0, 0);
            DestroyMenu(menu);
            u32::try_from(chosen).ok().filter(|&c| c > 0)
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        // SAFETY: our own icon and window, removed on the thread that made them.
        unsafe {
            Shell_NotifyIconW(NIM_DELETE, &self.data());
            if self.own_icon {
                DestroyIcon(self.icon);
            }
            DestroyWindow(self.hwnd);
        }
    }
}

/// Asks the tray named `name` (in another Feline) to show its window.
/// False if there's no such tray.
pub fn wake(name: &str) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW;
    let (class, title) = (wide("FelineTray"), wide(name));
    // SAFETY: NUL-terminated names; posting to a found window is safe.
    unsafe {
        let hwnd = FindWindowW(class.as_ptr(), title.as_ptr());
        !hwnd.is_null() && PostMessageW(hwnd, WAKE, 0, 0) != 0
    }
}

/// Puts `text` on the clipboard. False if Windows refused (another
/// program holding it).
pub fn copy_text(text: &str) -> bool {
    use windows_sys::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
    const CF_UNICODETEXT: u32 = 13;
    let units = wide(text);
    // SAFETY: the clipboard is opened and closed here; the memory block is
    // the clipboard's once SetClipboardData succeeds, ours to free otherwise.
    unsafe {
        if OpenClipboard(null_mut()) == 0 {
            return false;
        }
        EmptyClipboard();
        let bytes = units.len() * 2;
        let block = GlobalAlloc(GMEM_MOVEABLE, bytes);
        let mut ok = false;
        if !block.is_null() {
            let dest = GlobalLock(block).cast::<u16>();
            if !dest.is_null() {
                std::ptr::copy_nonoverlapping(units.as_ptr(), dest, units.len());
                GlobalUnlock(block);
                ok = !SetClipboardData(CF_UNICODETEXT, block).is_null();
            }
            if !ok {
                GlobalFree(block);
            }
        }
        CloseClipboard();
        ok
    }
}

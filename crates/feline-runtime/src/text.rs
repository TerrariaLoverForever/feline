// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! Typing into Roblox's text boxes.
//!
//! On Android, a text box doesn't get its text as key presses. When one is
//! focused, the engine asks Java to show the on-screen keyboard
//! (`showKeyboard(long textBox, boolean, byte[] text, NativeTextBoxInfo)`,
//! with the box's current text as UTF-8), and the keyboard sends the whole
//! text back after each edit. Feline plays the keyboard. While a box is
//! focused, typed characters, Backspace, Enter and Escape edit Feline's copy
//! of the text, and each edit goes to the engine as Android's keyboard code
//! sends it: `syncTextboxTextAndCursorPosition2(text, cursor)`, then
//! `nativePassText(textBox, text, false, cursor)`. Enter syncs, then
//! `nativeReturnPressedFromOnScreenKeyboard(textBox)`, then
//! `nativePassText(…, true, …)`; Escape is `nativeReleaseFocus(textBox)`.
//! Cursors count UTF-16 units, as Java strings do. (Order and units follow
//! mocktail's text editor, Apache-2.0, github.com/komaruworld/mocktail.)
//!
//! The cursor stays at the end of the text for now.

use std::sync::Mutex;

struct Session {
    text_box: i64,
    text: String,
    /// Ctrl+A: the whole text is selected (typing replaces it).
    all_selected: bool,
    /// Where and how the box is shown (from its first, sized report).
    info: BoxInfo,
    /// The text when it was focused (or last set by the engine): a box
    /// left with this text wasn't typed into.
    initial: String,
}

static FOCUSED: Mutex<Option<Session>> = Mutex::new(None);

/// Where and how the engine shows the focused text box, from the
/// `NativeTextBoxInfo` it builds for showKeyboard: (x, y, width, height,
/// fontSize, multiline, xAlignment, yAlignment, textColor, font,
/// textInputType, returnKeyType, manualFocusRelease, textWrapped, …), as
/// mocktail's reader names them.
#[derive(Debug, Clone, Copy, Default)]
pub struct BoxInfo {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub font_size: f32,
    pub x_alignment: i32,
    pub y_alignment: i32,
    /// 0xAARRGGBB or 0xRRGGBB, as Android colors.
    pub text_color: u32,
    pub input_type: i32,
}

static INFO: Mutex<Option<BoxInfo>> = Mutex::new(None);

/// The latest NativeTextBoxInfo (the constructor's arguments).
pub fn set_info(args: &[crate::jni::Arg]) {
    use crate::jni::Arg;
    let float = |i: usize| match args.get(i) {
        Some(Arg::Float(v)) => *v as f32,
        _ => 0.0,
    };
    let int = |i: usize| match args.get(i) {
        Some(Arg::Int(v)) => *v as i32,
        _ => 0,
    };
    let info = BoxInfo {
        x: float(0),
        y: float(1),
        width: float(2),
        height: float(3),
        font_size: float(4),
        x_alignment: int(6),
        y_alignment: int(7),
        text_color: int(8) as u32,
        input_type: int(10),
    };
    eprintln!("feline-runtime: text box info {info:?} (all: {args:?})");
    *INFO.lock().unwrap_or_else(|p| p.into_inner()) = Some(info);
}

/// What a key did to the focused text box.
pub enum Edit {
    /// The text changed (or the key belongs to the box but changed nothing).
    Changed {
        text_box: i64,
        text: String,
        cursor: i32,
    },
    Unchanged,
    /// Enter: the text is finished.
    Enter {
        text_box: i64,
        text: String,
        cursor: i32,
    },
    /// Escape: leave the box.
    Release {
        text_box: i64,
    },
}

fn cursor(text: &str) -> i32 {
    i32::try_from(text.encode_utf16().count()).unwrap_or(i32::MAX)
}

/// The engine focused `text_box`, whose text is `initial` (showKeyboard;
/// `native` is its `showNativeInput` argument).
///
/// Roblox reports one focus twice: the box with its position, then a
/// "refresh" with another handle and no size, which is the handle that
/// takes the text (mocktail's text bridge, Apache-2.0, describes the same:
/// "a non-native refresh changes the engine handle but retains the shared
/// keyboard view"). A size-less show while a box is being edited switches
/// to the new handle and keeps the text and the position. Treating it as a
/// new invisible box made chat lose its text or its focus: `/` needed two
/// presses.
pub fn focus(text_box: i64, initial: String, native: bool) {
    let latest = (*INFO.lock().unwrap_or_else(|p| p.into_inner())).unwrap_or_default();
    let usable = latest.width > 0.0 && latest.height > 0.0;
    let mut focused = FOCUSED.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(session) = focused.as_mut() {
        if session.text_box == text_box {
            // The second report of the same focus (two Java classes).
            if usable {
                session.info = latest;
            }
            return;
        }
        if !usable {
            // A refresh of the box being edited.
            eprintln!(
                "feline-runtime: text box {:#x} continues as {text_box:#x} (native {native})",
                session.text_box
            );
            session.text_box = text_box;
            return;
        }
    } else if !usable {
        // A size-less show with no box being edited: Roblox's leftover
        // after a message is sent. It can't be seen; keys stay with the
        // game (taking them blocked movement and the next `/`).
        eprintln!("feline-runtime: text box {text_box:#x} has no size; keys stay with the game");
        return;
    } else {
        OPENED_BY.store(
            LAST_GAME_KEY.load(std::sync::atomic::Ordering::Relaxed),
            std::sync::atomic::Ordering::Relaxed,
        );
    }
    eprintln!("feline-runtime: text box {text_box:#x} focused");
    *focused = Some(Session {
        text_box,
        text: initial.clone(),
        all_selected: false,
        info: latest,
        initial,
    });
}

/// Roblox said the focused box's properties changed: its info is asked for
/// again from the main loop (see `init::refresh_text_box_info`).
static INFO_REFRESH: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn request_info_refresh() {
    if FOCUSED.lock().unwrap_or_else(|p| p.into_inner()).is_some() {
        INFO_REFRESH.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

pub fn take_info_refresh() -> bool {
    INFO_REFRESH.swap(false, std::sync::atomic::Ordering::Relaxed)
}

/// The focused box takes the latest info's position, if it has a size.
pub fn update_position() {
    let latest = (*INFO.lock().unwrap_or_else(|p| p.into_inner())).unwrap_or_default();
    if latest.width > 0.0
        && latest.height > 0.0
        && let Some(session) = FOCUSED.lock().unwrap_or_else(|p| p.into_inner()).as_mut()
    {
        session.info = latest;
    }
}

/// The engine changed the focused box's text itself
/// (`onLuaTextBoxChangedCallback`, e.g. chat clearing after a message):
/// Feline's copy follows it.
pub fn replace_text(text: String) {
    if let Some(session) = FOCUSED.lock().unwrap_or_else(|p| p.into_inner()).as_mut()
        && session.text != text
    {
        session.text = text.clone();
        session.initial = text;
        session.all_selected = false;
    }
}

/// The focused box's text and where to show it, for the overlay (the
/// engine doesn't draw a box's text while the keyboard edits it).
/// The bool: the whole text is selected.
pub fn shown() -> Option<(BoxInfo, String, bool)> {
    let focused = FOCUSED.lock().unwrap_or_else(|p| p.into_inner());
    let session = focused.as_ref()?;
    Some((session.info, session.text.clone(), session.all_selected))
}

/// Whether the focused box is a password box (Roblox's
/// Enum.TextInputType.Password, 5): no copying out of it, bullets shown.
pub fn is_password(input_type: i32) -> bool {
    input_type == 5
}

fn focused_is_password() -> bool {
    FOCUSED
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .is_some_and(|s| is_password(s.info.input_type))
}

/// The desktop clipboard (kept open: on Linux, copied text is served by the
/// process that copied it).
fn clipboard<T>(use_it: impl FnOnce(&mut arboard::Clipboard) -> Option<T>) -> Option<T> {
    static CLIPBOARD: Mutex<Option<arboard::Clipboard>> = Mutex::new(None);
    let mut guard = CLIPBOARD.lock().unwrap_or_else(|p| p.into_inner());
    if guard.is_none() {
        match arboard::Clipboard::new() {
            Ok(c) => *guard = Some(c),
            Err(e) => {
                eprintln!("feline-runtime: no clipboard: {e}");
                return None;
            }
        }
    }
    use_it(guard.as_mut()?)
}

/// The last key pressed down in the game (Android key code; 0 = none since
/// it was released), and the one whose press opened the focused box.
static LAST_GAME_KEY: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
static OPENED_BY: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

/// Whether a key event for the game should really go to it. A key whose
/// press opened a text box (`/` for chat) has its release kept from the
/// game: on Android the on-screen keyboard owns keys once a box has focus.
/// Sending that release made Roblox move the focus away from chat at once,
/// so `/` needed a second press.
pub fn game_key(key: crate::window::Key) -> bool {
    use std::sync::atomic::Ordering::Relaxed;
    if key.down {
        if !key.repeat {
            LAST_GAME_KEY.store(key.code, Relaxed);
        }
        return true;
    }
    if LAST_GAME_KEY.load(Relaxed) == key.code {
        LAST_GAME_KEY.store(0, Relaxed);
    }
    let focused = FOCUSED.lock().unwrap_or_else(|p| p.into_inner()).is_some();
    if focused && OPENED_BY.load(Relaxed) == key.code {
        OPENED_BY.store(0, Relaxed);
        eprintln!("feline-runtime: key {} opened the text box; its release stays with it", key.code);
        return false;
    }
    true
}

/// The engine hid the keyboard (e.g. another box or button was clicked):
/// the box's text still has to be passed as finished, as Android's keyboard
/// does, or the engine drops it. That happens from the main loop
/// ([`take_left`]), not inside the engine's own callback.
pub fn blur() {
    let mut focused = FOCUSED.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(session) = focused.take() {
        eprintln!("feline-runtime: text box left");
        // Only text the user typed is passed on as finished: finishing an
        // untouched box (e.g. the leftover after a chat message) acted on
        // whatever box had focus by then, and closed chat.
        if session.text != session.initial {
            *LEFT.lock().unwrap_or_else(|p| p.into_inner()) = Some((session.text_box, session.text));
        }
    }
}

static LEFT: Mutex<Option<(i64, String)>> = Mutex::new(None);

/// A box that was left with text to finish: (box, text, UTF-16 cursor).
pub fn take_left() -> Option<(i64, String, i32)> {
    let (text_box, text) = LEFT.lock().unwrap_or_else(|p| p.into_inner()).take()?;
    let at = cursor(&text);
    Some((text_box, text, at))
}

/// Android KEYCODE_DEL (Backspace), KEYCODE_ENTER, KEYCODE_NUMPAD_ENTER,
/// KEYCODE_ESCAPE.
const DEL: i32 = 67;
const ENTER: i32 = 66;
const NUMPAD_ENTER: i32 = 160;
const ESCAPE: i32 = 111;
/// KEYCODE_A, C, V, X (with Ctrl: select all, copy, paste, cut) and
/// KeyEvent.META_CTRL_ON.
const KEY_A: i32 = 29;
const KEY_C: i32 = 31;
const KEY_V: i32 = 50;
const KEY_X: i32 = 52;
const META_CTRL_ON: i32 = 0x1000;

/// Applies a key to the focused box. None if no box is focused or the key
/// isn't for it (then it goes to the engine as a key press).
pub fn key(key: crate::window::Key) -> Option<Edit> {
    let password = focused_is_password();
    let mut guard = FOCUSED.lock().unwrap_or_else(|p| p.into_inner());
    let session = guard.as_mut()?;
    let ctrl = key.meta & META_CTRL_ON != 0;
    let shortcut = ctrl && matches!(key.code, KEY_A | KEY_C | KEY_V | KEY_X);
    let editing =
        shortcut || matches!(key.code, DEL | ENTER | NUMPAD_ENTER | ESCAPE) || key.unicode > 0;
    if !editing {
        return None;
    }
    // The box owns both halves of its keys.
    if !key.down {
        return Some(Edit::Unchanged);
    }
    let text_box = session.text_box;
    let changed = |session: &Session| Edit::Changed {
        text_box,
        text: session.text.clone(),
        cursor: cursor(&session.text),
    };
    if shortcut {
        return Some(match key.code {
            KEY_A => {
                session.all_selected = !session.text.is_empty();
                Edit::Unchanged
            }
            KEY_C | KEY_X if password => Edit::Unchanged,
            KEY_C => {
                let text = session.text.clone();
                clipboard(|c| c.set_text(text).ok());
                Edit::Unchanged
            }
            KEY_X => {
                let text = std::mem::take(&mut session.text);
                clipboard(|c| c.set_text(text).ok());
                session.all_selected = false;
                changed(session)
            }
            _ => {
                // Paste: one line (these boxes are single-line).
                let Some(pasted) = clipboard(|c| c.get_text().ok()) else {
                    return Some(Edit::Unchanged);
                };
                let line: String = pasted
                    .lines()
                    .next()
                    .unwrap_or("")
                    .chars()
                    .filter(|c| !c.is_control())
                    .collect();
                if std::mem::take(&mut session.all_selected) {
                    session.text.clear();
                }
                session.text.push_str(&line);
                changed(session)
            }
        });
    }
    // Typing or Backspace over a selection replaces it.
    if std::mem::take(&mut session.all_selected) && (key.code == DEL || key.unicode > 0) {
        session.text.clear();
        if key.code == DEL {
            return Some(changed(session));
        }
    }
    Some(match key.code {
        ENTER | NUMPAD_ENTER => {
            let text = session.text.clone();
            *guard = None;
            Edit::Enter {
                text_box,
                cursor: cursor(&text),
                text,
            }
        }
        ESCAPE => {
            *guard = None;
            Edit::Release { text_box }
        }
        DEL => {
            session.text.pop();
            Edit::Changed {
                text_box,
                text: session.text.clone(),
                cursor: cursor(&session.text),
            }
        }
        _ => {
            match char::from_u32(key.unicode as u32) {
                Some(c) => session.text.push(c),
                None => return Some(Edit::Unchanged),
            }
            Edit::Changed {
                text_box,
                text: session.text.clone(),
                cursor: cursor(&session.text),
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tests share the one focused box, so they take turns.
    static TURN: Mutex<()> = Mutex::new(());

    fn press(code: i32, unicode: i32) -> crate::window::Key {
        crate::window::Key {
            down: true,
            code,
            scan_code: 0,
            meta: 0,
            unicode,
            repeat: false,
        }
    }

    #[test]
    fn select_all_then_type_or_delete() {
        let _turn = TURN.lock().unwrap_or_else(|p| p.into_inner());
        let ctrl_a = crate::window::Key {
            meta: META_CTRL_ON,
            ..press(KEY_A, 0)
        };
        focus(9, "old".into(), true);
        assert!(matches!(key(ctrl_a), Some(Edit::Unchanged)));
        assert!(
            matches!(key(press(0, 'n' as i32)), Some(Edit::Changed { ref text, .. }) if text == "n")
        );
        assert!(matches!(key(ctrl_a), Some(Edit::Unchanged)));
        assert!(
            matches!(key(press(DEL, 0)), Some(Edit::Changed { ref text, .. }) if text.is_empty())
        );
        blur();
    }

    #[test]
    fn a_sizeless_refresh_keeps_the_text_and_takes_the_new_handle() {
        let _turn = TURN.lock().unwrap_or_else(|p| p.into_inner());
        blur();
        let _ = take_left();
        let sized = BoxInfo {
            x: 38.0,
            y: 293.0,
            width: 387.0,
            height: 18.0,
            ..BoxInfo::default()
        };
        *INFO.lock().unwrap() = Some(sized);
        focus(1, String::new(), true);
        assert!(matches!(key(press(29, 'h' as i32)), Some(Edit::Changed { .. })));
        // Roblox's second report: another handle, no size.
        *INFO.lock().unwrap() = Some(BoxInfo::default());
        focus(2, String::new(), false);
        assert!(take_left().is_none(), "the first box isn't finished");
        assert!(
            matches!(key(press(30, 'i' as i32)), Some(Edit::Changed { text_box: 2, ref text, .. }) if text == "hi")
        );
        let (info, text, _) = shown().expect("still shown");
        assert_eq!((info.x, info.width, text.as_str()), (38.0, 387.0, "hi"));
        // The engine clears the box itself.
        replace_text(String::new());
        assert!(matches!(key(press(31, 'x' as i32)), Some(Edit::Changed { ref text, .. }) if text == "x"));
        blur();
        let _ = take_left();
    }

    #[test]
    fn leftovers_after_sending_take_no_keys_and_untouched_boxes_finish_nothing() {
        let _turn = TURN.lock().unwrap_or_else(|p| p.into_inner());
        blur();
        let _ = take_left();
        // No box being edited, a size-less show: not a text box for keys.
        *INFO.lock().unwrap() = Some(BoxInfo::default());
        focus(5, String::new(), true);
        assert!(key(press(51, 'w' as i32)).is_none(), "W goes to the game");
        // A real box, focused and left without typing: nothing to finish.
        *INFO.lock().unwrap() = Some(BoxInfo {
            width: 100.0,
            height: 20.0,
            ..BoxInfo::default()
        });
        focus(6, "keep".into(), true);
        blur();
        assert!(take_left().is_none());
        // Typed into, then left: finished.
        focus(6, String::new(), true);
        let _ = key(press(29, 'a' as i32));
        blur();
        assert!(matches!(take_left(), Some((6, ref text, _)) if text == "a"));
    }

    #[test]
    fn typing_backspace_and_enter() {
        let _turn = TURN.lock().unwrap_or_else(|p| p.into_inner());
        blur();
        focus(7, "ab".into(), true);
        assert!(
            matches!(key(press(29, 'c' as i32)), Some(Edit::Changed { ref text, cursor: 3, .. }) if text == "abc")
        );
        assert!(matches!(key(press(DEL, 0)), Some(Edit::Changed { ref text, .. }) if text == "ab"));
        // Emoji count as two UTF-16 units.
        assert!(matches!(
            key(press(0, '😺' as i32)),
            Some(Edit::Changed { cursor: 4, .. })
        ));
        assert!(matches!(
            key(press(ENTER, 0)),
            Some(Edit::Enter { text_box: 7, .. })
        ));
        // Nothing focused any more: keys go to the engine.
        assert!(key(press(29, 'a' as i32)).is_none());
    }
}

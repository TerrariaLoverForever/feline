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

/// The engine focused `text_box`, whose text is `initial`.
pub fn focus(text_box: i64, initial: String) {
    let mut focused = FOCUSED.lock().unwrap_or_else(|p| p.into_inner());
    // showKeyboard comes twice per focus (two Java classes); keep the text
    // already typed if it's the same box.
    if focused.as_ref().is_some_and(|s| s.text_box == text_box) {
        return;
    }
    eprintln!("feline-runtime: text box {text_box:#x} focused");
    *focused = Some(Session {
        text_box,
        text: initial,
    });
}

/// The focused box's text and where to show it, for the overlay (the
/// engine doesn't draw a box's text while the keyboard edits it).
pub fn shown() -> Option<(BoxInfo, String)> {
    let focused = FOCUSED.lock().unwrap_or_else(|p| p.into_inner());
    let text = focused.as_ref()?.text.clone();
    let info = (*INFO.lock().unwrap_or_else(|p| p.into_inner()))?;
    Some((info, text))
}

/// The engine hid the keyboard: no box is focused.
/// The engine hid the keyboard (e.g. another box or button was clicked):
/// the box's text still has to be passed as finished, as Android's keyboard
/// does, or the engine drops it. That happens from the main loop
/// ([`take_left`]), not inside the engine's own callback.
pub fn blur() {
    let mut focused = FOCUSED.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(session) = focused.take() {
        eprintln!("feline-runtime: text box left");
        *LEFT.lock().unwrap_or_else(|p| p.into_inner()) = Some((session.text_box, session.text));
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

/// Applies a key to the focused box. None if no box is focused or the key
/// isn't for it (then it goes to the engine as a key press).
pub fn key(key: crate::window::Key) -> Option<Edit> {
    let mut guard = FOCUSED.lock().unwrap_or_else(|p| p.into_inner());
    let session = guard.as_mut()?;
    let editing = matches!(key.code, DEL | ENTER | NUMPAD_ENTER | ESCAPE) || key.unicode > 0;
    if !editing {
        return None;
    }
    // The box owns both halves of its keys.
    if !key.down {
        return Some(Edit::Unchanged);
    }
    let text_box = session.text_box;
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
    fn typing_backspace_and_enter() {
        focus(7, "ab".into());
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

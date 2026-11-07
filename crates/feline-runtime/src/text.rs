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

/// The engine hid the keyboard: no box is focused.
pub fn blur() {
    let mut focused = FOCUSED.lock().unwrap_or_else(|p| p.into_inner());
    if focused.take().is_some() {
        eprintln!("feline-runtime: text box left");
    }
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

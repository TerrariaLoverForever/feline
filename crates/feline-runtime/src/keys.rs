// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! Desktop keys (winit) to Android key events.
//!
//! The `KEYCODE_*` and `META_*` numbers are the constants of Android's public
//! `android.view.KeyEvent` API. Keys are matched by their *physical*
//! position, as Roblox's controls (WASD, …) expect; the typed character comes
//! from the keyboard layout.

use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::platform::scancode::PhysicalKeyExtScancode;

use crate::window::Key;

const META_SHIFT_ON: i32 = 0x1;
const META_ALT_ON: i32 = 0x2;
const META_CTRL_ON: i32 = 0x1000;

/// Android's `KEYCODE_*` for a physical key, if it has one.
pub fn android_code(key: KeyCode) -> Option<i32> {
    use KeyCode as K;
    Some(match key {
        K::Digit0 => 7,
        K::Digit1 => 8,
        K::Digit2 => 9,
        K::Digit3 => 10,
        K::Digit4 => 11,
        K::Digit5 => 12,
        K::Digit6 => 13,
        K::Digit7 => 14,
        K::Digit8 => 15,
        K::Digit9 => 16,
        K::ArrowUp => 19,
        K::ArrowDown => 20,
        K::ArrowLeft => 21,
        K::ArrowRight => 22,
        K::KeyA => 29,
        K::KeyB => 30,
        K::KeyC => 31,
        K::KeyD => 32,
        K::KeyE => 33,
        K::KeyF => 34,
        K::KeyG => 35,
        K::KeyH => 36,
        K::KeyI => 37,
        K::KeyJ => 38,
        K::KeyK => 39,
        K::KeyL => 40,
        K::KeyM => 41,
        K::KeyN => 42,
        K::KeyO => 43,
        K::KeyP => 44,
        K::KeyQ => 45,
        K::KeyR => 46,
        K::KeyS => 47,
        K::KeyT => 48,
        K::KeyU => 49,
        K::KeyV => 50,
        K::KeyW => 51,
        K::KeyX => 52,
        K::KeyY => 53,
        K::KeyZ => 54,
        K::Comma => 55,
        K::Period => 56,
        K::AltLeft => 57,
        K::AltRight => 58,
        K::ShiftLeft => 59,
        K::ShiftRight => 60,
        K::Tab => 61,
        K::Space => 62,
        K::Enter => 66,
        K::Backspace => 67, // KEYCODE_DEL
        K::Backquote => 68,
        K::Minus => 69,
        K::Equal => 70,
        K::BracketLeft => 71,
        K::BracketRight => 72,
        K::Backslash => 73,
        K::Semicolon => 74,
        K::Quote => 75,
        K::Slash => 76,
        K::PageUp => 92,
        K::PageDown => 93,
        K::Escape => 111,
        K::Delete => 112, // KEYCODE_FORWARD_DEL
        K::ControlLeft => 113,
        K::ControlRight => 114,
        K::CapsLock => 115,
        K::SuperLeft => 117,
        K::SuperRight => 118,
        K::Home => 122,
        K::End => 123,
        K::Insert => 124,
        K::F1 => 131,
        K::F2 => 132,
        K::F3 => 133,
        K::F4 => 134,
        K::F5 => 135,
        K::F6 => 136,
        K::F7 => 137,
        K::F8 => 138,
        K::F9 => 139,
        K::F10 => 140,
        K::F11 => 141,
        K::F12 => 142,
        K::Numpad0 => 144,
        K::Numpad1 => 145,
        K::Numpad2 => 146,
        K::Numpad3 => 147,
        K::Numpad4 => 148,
        K::Numpad5 => 149,
        K::Numpad6 => 150,
        K::Numpad7 => 151,
        K::Numpad8 => 152,
        K::Numpad9 => 153,
        K::NumpadDivide => 154,
        K::NumpadMultiply => 155,
        K::NumpadSubtract => 156,
        K::NumpadAdd => 157,
        K::NumpadDecimal => 158,
        K::NumpadEnter => 160,
        _ => return None,
    })
}

/// Android's `META_*` bits for the held modifiers.
pub fn meta_state(modifiers: ModifiersState) -> i32 {
    let mut meta = 0;
    if modifiers.shift_key() {
        meta |= META_SHIFT_ON;
    }
    if modifiers.alt_key() {
        meta |= META_ALT_ON;
    }
    if modifiers.control_key() {
        meta |= META_CTRL_ON;
    }
    meta
}

/// The Android key event for a desktop key event (None for keys Android
/// has no code for).
pub fn android_key(event: &KeyEvent, modifiers: ModifiersState) -> Option<Key> {
    let PhysicalKey::Code(physical) = event.physical_key else {
        return None;
    };
    let code = android_code(physical)?;
    let unicode = event
        .text
        .as_ref()
        .and_then(|t| t.chars().next())
        .filter(|c| !c.is_control())
        .map_or(0, |c| c as i32);
    Some(Key {
        down: event.state == ElementState::Pressed,
        code,
        scan_code: event.physical_key.to_scancode().map_or(0, |s| s as i32),
        meta: meta_state(modifiers),
        unicode,
        repeat: event.repeat,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_match_android() {
        assert_eq!(android_code(KeyCode::KeyA), Some(29));
        assert_eq!(android_code(KeyCode::KeyW), Some(51));
        assert_eq!(android_code(KeyCode::Space), Some(62));
        assert_eq!(android_code(KeyCode::Escape), Some(111));
        assert_eq!(android_code(KeyCode::F12), Some(142));
        assert_eq!(
            meta_state(ModifiersState::SHIFT | ModifiersState::CONTROL),
            0x1001
        );
    }
}

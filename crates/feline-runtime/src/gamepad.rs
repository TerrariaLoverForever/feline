// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! Controllers (gamepads): read with `gilrs` (the desktop's input devices,
//! with SDL's controller database for button names) and handed to the
//! engine through `NativeInputInterface`'s declared gamepad natives, as
//! Android's Java side does with its InputDevice events.
//!
//! Like Android, a controller is announced to Roblox when it's first used:
//! its supported keys and axes (`nativeSetGamepadSupported…WithGamepadType`),
//! then `nativeGamepadConnectEventWithGamepadType`, then the input. Buttons
//! use Android key codes (KEYCODE_BUTTON_A = 96, …), sticks and triggers
//! Android axis numbers (AXIS_X = 0, AXIS_Y = 1, AXIS_Z = 11, AXIS_RZ = 14,
//! AXIS_LTRIGGER = 17, AXIS_RTRIGGER = 18), with up as positive on the
//! sticks. The mapping, the 0.2 stick deadzone and Roblox's controller
//! types (PlayStation 4 = 1, PlayStation 5 = 2, Xbox = 3, other = 0) follow
//! Mocktail (<https://github.com/komaruworld/mocktail>, Apache-2.0,
//! src/runtime/roblox_gamepad_input.cc); the code is Feline's own.
//!
//! Unsafe: calling the engine's natives through their addresses.
#![allow(unsafe_code)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;

use gilrs::{Axis, Button, EventType, GamepadId, Gilrs};

use crate::elf::Elf;
use crate::jni::Handle;
use crate::load::Image;

const INPUT: &str = "com/roblox/engine/jni/NativeInputInterface";

/// The engine's gamepad natives (static methods of NativeInputInterface).
struct Natives {
    connect: usize,
    disconnect: usize,
    button: usize,
    axis: usize,
    supported_key: usize,
    supported_motion: usize,
}

static NATIVES: OnceLock<Natives> = OnceLock::new();

/// Finds the gamepad natives (once the engine is loaded).
pub fn remember_natives(elf: &Elf, image: &Image) -> Result<(), String> {
    let symbols = elf.symbols().map_err(|e| e.to_string())?;
    let find = |method: &str| {
        let name = crate::init::jni_symbol(INPUT, method);
        symbols
            .iter()
            .find(|s| s.defined && s.name == name)
            .map(|s| (image.bias() + s.value) as usize)
            .ok_or_else(|| format!("no {name}"))
    };
    let _ = NATIVES.set(Natives {
        connect: find("nativeGamepadConnectEventWithGamepadType")?,
        disconnect: find("nativeGamepadDisconnectEvent")?,
        button: find("nativeGamepadButtonEvent")?,
        axis: find("nativeGamepadAxisEvent")?,
        supported_key: find("nativeSetGamepadSupportedKeyWithGamepadType")?,
        supported_motion: find("nativeSetGamepadSupportedMotionWithGamepadType")?,
    });
    Ok(())
}

/// Android key codes of the buttons Roblox knows, in the order announced.
const KEYS: [i32; 14] = [96, 97, 99, 100, 19, 20, 21, 22, 103, 102, 106, 107, 109, 108];
/// Android axis numbers: left stick X/Y, right stick X/Y, triggers.
const AXES: [i32; 6] = [0, 1, 11, 14, 17, 18];

fn key_code(button: Button) -> Option<i32> {
    Some(match button {
        Button::South => 96,
        Button::East => 97,
        Button::West => 99,
        Button::North => 100,
        Button::DPadUp => 19,
        Button::DPadDown => 20,
        Button::DPadLeft => 21,
        Button::DPadRight => 22,
        Button::LeftTrigger => 102,
        Button::RightTrigger => 103,
        Button::LeftThumb => 106,
        Button::RightThumb => 107,
        Button::Start => 108,
        Button::Select => 109,
        _ => return None,
    })
}

/// Roblox's controller type from the USB vendor and product.
fn controller_type(vendor: Option<u16>, product: Option<u16>) -> i32 {
    match (vendor, product) {
        // Sony: DualSense and DualSense Edge are PlayStation 5.
        (Some(0x054c), Some(0x0ce6 | 0x0df2)) => 2,
        (Some(0x054c), _) => 1,
        (Some(0x045e), _) => 3,
        _ => 0,
    }
}

/// A stick position with a round 0.2 deadzone, rescaled so it still
/// reaches 1 at the edge.
fn stick(x: f32, y: f32) -> (f32, f32) {
    const DEADZONE: f32 = 0.2;
    let length = (x * x + y * y).sqrt();
    if length <= DEADZONE {
        return (0.0, 0.0);
    }
    let scale = (length.min(1.0) - DEADZONE) / ((1.0 - DEADZONE) * length);
    (x * scale, y * scale)
}

fn trigger(value: f32) -> f32 {
    const DEADZONE: f32 = 0.02;
    let value = value.clamp(0.0, 1.0);
    if value <= DEADZONE { 0.0 } else { (value - DEADZONE) / (1.0 - DEADZONE) }
}

/// One controller, as Roblox knows it.
#[derive(Default)]
struct Pad {
    device: i32,
    kind: i32,
    announced: bool,
    left: (f32, f32),
    right: (f32, f32),
    /// Last values sent, per Android axis, to skip repeats.
    sent: HashMap<i32, (f32, f32, f32)>,
    /// D-pad held through hat axes (some controllers report it so).
    dpad: HashMap<i32, bool>,
}

struct State {
    gilrs: Option<Gilrs>,
    pads: HashMap<GamepadId, Pad>,
    next_device: i32,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// Reads the controllers' events and passes them on. From the window loop.
pub fn poll(env: Handle) {
    let Some(natives) = NATIVES.get() else {
        return;
    };
    STATE.with(|cell| {
        let mut cell = cell.borrow_mut();
        let state = cell.get_or_insert_with(|| State {
            gilrs: match Gilrs::new() {
                Ok(g) => Some(g),
                Err(e) => {
                    eprintln!("feline-runtime: controllers unavailable: {e}");
                    None
                }
            },
            pads: HashMap::new(),
            next_device: 1,
        });
        let Some(gilrs) = state.gilrs.as_mut() else {
            return;
        };
        // Made once: a class object per frame would pile up.
        static CLASS: OnceLock<usize> = OnceLock::new();
        let class = *CLASS.get_or_init(|| crate::jni::new_class(INPUT) as usize) as Handle;
        let call = Calls { env, class, natives };
        while let Some(event) = gilrs.next_event() {
            let id = event.id;
            if !state.pads.contains_key(&id) {
                let gamepad = gilrs.gamepad(id);
                let pad = Pad {
                    device: state.next_device,
                    kind: controller_type(gamepad.vendor_id(), gamepad.product_id()),
                    ..Pad::default()
                };
                eprintln!(
                    "feline-runtime: controller {:?} found (device {}, type {})",
                    gamepad.name(),
                    pad.device,
                    pad.kind
                );
                state.next_device += 1;
                state.pads.insert(id, pad);
            }
            let Some(pad) = state.pads.get_mut(&id) else {
                continue;
            };
            match event.event {
                EventType::Disconnected => {
                    if pad.announced {
                        call.disconnect(pad.device);
                    }
                    eprintln!("feline-runtime: controller (device {}) disconnected", pad.device);
                    state.pads.remove(&id);
                }
                EventType::ButtonPressed(button, _) | EventType::ButtonReleased(button, _) => {
                    let pressed = matches!(event.event, EventType::ButtonPressed(..));
                    if let Some(key) = key_code(button) {
                        call.announce(pad);
                        call.button(pad.device, key, pressed);
                    }
                }
                EventType::ButtonChanged(button @ (Button::LeftTrigger2 | Button::RightTrigger2), value, _) => {
                    let axis = if button == Button::LeftTrigger2 { 17 } else { 18 };
                    call.announce(pad);
                    call.axis(pad, axis, 0.0, 0.0, trigger(value));
                }
                EventType::AxisChanged(axis, value, _) => {
                    call.announce(pad);
                    match axis {
                        Axis::LeftStickX => pad.left.0 = value,
                        Axis::LeftStickY => pad.left.1 = value,
                        Axis::RightStickX => pad.right.0 = value,
                        Axis::RightStickY => pad.right.1 = value,
                        Axis::LeftZ => call.axis(pad, 17, 0.0, 0.0, trigger(value)),
                        Axis::RightZ => call.axis(pad, 18, 0.0, 0.0, trigger(value)),
                        Axis::DPadX => {
                            call.hat(pad, 21, value < -0.5);
                            call.hat(pad, 22, value > 0.5);
                        }
                        Axis::DPadY => {
                            call.hat(pad, 19, value > 0.5);
                            call.hat(pad, 20, value < -0.5);
                        }
                        _ => {}
                    }
                    if matches!(axis, Axis::LeftStickX | Axis::LeftStickY) {
                        let (x, y) = stick(pad.left.0, pad.left.1);
                        call.axis(pad, 0, x, y, 0.0);
                        call.axis(pad, 1, x, y, 0.0);
                    }
                    if matches!(axis, Axis::RightStickX | Axis::RightStickY) {
                        let (x, y) = stick(pad.right.0, pad.right.1);
                        call.axis(pad, 11, x, y, 0.0);
                        call.axis(pad, 14, x, y, 0.0);
                    }
                }
                _ => {}
            }
        }
    });
}

/// The natives, bound to this frame's env and class.
struct Calls<'a> {
    env: Handle,
    class: Handle,
    natives: &'a Natives,
}

// SAFETY (all calls below): the DEX signatures of NativeInputInterface's
// static natives, called with the env and class Java would pass.
impl Calls<'_> {
    /// Tells Roblox about the controller, once, before its first input.
    fn announce(&self, pad: &mut Pad) {
        if pad.announced {
            return;
        }
        pad.announced = true;
        type Key = extern "C" fn(Handle, Handle, i32, i32, u8, i32);
        type Motion = extern "C" fn(Handle, Handle, i32, i32, i32, u8, i32);
        type Connect = extern "C" fn(Handle, Handle, i32, i32);
        let key: Key = unsafe { std::mem::transmute(self.natives.supported_key) };
        let motion: Motion = unsafe { std::mem::transmute(self.natives.supported_motion) };
        let connect: Connect = unsafe { std::mem::transmute(self.natives.connect) };
        for code in KEYS {
            key(self.env, self.class, pad.device, code, 1, pad.kind);
        }
        for axis in AXES {
            motion(self.env, self.class, pad.device, axis, -1, 1, pad.kind);
        }
        // The D-pad comes as keys; no hat or brake/gas axes.
        for axis in [22, 23, 15, 16] {
            motion(self.env, self.class, pad.device, axis, -1, 0, pad.kind);
        }
        connect(self.env, self.class, pad.device, pad.kind);
        eprintln!("feline-runtime: controller (device {}) connected to Roblox", pad.device);
    }

    fn disconnect(&self, device: i32) {
        type Disconnect = extern "C" fn(Handle, Handle, i32);
        let disconnect: Disconnect = unsafe { std::mem::transmute(self.natives.disconnect) };
        disconnect(self.env, self.class, device);
    }

    fn button(&self, device: i32, key: i32, pressed: bool) {
        type ButtonEvent = extern "C" fn(Handle, Handle, i32, i32, i32);
        let button: ButtonEvent = unsafe { std::mem::transmute(self.natives.button) };
        button(self.env, self.class, device, key, i32::from(pressed));
    }

    fn axis(&self, pad: &mut Pad, axis: i32, x: f32, y: f32, z: f32) {
        if pad.sent.get(&axis) == Some(&(x, y, z)) {
            return;
        }
        pad.sent.insert(axis, (x, y, z));
        type AxisEvent = extern "C" fn(Handle, Handle, i32, i32, f32, f32, f32);
        let send: AxisEvent = unsafe { std::mem::transmute(self.natives.axis) };
        send(self.env, self.class, pad.device, axis, x, y, z);
    }

    /// A D-pad direction from a hat axis, as a key press or release.
    fn hat(&self, pad: &mut Pad, key: i32, held: bool) {
        if pad.dpad.get(&key).copied().unwrap_or(false) != held {
            pad.dpad.insert(key, held);
            self.button(pad.device, key, held);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapping_and_deadzones() {
        assert_eq!(key_code(Button::South), Some(96));
        assert_eq!(key_code(Button::Select), Some(109));
        assert_eq!(key_code(Button::Mode), None);
        assert_eq!(controller_type(Some(0x054c), Some(0x0ce6)), 2);
        assert_eq!(controller_type(Some(0x054c), Some(0x09cc)), 1);
        assert_eq!(controller_type(Some(0x045e), Some(0x0b12)), 3);
        assert_eq!(controller_type(None, None), 0);
        assert_eq!(stick(0.1, 0.1), (0.0, 0.0));
        let (x, y) = stick(1.0, 0.0);
        assert!((x - 1.0).abs() < 1e-6 && y == 0.0);
        assert_eq!(trigger(0.01), 0.0);
        assert!((trigger(1.0) - 1.0).abs() < 1e-6);
    }
}

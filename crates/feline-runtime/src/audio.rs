// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! Sound: FMOD's Java audio output (`org.fmod.AudioDevice`), played on the
//! desktop's sound server.
//!
//! Roblox's sound engine (FMOD) asks Java whether AAudio works (Feline says
//! no), then plays through `AudioDevice`, its small wrapper around Android's
//! `AudioTrack`, as seen in Feline's JNI log:
//!
//! - `init(int channels, int sampleRate, int sampleSize, int frames)`: open
//!   an output; `false` makes FMOD give up ("error initializing output
//!   device", FMOD_RESULT 51, and no sound at all);
//! - `write(byte[] data, int length)`: play PCM bytes. Like
//!   `AudioTrack.write`, it blocks while the output is full, which is what
//!   paces FMOD's mixer;
//! - `close()`.
//!
//! The output is PulseAudio's simple API (`libpulse-simple.so.0`, opened at
//! run time like the other host libraries), which PipeWire also provides.
//! Its blocking write matches AudioTrack's. Without it, `init` says no and
//! the game is silent, as before.
//!
//! # `unsafe` in this module
//!
//! The PulseAudio functions are C functions found with `dlsym` and called
//! with valid pointers; the stream handle is only used under a lock.

#![allow(unsafe_code)]

use std::ffi::{CString, c_char, c_int, c_void};
use std::sync::{Mutex, OnceLock};

/// `pa_sample_spec`.
#[repr(C)]
struct SampleSpec {
    format: c_int,
    rate: u32,
    channels: u8,
}

/// `pa_buffer_attr` (`u32::MAX` = the server's default).
#[repr(C)]
struct BufferAttr {
    maxlength: u32,
    tlength: u32,
    prebuf: u32,
    minreq: u32,
    fragsize: u32,
}

const PA_STREAM_PLAYBACK: c_int = 1;
const PA_SAMPLE_S16LE: c_int = 3;
const PA_SAMPLE_FLOAT32LE: c_int = 5;

type New = unsafe extern "C" fn(
    *const c_char,
    *const c_char,
    c_int,
    *const c_char,
    *const c_char,
    *const SampleSpec,
    *const c_void,
    *const BufferAttr,
    *mut c_int,
) -> *mut c_void;
type Write = unsafe extern "C" fn(*mut c_void, *const c_void, usize, *mut c_int) -> c_int;
type Free = unsafe extern "C" fn(*mut c_void);

struct Pulse {
    new: New,
    write: Write,
    free: Free,
}

fn pulse() -> Option<&'static Pulse> {
    static PULSE: OnceLock<Option<Pulse>> = OnceLock::new();
    PULSE
        .get_or_init(|| {
            let name = CString::new("libpulse-simple.so.0").ok()?;
            // SAFETY: a valid C string; the library is never closed.
            let handle = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
            if handle.is_null() {
                eprintln!("feline-runtime: no sound: libpulse-simple.so.0 isn't installed");
                return None;
            }
            let find = |symbol: &str| {
                let symbol = CString::new(symbol).ok()?;
                // SAFETY: a live handle and a valid C string.
                let address = unsafe { libc::dlsym(handle, symbol.as_ptr()) };
                (!address.is_null()).then_some(address)
            };
            // SAFETY: PulseAudio's documented signatures (pulse/simple.h).
            unsafe {
                Some(Pulse {
                    new: std::mem::transmute::<*mut c_void, New>(find("pa_simple_new")?),
                    write: std::mem::transmute::<*mut c_void, Write>(find("pa_simple_write")?),
                    free: std::mem::transmute::<*mut c_void, Free>(find("pa_simple_free")?),
                })
            }
        })
        .as_ref()
}

/// The open output (a `pa_simple*`), if any.
static STREAM: Mutex<usize> = Mutex::new(0);

/// The PCM format for AudioDevice's `sampleSize` (bytes, or bits).
fn sample_format(sample_size: i32) -> Option<c_int> {
    let bytes = if sample_size >= 8 { sample_size / 8 } else { sample_size };
    match bytes {
        2 => Some(PA_SAMPLE_S16LE),
        4 => Some(PA_SAMPLE_FLOAT32LE),
        _ => None,
    }
}

/// `AudioDevice.init`: opens the output. False if there's no sound server.
pub fn init(channels: i32, rate: i32, sample_size: i32, frames: i32) -> bool {
    eprintln!(
        "feline-runtime: sound: FMOD asks for {channels} channels, {rate} Hz, sample size {sample_size}, {frames} frames"
    );
    let (Some(pulse), Some(format), Ok(channels), Ok(rate)) = (
        pulse(),
        sample_format(sample_size),
        u8::try_from(channels),
        u32::try_from(rate),
    ) else {
        eprintln!("feline-runtime: sound: can't open that output");
        return false;
    };
    close();
    let frame_bytes = u32::from(channels) * if format == PA_SAMPLE_S16LE { 2 } else { 4 };
    // About two of FMOD's blocks queued: low delay, no gaps.
    let block = frame_bytes * u32::try_from(frames.max(256)).unwrap_or(1024);
    let attr = BufferAttr {
        maxlength: u32::MAX,
        tlength: block * 2,
        prebuf: u32::MAX,
        minreq: u32::MAX,
        fragsize: u32::MAX,
    };
    let spec = SampleSpec {
        format,
        rate,
        channels,
    };
    let mut error = 0;
    // SAFETY: valid C strings, spec and attr; NULL server/device/map mean
    // the defaults.
    let stream = unsafe {
        (pulse.new)(
            std::ptr::null(),
            c"Roblox (Feline)".as_ptr(),
            PA_STREAM_PLAYBACK,
            std::ptr::null(),
            c"Game sound".as_ptr(),
            &spec,
            std::ptr::null(),
            &attr,
            &mut error,
        )
    };
    if stream.is_null() {
        eprintln!("feline-runtime: sound: the sound server refused the output (error {error})");
        return false;
    }
    *STREAM.lock().unwrap_or_else(|p| p.into_inner()) = stream as usize;
    eprintln!("feline-runtime: sound: playing through the desktop's sound server");
    true
}

/// `AudioDevice.write`: plays `data` (blocking while the output is full).
pub fn write(data: &[u8]) {
    let Some(pulse) = pulse() else {
        return;
    };
    let stream = *STREAM.lock().unwrap_or_else(|p| p.into_inner());
    if stream == 0 || data.is_empty() {
        return;
    }
    let mut error = 0;
    // SAFETY: a live stream (only freed by `close`, which FMOD calls after
    // its last write) and `data.len()` readable bytes.
    unsafe { (pulse.write)(stream as *mut c_void, data.as_ptr().cast(), data.len(), &mut error) };
}

/// `AudioDevice.close`.
pub fn close() {
    let stream = std::mem::take(&mut *STREAM.lock().unwrap_or_else(|p| p.into_inner()));
    if stream != 0
        && let Some(pulse) = pulse()
    {
        // SAFETY: a stream from `pa_simple_new`, freed once.
        unsafe { (pulse.free)(stream as *mut c_void) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(sample_format(2), Some(PA_SAMPLE_S16LE));
        assert_eq!(sample_format(16), Some(PA_SAMPLE_S16LE));
        assert_eq!(sample_format(4), Some(PA_SAMPLE_FLOAT32LE));
        assert_eq!(sample_format(3), None);
    }
}

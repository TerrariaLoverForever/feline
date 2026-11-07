// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! Sound: FMOD's Java audio output (`org.fmod.AudioDevice`), played on the
//! desktop's sound server.
//!
//! Roblox's sound engine (FMOD) asks Java whether AAudio works (Feline says
//! no), then plays through `AudioDevice`, its small wrapper around Android's
//! `AudioTrack`, as seen in Feline's JNI log:
//!
//! - `init(int channels, int sampleRate, int blockFrames, int blocks)`:
//!   open an output (Roblox asks for 2, 48000, 512, 4); `false` makes FMOD
//!   give up ("error initializing output device", FMOD_RESULT 51, and no
//!   sound at all);
//! - `write(byte[] data, int length)`: play 16-bit PCM bytes (AudioTrack's
//!   default encoding; 512 stereo frames arrive as 2048 bytes). Like
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
const PA_STREAM_RECORD: c_int = 2;
const PA_SAMPLE_S16LE: c_int = 3;

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
type Read = unsafe extern "C" fn(*mut c_void, *mut c_void, usize, *mut c_int) -> c_int;
type Free = unsafe extern "C" fn(*mut c_void);

struct Pulse {
    new: New,
    write: Write,
    read: Read,
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
                    read: std::mem::transmute::<*mut c_void, Read>(find("pa_simple_read")?),
                    free: std::mem::transmute::<*mut c_void, Free>(find("pa_simple_free")?),
                })
            }
        })
        .as_ref()
}

/// The open output (a `pa_simple*`), if any.
static STREAM: Mutex<usize> = Mutex::new(0);

/// `AudioDevice.init`: opens the output. False if there's no sound server.
pub fn init(channels: i32, rate: i32, block_frames: i32, blocks: i32) -> bool {
    eprintln!(
        "feline-runtime: sound: FMOD asks for {channels} channels, {rate} Hz, {blocks} blocks of {block_frames} frames"
    );
    let (Some(pulse), Ok(channels @ 1..=8), Ok(rate @ 8000..=192_000)) =
        (pulse(), u8::try_from(channels), u32::try_from(rate))
    else {
        eprintln!("feline-runtime: sound: can't open that output");
        return false;
    };
    close();
    let frame_bytes = u32::from(channels) * 2;
    // FMOD's own buffer (blocks × block size) queued: about 43 ms at
    // 4 × 512 frames and 48 kHz. Low delay, no gaps.
    let block = frame_bytes * u32::try_from(block_frames.clamp(64, 8192)).unwrap_or(512);
    let attr = BufferAttr {
        maxlength: u32::MAX,
        tlength: block * u32::try_from(blocks.clamp(2, 16)).unwrap_or(4),
        prebuf: u32::MAX,
        minreq: u32::MAX,
        fragsize: u32::MAX,
    };
    let spec = SampleSpec {
        format: PA_SAMPLE_S16LE,
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

/// A 16-bit stream of its own, for voice chat: recording (the microphone)
/// or playback (other players' voices), separate from the game's sound.
pub struct Stream {
    pulse: &'static Pulse,
    handle: *mut c_void,
}

// SAFETY: a pa_simple stream may be used from another thread as long as
// only one thread uses it at a time; `Stream` is moved, not shared.
unsafe impl Send for Stream {}

impl Stream {
    /// Opens a stream; `fragment_bytes` is how much the server hands over
    /// (recording) or keeps queued (playback) at a time.
    pub fn open(record: bool, rate: u32, channels: u8, fragment_bytes: u32, name: &std::ffi::CStr) -> Option<Self> {
        let pulse = pulse()?;
        let spec = SampleSpec {
            format: PA_SAMPLE_S16LE,
            rate,
            channels,
        };
        let attr = BufferAttr {
            maxlength: u32::MAX,
            tlength: if record { u32::MAX } else { fragment_bytes * 4 },
            prebuf: u32::MAX,
            minreq: u32::MAX,
            fragsize: if record { fragment_bytes } else { u32::MAX },
        };
        let mut error = 0;
        // SAFETY: valid C strings, spec and attr; NULL server/device/map
        // mean the defaults.
        let handle = unsafe {
            (pulse.new)(
                std::ptr::null(),
                c"Roblox (Feline)".as_ptr(),
                if record { PA_STREAM_RECORD } else { PA_STREAM_PLAYBACK },
                std::ptr::null(),
                name.as_ptr(),
                &spec,
                std::ptr::null(),
                &attr,
                &mut error,
            )
        };
        if handle.is_null() {
            eprintln!("feline-runtime: sound: the sound server refused {name:?} (error {error})");
            return None;
        }
        Some(Self { pulse, handle })
    }

    /// Fills `buffer` from the microphone (blocks until it's full).
    pub fn read(&mut self, buffer: &mut [u8]) -> bool {
        let mut error = 0;
        // SAFETY: a live stream and `buffer.len()` writable bytes.
        unsafe { (self.pulse.read)(self.handle, buffer.as_mut_ptr().cast(), buffer.len(), &mut error) == 0 }
    }

    /// Plays `data` (blocks while the stream is full).
    pub fn write(&mut self, data: &[u8]) -> bool {
        let mut error = 0;
        // SAFETY: a live stream and `data.len()` readable bytes.
        unsafe { (self.pulse.write)(self.handle, data.as_ptr().cast(), data.len(), &mut error) == 0 }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        // SAFETY: a stream from `pa_simple_new`, freed once.
        unsafe { (self.pulse.free)(self.handle) };
    }
}

#[cfg(test)]
mod tests {
    /// Opens and closes an output on this machine's sound server (no sound
    /// is played). Ignored by default: needs a desktop session.
    #[test]
    #[ignore]
    fn opens_an_output() {
        assert!(super::init(2, 48000, 512, 4));
        super::close();
    }
}

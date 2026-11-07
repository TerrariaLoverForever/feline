// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! Voice chat: WebRTC's Android audio classes (`org.webrtc.voiceengine`),
//! which Roblox's voice chat uses for the microphone and for other
//! players' voices. On Android they're Java code around AudioRecord and
//! AudioTrack; here they're Feline's, around the desktop's sound server
//! (see `audio::Stream`).
//!
//! - `WebRtcAudioManager(long)`: reports the audio format to the engine at
//!   once (`nativeCacheAudioParameters`: 48 kHz, mono, 10 ms buffers, no
//!   hardware effects, no OpenSL ES), and `init()` says yes.
//! - `WebRtcAudioRecord(long)`: `initRecording(rate, channels)` hands the
//!   engine a 10 ms buffer (`nativeCacheDirectBufferAddress`);
//!   `startRecording()` starts a thread that fills it from the microphone
//!   and calls `nativeDataIsRecorded(bytes)` every 10 ms.
//! - `WebRtcAudioTrack(long)`: the reverse; a thread asks
//!   `nativeGetPlayoutData(bytes)` and plays the buffer.
//!
//! These follow WebRTC's public Java API (BSD-licensed) and the values
//! Mocktail uses (<https://github.com/komaruworld/mocktail>, Apache-2.0,
//! src/audio/webrtc_jni_audio_bridge.cc); the code is Feline's own.
//!
//! Unsafe: calling the engine's registered natives through their addresses,
//! and the engine reading and writing the leaked 10 ms buffers.
#![allow(unsafe_code)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::jni::{Handle, attach_env, current_env, new_direct_buffer, registered_native_of};

const MANAGER: &str = "org/webrtc/voiceengine/WebRtcAudioManager";
const RECORD: &str = "org/webrtc/voiceengine/WebRtcAudioRecord";
const TRACK: &str = "org/webrtc/voiceengine/WebRtcAudioTrack";

const RATE: i32 = 48_000;
/// Frames in 10 ms, WebRTC's unit.
const FRAMES_10MS: i32 = RATE / 100;

pub fn is_voice_class(class: &str) -> bool {
    matches!(class, MANAGER | RECORD | TRACK)
}

/// One recording or playback object and its thread.
struct Session {
    /// The engine's native peer (the constructor's `long`).
    native: i64,
    /// The 10 ms buffer the engine reads or writes (leaked; never freed,
    /// as the engine keeps its address).
    buffer: usize,
    bytes: usize,
    rate: u32,
    channels: u8,
    running: Option<(Arc<AtomicBool>, std::thread::JoinHandle<()>)>,
}

static SESSIONS: Mutex<Option<HashMap<usize, Session>>> = Mutex::new(None);
static MUTED: AtomicBool = AtomicBool::new(false);

fn with_sessions<T>(f: impl FnOnce(&mut HashMap<usize, Session>) -> T) -> T {
    let mut guard = SESSIONS.lock().unwrap_or_else(|p| p.into_inner());
    f(guard.get_or_insert_with(HashMap::new))
}

/// A WebRTC audio object was made (`new X(long nativePeer)`).
pub fn constructed(class: &str, object: Handle, native: i64) {
    eprintln!("feline-runtime: voice: {} created", short(class));
    if class == MANAGER {
        cache_audio_parameters(object, native);
        return;
    }
    with_sessions(|s| {
        s.insert(
            object as usize,
            Session {
                native,
                buffer: 0,
                bytes: 0,
                rate: RATE as u32,
                channels: 1,
                running: None,
            },
        )
    });
}

/// What the manager's Java constructor does: tell the engine the format.
fn cache_audio_parameters(object: Handle, native: i64) {
    let (Some(function), Some(env)) = (
        registered_native_of(MANAGER, "nativeCacheAudioParameters"),
        current_env(),
    ) else {
        eprintln!("feline-runtime: voice: can't report the audio format (no native)");
        return;
    };
    // SAFETY: the DEX signature (IIIZZZZZZZIIJ)V: sample rate, output and
    // input channels, hardware AEC/AGC/NS, low-latency output/input, pro
    // audio, AAudio, output and input buffer sizes (frames), native peer.
    type Cache = extern "C" fn(Handle, Handle, i32, i32, i32, u8, u8, u8, u8, u8, u8, u8, i32, i32, i64);
    let cache: Cache = unsafe { std::mem::transmute(function) };
    cache(env, object, RATE, 1, 1, 0, 0, 0, 0, 0, 0, 0, FRAMES_10MS, FRAMES_10MS, native);
    eprintln!("feline-runtime: voice: audio format reported ({RATE} Hz, mono)");
}

/// A Java call on a WebRTC audio object. None if Feline doesn't handle it.
pub fn call(class: &str, object: Handle, name: &str, read: &mut dyn FnMut() -> i64) -> Option<f64> {
    match class {
        MANAGER => manager_call(name, read),
        RECORD | TRACK => {
            let record = class == RECORD;
            session_call(record, object, name, read)
        }
        _ => None,
    }
}

fn manager_call(name: &str, read: &mut dyn FnMut() -> i64) -> Option<f64> {
    Some(match name {
        "init" => 1.0,
        // Recording goes through WebRtcAudioRecord (Feline's), not OpenSL ES.
        "isDeviceBlacklistedForOpenSLESUsage" => 1.0,
        "isCommunicationModeEnabled"
        | "isLowLatencyOutputSupported"
        | "isLowLatencyInputSupported"
        | "isProAudioSupported"
        | "isAAudioSupported"
        | "dispose" => 0.0,
        "setMicrophoneMute" => {
            let muted = read() & 0xff != 0;
            MUTED.store(muted, Ordering::Relaxed);
            eprintln!("feline-runtime: voice: microphone {}", if muted { "muted" } else { "on" });
            0.0
        }
        _ => return None,
    })
}

fn session_call(record: bool, object: Handle, name: &str, read: &mut dyn FnMut() -> i64) -> Option<f64> {
    Some(match name {
        // initRecording(int sampleRate, int channels) int framesPerBuffer;
        // initPlayout(int sampleRate, int channels, double factor) int
        // (the factor, a double, isn't read: it's last).
        "initRecording" | "initPlayout" => {
            let rate = read() as i32;
            let channels = read() as i32;
            match init_session(record, object, rate, channels) {
                Some(frames) if record => f64::from(frames),
                // AudioTrack's buffer: a few 10 ms blocks.
                Some(frames) => f64::from(frames * 4),
                None => -1.0,
            }
        }
        "startRecording" | "startPlayout" => f64::from(u8::from(start(record, object))),
        "stopRecording" | "stopPlayout" => {
            stop(object);
            1.0
        }
        "getBufferSizeInFrames" => f64::from(FRAMES_10MS * 4),
        "getStreamMaxVolume" | "getStreamVolume" => 100.0,
        "setStreamVolume" => {
            let _ = read();
            1.0
        }
        // Android's built-in echo cancelling etc.: accepted; WebRTC keeps
        // its own processing.
        "enableBuiltInAEC" | "enableBuiltInNS" | "enableBuiltInAGC" => {
            let _ = read();
            1.0
        }
        "isAudioConfigVerified" | "isAudioSourceMatchingRecordingSession" => 1.0,
        _ => return None,
    })
}

/// Makes the 10 ms buffer and hands it to the engine. The frames per
/// buffer, or None.
fn init_session(record: bool, object: Handle, rate: i32, channels: i32) -> Option<i32> {
    let class = if record { RECORD } else { TRACK };
    let (Ok(rate_u @ 8000..=192_000), Ok(channels_u @ 1..=2)) = (u32::try_from(rate), u8::try_from(channels)) else {
        eprintln!("feline-runtime: voice: unusable format {rate} Hz × {channels}");
        return None;
    };
    let frames = rate / 100;
    let bytes = frames as usize * usize::from(channels_u) * 2;
    let buffer: &'static mut [u8] = Box::leak(vec![0u8; bytes].into_boxed_slice());
    let address = buffer.as_mut_ptr();
    let native = with_sessions(|s| {
        let session = s.get_mut(&(object as usize))?;
        session.buffer = address as usize;
        session.bytes = bytes;
        session.rate = rate_u;
        session.channels = channels_u;
        Some(session.native)
    })?;
    let function = registered_native_of(class, "nativeCacheDirectBufferAddress")?;
    let env = current_env()?;
    // SAFETY: the DEX signature (Ljava/nio/ByteBuffer;J)V.
    type CacheBuffer = extern "C" fn(Handle, Handle, Handle, i64);
    let cache: CacheBuffer = unsafe { std::mem::transmute(function) };
    cache(env, object, new_direct_buffer(address, bytes), native);
    eprintln!(
        "feline-runtime: voice: {} ready ({rate} Hz × {channels}, {bytes} bytes per 10 ms)",
        short(class)
    );
    Some(frames)
}

/// Starts the 10 ms thread for a recording or playback object.
fn start(record: bool, object: Handle) -> bool {
    let class = if record { RECORD } else { TRACK };
    let native_name = if record { "nativeDataIsRecorded" } else { "nativeGetPlayoutData" };
    let Some(function) = registered_native_of(class, native_name) else {
        eprintln!("feline-runtime: voice: no {native_name}");
        return false;
    };
    let stop_flag = Arc::new(AtomicBool::new(false));
    let started = with_sessions(|s| {
        let session = s.get_mut(&(object as usize))?;
        if session.buffer == 0 || session.running.is_some() {
            return None;
        }
        let (buffer, bytes, rate, channels, native) =
            (session.buffer, session.bytes, session.rate, session.channels, session.native);
        let flag = Arc::clone(&stop_flag);
        let object = object as usize;
        let thread = std::thread::Builder::new()
            .name(if record { "voice-mic".into() } else { "voice-out".into() })
            .spawn(move || {
                // SAFETY: the DEX signatures (IJ)V: bytes, native peer.
                type Pass = extern "C" fn(Handle, Handle, i32, i64);
                let pass: Pass = unsafe { std::mem::transmute(function) };
                let Some(env) = attach_env() else {
                    return;
                };
                // SAFETY: the leaked buffer of `bytes` bytes, shared with
                // the engine, which only touches it inside `pass`.
                let shared = unsafe { std::slice::from_raw_parts_mut(buffer as *mut u8, bytes) };
                if record && !crate::prefs::get().microphone {
                    // Microphone off (runtime setting): silence every 10 ms,
                    // without opening the microphone.
                    eprintln!("feline-runtime: voice: microphone is off in Feline's settings");
                    shared.fill(0);
                    while !flag.load(Ordering::Relaxed) {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                        pass(env, object as Handle, bytes as i32, native);
                    }
                } else if record {
                    let Some(mut mic) =
                        crate::audio::Stream::open(true, rate, channels, bytes as u32, c"Voice chat microphone")
                    else {
                        return;
                    };
                    // FELINE_VOICE_SILENT=1 (for tests): the microphone is
                    // read but silence is sent.
                    let silent = std::env::var_os("FELINE_VOICE_SILENT").is_some();
                    eprintln!(
                        "feline-runtime: voice: microphone recording{}",
                        if silent { " (sending silence)" } else { "" }
                    );
                    let mut block = vec![0u8; bytes];
                    let mut loudest = 0i32;
                    let mut blocks = 0u32;
                    while !flag.load(Ordering::Relaxed) {
                        if !mic.read(&mut block) {
                            break;
                        }
                        // Once a second: how loud the microphone was (for
                        // checking it works, not what was said).
                        loudest = block
                            .as_chunks::<2>()
                            .0
                            .iter()
                            .map(|s| i32::from(i16::from_le_bytes(*s)).abs())
                            .fold(loudest, i32::max);
                        blocks += 1;
                        if blocks == 100 {
                            eprintln!("feline-runtime: voice: microphone level {loudest} / 32767");
                            (loudest, blocks) = (0, 0);
                        }
                        if silent || MUTED.load(Ordering::Relaxed) {
                            block.fill(0);
                        }
                        shared.copy_from_slice(&block);
                        pass(env, object as Handle, bytes as i32, native);
                    }
                } else {
                    let Some(mut out) =
                        crate::audio::Stream::open(false, rate, channels, bytes as u32, c"Voice chat")
                    else {
                        return;
                    };
                    eprintln!("feline-runtime: voice: playing other players' voices");
                    while !flag.load(Ordering::Relaxed) {
                        pass(env, object as Handle, bytes as i32, native);
                        if !out.write(shared) {
                            break;
                        }
                    }
                }
            })
            .ok()?;
        session.running = Some((Arc::clone(&stop_flag), thread));
        Some(())
    });
    started.is_some()
}

fn stop(object: Handle) {
    let running = with_sessions(|s| s.get_mut(&(object as usize)).and_then(|s| s.running.take()));
    if let Some((flag, thread)) = running {
        flag.store(true, Ordering::Relaxed);
        let _ = thread.join();
    }
}

fn short(class: &str) -> &str {
    class.rsplit('/').next().unwrap_or(class)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manager_answers() {
        let mut none = || 0;
        assert_eq!(manager_call("init", &mut none), Some(1.0));
        assert_eq!(manager_call("isDeviceBlacklistedForOpenSLESUsage", &mut none), Some(1.0));
        assert_eq!(manager_call("isAAudioSupported", &mut none), Some(0.0));
        let mut yes = || 1;
        assert_eq!(manager_call("setMicrophoneMute", &mut yes), Some(0.0));
        assert!(MUTED.load(Ordering::Relaxed));
        MUTED.store(false, Ordering::Relaxed);
        assert_eq!(manager_call("somethingElse", &mut none), None);
    }
}

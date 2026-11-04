// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! Android's asset manager (`AAssetManager`, `AAsset`) over the APK's
//! `assets/` folder, and `AConfiguration` (screen, language).
//!
//! The APK is a zip file. It's mapped into memory (not copied) and its
//! central directory is read with bounds-checked reads, like the ELF reader.
//! Stored (uncompressed) assets are handed out straight from the mapping,
//! and `AAsset_openFileDescriptor` returns the APK file plus the asset's
//! offset, as Android does. Deflated assets are unpacked when opened
//! (`miniz_oxide`).
//!
//! # `unsafe` in this module
//!
//! Mapping the APK, and C pointers from the engine (names, out values,
//! asset handles it got from us).

#![allow(unsafe_code)]
#![allow(non_snake_case)] // the NDK's exact names

use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::sync::OnceLock;

/// One file in the zip: how it's stored and where its data is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZipEntry {
    pub deflated: bool,
    /// Offset of the (possibly compressed) data in the file.
    pub data_offset: u64,
    pub compressed: u64,
    pub size: u64,
}

fn u16_at(d: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(at..at + 2)?.try_into().ok()?))
}
fn u32_at(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(at..at + 4)?.try_into().ok()?))
}

/// Reads a zip's central directory: name → entry. `None` for a file that
/// isn't a (non-zip64) zip or is damaged.
pub fn read_zip(data: &[u8]) -> Option<HashMap<String, ZipEntry>> {
    // The end-of-central-directory record is in the last 64 KiB + 22 bytes.
    let search_from = data.len().saturating_sub(65_536 + 22);
    let eocd = (search_from..data.len().saturating_sub(21))
        .rev()
        .find(|&i| u32_at(data, i) == Some(0x0605_4b50))?;
    let count = u16_at(data, eocd + 10)? as usize;
    let cd_offset = u32_at(data, eocd + 16)?;
    if cd_offset == u32::MAX {
        return None; // zip64: not used by APKs of this size
    }
    let mut at = cd_offset as usize;
    let mut entries = HashMap::with_capacity(count);
    for _ in 0..count {
        if u32_at(data, at)? != 0x0201_4b50 {
            return None;
        }
        let method = u16_at(data, at + 10)?;
        let compressed = u64::from(u32_at(data, at + 20)?);
        let size = u64::from(u32_at(data, at + 24)?);
        let (name_len, extra_len, comment_len) = (
            u16_at(data, at + 28)? as usize,
            u16_at(data, at + 30)? as usize,
            u16_at(data, at + 32)? as usize,
        );
        let local = u32_at(data, at + 42)? as usize;
        let name = String::from_utf8_lossy(data.get(at + 46..at + 46 + name_len)?).into_owned();
        // The data starts after the local header, whose name/extra lengths
        // may differ from the central directory's.
        if u32_at(data, local)? != 0x0403_4b50 {
            return None;
        }
        let local_name = u16_at(data, local + 26)? as usize;
        let local_extra = u16_at(data, local + 28)? as usize;
        let data_offset = (local + 30 + local_name + local_extra) as u64;
        if method == 0 || method == 8 {
            entries.insert(
                name,
                ZipEntry {
                    deflated: method == 8,
                    data_offset,
                    compressed,
                    size,
                },
            );
        }
        at += 46 + name_len + extra_len + comment_len;
    }
    Some(entries)
}

pub struct AssetManager {
    apk_path: CString,
    map: &'static [u8],
    entries: HashMap<String, ZipEntry>,
}

static MANAGER: OnceLock<AssetManager> = OnceLock::new();

/// Opens the APK for the asset manager (once). Maps it read-only.
pub fn set_apk(path: &std::path::Path) -> Result<(), String> {
    if MANAGER.get().is_some() {
        return Ok(());
    }
    let file =
        std::fs::File::open(path).map_err(|e| format!("couldn't open {}: {e}", path.display()))?;
    let len = file.metadata().map_err(|e| e.to_string())?.len() as usize;
    // SAFETY: a read-only private mapping of a file we opened; it's never
    // unmapped, so the slice stays valid for the process.
    let map = unsafe {
        rustix::mm::mmap(
            std::ptr::null_mut(),
            len,
            rustix::mm::ProtFlags::READ,
            rustix::mm::MapFlags::PRIVATE,
            &file,
            0,
        )
    }
    .map_err(|e| format!("couldn't map the APK: {e}"))?;
    // SAFETY: `map` is `len` readable bytes, valid forever (see above).
    let map: &'static [u8] = unsafe { std::slice::from_raw_parts(map.cast::<u8>(), len) };
    let entries = read_zip(map).ok_or("the APK isn't a readable zip file")?;
    let apk_path = CString::new(path.as_os_str().as_encoded_bytes()).map_err(|_| "bad APK path")?;
    let _ = MANAGER.set(AssetManager {
        apk_path,
        map,
        entries,
    });
    Ok(())
}

/// An open asset.
pub struct Asset {
    data: Vec<u8>,
    /// For stored assets: where the bytes are in the mapping / APK file.
    stored: Option<(&'static [u8], u64)>,
}

impl Asset {
    fn bytes(&self) -> &[u8] {
        self.stored.map_or(&self.data, |(slice, _)| slice)
    }
}

/// `AAssetManager* AAssetManager_fromJava(JNIEnv*, jobject)`
pub extern "C" fn AAssetManager_fromJava(
    _env: *mut c_void,
    _object: *mut c_void,
) -> *const AssetManager {
    MANAGER
        .get()
        .map_or(std::ptr::null(), |m| m as *const AssetManager)
}

/// `AAsset* AAssetManager_open(AAssetManager*, const char* filename, int mode)`
///
/// # Safety
/// `filename` is a valid C string.
pub unsafe extern "C" fn AAssetManager_open(
    manager: *const AssetManager,
    filename: *const c_char,
    _mode: c_int,
) -> *mut Asset {
    if manager.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: our own manager, and the engine's valid C string.
    let (manager, name) = unsafe { (&*manager, CStr::from_ptr(filename).to_string_lossy()) };
    let Some(entry) = manager.entries.get(&format!("assets/{name}")).copied() else {
        eprintln!("feline-runtime: asset {name} isn't in the APK");
        return std::ptr::null_mut();
    };
    let start = entry.data_offset as usize;
    let Some(raw) = manager.map.get(start..start + entry.compressed as usize) else {
        return std::ptr::null_mut();
    };
    let asset = if entry.deflated {
        match miniz_oxide::inflate::decompress_to_vec(raw) {
            Ok(data) => Asset { data, stored: None },
            Err(_) => {
                eprintln!("feline-runtime: asset {name} couldn't be unpacked");
                return std::ptr::null_mut();
            }
        }
    } else {
        Asset {
            data: Vec::new(),
            stored: Some((raw, entry.data_offset)),
        }
    };
    Box::into_raw(Box::new(asset))
}

/// # Safety
/// `asset` came from AAssetManager_open.
pub unsafe extern "C" fn AAsset_close(asset: *mut Asset) {
    if !asset.is_null() {
        // SAFETY: created with Box::into_raw in AAssetManager_open.
        drop(unsafe { Box::from_raw(asset) });
    }
}

/// # Safety
/// `asset` came from AAssetManager_open.
pub unsafe extern "C" fn AAsset_getBuffer(asset: *mut Asset) -> *const c_void {
    // SAFETY: a live asset.
    unsafe { (*asset).bytes().as_ptr().cast() }
}

/// # Safety
/// `asset` came from AAssetManager_open.
pub unsafe extern "C" fn AAsset_getLength(asset: *mut Asset) -> i64 {
    // SAFETY: a live asset.
    unsafe { (*asset).bytes().len() as i64 }
}

/// `int AAsset_openFileDescriptor(AAsset*, off_t* outStart, off_t* outLength)`:
/// only for stored assets, as on Android.
///
/// # Safety
/// `asset` came from AAssetManager_open; out pointers are writable.
pub unsafe extern "C" fn AAsset_openFileDescriptor(
    asset: *mut Asset,
    out_start: *mut i64,
    out_length: *mut i64,
) -> c_int {
    // SAFETY: a live asset.
    let Some((slice, offset)) = (unsafe { (*asset).stored }) else {
        return -1;
    };
    let Some(manager) = MANAGER.get() else {
        return -1;
    };
    // SAFETY: a valid path C string.
    let fd = unsafe { libc::open(manager.apk_path.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
    if fd >= 0 {
        // SAFETY: the engine's writable out pointers.
        unsafe {
            *out_start = offset as i64;
            *out_length = slice.len() as i64;
        }
    }
    fd
}

// ---------------------------------------------------------------------------
// AConfiguration
// ---------------------------------------------------------------------------

/// What Feline reports about the "device": a desktop screen, the language
/// and country from `LANG` (e.g. `en_US.UTF-8`).
#[derive(Debug, Clone, Copy)]
pub struct Configuration {
    language: [u8; 2],
    country: [u8; 2],
    width_dp: c_int,
    height_dp: c_int,
}

const ACONFIGURATION_SCREENSIZE_XLARGE: c_int = 4;
const ACONFIGURATION_NAVHIDDEN_YES: c_int = 2;

fn desktop_configuration() -> Configuration {
    let lang = std::env::var("LANG").unwrap_or_default();
    let two = |s: &str, upper: bool| -> [u8; 2] {
        let b = s.as_bytes();
        if b.len() >= 2 && b[..2].iter().all(u8::is_ascii_alphabetic) {
            let f = |c: u8| {
                if upper {
                    c.to_ascii_uppercase()
                } else {
                    c.to_ascii_lowercase()
                }
            };
            [f(b[0]), f(b[1])]
        } else if upper {
            *b"US"
        } else {
            *b"en"
        }
    };
    let country = lang.split(['_', '.']).nth(1).unwrap_or("US");
    Configuration {
        language: two(&lang, false),
        country: two(country, true),
        width_dp: 1920,
        height_dp: 1080,
    }
}

pub extern "C" fn AConfiguration_new() -> *mut Configuration {
    Box::into_raw(Box::new(desktop_configuration()))
}
/// # Safety
/// From AConfiguration_new.
pub unsafe extern "C" fn AConfiguration_delete(config: *mut Configuration) {
    if !config.is_null() {
        // SAFETY: created by AConfiguration_new.
        drop(unsafe { Box::from_raw(config) });
    }
}
/// # Safety
/// `config` from AConfiguration_new.
pub unsafe extern "C" fn AConfiguration_fromAssetManager(
    config: *mut Configuration,
    _manager: *const AssetManager,
) {
    // SAFETY: a live configuration.
    unsafe { *config = desktop_configuration() };
}
/// # Safety
/// `config` live; `out` has room for 2 chars.
pub unsafe extern "C" fn AConfiguration_getLanguage(
    config: *const Configuration,
    out: *mut c_char,
) {
    // SAFETY: as documented by the NDK (2 chars, not NUL-terminated).
    unsafe { std::ptr::copy_nonoverlapping((*config).language.as_ptr().cast(), out, 2) };
}
/// # Safety
/// `config` live; `out` has room for 2 chars.
pub unsafe extern "C" fn AConfiguration_getCountry(config: *const Configuration, out: *mut c_char) {
    // SAFETY: as above.
    unsafe { std::ptr::copy_nonoverlapping((*config).country.as_ptr().cast(), out, 2) };
}
/// # Safety
/// `config` live.
pub unsafe extern "C" fn AConfiguration_getScreenWidthDp(config: *const Configuration) -> c_int {
    // SAFETY: a live configuration.
    unsafe { (*config).width_dp }
}
/// # Safety
/// `config` live.
pub unsafe extern "C" fn AConfiguration_getScreenHeightDp(config: *const Configuration) -> c_int {
    // SAFETY: a live configuration.
    unsafe { (*config).height_dp }
}
pub extern "C" fn AConfiguration_getScreenSize(_config: *const Configuration) -> c_int {
    ACONFIGURATION_SCREENSIZE_XLARGE
}
pub extern "C" fn AConfiguration_getNavHidden(_config: *const Configuration) -> c_int {
    ACONFIGURATION_NAVHIDDEN_YES
}

pub fn implementation(name: &str) -> Option<u64> {
    let address = match name {
        "AAssetManager_fromJava" => AAssetManager_fromJava as *const () as usize,
        "AAssetManager_open" => AAssetManager_open as *const () as usize,
        "AAsset_close" => AAsset_close as *const () as usize,
        "AAsset_getBuffer" => AAsset_getBuffer as *const () as usize,
        "AAsset_getLength" => AAsset_getLength as *const () as usize,
        "AAsset_openFileDescriptor" => AAsset_openFileDescriptor as *const () as usize,
        "AConfiguration_new" => AConfiguration_new as *const () as usize,
        "AConfiguration_delete" => AConfiguration_delete as *const () as usize,
        "AConfiguration_fromAssetManager" => AConfiguration_fromAssetManager as *const () as usize,
        "AConfiguration_getLanguage" => AConfiguration_getLanguage as *const () as usize,
        "AConfiguration_getCountry" => AConfiguration_getCountry as *const () as usize,
        "AConfiguration_getScreenWidthDp" => AConfiguration_getScreenWidthDp as *const () as usize,
        "AConfiguration_getScreenHeightDp" => {
            AConfiguration_getScreenHeightDp as *const () as usize
        }
        "AConfiguration_getScreenSize" => AConfiguration_getScreenSize as *const () as usize,
        "AConfiguration_getNavHidden" => AConfiguration_getNavHidden as *const () as usize,
        _ => return None,
    };
    Some(address as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A zip with one stored and one deflated file, built by hand.
    fn tiny_zip() -> Vec<u8> {
        let mut z = Vec::new();
        let mut central = Vec::new();
        let files: [(&str, &[u8], bool); 2] = [
            ("assets/a.txt", b"hello cat", false),
            ("assets/b.txt", b"paws paws paws paws", true),
        ];
        for (name, content, deflate) in files {
            let data = if deflate {
                miniz_oxide::deflate::compress_to_vec(content, 6)
            } else {
                content.to_vec()
            };
            let local = z.len() as u32;
            z.extend(0x0403_4b50u32.to_le_bytes());
            z.extend([20, 0, 0, 0]);
            z.extend((if deflate { 8u16 } else { 0 }).to_le_bytes());
            z.extend([0u8; 8]); // time, date, crc
            z.extend((data.len() as u32).to_le_bytes());
            z.extend((content.len() as u32).to_le_bytes());
            z.extend((name.len() as u16).to_le_bytes());
            z.extend(0u16.to_le_bytes());
            z.extend(name.as_bytes());
            z.extend(&data);
            central.extend(0x0201_4b50u32.to_le_bytes());
            central.extend([20, 0, 20, 0, 0, 0]);
            central.extend((if deflate { 8u16 } else { 0 }).to_le_bytes());
            central.extend([0u8; 8]);
            central.extend((data.len() as u32).to_le_bytes());
            central.extend((content.len() as u32).to_le_bytes());
            central.extend((name.len() as u16).to_le_bytes());
            central.extend([0u8; 12]); // extra, comment, disk, attrs
            central.extend(local.to_le_bytes());
            central.extend(name.as_bytes());
        }
        let cd_offset = z.len() as u32;
        z.extend(&central);
        z.extend(0x0605_4b50u32.to_le_bytes());
        z.extend([0u8; 4]);
        z.extend(2u16.to_le_bytes());
        z.extend(2u16.to_le_bytes());
        z.extend((central.len() as u32).to_le_bytes());
        z.extend(cd_offset.to_le_bytes());
        z.extend(0u16.to_le_bytes());
        z
    }

    #[test]
    fn reads_stored_and_deflated_entries() {
        let zip = tiny_zip();
        let entries = read_zip(&zip).unwrap();
        let a = entries["assets/a.txt"];
        assert!(!a.deflated);
        assert_eq!(
            &zip[a.data_offset as usize..][..a.size as usize],
            b"hello cat"
        );
        let b = entries["assets/b.txt"];
        assert!(b.deflated);
        let raw = &zip[b.data_offset as usize..][..b.compressed as usize];
        assert_eq!(
            miniz_oxide::inflate::decompress_to_vec(raw).unwrap(),
            b"paws paws paws paws"
        );
        assert!(read_zip(b"not a zip").is_none());
    }
}

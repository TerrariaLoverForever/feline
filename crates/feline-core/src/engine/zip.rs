// SPDX-License-Identifier: GPL-3.0-or-later
//! A small zip reader for APK and XAPK files: the central directory, and
//! stored or deflated entries (all an APK uses). Bounds-checked reads; a
//! malformed file is an error, never a crash.

use std::collections::HashMap;

/// One file in a zip.
#[derive(Debug, Clone, Copy)]
pub struct Entry {
    method: u16,
    compressed: usize,
    size: usize,
    local_header: usize,
}

fn u16_at(d: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(at..at + 4)?.try_into().ok()?))
}

/// The end-of-central-directory record: (its offset, central directory
/// size, central directory offset).
pub fn end_record(data: &[u8]) -> Option<(usize, usize, usize)> {
    let lowest = data.len().saturating_sub(65_557);
    let eocd = (lowest..data.len().saturating_sub(21))
        .rev()
        .find(|&i| u32_at(data, i) == Some(0x0605_4b50))?;
    let size = u32_at(data, eocd + 12)? as usize;
    let offset = u32_at(data, eocd + 16)? as usize;
    Some((eocd, size, offset))
}

/// The files in a zip, by name.
pub fn entries(data: &[u8]) -> Option<HashMap<String, Entry>> {
    let (eocd, _, mut at) = end_record(data)?;
    let count = u16_at(data, eocd + 10)? as usize;
    let mut found = HashMap::with_capacity(count);
    for _ in 0..count {
        if u32_at(data, at)? != 0x0201_4b50 {
            return None;
        }
        let method = u16_at(data, at + 10)?;
        let compressed = u32_at(data, at + 20)? as usize;
        let size = u32_at(data, at + 24)? as usize;
        let name_len = u16_at(data, at + 28)? as usize;
        let extra_len = u16_at(data, at + 30)? as usize;
        let comment_len = u16_at(data, at + 32)? as usize;
        let local_header = u32_at(data, at + 42)? as usize;
        let name = String::from_utf8_lossy(data.get(at + 46..at + 46 + name_len)?).into_owned();
        found.insert(
            name,
            Entry {
                method,
                compressed,
                size,
                local_header,
            },
        );
        at += 46 + name_len + extra_len + comment_len;
    }
    Some(found)
}

/// An entry's contents (inflated if needed).
pub fn read(data: &[u8], entry: &Entry) -> Result<Vec<u8>, String> {
    let at = entry.local_header;
    if u32_at(data, at) != Some(0x0403_4b50) {
        return Err("bad local header".into());
    }
    // The local header's name and extra lengths may differ from the
    // central directory's.
    let name_len = u16_at(data, at + 26).ok_or("truncated")? as usize;
    let extra_len = u16_at(data, at + 28).ok_or("truncated")? as usize;
    let start = at + 30 + name_len + extra_len;
    let raw = data
        .get(start..start + entry.compressed)
        .ok_or("entry outside the file")?;
    match entry.method {
        0 => Ok(raw.to_vec()),
        8 => {
            let out = miniz_oxide::inflate::decompress_to_vec_with_limit(raw, entry.size)
                .map_err(|e| format!("inflate: {e:?}"))?;
            if out.len() != entry.size {
                return Err("wrong size after inflating".into());
            }
            Ok(out)
        }
        m => Err(format!("compression method {m} isn't supported")),
    }
}

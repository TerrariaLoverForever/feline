// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! Reading the app's version name from `AndroidManifest.xml` in the APK.
//!
//! The manifest is stored as Android's binary XML (AOSP's documented
//! `ResXMLTree` format): a string pool, then element chunks whose attributes
//! point into it. Only the `<manifest>` element's `versionName` attribute is
//! read, the version the app reports about itself. Every read is
//! bounds-checked.

const CHUNK_STRING_POOL: u16 = 0x0001;
const CHUNK_START_ELEMENT: u16 = 0x0102;
const UTF8_FLAG: u32 = 0x100;

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

/// The string pool's strings (decoded lazily by index).
struct Pool<'a> {
    data: &'a [u8],
    chunk: usize,
    count: u32,
    strings_start: usize,
    utf8: bool,
}

impl Pool<'_> {
    fn get(&self, index: u32) -> Option<String> {
        if index >= self.count {
            return None;
        }
        let offset = u32_at(self.data, self.chunk + 28 + index as usize * 4)? as usize;
        let at = self.chunk + self.strings_start + offset;
        if self.utf8 {
            // Lengths: UTF-16 length, then UTF-8 length (1 or 2 bytes each).
            let skip = |at: usize| -> Option<(usize, usize)> {
                let first = *self.data.get(at)? as usize;
                if first & 0x80 != 0 {
                    Some((
                        ((first & 0x7f) << 8) | *self.data.get(at + 1)? as usize,
                        at + 2,
                    ))
                } else {
                    Some((first, at + 1))
                }
            };
            let (_, at) = skip(at)?;
            let (len, at) = skip(at)?;
            let bytes = self.data.get(at..at + len)?;
            Some(String::from_utf8_lossy(bytes).into_owned())
        } else {
            let first = u16_at(self.data, at)? as usize;
            let (len, at) = if first & 0x8000 != 0 {
                (
                    ((first & 0x7fff) << 16) | u16_at(self.data, at + 2)? as usize,
                    at + 4,
                )
            } else {
                (first, at + 2)
            };
            let units: Option<Vec<u16>> = (0..len).map(|i| u16_at(self.data, at + i * 2)).collect();
            Some(String::from_utf16_lossy(&units?))
        }
    }
}

/// The `versionName` of a binary `AndroidManifest.xml`.
pub fn version_name(manifest: &[u8]) -> Option<String> {
    if u16_at(manifest, 0)? != 0x0003 {
        return None; // not a binary XML file
    }
    let mut at = usize::from(u16_at(manifest, 2)?);
    let mut pool: Option<Pool> = None;
    while at + 8 <= manifest.len() {
        let kind = u16_at(manifest, at)?;
        let size = u32_at(manifest, at + 4)? as usize;
        if size < 8 {
            return None;
        }
        match kind {
            CHUNK_STRING_POOL => {
                pool = Some(Pool {
                    data: manifest,
                    chunk: at,
                    count: u32_at(manifest, at + 8)?,
                    strings_start: u32_at(manifest, at + 20)? as usize,
                    utf8: u32_at(manifest, at + 16)? & UTF8_FLAG != 0,
                });
            }
            CHUNK_START_ELEMENT => {
                let pool = pool.as_ref()?;
                if pool.get(u32_at(manifest, at + 20)?)?.as_str() == "manifest" {
                    let first = at + 16 + usize::from(u16_at(manifest, at + 24)?);
                    let attribute_size = usize::from(u16_at(manifest, at + 26)?);
                    let count = usize::from(u16_at(manifest, at + 28)?);
                    for i in 0..count {
                        let attribute = first + i * attribute_size;
                        let name = pool.get(u32_at(manifest, attribute + 4)?)?;
                        if name == "versionName" {
                            return pool.get(u32_at(manifest, attribute + 8)?);
                        }
                    }
                    return None;
                }
            }
            _ => {}
        }
        at += size;
    }
    None
}

/// The `versionName` of the APK's manifest.
pub fn apk_version_name(apk: &[u8]) -> Option<String> {
    let entries = crate::assets::read_zip(apk)?;
    let entry = entries.get("AndroidManifest.xml")?;
    let raw =
        apk.get(entry.data_offset as usize..(entry.data_offset + entry.compressed) as usize)?;
    let data = if entry.deflated {
        miniz_oxide::inflate::decompress_to_vec(raw).ok()?
    } else {
        raw.to_vec()
    };
    version_name(&data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_binary_xml() {
        assert_eq!(version_name(b"<manifest/>"), None);
        assert_eq!(version_name(&[]), None);
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! Reading the **declarations of native methods** from an app's DEX files.
//!
//! The engine's `Java_*` exports have names but no argument types; those are
//! declared in the app's Java part (DEX, the format documented by AOSP).
//! This reader takes only declarations from the DEX tables, the Java
//! equivalent of an ELF export table: class names, method names, type
//! signatures and access flags, for methods marked `native`. It never looks
//! at method bodies (each method's code offset is read past, never followed).
//! See CONTRIBUTING.md for this rule.
//!
//! Every read is bounds-checked.

use std::fmt;

const ACC_STATIC: u32 = 0x8;
const ACC_NATIVE: u32 = 0x100;

/// One `native` method declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeMethod {
    /// Slash-separated, e.g. `com/roblox/engine/jni/NativeGLInterface`.
    pub class: String,
    pub name: String,
    /// JNI signature, e.g. `(JLjava/lang/String;)V`.
    pub signature: String,
    pub is_static: bool,
}

impl fmt::Display for NativeMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = if self.is_static { "static " } else { "" };
        write!(f, "{s}{}.{} {}", self.class, self.name, self.signature)
    }
}

struct Dex<'a> {
    data: &'a [u8],
}

impl Dex<'_> {
    fn u16(&self, at: usize) -> Option<u16> {
        Some(u16::from_le_bytes(
            self.data.get(at..at + 2)?.try_into().ok()?,
        ))
    }
    fn u32(&self, at: usize) -> Option<u32> {
        Some(u32::from_le_bytes(
            self.data.get(at..at + 4)?.try_into().ok()?,
        ))
    }
    /// Unsigned LEB128 at `*at`, advancing it.
    fn uleb(&self, at: &mut usize) -> Option<u32> {
        let mut value = 0u32;
        for shift in (0..35).step_by(7) {
            let byte = *self.data.get(*at)?;
            *at += 1;
            value |= u32::from(byte & 0x7f).checked_shl(shift)?;
            if byte & 0x80 == 0 {
                return Some(value);
            }
        }
        None
    }
    fn table(&self, size_at: usize) -> Option<(usize, usize)> {
        Some((self.u32(size_at)? as usize, self.u32(size_at + 4)? as usize))
    }
    /// String `index` (MUTF-8; the names used here are plain ASCII).
    fn string(&self, index: u32) -> Option<String> {
        let (count, ids) = self.table(56)?;
        if index as usize >= count {
            return None;
        }
        let mut at = self.u32(ids + index as usize * 4)? as usize;
        self.uleb(&mut at)?; // length in UTF-16 units
        let rest = self.data.get(at..)?;
        let end = rest.iter().position(|&b| b == 0)?;
        Some(String::from_utf8_lossy(&rest[..end]).into_owned())
    }
    fn type_name(&self, index: u32) -> Option<String> {
        let (count, ids) = self.table(64)?;
        if index as usize >= count {
            return None;
        }
        self.string(self.u32(ids + index as usize * 4)?)
    }
    /// The JNI signature of prototype `index`: `(params)return`.
    fn signature(&self, index: u32) -> Option<String> {
        let (count, ids) = self.table(72)?;
        if index as usize >= count {
            return None;
        }
        let at = ids + index as usize * 12;
        let ret = self.type_name(self.u32(at + 4)?)?;
        let params_off = self.u32(at + 8)? as usize;
        let mut params = String::new();
        if params_off != 0 {
            let n = self.u32(params_off)? as usize;
            for i in 0..n {
                params.push_str(&self.type_name(u32::from(self.u16(params_off + 4 + i * 2)?))?);
            }
        }
        Some(format!("({params}){ret}"))
    }
    /// (class, name, signature) of method ID `index`.
    fn method(&self, index: u32) -> Option<(String, String, String)> {
        let (count, ids) = self.table(88)?;
        if index as usize >= count {
            return None;
        }
        let at = ids + index as usize * 8;
        let class = self.type_name(u32::from(self.u16(at)?))?;
        let signature = self.signature(u32::from(self.u16(at + 2)?))?;
        let name = self.string(self.u32(at + 4)?)?;
        Some((class, name, signature))
    }
}

/// `Lcom/foo/Bar;` → `com/foo/Bar`.
fn class_path(descriptor: &str) -> String {
    descriptor
        .strip_prefix('L')
        .and_then(|d| d.strip_suffix(';'))
        .unwrap_or(descriptor)
        .to_owned()
}

/// Every `native` method declared in one DEX file.
pub fn native_methods(data: &[u8]) -> Result<Vec<NativeMethod>, String> {
    if !data.starts_with(b"dex\n") {
        return Err("not a DEX file".into());
    }
    let dex = Dex { data };
    let (class_count, class_defs) = dex.table(96).ok_or("the DEX header is cut short")?;
    let mut out = Vec::new();
    for c in 0..class_count {
        let def = class_defs + c * 32;
        let class_data = dex.u32(def + 24).ok_or("a class definition is cut short")? as usize;
        if class_data == 0 {
            continue;
        }
        let mut at = class_data;
        let bad = || "damaged class data".to_owned();
        let static_fields = dex.uleb(&mut at).ok_or_else(bad)?;
        let instance_fields = dex.uleb(&mut at).ok_or_else(bad)?;
        let direct = dex.uleb(&mut at).ok_or_else(bad)?;
        let virtual_ = dex.uleb(&mut at).ok_or_else(bad)?;
        // Fields: (index difference, access flags), skipped.
        for _ in 0..u64::from(static_fields) + u64::from(instance_fields) {
            dex.uleb(&mut at).ok_or_else(bad)?;
            dex.uleb(&mut at).ok_or_else(bad)?;
        }
        // Methods: (index difference, access flags, code offset). The index
        // restarts for the virtual list. The code offset is read past only.
        for count in [direct, virtual_] {
            let mut index = 0u32;
            for _ in 0..count {
                index = index.wrapping_add(dex.uleb(&mut at).ok_or_else(bad)?);
                let flags = dex.uleb(&mut at).ok_or_else(bad)?;
                let _code_offset = dex.uleb(&mut at).ok_or_else(bad)?;
                if flags & ACC_NATIVE != 0
                    && let Some((class, name, signature)) = dex.method(index)
                {
                    out.push(NativeMethod {
                        class: class_path(&class),
                        name,
                        signature,
                        is_static: flags & ACC_STATIC != 0,
                    });
                }
            }
        }
    }
    Ok(out)
}

/// Every `native` method declared in the APK's `classes*.dex` files.
pub fn apk_native_methods(apk: &[u8]) -> Result<Vec<NativeMethod>, String> {
    let entries = crate::assets::read_zip(apk).ok_or("the APK isn't a readable zip file")?;
    let mut names: Vec<&String> = entries
        .keys()
        .filter(|n| n.starts_with("classes") && n.ends_with(".dex"))
        .collect();
    names.sort();
    let mut all = Vec::new();
    for name in names {
        let entry = entries[name];
        let raw = apk
            .get(entry.data_offset as usize..(entry.data_offset + entry.compressed) as usize)
            .ok_or("a DEX entry lies outside the APK")?;
        let data = if entry.deflated {
            miniz_oxide::inflate::decompress_to_vec(raw)
                .map_err(|_| format!("{name} couldn't be unpacked"))?
        } else {
            raw.to_vec()
        };
        all.extend(native_methods(&data).map_err(|e| format!("{name}: {e}"))?);
    }
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uleb128_and_class_paths() {
        let dex = Dex {
            data: &[0x7f, 0x80, 0x01, 0xe5, 0x8e, 0x26],
        };
        let mut at = 0;
        assert_eq!(dex.uleb(&mut at), Some(127));
        assert_eq!(dex.uleb(&mut at), Some(128));
        assert_eq!(dex.uleb(&mut at), Some(624_485)); // the DWARF spec's example
        assert_eq!(class_path("Lcom/roblox/A;"), "com/roblox/A");
        assert!(native_methods(b"not dex").is_err());
    }
}

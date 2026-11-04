// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! Putting a library into memory and relocating it (M3 step 2). Nothing in
//! it is run: no constructors, no `JNI_OnLoad`.
//!
//! What a dynamic loader does, in order:
//! 1. **Reserve** one block of memory big enough for every `PT_LOAD` segment
//!    (anonymous memory starts zero-filled, which covers `.bss`).
//! 2. **Copy** each segment's file bytes to its place in the block.
//! 3. **Relocate:** patch the addresses the compiler couldn't know. The
//!    library was linked as if loaded at address 0; the *load bias* is where
//!    it really is. `RELATIVE`: bias + addend. `GLOB_DAT`/`JUMP_SLOT`/`64`
//!    for a symbol the library defines itself: bias + its value (+ addend).
//!    Imports (functions from libc, EGL, …) get the address `imports.rs`
//!    bound them to (step 3); without bindings they're left at 0.
//! 4. **Protect:** set each segment's permissions (code read + execute, data
//!    read + write, never both write and execute), then make the RELRO part
//!    read-only, so data meant to be constant after loading can't be changed.
//!
//! Copying instead of mapping the file costs memory (the code isn't shared
//! with the page cache) but is simpler; mapping comes later, with
//! measurements.
//!
//! # `unsafe` in this module
//!
//! Only four operations: reserving memory (`mmap`), viewing it as a byte
//! slice, changing permissions (`mprotect`) and freeing it (`munmap`). All
//! copying and every relocation write goes through that slice, which Rust
//! bounds-checks, so a broken file gives an error instead of corrupting
//! memory. Each `unsafe` block says why it's sound.

#![allow(unsafe_code)]

use std::collections::BTreeSet;
use std::ffi::c_void;
use std::fmt;

use rustix::mm::{self, MapFlags, MprotectFlags, ProtFlags};

use crate::elf::{self, Elf, PT_GNU_RELRO, PT_LOAD, Segment, Symbol};
use crate::imports::Imports;

const R_X86_64_64: u32 = 1;
const R_X86_64_GLOB_DAT: u32 = 6;
const R_X86_64_JUMP_SLOT: u32 = 7;
const R_X86_64_RELATIVE: u32 = 8;

/// Refuse absurd sizes from a broken file instead of trying to reserve them.
const MAX_IMAGE: u64 = 4 << 30;

#[derive(Debug)]
pub enum LoadError {
    Elf(elf::ElfError),
    Layout(String),
    Unsupported(String),
    System(&'static str, rustix::io::Errno),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Elf(e) => e.fmt(f),
            Self::Layout(what) => write!(f, "bad memory layout: {what}"),
            Self::Unsupported(what) => write!(f, "not supported yet: {what}"),
            Self::System(what, e) => write!(f, "{what} failed: {e}"),
        }
    }
}

impl std::error::Error for LoadError {}

impl From<elf::ElfError> for LoadError {
    fn from(e: elf::ElfError) -> Self {
        Self::Elf(e)
    }
}

/// A library in memory. Freed (unmapped) when dropped.
pub struct Image {
    base: *mut c_void,
    len: usize,
    /// Added to the library's addresses to get real ones.
    bias: u64,
}

impl Image {
    /// The real address of the library's address 0.
    pub fn bias(&self) -> u64 {
        self.bias
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Reads 8 bytes at one of the library's addresses (e.g. a relocated
    /// pointer), if they're inside the image.
    pub fn read_u64(&self, vaddr: u64) -> Option<u64> {
        let start = self.bias.checked_add(vaddr)?;
        let base = self.base as u64;
        let end = start.checked_add(8)?;
        if start < base || end > base + self.len as u64 {
            return None;
        }
        // SAFETY: [start, start + 8) is inside the mapping (checked above),
        // every segment is mapped readable, and `read_unaligned` has no
        // alignment requirement.
        Some(unsafe { std::ptr::read_unaligned(start as *const u64) })
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        // SAFETY: `base`/`len` are exactly the mapping made in `load`, and
        // nothing borrows it any more (only `Image` holds the pointer).
        let _ = unsafe { mm::munmap(self.base, self.len) };
    }
}

/// What loading did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadReport {
    pub size: usize,
    pub relative: usize,
    /// Relocations to symbols the library defines itself.
    pub own_symbols: usize,
    /// Relocations to imports that were bound (step 3).
    pub imports_bound: usize,
    /// Relocations to imports left at 0 (no bindings given).
    pub imports_waiting: usize,
    /// The distinct imported names those relocations need.
    pub import_names: BTreeSet<String>,
    pub relro_bytes: u64,
}

fn align_down(v: u64, page: u64) -> u64 {
    v & !(page - 1)
}

fn align_up(v: u64, page: u64) -> Option<u64> {
    Some(v.checked_add(page - 1)? & !(page - 1))
}

/// Maps `elf`'s segments, applies its relocations (imports from `imports`,
/// if given) and sets the final permissions. Runs none of its code.
pub fn load(elf: &Elf, imports: Option<&Imports>) -> Result<(Image, LoadReport), LoadError> {
    let page = rustix::param::page_size() as u64;
    let loads: Vec<Segment> = elf
        .segments
        .iter()
        .copied()
        .filter(|s| s.kind == PT_LOAD)
        .collect();
    if loads.is_empty() {
        return Err(LoadError::Layout("no loadable segments".into()));
    }
    let overflow = || LoadError::Layout("addresses overflow".into());
    let min = align_down(loads.iter().map(|s| s.vaddr).min().unwrap_or(0), page);
    let mut max = 0u64;
    for s in &loads {
        max = max.max(
            align_up(s.vaddr.checked_add(s.memsz).ok_or_else(overflow)?, page)
                .ok_or_else(overflow)?,
        );
    }
    let size = max - min;
    if size == 0 || size > MAX_IMAGE {
        return Err(LoadError::Layout(format!("{size} bytes")));
    }
    let len = size as usize;

    // 1. Reserve. NORESERVE: don't count the whole block against memory
    //    limits up front; pages are only used when touched.
    // SAFETY: a fresh anonymous mapping at an address the kernel picks
    // can't overlap anything we own.
    let base = unsafe {
        mm::mmap_anonymous(
            std::ptr::null_mut(),
            len,
            ProtFlags::READ | ProtFlags::WRITE,
            MapFlags::PRIVATE | MapFlags::NORESERVE,
        )
    }
    .map_err(|e| LoadError::System("reserving memory (mmap)", e))?;
    // From here on, `image` frees the block if anything below fails.
    let image = Image {
        base,
        len,
        bias: (base as u64).wrapping_sub(min),
    };

    let report = {
        // SAFETY: the mapping is `len` bytes, readable and writable, and
        // only this slice refers to it until the end of this block (before
        // any permissions are changed).
        let memory: &mut [u8] = unsafe { std::slice::from_raw_parts_mut(base.cast::<u8>(), len) };

        // 2. Copy.
        for s in &loads {
            let data = elf.segment_data(s)?;
            let at = (s.vaddr - min) as usize;
            memory
                .get_mut(at..at + data.len())
                .ok_or_else(|| LoadError::Layout("a segment is outside the image".into()))?
                .copy_from_slice(data);
        }

        // 3. Relocate.
        relocate(elf, memory, min, image.bias, imports)?
    };

    // 4. Protect, each segment by its own flags, then RELRO read-only.
    for s in &loads {
        let start = align_down(s.vaddr, page);
        let end = align_up(s.vaddr + s.memsz, page).ok_or_else(overflow)?;
        let mut flags = MprotectFlags::empty();
        if s.flags & 4 != 0 {
            flags |= MprotectFlags::READ;
        }
        if s.flags & 2 != 0 {
            flags |= MprotectFlags::WRITE;
        }
        if s.flags & 1 != 0 {
            if s.flags & 2 != 0 {
                return Err(LoadError::Unsupported(
                    "a segment that is both writable and executable".into(),
                ));
            }
            flags |= MprotectFlags::EXEC;
        }
        protect(&image, start - min, end - start, flags)?;
    }
    let mut relro_bytes = 0;
    for r in elf.segments.iter().filter(|s| s.kind == PT_GNU_RELRO) {
        let start = align_down(r.vaddr, page);
        let end = align_up(r.vaddr + r.memsz, page).ok_or_else(overflow)?;
        protect(&image, start - min, end - start, MprotectFlags::READ)?;
        relro_bytes += end - start;
    }

    Ok((
        image,
        LoadReport {
            size: len,
            relro_bytes,
            ..report
        },
    ))
}

/// Changes the permissions of `[offset, offset + len)` inside the image.
fn protect(image: &Image, offset: u64, len: u64, flags: MprotectFlags) -> Result<(), LoadError> {
    if offset
        .checked_add(len)
        .is_none_or(|end| end > image.len as u64)
    {
        return Err(LoadError::Layout(
            "a protected range is outside the image".into(),
        ));
    }
    // SAFETY: the range is inside our mapping (checked above) and page
    // aligned (the callers align it); no Rust reference to it exists now.
    unsafe { mm::mprotect(image.base.byte_add(offset as usize), len as usize, flags) }
        .map_err(|e| LoadError::System("setting memory permissions (mprotect)", e))
}

/// Applies the relocations through the (bounds-checked) slice.
fn relocate(
    elf: &Elf,
    memory: &mut [u8],
    min: u64,
    bias: u64,
    imports: Option<&Imports>,
) -> Result<LoadReport, LoadError> {
    let symbols: Vec<Symbol> = elf.symbols()?;
    let mut report = LoadReport {
        size: 0,
        relative: 0,
        own_symbols: 0,
        imports_bound: 0,
        imports_waiting: 0,
        import_names: BTreeSet::new(),
        relro_bytes: 0,
    };
    for r in elf.relocations()? {
        let value = match r.kind() {
            R_X86_64_RELATIVE => {
                report.relative += 1;
                bias.wrapping_add(r.addend as u64)
            }
            kind @ (R_X86_64_GLOB_DAT | R_X86_64_JUMP_SLOT | R_X86_64_64) => {
                // `symbols()` skips the reserved index 0.
                let index = r.symbol() as usize;
                let symbol = index
                    .checked_sub(1)
                    .and_then(|i| symbols.get(i))
                    .ok_or_else(|| LoadError::Layout(format!("relocation to symbol {index}")))?;
                if !symbol.defined {
                    report.import_names.insert(symbol.name.clone());
                    let Some(imports) = imports else {
                        report.imports_waiting += 1;
                        continue;
                    };
                    let binding = imports.bindings.get(&symbol.name).ok_or_else(|| {
                        LoadError::Layout(format!("no binding for the import `{}`", symbol.name))
                    })?;
                    report.imports_bound += 1;
                    if kind == R_X86_64_64 {
                        binding.address.wrapping_add(r.addend as u64)
                    } else {
                        binding.address
                    }
                } else {
                    report.own_symbols += 1;
                    let address = bias.wrapping_add(symbol.value);
                    if kind == R_X86_64_64 {
                        address.wrapping_add(r.addend as u64)
                    } else {
                        address
                    }
                }
            }
            other => {
                return Err(LoadError::Unsupported(format!(
                    "relocation type {other} ({})",
                    elf::reloc_name(other)
                )));
            }
        };
        let at = r
            .offset
            .checked_sub(min)
            .ok_or_else(|| LoadError::Layout("a relocation is below the image".into()))?
            as usize;
        memory
            .get_mut(at..at + 8)
            .ok_or_else(|| {
                LoadError::Layout(format!(
                    "relocation at {:#x} is outside the image",
                    r.offset
                ))
            })?
            .copy_from_slice(&value.to_le_bytes());
    }
    Ok(report)
}

impl LoadReport {
    pub fn to_text(&self, image: &Image) -> String {
        format!(
            "Mapped          {:.1} MB, library address 0 is at {:#x}\n\
             Relocated       {} relative, {} to its own symbols\n\
             Imports         {} relocations bound, {} left at 0, to {} imported names\n\
             Protected       code read+execute, data read+write, {:.1} MB RELRO read-only\n\
             Run             nothing (no constructors, no JNI_OnLoad)\n",
            self.size as f64 / 1e6,
            image.bias(),
            self.relative,
            self.own_symbols,
            self.imports_bound,
            self.imports_waiting,
            self.import_names.len(),
            self.relro_bytes as f64 / 1e6,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::elf::{DT_NULL, DT_RELA, DT_RELASZ};

    /// The tiny test library from `elf.rs`, plus three relocations:
    /// a RELATIVE one, one to its own `Java_x` and one to the import `puts`.
    fn tiny_with_relocations() -> Vec<u8> {
        let mut f = crate::elf::tests::tiny_elf();
        let put = |f: &mut Vec<u8>, at: usize, b: &[u8]| f[at..at + b.len()].copy_from_slice(b);
        let relocs: [(u64, u64, i64); 3] = [
            (0x3e0, 8, 0x100),         // RELATIVE: bias + 0x100
            (0x3e8, (2 << 32) | 6, 0), // GLOB_DAT Java_x (value 0x100)
            (0x3f0, (1 << 32) | 7, 0), // JUMP_SLOT puts (import)
        ];
        for (i, (off, info, addend)) in relocs.iter().enumerate() {
            // After the dynamic section's last entry (which ends at 0x390).
            let at = 0x390 + i * 24;
            put(&mut f, at, &off.to_le_bytes());
            put(&mut f, at + 8, &info.to_le_bytes());
            put(&mut f, at + 16, &addend.to_le_bytes());
        }
        // Replace the dynamic section's DT_NULL (entry 6) with RELA entries.
        for (i, (tag, val)) in [(DT_RELA, 0x390u64), (DT_RELASZ, 72), (DT_NULL, 0)]
            .iter()
            .enumerate()
        {
            put(&mut f, 0x300 + (6 + i) * 16, &tag.to_le_bytes());
            put(&mut f, 0x308 + (6 + i) * 16, &val.to_le_bytes());
        }
        f
    }

    #[test]
    fn loads_relocates_and_protects_a_tiny_library() {
        let data = tiny_with_relocations();
        let elf = Elf::parse(&data).unwrap();
        let (image, report) = load(&elf, None).unwrap();

        assert_eq!(report.relative, 1);
        assert_eq!(report.own_symbols, 1);
        assert_eq!(report.imports_waiting, 1);
        assert_eq!(report.import_names, BTreeSet::from(["puts".to_owned()]));
        // The patched values, read back from the (now read+execute) memory.
        assert_eq!(image.read_u64(0x3e0), Some(image.bias() + 0x100));
        assert_eq!(image.read_u64(0x3e8), Some(image.bias() + 0x100));
        assert_eq!(
            image.read_u64(0x3f0),
            Some(0),
            "imports aren't resolved yet"
        );
        // The file's own bytes were copied: the ELF magic at address 0.
        assert_eq!(
            image.read_u64(0).map(|v| v.to_le_bytes()[..4].to_vec()),
            Some(b"\x7fELF".to_vec())
        );
        assert_eq!(image.read_u64(u64::MAX - 3), None, "outside the image");
    }

    #[test]
    fn binds_imports_to_their_host_functions() {
        let data = tiny_with_relocations();
        let elf = Elf::parse(&data).unwrap();
        let host = crate::host::Host::open();
        let imports = crate::imports::bind(&elf.symbols().unwrap(), &host).unwrap();
        let (image, report) = load(&elf, Some(&imports)).unwrap();
        assert_eq!((report.imports_bound, report.imports_waiting), (1, 0));
        // `puts` is forwarded to glibc's puts.
        assert_eq!(image.read_u64(0x3f0), host.libc("puts"));
    }
}

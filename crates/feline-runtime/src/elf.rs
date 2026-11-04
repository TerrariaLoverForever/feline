// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! Reading a 64-bit little-endian ELF shared library (such as Roblox's
//! `libroblox.so`) from bytes, the way a dynamic loader sees it: through its
//! program headers and dynamic section, not its section headers (those are
//! optional and a loader must not depend on them).
//!
//! Every read is bounds-checked, so a broken or hostile file gives an error,
//! never a crash. Layouts follow the ELF specification (System V gABI) and
//! the x86-64 psABI.

use std::fmt;

use crate::packed;

// Program header types
pub const PT_LOAD: u32 = 1;
pub const PT_DYNAMIC: u32 = 2;
pub const PT_TLS: u32 = 7;
pub const PT_GNU_RELRO: u32 = 0x6474_e552;

// Dynamic tags used here
pub const DT_NULL: i64 = 0;
pub const DT_NEEDED: i64 = 1;
pub const DT_PLTRELSZ: i64 = 2;
pub const DT_STRTAB: i64 = 5;
pub const DT_SYMTAB: i64 = 6;
pub const DT_RELA: i64 = 7;
pub const DT_RELASZ: i64 = 8;
pub const DT_STRSZ: i64 = 10;
pub const DT_SONAME: i64 = 14;
pub const DT_JMPREL: i64 = 23;
pub const DT_INIT_ARRAYSZ: i64 = 27;
pub const DT_GNU_HASH: i64 = 0x6fff_fef5;
pub const DT_VERSYM: i64 = 0x6fff_fff0;
pub const DT_VERNEED: i64 = 0x6fff_fffe;
pub const DT_VERNEEDNUM: i64 = 0x6fff_ffff;
/// Android's packed relocation table ("APS2"), see `packed.rs`.
pub const DT_ANDROID_RELA: i64 = 0x6000_0011;
pub const DT_ANDROID_RELASZ: i64 = 0x6000_0012;

const EM_X86_64: u16 = 62;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ElfError {
    /// The file ends before a structure it describes.
    Truncated {
        what: &'static str,
    },
    NotElf,
    /// Not a 64-bit little-endian x86-64 shared library.
    Unsupported(String),
    /// An address that no loadable segment contains.
    BadAddress {
        what: &'static str,
        addr: u64,
    },
    /// A required dynamic entry is missing.
    Missing(&'static str),
    Packed(packed::PackedError),
}

impl fmt::Display for ElfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated { what } => write!(f, "the file ends inside the {what}"),
            Self::NotElf => f.write_str("not an ELF file"),
            Self::Unsupported(what) => write!(f, "unsupported ELF file: {what}"),
            Self::BadAddress { what, addr } => {
                write!(
                    f,
                    "the {what} points to {addr:#x}, outside every loadable segment"
                )
            }
            Self::Missing(what) => write!(f, "no {what} in the dynamic section"),
            Self::Packed(e) => write!(f, "packed relocations: {e}"),
        }
    }
}

impl std::error::Error for ElfError {}

impl From<packed::PackedError> for ElfError {
    fn from(e: packed::PackedError) -> Self {
        Self::Packed(e)
    }
}

pub type Result<T> = std::result::Result<T, ElfError>;

// ---------------------------------------------------------------------------
// Bounds-checked little-endian reads
// ---------------------------------------------------------------------------

fn bytes<'a>(data: &'a [u8], at: u64, len: u64, what: &'static str) -> Result<&'a [u8]> {
    let start = usize::try_from(at).map_err(|_| ElfError::Truncated { what })?;
    let len = usize::try_from(len).map_err(|_| ElfError::Truncated { what })?;
    start
        .checked_add(len)
        .and_then(|end| data.get(start..end))
        .ok_or(ElfError::Truncated { what })
}

fn u16_at(data: &[u8], at: u64, what: &'static str) -> Result<u16> {
    let b = bytes(data, at, 2, what)?;
    Ok(u16::from_le_bytes([b[0], b[1]]))
}

fn u32_at(data: &[u8], at: u64, what: &'static str) -> Result<u32> {
    let b = bytes(data, at, 4, what)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn u64_at(data: &[u8], at: u64, what: &'static str) -> Result<u64> {
    let b = bytes(data, at, 8, what)?;
    let mut a = [0u8; 8];
    a.copy_from_slice(b);
    Ok(u64::from_le_bytes(a))
}

// ---------------------------------------------------------------------------
// Structures
// ---------------------------------------------------------------------------

/// A program header (`Elf64_Phdr`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    pub kind: u32,
    /// PF_X = 1, PF_W = 2, PF_R = 4.
    pub flags: u32,
    pub offset: u64,
    pub vaddr: u64,
    pub filesz: u64,
    pub memsz: u64,
    pub align: u64,
}

/// One relocation (`Elf64_Rela`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rela {
    pub offset: u64,
    pub info: u64,
    pub addend: i64,
}

impl Rela {
    /// The relocation type, e.g. 8 = `R_X86_64_RELATIVE`.
    pub fn kind(&self) -> u32 {
        (self.info & 0xffff_ffff) as u32
    }
    /// Index of the symbol it refers to (0 = none).
    pub fn symbol(&self) -> u32 {
        (self.info >> 32) as u32
    }
}

/// x86-64 relocation type names (psABI), for reports.
pub fn reloc_name(kind: u32) -> &'static str {
    match kind {
        0 => "R_X86_64_NONE",
        1 => "R_X86_64_64",
        6 => "R_X86_64_GLOB_DAT",
        7 => "R_X86_64_JUMP_SLOT",
        8 => "R_X86_64_RELATIVE",
        16 => "R_X86_64_DTPMOD64",
        17 => "R_X86_64_DTPOFF64",
        18 => "R_X86_64_TPOFF64",
        37 => "R_X86_64_IRELATIVE",
        _ => "unknown",
    }
}

/// A dynamic symbol (`Elf64_Sym`), with its name and version resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub value: u64,
    pub size: u64,
    /// 0 = local, 1 = global, 2 = weak.
    pub bind: u8,
    /// 0 = none, 1 = object, 2 = function, …
    pub kind: u8,
    /// True if the library defines it (exports); false if it imports it.
    pub defined: bool,
    /// The version it needs, e.g. "LIBC", for imports that have one.
    pub version: Option<String>,
}

/// A parsed ELF shared library. Borrows the file's bytes.
pub struct Elf<'a> {
    data: &'a [u8],
    pub segments: Vec<Segment>,
    dynamic: Vec<(i64, u64)>,
}

impl<'a> Elf<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        if data.get(..4) != Some(b"\x7fELF".as_slice()) {
            return Err(ElfError::NotElf);
        }
        // e_ident: [4] class (2 = 64-bit), [5] data (1 = little-endian).
        if data.get(4) != Some(&2) || data.get(5) != Some(&1) {
            return Err(ElfError::Unsupported("not 64-bit little-endian".into()));
        }
        let kind = u16_at(data, 16, "ELF header")?;
        if kind != 3 {
            return Err(ElfError::Unsupported(format!(
                "type {kind}, not a shared library"
            )));
        }
        let machine = u16_at(data, 18, "ELF header")?;
        if machine != EM_X86_64 {
            return Err(ElfError::Unsupported(format!(
                "machine {machine}, not x86-64"
            )));
        }
        let phoff = u64_at(data, 32, "ELF header")?;
        let phentsize = u64::from(u16_at(data, 54, "ELF header")?);
        let phnum = u64::from(u16_at(data, 56, "ELF header")?);
        if phentsize < 56 {
            return Err(ElfError::Unsupported("program headers too small".into()));
        }

        let mut segments = Vec::new();
        for i in 0..phnum {
            let at = phoff + i * phentsize;
            let what = "program headers";
            segments.push(Segment {
                kind: u32_at(data, at, what)?,
                flags: u32_at(data, at + 4, what)?,
                offset: u64_at(data, at + 8, what)?,
                vaddr: u64_at(data, at + 16, what)?,
                filesz: u64_at(data, at + 32, what)?,
                memsz: u64_at(data, at + 40, what)?,
                align: u64_at(data, at + 48, what)?,
            });
        }

        let mut elf = Self {
            data,
            segments,
            dynamic: Vec::new(),
        };
        let dyn_seg = elf
            .segments
            .iter()
            .find(|s| s.kind == PT_DYNAMIC)
            .copied()
            .ok_or(ElfError::Missing("PT_DYNAMIC segment"))?;
        // `Elf64_Dyn` entries are 16 bytes; the list ends at DT_NULL.
        for i in 0..dyn_seg.filesz / 16 {
            let at = dyn_seg.offset + i * 16;
            let tag = u64_at(data, at, "dynamic section")? as i64;
            if tag == DT_NULL {
                break;
            }
            elf.dynamic
                .push((tag, u64_at(data, at + 8, "dynamic section")?));
        }
        Ok(elf)
    }

    /// The first value of a dynamic tag.
    pub fn dyn_value(&self, tag: i64) -> Option<u64> {
        self.dynamic
            .iter()
            .find(|(t, _)| *t == tag)
            .map(|(_, v)| *v)
    }

    /// The file offset of a virtual address, through the `PT_LOAD` segment
    /// that contains it.
    pub fn offset_of(&self, addr: u64, what: &'static str) -> Result<u64> {
        self.segments
            .iter()
            .filter(|s| s.kind == PT_LOAD)
            .find(|s| addr >= s.vaddr && addr < s.vaddr + s.filesz)
            .map(|s| addr - s.vaddr + s.offset)
            .ok_or(ElfError::BadAddress { what, addr })
    }

    /// The bytes a segment takes from the file (`p_filesz` of them).
    pub fn segment_data(&self, segment: &Segment) -> Result<&'a [u8]> {
        if segment.filesz > segment.memsz {
            return Err(ElfError::Unsupported(
                "a segment is larger in the file than in memory".into(),
            ));
        }
        bytes(self.data, segment.offset, segment.filesz, "segment data")
    }

    /// True if `addr` lies in the memory of some loadable segment (including
    /// its zero-filled part beyond the file data).
    pub fn in_memory_image(&self, addr: u64) -> bool {
        self.segments
            .iter()
            .any(|s| s.kind == PT_LOAD && addr >= s.vaddr && addr < s.vaddr + s.memsz)
    }

    /// A NUL-terminated string from the dynamic string table.
    fn string(&self, index: u64) -> Result<String> {
        let table = self
            .dyn_value(DT_STRTAB)
            .ok_or(ElfError::Missing("DT_STRTAB"))?;
        let size = self
            .dyn_value(DT_STRSZ)
            .ok_or(ElfError::Missing("DT_STRSZ"))?;
        if index >= size {
            return Err(ElfError::Truncated {
                what: "string table",
            });
        }
        let start = self.offset_of(table, "string table")? + index;
        let rest = bytes(self.data, start, size - index, "string table")?;
        let end = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or(ElfError::Truncated {
                what: "string table",
            })?;
        Ok(String::from_utf8_lossy(&rest[..end]).into_owned())
    }

    /// Libraries the file needs (`DT_NEEDED`), in order.
    pub fn needed(&self) -> Result<Vec<String>> {
        self.dynamic
            .iter()
            .filter(|(t, _)| *t == DT_NEEDED)
            .map(|(_, v)| self.string(*v))
            .collect()
    }

    pub fn soname(&self) -> Result<Option<String>> {
        self.dyn_value(DT_SONAME)
            .map(|v| self.string(v))
            .transpose()
    }

    /// The number of dynamic symbols, worked out from the GNU hash table
    /// (there's no count field; this is how loaders do it).
    pub fn symbol_count(&self) -> Result<u64> {
        let hash = self
            .dyn_value(DT_GNU_HASH)
            .ok_or(ElfError::Missing("DT_GNU_HASH"))?;
        let at = self.offset_of(hash, "GNU hash table")?;
        let what = "GNU hash table";
        let nbuckets = u64::from(u32_at(self.data, at, what)?);
        let symoffset = u64::from(u32_at(self.data, at + 4, what)?);
        let bloom_size = u64::from(u32_at(self.data, at + 8, what)?);
        let buckets = at + 16 + bloom_size * 8;
        let chains = buckets + nbuckets * 4;

        let mut last = 0;
        for b in 0..nbuckets {
            last = last.max(u64::from(u32_at(self.data, buckets + b * 4, what)?));
        }
        if last < symoffset {
            return Ok(symoffset); // only symbols that aren't in the hash table
        }
        // Follow the last bucket's chain to its end (lowest bit set).
        loop {
            let entry = u32_at(self.data, chains + (last - symoffset) * 4, what)?;
            if entry & 1 == 1 {
                return Ok(last + 1);
            }
            last += 1;
        }
    }

    /// Version names by version index, from `DT_VERNEED` (e.g. 2 → "LIBC").
    fn needed_versions(&self) -> Result<Vec<(u16, String)>> {
        let (Some(addr), Some(count)) = (self.dyn_value(DT_VERNEED), self.dyn_value(DT_VERNEEDNUM))
        else {
            return Ok(Vec::new());
        };
        let what = "version requirements";
        let mut out = Vec::new();
        let mut entry = self.offset_of(addr, what)?;
        for _ in 0..count {
            // Elf64_Verneed: version u16, cnt u16, file u32, aux u32, next u32
            let cnt = u16_at(self.data, entry + 2, what)?;
            let mut aux = entry + u64::from(u32_at(self.data, entry + 8, what)?);
            for _ in 0..cnt {
                // Elf64_Vernaux: hash u32, flags u16, other u16, name u32, next u32
                let index = u16_at(self.data, aux + 6, what)?;
                let name = self.string(u64::from(u32_at(self.data, aux + 8, what)?))?;
                out.push((index, name));
                aux += u64::from(u32_at(self.data, aux + 12, what)?);
            }
            entry += u64::from(u32_at(self.data, entry + 12, what)?);
        }
        Ok(out)
    }

    /// All dynamic symbols except the reserved first one.
    pub fn symbols(&self) -> Result<Vec<Symbol>> {
        let table = self
            .dyn_value(DT_SYMTAB)
            .ok_or(ElfError::Missing("DT_SYMTAB"))?;
        let base = self.offset_of(table, "symbol table")?;
        let versym = self
            .dyn_value(DT_VERSYM)
            .map(|a| self.offset_of(a, "symbol versions"))
            .transpose()?;
        let versions = self.needed_versions()?;
        let count = self.symbol_count()?;

        let mut symbols = Vec::new();
        for i in 1..count {
            let at = base + i * 24;
            let what = "symbol table";
            let info = bytes(self.data, at + 4, 1, what)?[0];
            let shndx = u16_at(self.data, at + 6, what)?;
            let version = match versym {
                // The high bit marks "hidden"; the rest is the index.
                Some(v) => {
                    let index = u16_at(self.data, v + i * 2, "symbol versions")? & 0x7fff;
                    versions
                        .iter()
                        .find(|(n, _)| *n == index)
                        .map(|(_, name)| name.clone())
                }
                None => None,
            };
            symbols.push(Symbol {
                name: self.string(u64::from(u32_at(self.data, at, what)?))?,
                value: u64_at(self.data, at + 8, what)?,
                size: u64_at(self.data, at + 16, what)?,
                bind: info >> 4,
                kind: info & 0xf,
                defined: shndx != 0,
                version,
            });
        }
        Ok(symbols)
    }

    /// Reads a plain `Elf64_Rela` table.
    fn rela_table(&self, addr: u64, size: u64, what: &'static str) -> Result<Vec<Rela>> {
        let at = self.offset_of(addr, what)?;
        let raw = bytes(self.data, at, size, what)?;
        // `as_chunks` gives fixed-size `[u8; 24]` entries (one Elf64_Rela
        // each); a partial entry at the end is ignored.
        let (entries, _partial) = raw.as_chunks::<24>();
        Ok(entries
            .iter()
            .map(|c| {
                let word = |i: usize| {
                    let mut a = [0u8; 8];
                    a.copy_from_slice(&c[i..i + 8]);
                    u64::from_le_bytes(a)
                };
                Rela {
                    offset: word(0),
                    info: word(8),
                    addend: word(16) as i64,
                }
            })
            .collect())
    }

    /// Every relocation the loader must apply: the plain `DT_RELA` and
    /// `DT_JMPREL` tables and Android's packed `DT_ANDROID_RELA` table.
    pub fn relocations(&self) -> Result<Vec<Rela>> {
        let mut all = Vec::new();
        if let (Some(a), Some(s)) = (self.dyn_value(DT_RELA), self.dyn_value(DT_RELASZ)) {
            all.extend(self.rela_table(a, s, "relocation table")?);
        }
        if let (Some(a), Some(s)) = (
            self.dyn_value(DT_ANDROID_RELA),
            self.dyn_value(DT_ANDROID_RELASZ),
        ) {
            let at = self.offset_of(a, "packed relocations")?;
            all.extend(packed::decode(bytes(
                self.data,
                at,
                s,
                "packed relocations",
            )?)?);
        }
        if let (Some(a), Some(s)) = (self.dyn_value(DT_JMPREL), self.dyn_value(DT_PLTRELSZ)) {
            all.extend(self.rela_table(a, s, "PLT relocations")?);
        }
        Ok(all)
    }

    /// Number of constructors the loader must call (`DT_INIT_ARRAYSZ` / 8).
    pub fn init_array_len(&self) -> u64 {
        self.dyn_value(DT_INIT_ARRAYSZ).unwrap_or(0) / 8
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Builds a tiny but valid ELF64 shared library: one PT_LOAD covering the
    /// whole file, a PT_DYNAMIC, a string table, a GNU hash table and a
    /// symbol table with one import and one export.
    pub fn tiny_elf() -> Vec<u8> {
        let mut f = vec![0u8; 0x400];
        let put = |f: &mut Vec<u8>, at: usize, bytes: &[u8]| {
            f[at..at + bytes.len()].copy_from_slice(bytes)
        };
        // ELF header
        put(&mut f, 0, b"\x7fELF\x02\x01\x01");
        put(&mut f, 16, &3u16.to_le_bytes()); // ET_DYN
        put(&mut f, 18, &EM_X86_64.to_le_bytes());
        put(&mut f, 32, &64u64.to_le_bytes()); // e_phoff
        put(&mut f, 54, &56u16.to_le_bytes()); // e_phentsize
        put(&mut f, 56, &2u16.to_le_bytes()); // e_phnum
        // PT_LOAD: whole file at vaddr 0
        let ph = |f: &mut Vec<u8>, at: usize, kind: u32, off: u64, size: u64| {
            put(f, at, &kind.to_le_bytes());
            put(f, at + 4, &5u32.to_le_bytes());
            put(f, at + 8, &off.to_le_bytes());
            put(f, at + 16, &off.to_le_bytes());
            put(f, at + 32, &size.to_le_bytes());
            put(f, at + 40, &size.to_le_bytes());
            put(f, at + 48, &0x1000u64.to_le_bytes());
        };
        ph(&mut f, 64, PT_LOAD, 0, 0x400);
        ph(&mut f, 120, PT_DYNAMIC, 0x300, 0x100);
        // Strings at 0x200: "\0libc.so\0libtiny.so\0puts\0Java_x\0"
        let strings = b"\0libc.so\0libtiny.so\0puts\0Java_x\0";
        put(&mut f, 0x200, strings);
        // Symbols at 0x240: [0] reserved, [1] puts (undefined), [2] Java_x (defined)
        let sym = |f: &mut Vec<u8>, i: usize, name: u32, info: u8, shndx: u16, value: u64| {
            let at = 0x240 + i * 24;
            put(f, at, &name.to_le_bytes());
            f[at + 4] = info;
            put(f, at + 6, &shndx.to_le_bytes());
            put(f, at + 8, &value.to_le_bytes());
        };
        sym(&mut f, 1, 20, 0x12, 0, 0); // GLOBAL FUNC, undefined
        sym(&mut f, 2, 25, 0x12, 1, 0x100); // GLOBAL FUNC, defined
        // GNU hash at 0x2a0: 1 bucket, symoffset 1, bloom 1, shift 0;
        // bucket → symbol 1; chains: [1] continues, [2] ends.
        put(&mut f, 0x2a0, &1u32.to_le_bytes());
        put(&mut f, 0x2a4, &1u32.to_le_bytes());
        put(&mut f, 0x2a8, &1u32.to_le_bytes());
        put(&mut f, 0x2b8, &1u32.to_le_bytes()); // bucket[0] (after 8-byte bloom)
        put(&mut f, 0x2bc, &0u32.to_le_bytes()); // chain for sym 1
        put(&mut f, 0x2c0, &1u32.to_le_bytes()); // chain for sym 2: end
        // Dynamic section at 0x300
        let dynamic: [(i64, u64); 7] = [
            (DT_NEEDED, 1),
            (DT_SONAME, 9),
            (DT_STRTAB, 0x200),
            (DT_STRSZ, strings.len() as u64),
            (DT_SYMTAB, 0x240),
            (DT_GNU_HASH, 0x2a0),
            (DT_NULL, 0),
        ];
        for (i, (tag, val)) in dynamic.iter().enumerate() {
            put(&mut f, 0x300 + i * 16, &tag.to_le_bytes());
            put(&mut f, 0x308 + i * 16, &val.to_le_bytes());
        }
        f
    }

    #[test]
    fn parses_a_tiny_library() {
        let data = tiny_elf();
        let elf = Elf::parse(&data).unwrap();
        assert_eq!(elf.needed().unwrap(), ["libc.so"]);
        assert_eq!(elf.soname().unwrap().as_deref(), Some("libtiny.so"));
        assert_eq!(elf.symbol_count().unwrap(), 3);
        let symbols = elf.symbols().unwrap();
        assert_eq!(symbols.len(), 2);
        assert_eq!(
            (symbols[0].name.as_str(), symbols[0].defined),
            ("puts", false)
        );
        assert_eq!(
            (symbols[1].name.as_str(), symbols[1].defined),
            ("Java_x", true)
        );
        assert!(elf.relocations().unwrap().is_empty());
    }

    #[test]
    fn broken_files_give_errors_not_crashes() {
        assert_eq!(Elf::parse(b"hello").err(), Some(ElfError::NotElf));
        let data = tiny_elf();
        // Cut the file in the middle of the dynamic section.
        assert!(matches!(
            Elf::parse(&data[..0x310]),
            Err(ElfError::Truncated { .. })
        ));
        // Every possible truncation must fail cleanly (or parse a prefix).
        for len in 0..data.len() {
            if let Ok(elf) = Elf::parse(&data[..len]) {
                let _ = elf.symbols();
                let _ = elf.needed();
            }
        }
        let mut wrong_machine = data.clone();
        wrong_machine[18] = 183; // aarch64
        assert!(matches!(
            Elf::parse(&wrong_machine),
            Err(ElfError::Unsupported(_))
        ));
    }
}

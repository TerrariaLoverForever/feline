// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! `feline-runtime --check <library>`: what the loader would have to do,
//! without loading anything. Also a consistency check of the ELF reader and
//! the packed-relocation decoder: every relocation must land inside the
//! library's memory image, have a known type, and name an existing symbol.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::elf::{self, Elf, PT_GNU_RELRO, PT_LOAD, PT_TLS, Result, reloc_name};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub soname: Option<String>,
    pub needed: Vec<String>,
    /// (permissions like "r-x", file size, memory size) per PT_LOAD.
    pub loads: Vec<(String, u64, u64)>,
    pub has_tls: bool,
    pub has_relro: bool,
    pub symbols: u64,
    /// Imports per required version ("" = no version).
    pub imports_by_version: BTreeMap<String, usize>,
    pub weak_imports: usize,
    pub exports: usize,
    pub java_natives: usize,
    pub has_jni_onload: bool,
    pub relocations_by_type: BTreeMap<&'static str, usize>,
    /// Relocations that fail the consistency checks (should all be 0).
    pub outside_image: usize,
    pub unknown_type: usize,
    pub bad_symbol: usize,
    pub constructors: u64,
}

impl Report {
    pub fn is_consistent(&self) -> bool {
        self.outside_image == 0 && self.unknown_type == 0 && self.bad_symbol == 0
    }
}

pub fn check(data: &[u8]) -> Result<Report> {
    let elf = Elf::parse(data)?;
    let symbols = elf.symbols()?;
    let symbol_count = elf.symbol_count()?;

    let mut imports_by_version = BTreeMap::new();
    let mut weak_imports = 0;
    for s in symbols.iter().filter(|s| !s.defined) {
        *imports_by_version
            .entry(s.version.clone().unwrap_or_default())
            .or_insert(0) += 1;
        if s.bind == 2 {
            weak_imports += 1;
        }
    }
    let exports: Vec<&elf::Symbol> = symbols.iter().filter(|s| s.defined).collect();

    let mut relocations_by_type = BTreeMap::new();
    let (mut outside_image, mut unknown_type, mut bad_symbol) = (0, 0, 0);
    for r in elf.relocations()? {
        let name = reloc_name(r.kind());
        *relocations_by_type.entry(name).or_insert(0) += 1;
        if name == "unknown" {
            unknown_type += 1;
        }
        // The 8-byte value being patched must lie inside the image.
        if !elf.in_memory_image(r.offset) || !elf.in_memory_image(r.offset + 7) {
            outside_image += 1;
        }
        if u64::from(r.symbol()) >= symbol_count {
            bad_symbol += 1;
        }
    }

    let perms = |flags: u32| {
        let bit = |b: u32, c: char| if flags & b != 0 { c } else { '-' };
        format!("{}{}{}", bit(4, 'r'), bit(2, 'w'), bit(1, 'x'))
    };
    Ok(Report {
        soname: elf.soname()?,
        needed: elf.needed()?,
        loads: elf
            .segments
            .iter()
            .filter(|s| s.kind == PT_LOAD)
            .map(|s| (perms(s.flags), s.filesz, s.memsz))
            .collect(),
        has_tls: elf.segments.iter().any(|s| s.kind == PT_TLS),
        has_relro: elf.segments.iter().any(|s| s.kind == PT_GNU_RELRO),
        symbols: symbol_count.saturating_sub(1),
        imports_by_version,
        weak_imports,
        exports: exports.len(),
        java_natives: exports
            .iter()
            .filter(|s| s.name.starts_with("Java_"))
            .count(),
        has_jni_onload: exports.iter().any(|s| s.name == "JNI_OnLoad"),
        relocations_by_type,
        outside_image,
        unknown_type,
        bad_symbol,
        constructors: elf.init_array_len(),
    })
}

impl Report {
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        let mb = |b: u64| b as f64 / 1e6;
        let _ = writeln!(
            out,
            "Library         {}",
            self.soname.as_deref().unwrap_or("(no soname)")
        );
        let _ = writeln!(out, "Needs           {}", self.needed.join(", "));
        for (perms, file, mem) in &self.loads {
            let _ = writeln!(
                out,
                "Segment         {perms}  {:.1} MB in file, {:.1} MB in memory",
                mb(*file),
                mb(*mem)
            );
        }
        let _ = writeln!(
            out,
            "TLS segment     {}\nRELRO           {}",
            if self.has_tls { "yes" } else { "no" },
            if self.has_relro { "yes" } else { "no" }
        );
        let imports: usize = self.imports_by_version.values().sum();
        let _ = writeln!(
            out,
            "Symbols         {} ({imports} imported, {} exported)",
            self.symbols, self.exports
        );
        for (version, n) in &self.imports_by_version {
            let label = if version.is_empty() {
                "(no version)"
            } else {
                version
            };
            let _ = writeln!(out, "  imports       {n:>5}  {label}");
        }
        let _ = writeln!(out, "  weak imports  {:>5}", self.weak_imports);
        let _ = writeln!(
            out,
            "Java natives    {} (JNI_OnLoad: {})",
            self.java_natives,
            if self.has_jni_onload { "yes" } else { "no" }
        );
        let total: usize = self.relocations_by_type.values().sum();
        let _ = writeln!(out, "Relocations     {total}");
        for (kind, n) in &self.relocations_by_type {
            let _ = writeln!(out, "  {kind:<20} {n:>8}");
        }
        let _ = writeln!(out, "Constructors    {}", self.constructors);
        let _ = writeln!(
            out,
            "Consistency     {} (outside image: {}, unknown type: {}, bad symbol: {})",
            if self.is_consistent() {
                "OK"
            } else {
                "PROBLEMS"
            },
            self.outside_image,
            self.unknown_type,
            self.bad_symbol
        );
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_the_tiny_library() {
        let report = check(&elf::tests::tiny_elf()).unwrap();
        assert_eq!(report.needed, ["libc.so"]);
        assert_eq!(report.java_natives, 1);
        assert_eq!(report.imports_by_version.get(""), Some(&1));
        assert!(report.is_consistent());
        assert!(report.to_text().contains("Consistency     OK"));
    }
}

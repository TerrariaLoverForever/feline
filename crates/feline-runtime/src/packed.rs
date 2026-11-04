// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! Decoding Android's packed relocation format ("APS2").
//!
//! Android libraries can store their relocations compressed, under the
//! dynamic tag `DT_ANDROID_RELA`. The format, as published with Android's
//! relocation packer (AOSP, Apache-2.0), is:
//!
//! ```text
//! "APS2"                          4-byte magic
//! count, initial offset           SLEB128 numbers
//! then groups, until `count` relocations were produced:
//!   group size, group flags       SLEB128
//!   offset delta  if GROUPED_BY_OFFSET_DELTA
//!   info          if GROUPED_BY_INFO
//!   addend delta  if GROUPED_BY_ADDEND (and HAS_ADDEND)
//!   per relocation in the group:
//!     offset delta  unless GROUPED_BY_OFFSET_DELTA
//!     info          unless GROUPED_BY_INFO
//!     addend delta  if HAS_ADDEND and not GROUPED_BY_ADDEND
//! ```
//!
//! Offsets and addends are running totals: each relocation's offset is the
//! previous one plus a delta. A group without HAS_ADDEND has addend 0.

use std::fmt;

use crate::elf::Rela;

const GROUPED_BY_INFO: i64 = 1;
const GROUPED_BY_OFFSET_DELTA: i64 = 2;
const GROUPED_BY_ADDEND: i64 = 4;
const HAS_ADDEND: i64 = 8;

/// More relocations than this means a corrupt count; refuse instead of
/// trying to allocate it.
const MAX_RELOCATIONS: i64 = 50_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackedError {
    BadMagic,
    /// The data ended inside a number or a group.
    Truncated,
    /// A number didn't fit in 64 bits.
    Overflow,
    BadCount(i64),
    /// A group claims more relocations than are left, or none.
    BadGroup {
        size: i64,
        left: i64,
    },
}

impl fmt::Display for PackedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadMagic => f.write_str("doesn't start with \"APS2\""),
            Self::Truncated => f.write_str("the data ends early"),
            Self::Overflow => f.write_str("a number is too large"),
            Self::BadCount(n) => write!(f, "impossible relocation count {n}"),
            Self::BadGroup { size, left } => {
                write!(f, "a group of {size} with {left} relocations left")
            }
        }
    }
}

impl std::error::Error for PackedError {}

/// Reads signed LEB128 numbers: 7 bits per byte, low bits first, the top bit
/// meaning "more bytes follow", sign-extended from the last byte's bit 6.
struct Sleb<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Sleb<'_> {
    fn next(&mut self) -> Result<i64, PackedError> {
        let mut value: i64 = 0;
        let mut shift = 0u32;
        loop {
            let byte = *self.data.get(self.pos).ok_or(PackedError::Truncated)?;
            self.pos += 1;
            if shift >= 64 {
                return Err(PackedError::Overflow);
            }
            value |= i64::from(byte & 0x7f) << shift;
            shift += 7;
            if byte & 0x80 == 0 {
                if shift < 64 && byte & 0x40 != 0 {
                    value |= -1i64 << shift; // sign-extend
                }
                return Ok(value);
            }
        }
    }
}

/// Decodes a packed relocation table (the bytes `DT_ANDROID_RELA` points to).
pub fn decode(data: &[u8]) -> Result<Vec<Rela>, PackedError> {
    let rest = data.strip_prefix(b"APS2").ok_or(PackedError::BadMagic)?;
    let mut r = Sleb { data: rest, pos: 0 };

    let count = r.next()?;
    if !(0..=MAX_RELOCATIONS).contains(&count) {
        return Err(PackedError::BadCount(count));
    }
    let mut offset = r.next()? as u64;
    let mut info: u64 = 0;
    let mut addend: i64 = 0;
    let mut out = Vec::with_capacity(count as usize);

    let mut left = count;
    while left > 0 {
        let size = r.next()?;
        if size <= 0 || size > left {
            return Err(PackedError::BadGroup { size, left });
        }
        let flags = r.next()?;
        let by_offset = flags & GROUPED_BY_OFFSET_DELTA != 0;
        let by_info = flags & GROUPED_BY_INFO != 0;
        let has_addend = flags & HAS_ADDEND != 0;
        let by_addend = has_addend && flags & GROUPED_BY_ADDEND != 0;

        let group_offset_delta = if by_offset { r.next()? } else { 0 };
        if by_info {
            info = r.next()? as u64;
        }
        if by_addend {
            addend = addend.wrapping_add(r.next()?);
        } else if !has_addend {
            addend = 0;
        }

        for _ in 0..size {
            let delta = if by_offset {
                group_offset_delta
            } else {
                r.next()?
            };
            offset = offset.wrapping_add(delta as u64);
            if !by_info {
                info = r.next()? as u64;
            }
            if has_addend && !by_addend {
                addend = addend.wrapping_add(r.next()?);
            }
            out.push(Rela {
                offset,
                info,
                addend,
            });
        }
        left -= size;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test-only SLEB128 encoder, so streams can be written by hand.
    fn sleb(mut v: i64, out: &mut Vec<u8>) {
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            let done = (v == 0 && byte & 0x40 == 0) || (v == -1 && byte & 0x40 != 0);
            out.push(if done { byte } else { byte | 0x80 });
            if done {
                break;
            }
        }
    }

    fn stream(numbers: &[i64]) -> Vec<u8> {
        let mut out = b"APS2".to_vec();
        for &n in numbers {
            sleb(n, &mut out);
        }
        out
    }

    #[test]
    fn sleb128_round_trips() {
        for v in [
            0,
            1,
            -1,
            63,
            64,
            -64,
            -65,
            127,
            128,
            0x1234_5678,
            i64::MAX,
            i64::MIN,
        ] {
            let mut bytes = Vec::new();
            sleb(v, &mut bytes);
            let mut r = Sleb {
                data: &bytes,
                pos: 0,
            };
            assert_eq!(r.next().unwrap(), v, "{v}");
            assert_eq!(r.pos, bytes.len());
        }
        // Known encodings from the DWARF specification.
        let mut r = Sleb {
            data: &[0x02, 0x7e, 0xff, 0x00, 0x81, 0x7f],
            pos: 0,
        };
        assert_eq!(
            [r.next(), r.next(), r.next(), r.next()].map(Result::unwrap),
            [2, -2, 127, -127]
        );
    }

    #[test]
    fn decodes_the_typical_relative_group() {
        // Three R_X86_64_RELATIVE (8) entries, 8 bytes apart from 0x1000,
        // grouped by offset delta and info, with per-entry addends.
        let flags = GROUPED_BY_OFFSET_DELTA | GROUPED_BY_INFO | HAS_ADDEND;
        let data = stream(&[3, 0x1000 - 8, 3, flags, 8, 8, 0x500, 0x10, -0x8]);
        let relocs = decode(&data).unwrap();
        let offsets: Vec<u64> = relocs.iter().map(|r| r.offset).collect();
        let addends: Vec<i64> = relocs.iter().map(|r| r.addend).collect();
        assert_eq!(offsets, [0x1000, 0x1008, 0x1010]);
        assert_eq!(addends, [0x500, 0x510, 0x508]);
        assert!(relocs.iter().all(|r| r.kind() == 8 && r.symbol() == 0));
    }

    #[test]
    fn decodes_ungrouped_and_no_addend_groups() {
        // Group 1: two GLOB_DAT (6) with symbols 5 and 9, ungrouped, no addend.
        // Group 2: one R_X86_64_64 (1), grouped by addend.
        let info = |sym: i64, kind: i64| (sym << 32) | kind;
        let data = stream(&[
            3,
            0x2000,
            2,
            0,
            0,
            info(5, 6),
            0x10,
            info(9, 6),
            1,
            GROUPED_BY_ADDEND | HAS_ADDEND | GROUPED_BY_INFO,
            info(3, 1),
            0x40,
            8,
        ]);
        let relocs = decode(&data).unwrap();
        assert_eq!(relocs.len(), 3);
        assert_eq!(
            (relocs[0].offset, relocs[0].symbol(), relocs[0].addend),
            (0x2000, 5, 0)
        );
        assert_eq!((relocs[1].offset, relocs[1].symbol()), (0x2010, 9));
        assert_eq!(
            (relocs[2].offset, relocs[2].kind(), relocs[2].addend),
            (0x2018, 1, 0x40)
        );
    }

    #[test]
    fn rejects_broken_streams() {
        assert_eq!(decode(b"APS1"), Err(PackedError::BadMagic));
        assert_eq!(decode(b"APS2"), Err(PackedError::Truncated));
        assert_eq!(decode(&stream(&[-5])), Err(PackedError::BadCount(-5)));
        // A group larger than what's left.
        assert!(matches!(
            decode(&stream(&[1, 0, 2, 0])),
            Err(PackedError::BadGroup { .. })
        ));
        // Ten bytes with the "more" bit set: too long for 64 bits.
        let mut long = b"APS2".to_vec();
        long.extend([0xffu8; 11]);
        assert_eq!(decode(&long), Err(PackedError::Overflow));
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! Swapping icons in a TrueType icon font, by glyph name.
//!
//! Roblox draws its top bar and menu icons from an icon font (BuilderIcons),
//! one named glyph per icon (`speech-bubble-align-center`, `gear`, …). The
//! Feline Theme replaces some of them with Feline's own drawings: Feline
//! ships a small font of its own glyphs, named like the icons they stand
//! in for, and [`replace_glyphs`] copies those glyphs into the user's own copy
//! of Roblox's font. Every other glyph, table and byte of layout information
//! stays as it was; Feline never ships or downloads Roblox's font.
//!
//! What's touched: `glyf` and `loca` (rebuilt, with 32-bit offsets), `head`
//! (the offset format and the whole-font checksum) and `maxp` (the largest
//! point and contour counts). Both fonts must be plain TrueType fonts with
//! glyph names (`post` format 2), as Roblox's are. Replacement glyphs must
//! be simple glyphs (no references to other glyphs).

use std::collections::HashMap;

fn u16_at(d: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(d.get(at..at + 2)?.try_into().ok()?))
}
fn u32_at(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(d.get(at..at + 4)?.try_into().ok()?))
}

/// A font's tables, in the order of its table directory.
struct Font<'a> {
    tables: Vec<([u8; 4], &'a [u8])>,
}

impl<'a> Font<'a> {
    fn parse(data: &'a [u8]) -> Result<Self, String> {
        let version = u32_at(data, 0).ok_or("not a font")?;
        if version != 0x0001_0000 && version != u32::from_be_bytes(*b"true") {
            return Err("not a TrueType font".to_owned());
        }
        let count = u16_at(data, 4).ok_or("not a font")? as usize;
        let mut tables = Vec::with_capacity(count);
        for i in 0..count {
            let rec = 12 + i * 16;
            let tag: [u8; 4] = data
                .get(rec..rec + 4)
                .and_then(|t| t.try_into().ok())
                .ok_or("damaged table directory")?;
            let offset = u32_at(data, rec + 8).ok_or("damaged table directory")? as usize;
            let length = u32_at(data, rec + 12).ok_or("damaged table directory")? as usize;
            let table = data
                .get(offset..offset.checked_add(length).ok_or("damaged table")?)
                .ok_or("a table lies outside the file")?;
            tables.push((tag, table));
        }
        Ok(Self { tables })
    }

    fn table(&self, tag: &[u8; 4]) -> Result<&'a [u8], String> {
        self.tables
            .iter()
            .find(|(t, _)| t == tag)
            .map(|(_, d)| *d)
            .ok_or_else(|| format!("the font has no {} table", String::from_utf8_lossy(tag)))
    }

    fn glyph_count(&self) -> Result<usize, String> {
        Ok(u16_at(self.table(b"maxp")?, 4).ok_or("damaged maxp")? as usize)
    }

    /// Each glyph's bytes in `glyf`.
    fn glyphs(&self) -> Result<Vec<&'a [u8]>, String> {
        let count = self.glyph_count()?;
        let long = u16_at(self.table(b"head")?, 50).ok_or("damaged head")? == 1;
        let loca = self.table(b"loca")?;
        let glyf = self.table(b"glyf")?;
        let offset = |i: usize| -> Option<usize> {
            if long {
                Some(u32_at(loca, i * 4)? as usize)
            } else {
                Some(u16_at(loca, i * 2)? as usize * 2)
            }
        };
        (0..count)
            .map(|i| {
                let (start, end) = (offset(i), offset(i + 1));
                match (start, end) {
                    (Some(s), Some(e)) if s <= e => glyf.get(s..e).ok_or_else(|| "damaged glyf".to_owned()),
                    _ => Err("damaged loca".to_owned()),
                }
            })
            .collect()
    }

    /// Glyph names from `post` format 2. Glyphs with one of the standard
    /// Macintosh names (`.notdef`, `space`, …) get `None`: icon names are
    /// never among them.
    fn glyph_names(&self) -> Result<Vec<Option<String>>, String> {
        let post = self.table(b"post")?;
        if u32_at(post, 0) != Some(0x0002_0000) {
            return Err("the font has no glyph names".to_owned());
        }
        let count = u16_at(post, 32).ok_or("damaged post")? as usize;
        let indexes: Vec<u16> = (0..count)
            .map(|i| u16_at(post, 34 + i * 2).ok_or("damaged post"))
            .collect::<Result<_, _>>()?;
        // The custom names: Pascal strings after the index array.
        let mut strings = Vec::new();
        let mut at = 34 + count * 2;
        while let Some(&len) = post.get(at) {
            let Some(bytes) = post.get(at + 1..at + 1 + len as usize) else {
                break; // some fonts end with stray bytes
            };
            strings.push(String::from_utf8_lossy(bytes).into_owned());
            at += 1 + len as usize;
        }
        Ok(indexes
            .iter()
            .map(|&i| {
                (i >= 258)
                    .then(|| strings.get(i as usize - 258).cloned())
                    .flatten()
            })
            .collect())
    }
}

/// Points and contours of a simple glyph (0, 0 for empty or composite ones).
fn glyph_size(glyph: &[u8]) -> (u16, u16) {
    let contours = u16_at(glyph, 0).unwrap_or(0) as i16;
    if contours <= 0 {
        return (0, 0);
    }
    let last = 10 + (contours as usize - 1) * 2;
    let points = u16_at(glyph, last).map_or(0, |p| p.saturating_add(1));
    (points, contours as u16)
}

/// The checksum of a table (big-endian u32 words, zero-padded).
fn checksum(data: &[u8]) -> u32 {
    data.chunks(4).fold(0u32, |sum, chunk| {
        let mut word = [0u8; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        sum.wrapping_add(u32::from_be_bytes(word))
    })
}

/// Copies every glyph of `donor` whose name also exists in `target` into
/// `target`. Returns the new font and how many glyphs were replaced.
pub fn replace_glyphs(target: &[u8], donor: &[u8]) -> Result<(Vec<u8>, usize), String> {
    let font = Font::parse(target)?;
    let donor_font = Font::parse(donor).map_err(|e| format!("Feline's icon font: {e}"))?;

    let donor_glyphs = donor_font.glyphs()?;
    let replacements: HashMap<String, &[u8]> = donor_font
        .glyph_names()?
        .into_iter()
        .zip(donor_glyphs)
        .filter_map(|(name, glyph)| Some((name?, glyph)))
        .filter(|(_, glyph)| u16_at(glyph, 0).is_some_and(|n| (n as i16) > 0))
        .collect();

    let names = font.glyph_names()?;
    let mut glyphs = font.glyphs()?;
    let mut replaced = 0;
    for (glyph, name) in glyphs.iter_mut().zip(&names) {
        if let Some(new) = name.as_ref().and_then(|n| replacements.get(n)) {
            *glyph = new;
            replaced += 1;
        }
    }

    // glyf and a 32-bit loca.
    let mut glyf = Vec::new();
    let mut loca = Vec::with_capacity((glyphs.len() + 1) * 4);
    let (mut max_points, mut max_contours) = (0u16, 0u16);
    for glyph in &glyphs {
        loca.extend_from_slice(&(glyf.len() as u32).to_be_bytes());
        glyf.extend_from_slice(glyph);
        while glyf.len() % 4 != 0 {
            glyf.push(0);
        }
        let (points, contours) = glyph_size(glyph);
        max_points = max_points.max(points);
        max_contours = max_contours.max(contours);
    }
    loca.extend_from_slice(&(glyf.len() as u32).to_be_bytes());

    let mut head = font.table(b"head")?.to_vec();
    if head.len() < 54 {
        return Err("damaged head".to_owned());
    }
    head[8..12].copy_from_slice(&[0; 4]); // checkSumAdjustment, set below
    head[50..52].copy_from_slice(&1u16.to_be_bytes()); // long loca offsets

    let mut maxp = font.table(b"maxp")?.to_vec();
    if u32_at(&maxp, 0) == Some(0x0001_0000) && maxp.len() >= 10 {
        let points = max_points.max(u16_at(&maxp, 6).unwrap_or(0));
        let contours = max_contours.max(u16_at(&maxp, 8).unwrap_or(0));
        maxp[6..8].copy_from_slice(&points.to_be_bytes());
        maxp[8..10].copy_from_slice(&contours.to_be_bytes());
    }

    // Write the font: the same tables in the same order, new ones swapped in.
    let tables: Vec<([u8; 4], Vec<u8>)> = font
        .tables
        .iter()
        .map(|(tag, data)| {
            let data = match tag {
                b"glyf" => glyf.clone(),
                b"loca" => loca.clone(),
                b"head" => head.clone(),
                b"maxp" => maxp.clone(),
                _ => data.to_vec(),
            };
            (*tag, data)
        })
        .collect();
    let mut out = Vec::new();
    out.extend_from_slice(&target[..12]); // sfnt version, table count, search fields
    let mut offset = 12 + tables.len() * 16;
    let mut head_offset = 0;
    for (tag, data) in &tables {
        if tag == b"head" {
            head_offset = offset;
        }
        out.extend_from_slice(tag);
        out.extend_from_slice(&checksum(data).to_be_bytes());
        out.extend_from_slice(&(offset as u32).to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        offset += data.len().div_ceil(4) * 4;
    }
    for (_, data) in &tables {
        out.extend_from_slice(data);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }
    let adjustment = 0xB1B0_AFBAu32.wrapping_sub(checksum(&out));
    out[head_offset + 8..head_offset + 12].copy_from_slice(&adjustment.to_be_bytes());
    Ok((out, replaced))
}

#[cfg(test)]
mod tests {
    use super::*;

    const REGULAR: &[u8] =
        include_bytes!("../../../assets/mods/feline-theme/font-patches/ExtraContent/LuaPackages/Packages/_Index/BuilderIcons/BuilderIcons/Font/BuilderIcons-Regular.ttf");
    const FILLED: &[u8] =
        include_bytes!("../../../assets/mods/feline-theme/font-patches/ExtraContent/LuaPackages/Packages/_Index/BuilderIcons/BuilderIcons/Font/BuilderIcons-Filled.ttf");

    #[test]
    fn swaps_named_glyphs_and_keeps_the_font_valid() {
        let (out, replaced) = replace_glyphs(REGULAR, FILLED).unwrap();
        assert_eq!(replaced, 9);
        let font = Font::parse(&out).unwrap();
        let names = font.glyph_names().unwrap();
        let glyphs = font.glyphs().unwrap();
        let filled = Font::parse(FILLED).unwrap();
        let filled_glyphs = filled.glyphs().unwrap();
        let filled_names = filled.glyph_names().unwrap();
        for (name, glyph) in names.iter().zip(&glyphs) {
            let Some(name) = name else { continue };
            let i = filled_names.iter().position(|n| n.as_deref() == Some(name)).unwrap();
            // The same glyph; only the zero padding after it may differ.
            let (ours, theirs) = (*glyph, filled_glyphs[i]);
            let len = ours.len().min(theirs.len());
            assert_eq!(ours[..len], theirs[..len], "{name}");
            assert!(ours[len..].iter().chain(&theirs[len..]).all(|&b| b == 0), "{name}");
        }
        // The whole-font checksum comes out right.
        assert_eq!(checksum(&out), 0xB1B0_AFBA);
    }

    #[test]
    fn roblox_icons_are_swapped_if_roblox_is_installed() {
        let Some(local) = std::env::var_os("LOCALAPPDATA") else {
            return;
        };
        let versions = std::path::Path::new(&local).join("Roblox").join("Versions");
        let Some(font) = std::fs::read_dir(versions)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| {
                e.path()
                    .join("ExtraContent/LuaPackages/Packages/_Index/BuilderIcons/BuilderIcons/Font/BuilderIcons-Regular.ttf")
            })
            .find(|p| p.is_file())
        else {
            return;
        };
        let original = std::fs::read(&font).unwrap();
        let (out, replaced) = replace_glyphs(&original, REGULAR).unwrap();
        assert_eq!(replaced, 9, "all nine icons exist in Roblox's font");
        let before = Font::parse(&original).unwrap();
        let after = Font::parse(&out).unwrap();
        assert_eq!(before.glyph_count().unwrap(), after.glyph_count().unwrap());
        // An icon Feline doesn't replace is byte for byte the same.
        let names = before.glyph_names().unwrap();
        let backpack = names.iter().position(|n| n.as_deref() == Some("backpack")).unwrap();
        assert_eq!(before.glyphs().unwrap()[backpack], after.glyphs().unwrap()[backpack]);
        assert_eq!(checksum(&out), 0xB1B0_AFBA);
    }

    #[test]
    fn other_files_are_refused() {
        assert!(replace_glyphs(b"not a font at all", REGULAR).is_err());
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! A small PNG reader and writer, for recolouring Roblox's UI images (see
//! [`crate::mods`]'s tints). Reads every non-interlaced PNG Roblox uses
//! (greyscale, RGB, palette with transparency, grey + alpha, RGBA; 1–16
//! bits) into 8-bit RGBA, and writes 8-bit RGBA. Deflate is `miniz_oxide`,
//! already a dependency.

/// An image as 8-bit RGBA, row by row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

fn be32(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(d.get(at..at + 4)?.try_into().ok()?))
}

pub fn decode(data: &[u8]) -> Result<Rgba, String> {
    if !data.starts_with(SIGNATURE) {
        return Err("not a PNG".to_owned());
    }
    let mut at = 8;
    let (mut width, mut height, mut depth, mut kind, mut interlace) = (0, 0, 0u8, 0u8, 0u8);
    let mut palette: &[u8] = &[];
    let mut trns: &[u8] = &[];
    let mut idat = Vec::new();
    while at + 8 <= data.len() {
        let len = be32(data, at).ok_or("damaged chunk")? as usize;
        let tag = &data[at + 4..at + 8];
        let body = data.get(at + 8..at + 8 + len).ok_or("damaged chunk")?;
        match tag {
            b"IHDR" if body.len() >= 13 => {
                width = be32(body, 0).ok_or("damaged header")?;
                height = be32(body, 4).ok_or("damaged header")?;
                depth = body[8];
                kind = body[9];
                interlace = body[12];
            }
            b"PLTE" => palette = body,
            b"tRNS" => trns = body,
            b"IDAT" => idat.extend_from_slice(body),
            b"IEND" => break,
            _ => {}
        }
        at += 12 + len;
    }
    if width == 0 || height == 0 || width > 16384 || height > 16384 {
        return Err("bad size".to_owned());
    }
    if interlace != 0 {
        return Err("interlaced PNGs aren't supported".to_owned());
    }
    let channels: usize = match kind {
        0 => 1,
        2 => 3,
        3 => 1,
        4 => 2,
        6 => 4,
        _ => return Err("unknown colour type".to_owned()),
    };
    if !matches!(depth, 1 | 2 | 4 | 8 | 16) || (depth < 8 && !matches!(kind, 0 | 3)) {
        return Err("unsupported bit depth".to_owned());
    }
    let raw = miniz_oxide::inflate::decompress_to_vec_zlib(&idat).map_err(|_| "damaged image data")?;
    let bits_per_pixel = channels * depth as usize;
    let stride = (width as usize * bits_per_pixel).div_ceil(8);
    let bpp = bits_per_pixel.div_ceil(8).max(1);
    if raw.len() < (stride + 1) * height as usize {
        return Err("image data too short".to_owned());
    }

    // Undo the row filters.
    let mut rows = vec![0u8; stride * height as usize];
    for y in 0..height as usize {
        let filter = raw[y * (stride + 1)];
        let src = &raw[y * (stride + 1) + 1..(y + 1) * (stride + 1)];
        let (done, rest) = rows.split_at_mut(y * stride);
        let prev = if y > 0 { &done[(y - 1) * stride..] } else { &[][..] };
        let cur = &mut rest[..stride];
        for x in 0..stride {
            let a = if x >= bpp { cur[x - bpp] } else { 0 };
            let b = prev.get(x).copied().unwrap_or(0);
            let c = if x >= bpp { prev.get(x - bpp).copied().unwrap_or(0) } else { 0 };
            cur[x] = src[x].wrapping_add(match filter {
                0 => 0,
                1 => a,
                2 => b,
                3 => ((a as u16 + b as u16) / 2) as u8,
                4 => paeth(a, b, c),
                _ => return Err("unknown row filter".to_owned()),
            });
        }
    }

    // To 8-bit RGBA.
    let sample = |row: &[u8], i: usize| -> u16 {
        match depth {
            16 => u16::from_be_bytes([row[i * 2], row[i * 2 + 1]]) >> 8,
            8 => row[i] as u16,
            d => {
                let per_byte = 8 / d as usize;
                let byte = row[i / per_byte];
                let shift = 8 - d as usize * (i % per_byte + 1);
                ((byte >> shift) & ((1 << d) - 1)) as u16
            }
        }
    };
    let scale = |v: u16| -> u8 {
        match depth {
            1 => (v * 255) as u8,
            2 => (v * 85) as u8,
            4 => (v * 17) as u8,
            _ => v as u8,
        }
    };
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height as usize {
        let row = &rows[y * stride..(y + 1) * stride];
        for x in 0..width as usize {
            let s = |c: usize| sample(row, x * channels + c);
            let px = match kind {
                0 => {
                    let g = scale(s(0));
                    // tRNS names one grey value (in the image's own bit depth).
                    let transparent = trns.len() >= 2 && {
                        let key = u16::from_be_bytes([trns[0], trns[1]]);
                        (if depth == 16 { key >> 8 } else { key }) == s(0)
                    };
                    [g, g, g, if transparent { 0 } else { 255 }]
                }
                2 => [s(0) as u8, s(1) as u8, s(2) as u8, 255],
                3 => {
                    let i = s(0) as usize;
                    let rgb = palette.get(i * 3..i * 3 + 3).unwrap_or(&[0, 0, 0]);
                    [rgb[0], rgb[1], rgb[2], trns.get(i).copied().unwrap_or(255)]
                }
                4 => [s(0) as u8, s(0) as u8, s(0) as u8, s(1) as u8],
                _ => [s(0) as u8, s(1) as u8, s(2) as u8, s(3) as u8],
            };
            pixels.extend_from_slice(&px);
        }
    }
    Ok(Rgba { width, height, pixels })
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i16 + b as i16 - c as i16;
    let (pa, pb, pc) = ((p - a as i16).abs(), (p - b as i16).abs(), (p - c as i16).abs());
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

pub(crate) fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn chunk(out: &mut Vec<u8>, tag: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(tag);
    out.extend_from_slice(body);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// Writes an 8-bit RGBA PNG (each row filtered with "Sub", which suits UI
/// art well enough).
pub fn encode(image: &Rgba) -> Vec<u8> {
    let stride = image.width as usize * 4;
    let mut raw = Vec::with_capacity((stride + 1) * image.height as usize);
    for row in image.pixels.chunks_exact(stride) {
        raw.push(1); // Sub
        for x in 0..stride {
            let left = if x >= 4 { row[x - 4] } else { 0 };
            raw.push(row[x].wrapping_sub(left));
        }
    }
    let mut out = SIGNATURE.to_vec();
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&image.width.to_be_bytes());
    header.extend_from_slice(&image.height.to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &header);
    chunk(&mut out, b"IDAT", &miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6));
    chunk(&mut out, b"IEND", &[]);
    out
}

/// `image` scaled to fit a `size`×`size` square (keeping its shape, centred
/// on transparency), with bilinear filtering. For cursors.
pub fn fit_square(image: &Rgba, size: u32) -> Rgba {
    let scale = (size as f32 / image.width as f32).min(size as f32 / image.height as f32);
    let (w, h) = (
        ((image.width as f32 * scale).round() as u32).clamp(1, size),
        ((image.height as f32 * scale).round() as u32).clamp(1, size),
    );
    let (ox, oy) = ((size - w) / 2, (size - h) / 2);
    let mut out = vec![0u8; (size * size * 4) as usize];
    let at = |x: u32, y: u32, c: usize| -> f32 {
        let x = x.min(image.width - 1);
        let y = y.min(image.height - 1);
        image.pixels[((y * image.width + x) * 4) as usize + c] as f32
    };
    for y in 0..h {
        for x in 0..w {
            let sx = ((x as f32 + 0.5) / scale - 0.5).max(0.0);
            let sy = ((y as f32 + 0.5) / scale - 0.5).max(0.0);
            let (x0, y0) = (sx.floor() as u32, sy.floor() as u32);
            let (fx, fy) = (sx - x0 as f32, sy - y0 as f32);
            let i = (((y + oy) * size + x + ox) * 4) as usize;
            for c in 0..4 {
                let top = at(x0, y0, c) * (1.0 - fx) + at(x0 + 1, y0, c) * fx;
                let bottom = at(x0, y0 + 1, c) * (1.0 - fx) + at(x0 + 1, y0 + 1, c) * fx;
                out[i + c] = (top * (1.0 - fy) + bottom * fy).round() as u8;
            }
        }
    }
    Rgba { width: size, height: size, pixels: out }
}

/// Recolours the near-grey pixels (UI art: white icons, grey panels and
/// outlines) towards `tint` (0xRRGGBB), keeping their brightness and
/// transparency. Colourful pixels (pictures, coloured badges) stay as they
/// are. Returns whether any pixel changed.
pub fn tint(image: &mut Rgba, tint: u32) -> bool {
    let ratio = tint_ratio(rgb_f32(tint));
    let mut changed = false;
    for px in image.pixels.as_chunks_mut::<4>().0 {
        changed |= tint_pixel(px, ratio);
    }
    changed
}

/// Which way a gradient runs across each icon.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Direction {
    /// Left to right.
    #[default]
    Across,
    /// Top to bottom.
    Down,
    /// Top left to bottom right.
    Diagonal,
}

/// Like [`tint`], with a gradient from `from` to `to` instead of one
/// colour. The gradient runs across each separate shape in the image (each
/// icon of a sprite sheet gets the whole gradient), not across the sheet.
pub fn tint_gradient(image: &mut Rgba, from: u32, to: u32, direction: Direction) -> bool {
    recolour_shapes(image, from, to, direction, |px, colour| tint_pixel(px, tint_ratio(colour)))
}

/// Like [`tint_gradient`] (`from == to` for one colour), for Feline's own
/// art (the Feline Theme's icons): besides the near-grey pixels, its violet
/// ones (fills, details) take the tint's hue, keeping their own lightness,
/// so the whole icon follows the tint instead of staying purple.
pub fn tint_theme(image: &mut Rgba, from: u32, to: u32, direction: Direction) -> bool {
    recolour_shapes(image, from, to, direction, theme_pixel)
}

/// Runs `pixel` on every non-transparent pixel with its colour along the
/// gradient from `from` to `to` across the pixel's shape.
fn recolour_shapes(
    image: &mut Rgba,
    from: u32,
    to: u32,
    direction: Direction,
    pixel: impl Fn(&mut [u8; 4], [f32; 3]) -> bool,
) -> bool {
    let (w, h) = (image.width as usize, image.height as usize);
    if image.pixels.len() < w * h * 4 {
        return false;
    }
    let (from, to) = (rgb_f32(from), rgb_f32(to));
    let solid = from == to;
    let (labels, boxes) = if solid { (Vec::new(), Vec::new()) } else { shapes(image) };
    let mut changed = false;
    for (i, px) in image.pixels.as_chunks_mut::<4>().0.iter_mut().enumerate().take(w * h) {
        if solid {
            changed |= pixel(px, from);
            continue;
        }
        let Some(label) = labels[i].checked_sub(1) else { continue };
        let [x0, y0, x1, y1] = boxes[label as usize];
        let (x, y) = ((i % w) as f32, (i / w) as f32);
        let along = |v: f32, lo: u32, hi: u32| {
            if hi > lo { (v - lo as f32) / (hi - lo) as f32 } else { 0.5 }
        };
        let t = match direction {
            Direction::Across => along(x, x0, x1),
            Direction::Down => along(y, y0, y1),
            Direction::Diagonal => (along(x, x0, x1) + along(y, y0, y1)) / 2.0,
        };
        let colour = [0, 1, 2].map(|c| from[c] + (to[c] - from[c]) * t);
        changed |= pixel(px, colour);
    }
    changed
}

/// Feline's violet (`#9D8CFF`, the theme's fills) sits at this hue.
const FELINE_HUE: f32 = 249.0;

/// One pixel of Feline's art towards `colour`: near-grey as [`tint_pixel`],
/// violet ones to `colour`'s hue and saturation at their own lightness.
fn theme_pixel(px: &mut [u8; 4], colour: [f32; 3]) -> bool {
    if tint_pixel(px, tint_ratio(colour)) || px[3] == 0 {
        return true;
    }
    let [h, s, l] = hsl([px[0], px[1], px[2]].map(|c| c as f32));
    let off = ((h - FELINE_HUE).rem_euclid(360.0)).min((FELINE_HUE - h).rem_euclid(360.0));
    if s < 0.1 || off > 45.0 {
        return false; // not Feline's violet (or already done as grey)
    }
    let [th, ts, _] = hsl(colour);
    let new = from_hsl(th, (s * ts).min(1.0), l).map(|c| c.round().clamp(0.0, 255.0) as u8);
    if new == [px[0], px[1], px[2]] {
        return false;
    }
    px[..3].copy_from_slice(&new);
    true
}

/// RGB (0–255) → hue (degrees), saturation and lightness (0–1).
fn hsl(rgb: [f32; 3]) -> [f32; 3] {
    let [r, g, b] = rgb.map(|c| c / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    let d = max - min;
    if d < 1e-6 {
        return [0.0, 0.0, l];
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs()).max(1e-6);
    let h = if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    [h, s.min(1.0), l]
}

/// Hue (degrees), saturation and lightness (0–1) → RGB (0–255).
fn from_hsl(h: f32, s: f32, l: f32) -> [f32; 3] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match (h.rem_euclid(360.0) / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [r, g, b].map(|v| (v + m) * 255.0)
}

fn rgb_f32(rgb: u32) -> [f32; 3] {
    [(rgb >> 16) & 0xFF, (rgb >> 8) & 0xFF, rgb & 0xFF].map(|c| c as f32)
}

/// A colour as a share of its brightest channel.
fn tint_ratio(rgb: [f32; 3]) -> [f32; 3] {
    let peak = rgb.iter().cloned().fold(1.0, f32::max);
    rgb.map(|c| c / peak)
}

/// Recolours one pixel if it's near-grey. True if it changed.
fn tint_pixel(px: &mut [u8; 4], ratio: [f32; 3]) -> bool {
    if px[3] == 0 {
        return false;
    }
    let (r, g, b) = (px[0] as f32, px[1] as f32, px[2] as f32);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    if max < 1.0 || (max - min) / max > 0.12 {
        return false; // black stays black; colourful stays colourful
    }
    let light = 0.299 * r + 0.587 * g + 0.114 * b;
    let new = ratio.map(|c| (light * c).round().clamp(0.0, 255.0) as u8);
    if new == [px[0], px[1], px[2]] {
        return false;
    }
    px[..3].copy_from_slice(&new);
    true
}

/// The sizes of icon [`pick_icons`] picks, and of a cell in a strip.
const SMALLEST: u32 = 24;
const LARGEST: u32 = 72;
const CELL: u32 = LARGEST + 16;

/// Up to `count` small grey and white icons picked out of `sheets` (sprite
/// sheets), side by side in one strip. `None` if none were found.
pub fn icon_strip(sheets: impl IntoIterator<Item = Rgba>, count: usize) -> Option<Rgba> {
    strip_of(&pick_icons(sheets, count))
}

/// Up to `count` small grey and white icons cut out of `sheets`.
pub fn pick_icons(sheets: impl IntoIterator<Item = Rgba>, count: usize) -> Vec<Rgba> {
    let mut icons: Vec<Rgba> = Vec::new();
    'sheets: for sheet in sheets {
        let w = sheet.width as usize;
        if sheet.pixels.len() < w * sheet.height as usize * 4 {
            continue;
        }
        let (labels, boxes) = shapes(&sheet);
        let mut counts = vec![(0usize, 0usize); boxes.len()]; // (solid, light grey)
        for (i, &label) in labels.iter().enumerate() {
            let Some(l) = label.checked_sub(1) else { continue };
            let px = &sheet.pixels[i * 4..i * 4 + 4];
            let (max, min) = (px[0].max(px[1]).max(px[2]), px[0].min(px[1]).min(px[2]));
            counts[l as usize].0 += 1;
            if max > 150 && f32::from(max - min) / f32::from(max) <= 0.12 {
                counts[l as usize].1 += 1;
            }
        }
        for (l, b) in boxes.iter().enumerate() {
            let (bw, bh) = (b[2] - b[0] + 1, b[3] - b[1] + 1);
            let (solid, light) = counts[l];
            let icon_sized = (SMALLEST..=LARGEST).contains(&bw) && (SMALLEST..=LARGEST).contains(&bh);
            // An icon: not a thin line or a filled square, and mostly
            // light grey/white (what a tint changes).
            if icon_sized
                && bw.max(bh) < bw.min(bh) * 2
                && solid * 100 < (bw * bh) as usize * 80
                && solid * 100 > (bw * bh) as usize * 12
                && light * 10 > solid * 7
            {
                let mut icon = Rgba { width: bw, height: bh, pixels: Vec::with_capacity((bw * bh * 4) as usize) };
                for y in b[1]..=b[3] {
                    let row = ((y * sheet.width + b[0]) * 4) as usize;
                    icon.pixels.extend_from_slice(&sheet.pixels[row..row + (bw * 4) as usize]);
                }
                icons.push(icon);
                if icons.len() == count {
                    break 'sheets;
                }
            }
        }
    }
    icons
}

/// `icons` side by side, each centred in its own cell (larger ones are left
/// out). `None` if there are none.
pub fn strip_of(icons: &[Rgba]) -> Option<Rgba> {
    let icons: Vec<&Rgba> = icons.iter().filter(|i| i.width <= CELL && i.height <= CELL).collect();
    if icons.is_empty() {
        return None;
    }
    let mut strip = Rgba {
        width: CELL * icons.len() as u32,
        height: CELL,
        pixels: vec![0; (CELL * icons.len() as u32 * CELL * 4) as usize],
    };
    for (n, icon) in icons.iter().enumerate() {
        let (ox, oy) = (n as u32 * CELL + (CELL - icon.width) / 2, (CELL - icon.height) / 2);
        for y in 0..icon.height {
            let from = (y * icon.width * 4) as usize;
            let to = (((oy + y) * strip.width + ox) * 4) as usize;
            strip.pixels[to..to + (icon.width * 4) as usize]
                .copy_from_slice(&icon.pixels[from..from + (icon.width * 4) as usize]);
        }
    }
    Some(strip)
}

/// The separate shapes of an image: groups of touching non-transparent
/// pixels. Per pixel, its shape's number + 1 (0 = transparent); per shape,
/// its bounding box `[x0, y0, x1, y1]`.
fn shapes(image: &Rgba) -> (Vec<u32>, Vec<[u32; 4]>) {
    let (w, h) = (image.width as usize, image.height as usize);
    let solid = |i: usize| image.pixels[i * 4 + 3] != 0;
    let mut labels = vec![0u32; w * h];
    let mut boxes: Vec<[u32; 4]> = Vec::new();
    let mut stack = Vec::new();
    for start in 0..w * h {
        if labels[start] != 0 || !solid(start) {
            continue;
        }
        boxes.push([u32::MAX, u32::MAX, 0, 0]);
        let label = boxes.len() as u32;
        let bbox = boxes.last_mut().expect("just pushed");
        labels[start] = label;
        stack.push(start);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            *bbox = [bbox[0].min(x as u32), bbox[1].min(y as u32), bbox[2].max(x as u32), bbox[3].max(y as u32)];
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                    if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                        continue;
                    }
                    let n = ny as usize * w + nx as usize;
                    if labels[n] == 0 && solid(n) {
                        labels[n] = label;
                        stack.push(n);
                    }
                }
            }
        }
    }
    (labels, boxes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_violet_follows_the_tint() {
        // Feline's violet fill, lavender outline, deep detail, and a red dot.
        let mut image = Rgba {
            width: 4,
            height: 1,
            pixels: vec![157, 140, 255, 255, 236, 232, 255, 255, 36, 26, 64, 255, 220, 30, 30, 255],
        };
        assert!(tint_theme(&mut image, 0x3fbfc4, 0x3fbfc4, Direction::Across));
        let px: Vec<[u8; 4]> = image.pixels.as_chunks::<4>().0.to_vec();
        for p in &px[..3] {
            let [h, ..] = hsl([p[0], p[1], p[2]].map(|c| c as f32));
            assert!((h - 182.0).abs() < 12.0, "{p:?} hue {h}");
        }
        assert_eq!(px[3], [220, 30, 30, 255], "other colours stay");
        let back = from_hsl(249.0, 1.0, 0.77).map(|c| c.round() as u8);
        assert_eq!(hsl(back.map(|c| c as f32))[0].round(), 249.0);
    }

    #[test]
    fn round_trips_and_tints() {
        let mut img = Rgba {
            width: 3,
            height: 2,
            pixels: vec![
                255, 255, 255, 255, /**/ 128, 128, 128, 200, /**/ 255, 0, 0, 255, //
                0, 0, 0, 255, /**/ 10, 20, 30, 0, /**/ 250, 250, 250, 255,
            ],
        };
        let back = decode(&encode(&img)).unwrap();
        assert_eq!(back, img);
        assert!(tint(&mut img, 0xCDB8FF));
        assert_eq!(&img.pixels[0..4], &[205, 184, 255, 255], "white turns lavender");
        assert_eq!(&img.pixels[8..12], &[255, 0, 0, 255], "red stays red");
        assert_eq!(&img.pixels[12..16], &[0, 0, 0, 255], "black stays black");
        assert_eq!(img.pixels[7], 200, "transparency kept");
    }

    #[test]
    fn gradients_run_across_each_icon() {
        // Two white 2×1 icons with a transparent gap: each gets the whole
        // gradient (red on its left, blue on its right).
        let white = [255, 255, 255, 255];
        let clear = [0, 0, 0, 0];
        let mut img = Rgba {
            width: 5,
            height: 1,
            pixels: [white, white, clear, white, white].concat(),
        };
        assert!(tint_gradient(&mut img, 0xFF0000, 0x0000FF, Direction::Across));
        let px = |i: usize| &img.pixels[i * 4..i * 4 + 3];
        for left in [0, 3] {
            assert!(px(left)[0] > 200 && px(left)[2] < 10, "{:?}", px(left));
            assert!(px(left + 1)[2] > 200 && px(left + 1)[0] < 10, "{:?}", px(left + 1));
        }
        assert_eq!(&img.pixels[8..12], &clear, "the gap stays clear");
    }

    #[test]
    fn reads_roblox_images_if_installed() {
        // Every UI image of a real Roblox install decodes (palette, grey +
        // alpha, RGB, RGBA…), and writes back the same pixels.
        let Some(local) = std::env::var_os("LOCALAPPDATA") else {
            return;
        };
        let versions = std::path::Path::new(&local).join("Roblox").join("Versions");
        let Some(root) = std::fs::read_dir(versions)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .find(|p| p.join("RobloxPlayerBeta.exe").is_file())
        else {
            return;
        };
        let mut count = 0;
        let mut stack: Vec<std::path::PathBuf> = crate::mods::TINT_FOLDERS.iter().map(|f| root.join(f)).collect();
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "png") {
                    // Every 10th image is plenty (debug builds are slow).
                    count += 1;
                    if count % 10 != 0 {
                        continue;
                    }
                    let data = std::fs::read(&path).unwrap();
                    let img = decode(&data).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                    assert_eq!(decode(&encode(&img)).unwrap(), img, "{}", path.display());
                }
            }
        }
        assert!(count > 1000, "{count}");
    }
}

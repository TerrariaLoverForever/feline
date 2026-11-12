// SPDX-License-Identifier: GPL-3.0-or-later
//! Animated cursors: reading `.gif`, Windows `.ani` (and still `.cur`/`.ico`
//! and `.png`) files into frames.
//!
//! Roblox's cursor is a still picture, so an animated one is drawn by Feline
//! instead: Roblox gets a see-through cursor while Feline started the game,
//! and a small click-through window of Feline's own (like the desk cat)
//! follows the pointer with the animation. Nothing is drawn into the game.
//! See `feline --with-roblox` and [`crate::mods::animated_cursor`].

use crate::png::Rgba;

/// Bigger pictures aren't cursors (and would only waste memory).
const MAX_SIDE: u32 = 256;
const MAX_FRAMES: usize = 400;

/// One picture of the animation and how long it stays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub image: Rgba,
    pub ms: u32,
}

/// A cursor: frames of the same size, and where the pointer's tip is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Animation {
    pub frames: Vec<Frame>,
    pub width: u32,
    pub height: u32,
    /// The point of the picture that sits at the pointer's position.
    pub hotspot: (u32, u32),
}

impl Animation {
    pub fn is_animated(&self) -> bool {
        self.frames.len() > 1
    }

    /// The animation `factor` times bigger (nearest neighbour, for crisp
    /// pixel art on high-DPI screens).
    pub fn scaled(&self, factor: u32) -> Self {
        if factor <= 1 {
            return self.clone();
        }
        let frames = self
            .frames
            .iter()
            .map(|f| Frame { image: scale_up(&f.image, factor), ms: f.ms })
            .collect();
        Self {
            frames,
            width: self.width * factor,
            height: self.height * factor,
            hotspot: (self.hotspot.0 * factor, self.hotspot.1 * factor),
        }
    }

    /// The first frame on a `size` × `size` canvas with the hotspot in the
    /// middle (how Roblox places its cursor pictures), shrunk if needed.
    pub fn still_for_roblox(&self, size: u32) -> Rgba {
        let half = size / 2;
        let first = &self.frames[0].image;
        // Shrink so the picture fits around the hotspot.
        let (hx, hy) = self.hotspot;
        let reach = [hx, hy, self.width - hx, self.height - hy].into_iter().max().unwrap_or(1).max(1);
        let scale = (half as f32 / reach as f32).min(1.0);
        let mut out = Rgba { width: size, height: size, pixels: vec![0; (size * size * 4) as usize] };
        for y in 0..size {
            for x in 0..size {
                let sx = ((x as f32 - half as f32) / scale + hx as f32).floor();
                let sy = ((y as f32 - half as f32) / scale + hy as f32).floor();
                if sx < 0.0 || sy < 0.0 || sx >= self.width as f32 || sy >= self.height as f32 {
                    continue;
                }
                let from = ((sy as u32 * first.width + sx as u32) * 4) as usize;
                let to = ((y * size + x) * 4) as usize;
                out.pixels[to..to + 4].copy_from_slice(&first.pixels[from..from + 4]);
            }
        }
        out
    }
}

/// The classic arrow, drawn by Feline (`#` outline, `o` fill), tip at the
/// top left. The shape every desktop pointer has had since the 90s.
const ARROW: &[&str] = &[
    "#",
    "##",
    "#o#",
    "#oo#",
    "#ooo#",
    "#oooo#",
    "#ooooo#",
    "#oooooo#",
    "#ooooooo#",
    "#oooooooo#",
    "#ooooo#####",
    "#oo#oo#",
    "#o# #oo#",
    "##  #oo#",
    "#    #oo#",
    "     #oo#",
    "      ##",
];

/// A classic arrow cursor: white with a black edge, or black with a white
/// edge, twice the pixel size (crisp, like old cursors).
pub fn classic(white: bool) -> Animation {
    let (edge, fill) = if white { ([0, 0, 0, 255], [255, 255, 255, 255]) } else { ([255, 255, 255, 255], [0, 0, 0, 255]) };
    let (w, h) = (ARROW.iter().map(|r| r.len()).max().unwrap_or(1) as u32, ARROW.len() as u32);
    let mut image = Rgba { width: w, height: h, pixels: vec![0; (w * h * 4) as usize] };
    for (y, row) in ARROW.iter().enumerate() {
        for (x, c) in row.chars().enumerate() {
            let px = match c {
                '#' => edge,
                'o' => fill,
                _ => continue,
            };
            let i = (y * w as usize + x) * 4;
            image.pixels[i..i + 4].copy_from_slice(&px);
        }
    }
    Animation { width: w, height: h, frames: vec![Frame { image, ms: 100 }], hotspot: (0, 0) }.scaled(2)
}

fn scale_up(image: &Rgba, factor: u32) -> Rgba {
    let (w, h) = (image.width * factor, image.height * factor);
    let mut pixels = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let i = (((y / factor) * image.width + x / factor) * 4) as usize;
            pixels.extend_from_slice(&image.pixels[i..i + 4]);
        }
    }
    Rgba { width: w, height: h, pixels }
}

/// Reads a cursor file: `.gif`, `.ani`, `.cur`/`.ico` or `.png`.
pub fn load(data: &[u8]) -> Result<Animation, String> {
    let animation = if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        gif(data)?
    } else if data.starts_with(b"RIFF") && data.get(8..12) == Some(b"ACON") {
        ani(data)?
    } else if data.starts_with(&[0, 0, 1, 0]) || data.starts_with(&[0, 0, 2, 0]) {
        let (image, hotspot) = ico(data)?;
        Animation { width: image.width, height: image.height, frames: vec![Frame { image, ms: 100 }], hotspot }
    } else if data.starts_with(b"\x89PNG") {
        let image = crate::png::decode(data)?;
        Animation { width: image.width, height: image.height, frames: vec![Frame { image, ms: 100 }], hotspot: (0, 0) }
    } else {
        return Err("that isn't a .gif, .ani, .cur or .png file".to_owned());
    };
    if animation.frames.is_empty() || animation.width == 0 || animation.height == 0 {
        return Err("there are no pictures in it".to_owned());
    }
    if animation.width > MAX_SIDE || animation.height > MAX_SIDE {
        return Err(format!("it's {}×{}; a cursor can be at most {MAX_SIDE}×{MAX_SIDE}", animation.width, animation.height));
    }
    Ok(animation)
}

// ---------------------------------------------------------------------------
// GIF
// ---------------------------------------------------------------------------

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn byte(&mut self) -> Result<u8, String> {
        let b = *self.data.get(self.at).ok_or("the file ends too soon")?;
        self.at += 1;
        Ok(b)
    }
    fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from(self.byte()?) | (u16::from(self.byte()?) << 8))
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let out = self.data.get(self.at..self.at + n).ok_or("the file ends too soon")?;
        self.at += n;
        Ok(out)
    }
    /// GIF's data sub-blocks (length byte + data, until a 0), joined.
    fn blocks(&mut self) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        loop {
            let len = self.byte()? as usize;
            if len == 0 {
                return Ok(out);
            }
            out.extend_from_slice(self.take(len)?);
        }
    }
}

fn gif(data: &[u8]) -> Result<Animation, String> {
    let mut r = Reader { data, at: 6 };
    let (width, height) = (u32::from(r.u16()?), u32::from(r.u16()?));
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return Err(format!("it's {width}×{height}; a cursor can be at most {MAX_SIDE}×{MAX_SIDE}"));
    }
    let flags = r.byte()?;
    let _background = r.byte()?;
    let _aspect = r.byte()?;
    let global = if flags & 0x80 != 0 { r.take(3 << ((flags & 7) + 1))?.to_vec() } else { Vec::new() };

    let mut canvas = vec![0u8; (width * height * 4) as usize];
    let mut frames = Vec::new();
    let (mut delay, mut disposal, mut transparent) = (10u32, 0u8, None::<u8>);
    loop {
        match r.byte()? {
            0x21 => {
                let label = r.byte()?;
                let body = r.blocks()?;
                if label == 0xF9 && body.len() >= 4 {
                    disposal = (body[0] >> 2) & 7;
                    delay = u32::from(body[1]) | (u32::from(body[2]) << 8);
                    transparent = (body[0] & 1 != 0).then_some(body[3]);
                }
            }
            0x2C => {
                let (left, top) = (u32::from(r.u16()?), u32::from(r.u16()?));
                let (w, h) = (u32::from(r.u16()?), u32::from(r.u16()?));
                let flags = r.byte()?;
                let local = if flags & 0x80 != 0 { Some(r.take(3 << ((flags & 7) + 1))?.to_vec()) } else { None };
                let palette = local.as_deref().unwrap_or(&global);
                let min_code = r.byte()?;
                let indices = lzw(min_code, &r.blocks()?, (w * h) as usize)?;
                let rows: Vec<u32> = if flags & 0x40 != 0 {
                    (0..h).step_by(8).chain((4..h).step_by(8)).chain((2..h).step_by(4)).chain((1..h).step_by(2)).collect()
                } else {
                    (0..h).collect()
                };
                let before = (disposal == 3).then(|| canvas.clone());
                for (n, &y) in rows.iter().enumerate() {
                    for x in 0..w {
                        let Some(&index) = indices.get(n * w as usize + x as usize) else { continue };
                        let (cx, cy) = (left + x, top + y);
                        if cx >= width || cy >= height || Some(index) == transparent {
                            continue;
                        }
                        let p = index as usize * 3;
                        let Some(rgb) = palette.get(p..p + 3) else { continue };
                        let i = ((cy * width + cx) * 4) as usize;
                        canvas[i..i + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
                    }
                }
                // GIF players treat very short delays as 100 ms.
                let ms = if delay < 2 { 100 } else { delay * 10 };
                frames.push(Frame { image: Rgba { width, height, pixels: canvas.clone() }, ms });
                if frames.len() >= MAX_FRAMES {
                    break;
                }
                match disposal {
                    2 => {
                        for y in top..(top + h).min(height) {
                            for x in left..(left + w).min(width) {
                                let i = ((y * width + x) * 4) as usize;
                                canvas[i..i + 4].fill(0);
                            }
                        }
                    }
                    3 => canvas = before.unwrap_or(canvas),
                    _ => {}
                }
                (delay, disposal, transparent) = (10, 0, None);
            }
            0x3B => break,
            _ => return Err("the GIF is damaged".to_owned()),
        }
    }
    Ok(Animation { frames, width, height, hotspot: (0, 0) })
}

/// GIF's LZW: codes of growing width (LSB first) → colour indices.
fn lzw(min_code: u8, data: &[u8], expected: usize) -> Result<Vec<u8>, String> {
    if !(1..=11).contains(&min_code) {
        return Err("the GIF is damaged".to_owned());
    }
    let clear = 1u16 << min_code;
    let end = clear + 1;
    let mut prefix = [0u16; 4096];
    let mut suffix = [0u8; 4096];
    let mut length = [0u16; 4096];
    for i in 0..clear {
        suffix[i as usize] = i as u8;
        length[i as usize] = 1;
    }
    let mut out = Vec::with_capacity(expected);
    let (mut size, mut next, mut prev) = (min_code + 1, end + 1, None::<u16>);
    let (mut bits, mut have, mut at) = (0u32, 0u8, 0usize);
    let first_of = |mut code: u16, prefix: &[u16; 4096], length: &[u16; 4096]| {
        while length[code as usize] > 1 {
            code = prefix[code as usize];
        }
        code as u8
    };
    loop {
        while have < size {
            let Some(&b) = data.get(at) else { return Ok(out) };
            bits |= u32::from(b) << have;
            have += 8;
            at += 1;
        }
        let code = (bits & ((1 << size) - 1)) as u16;
        bits >>= size;
        have -= size;
        if code == clear {
            (size, next, prev) = (min_code + 1, end + 1, None);
            continue;
        }
        if code == end {
            return Ok(out);
        }
        let known = code < next && (code < clear || length[code as usize] > 0);
        match prev {
            Some(p) if next < 4096 => {
                let first = if known { first_of(code, &prefix, &length) } else if code == next {
                    first_of(p, &prefix, &length)
                } else {
                    return Err("the GIF is damaged".to_owned());
                };
                prefix[next as usize] = p;
                suffix[next as usize] = first;
                length[next as usize] = length[p as usize] + 1;
                next += 1;
            }
            None if !known => return Err("the GIF is damaged".to_owned()),
            _ => {}
        }
        // Write the code's bytes (walking back from its last one).
        let len = length[code as usize] as usize;
        let start = out.len();
        out.resize(start + len, 0);
        let mut c = code;
        for i in (0..len).rev() {
            out[start + i] = suffix[c as usize];
            c = prefix[c as usize];
        }
        prev = Some(code);
        if next == (1 << size) && size < 12 {
            size += 1;
        }
        if out.len() >= expected {
            out.truncate(expected);
            return Ok(out);
        }
    }
}

// ---------------------------------------------------------------------------
// Windows .ani / .cur / .ico
// ---------------------------------------------------------------------------

fn le16(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from(u16::from_le_bytes(d.get(at..at + 2)?.try_into().ok()?)))
}
fn le32(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(at..at + 4)?.try_into().ok()?))
}

/// The chunks of a RIFF list: (id, body).
fn chunks(mut d: &[u8]) -> Vec<(&[u8], &[u8])> {
    let mut out = Vec::new();
    while d.len() >= 8 {
        let id = &d[..4];
        let Some(len) = le32(d, 4).map(|l| l as usize) else { break };
        let Some(body) = d.get(8..8 + len) else { break };
        out.push((id, body));
        d = d.get(8 + len + (len & 1)..).unwrap_or_default();
    }
    out
}

fn ani(data: &[u8]) -> Result<Animation, String> {
    let damaged = || "the .ani file is damaged".to_owned();
    let mut icons: Vec<(Rgba, (u32, u32))> = Vec::new();
    let (mut rate, mut seq, mut default_rate) = (Vec::new(), Vec::new(), 6u32);
    for (id, body) in chunks(data.get(12..).ok_or_else(damaged)?) {
        match id {
            b"anih" => default_rate = le32(body, 28).filter(|&r| r > 0).unwrap_or(6),
            b"rate" => rate = body.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect(),
            b"seq " => seq = body.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect(),
            b"LIST" if body.starts_with(b"fram") => {
                for (id, icon) in chunks(&body[4..]) {
                    if id == b"icon" && icons.len() < MAX_FRAMES {
                        icons.push(ico(icon)?);
                    }
                }
            }
            _ => {}
        }
    }
    if icons.is_empty() {
        return Err(damaged());
    }
    let steps: Vec<usize> = if seq.is_empty() { (0..icons.len()).collect() } else { seq.iter().map(|&s| s as usize).collect() };
    let width = icons.iter().map(|(i, _)| i.width).max().unwrap_or(1);
    let height = icons.iter().map(|(i, _)| i.height).max().unwrap_or(1);
    let hotspot = icons[0].1;
    let mut frames = Vec::new();
    for (n, &index) in steps.iter().enumerate().take(MAX_FRAMES) {
        let Some((image, _)) = icons.get(index) else { continue };
        // Jiffies: 1/60 s.
        let jiffies = rate.get(n).copied().filter(|&r| r > 0).unwrap_or(default_rate);
        frames.push(Frame { image: pad(image, width, height), ms: jiffies * 1000 / 60 });
    }
    Ok(Animation { frames, width, height, hotspot })
}

fn pad(image: &Rgba, width: u32, height: u32) -> Rgba {
    if image.width == width && image.height == height {
        return image.clone();
    }
    let mut out = Rgba { width, height, pixels: vec![0; (width * height * 4) as usize] };
    for y in 0..image.height.min(height) {
        let from = (y * image.width * 4) as usize;
        let to = (y * width * 4) as usize;
        let n = (image.width.min(width) * 4) as usize;
        out.pixels[to..to + n].copy_from_slice(&image.pixels[from..from + n]);
    }
    out
}

/// The biggest picture in an `.ico`/`.cur` file, and its hotspot (cursors).
fn ico(data: &[u8]) -> Result<(Rgba, (u32, u32)), String> {
    let damaged = || "the cursor file is damaged".to_owned();
    let cursor = le16(data, 2) == Some(2);
    let count = le16(data, 4).ok_or_else(damaged)? as usize;
    let mut best: Option<(u32, usize)> = None;
    for n in 0..count.min(64) {
        let e = 6 + n * 16;
        let w = match data.get(e) { Some(0) => 256, Some(&w) => u32::from(w), None => break };
        if best.is_none_or(|(bw, _)| w > bw) {
            best = Some((w, e));
        }
    }
    let (_, e) = best.ok_or_else(damaged)?;
    let size = le32(data, e + 8).ok_or_else(damaged)? as usize;
    let offset = le32(data, e + 12).ok_or_else(damaged)? as usize;
    let body = data.get(offset..offset + size).ok_or_else(damaged)?;
    let image = if body.starts_with(b"\x89PNG") { crate::png::decode(body)? } else { dib(body).ok_or_else(damaged)? };
    let hotspot = if cursor {
        (le16(data, e + 4).unwrap_or(0).min(image.width.saturating_sub(1)), le16(data, e + 6).unwrap_or(0).min(image.height.saturating_sub(1)))
    } else {
        (0, 0)
    };
    Ok((image, hotspot))
}

/// An icon's bitmap: a BITMAPINFOHEADER, the colour pixels (bottom row
/// first) and a 1-bit transparency mask.
fn dib(d: &[u8]) -> Option<Rgba> {
    let header = le32(d, 0)? as usize;
    let width = le32(d, 4)?;
    let height = le32(d, 8)? / 2; // colour + mask
    let bits = le16(d, 14)?;
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE || le32(d, 16)? != 0 {
        return None;
    }
    let colours = match le32(d, 32)? {
        0 if bits <= 8 => 1 << bits,
        n => n,
    } as usize;
    let palette = if bits <= 8 { d.get(header..header + colours * 4)? } else { &[][..] };
    let pixels_at = header + palette.len();
    let stride = (width * bits).div_ceil(32) as usize * 4;
    let mask_at = pixels_at + stride * height as usize;
    let mask_stride = width.div_ceil(32) as usize * 4;
    let mut out = vec![0u8; (width * height * 4) as usize];
    let mut any_alpha = false;
    for y in 0..height as usize {
        let row = d.get(pixels_at + (height as usize - 1 - y) * stride..)?;
        for x in 0..width as usize {
            let px: [u8; 4] = match bits {
                32 => {
                    let p = row.get(x * 4..x * 4 + 4)?;
                    any_alpha |= p[3] != 0;
                    [p[2], p[1], p[0], p[3]]
                }
                24 => {
                    let p = row.get(x * 3..x * 3 + 3)?;
                    [p[2], p[1], p[0], 255]
                }
                1 | 4 | 8 => {
                    let bit = x * bits as usize;
                    let byte = *row.get(bit / 8)?;
                    let index = (byte >> (8 - bits as usize - bit % 8)) & ((1 << bits) - 1) as u8;
                    let p = palette.get(index as usize * 4..index as usize * 4 + 4)?;
                    [p[2], p[1], p[0], 255]
                }
                _ => return None,
            };
            let i = (y * width as usize + x) * 4;
            out[i..i + 4].copy_from_slice(&px);
        }
    }
    // No alpha of its own: the mask says what's see-through.
    if !any_alpha {
        for y in 0..height as usize {
            let row = d.get(mask_at + (height as usize - 1 - y) * mask_stride..);
            for x in 0..width as usize {
                let clear = row.and_then(|r| r.get(x / 8)).is_some_and(|b| b & (0x80 >> (x % 8)) != 0);
                let i = (y * width as usize + x) * 4;
                if clear {
                    // A "clear" pixel with colour is Windows' invert; show it dark.
                    let inverted = out[i..i + 3].iter().any(|&c| c != 0);
                    out[i..i + 4].copy_from_slice(if inverted { &[0, 0, 0, 160] } else { &[0, 0, 0, 0] });
                } else {
                    out[i + 3] = 255;
                }
            }
        }
    }
    Some(Rgba { width, height, pixels: out })
}

/// A small animated GIF for other modules' tests.
#[cfg(test)]
pub(crate) fn tests_gif() -> Vec<u8> {
    tests::tiny_gif()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2×2, two-frame GIF: red then blue (built by hand: LZW codes for a
    /// 2-colour palette with min code size 2).
    pub(crate) fn tiny_gif() -> Vec<u8> {
        let mut g = b"GIF89a".to_vec();
        g.extend_from_slice(&[2, 0, 2, 0, 0x80, 0, 0]); // 2×2, global table of 2
        g.extend_from_slice(&[255, 0, 0, 0, 0, 255]); // red, blue
        for (index, delay) in [(0u8, 5u8), (1, 20)] {
            g.extend_from_slice(&[0x21, 0xF9, 4, 0, delay, 0, 0, 0]);
            g.extend_from_slice(&[0x2C, 0, 0, 0, 0, 2, 0, 2, 0, 0]);
            // min code 2: clear(4), 4× index, end(5), 3-bit codes.
            let codes = [4u16, index as u16, index as u16, index as u16, index as u16, 5];
            let (mut bits, mut n, mut bytes) = (0u32, 0, Vec::new());
            let mut size = 3;
            let mut next = 6;
            for (i, &c) in codes.iter().enumerate() {
                bits |= u32::from(c) << n;
                n += size;
                while n >= 8 {
                    bytes.push(bits as u8);
                    bits >>= 8;
                    n -= 8;
                }
                if i >= 2 && c != 5 {
                    next += 1;
                    if next == 1 << size {
                        size += 1;
                    }
                }
            }
            if n > 0 {
                bytes.push(bits as u8);
            }
            g.push(2);
            g.push(bytes.len() as u8);
            g.extend_from_slice(&bytes);
            g.push(0);
        }
        g.push(0x3B);
        g
    }

    #[test]
    fn gifs_become_frames() {
        let a = load(&tiny_gif()).unwrap();
        assert_eq!((a.width, a.height, a.frames.len()), (2, 2, 2));
        assert_eq!(&a.frames[0].image.pixels[..4], &[255, 0, 0, 255]);
        assert_eq!(&a.frames[1].image.pixels[12..16], &[0, 0, 255, 255]);
        assert_eq!((a.frames[0].ms, a.frames[1].ms), (50, 200));
        assert!(a.is_animated());
    }

    /// A .cur with one 2×2 32-bit picture, hotspot (1, 0).
    fn tiny_cur() -> Vec<u8> {
        let mut bmp = Vec::new();
        for v in [40u32, 2, 4] {
            bmp.extend_from_slice(&v.to_le_bytes());
        }
        bmp.extend_from_slice(&1u16.to_le_bytes());
        bmp.extend_from_slice(&32u16.to_le_bytes());
        bmp.extend_from_slice(&[0; 24]);
        // Bottom row first: green, green / white, white (BGRA).
        bmp.extend_from_slice(&[0, 255, 0, 255, 0, 255, 0, 255, 255, 255, 255, 255, 255, 255, 255, 255]);
        bmp.extend_from_slice(&[0; 8]); // mask
        let mut c = vec![0, 0, 2, 0, 1, 0, 2, 2, 0, 0, 1, 0, 0, 0];
        c.extend_from_slice(&(bmp.len() as u32).to_le_bytes());
        c.extend_from_slice(&22u32.to_le_bytes());
        c.extend_from_slice(&bmp);
        c
    }

    #[test]
    fn ani_files_play_their_sequence() {
        let cur = tiny_cur();
        let mut list = b"fram".to_vec();
        for _ in 0..2 {
            list.extend_from_slice(b"icon");
            list.extend_from_slice(&(cur.len() as u32).to_le_bytes());
            list.extend_from_slice(&cur);
        }
        let mut body = b"ACON".to_vec();
        let mut anih = vec![0u8; 36];
        anih[28..32].copy_from_slice(&6u32.to_le_bytes());
        body.extend_from_slice(b"anih");
        body.extend_from_slice(&36u32.to_le_bytes());
        body.extend_from_slice(&anih);
        body.extend_from_slice(b"seq ");
        body.extend_from_slice(&12u32.to_le_bytes());
        for s in [0u32, 1, 0] {
            body.extend_from_slice(&s.to_le_bytes());
        }
        body.extend_from_slice(b"LIST");
        body.extend_from_slice(&(list.len() as u32).to_le_bytes());
        body.extend_from_slice(&list);
        let mut ani = b"RIFF".to_vec();
        ani.extend_from_slice(&(body.len() as u32).to_le_bytes());
        ani.extend_from_slice(&body);

        let a = load(&ani).unwrap();
        assert_eq!((a.frames.len(), a.hotspot), (3, (1, 0)));
        assert_eq!(a.frames[0].ms, 100);
        // Top row is white, bottom row green.
        assert_eq!(&a.frames[0].image.pixels[..4], &[255, 255, 255, 255]);
        assert_eq!(&a.frames[0].image.pixels[8..12], &[0, 255, 0, 255]);
        let still = a.still_for_roblox(8);
        assert_eq!(&still.pixels[((4 * 8 + 3) * 4) as usize..][..4], &[255, 255, 255, 255], "hotspot in the middle");
        assert_eq!(a.scaled(2).hotspot, (2, 0));
    }

    /// Windows' own animated cursors, where there are any.
    #[test]
    fn reads_windows_cursors_if_there() {
        let Ok(entries) = std::fs::read_dir("C:/Windows/Cursors") else { return };
        let mut read = 0;
        for path in entries.filter_map(Result::ok).map(|e| e.path()) {
            let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase());
            if !matches!(ext.as_deref(), Some("ani" | "cur")) {
                continue;
            }
            let a = load(&std::fs::read(&path).unwrap()).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            assert!(a.frames.iter().all(|f| f.image.pixels.len() == (a.width * a.height * 4) as usize));
            assert!(a.frames[0].image.pixels.chunks(4).any(|p| p[3] > 0), "{} is all clear", path.display());
            read += 1;
        }
        eprintln!("read {read} Windows cursors");
    }

    #[test]
    fn classic_arrows_have_their_tip_in_the_middle_for_roblox() {
        let still = classic(true).still_for_roblox(64);
        let at = |x: u32, y: u32| still.pixels[((y * 64 + x) * 4) as usize..][..4].to_vec();
        assert_eq!(at(32, 32), [0, 0, 0, 255], "the tip");
        assert_eq!(at(35, 40), [255, 255, 255, 255], "white inside");
        assert_eq!(at(20, 20), [0, 0, 0, 0], "clear above-left of the tip");
    }

    #[test]
    fn junk_is_refused() {
        assert!(load(b"hello").is_err());
        assert!(load(b"GIF89a\x01").is_err());
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! Skyboxes: Roblox's default sky, made by Feline. Roblox's sky is six
//! 512 × 512 pictures (`PlatformContent/pc/textures/sky/sky512_*.tex`):
//! DDS files, DXT1-compressed, with smaller copies (mipmaps) down to 1 × 1,
//! the same layout as Roblox's own files.
//!
//! Feline draws a few skies itself (gradients, stars) and turns a picture
//! of yours (a panorama) into one. Games with their own sky keep it; this
//! changes the default sky.

use crate::png::Rgba;

pub const FACES: [&str; 6] = ["bk", "dn", "ft", "lf", "rt", "up"];
const SIZE: u32 = 512;

/// The sky files in a mod, relative paths.
pub fn face_path(face: &str) -> String {
    format!("PlatformContent/pc/textures/sky/sky512_{face}.tex")
}

/// Feline's own skies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    Sunset,
    StarryNight,
    Pastel,
    FelinePurple,
}

impl Preset {
    pub const ALL: [Preset; 4] = [Preset::Sunset, Preset::StarryNight, Preset::Pastel, Preset::FelinePurple];

    pub fn name(self) -> &'static str {
        match self {
            Preset::Sunset => "Sunset",
            Preset::StarryNight => "Starry night",
            Preset::Pastel => "Pastel dawn",
            Preset::FelinePurple => "Feline purple",
        }
    }

    /// (zenith, horizon, ground) colours and whether there are stars.
    fn look(self) -> ([f32; 3], [f32; 3], [f32; 3], bool) {
        let c = |v: u32| [(v >> 16) as f32, ((v >> 8) & 0xFF) as f32, (v & 0xFF) as f32];
        match self {
            Preset::Sunset => (c(0x2B3A67), c(0xFF9E6B), c(0x3B2A3A), false),
            Preset::StarryNight => (c(0x05060F), c(0x1B2346), c(0x0A0B12), true),
            Preset::Pastel => (c(0xA8D8FF), c(0xFFD6E8), c(0xCFC6E8), false),
            Preset::FelinePurple => (c(0x1A1033), c(0x9D8CFF), c(0x24183F), true),
        }
    }

    /// The six faces, in [`FACES`]' order.
    pub fn faces(self) -> [Rgba; 6] {
        let (zenith, horizon, ground, stars) = self.look();
        let side = |seed: u32| {
            let mut img = gradient(zenith, horizon, ground);
            if stars {
                sprinkle_stars(&mut img, seed, 0.62);
            }
            img
        };
        let mut up = flat(zenith);
        if stars {
            sprinkle_stars(&mut up, 99, 1.0);
        }
        [side(1), flat(ground), side(2), side(3), side(4), up]
    }
}

fn flat(c: [f32; 3]) -> Rgba {
    let mut pixels = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for _ in 0..SIZE * SIZE {
        pixels.extend_from_slice(&[c[0] as u8, c[1] as u8, c[2] as u8, 255]);
    }
    Rgba { width: SIZE, height: SIZE, pixels }
}

/// A side of the sky: zenith colour at the top, horizon in the middle,
/// the ground below it.
fn gradient(zenith: [f32; 3], horizon: [f32; 3], ground: [f32; 3]) -> Rgba {
    let mut pixels = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        let t = y as f32 / (SIZE - 1) as f32;
        let c = if t < 0.5 {
            // Ease towards the horizon, brighter close to it.
            let k = (t / 0.5).powf(1.8);
            [0, 1, 2].map(|i| zenith[i] + (horizon[i] - zenith[i]) * k)
        } else {
            let k = ((t - 0.5) / 0.5).powf(0.5).min(1.0);
            [0, 1, 2].map(|i| horizon[i] + (ground[i] - horizon[i]) * k)
        };
        for _ in 0..SIZE {
            pixels.extend_from_slice(&[c[0] as u8, c[1] as u8, c[2] as u8, 255]);
        }
    }
    Rgba { width: SIZE, height: SIZE, pixels }
}

/// Little stars in the top `upto` part of the picture (a simple
/// deterministic pseudo-random scatter, so a sky is always the same).
fn sprinkle_stars(img: &mut Rgba, seed: u32, upto: f32) {
    let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(12345);
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    let limit = (SIZE as f32 * upto) as u32;
    for _ in 0..420 {
        let (x, y) = (next() % SIZE, next() % limit.max(1));
        let bright = 150 + (next() % 106) as u8;
        let big = next() % 9 == 0;
        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)].iter().take(if big { 4 } else { 1 }) {
            let (px, py) = ((x + dx).min(SIZE - 1), (y + dy).min(SIZE - 1));
            let i = ((py * SIZE + px) * 4) as usize;
            img.pixels[i..i + 3].copy_from_slice(&[bright, bright, bright.saturating_add(10)]);
        }
    }
}

/// A sky from a picture: a panorama is cut into four sides (front, right,
/// back, left); the top and ground take its top and bottom rows' colours.
pub fn from_picture(picture: &Rgba) -> [Rgba; 6] {
    // Four sides of equal width (a narrow picture repeats its last column).
    let quarter = (picture.width / 4).max(1);
    let side = |i: u32| {
        let mut part = Rgba { width: quarter, height: picture.height, pixels: Vec::new() };
        for y in 0..picture.height {
            for x in 0..quarter {
                let sx = (i * quarter + x).min(picture.width - 1);
                let from = ((y * picture.width + sx) * 4) as usize;
                part.pixels.extend_from_slice(&picture.pixels[from..from + 4]);
            }
        }
        stretch(&part, SIZE)
    };
    let average_row = |y: u32| {
        let mut sum = [0u64; 3];
        for x in 0..picture.width {
            let i = ((y * picture.width + x) * 4) as usize;
            for (c, total) in sum.iter_mut().enumerate() {
                *total += u64::from(picture.pixels[i + c]);
            }
        }
        sum.map(|s| (s / u64::from(picture.width.max(1))) as f32)
    };
    let top = average_row(0);
    let bottom = average_row(picture.height - 1);
    [side(2), flat(bottom), side(0), side(3), side(1), flat(top)]
}

/// `image` scaled to `size` × `size` (bilinear).
fn stretch(image: &Rgba, size: u32) -> Rgba {
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    let (w, h) = (image.width.max(1), image.height.max(1));
    let at = |x: u32, y: u32, c: usize| f32::from(image.pixels[((y.min(h - 1) * w + x.min(w - 1)) * 4) as usize + c]);
    for y in 0..size {
        let sy = (y as f32 + 0.5) * h as f32 / size as f32 - 0.5;
        let (y0, fy) = (sy.max(0.0).floor() as u32, sy.max(0.0).fract());
        for x in 0..size {
            let sx = (x as f32 + 0.5) * w as f32 / size as f32 - 0.5;
            let (x0, fx) = (sx.max(0.0).floor() as u32, sx.max(0.0).fract());
            for c in 0..3 {
                let top = at(x0, y0, c) * (1.0 - fx) + at(x0 + 1, y0, c) * fx;
                let bottom = at(x0, y0 + 1, c) * (1.0 - fx) + at(x0 + 1, y0 + 1, c) * fx;
                out.push((top * (1.0 - fy) + bottom * fy).round() as u8);
            }
            out.push(255);
        }
    }
    Rgba { width: size, height: size, pixels: out }
}

/// Half the size (average of 2 × 2), for the next mipmap.
fn halve(image: &Rgba) -> Rgba {
    let (w, h) = ((image.width / 2).max(1), (image.height / 2).max(1));
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            for c in 0..4 {
                let mut sum = 0u32;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let (sx, sy) = ((x * 2 + dx).min(image.width - 1), (y * 2 + dy).min(image.height - 1));
                    sum += u32::from(image.pixels[((sy * image.width + sx) * 4) as usize + c]);
                }
                out.push((sum / 4) as u8);
            }
        }
    }
    Rgba { width: w, height: h, pixels: out }
}

fn rgb565(c: [u8; 3]) -> u16 {
    (u16::from(c[0] >> 3) << 11) | (u16::from(c[1] >> 2) << 5) | u16::from(c[2] >> 3)
}

fn from565(v: u16) -> [i32; 3] {
    let r = i32::from((v >> 11) & 31);
    let g = i32::from((v >> 5) & 63);
    let b = i32::from(v & 31);
    [(r << 3) | (r >> 2), (g << 2) | (g >> 4), (b << 3) | (b >> 2)]
}

/// One mipmap level as DXT1 (4 × 4 blocks: two end colours, 2-bit picks).
fn dxt1(image: &Rgba) -> Vec<u8> {
    let (w, h) = (image.width, image.height);
    let mut out = Vec::new();
    for by in (0..h.max(4)).step_by(4) {
        for bx in (0..w.max(4)).step_by(4) {
            let mut block = [[0u8; 3]; 16];
            for (i, px) in block.iter_mut().enumerate() {
                let (x, y) = ((bx + i as u32 % 4).min(w - 1), (by + i as u32 / 4).min(h - 1));
                let at = ((y * w + x) * 4) as usize;
                px.copy_from_slice(&image.pixels[at..at + 3]);
            }
            // End colours: the darkest and brightest by luma.
            let luma = |c: &[u8; 3]| u32::from(c[0]) * 3 + u32::from(c[1]) * 6 + u32::from(c[2]);
            let lo = *block.iter().min_by_key(|c| luma(c)).unwrap_or(&[0; 3]);
            let hi = *block.iter().max_by_key(|c| luma(c)).unwrap_or(&[0; 3]);
            let (mut a, mut b) = (rgb565(hi), rgb565(lo));
            if a < b {
                std::mem::swap(&mut a, &mut b);
            }
            let (ca, cb) = (from565(a), from565(b));
            let palette = [ca, cb, [0, 1, 2].map(|i| (2 * ca[i] + cb[i]) / 3), [0, 1, 2].map(|i| (ca[i] + 2 * cb[i]) / 3)];
            let mut indices = 0u32;
            for (i, px) in block.iter().enumerate() {
                let best = (0..4)
                    .min_by_key(|&k| (0..3).map(|c| (i32::from(px[c]) - palette[k][c]).pow(2)).sum::<i32>())
                    .unwrap_or(0);
                indices |= (best as u32) << (i * 2);
            }
            // a == b would mean the 3-colour mode; all picks are 0 then.
            if a == b {
                indices = 0;
            }
            out.extend_from_slice(&a.to_le_bytes());
            out.extend_from_slice(&b.to_le_bytes());
            out.extend_from_slice(&indices.to_le_bytes());
        }
    }
    out
}

/// A face as Roblox's sky texture: DDS, DXT1, with every mipmap.
pub fn tex(face: &Rgba) -> Vec<u8> {
    let image = if face.width == SIZE && face.height == SIZE { face.clone() } else { stretch(face, SIZE) };
    let levels = SIZE.ilog2() + 1;
    let mut dds = Vec::new();
    dds.extend_from_slice(b"DDS ");
    let dword = |v: u32, d: &mut Vec<u8>| d.extend_from_slice(&v.to_le_bytes());
    dword(124, &mut dds); // header size
    dword(0x000A_1007, &mut dds); // caps | height | width | pixel format | mipmaps | linear size
    dword(SIZE, &mut dds);
    dword(SIZE, &mut dds);
    dword(SIZE * SIZE / 2, &mut dds); // size of the first level
    dword(0, &mut dds); // depth
    dword(levels, &mut dds);
    dds.extend_from_slice(&[0; 44]); // reserved
    dword(32, &mut dds); // pixel format size
    dword(0x4, &mut dds); // FOURCC
    dds.extend_from_slice(b"DXT1");
    dds.extend_from_slice(&[0; 20]);
    dword(0x0040_1008, &mut dds); // complex | texture | mipmap
    dds.extend_from_slice(&[0; 16]);
    let mut level = image;
    for _ in 0..levels {
        dds.extend_from_slice(&dxt1(&level));
        level = halve(&level);
    }
    dds
}

/// The mod holding Feline's sky.
pub const SKY_MOD: &str = "Feline Sky";

/// Writes `faces` as the "Feline Sky" mod in `mods` (replacing an older
/// one), with a preview strip of its four sides.
pub fn install(mods: &std::path::Path, faces: &[Rgba; 6]) -> std::io::Result<()> {
    let dir = mods.join(SKY_MOD);
    let _ = std::fs::remove_dir_all(&dir);
    for (face, image) in FACES.iter().zip(faces) {
        let path = dir.join(face_path(face));
        std::fs::create_dir_all(path.parent().unwrap_or(&dir))?;
        std::fs::write(path, tex(image))?;
    }
    // Front, right, back, left side by side.
    let sides = [&faces[2], &faces[4], &faces[0], &faces[3]];
    let small: Vec<Rgba> = sides.iter().map(|f| stretch(f, 128)).collect();
    let mut strip = Rgba { width: 512, height: 128, pixels: vec![0; 512 * 128 * 4] };
    for (n, s) in small.iter().enumerate() {
        for y in 0..128usize {
            let to = (y * 512 + n * 128) * 4;
            strip.pixels[to..to + 512].copy_from_slice(&s.pixels[y * 512..y * 512 + 512]);
        }
    }
    std::fs::write(dir.join("preview.png"), crate::png::encode(&strip))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sky_files_look_like_robloxs() {
        let faces = Preset::StarryNight.faces();
        let file = tex(&faces[0]);
        assert!(file.starts_with(b"DDS "));
        assert_eq!(&file[84..88], b"DXT1");
        // 512² ÷ 2 for the first level, … down to 1 × 1: Roblox's size.
        assert_eq!(file.len(), 174_904);
        assert_eq!(u32::from_le_bytes(file[28..32].try_into().unwrap()), 10);
    }

    #[test]
    fn colours_survive_compression() {
        let face = flat([200.0, 100.0, 50.0]);
        let block = dxt1(&face);
        let colour = from565(u16::from_le_bytes([block[0], block[1]]));
        assert!((colour[0] - 200).abs() < 9 && (colour[1] - 100).abs() < 5 && (colour[2] - 50).abs() < 9, "{colour:?}");
    }

    #[test]
    fn pictures_become_six_faces() {
        let pano = Rgba { width: 800, height: 200, pixels: vec![120; 800 * 200 * 4] };
        let faces = from_picture(&pano);
        assert!(faces.iter().all(|f| f.width == SIZE && f.height == SIZE));
    }
}

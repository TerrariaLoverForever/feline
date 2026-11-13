// SPDX-License-Identifier: GPL-3.0-or-later
// (with the additional permission in LICENSE.exception)
//! ETC2 and EAC texture blocks to plain pixels.
//!
//! Roblox's Android app downloads its textures as ETC2 when the GPU says it
//! can sample ETC2 (mobile GPUs and desktop OpenGL ES can). Desktop Vulkan
//! drivers usually can't, and then the engine takes PNG textures instead,
//! which look blurry. Feline's Vulkan layer (vktex.rs) says ETC2 is there
//! and decodes each uploaded block with this module.
//!
//! Follows the Khronos Data Format Specification 1.3, "ETC2 compressed
//! texture image formats" and "EAC" (the public format definitions).
//! Pixels in a 4×4 block are numbered down the columns: pixel `x*4 + y`.

/// ETC1/ETC2 intensity modifiers, by table codeword, for pixel indices
/// 0..=3 (`msb << 1 | lsb`).
const MODIFIERS: [[i32; 4]; 8] = [
    [2, 8, -2, -8],
    [5, 17, -5, -17],
    [9, 29, -9, -29],
    [13, 42, -13, -42],
    [18, 60, -18, -60],
    [24, 80, -24, -80],
    [33, 106, -33, -106],
    [47, 183, -47, -183],
];

/// T and H mode distances.
const DISTANCES: [i32; 8] = [3, 6, 11, 16, 23, 32, 41, 64];

/// EAC modifier tables.
const EAC: [[i32; 8]; 16] = [
    [-3, -6, -9, -15, 2, 5, 8, 14],
    [-3, -7, -10, -13, 2, 6, 9, 12],
    [-2, -5, -8, -13, 1, 4, 7, 12],
    [-2, -4, -6, -13, 1, 3, 5, 12],
    [-3, -6, -8, -12, 2, 5, 7, 11],
    [-3, -7, -9, -11, 2, 6, 8, 10],
    [-4, -7, -8, -11, 3, 6, 7, 10],
    [-3, -5, -8, -11, 2, 4, 7, 10],
    [-2, -6, -8, -10, 1, 5, 7, 9],
    [-2, -5, -8, -10, 1, 4, 7, 9],
    [-2, -4, -8, -10, 1, 3, 7, 9],
    [-2, -5, -7, -10, 1, 4, 6, 9],
    [-3, -4, -7, -10, 2, 3, 6, 9],
    [-1, -2, -3, -10, 0, 1, 2, 9],
    [-4, -6, -8, -9, 3, 5, 7, 8],
    [-3, -5, -7, -9, 2, 4, 6, 8],
];

fn bits(v: u64, high: u32, count: u32) -> i32 {
    ((v >> (high + 1 - count)) & ((1 << count) - 1)) as i32
}
fn ext4(x: i32) -> i32 {
    (x << 4) | x
}
fn ext5(x: i32) -> i32 {
    (x << 3) | (x >> 2)
}
fn ext6(x: i32) -> i32 {
    (x << 2) | (x >> 4)
}
fn ext7(x: i32) -> i32 {
    (x << 1) | (x >> 6)
}
fn clamp(x: i32) -> u8 {
    x.clamp(0, 255) as u8
}

/// The 2-bit index of pixel `i` (column-major) in an ETC block.
fn index(v: u64, i: usize) -> usize {
    let msb = (v >> (16 + i)) & 1;
    let lsb = (v >> i) & 1;
    ((msb << 1) | lsb) as usize
}

/// One 8-byte ETC2 colour block → 16 RGBA pixels (row-major, `y*4 + x`).
/// `punchthrough`: the RGB8A1 format, where the "diff" bit is "opaque".
pub fn decode_color(block: &[u8; 8], punchthrough: bool) -> [[u8; 4]; 16] {
    let v = u64::from_be_bytes(*block);
    let mut out = [[0u8, 0, 0, 255]; 16];
    let diff = punchthrough || (v >> 33) & 1 == 1;
    let opaque = !punchthrough || (v >> 33) & 1 == 1;
    let flip = (v >> 32) & 1 == 1;
    let set = |out: &mut [[u8; 4]; 16], i: usize, c: [i32; 3]| {
        let (x, y) = (i / 4, i % 4);
        out[y * 4 + x] = [clamp(c[0]), clamp(c[1]), clamp(c[2]), 255];
    };
    let transparent = |out: &mut [[u8; 4]; 16], i: usize| {
        let (x, y) = (i / 4, i % 4);
        out[y * 4 + x] = [0, 0, 0, 0];
    };

    if !diff {
        // Individual mode: two 4-bit colours.
        let c1 = [ext4(bits(v, 63, 4)), ext4(bits(v, 55, 4)), ext4(bits(v, 47, 4))];
        let c2 = [ext4(bits(v, 59, 4)), ext4(bits(v, 51, 4)), ext4(bits(v, 43, 4))];
        let tables = [bits(v, 39, 3) as usize, bits(v, 36, 3) as usize];
        for i in 0..16 {
            let (x, y) = (i / 4, i % 4);
            let second = if flip { y >= 2 } else { x >= 2 };
            let (base, table) = if second { (c2, tables[1]) } else { (c1, tables[0]) };
            let m = MODIFIERS[table][index(v, i)];
            set(&mut out, i, [base[0] + m, base[1] + m, base[2] + m]);
        }
        return out;
    }

    let (r, g, b) = (bits(v, 63, 5), bits(v, 55, 5), bits(v, 47, 5));
    let signed3 = |x: i32| if x >= 4 { x - 8 } else { x };
    let (dr, dg, db) = (signed3(bits(v, 58, 3)), signed3(bits(v, 50, 3)), signed3(bits(v, 42, 3)));

    if !(0..=31).contains(&(r + dr)) {
        // T mode.
        let c1 = [
            ext4((bits(v, 60, 2) << 2) | bits(v, 57, 2)),
            ext4(bits(v, 55, 4)),
            ext4(bits(v, 51, 4)),
        ];
        let c2 = [ext4(bits(v, 47, 4)), ext4(bits(v, 43, 4)), ext4(bits(v, 39, 4))];
        let d = DISTANCES[((bits(v, 35, 2) << 1) | bits(v, 32, 1)) as usize];
        let paint = [
            c1,
            [c2[0] + d, c2[1] + d, c2[2] + d],
            c2,
            [c2[0] - d, c2[1] - d, c2[2] - d],
        ];
        for i in 0..16 {
            let k = index(v, i);
            if !opaque && k == 2 {
                transparent(&mut out, i);
            } else {
                set(&mut out, i, paint[k]);
            }
        }
        return out;
    }
    if !(0..=31).contains(&(g + dg)) {
        // H mode.
        let r1 = bits(v, 62, 4);
        let g1 = (bits(v, 58, 3) << 1) | bits(v, 52, 1);
        let b1 = (bits(v, 51, 1) << 3) | bits(v, 49, 3);
        let (r2, g2, b2) = (bits(v, 46, 4), bits(v, 42, 4), bits(v, 38, 4));
        let order = i32::from((r1 << 8) + (g1 << 4) + b1 >= (r2 << 8) + (g2 << 4) + b2);
        let d = DISTANCES[((bits(v, 34, 1) << 2) | (bits(v, 32, 1) << 1) | order) as usize];
        let c1 = [ext4(r1), ext4(g1), ext4(b1)];
        let c2 = [ext4(r2), ext4(g2), ext4(b2)];
        let paint = [
            [c1[0] + d, c1[1] + d, c1[2] + d],
            [c1[0] - d, c1[1] - d, c1[2] - d],
            [c2[0] + d, c2[1] + d, c2[2] + d],
            [c2[0] - d, c2[1] - d, c2[2] - d],
        ];
        for i in 0..16 {
            let k = index(v, i);
            if !opaque && k == 2 {
                transparent(&mut out, i);
            } else {
                set(&mut out, i, paint[k]);
            }
        }
        return out;
    }
    if !(0..=31).contains(&(b + db)) {
        // Planar mode (always opaque).
        let ro = ext6(bits(v, 62, 6));
        let go = ext7((bits(v, 56, 1) << 6) | bits(v, 54, 6));
        let bo = ext6((bits(v, 48, 1) << 5) | (bits(v, 44, 2) << 3) | bits(v, 41, 3));
        let rh = ext6((bits(v, 38, 5) << 1) | bits(v, 32, 1));
        let gh = ext7(bits(v, 31, 7));
        let bh = ext6(bits(v, 24, 6));
        let rv = ext6(bits(v, 18, 6));
        let gv = ext7(bits(v, 12, 7));
        let bv = ext6(bits(v, 5, 6));
        for i in 0..16 {
            let (x, y) = ((i / 4) as i32, (i % 4) as i32);
            let f = |o: i32, h: i32, vv: i32| (x * (h - o) + y * (vv - o) + 4 * o + 2) >> 2;
            set(&mut out, i, [f(ro, rh, rv), f(go, gh, gv), f(bo, bh, bv)]);
        }
        return out;
    }

    // Differential mode.
    let c1 = [ext5(r), ext5(g), ext5(b)];
    let c2 = [ext5(r + dr), ext5(g + dg), ext5(b + db)];
    let tables = [bits(v, 39, 3) as usize, bits(v, 36, 3) as usize];
    for i in 0..16 {
        let (x, y) = (i / 4, i % 4);
        let second = if flip { y >= 2 } else { x >= 2 };
        let (base, table) = if second { (c2, tables[1]) } else { (c1, tables[0]) };
        let k = index(v, i);
        if !opaque && k == 2 {
            transparent(&mut out, i);
            continue;
        }
        // Punch-through, not opaque: index 0 has no modifier.
        let m = if !opaque && k == 0 { 0 } else { MODIFIERS[table][k] };
        set(&mut out, i, [base[0] + m, base[1] + m, base[2] + m]);
    }
    out
}

/// One 8-byte EAC block as 8-bit values (the alpha of RGBA8 ETC2), row-major.
pub fn decode_alpha(block: &[u8; 8]) -> [u8; 16] {
    let v = u64::from_be_bytes(*block);
    let base = bits(v, 63, 8);
    let mult = bits(v, 55, 4);
    let table = EAC[bits(v, 51, 4) as usize];
    let mut out = [0u8; 16];
    for i in 0..16 {
        let k = ((v >> (45 - 3 * i)) & 7) as usize;
        let (x, y) = (i / 4, i % 4);
        out[y * 4 + x] = clamp(base + table[k] * mult);
    }
    out
}

/// One 8-byte EAC R11 block as 16-bit values, row-major (unsigned:
/// 0..=65535; signed: as i16 bits, -32767..=32767).
pub fn decode_r11(block: &[u8; 8], signed: bool) -> [u16; 16] {
    let v = u64::from_be_bytes(*block);
    let mult = bits(v, 55, 4);
    let table = EAC[bits(v, 51, 4) as usize];
    let mut out = [0u16; 16];
    for i in 0..16 {
        let k = ((v >> (45 - 3 * i)) & 7) as usize;
        let (x, y) = (i / 4, i % 4);
        let m = if mult == 0 { table[k] } else { table[k] * mult * 8 };
        out[y * 4 + x] = if signed {
            let base = i32::from(bits(v, 63, 8) as u8 as i8).max(-127);
            let value = (base * 8 + m).clamp(-1023, 1023);
            // 11 bits signed → 16 bits signed.
            let wide = if value >= 0 {
                (value << 5) | (value >> 5)
            } else {
                -(((-value) << 5) | ((-value) >> 5))
            };
            wide as i16 as u16
        } else {
            let value = (bits(v, 63, 8) * 8 + 4 + m).clamp(0, 2047);
            ((value << 5) | (value >> 6)) as u16
        };
    }
    out
}

/// The ETC2/EAC Vulkan formats Feline decodes, and what they become.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// ETC2 RGB (no alpha) → RGBA8.
    Rgb,
    /// ETC2 RGB with 1-bit alpha → RGBA8.
    RgbA1,
    /// ETC2 RGB with an EAC alpha block → RGBA8.
    Rgba,
    /// EAC R11 → R16.
    R11 { signed: bool },
    /// EAC RG11 → RG16.
    Rg11 { signed: bool },
}

impl Kind {
    /// `(kind, the uncompressed Vulkan format)` for an ETC2/EAC format.
    pub fn of(format: i32) -> Option<(Kind, i32)> {
        Some(match format {
            147 => (Kind::Rgb, 37),             // R8G8B8A8_UNORM
            148 => (Kind::Rgb, 43),             // R8G8B8A8_SRGB
            149 => (Kind::RgbA1, 37),
            150 => (Kind::RgbA1, 43),
            151 => (Kind::Rgba, 37),
            152 => (Kind::Rgba, 43),
            153 => (Kind::R11 { signed: false }, 70),  // R16_UNORM
            154 => (Kind::R11 { signed: true }, 71),   // R16_SNORM
            155 => (Kind::Rg11 { signed: false }, 77), // R16G16_UNORM
            156 => (Kind::Rg11 { signed: true }, 78),  // R16G16_SNORM
            _ => return None,
        })
    }

    /// Bytes per compressed 4×4 block.
    pub fn block_bytes(self) -> usize {
        match self {
            Kind::Rgb | Kind::RgbA1 | Kind::R11 { .. } => 8,
            Kind::Rgba | Kind::Rg11 { .. } => 16,
        }
    }

    /// Bytes per decoded pixel.
    pub fn pixel_bytes(self) -> usize {
        match self {
            Kind::R11 { .. } => 2,
            _ => 4,
        }
    }

    /// One block → 16 decoded pixels, row-major, `pixel_bytes()` each.
    pub fn decode(self, block: &[u8]) -> [[u8; 4]; 16] {
        let half = |at: usize| -> [u8; 8] { block[at..at + 8].try_into().unwrap_or([0; 8]) };
        match self {
            Kind::Rgb => decode_color(&half(0), false),
            Kind::RgbA1 => decode_color(&half(0), true),
            Kind::Rgba => {
                let mut px = decode_color(&half(8), false);
                for (p, a) in px.iter_mut().zip(decode_alpha(&half(0))) {
                    p[3] = a;
                }
                px
            }
            Kind::R11 { signed } => decode_r11(&half(0), signed).map(|r| {
                let [lo, hi] = r.to_le_bytes();
                [lo, hi, 0, 0]
            }),
            Kind::Rg11 { signed } => {
                let (r, g) = (decode_r11(&half(0), signed), decode_r11(&half(8), signed));
                std::array::from_fn(|i| {
                    let [r0, r1] = r[i].to_le_bytes();
                    let [g0, g1] = g[i].to_le_bytes();
                    [r0, r1, g0, g1]
                })
            }
        }
    }
}

/// Decodes a `width`×`height` area of blocks (`row_blocks` blocks per row
/// in `src`) into `dst`, `width * pixel_bytes()` bytes per row.
pub fn decode_area(kind: Kind, src: &[u8], row_blocks: usize, width: usize, height: usize, dst: &mut [u8]) {
    let bb = kind.block_bytes();
    let pb = kind.pixel_bytes();
    for by in 0..height.div_ceil(4) {
        for bx in 0..width.div_ceil(4) {
            let at = (by * row_blocks + bx) * bb;
            let Some(block) = src.get(at..at + bb) else {
                return;
            };
            let px = kind.decode(block);
            for y in 0..4 {
                let py = by * 4 + y;
                if py >= height {
                    break;
                }
                for x in 0..4 {
                    let pxx = bx * 4 + x;
                    if pxx >= width {
                        break;
                    }
                    let o = (py * width + pxx) * pb;
                    if let Some(d) = dst.get_mut(o..o + pb) {
                        d.copy_from_slice(&px[y * 4 + x][..pb]);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encodes a differential-mode block of one flat colour (5-bit) with
    /// table 0 and every pixel at index `k`.
    fn flat(r: u64, g: u64, b: u64, k: u64) -> [u8; 8] {
        let mut v = (r << 59) | (g << 51) | (b << 43) | (1 << 33);
        let msb = (k >> 1) & 1;
        let lsb = k & 1;
        v |= (if msb == 1 { 0xFFFF } else { 0 }) << 16;
        v |= if lsb == 1 { 0xFFFF } else { 0 };
        v.to_be_bytes()
    }

    #[test]
    fn differential_flat_colour() {
        let px = decode_color(&flat(31, 0, 16, 0), false);
        // ext5(31)=255, ext5(0)=0, ext5(16)=132; modifier +2.
        assert!(px.iter().all(|p| *p == [255, 2, 134, 255]));
        let px = decode_color(&flat(31, 0, 16, 3), false);
        assert!(px.iter().all(|p| *p == [247, 0, 124, 255]));
    }

    #[test]
    fn individual_mode_halves() {
        // R1=15, R2=0 (others 0), no flip, table 0, index 0 (+2).
        let v: u64 = 0xF << 60;
        let px = decode_color(&v.to_be_bytes(), false);
        assert_eq!(px[0], [255, 2, 2, 255]); // x=0: first half
        assert_eq!(px[3], [2, 2, 2, 255]); // x=3: second half
        // Flipped: rows split instead.
        let px = decode_color(&(v | 1 << 32).to_be_bytes(), false);
        assert_eq!(px[3], [255, 2, 2, 255]); // y=0
        assert_eq!(px[12], [2, 2, 2, 255]); // y=3
    }

    #[test]
    fn planar_gradient() {
        // R: O=0, H=63 (6 bits), V=0; G,B zero. Blue overflows → planar.
        // b=31 with db=+1 (bits 42..40 = 001) overflows the blue sum.
        let mut v: u64 = (31 << 43) | (1 << 40) | (1 << 33);
        // RO = bits 62..57 = 0. RH = bits 38..34 <<1 | bit 32 = 63.
        v &= !(0b11111 << 34);
        v |= 0b11111 << 34;
        v |= 1 << 32;
        // Keep the overflow bits: blue base bits are 48 and 44..43, 41..39;
        // only check that red rises from left to right.
        let px = decode_color(&v.to_be_bytes(), false);
        assert!(px[0][0] < px[3][0], "{:?}", &px[..4]);
    }

    #[test]
    fn eac_alpha_flat() {
        // base 128, mult 1, table 13 (has a 0 entry at index 4).
        let mut v: u64 = (128 << 56) | (1 << 52) | (13 << 48);
        for i in 0..16 {
            v |= 4 << (45 - 3 * i);
        }
        assert!(decode_alpha(&v.to_be_bytes()).iter().all(|a| *a == 128));
    }

    #[test]
    fn rgba_blocks_combine_alpha_and_colour() {
        let mut alpha: u64 = (200 << 56) | (1 << 52) | (13 << 48);
        for i in 0..16 {
            alpha |= 4 << (45 - 3 * i);
        }
        let mut block = [0u8; 16];
        block[..8].copy_from_slice(&alpha.to_be_bytes());
        block[8..].copy_from_slice(&flat(31, 0, 0, 0));
        let px = Kind::Rgba.decode(&block);
        assert!(px.iter().all(|p| *p == [255, 2, 2, 200]));
    }

    #[test]
    fn areas_smaller_than_a_block() {
        let block = flat(31, 31, 31, 0);
        let mut dst = vec![0u8; 2 * 2 * 4];
        decode_area(Kind::Rgb, &block, 1, 2, 2, &mut dst);
        assert!(dst.chunks(4).all(|p| p == [255, 255, 255, 255]));
    }

    #[test]
    fn formats() {
        assert_eq!(Kind::of(151), Some((Kind::Rgba, 37)));
        assert_eq!(Kind::of(147).unwrap().0.block_bytes(), 8);
        assert_eq!(Kind::of(37), None);
    }

    /// Decodes a real texture for a look: FELINE_ETC2_SAMPLE=file,format,w,h
    /// (one mip level of raw blocks) → file.ppm.
    #[test]
    #[ignore = "needs a texture file"]
    fn decode_a_sample() {
        let Ok(spec) = std::env::var("FELINE_ETC2_SAMPLE") else { return };
        let parts: Vec<&str> = spec.split(',').collect();
        let (kind, _) = Kind::of(parts[1].parse().unwrap()).unwrap();
        let (w, h): (usize, usize) = (parts[2].parse().unwrap(), parts[3].parse().unwrap());
        let src = std::fs::read(parts[0]).unwrap();
        let mut dst = vec![0u8; w * h * 4];
        decode_area(kind, &src, w.div_ceil(4), w, h, &mut dst);
        let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
        ppm.extend(dst.chunks(4).flat_map(|p| [p[0], p[1], p[2]]));
        std::fs::write(format!("{}.ppm", parts[0]), ppm).unwrap();
    }

}

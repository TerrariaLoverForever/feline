// SPDX-License-Identifier: GPL-3.0-or-later
//! A small PNG reader and writer. Reads every non-interlaced PNG (greyscale,
//! RGB, palette with transparency, grey + alpha, RGBA; 1–16 bits) into
//! 8-bit RGBA, and writes 8-bit RGBA. Deflate is `miniz_oxide`.
//!
//! Shared by the launcher (`feline-core`: recolouring Roblox's UI images)
//! and the runtime (colour emoji in typed text, which fonts store as PNG).

#![forbid(unsafe_code)]

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

pub fn crc32(data: &[u8]) -> u32 {
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

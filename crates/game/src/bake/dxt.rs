//! DXT (BC1-3) blocks to RGBA8, for reading textures on the CPU.

use iw3::iwi::{Format, Iwi};

/// One level of `iwi` (cube maps: face `face` of it) as RGBA8, with its size.
pub fn decode(iwi: &Iwi, level: usize, face: usize) -> Option<(usize, usize, Vec<[u8; 4]>)> {
    let (w, h) = ((iwi.width as usize >> level).max(1), (iwi.height as usize >> level).max(1));
    let data = iwi.levels.get(level)?;
    let faces = if iwi.is_cube() { 6 } else { 1 };
    let size = data.len() / faces;
    let data = data.get(face * size..(face + 1) * size)?;
    let block_bytes = match iwi.format {
        Format::Dxt1 => 8,
        Format::Dxt3 | Format::Dxt5 => 16,
        _ => {
            let rgba = Iwi { format: iwi.format, flags: 0, width: w as u32, height: h as u32, depth: 1, levels: vec![data.to_vec()] }.to_rgba8(0)?;
            return Some((w, h, rgba.chunks_exact(4).map(|p| [p[0], p[1], p[2], p[3]]).collect()));
        }
    };
    let (bw, bh) = (w.div_ceil(4), h.div_ceil(4));
    if data.len() < bw * bh * block_bytes {
        return None;
    }
    let mut out = vec![[0u8; 4]; w * h];
    for by in 0..bh {
        for bx in 0..bw {
            let b = &data[(by * bw + bx) * block_bytes..][..block_bytes];
            let (alpha, colour) = match iwi.format {
                Format::Dxt1 => (None, b),
                Format::Dxt3 => (Some(explicit_alpha(&b[..8])), &b[8..]),
                _ => (Some(interpolated_alpha(&b[..8])), &b[8..]),
            };
            let px = colour_block(colour, iwi.format == Format::Dxt1);
            for i in 0..16 {
                let (x, y) = (bx * 4 + i % 4, by * 4 + i / 4);
                if x < w && y < h {
                    let mut p = px[i];
                    if let Some(a) = &alpha {
                        p[3] = a[i];
                    }
                    out[y * w + x] = p;
                }
            }
        }
    }
    Some((w, h, out))
}

fn rgb565(c: u16) -> [u32; 3] {
    let r = (c >> 11) as u32 & 31;
    let g = (c >> 5) as u32 & 63;
    let b = c as u32 & 31;
    [(r << 3) | (r >> 2), (g << 2) | (g >> 4), (b << 3) | (b >> 2)]
}

fn colour_block(b: &[u8], dxt1: bool) -> [[u8; 4]; 16] {
    let c0 = u16::from_le_bytes([b[0], b[1]]);
    let c1 = u16::from_le_bytes([b[2], b[3]]);
    let (p0, p1) = (rgb565(c0), rgb565(c1));
    let mix = |a: u32, b: u32, wa: u32, wb: u32| ((a * wa + b * wb) / (wa + wb)) as u8;
    let mut pal = [[0u8; 4]; 4];
    pal[0] = [p0[0] as u8, p0[1] as u8, p0[2] as u8, 255];
    pal[1] = [p1[0] as u8, p1[1] as u8, p1[2] as u8, 255];
    if c0 > c1 || !dxt1 {
        pal[2] = [mix(p0[0], p1[0], 2, 1), mix(p0[1], p1[1], 2, 1), mix(p0[2], p1[2], 2, 1), 255];
        pal[3] = [mix(p0[0], p1[0], 1, 2), mix(p0[1], p1[1], 1, 2), mix(p0[2], p1[2], 1, 2), 255];
    } else {
        pal[2] = [mix(p0[0], p1[0], 1, 1), mix(p0[1], p1[1], 1, 1), mix(p0[2], p1[2], 1, 1), 255];
        pal[3] = [0, 0, 0, 0];
    }
    let bits = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
    std::array::from_fn(|i| pal[(bits >> (2 * i)) as usize & 3])
}

fn explicit_alpha(b: &[u8]) -> [u8; 16] {
    let bits = u64::from_le_bytes(b.try_into().unwrap());
    std::array::from_fn(|i| ((bits >> (4 * i)) & 15) as u8 * 17)
}

fn interpolated_alpha(b: &[u8]) -> [u8; 16] {
    let (a0, a1) = (b[0] as u32, b[1] as u32);
    let mut pal = [0u32; 8];
    pal[0] = a0;
    pal[1] = a1;
    if a0 > a1 {
        for i in 1..7 {
            pal[i + 1] = ((7 - i as u32) * a0 + i as u32 * a1) / 7;
        }
    } else {
        for i in 1..5 {
            pal[i + 1] = ((5 - i as u32) * a0 + i as u32 * a1) / 5;
        }
        pal[6] = 0;
        pal[7] = 255;
    }
    let mut bits = 0u64;
    for (i, &v) in b[2..8].iter().enumerate() {
        bits |= (v as u64) << (8 * i);
    }
    std::array::from_fn(|i| pal[((bits >> (3 * i)) & 7) as usize] as u8)
}

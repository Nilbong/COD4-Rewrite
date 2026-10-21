//! ETC2 RGB8 blocks from RGBA8 pixels: what Android GPUs sample natively (a
//! quarter of RGBA8's memory). A fast encoder: per 4x4 block, both sub-block
//! splits, individual (444) and differential (555) base colours from each
//! sub-block's average, and each sub-block's best intensity table. Only the
//! ETC1-compatible modes are written, so any ETC2 (or ETC1) decoder reads it.

/// ETC1's intensity modifier tables: index 0..3 is +a, +b, -a, -b.
const TABLES: [[i32; 4]; 8] = [
    [2, 8, -2, -8],
    [5, 17, -5, -17],
    [9, 29, -9, -29],
    [13, 42, -13, -42],
    [18, 60, -18, -60],
    [24, 80, -24, -80],
    [33, 106, -33, -106],
    [47, 183, -47, -183],
];

/// RGBA8 rows (`width` x `height`, alpha ignored) as ETC2 RGB8 blocks, in
/// row-major block order. Sides needn't be multiples of 4 (edges repeat).
pub fn encode(rgba: &[u8], width: usize, height: usize) -> Vec<u8> {
    let (bw, bh) = (width.div_ceil(4), height.div_ceil(4));
    let mut out = Vec::with_capacity(bw * bh * 8);
    for by in 0..bh {
        for bx in 0..bw {
            let mut block = [[0u8; 3]; 16];
            for y in 0..4 {
                for x in 0..4 {
                    let (px, py) = ((bx * 4 + x).min(width - 1), (by * 4 + y).min(height - 1));
                    let i = (py * width + px) * 4;
                    block[y * 4 + x] = [rgba[i], rgba[i + 1], rgba[i + 2]];
                }
            }
            out.extend_from_slice(&encode_block(&block).to_be_bytes());
        }
    }
    out
}

/// The pixels (indices into a row-major 4x4 block) of sub-block `half` under
/// `flip` (false: left/right 2x4 halves, true: top/bottom 4x2 halves).
fn sub_block(flip: bool, half: usize) -> [usize; 8] {
    let mut px = [0; 8];
    let mut n = 0;
    for y in 0..4 {
        for x in 0..4 {
            let side = if flip { y / 2 } else { x / 2 };
            if side == half {
                px[n] = y * 4 + x;
                n += 1;
            }
        }
    }
    px
}

fn average(block: &[[u8; 3]; 16], px: &[usize; 8]) -> [f32; 3] {
    let mut sum = [0.0f32; 3];
    for &i in px {
        for c in 0..3 {
            sum[c] += block[i][c] as f32;
        }
    }
    sum.map(|s| s / 8.0)
}

/// The best table for one sub-block around `base`: (table, per-pixel
/// modifier indices, squared error).
fn best_table(block: &[[u8; 3]; 16], px: &[usize; 8], base: [i32; 3]) -> (usize, [u8; 8], i64) {
    let mut best = (0, [0u8; 8], i64::MAX);
    for (t, mods) in TABLES.iter().enumerate() {
        let mut idx = [0u8; 8];
        let mut err = 0i64;
        for (k, &i) in px.iter().enumerate() {
            let p = block[i];
            let mut pick = (0u8, i64::MAX);
            for (m, &d) in mods.iter().enumerate() {
                let e: i64 = (0..3)
                    .map(|c| {
                        let v = (base[c] + d).clamp(0, 255) - p[c] as i32;
                        (v * v) as i64
                    })
                    .sum();
                if e < pick.1 {
                    pick = (m as u8, e);
                }
            }
            idx[k] = pick.0;
            err += pick.1;
            if err >= best.2 {
                break;
            }
        }
        if err < best.2 {
            best = (t, idx, err);
        }
    }
    best
}

fn expand4(v: i32) -> i32 {
    (v << 4) | v
}

fn expand5(v: i32) -> i32 {
    (v << 3) | (v >> 2)
}

/// One block as its 64 bits (big-endian order, as stored).
fn encode_block(block: &[[u8; 3]; 16]) -> u64 {
    let mut best: Option<(i64, u64)> = None;
    for flip in [false, true] {
        let halves = [sub_block(flip, 0), sub_block(flip, 1)];
        let avg = [average(block, &halves[0]), average(block, &halves[1])];
        // Differential: 5-bit colours, the second within -4..3 of the first.
        let q5 = avg.map(|a| a.map(|v| (v * 31.0 / 255.0).round() as i32));
        let delta = [0, 1, 2].map(|c| q5[1][c] - q5[0][c]);
        let mut modes = vec![false];
        if delta.iter().all(|d| (-4..=3).contains(d)) {
            modes.push(true);
        }
        for diff in modes {
            let (bases, colour_bits) = if diff {
                let bases = [q5[0].map(expand5), q5[1].map(expand5)];
                let mut bits = 0u64;
                for c in 0..3 {
                    bits |= (q5[0][c] as u64) << (59 - 8 * c);
                    bits |= ((delta[c] & 7) as u64) << (56 - 8 * c);
                }
                (bases, bits)
            } else {
                let q4 = avg.map(|a| a.map(|v| (v * 15.0 / 255.0).round() as i32));
                let mut bits = 0u64;
                for c in 0..3 {
                    bits |= (q4[0][c] as u64) << (60 - 8 * c);
                    bits |= (q4[1][c] as u64) << (56 - 8 * c);
                }
                ([q4[0].map(expand4), q4[1].map(expand4)], bits)
            };
            let mut word = colour_bits | (u64::from(diff) << 33) | (u64::from(flip) << 32);
            let mut err = 0;
            for half in 0..2 {
                let (table, idx, e) = best_table(block, &halves[half], bases[half]);
                err += e;
                word |= (table as u64) << (37 - 3 * half);
                for (k, &i) in halves[half].iter().enumerate() {
                    // Pixel (x, y) is index bit x * 4 + y: msb plane in bits
                    // 16..31, lsb plane in 0..15.
                    let bit = (i % 4) * 4 + i / 4;
                    word |= u64::from(idx[k] >> 1) << (16 + bit);
                    word |= u64::from(idx[k] & 1) << bit;
                }
            }
            if best.is_none_or(|b| err < b.0) {
                best = Some((err, word));
            }
        }
    }
    best.map_or(0, |b| b.1)
}

/// The block back as pixels (row-major), for tests: an independent reading
/// of the format.
#[cfg(test)]
fn decode_block(word: u64) -> [[u8; 3]; 16] {
    let bits = |at: u32, n: u32| ((word >> at) & ((1 << n) - 1)) as i32;
    let diff = bits(33, 1) == 1;
    let flip = bits(32, 1) == 1;
    let mut bases = [[0i32; 3]; 2];
    for c in 0..3u32 {
        let at = 56 - 8 * c;
        if diff {
            let a = bits(at + 3, 5);
            let d = (bits(at, 3) << 29) >> 29;
            bases[0][c as usize] = expand5(a);
            bases[1][c as usize] = expand5(a + d);
        } else {
            bases[0][c as usize] = expand4(bits(at + 4, 4));
            bases[1][c as usize] = expand4(bits(at, 4));
        }
    }
    let tables = [bits(37, 3) as usize, bits(34, 3) as usize];
    let mut out = [[0u8; 3]; 16];
    for y in 0..4 {
        for x in 0..4 {
            let half = if flip { y / 2 } else { x / 2 };
            let bit = (x * 4 + y) as u32;
            let m = (bits(16 + bit, 1) << 1 | bits(bit, 1)) as usize;
            let d = TABLES[tables[half]][m];
            out[y * 4 + x] = [0, 1, 2].map(|c| (bases[half][c] + d).clamp(0, 255) as u8);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn psnr(a: &[[u8; 3]; 16], b: &[[u8; 3]; 16]) -> f64 {
        let mse: f64 = a.iter().zip(b).flat_map(|(p, q)| (0..3).map(move |c| (p[c] as f64 - q[c] as f64).powi(2))).sum::<f64>() / 48.0;
        if mse == 0.0 { 99.0 } else { 10.0 * (255.0f64 * 255.0 / mse).log10() }
    }

    #[test]
    fn flat_blocks_are_near_exact() {
        for colour in [[0, 0, 0], [255, 255, 255], [128, 64, 200], [17, 230, 90]] {
            let block = [colour; 16];
            let back = decode_block(encode_block(&block));
            assert!(psnr(&block, &back) > 40.0, "{colour:?} -> {:?}", back[0]);
        }
    }

    #[test]
    fn gradients_and_noise_survive() {
        let mut seed = 12345u32;
        let mut rnd = || {
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            (seed >> 16) as u8
        };
        let mut total = 0.0;
        for n in 0..200 {
            let base = [rnd(), rnd(), rnd()];
            let mut block = [[0u8; 3]; 16];
            for (i, p) in block.iter_mut().enumerate() {
                let g = (i % 4 * 12 + i / 4 * 6) as i32 + if n % 2 == 0 { (rnd() % 16) as i32 } else { 0 };
                *p = base.map(|b| (b as i32 + g - 30).clamp(0, 255) as u8);
            }
            let back = decode_block(encode_block(&block));
            total += psnr(&block, &back);
        }
        let mean = total / 200.0;
        assert!(mean > 30.0, "mean PSNR {mean:.1} dB");
    }

    #[test]
    fn two_colours_split_by_the_flip() {
        let mut block = [[200, 30, 30]; 16];
        for i in 8..16 {
            block[i] = [20, 40, 220];
        }
        let back = decode_block(encode_block(&block));
        assert!(psnr(&block, &back) > 30.0);
    }

    #[test]
    fn encode_pads_odd_sizes() {
        let rgba = vec![100u8; 6 * 5 * 4];
        assert_eq!(encode(&rgba, 6, 5).len(), 2 * 2 * 8);
    }
}

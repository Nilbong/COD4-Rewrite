//! The wavelet-compressed `.iwi` formats (`IMG_FORMAT_WAVELET_*`, 0x06-0x0A),
//! used by some menu art such as the main menu logo and the perk icons.
//!
//! Mip levels are coded smallest first. Levels thinner than two pixels are
//! stored as raw bytes; every other level is predicted from the next smaller
//! one: each 2x2 block of a channel is rebuilt from the smaller level's pixel
//! and three Huffman-coded coefficients (a Haar-style step), and the smaller
//! level can first be corrected by coded deltas. Bits are read LSB first.
//!
//! The code tables and reconstruction step are as documented by the
//! OpenAssetTools project (`IwiWaveletDecoder.cpp`).

use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaveletFormat {
    Rgba,
    Rgb,
    LuminanceAlpha,
    Luminance,
    Alpha,
}

impl WaveletFormat {
    pub fn from_u8(v: u8) -> Option<WaveletFormat> {
        Some(match v {
            0x06 => WaveletFormat::Rgba,
            0x07 => WaveletFormat::Rgb,
            0x08 => WaveletFormat::LuminanceAlpha,
            0x09 => WaveletFormat::Luminance,
            0x0A => WaveletFormat::Alpha,
            _ => return None,
        })
    }

    /// (coded channels, bytes per pixel while decoding).
    fn layout(self) -> (usize, usize) {
        match self {
            WaveletFormat::Rgba => (4, 4),
            WaveletFormat::Rgb => (3, 4),
            WaveletFormat::LuminanceAlpha => (2, 2),
            WaveletFormat::Luminance | WaveletFormat::Alpha => (1, 1),
        }
    }
}

/// Marks a value that follows as raw bits.
const ESCAPE: i16 = i16::MIN;

/// (LSB-first code, length, value)
type Code = (u16, u8, i16);

/// Blue (and luminance) coefficients.
#[rustfmt::skip]
const BLUE: [Code; 180] = [
    (0x001, 3, 0), (0x004, 5, 4), (0x005, 5, 2), (0x007, 5, 1), (0x00A, 5, 3), (0x014, 5, -4),
    (0x015, 5, -2), (0x017, 5, -1), (0x01A, 5, -3), (0x000, 6, 12), (0x002, 6, 10), (0x003, 6, 7),
    (0x006, 6, 9), (0x00B, 6, 6), (0x018, 6, 11), (0x01E, 6, 8), (0x01F, 6, 5), (0x020, 6, -12),
    (0x022, 6, -10), (0x023, 6, -7), (0x026, 6, -9), (0x02B, 6, -6), (0x038, 6, -11), (0x03C, 6, ESCAPE),
    (0x03E, 6, -8), (0x03F, 6, -5), (0x00F, 7, 13), (0x012, 7, 19), (0x016, 7, 18), (0x01B, 7, 14),
    (0x028, 7, 21), (0x02C, 7, 20), (0x02D, 7, 16), (0x02E, 7, 17), (0x030, 7, 22), (0x03D, 7, 15),
    (0x04F, 7, -13), (0x052, 7, -19), (0x056, 7, -18), (0x05B, 7, -14), (0x068, 7, -21), (0x06C, 7, -20),
    (0x06D, 7, -16), (0x06E, 7, -17), (0x070, 7, -22), (0x07D, 7, -15), (0x008, 8, 34), (0x00D, 8, 28),
    (0x00E, 8, 29), (0x013, 8, 26), (0x01D, 8, 27), (0x02F, 8, 23), (0x033, 8, 25), (0x03B, 8, 24),
    (0x048, 8, 33), (0x04C, 8, 32), (0x05C, 8, 31), (0x072, 8, 30), (0x088, 8, -34), (0x08D, 8, -28),
    (0x08E, 8, -29), (0x093, 8, -26), (0x09D, 8, -27), (0x0AF, 8, -23), (0x0B3, 8, -25), (0x0BB, 8, -24),
    (0x0C8, 8, -33), (0x0CC, 8, -32), (0x0DC, 8, -31), (0x0F2, 8, -30), (0x00C, 9, 47), (0x01C, 9, 46),
    (0x032, 9, 45), (0x036, 9, 44), (0x050, 9, 48), (0x076, 9, 43), (0x07B, 9, 37), (0x090, 9, 49),
    (0x0CD, 9, 40), (0x0CE, 9, 41), (0x0D3, 9, 38), (0x0DD, 9, 39), (0x0EF, 9, 35), (0x0F6, 9, 42),
    (0x0FB, 9, 36), (0x10C, 9, -47), (0x11C, 9, -46), (0x132, 9, -45), (0x136, 9, -44), (0x150, 9, -48),
    (0x176, 9, -43), (0x17B, 9, -37), (0x190, 9, -49), (0x1CD, 9, -40), (0x1CE, 9, -41), (0x1D3, 9, -38),
    (0x1DD, 9, -39), (0x1EF, 9, -35), (0x1F6, 9, -42), (0x1FB, 9, -36), (0x010, 10, 65), (0x04D, 10, 56),
    (0x04E, 10, 57), (0x05D, 10, 55), (0x08C, 10, 62), (0x09C, 10, 61), (0x110, 10, 64), (0x153, 10, 53),
    (0x15D, 10, 54), (0x16F, 10, 50), (0x173, 10, 52), (0x19C, 10, 60), (0x1B2, 10, 59), (0x1B6, 10, 58),
    (0x1D0, 10, 63), (0x1F3, 10, 51), (0x210, 10, -65), (0x24D, 10, -56), (0x24E, 10, -57), (0x25D, 10, -55),
    (0x28C, 10, -62), (0x29C, 10, -61), (0x310, 10, -64), (0x353, 10, -53), (0x35D, 10, -54), (0x36F, 10, -50),
    (0x373, 10, -52), (0x39C, 10, -60), (0x3B2, 10, -59), (0x3B6, 10, -58), (0x3D0, 10, -63), (0x3F3, 10, -51),
    (0x053, 11, 70), (0x06F, 11, 66), (0x073, 11, 69), (0x0B2, 11, 77), (0x0B6, 11, 75), (0x0D0, 11, 81),
    (0x14E, 11, 73), (0x18C, 11, 79), (0x273, 11, 68), (0x2B2, 11, 76), (0x2B6, 11, 74), (0x2D0, 11, 80),
    (0x2F3, 11, 67), (0x34D, 11, 71), (0x34E, 11, 72), (0x38C, 11, 78), (0x453, 11, -70), (0x46F, 11, -66),
    (0x473, 11, -69), (0x4B2, 11, -77), (0x4B6, 11, -75), (0x4D0, 11, -81), (0x54E, 11, -73), (0x58C, 11, -79),
    (0x673, 11, -68), (0x6B2, 11, -76), (0x6B6, 11, -74), (0x6D0, 11, -80), (0x6F3, 11, -67), (0x74D, 11, -71),
    (0x74E, 11, -72), (0x78C, 11, -78), (0x0F3, 12, 85), (0x14D, 12, 89), (0x253, 12, 87), (0x26F, 12, 83),
    (0x4F3, 12, 84), (0x54D, 12, 88), (0x653, 12, 86), (0x66F, 12, 82), (0x8F3, 12, -85), (0x94D, 12, -89),
    (0xA53, 12, -87), (0xA6F, 12, -83), (0xCF3, 12, -84), (0xD4D, 12, -88), (0xE53, 12, -86), (0xE6F, 12, -82),
];

/// Red and green coefficients, coded relative to blue's.
#[rustfmt::skip]
const RED_GREEN: [Code; 80] = [
    (0x003, 2, 0), (0x002, 3, 1), (0x006, 3, -1), (0x001, 4, 2), (0x009, 4, -2), (0x004, 5, 4),
    (0x00D, 5, 3), (0x014, 5, -4), (0x01D, 5, -3), (0x00C, 6, 6), (0x010, 6, 7), (0x015, 6, 5),
    (0x02C, 6, -6), (0x030, 6, -7), (0x035, 6, -5), (0x018, 7, 10), (0x01C, 7, 9), (0x020, 7, 11),
    (0x025, 7, 8), (0x058, 7, -10), (0x05C, 7, -9), (0x060, 7, -11), (0x065, 7, -8), (0x068, 7, ESCAPE),
    (0x038, 8, 14), (0x040, 8, 16), (0x045, 8, 12), (0x048, 8, 15), (0x07C, 8, 13), (0x0B8, 8, -14),
    (0x0C0, 8, -16), (0x0C5, 8, -12), (0x0C8, 8, -15), (0x0FC, 8, -13), (0x080, 9, 22), (0x085, 9, 17),
    (0x088, 9, 21), (0x0A8, 9, 20), (0x0BC, 9, 18), (0x0F8, 9, 19), (0x180, 9, -22), (0x185, 9, -17),
    (0x188, 9, -21), (0x1A8, 9, -20), (0x1BC, 9, -18), (0x1F8, 9, -19), (0x000, 10, 30), (0x03C, 10, 25),
    (0x078, 10, 26), (0x100, 10, 29), (0x105, 10, 23), (0x108, 10, 28), (0x128, 10, 27), (0x13C, 10, 24),
    (0x200, 10, -30), (0x23C, 10, -25), (0x278, 10, -26), (0x300, 10, -29), (0x305, 10, -23), (0x308, 10, -28),
    (0x328, 10, -27), (0x33C, 10, -24), (0x005, 11, 31), (0x008, 11, 37), (0x028, 11, 35), (0x178, 11, 33),
    (0x208, 11, 36), (0x228, 11, 34), (0x378, 11, 32), (0x405, 11, -31), (0x408, 11, -37), (0x428, 11, -35),
    (0x578, 11, -33), (0x608, 11, -36), (0x628, 11, -34), (0x778, 11, -32), (0x205, 12, 39), (0x605, 12, 38),
    (0xA05, 12, -39), (0xE05, 12, -38),
];

/// Alpha coefficients, and the deltas that correct a smaller level.
#[rustfmt::skip]
const ALPHA: [Code; 108] = [
    (0x001, 1, 0), (0x000, 4, ESCAPE), (0x002, 4, 1), (0x00A, 4, -1), (0x00C, 5, 2), (0x01C, 5, -2), (0x016, 6, 3),
    (0x018, 6, 4), (0x036, 6, -3), (0x038, 6, -4), (0x004, 7, 7), (0x02E, 7, 5), (0x034, 7, 6), (0x044, 7, -7),
    (0x06E, 7, -5), (0x074, 7, -6), (0x006, 8, 11), (0x008, 8, 14), (0x014, 8, 12), (0x01E, 8, 9), (0x048, 8, 15),
    (0x066, 8, 10), (0x068, 8, 13), (0x07E, 8, 8), (0x086, 8, -11), (0x088, 8, -14), (0x094, 8, -12), (0x09E, 8, -9),
    (0x0C8, 8, -15), (0x0E6, 8, -10), (0x0E8, 8, -13), (0x0FE, 8, -8), (0x028, 9, 23), (0x046, 9, 19), (0x054, 9, 20),
    (0x08E, 9, 17), (0x0A4, 9, 22), (0x0A8, 9, 24), (0x0C6, 9, 18), (0x0DE, 9, 16), (0x0E4, 9, 21), (0x128, 9, -23),
    (0x146, 9, -19), (0x154, 9, -20), (0x18E, 9, -17), (0x1A4, 9, -22), (0x1A8, 9, -24), (0x1C6, 9, -18), (0x1DE, 9, -16),
    (0x1E4, 9, -21), (0x00E, 10, 29), (0x024, 10, 37), (0x026, 10, 31), (0x04E, 10, 28), (0x064, 10, 35), (0x0BE, 10, 32),
    (0x0D4, 10, 33), (0x124, 10, 36), (0x126, 10, 30), (0x13E, 10, 25), (0x15E, 10, 26), (0x164, 10, 34), (0x1A6, 10, 127),
    (0x1CE, 10, 27), (0x1D4, 10, 128), (0x20E, 10, -29), (0x224, 10, -37), (0x226, 10, -31), (0x24E, 10, -28), (0x264, 10, -35),
    (0x2BE, 10, -32), (0x2D4, 10, -33), (0x324, 10, -36), (0x326, 10, -30), (0x33E, 10, -25), (0x35E, 10, -26), (0x364, 10, -34),
    (0x3A6, 10, -127), (0x3CE, 10, -27), (0x3D4, 10, -128), (0x03E, 11, 41), (0x05E, 11, 43), (0x0A6, 11, 50), (0x0CE, 11, 48),
    (0x10E, 11, 49), (0x14E, 11, 64), (0x1BE, 11, 39), (0x23E, 11, 40), (0x25E, 11, 42), (0x2A6, 11, 47), (0x2CE, 11, 44),
    (0x30E, 11, 46), (0x34E, 11, 45), (0x3BE, 11, 38), (0x43E, 11, -41), (0x45E, 11, -43), (0x4A6, 11, -50), (0x4CE, 11, -48),
    (0x50E, 11, -49), (0x54E, 11, -64), (0x5BE, 11, -39), (0x63E, 11, -40), (0x65E, 11, -42), (0x6A6, 11, -47), (0x6CE, 11, -44),
    (0x70E, 11, -46), (0x74E, 11, -45), (0x7BE, 11, -38),
];

const LOOKUP_BITS: u32 = 12;

/// Every 12-bit window -> (value, code length).
struct Lookup(Vec<(i16, u8)>);

impl Lookup {
    fn build(codes: &[Code]) -> Lookup {
        let mut t = vec![(0i16, 0u8); 1 << LOOKUP_BITS];
        for &(code, len, value) in codes {
            let mut i = code as usize;
            while i < t.len() {
                t[i] = (value, len);
                i += 1 << len;
            }
        }
        Lookup(t)
    }
}

fn tables() -> &'static [Lookup; 3] {
    static T: OnceLock<[Lookup; 3]> = OnceLock::new();
    T.get_or_init(|| [Lookup::build(&BLUE), Lookup::build(&RED_GREEN), Lookup::build(&ALPHA)])
}

/// Reads raw bytes until the first bit is read, then LSB-first bits.
struct Bits<'a> {
    data: &'a [u8],
    byte: usize,
    bit: Option<usize>,
}

impl Bits<'_> {
    fn raw_byte(&mut self) -> Option<u8> {
        if self.bit.is_some() {
            return None;
        }
        let b = *self.data.get(self.byte)?;
        self.byte += 1;
        Some(b)
    }

    fn pos(&mut self) -> usize {
        *self.bit.get_or_insert(self.byte * 8)
    }

    /// The next `n` (<= 24) bits; past the end reads as zero.
    fn peek(&mut self, n: u32) -> u32 {
        let pos = self.pos();
        let mut window = 0u64;
        for i in 0..4 {
            window |= (*self.data.get(pos / 8 + i).unwrap_or(&0) as u64) << (8 * i);
        }
        ((window >> (pos % 8)) & ((1 << n) - 1)) as u32
    }

    fn read(&mut self, n: u32) -> Option<u32> {
        let pos = self.pos();
        if pos + n as usize > self.data.len() * 8 {
            return None;
        }
        let v = self.peek(n);
        self.bit = Some(pos + n as usize);
        Some(v)
    }

    fn value(&mut self, table: &Lookup, escape_bits: u32, bias: i32) -> Option<i32> {
        let (value, len) = table.0[self.peek(LOOKUP_BITS) as usize];
        self.read(len as u32)?;
        if value == ESCAPE { Some(self.read(escape_bits)? as i32 - bias) } else { Some(value as i32) }
    }

    fn coefficients(&mut self, table: &Lookup, escape_bits: u32, bias: i32) -> Option<[i32; 3]> {
        Some([self.value(table, escape_bits, bias)?, self.value(table, escape_bits, bias)?, self.value(table, escape_bits, bias)?])
    }
}

/// Rebuild one channel of a 2x2 block from the smaller level's value and the
/// (horizontal, vertical, diagonal) coefficients.
fn reconstruct(src: u8, dst: &mut [u8], at: usize, bpp: usize, stride: usize, parity: u32, [h, v, d]: [i32; 3]) {
    let base = 2 * src as i32;
    dst[at] = (parity as i32 + ((d + v + h + base) >> 1)) as u8;
    dst[at + bpp] = ((h + base - d - v) >> 1) as u8;
    dst[at + stride] = ((v - d + base - h) >> 1) as u8;
    dst[at + stride + bpp] = ((base - h - v + d) >> 1) as u8;
}

/// Decode a wavelet texture (the data after the iwi header) to RGBA8 mip
/// levels, largest first. Sizes must be powers of two.
pub fn decode(format: WaveletFormat, width: u32, height: u32, data: &[u8]) -> Option<Vec<Vec<u8>>> {
    if !width.is_power_of_two() || !height.is_power_of_two() {
        return None;
    }
    let [blue, red_green, alpha] = tables();
    let (channels, bpp) = format.layout();
    let count = (width.max(height).ilog2() + 1) as usize;
    let dims = |l: usize| ((width >> l).max(1) as usize, (height >> l).max(1) as usize);
    let mut levels: Vec<Vec<u8>> = vec![Vec::new(); count];
    let mut bits = Bits { data, byte: 0, bit: None };
    for l in (0..count).rev() {
        let (w, h) = dims(l);
        let mut dst = vec![0u8; w * h * bpp];
        if w <= 1 || h <= 1 {
            for p in dst.chunks_exact_mut(bpp) {
                for c in p.iter_mut().take(channels) {
                    *c = bits.raw_byte()?;
                }
                for c in p.iter_mut().skip(channels) {
                    *c = 255;
                }
            }
        } else {
            let mut src = levels[l + 1].clone();
            if bits.read(1)? == 1 {
                for p in src.chunks_exact_mut(bpp) {
                    for c in p.iter_mut().take(channels) {
                        *c = c.wrapping_add(bits.value(alpha, 9, 255)? as u8);
                    }
                }
            }
            let stride = w * bpp;
            for y in (0..h).step_by(2) {
                for x in (0..w).step_by(2) {
                    let s = &src[((y / 2) * (w / 2) + x / 2) * bpp..];
                    let at = (y * w + x) * bpp;
                    if channels != 1 {
                        let parity = bits.read(1)?;
                        let b = bits.coefficients(blue, 9, 0xFF)?;
                        reconstruct(s[0], &mut dst, at, bpp, stride, parity, b);
                        if channels >= 3 {
                            for c in 1..=2 {
                                let parity = bits.read(1)?;
                                let k = bits.coefficients(red_green, 10, 0x1FE)?;
                                let k = [k[0] + b[0], k[1] + b[1], k[2] + b[2]];
                                reconstruct(s[c], &mut dst, at + c, bpp, stride, parity, k);
                            }
                        }
                    }
                    if channels == 3 {
                        for o in [0, bpp, stride, stride + bpp] {
                            dst[at + o + 3] = 255;
                        }
                    } else {
                        let c = channels - 1;
                        let parity = bits.read(1)?;
                        let k = bits.coefficients(alpha, 9, 0xFF)?;
                        reconstruct(s[c], &mut dst, at + c, bpp, stride, parity, k);
                    }
                }
            }
        }
        levels[l] = dst;
    }
    // To RGBA8. RGB(A) decodes as BGRA.
    Some(
        levels
            .into_iter()
            .map(|lv| {
                lv.chunks_exact(bpp)
                    .flat_map(|p| match format {
                        WaveletFormat::Rgba => [p[2], p[1], p[0], p[3]],
                        WaveletFormat::Rgb => [p[2], p[1], p[0], 255],
                        WaveletFormat::LuminanceAlpha => [p[0], p[0], p[0], p[1]],
                        WaveletFormat::Luminance => [p[0], p[0], p[0], 255],
                        WaveletFormat::Alpha => [255, 255, 255, p[0]],
                    })
                    .collect()
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each table is a complete prefix code, and every non-zero value sits
    /// next to its negation (same code with the top bit flipped), which
    /// catches transcription slips in the values.
    #[test]
    fn code_tables_are_consistent() {
        for (name, codes) in [("blue", &BLUE[..]), ("red_green", &RED_GREEN[..]), ("alpha", &ALPHA[..])] {
            let kraft: f64 = codes.iter().map(|&(_, len, _)| 0.5f64.powi(len as i32)).sum();
            assert!((kraft - 1.0).abs() < 1e-12, "{name}: Kraft sum {kraft}");
            let t = Lookup::build(codes);
            assert!(t.0.iter().all(|&(_, len)| len > 0), "{name}: incomplete");
            for &(code, len, value) in codes {
                assert!(code < (1 << len), "{name}: {code:#x} longer than {len} bits");
                // No code is a prefix of another.
                for &(c2, l2, _) in codes {
                    if (c2, l2) != (code, len) && l2 >= len {
                        assert_ne!(c2 & ((1 << len) - 1), code, "{name}: {code:#x}/{len} prefixes {c2:#x}/{l2}");
                    }
                }
                if value != 0 && value != ESCAPE {
                    let mirror = code ^ (1 << (len - 1));
                    assert!(
                        codes.iter().any(|&(c, l, v)| c == mirror && l == len && v == -value),
                        "{name}: {value} at {code:#x}/{len} has no negation"
                    );
                }
            }
        }
    }

    #[test]
    fn decodes_a_flat_image() {
        // 2x2 luminance: raw 1x1 level (100), no delta, zero coefficients.
        // Bits (LSB first): delta flag 0, parity 0, then three ALPHA zeros
        // (code 1, length 1).
        let data = [100u8, 0b0001_1100];
        let levels = decode(WaveletFormat::Luminance, 2, 2, &data).unwrap();
        assert_eq!(levels[1], vec![100, 100, 100, 255]);
        assert_eq!(levels[0], [100, 100, 100, 255].repeat(4));
    }
}

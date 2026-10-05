//! Decoders for IW3's packed vertex attributes.

/// `PackedUnitVec`: three biased bytes plus a per-vector scale byte.
pub fn unit_vec(packed: u32) -> [f32; 3] {
    let b = packed.to_le_bytes();
    let scale = (b[3] as f32 - -192.0) / 32385.0;
    [(b[0] as f32 - 127.0) * scale, (b[1] as f32 - 127.0) * scale, (b[2] as f32 - 127.0) * scale]
}

/// `GfxColor`: stored as BGRA bytes.
pub fn color(packed: u32) -> [f32; 4] {
    let b = packed.to_le_bytes();
    [b[2] as f32 / 255.0, b[1] as f32 / 255.0, b[0] as f32 / 255.0, b[3] as f32 / 255.0]
}

/// `PackedTexCoords`: two IEEE half floats, high word = u, low word = v.
pub fn tex_coords(packed: u32) -> [f32; 2] {
    [half_to_f32((packed >> 16) as u16), half_to_f32(packed as u16)]
}

pub fn half_to_f32(h: u16) -> f32 {
    let sign = ((h as u32) & 0x8000) << 16;
    let exp = ((h >> 10) & 0x1f) as u32;
    let mant = (h & 0x3ff) as u32;
    let bits = match exp {
        0 if mant == 0 => sign,
        0 => {
            // Subnormal: renormalise.
            let mut e = 127 - 15 + 1;
            let mut m = mant;
            while m & 0x400 == 0 {
                m <<= 1;
                e -= 1;
            }
            sign | (e << 23) | ((m & 0x3ff) << 13)
        }
        31 => sign | 0x7f80_0000 | (mant << 13),
        _ => sign | ((exp + 127 - 15) << 23) | (mant << 13),
    };
    f32::from_bits(bits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_floats() {
        assert_eq!(half_to_f32(0x3c00), 1.0);
        assert_eq!(half_to_f32(0xc000), -2.0);
        assert_eq!(half_to_f32(0x3800), 0.5);
        assert_eq!(half_to_f32(0), 0.0);
    }

    #[test]
    fn unit_vec_axis() {
        // Scale byte 63 gives a decode scale of exactly 1/127.
        let v = unit_vec(u32::from_le_bytes([127, 127, 254, 63]));
        assert!(v[0].abs() < 1e-6 && v[1].abs() < 1e-6);
        assert!((v[2] - 1.0).abs() < 0.02, "{v:?}");
    }
}

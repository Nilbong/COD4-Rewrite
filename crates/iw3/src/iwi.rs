//! `.iwi` textures (CoD4 uses version 6, Modern Warfare 2 version 8, Black
//! Ops version 13).
//!
//! Layout: `"IWi"`, version, format, flags, width, height, depth (u16 each),
//! four u32 file offsets, then mip levels stored smallest first. Black Ops
//! adds a float after the depth and has eight offsets. Modern Warfare 2
//! widens the flags to a u32 before the format (and a spare byte after it).

pub const HEADER_SIZE_T5: usize = 48;

use anyhow::{Result, bail};

pub const HEADER_SIZE: usize = 28;
pub const HEADER_SIZE_IW4: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// 8 bits per channel, stored BGRA.
    Argb32,
    /// Stored BGR.
    Rgb24,
    /// Luminance + alpha, 8 bits each.
    La16,
    L8,
    A8,
    Dxt1,
    Dxt3,
    Dxt5,
    /// A wavelet-compressed file (see [`crate::wavelet`]), already decoded
    /// to RGBA8.
    Rgba8,
}

impl Format {
    fn from_u8(v: u8) -> Option<Format> {
        Some(match v {
            0x01 => Format::Argb32,
            0x02 => Format::Rgb24,
            0x03 => Format::La16,
            0x04 => Format::A8,
            0x05 => Format::L8,
            0x0B => Format::Dxt1,
            0x0C => Format::Dxt3,
            0x0D => Format::Dxt5,
            _ => return None,
        })
    }

    pub fn is_compressed(self) -> bool {
        matches!(self, Format::Dxt1 | Format::Dxt3 | Format::Dxt5)
    }

    /// Bytes needed for one 2D level of the given size.
    pub fn level_size(self, w: u32, h: u32) -> usize {
        let (w, h) = (w.max(1) as usize, h.max(1) as usize);
        match self {
            Format::Dxt1 => w.div_ceil(4) * h.div_ceil(4) * 8,
            Format::Dxt3 | Format::Dxt5 => w.div_ceil(4) * h.div_ceil(4) * 16,
            Format::Argb32 | Format::Rgba8 => w * h * 4,
            Format::Rgb24 => w * h * 3,
            Format::La16 => w * h * 2,
            Format::L8 | Format::A8 => w * h,
        }
    }
}

pub const FLAG_NOMIPMAPS: u8 = 0x03;
pub const FLAG_CUBEMAP: u8 = 0x04;
pub const FLAG_VOLMAP: u8 = 0x08;
pub const FLAG_NORMALMAP: u8 = 0x20;

/// MW2's (u32) image flags as CoD4's: no mip maps 0x2, cube 0x10000,
/// volume 0x20000, normal map 0x40000.
fn iw4_flags(f: u32) -> u8 {
    let mut out = 0;
    if f & 0x2 != 0 {
        out |= 0x02;
    }
    match f & 0x30000 {
        0x10000 => out |= FLAG_CUBEMAP,
        0x20000 => out |= FLAG_VOLMAP,
        _ => {}
    }
    if f & 0x40000 != 0 {
        out |= FLAG_NORMALMAP;
    }
    out
}

#[derive(Debug)]
pub struct Iwi {
    pub format: Format,
    pub flags: u8,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    /// Mip levels, largest first. Cube maps hold six faces per level.
    pub levels: Vec<Vec<u8>>,
}

impl Iwi {
    pub fn is_cube(&self) -> bool {
        self.flags & FLAG_CUBEMAP != 0
    }

    pub fn parse(data: &[u8]) -> Result<Iwi> {
        if data.len() < HEADER_SIZE || &data[0..3] != b"IWi" {
            bail!("not an iwi file");
        }
        let version = data[3];
        // Where the format, flags and dimensions are, and the header's size.
        let (header, format_at, flags, dims_at) = match version {
            6 => (HEADER_SIZE, 4, data[5], 6),
            8 if data.len() >= HEADER_SIZE_IW4 => (HEADER_SIZE_IW4, 8, iw4_flags(u32::from_le_bytes([data[4], data[5], data[6], data[7]])), 10),
            13 if data.len() >= HEADER_SIZE_T5 => (HEADER_SIZE_T5, 4, data[5], 6),
            _ => bail!("unsupported iwi version {version} (CoD4 uses 6, MW2 8, Black Ops 13)"),
        };
        let format_byte = data[format_at];
        let rd16 = |o: usize| u16::from_le_bytes([data[o], data[o + 1]]) as u32;
        let (width, height, depth) = (rd16(dims_at), rd16(dims_at + 2), rd16(dims_at + 4).max(1));
        if let Some(wf) = crate::wavelet::WaveletFormat::from_u8(format_byte) {
            if flags & FLAG_CUBEMAP != 0 || depth > 1 {
                bail!("wavelet cube and volume textures are unsupported");
            }
            let Some(levels) = crate::wavelet::decode(wf, width, height, &data[header..]) else {
                bail!("bad wavelet data");
            };
            return Ok(Iwi { format: Format::Rgba8, flags, width, height, depth, levels });
        }
        let Some(format) = Format::from_u8(format_byte) else {
            bail!("unsupported iwi format {format_byte:#x}");
        };
        let faces = if flags & FLAG_CUBEMAP != 0 { 6 } else { 1 };

        // Work out how many levels fit: with mips, every level down to 1x1.
        let body = &data[header..];
        let mut sizes = Vec::new();
        let (mut w, mut h) = (width, height);
        loop {
            sizes.push(format.level_size(w, h) * faces * depth as usize);
            if flags & FLAG_NOMIPMAPS != 0 || (w == 1 && h == 1) {
                break;
            }
            w = (w / 2).max(1);
            h = (h / 2).max(1);
        }
        let total: usize = sizes.iter().sum();
        if body.len() < total {
            // Some files only carry the top level despite the flag.
            if body.len() >= sizes[0] {
                sizes.truncate(1);
            } else {
                bail!("iwi body is {} bytes, expected {total}", body.len());
            }
        }

        // Stored smallest level first; the largest level ends the file.
        let mut levels = Vec::with_capacity(sizes.len());
        let mut end = body.len();
        for &size in &sizes {
            levels.push(body[end - size..end].to_vec());
            end -= size;
        }
        Ok(Iwi { format, flags, width, height, depth, levels })
    }

    /// Decode level 0 to RGBA8. Compressed formats are left to the GPU.
    pub fn to_rgba8(&self, level: usize) -> Option<Vec<u8>> {
        let src = self.levels.get(level)?;
        let mut out = Vec::with_capacity(src.len() * 4);
        match self.format {
            Format::Argb32 => {
                for p in src.chunks_exact(4) {
                    out.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
                }
            }
            Format::Rgb24 => {
                for p in src.chunks_exact(3) {
                    out.extend_from_slice(&[p[2], p[1], p[0], 255]);
                }
            }
            Format::La16 => {
                for p in src.chunks_exact(2) {
                    out.extend_from_slice(&[p[0], p[0], p[0], p[1]]);
                }
            }
            Format::L8 => {
                for &l in src {
                    out.extend_from_slice(&[l, l, l, 255]);
                }
            }
            Format::A8 => {
                for &a in src {
                    out.extend_from_slice(&[255, 255, 255, a]);
                }
            }
            Format::Rgba8 => out.extend_from_slice(src),
            _ => return None,
        }
        Some(out)
    }
}

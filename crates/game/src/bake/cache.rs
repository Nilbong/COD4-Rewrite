//! The bake's cache: `<root>\<map>\v<VERSION>\` (see [`root`]).
//!
//! `light.bin` (deflated): a header, then each lightmap atlas, then the
//! light grid (one ambient cube per CoD4 grid point, half floats). In
//! memory an atlas is `w` x `2h` RGBA half floats laid out like IW3's
//! `lightmapN_secondary` (layer A on top: the even light and the
//! direction's x in alpha; layer B below: the directional light and the
//! direction's y), linear, in lightmap units. On disk each layer's colour
//! channels are 11-bit gamma codes and its direction 8 bits, every plane
//! predicted from its neighbours as PNG's Paeth filter does, so deflate
//! has little left to store (~10-15 MB a map rather than ~35). The codes
//! are within ~1% of the baked light. `bake.json` describes the bake.

use std::io::{Read, Write};
use std::path::PathBuf;

/// Bumped whenever the bake or its format changes, so old bakes are
/// ignored (and rebaked).
pub const VERSION: u32 = 2;
const MAGIC: &[u8; 4] = b"C4RB";

/// Light values are coded up to this (lightmap units; maps peak near 15).
const CODE_MAX: f32 = 24.0;
/// Colour code steps (11 bits: steps of ~0.5% of a value, below what shows;
/// 12 bits mostly stored the path tracer's noise).
const CODE_STEPS: f32 = 2047.0;

pub struct BakedAtlas {
    /// One layer's size (the image is `w` x `2h`).
    pub w: u32,
    pub h: u32,
    /// RGBA f16 bits, `w * 2h * 4`.
    pub data: Vec<u16>,
}

pub struct Baked {
    /// Indexed like `GfxWorld::lightmaps`; `None` where nothing was baked.
    pub atlases: Vec<Option<BakedAtlas>>,
    /// Grid point (grid coordinates) and its light on surfaces facing +x,
    /// -x, +y, -y, +z, -z (CoD axes), linear lightmap units.
    pub grid: Vec<([u32; 3], [[f32; 3]; 6])>,
}

/// Where bakes are kept: `%LOCALAPPDATA%/cod4rw/bake`, or the folder named
/// in `%LOCALAPPDATA%/cod4rw/bake_dir.txt` (another drive, when the system
/// drive is short of space).
pub fn root() -> Option<PathBuf> {
    let app = PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("cod4rw");
    let chosen = std::fs::read_to_string(app.join("bake_dir.txt")).ok().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
    Some(chosen.map_or_else(|| app.join("bake"), PathBuf::from))
}

/// The bake profile (`COD4RW_BAKE_PROFILE`, e.g. `safe`): its bakes are
/// kept apart from the default ones, for comparing. Read by the bake and the
/// game alike.
pub fn profile() -> Option<String> {
    // ("bold" is the default's own name now.)
    std::env::var("COD4RW_BAKE_PROFILE").ok().map(|p| p.trim().to_ascii_lowercase()).filter(|p| !p.is_empty() && p != "default" && p != "bold")
}

pub fn dir(map: &str) -> Option<PathBuf> {
    dir_for(map, profile().as_deref())
}

/// The cache folder of a variant of a map's bake (a profile, or a
/// time-of-day keyframe: `crate::bake::tod`); `None`: the default bake.
pub fn dir_for(map: &str, variant: Option<&str>) -> Option<PathBuf> {
    let name = match variant {
        Some(v) => format!("v{VERSION}-{v}"),
        None => format!("v{VERSION}"),
    };
    Some(root()?.join(map).join(name))
}

/// What the bake was made from: the zone file's size and time, so a
/// changed map is rebaked.
pub fn source_stamp(zone: &std::path::Path) -> (u64, u64) {
    let meta = std::fs::metadata(zone).ok();
    let len = meta.as_ref().map_or(0, |m| m.len());
    let time = meta.and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    (len, time)
}

fn colour_code(v: f32) -> i32 {
    ((v / CODE_MAX).clamp(0.0, 1.0).powf(1.0 / 2.2) * CODE_STEPS).round() as i32
}

fn colour_value(q: i32) -> f32 {
    (q as f32 / CODE_STEPS).powf(2.2) * CODE_MAX
}

fn dir_code(v: f32) -> i32 {
    ((v.clamp(-1.0, 1.0) * 0.5 + 0.5) * 255.0).round() as i32
}

fn dir_value(q: i32) -> f32 {
    q as f32 / 255.0 * 2.0 - 1.0
}

/// PNG's Paeth predictor of a value from its left, upper and upper-left
/// neighbours.
fn paeth(l: i32, u: i32, ul: i32) -> i32 {
    let p = l + u - ul;
    let (pa, pb, pc) = ((p - l).abs(), (p - u).abs(), (p - ul).abs());
    if pa <= pb && pa <= pc {
        l
    } else if pb <= pc {
        u
    } else {
        ul
    }
}

/// Append a plane of codes as Paeth residuals, zigzagged, low bytes then
/// high bytes.
fn put_plane(out: &mut Vec<u8>, q: &[i32], w: usize) {
    let at = |i: usize, dx: usize, dy: usize| -> i32 {
        let (x, y) = (i % w, i / w);
        if x < dx || y < dy { 0 } else { q[(y - dy) * w + x - dx] }
    };
    let z: Vec<u16> = (0..q.len())
        .map(|i| {
            let r = q[i] - paeth(at(i, 1, 0), at(i, 0, 1), at(i, 1, 1));
            ((r << 1) ^ (r >> 31)) as u16
        })
        .collect();
    out.extend(z.iter().map(|v| *v as u8));
    out.extend(z.iter().map(|v| (v >> 8) as u8));
}

fn get_plane(r: &mut Reader, w: usize, h: usize) -> Option<Vec<i32>> {
    let n = w * h;
    let lo = r.take(n)?;
    let hi = r.take(n)?;
    let mut q = vec![0i32; n];
    for i in 0..n {
        let z = lo[i] as u32 | (hi[i] as u32) << 8;
        let res = (z >> 1) as i32 ^ -((z & 1) as i32);
        let (x, y) = (i % w, i / w);
        let l = if x > 0 { q[i - 1] } else { 0 };
        let u = if y > 0 { q[i - w] } else { 0 };
        let ul = if x > 0 && y > 0 { q[i - w - 1] } else { 0 };
        q[i] = res + paeth(l, u, ul);
    }
    Some(q)
}

pub fn save(map: &str, stamp: (u64, u64), baked: &Baked, meta: &str) -> std::io::Result<PathBuf> {
    save_in(dir(map).ok_or_else(|| std::io::Error::other("no LOCALAPPDATA"))?, stamp, baked, meta)
}

pub fn save_in(dir: PathBuf, stamp: (u64, u64), baked: &Baked, meta: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(&dir)?;
    let mut raw = Vec::new();
    raw.extend_from_slice(MAGIC);
    raw.extend_from_slice(&VERSION.to_le_bytes());
    raw.extend_from_slice(&stamp.0.to_le_bytes());
    raw.extend_from_slice(&stamp.1.to_le_bytes());
    raw.extend_from_slice(&(baked.atlases.len() as u32).to_le_bytes());
    for a in &baked.atlases {
        let (w, h) = a.as_ref().map_or((0, 0), |a| (a.w, a.h));
        raw.extend_from_slice(&w.to_le_bytes());
        raw.extend_from_slice(&h.to_le_bytes());
        let Some(a) = a else { continue };
        let (w, h) = (w as usize, h as usize);
        let value = |layer: usize, i: usize, k: usize| half::f16::from_bits(a.data[(layer * w * h + i) * 4 + k]).to_f32();
        for layer in 0..2 {
            for k in 0..4 {
                let q: Vec<i32> = (0..w * h).map(|i| if k < 3 { colour_code(value(layer, i, k)) } else { dir_code(value(layer, i, k)) }).collect();
                put_plane(&mut raw, &q, w);
            }
        }
    }
    raw.extend_from_slice(&(baked.grid.len() as u32).to_le_bytes());
    for (p, cube) in &baked.grid {
        for v in p {
            raw.extend_from_slice(&v.to_le_bytes());
        }
        for f in cube.iter().flatten() {
            raw.extend_from_slice(&half::f16::from_f32(*f).to_bits().to_le_bytes());
        }
    }
    let tmp = dir.join("light.bin.tmp");
    {
        let mut enc = flate2::write::DeflateEncoder::new(std::fs::File::create(&tmp)?, flate2::Compression::best());
        enc.write_all(&raw)?;
        enc.finish()?;
    }
    std::fs::rename(&tmp, dir.join("light.bin"))?;
    std::fs::write(dir.join("bake.json"), meta)?;
    Ok(dir)
}

/// The cached bake of `map`, if there is one made from this `stamp`.
pub fn load(map: &str, stamp: (u64, u64)) -> Option<Baked> {
    load_in(&dir(map)?, stamp)
}

pub fn load_in(dir: &std::path::Path, stamp: (u64, u64)) -> Option<Baked> {
    let file = std::fs::File::open(dir.join("light.bin")).ok()?;
    let mut raw = Vec::new();
    flate2::read::DeflateDecoder::new(file).read_to_end(&mut raw).ok()?;
    let mut r = Reader { b: &raw, at: 0 };
    if r.take(4)? != MAGIC || r.u32()? != VERSION || r.u64()? != stamp.0 || r.u64()? != stamp.1 {
        return None;
    }
    let n = r.u32()? as usize;
    let mut atlases = Vec::with_capacity(n);
    for _ in 0..n {
        let (w, h) = (r.u32()? as usize, r.u32()? as usize);
        if w == 0 {
            atlases.push(None);
            continue;
        }
        let mut data = vec![0u16; w * h * 2 * 4];
        for layer in 0..2 {
            for k in 0..4 {
                let q = get_plane(&mut r, w, h)?;
                for (i, q) in q.into_iter().enumerate() {
                    let v = if k < 3 { colour_value(q) } else { dir_value(q) };
                    data[(layer * w * h + i) * 4 + k] = half::f16::from_f32(v).to_bits();
                }
            }
        }
        atlases.push(Some(BakedAtlas { w: w as u32, h: h as u32, data }));
    }
    let n = r.u32()? as usize;
    let mut grid = Vec::with_capacity(n);
    for _ in 0..n {
        let p = [r.u32()?, r.u32()?, r.u32()?];
        let mut cube = [[0.0; 3]; 6];
        for f in cube.iter_mut().flatten() {
            *f = half::f16::from_bits(u16::from_le_bytes(r.take(2)?.try_into().ok()?)).to_f32();
        }
        grid.push((p, cube));
    }
    Some(Baked { atlases, grid })
}

/// A version-1 `light.bin`, with the stamp it was made from.
pub fn load_v1(path: &std::path::Path) -> Option<((u64, u64), Baked)> {
    let mut raw = Vec::new();
    flate2::read::DeflateDecoder::new(std::fs::File::open(path).ok()?).read_to_end(&mut raw).ok()?;
    let mut r = Reader { b: &raw, at: 0 };
    if r.take(4)? != MAGIC || r.u32()? != 1 {
        return None;
    }
    let stamp = (r.u64()?, r.u64()?);
    let n = r.u32()? as usize;
    let mut atlases = Vec::with_capacity(n);
    for _ in 0..n {
        let (w, h) = (r.u32()?, r.u32()?);
        if w == 0 {
            atlases.push(None);
            continue;
        }
        let len = w as usize * h as usize * 2 * 4;
        let data = r.take(len * 2)?.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        atlases.push(Some(BakedAtlas { w, h, data }));
    }
    let n = r.u32()? as usize;
    let mut grid = Vec::with_capacity(n);
    for _ in 0..n {
        let p = [r.u32()?, r.u32()?, r.u32()?];
        let mut cube = [[0.0; 3]; 6];
        for f in cube.iter_mut().flatten() {
            *f = r.f32()?;
        }
        grid.push((p, cube));
    }
    Some((stamp, Baked { atlases, grid }))
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.at..self.at + n)?;
        self.at += n;
        Some(s)
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn f32(&mut self) -> Option<f32> {
        Some(f32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
}

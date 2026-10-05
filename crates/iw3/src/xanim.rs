//! Decoding and sampling `XAnimParts` (skeletal animations).
//!
//! An animation stores, per bone, a rotation track and a translation track
//! in one of several compressed forms. The data is packed into shared
//! streams (`dataByte`, `dataShort`, `dataInt`, the `randomData*` pools and
//! `indices`) that are consumed in a fixed order: rotation tracks grouped by
//! type, then translation tracks (each prefixed with its bone index).
//! Layout from the OpenAssetTools flat XAnim reader.

use crate::zone::generic::GNode;
use anyhow::{Result, anyhow, bail};

#[derive(Debug, Clone)]
pub enum RotTrack {
    /// Bone keeps the identity rotation.
    None,
    /// Keyed quaternions (x, y, z, w), already normalised. `frames` holds the
    /// key frame numbers, empty for a constant.
    Keys { frames: Vec<u16>, values: Vec<[f32; 4]> },
}

#[derive(Debug, Clone)]
pub enum TransTrack {
    /// Bone keeps its model translation.
    None,
    Keys { frames: Vec<u16>, values: Vec<[f32; 3]> },
}

#[derive(Debug, Clone)]
pub struct BoneTrack {
    pub name: String,
    pub rot: RotTrack,
    pub trans: TransTrack,
}

#[derive(Debug, Clone)]
pub struct Notify {
    pub name: String,
    /// Fraction of the animation (0..1).
    pub time: f32,
}

#[derive(Debug, Clone)]
pub struct XAnim {
    pub name: String,
    pub num_frames: u16,
    pub framerate: f32,
    pub looping: bool,
    pub delta: bool,
    pub bones: Vec<BoneTrack>,
    pub notifies: Vec<Notify>,
    /// Root motion (`deltaPart` translation), in the animation's own axes:
    /// x forward, y left, z up.
    pub delta_trans: TransTrack,
}

struct Cursor<'a> {
    data_byte: &'a [u8],
    data_short: &'a [u8],
    data_int: &'a [u8],
    random_byte: &'a [u8],
    random_short: &'a [u8],
    indices: &'a [u8],
}

fn take<'a>(s: &mut &'a [u8], n: usize, what: &str) -> Result<&'a [u8]> {
    if s.len() < n {
        bail!("xanim {what} stream exhausted ({} < {n})", s.len());
    }
    let (a, b) = s.split_at(n);
    *s = b;
    Ok(a)
}

impl Cursor<'_> {
    fn byte(&mut self) -> Result<u8> {
        Ok(take(&mut self.data_byte, 1, "dataByte")?[0])
    }
    fn short(&mut self) -> Result<i16> {
        let b = take(&mut self.data_short, 2, "dataShort")?;
        Ok(i16::from_le_bytes([b[0], b[1]]))
    }
    fn shorts(&mut self, n: usize) -> Result<Vec<i16>> {
        let b = take(&mut self.data_short, n * 2, "dataShort")?;
        Ok(b.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect())
    }
    fn float3(&mut self) -> Result<[f32; 3]> {
        let b = take(&mut self.data_int, 12, "dataInt")?;
        Ok([0, 4, 8].map(|o| f32::from_le_bytes(b[o..o + 4].try_into().unwrap())))
    }
    fn random_shorts(&mut self, n: usize) -> Result<Vec<i16>> {
        let b = take(&mut self.random_short, n * 2, "randomDataShort")?;
        Ok(b.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect())
    }
    fn random_bytes(&mut self, n: usize) -> Result<&[u8]> {
        take(&mut self.random_byte, n, "randomDataByte")
    }

    /// Key frame numbers for a track with `stored + 1` keys.
    fn packed_indices(&mut self, stored: u16, byte_indices: bool) -> Result<Vec<u16>> {
        let count = stored as usize + 1;
        if byte_indices {
            return Ok(take(&mut self.data_byte, count, "dataByte")?.iter().map(|&b| b as u16).collect());
        }
        if stored >= 64 {
            let b = take(&mut self.indices, count * 2, "indices")?;
            // The game also stores checkpoints (every 256th index and the
            // last) in dataShort.
            take(&mut self.data_short, ((count - 2) / 256 + 2) * 2, "dataShort")?;
            return Ok(b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect());
        }
        Ok(self.shorts(count)?.into_iter().map(|v| v as u16).collect())
    }
}

fn quat(v: &[i16]) -> [f32; 4] {
    let q = [v[0] as f32, v[1] as f32, v[2] as f32, v[3] as f32];
    normalize(q)
}

/// Half quaternions only rotate about Z: (0, 0, z, w).
fn half_quat(v: &[i16]) -> [f32; 4] {
    normalize([0.0, 0.0, v[0] as f32, v[1] as f32])
}

fn normalize(q: [f32; 4]) -> [f32; 4] {
    let len = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if len < 1e-9 { [0.0, 0.0, 0.0, 1.0] } else { q.map(|c| c / len) }
}

impl XAnim {
    /// Decode an animation from its generically loaded `XAnimParts` node.
    pub fn from_node(name: &str, node: &GNode, script_strings: &[String]) -> Result<XAnim> {
        let num_frames = node.int("numframes") as u16;
        let byte_indices = num_frames < 256;
        let counts: Vec<usize> = (0..10).map(|i| node.int(&format!("boneCount[{i}]")) as usize).collect();
        let bone_names: Vec<String> = node
            .bytes("names")
            .chunks_exact(2)
            .map(|c| script_strings.get(u16::from_le_bytes([c[0], c[1]]) as usize).cloned().unwrap_or_default())
            .collect();
        let total = counts[9];
        if bone_names.len() != total {
            bail!("{name}: {} bone names but boneCount[ALL] = {total}", bone_names.len());
        }
        let indices_node = node.node("indices");
        let indices = indices_node.map_or(&[][..], |n| if !n.bytes("_2").is_empty() { n.bytes("_2") } else { n.bytes("_1") });
        let mut c = Cursor {
            data_byte: node.bytes("dataByte"),
            data_short: node.bytes("dataShort"),
            data_int: node.bytes("dataInt"),
            random_byte: node.bytes("randomDataByte"),
            random_short: node.bytes("randomDataShort"),
            indices,
        };

        let mut bones: Vec<BoneTrack> = bone_names
            .into_iter()
            .map(|name| BoneTrack { name, rot: RotTrack::None, trans: TransTrack::None })
            .collect();
        let mut b = 0usize;
        // Rotations, grouped by type in bone order.
        b += counts[0];
        for _ in 0..counts[1] {
            let stored = c.short()? as u16;
            let frames = c.packed_indices(stored, byte_indices)?;
            let raw = c.random_shorts((stored as usize + 1) * 2)?;
            let values = raw.chunks_exact(2).map(half_quat).collect();
            bones[b].rot = RotTrack::Keys { frames, values };
            b += 1;
        }
        for _ in 0..counts[2] {
            let stored = c.short()? as u16;
            let frames = c.packed_indices(stored, byte_indices)?;
            let raw = c.random_shorts((stored as usize + 1) * 4)?;
            let values = raw.chunks_exact(4).map(quat).collect();
            bones[b].rot = RotTrack::Keys { frames, values };
            b += 1;
        }
        for _ in 0..counts[3] {
            let v = c.shorts(2)?;
            bones[b].rot = RotTrack::Keys { frames: Vec::new(), values: vec![half_quat(&v)] };
            b += 1;
        }
        for _ in 0..counts[4] {
            let v = c.shorts(4)?;
            bones[b].rot = RotTrack::Keys { frames: Vec::new(), values: vec![quat(&v)] };
            b += 1;
        }
        // Translations, each prefixed with its bone index.
        let bone_at = |c: &mut Cursor, total: usize| -> Result<usize> {
            let i = c.byte()? as usize;
            if i >= total { Err(anyhow!("{name}: translation bone {i} out of range")) } else { Ok(i) }
        };
        for small in [true, false] {
            let n = if small { counts[5] } else { counts[6] };
            for _ in 0..n {
                let bone = bone_at(&mut c, total)?;
                let stored = c.short()? as u16;
                let mins = c.float3()?;
                let size = c.float3()?;
                let frames = c.packed_indices(stored, byte_indices)?;
                let keys = stored as usize + 1;
                let values: Vec<[f32; 3]> = if small {
                    c.random_bytes(keys * 3)?
                        .chunks_exact(3)
                        .map(|v| [0, 1, 2].map(|k| mins[k] + size[k] * v[k] as f32))
                        .collect()
                } else {
                    c.random_shorts(keys * 3)?
                        .chunks_exact(3)
                        .map(|v| [0, 1, 2].map(|k| mins[k] + size[k] * (v[k] as u16) as f32))
                        .collect()
                };
                bones[bone].trans = TransTrack::Keys { frames, values };
            }
        }
        for _ in 0..counts[7] {
            let bone = bone_at(&mut c, total)?;
            let v = c.float3()?;
            bones[bone].trans = TransTrack::Keys { frames: Vec::new(), values: vec![v] };
        }
        for _ in 0..counts[8] {
            bone_at(&mut c, total)?;
        }
        let left = [c.data_byte.len(), c.data_short.len(), c.data_int.len(), c.random_byte.len(), c.random_short.len(), c.indices.len()];
        if left.iter().any(|&n| n > 0) {
            bail!("{name}: undecoded xanim data left over {left:?}");
        }

        let notifies = node
            .nodes("notify")
            .iter()
            .map(|n| Notify {
                name: script_strings.get(n.int("name") as usize).cloned().unwrap_or_default(),
                time: n.float("time"),
            })
            .collect();
        let delta_trans = node
            .node("deltaPart")
            .and_then(|d| d.node("trans"))
            .map_or(TransTrack::None, |t| delta_trans(t, num_frames));
        Ok(XAnim {
            name: name.to_owned(),
            num_frames,
            framerate: node.float("framerate"),
            looping: node.int("bLoop") != 0,
            delta: node.int("bDelta") != 0,
            bones,
            notifies,
            delta_trans,
        })
    }

    /// Root motion from the start to `frac` (0..1) of the way through, in
    /// the animation's axes (`XAnimGetAbsDelta`).
    pub fn delta_at(&self, frac: f32) -> [f32; 3] {
        // Frames run 0..=numframes.
        self.delta_trans.sample(frac.clamp(0.0, 1.0) * self.num_frames as f32).unwrap_or_default()
    }

    /// Length in seconds.
    pub fn duration(&self) -> f32 {
        if self.framerate > 0.0 { self.num_frames as f32 / self.framerate } else { 0.0 }
    }

    /// Frame position for a time in seconds (wrapping for looping anims).
    pub fn frame_at(&self, t: f32) -> f32 {
        let f = t * self.framerate;
        let n = self.num_frames.max(1) as f32;
        if self.looping { f.rem_euclid(n) } else { f.clamp(0.0, n) }
    }
}

/// An `XAnimPartTrans`: `size` u16, `smallTrans` u8, then either one
/// translation (`size` 0) or `mins` and `size` vectors scaling `size + 1`
/// quantised keys (u8 when small, else u16), with their frame numbers
/// embedded after (u8 under 256 frames) unless every frame has a key.
fn delta_trans(t: &GNode, num_frames: u16) -> TransTrack {
    let d = &t.data;
    let f32_at = |o: usize| d.get(o..o + 4).map_or(0.0, |b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    if d.len() < 16 {
        return TransTrack::None;
    }
    let size = u16::from_le_bytes([d[0], d[1]]) as usize;
    let small = d[2] != 0;
    if size == 0 {
        return TransTrack::Keys { frames: Vec::new(), values: vec![[f32_at(4), f32_at(8), f32_at(12)]] };
    }
    let keys = size + 1;
    let mins = [f32_at(4), f32_at(8), f32_at(12)];
    let range = [f32_at(16), f32_at(20), f32_at(24)];
    let raw = t.node("u").and_then(|u| u.node("frames")).and_then(|f| f.node("frames"));
    let values: Vec<[f32; 3]> = match raw {
        Some(n) if small => n.bytes("_1").chunks_exact(3).map(|v| [0, 1, 2].map(|k| mins[k] + range[k] * v[k] as f32)).collect(),
        Some(n) => n
            .bytes("_2")
            .chunks_exact(6)
            .map(|v| [0, 1, 2].map(|k| mins[k] + range[k] * u16::from_le_bytes([v[2 * k], v[2 * k + 1]]) as f32))
            .collect(),
        None => Vec::new(),
    };
    if values.len() != keys {
        return TransTrack::None;
    }
    let frames: Vec<u16> = if num_frames < 256 {
        d.get(32..32 + keys).map(|b| b.iter().map(|&f| f as u16).collect()).unwrap_or_default()
    } else {
        d.get(32..32 + 2 * keys).map(|b| b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect()).unwrap_or_default()
    };
    let frames = if frames.len() == keys { frames } else { (0..keys as u16).collect() };
    TransTrack::Keys { frames, values }
}

fn key_span(frames: &[u16], f: f32) -> (usize, usize, f32) {
    if frames.len() <= 1 {
        return (0, 0, 0.0);
    }
    // Last key whose frame <= f.
    let i = frames.partition_point(|&k| (k as f32) <= f).saturating_sub(1).min(frames.len() - 1);
    if i + 1 >= frames.len() {
        return (i, i, 0.0);
    }
    let (a, b) = (frames[i] as f32, frames[i + 1] as f32);
    let t = if b > a { ((f - a) / (b - a)).clamp(0.0, 1.0) } else { 0.0 };
    (i, i + 1, t)
}

impl RotTrack {
    pub fn sample(&self, f: f32) -> Option<[f32; 4]> {
        match self {
            RotTrack::None => None,
            RotTrack::Keys { frames, values } => {
                let (i, j, t) = key_span(frames, f);
                let (a, mut b) = (values.get(i)?, *values.get(j)?);
                if a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3] < 0.0 {
                    b = b.map(|c| -c);
                }
                Some(normalize([0, 1, 2, 3].map(|k| a[k] + (b[k] - a[k]) * t)))
            }
        }
    }
}

impl TransTrack {
    pub fn sample(&self, f: f32) -> Option<[f32; 3]> {
        match self {
            TransTrack::None => None,
            TransTrack::Keys { frames, values } => {
                let (i, j, t) = key_span(frames, f);
                let (a, b) = (values.get(i)?, values.get(j)?);
                Some([0, 1, 2].map(|k| a[k] + (b[k] - a[k]) * t))
            }
        }
    }
}

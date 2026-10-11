//! Modern Warfare 2's sounds: aliases (`weap_m4carbine_fire_plr`, ...),
//! their sound files, falloff curves and channels.
//!
//! As in CoD4, an alias is a list of variations, each a sound file plus
//! volume, pitch and distance ranges. A file is either *loaded* (the
//! samples are inside the zone) or *streamed* (`sound/<dir>/<name>`, a file
//! in the iwds). Unlike World at War, a loaded sound is not a `.wav` file but
//! Miles' `AILSOUNDINFO` header plus the bare sample data; every one in the
//! multiplayer zones is 16-bit PCM ([`LoadedSound::pcm`], [`decode`]).
//!
//! `snd_alias_t::flags` is CoD4's layout shifted by one bit: bit 0 loops,
//! bits 7..9 are the file's type (1 loaded, 2 streamed; matches
//! `SoundFile::type` on every alias), bits 9..15 the channel, an index into
//! `soundaliases/channels.def` ([`channels`], in `code_post_gfx_mp`), which
//! says whether the channel is positional. The player's own sounds are on 2D
//! channels (`weap_m4carbine_fire_plr` on `local`, its mechanism layer on
//! `local3`, reloads on `reload2d`), other players' on 3D ones (`_npc` fire
//! on `weapon`).
//!
//! Falloff curves are `SoundCurve` assets, named (`default`, `weapon2`,
//! ...); a zone that uses another zone's curve has a `,name` stub without
//! knots, so [`Variant::curve`] is the name and [`curves`] reads the real
//! ones (`common_mp` and `code_post_gfx_mp` hold them).

use crate::zone::{AssetType, GNode, GVal, Zone};
use anyhow::{Result, bail};
use std::collections::HashMap;
use std::sync::Arc;

/// `snd_alias_t::flags`: the sound loops.
const LOOPING: i64 = 0x1;
/// `snd_alias_t::flags >> CHANNEL_SHIFT & 63` is the channel.
const CHANNEL_SHIFT: u32 = 9;
/// `snd_alias_t::flags >> TYPE_SHIFT & 3` is the file type.
const TYPE_SHIFT: u32 = 7;
/// `SoundFile::type` / the flags' type field.
const SAT_LOADED: i64 = 1;

/// A falloff curve (`SndCurve`): (fraction of the way from the near to the
/// far distance, volume) knots.
#[derive(Clone, Debug, Default)]
pub struct Curve {
    pub name: String,
    pub knots: Vec<[f32; 2]>,
}

/// Every curve with knots in a zone, by name.
pub fn curves(zone: &Zone) -> HashMap<String, Curve> {
    zone.of_type(AssetType::SoundCurve)
        .filter(|(_, a)| !a.name.starts_with(','))
        .map(|(_, a)| (a.name.clone(), curve(&a.root, &a.name)))
        .collect()
}

fn curve(n: &GNode, name: &str) -> Curve {
    // (`knots` is float[16][2] at offset 8, read directly: paths don't
    // index two-dimensional arrays.)
    let count = (n.int("knotCount") as usize).min(16);
    let f = |o: usize| n.data.get(o..o + 4).map_or(0.0, |b| f32::from_le_bytes(b.try_into().unwrap()));
    Curve { name: name.to_owned(), knots: (0..count).map(|k| [f(8 + k * 8), f(12 + k * 8)]).collect() }
}

/// A channel (`soundaliases/channels.def`).
#[derive(Clone, Debug)]
pub struct Channel {
    /// `weapon`, `weapon2d`, `voice`, `music`, ...
    pub name: String,
    pub spatial: bool,
}

/// The channels, by index, from the `soundaliases/channels.def` raw file
/// (in `code_post_gfx_mp`). Each line is `name, priority, 3d|2d, ...`;
/// `#` starts a comment.
pub fn channels(zone: &Zone) -> Option<Vec<Channel>> {
    let (_, a) = zone.of_type(AssetType::RawFile).find(|(_, a)| a.name.eq_ignore_ascii_case("soundaliases/channels.def"))?;
    let text = raw_file(&a.root)?;
    let text = String::from_utf8_lossy(&text);
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let cols: Vec<&str> = line.split(',').map(str::trim).collect();
        if cols[0].is_empty() {
            continue;
        }
        // (A missing is3d column means 2D.)
        out.push(Channel { name: cols[0].to_owned(), spatial: cols.get(2).is_some_and(|c| c.eq_ignore_ascii_case("3d")) });
    }
    Some(out)
}

/// A raw file's contents, inflated when compressed.
fn raw_file(n: &GNode) -> Option<Vec<u8>> {
    let d = n.node("data")?;
    let field = if n.int("compressedLen") > 0 { "compressedBuffer" } else { "buffer" };
    let data = match d.field(field) {
        Some(GVal::Bytes(b)) => b.clone(),
        Some(GVal::Nodes(ns)) => ns.iter().flat_map(|c| c.data.iter().copied()).collect(),
        _ => return None,
    };
    if n.int("compressedLen") <= 0 {
        return Some(data.into_iter().take_while(|&b| b != 0).collect());
    }
    let mut out = Vec::new();
    use std::io::Read;
    flate2::read::ZlibDecoder::new(&data[..]).read_to_end(&mut out).ok()?;
    Some(out)
}

/// A loaded sound's samples as stored: Miles' `AILSOUNDINFO` plus data.
#[derive(Clone, Debug)]
pub struct LoadedSound {
    /// `WAVE_FORMAT_*`: 1 is PCM (all the multiplayer zones have).
    pub format: u16,
    pub rate: u32,
    pub bits: u16,
    pub channels: u16,
    /// Frames (samples per channel).
    pub frames: u32,
    /// Bytes per frame (PCM) or per block (ADPCM).
    pub block_size: u32,
    pub data: Arc<[u8]>,
}

impl LoadedSound {
    /// From a `LoadedSound` asset; `None` for a `,name` stub (the sound is in
    /// another zone) or an empty sound.
    pub fn from_asset(root: &GNode) -> Option<LoadedSound> {
        let s = root.node("sound")?;
        let data = s.bytes("data");
        if data.is_empty() {
            return None;
        }
        Some(LoadedSound {
            format: s.int("info::format") as u16,
            rate: s.int("info::rate") as u32,
            bits: s.int("info::bits") as u16,
            channels: s.int("info::channels") as u16,
            // (`samples` counts every channel's samples.)
            frames: (s.int("info::samples") / s.int("info::channels").max(1)) as u32,
            block_size: s.int("info::block_size") as u32,
            data: Arc::from(data),
        })
    }

    /// Interleaved 16-bit samples.
    pub fn pcm(&self) -> Result<Vec<i16>> {
        let ch = self.channels.max(1) as usize;
        match (self.format, self.bits) {
            (1, 16) => {
                let n = (self.frames as usize * ch).min(self.data.len() / 2);
                Ok(self.data[..n * 2].chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect())
            }
            (1, 8) => {
                let n = (self.frames as usize * ch).min(self.data.len());
                Ok(self.data[..n].iter().map(|&b| ((b as i16) - 128) << 8).collect())
            }
            (0x11, 4) => {
                let mut out = decode_ima_adpcm(&self.data, ch, self.block_size as usize)?;
                out.truncate(self.frames as usize * ch);
                Ok(out)
            }
            (f, b) => bail!("unsupported sound format {f:#x}, {b} bits"),
        }
    }

    pub fn seconds(&self) -> f32 {
        self.frames as f32 / self.rate.max(1) as f32
    }
}

/// A loaded sound as a plain 16-bit PCM WAV file.
pub fn decode(s: &LoadedSound) -> Result<Vec<u8>> {
    Ok(pcm_wav(s.channels.max(1), s.rate, &s.pcm()?))
}

/// Where a variation's audio is.
#[derive(Clone, Debug)]
pub enum SoundFile {
    /// Samples inside the zone; `name` is the original path, e.g.
    /// `weapons/m4/weap_m4_fire_plr.wav`. `sound` is `None` when the zone
    /// only refers to another zone's copy (a `,name` asset).
    Loaded { name: String, sound: Option<LoadedSound> },
    /// A file in the iwds, e.g. `sound/stream/music/...` (lower case).
    Streamed(String),
}

/// One variation of an alias.
#[derive(Clone, Debug)]
pub struct Variant {
    pub file: SoundFile,
    pub volume: (f32, f32),
    pub pitch: (f32, f32),
    /// Full volume up to the first distance, silent past the second.
    pub dist: (f32, f32),
    pub probability: f32,
    pub looping: bool,
    /// Index into [`channels`].
    pub channel: usize,
    /// Positional, from the channel's entry in [`channels`]; without the
    /// table every sound counts as positional.
    pub spatial: bool,
    /// Falloff curve name (see [`curves`]): `default`, `weapon5`, ...
    pub curve: Option<String>,
    /// An alias played along with this one (layers: the mechanical layer
    /// under a gunshot).
    pub secondary: Option<String>,
    /// An alias played after this one.
    pub chain: Option<String>,
    pub subtitle: Option<String>,
    /// `weapons`, `ambience`, ...
    pub mixer_group: Option<String>,
    /// Milliseconds before it starts.
    pub start_delay: i32,
    /// The raw `snd_alias_t::flags`.
    pub flags: u32,
}

/// A named sound and its variations.
#[derive(Clone, Debug)]
pub struct Alias {
    pub name: String,
    pub variants: Vec<Variant>,
}

/// Every sound alias in a zone. `chans` (from [`channels`]) decides which
/// are positional. Variations sharing a loaded sound share its data.
pub fn aliases(zone: &Zone, chans: Option<&[Channel]>) -> Vec<Alias> {
    let mut loaded: HashMap<usize, (String, Option<LoadedSound>)> = HashMap::new();
    zone.of_type(AssetType::Sound)
        .map(|(_, a)| Alias {
            name: a.name.clone(),
            variants: a.root.nodes("head").iter().filter_map(|h| variant(zone, h, chans, &mut loaded)).collect(),
        })
        .collect()
}

fn variant(zone: &Zone, h: &GNode, chans: Option<&[Channel]>, loaded: &mut HashMap<usize, (String, Option<LoadedSound>)>) -> Option<Variant> {
    let sf = h.node("soundFile")?;
    let u = sf.node("u")?;
    let file = if sf.int("type") == SAT_LOADED {
        let id = u.asset("loadSnd")?;
        let (name, sound) = loaded
            .entry(id)
            .or_insert_with(|| {
                let a = &zone.assets[id];
                let sound = if a.name.starts_with(',') { None } else { LoadedSound::from_asset(&a.root) };
                (a.name.strip_prefix(',').unwrap_or(&a.name).to_owned(), sound)
            })
            .clone();
        SoundFile::Loaded { name, sound }
    } else {
        let f = u.node("streamSnd")?;
        let dir = f.string("dir").unwrap_or("");
        let path = if dir.is_empty() { format!("sound/{}", f.string("name")?) } else { format!("sound/{dir}/{}", f.string("name")?) };
        SoundFile::Streamed(path.replace('\\', "/").to_ascii_lowercase())
    };
    let flags = h.int("flags");
    let channel = ((flags >> CHANNEL_SHIFT) & 63) as usize;
    let spatial = match chans.and_then(|c| c.get(channel)) {
        Some(c) => c.spatial,
        None => true,
    };
    let text = |f: &str| h.string(f).filter(|s| !s.is_empty()).map(str::to_owned);
    // (Aliases name the engine's default curve `$default`; its asset, in
    // `code_post_gfx_mp`, is `default`.)
    let curve = h.asset("volumeFalloffCurve").map(|id| zone.assets[id].name.trim_start_matches(',').trim_start_matches('$').to_owned());
    Some(Variant {
        file,
        volume: (h.float("volMin"), h.float("volMax").max(h.float("volMin"))),
        pitch: (h.float("pitchMin"), h.float("pitchMax").max(h.float("pitchMin"))),
        dist: (h.float("distMin"), h.float("distMax")),
        probability: h.float("probability"),
        looping: flags & LOOPING != 0,
        channel,
        spatial,
        curve,
        secondary: text("secondaryAliasName"),
        chain: text("chainAliasName"),
        subtitle: text("subtitle"),
        mixer_group: text("mixerGroup"),
        start_delay: h.int("startDelay") as i32,
        flags: flags as u32,
    })
}

/// The file type in an alias's flags (1 loaded, 2 streamed): for checking
/// the flag layout.
pub fn flags_file_type(flags: u32) -> u32 {
    (flags >> TYPE_SHIFT) & 3
}

const IMA_STEPS: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66, 73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337,
    371, 408, 449, 494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272, 2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894,
    6484, 7132, 7845, 8630, 9493, 10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];
const IMA_INDEX: [i32; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];

/// Microsoft IMA ADPCM (`WAVE_FORMAT_IMA_ADPCM`) blocks to interleaved
/// 16-bit samples. None of MW2's multiplayer sounds use it; it is here for
/// zones that might.
fn decode_ima_adpcm(data: &[u8], channels: usize, block: usize) -> Result<Vec<i16>> {
    if channels == 0 || channels > 2 || block < 4 * channels {
        bail!("bad IMA ADPCM layout ({channels} channels, block {block})");
    }
    let mut out = Vec::new();
    for b in data.chunks(block) {
        if b.len() < 4 * channels {
            break;
        }
        let mut pred = [0i32; 2];
        let mut idx = [0i32; 2];
        for c in 0..channels {
            pred[c] = i16::from_le_bytes([b[c * 4], b[c * 4 + 1]]) as i32;
            idx[c] = (b[c * 4 + 2] as i32).clamp(0, 88);
        }
        let body = &b[4 * channels..];
        // Channels interleave in 4-byte (8-sample) groups.
        let groups = body.len() / (4 * channels);
        let frames = 1 + groups * 8;
        let start = out.len();
        out.resize(start + frames * channels, 0);
        for c in 0..channels {
            out[start + c] = pred[c] as i16;
        }
        for g in 0..groups {
            for c in 0..channels {
                let bytes = &body[(g * channels + c) * 4..][..4];
                for (k, &byte) in bytes.iter().enumerate() {
                    for (j, nib) in [byte & 15, byte >> 4].into_iter().enumerate() {
                        let step = IMA_STEPS[idx[c] as usize];
                        let mut diff = step >> 3;
                        if nib & 1 != 0 {
                            diff += step >> 2;
                        }
                        if nib & 2 != 0 {
                            diff += step >> 1;
                        }
                        if nib & 4 != 0 {
                            diff += step;
                        }
                        pred[c] = if nib & 8 != 0 { pred[c] - diff } else { pred[c] + diff }.clamp(-32768, 32767);
                        idx[c] = (idx[c] + IMA_INDEX[(nib & 7) as usize]).clamp(0, 88);
                        let frame = 1 + g * 8 + k * 2 + j;
                        out[start + frame * channels + c] = pred[c] as i16;
                    }
                }
            }
        }
    }
    Ok(out)
}

/// A 16-bit PCM WAV file.
pub fn pcm_wav(channels: u16, rate: u32, samples: &[i16]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut w = Vec::with_capacity(44 + samples.len() * 2);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data_len).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&channels.to_le_bytes());
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * channels as u32 * 2).to_le_bytes());
    w.extend_from_slice(&(channels * 2).to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        w.extend_from_slice(&s.to_le_bytes());
    }
    w
}

//! Black Ops sounds. Each zone has one sound bank (`SndBank`, e.g.
//! `mpl_common.all` in `common_mp`) naming its aliases; an alias has one or
//! more variants (one is picked at random each time it plays), each with
//! its own volume, pitch and distance ranges and a sound file:
//!
//! * loaded sounds (gunshots, reload foley, short effects) are in the zone
//!   itself: a `snd_asset` header and the encoded data
//! * streamed sounds (ambience, music, dialogue) name a `.wav` in the `.iwd`
//!   archives, which is not a RIFF file but the same `snd_asset` header,
//!   padded to 2096 bytes, then the data
//!
//! On PC the data is MS-ADPCM (every streamed sound, a few loaded ones),
//! xWMA (most loaded sounds, see [`xwma`]) or, rarely, 16-bit PCM.
//! Variants also name a secondary alias that plays alongside them (a gun's
//! `wpn_ak47_fire_npc` brings in its distant layer `wpn_ak47_fire_npc_dist`,
//! which brings in the shell casing).

pub mod xwma;

use crate::Install;
use crate::zone::reader::{Ptr, S, key};
use crate::zone::{AssetType, GNode, Zone};
use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::sync::Arc;

/// Zones whose banks hold the multiplayer sounds: weapons, players,
/// killstreaks (`common_mp`) and menus (`code_post_gfx_mp`). Maps have
/// their own banks for their ambience and some player foley (the
/// `fly_gear_reload_plr` rattle in reload notetracks is only there).
pub const MP_ZONES: [&str; 2] = ["code_post_gfx_mp", "common_mp"];

/// Size of a `snd_asset` (`SoundFile` header).
const ASSET_SIZE: usize = 56;

/// Decoded audio: interleaved signed 16-bit samples.
#[derive(Clone, Debug)]
pub struct Pcm {
    pub sample_rate: u32,
    pub channels: u16,
    pub samples: Vec<i16>,
}

impl Pcm {
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1) as usize
    }

    pub fn seconds(&self) -> f32 {
        self.frames() as f32 / self.sample_rate.max(1) as f32
    }

    /// A RIFF `.wav` file of it.
    pub fn to_wav(&self) -> Vec<u8> {
        let data_len = self.samples.len() as u32 * 2;
        let align = self.channels * 2;
        let mut w = Vec::with_capacity(44 + data_len as usize);
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + data_len).to_le_bytes());
        w.extend_from_slice(b"WAVEfmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes());
        w.extend_from_slice(&self.channels.to_le_bytes());
        w.extend_from_slice(&self.sample_rate.to_le_bytes());
        w.extend_from_slice(&(self.sample_rate * align as u32).to_le_bytes());
        w.extend_from_slice(&align.to_le_bytes());
        w.extend_from_slice(&16u16.to_le_bytes());
        w.extend_from_slice(b"data");
        w.extend_from_slice(&data_len.to_le_bytes());
        for s in &self.samples {
            w.extend_from_slice(&s.to_le_bytes());
        }
        w
    }
}

/// `snd_asset_format`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Format {
    Pcm16,
    Pcm24,
    Pcm32,
    Float,
    /// Xbox 360 only.
    Xma,
    Mp3,
    MsAdpcm,
    /// xWMA: WMA v2 packets.
    Wma,
    Unknown(u32),
}

impl Format {
    fn from_u32(v: u32) -> Format {
        match v {
            0 => Format::Pcm16,
            1 => Format::Pcm24,
            2 => Format::Pcm32,
            3 => Format::Float,
            4 => Format::Xma,
            5 => Format::Mp3,
            6 => Format::MsAdpcm,
            7 => Format::Wma,
            v => Format::Unknown(v),
        }
    }
}

/// A `snd_asset` header: what the encoded data holds.
#[derive(Clone, Debug)]
pub struct Header {
    pub format: Format,
    pub frames: u32,
    pub sample_rate: u32,
    pub channels: u16,
    /// Bytes before the data in a streamed file.
    pub header_size: u32,
    /// The data is meant to loop.
    pub looping: bool,
    pub data_size: u32,
    /// xWMA: decoded bytes after each packet (XAudio2's `dpds`), one per
    /// packet.
    pub seek_table: Vec<u32>,
}

impl Header {
    fn parse(s: S) -> Header {
        Header {
            frames: s.u32(4),
            sample_rate: s.u32(8),
            channels: s.u32(12) as u16,
            header_size: s.u32(16),
            format: Format::from_u32(s.u32(28)),
            looping: s.u32(36) & 1 != 0,
            data_size: s.u32(48),
            seek_table: Vec::new(),
        }
    }
}

/// Where a sound's data is.
#[derive(Clone, Debug)]
pub enum Source {
    /// In the zone.
    Loaded { header: Header, data: Arc<[u8]> },
    /// A file in the `.iwd` archives (its [`Sound::name`]).
    Streamed,
}

/// One sound file.
#[derive(Clone, Debug)]
pub struct Sound {
    /// `sound\wpn\assault\ak47\plr\shot\shot_00.wav`.
    pub name: String,
    pub source: Source,
}

/// One variant of an alias.
#[derive(Clone, Debug)]
pub struct Alias {
    pub name: String,
    /// An alias played along with this one.
    pub secondary: Option<String>,
    pub subtitle: Option<String>,
    /// Linear gain range, 0..1.
    pub volume: [f32; 2],
    /// Playback rate range (1 = as recorded).
    pub pitch: [f32; 2],
    /// Full volume up to the first, silent beyond the second (inches).
    /// Both 0 for 2D sounds.
    pub distance: [f32; 2],
    /// Distance at which the reverb send fades out.
    pub reverb_distance: f32,
    /// Weight when picking a variant, 0..1.
    pub probability: f32,
    /// Seconds before it starts.
    pub start_delay: f32,
    /// Raw `snd_alias_t::flags`. Bit 0 is looping and bit 1 3D (see
    /// [`Alias::looping`], [`Alias::spatialized`]); bits 14-15 the load type
    /// (1 loaded, 2 streamed). The rest (bus, volume group, ...) are not
    /// worked out.
    pub flags: u32,
    /// The `pan` index (into the driver globals' speaker maps).
    pub pan: u8,
    /// Index into [`SoundBank::sounds`], `None` if the variant is silent.
    pub sound: Option<usize>,
}

impl Alias {
    /// Plays in a loop until stopped.
    pub fn looping(&self) -> bool {
        self.flags & 1 != 0
    }

    /// Positioned in the world (3D); otherwise it plays on the listener.
    pub fn spatialized(&self) -> bool {
        self.flags & 2 != 0
    }

    fn parse(n: &GNode, sound: Option<usize>) -> Alias {
        let u16f = |k: &str| n.int(k) as u16 as f32;
        let byte = |k: &str| n.int(k) as u8;
        Alias {
            name: n.string("name").unwrap_or_default().to_owned(),
            secondary: n.string("secondaryname").map(str::to_owned),
            subtitle: n.string("subtitle").map(str::to_owned),
            volume: [u16f("volMin") / 65535.0, u16f("volMax") / 65535.0],
            pitch: [u16f("pitchMin") / 32767.0, u16f("pitchMax") / 32767.0],
            distance: [u16f("distMin"), u16f("distMax")],
            reverb_distance: u16f("distReverbMax"),
            probability: byte("probability") as f32 / 255.0,
            start_delay: u16f("startDelay") / 1000.0,
            flags: n.int("flags") as u32,
            pan: byte("pan"),
            sound,
        }
    }
}

/// An alias variant with its decoded audio.
#[derive(Debug)]
pub struct AliasVariant<'a> {
    pub alias: &'a Alias,
    pub sound: Option<&'a Sound>,
    /// `None` for a silent variant; an error if the data could not be read
    /// or decoded (xWMA off Windows, MP3, ...).
    pub audio: Option<Result<Pcm>>,
}

/// Sound banks of one or more zones.
pub struct SoundBank {
    /// Alias name -> its variants. A later zone's alias replaces an earlier
    /// one of the same name.
    aliases: HashMap<String, Vec<Alias>>,
    pub sounds: Vec<Sound>,
    /// For streamed sounds.
    vfs: Option<Arc<iw3::iwd::Vfs>>,
    /// Variants whose sound file could not be found in the zone.
    pub unresolved: usize,
}

/// The multiplayer sound banks ([`MP_ZONES`]), with the install's archives
/// for streamed sounds.
pub fn load_bank(install: &Install) -> Result<SoundBank> {
    let mut bank = SoundBank::new(Some(Arc::new(install.vfs()?)));
    for name in MP_ZONES {
        let zone = Zone::parse(&crate::fastfile::load(&install.zone_path(name))?, Default::default())?;
        if let Some(stop) = &zone.stats.stopped_at {
            bail!("{name}: parsing stopped at {stop}");
        }
        bank.add_zone(&zone);
    }
    Ok(bank)
}

impl SoundBank {
    /// An empty bank. Without `vfs`, streamed sounds do not decode.
    pub fn new(vfs: Option<Arc<iw3::iwd::Vfs>>) -> SoundBank {
        SoundBank { aliases: HashMap::new(), sounds: Vec::new(), vfs, unresolved: 0 }
    }

    /// Add the aliases of a zone's sound bank.
    pub fn add_zone(&mut self, zone: &Zone) {
        // Sound files are loaded with the first variant that uses them;
        // later ones point back at them.
        let mut by_key: HashMap<u32, Option<usize>> = HashMap::new();
        for (_, bank) in zone.of_type(AssetType::Sound) {
            for list in bank.root.nodes("alias") {
                let Some(name) = list.string("name") else { continue };
                let variants = list
                    .nodes("head")
                    .iter()
                    .map(|a| {
                        let sound = match a.node("soundFile") {
                            Some(file) => {
                                let sound = self.sound_file(file, &mut by_key);
                                if let Some(k) = file.key {
                                    by_key.insert(k, sound);
                                }
                                sound
                            }
                            // `snd_alias_t::soundFile` is at offset 16.
                            None => self.reference(S(&a.data).ptr(16), &by_key),
                        };
                        Alias::parse(a, sound)
                    })
                    .collect();
                self.aliases.insert(name.to_owned(), variants);
            }
        }
    }

    /// A sound loaded earlier, or `None` (counted) for a dangling reference.
    fn reference(&mut self, ptr: Ptr, by_key: &HashMap<u32, Option<usize>>) -> Option<usize> {
        match ptr {
            Ptr::Null => None,
            Ptr::Ref { block, offset } => {
                let found = by_key.get(&key(block, offset)).copied();
                if found.is_none() {
                    self.unresolved += 1;
                }
                found.flatten()
            }
            Ptr::Inline | Ptr::Insert => {
                self.unresolved += 1;
                None
            }
        }
    }

    /// Add the sound a `SoundFile` names.
    fn sound_file(&mut self, file: &GNode, by_key: &mut HashMap<u32, Option<usize>>) -> Option<usize> {
        if file.int("exists") == 0 {
            return None;
        }
        let target = file.node("u").and_then(|u| u.node("loadSnd").or_else(|| u.node("streamSnd")));
        let Some(t) = target else {
            // `SoundFile::u` (at offset 0) points at a LoadedSound or
            // StreamedSound loaded earlier.
            return self.reference(S(&file.data).ptr(0), by_key);
        };
        let source = match t.node("sound") {
            Some(asset) => {
                let mut header = Header::parse(S(&asset.data));
                header.seek_table = u32s(asset.bytes("seek_table"));
                Source::Loaded { header, data: asset.bytes("data").into() }
            }
            None => Source::Streamed,
        };
        let name = t.string("name").or_else(|| t.string("filename")).unwrap_or_default().to_owned();
        let index = self.sounds.len();
        self.sounds.push(Sound { name, source });
        if let Some(k) = t.key {
            by_key.insert(k, Some(index));
        }
        Some(index)
    }

    pub fn alias_names(&self) -> impl Iterator<Item = &str> {
        self.aliases.keys().map(String::as_str)
    }

    /// An alias's variants, without decoding anything.
    pub fn variants(&self, name: &str) -> Option<&[Alias]> {
        self.aliases.get(name).map(Vec::as_slice)
    }

    /// An alias's variants with their audio decoded.
    pub fn alias(&self, name: &str) -> Option<Vec<AliasVariant<'_>>> {
        let variants = self.aliases.get(name)?;
        Some(
            variants
                .iter()
                .map(|alias| {
                    let sound = alias.sound.map(|i| &self.sounds[i]);
                    AliasVariant { alias, sound, audio: sound.map(|s| self.decode(s)) }
                })
                .collect(),
        )
    }

    /// A sound's header and encoded data.
    pub fn open(&self, sound: &Sound) -> Result<(Header, Arc<[u8]>)> {
        match &sound.source {
            Source::Loaded { header, data } => Ok((header.clone(), data.clone())),
            Source::Streamed => {
                let Some(vfs) = &self.vfs else { bail!("{}: streamed, and no .iwd archives mounted", sound.name) };
                let file =
                    vfs.read(&sound.name)?.with_context(|| format!("{} is not in the .iwd archives", sound.name))?;
                if file.len() < ASSET_SIZE {
                    bail!("{}: too short for a sound header", sound.name);
                }
                let mut header = Header::parse(S(&file));
                // A seek table would follow the header (none of the PC's
                // streamed sounds have one: they are all MS-ADPCM).
                let count = S(&file).u32(40) as usize;
                header.seek_table = u32s(file.get(ASSET_SIZE..ASSET_SIZE + count * 4).unwrap_or_default());
                let start = header.header_size as usize;
                let end = (start + header.data_size as usize).min(file.len());
                let data = file.get(start..end).with_context(|| format!("{}: data out of range", sound.name))?;
                Ok((header, data.into()))
            }
        }
    }

    /// Decode a sound to 16-bit PCM.
    pub fn decode(&self, sound: &Sound) -> Result<Pcm> {
        let (h, data) = self.open(sound)?;
        decode(&h, &data).with_context(|| format!("decoding {} ({:?})", sound.name, h.format))
    }
}

fn u32s(b: &[u8]) -> Vec<u32> {
    b.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect()
}

/// Decode encoded data described by `h`.
pub fn decode(h: &Header, data: &[u8]) -> Result<Pcm> {
    let channels = h.channels.max(1);
    let mut samples: Vec<i16> = match h.format {
        Format::Pcm16 => data.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect(),
        Format::Pcm24 => data.chunks_exact(3).map(|b| i16::from_le_bytes([b[1], b[2]])).collect(),
        Format::Pcm32 => data.chunks_exact(4).map(|b| i16::from_le_bytes([b[2], b[3]])).collect(),
        Format::Float => data
            .chunks_exact(4)
            .map(|b| (f32::from_le_bytes(b.try_into().unwrap()) * 32767.0).clamp(-32768.0, 32767.0) as i16)
            .collect(),
        Format::MsAdpcm => ms_adpcm(data, channels as usize),
        Format::Wma => {
            let packets = h.seek_table.len().max(1);
            xwma::decode(data, h.sample_rate, channels, (data.len() / packets) as u32)?
        }
        f => bail!("{f:?} data is not supported"),
    };
    samples.truncate(h.frames as usize * channels as usize);
    Ok(Pcm { sample_rate: h.sample_rate, channels, samples })
}

/// Frames per MS-ADPCM block. Black Ops always uses 512 (blocks of 262
/// bytes per channel).
const ADPCM_BLOCK_FRAMES: usize = 512;

/// Decode MS-ADPCM: blocks of a 7 byte header per channel (predictor,
/// step, the last two samples) then 4-bit codes, channels interleaved.
pub fn ms_adpcm(data: &[u8], channels: usize) -> Vec<i16> {
    // The standard predictor pairs and step adaptation table.
    const COEF: [(i32, i32); 7] = [(256, 0), (512, -256), (0, 0), (192, 64), (240, 0), (460, -208), (392, -232)];
    const ADAPT: [i32; 16] = [230, 230, 230, 230, 307, 409, 512, 614, 768, 614, 512, 409, 307, 230, 230, 230];
    let block = channels * (7 + (ADPCM_BLOCK_FRAMES - 2) / 2);
    let mut out = Vec::with_capacity(data.len() / block * ADPCM_BLOCK_FRAMES * channels);
    let (mut coef, mut step, mut s1, mut s2) =
        (vec![(0, 0); channels], vec![0i32; channels], vec![0i32; channels], vec![0i32; channels]);
    for b in data.chunks_exact(block) {
        let h = S(b);
        for c in 0..channels {
            coef[c] = COEF[(b[c] as usize).min(6)];
            step[c] = h.i16(channels + c * 2) as i32;
            s1[c] = h.i16(channels * 3 + c * 2) as i32;
            s2[c] = h.i16(channels * 5 + c * 2) as i32;
        }
        out.extend(s2.iter().map(|&s| s as i16));
        out.extend(s1.iter().map(|&s| s as i16));
        let mut c = 0;
        for &byte in &b[channels * 7..] {
            for code in [byte >> 4, byte & 15] {
                let signed = (code as i32 ^ 8) - 8;
                // Division (rounding towards zero) as in Microsoft's codec.
                let predicted = (s1[c] * coef[c].0 + s2[c] * coef[c].1) / 256;
                let s = (predicted + signed * step[c]).clamp(-32768, 32767);
                s2[c] = s1[c];
                s1[c] = s;
                step[c] = ((ADAPT[code as usize] * step[c]) >> 8).max(16);
                out.push(s as i16);
                c = (c + 1) % channels;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adpcm_block_layout() {
        // One mono block: predictor 0 (repeat), step 16, samples 100 and 50,
        // then zero codes: the signal holds at the last sample.
        let mut b = vec![0u8; 262];
        b[1..3].copy_from_slice(&16i16.to_le_bytes());
        b[3..5].copy_from_slice(&100i16.to_le_bytes());
        b[5..7].copy_from_slice(&50i16.to_le_bytes());
        let out = ms_adpcm(&b, 1);
        assert_eq!(out.len(), 512);
        assert_eq!(&out[..3], &[50, 100, 100]);
    }

    #[test]
    fn xwma_byte_rates() {
        assert_eq!(xwma::byte_rate(2230, 44100), 6000);
        assert_eq!(xwma::byte_rate(4096, 48000), 12000);
        assert_eq!(xwma::byte_rate(8917, 44100), 24000);
    }
}

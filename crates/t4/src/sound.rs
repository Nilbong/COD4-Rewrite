//! World at War's sounds: aliases (`weap_thompson_fire_plr`, `mouse_click`,
//! `music_mainmenu`, ...), their sound files, and the mixer buses and
//! falloff curves they refer to.
//!
//! Like CoD4, an alias is a list of variations, each a sound file plus
//! volume, pitch and distance ranges. A file is either *loaded* (a whole
//! `.wav` file inside the zone) or *streamed* (`sound/<dir>/<name>` in the
//! iwds; all music is streamed from `sound/Stream/Music`). Almost every file
//! is MS-ADPCM, which [`decode_wav`] turns into plain 16-bit PCM; about a
//! hundred (mostly zombies effects) are xWMA, which it hands to Black Ops'
//! decoder ([`t5::sound::xwma`], Windows' own WMA decoder; Windows only).
//!
//! Where CoD4 has channels, World at War has mixer buses, defined with the
//! falloff curves in the `SndDriverGlobals` asset ([`Mixer`], in
//! `code_post_gfx_mp`). An alias's `flags` were matched against known
//! sounds: bit 0 loops, bit 6 (`0x40`) is positional (3D), and the bus index
//! is `flags >> 22` (music on `music`, menu clicks on `ui`, breathing on
//! `voice`, pistol shots on `pis_1st`/`pis_3rd`, ...).

use crate::zone::{AssetType, GNode, Zone};
use anyhow::{Result, bail};
use std::collections::HashMap;
use std::sync::Arc;

/// `snd_alias_t::flags`: the sound loops.
const LOOPING: i64 = 0x1;
/// `snd_alias_t::flags`: the sound is positional.
const SPATIAL: i64 = 0x40;
/// `snd_alias_t::flags >> BUS_SHIFT & 63` is the mixer bus.
const BUS_SHIFT: u32 = 22;

/// A mixer bus (`snd_bus_info_t`).
#[derive(Clone, Debug, Default)]
pub struct Bus {
    /// `music`, `ambience`, `voice`, `ui`, `smg_1st`, ...
    pub name: String,
    pub volume: f32,
    pub is_music: bool,
    pub pausable: bool,
    pub stop_on_death: bool,
}

/// A falloff curve (`snd_curve_t`): (fraction of the way from the near to
/// the far distance, volume) knots.
#[derive(Clone, Debug, Default)]
pub struct Curve {
    /// `default`, `log0`, `curve2`, ...
    pub name: String,
    pub knots: Vec<[f32; 2]>,
}

/// The buses and curves, from the `SndDriverGlobals` asset.
#[derive(Clone, Debug, Default)]
pub struct Mixer {
    /// By index; unused slots have empty names.
    pub buses: Vec<Bus>,
    pub curves: Vec<Curve>,
}

impl Mixer {
    /// The mixer of a zone that has one (`code_post_gfx_mp`).
    pub fn from_zone(zone: &Zone) -> Option<Mixer> {
        let (_, g) = zone.of_type(AssetType::SndDriverGlobals).next()?;
        let s = crate::zone::schema::schema();
        let offset = |m: &str| s.types[&g.root.ty].members.iter().find(|x| x.name.as_deref() == Some(m)).map(|x| x.offset as usize);
        let d = &g.root.data;
        let name = |b: &[u8]| String::from_utf8_lossy(&b[..32]).trim_end_matches('\0').to_owned();
        let f = |b: &[u8], o: usize| f32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        let i = |b: &[u8], o: usize| i32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        let bus_size = s.types["snd_bus_info_t"].size as usize;
        let buses = (0..64)
            .filter_map(|k| d.get(offset("buses")? + k * bus_size..)?.get(..bus_size))
            .map(|b| Bus {
                name: name(b),
                volume: f(b, 32),
                pausable: i(b, 44) != 0,
                stop_on_death: i(b, 48) != 0,
                is_music: i(b, 52) != 0,
            })
            .collect();
        let curve_size = s.types["snd_curve_t"].size as usize;
        let curves = (0..32)
            .filter_map(|k| d.get(offset("curves")? + k * curve_size..)?.get(..curve_size))
            .map(|c| Curve {
                name: name(c),
                knots: (0..i(c, 32).clamp(0, 8) as usize).map(|k| [f(c, 36 + k * 8), f(c, 40 + k * 8)]).collect(),
            })
            .collect();
        Some(Mixer { buses, curves })
    }
}

/// Where a variation's audio is.
#[derive(Clone, Debug)]
pub enum SoundFile {
    /// A `.wav` file inside the zone (see [`decode_wav`]); `name` is its
    /// original path, e.g. `sfx/ui/heart_beat.wav`. `data` is `None` when the
    /// zone only refers to another zone's copy (a `,name` asset): the same
    /// name loaded elsewhere, usually `common_mp`.
    Loaded { name: String, data: Option<Arc<[u8]>> },
    /// A file in the iwds, e.g. `sound/stream/music/mainmenu/mx_main.wav`
    /// (lower case; `iw3::iwd::Vfs` lookups ignore case).
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
    /// Positional; otherwise heard without position (music, UI, the
    /// player's own sounds).
    pub spatial: bool,
    /// Index into [`Mixer::buses`].
    pub bus: usize,
    /// Index into [`Mixer::curves`]: how the volume falls off.
    pub curve: usize,
    /// An alias played along with this one (layers).
    pub secondary: Option<String>,
    /// An alias played after this one.
    pub chain: Option<String>,
    /// A subtitle string key.
    pub subtitle: Option<String>,
}

/// A named sound and its variations.
#[derive(Clone, Debug)]
pub struct Alias {
    pub name: String,
    pub variants: Vec<Variant>,
}

/// Every sound alias in a zone. Variations sharing a loaded sound share its
/// data.
pub fn aliases(zone: &Zone) -> Vec<Alias> {
    let mut loaded: HashMap<usize, Option<(String, Option<Arc<[u8]>>)>> = HashMap::new();
    zone.of_type(AssetType::Sound)
        .map(|(_, a)| Alias {
            name: a.name.clone(),
            variants: a.root.nodes("head").iter().filter_map(|h| variant(zone, h, &mut loaded)).collect(),
        })
        .collect()
}

fn variant(zone: &Zone, h: &GNode, loaded: &mut HashMap<usize, Option<(String, Option<Arc<[u8]>>)>>) -> Option<Variant> {
    let u = h.node("soundFile")?.node("u")?;
    let file = if let Some(id) = u.asset("loadSnd") {
        let (name, data) = loaded
            .entry(id)
            .or_insert_with(|| {
                let a = &zone.assets[id];
                let data = a.root.node("sound").map(|s| s.bytes("data")).filter(|d| !d.is_empty()).map(Arc::from);
                let name = a.name.strip_prefix(',').unwrap_or(&a.name).to_owned();
                Some((name, data.filter(|_| !a.name.starts_with(','))))
            })
            .clone()?;
        SoundFile::Loaded { name, data }
    } else {
        let f = u.node("streamSnd")?.node("filename")?;
        let path = format!("sound/{}/{}", f.string("dir").unwrap_or(""), f.string("name")?);
        // Some directories use backslashes (`stream\level\mp_subway`).
        SoundFile::Streamed(path.replace('\\', "/").to_ascii_lowercase())
    };
    let flags = h.int("flags");
    let text = |f: &str| h.string(f).filter(|s| !s.is_empty()).map(str::to_owned);
    Some(Variant {
        file,
        volume: (h.float("volMin"), h.float("volMax").max(h.float("volMin"))),
        pitch: (h.float("pitchMin"), h.float("pitchMax").max(h.float("pitchMin"))),
        dist: (h.float("distMin"), h.float("distMax")),
        probability: h.float("probability"),
        looping: flags & LOOPING != 0,
        spatial: flags & SPATIAL != 0,
        bus: ((flags >> BUS_SHIFT) & 63) as usize,
        curve: h.int("volumeFalloffCurve") as usize,
        secondary: text("secondaryAliasName"),
        chain: text("chainAliasName"),
        subtitle: text("subtitle"),
    })
}

/// Every music file: everything under `sound/stream/music` in the iwds.
pub fn music_files(vfs: &iw3::iwd::Vfs) -> Vec<String> {
    let mut out: Vec<String> = vfs.files().filter(|f| f.starts_with("sound/stream/music/")).map(str::to_owned).collect();
    out.sort();
    out
}

/// The audio encoding of a World at War `.wav` file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Pcm,
    MsAdpcm,
    /// Windows Media Audio in a `XWMA` RIFF (decoded on Windows only).
    Xwma,
    Other(u16),
}

/// The encoding of a `.wav` file's audio, if it is one.
pub fn encoding(wav: &[u8]) -> Option<Encoding> {
    if wav.get(..4)? != b"RIFF" {
        return None;
    }
    if wav.get(8..12)? == b"XWMA" {
        return Some(Encoding::Xwma);
    }
    let fmt = chunk(wav, b"fmt ")?;
    Some(match u16::from_le_bytes(fmt.get(..2)?.try_into().ok()?) {
        1 => Encoding::Pcm,
        2 => Encoding::MsAdpcm,
        t => Encoding::Other(t),
    })
}

/// A RIFF chunk's contents.
fn chunk<'a>(wav: &'a [u8], id: &[u8; 4]) -> Option<&'a [u8]> {
    let mut o = 12;
    while o + 8 <= wav.len() {
        let len = u32::from_le_bytes(wav[o + 4..o + 8].try_into().unwrap()) as usize;
        let body = o + 8;
        if &wav[o..o + 4] == id {
            return wav.get(body..(body + len).min(wav.len()));
        }
        o = body + len + (len & 1);
    }
    None
}

/// A World at War `.wav` file (loaded or streamed) as a plain 16-bit PCM
/// WAV file that any decoder reads. PCM files are returned as they are.
pub fn decode_wav(wav: &[u8]) -> Result<Vec<u8>> {
    match encoding(wav) {
        Some(Encoding::Pcm) => Ok(wav.to_vec()),
        Some(Encoding::MsAdpcm) => {
            let (Some(fmt), Some(data)) = (chunk(wav, b"fmt "), chunk(wav, b"data")) else { bail!("missing fmt or data chunk") };
            let samples = chunk(wav, b"fact").and_then(|f| f.get(..4)).map(|f| u32::from_le_bytes(f.try_into().unwrap()) as usize);
            let (channels, rate, pcm) = decode_ms_adpcm(fmt, data, samples)?;
            Ok(pcm_wav(channels, rate, &pcm))
        }
        Some(Encoding::Xwma) => {
            let (Some(fmt), Some(data)) = (chunk(wav, b"fmt "), chunk(wav, b"data")) else { bail!("missing fmt or data chunk") };
            if fmt.len() < 14 {
                bail!("short xWMA fmt chunk");
            }
            let channels = u16::from_le_bytes([fmt[2], fmt[3]]).max(1);
            let rate = u32::from_le_bytes(fmt[4..8].try_into().unwrap());
            let block_align = u16::from_le_bytes([fmt[12], fmt[13]]) as u32;
            let mut pcm = t5::sound::xwma::decode(data, rate, channels, block_align)?;
            // The seek table (`dpds`) holds the decoded byte count after
            // each packet: the last is the sound's length.
            if let Some(total) = chunk(wav, b"dpds").and_then(|d| d.rchunks_exact(4).next()) {
                pcm.truncate(u32::from_le_bytes(total.try_into().unwrap()) as usize / 2);
            }
            Ok(pcm_wav(channels, rate, &pcm))
        }
        Some(Encoding::Other(t)) => bail!("unsupported wav format {t:#x}"),
        None => bail!("not a wav file"),
    }
}

/// MS-ADPCM's step adaptation table.
const ADAPTATION: [i32; 16] = [230, 230, 230, 230, 307, 409, 512, 614, 768, 614, 512, 409, 307, 230, 230, 230];

/// Decode MS-ADPCM (`fmt` and `data` chunk contents) to interleaved 16-bit
/// samples, `frames` long if the `fact` chunk gives a length.
fn decode_ms_adpcm(fmt: &[u8], data: &[u8], frames: Option<usize>) -> Result<(u16, u32, Vec<i16>)> {
    if fmt.len() < 22 {
        bail!("short MS-ADPCM fmt chunk");
    }
    let u16_at = |o: usize| u16::from_le_bytes([fmt[o], fmt[o + 1]]);
    let channels = u16_at(2).max(1) as usize;
    let rate = u32::from_le_bytes(fmt[4..8].try_into().unwrap());
    let block_align = u16_at(12) as usize;
    let per_block = u16_at(18) as usize;
    let num_coef = u16_at(20) as usize;
    if channels > 2 || block_align < 7 * channels || fmt.len() < 22 + num_coef * 4 {
        bail!("bad MS-ADPCM header ({channels} channels, block {block_align})");
    }
    let coefs: Vec<(i32, i32)> = (0..num_coef).map(|k| (u16_at(22 + k * 4) as i16 as i32, u16_at(24 + k * 4) as i16 as i32)).collect();
    let mut out = Vec::with_capacity(data.len() / block_align.max(1) * per_block * channels + per_block * channels);
    for block in data.chunks(block_align) {
        if block.len() < 7 * channels {
            break;
        }
        // Per channel: predictor index, step, then the two newest samples.
        let rd = |o: usize| i16::from_le_bytes([block[o], block[o + 1]]) as i32;
        let mut coef = [(0, 0); 2];
        let mut delta = [0i32; 2];
        let mut s1 = [0i32; 2];
        let mut s2 = [0i32; 2];
        for c in 0..channels {
            coef[c] = *coefs.get(block[c] as usize).unwrap_or(&(256, 0));
            delta[c] = rd(channels + c * 2);
            s1[c] = rd(channels * 3 + c * 2);
            s2[c] = rd(channels * 5 + c * 2);
        }
        let mut frames_out = 2;
        for c in 0..channels {
            out.push(s2[c] as i16);
        }
        for c in 0..channels {
            out.push(s1[c] as i16);
        }
        let mut c = 0;
        'nibbles: for &byte in &block[7 * channels..] {
            for nibble in [byte >> 4, byte & 15] {
                if frames_out >= per_block {
                    break 'nibbles;
                }
                let signed = if nibble >= 8 { nibble as i32 - 16 } else { nibble as i32 };
                let predicted = (s1[c] * coef[c].0 + s2[c] * coef[c].1) / 256 + signed * delta[c];
                let sample = predicted.clamp(i16::MIN as i32, i16::MAX as i32);
                s2[c] = s1[c];
                s1[c] = sample;
                delta[c] = (ADAPTATION[nibble as usize] * delta[c] / 256).max(16);
                out.push(sample as i16);
                c += 1;
                if c == channels {
                    c = 0;
                    frames_out += 1;
                }
            }
        }
    }
    if let Some(n) = frames {
        out.truncate(n * channels);
    }
    Ok((channels as u16, rate, out))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A mono MS-ADPCM file of one block: header samples 100 then 200, and
    /// zero nibbles, which with the first coefficient pair (256, 0) repeat
    /// the newest sample.
    #[test]
    fn decodes_ms_adpcm() {
        let mut fmt = vec![2, 0, 1, 0];
        fmt.extend_from_slice(&22050u32.to_le_bytes());
        fmt.extend_from_slice(&11025u32.to_le_bytes());
        fmt.extend_from_slice(&[9, 0, 4, 0, 32, 0]); // block align 9, 4 bits, cbSize
        fmt.extend_from_slice(&[6, 0, 7, 0]); // 6 samples per block, 7 coefficients
        for (a, b) in [(256i16, 0i16), (512, -256), (0, 0), (192, 64), (240, 0), (460, -208), (392, -232)] {
            fmt.extend_from_slice(&a.to_le_bytes());
            fmt.extend_from_slice(&b.to_le_bytes());
        }
        // Predictor 0, delta 16, newest sample 200, older sample 100, then
        // four zero nibbles.
        let data = [0u8, 16, 0, 200, 0, 100, 0, 0, 0];
        let mut wav = b"RIFF\0\0\0\0WAVE".to_vec();
        for (id, body) in [(b"fmt ", &fmt[..]), (b"data", &data[..])] {
            wav.extend_from_slice(id);
            wav.extend_from_slice(&(body.len() as u32).to_le_bytes());
            wav.extend_from_slice(body);
        }
        assert_eq!(encoding(&wav), Some(Encoding::MsAdpcm));
        let pcm = decode_wav(&wav).unwrap();
        assert_eq!(&pcm[..4], b"RIFF");
        let samples: Vec<i16> = pcm[44..].chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
        assert_eq!(samples, [100, 200, 200, 200, 200, 200]);
        assert_eq!(u32::from_le_bytes(pcm[24..28].try_into().unwrap()), 22050);
    }
}

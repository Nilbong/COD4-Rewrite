//! Sound aliases: CoD4's named sounds (`weap_ak47_fire_plr`,
//! `step_run_concrete`, ...), each a set of variations with volume and
//! pitch ranges, a distance falloff and a channel. A loaded sound is PCM in
//! the zone (given a WAV header here); a streamed one is an mp3 or wav in the
//! iwds, repacked into a plain WAV so Bevy's decoder can always read it.
//! Black Ops' weapon sounds come from its own banks ([`t5::sound`]),
//! decoded the first time they play. World at War's weapon sounds come from
//! its zones and iwds ([`t4::sound`], mostly MS-ADPCM), also decoded on
//! first play, under names of their own ([`crate::waw::sound_alias`]).

use bevy::prelude::*;
use iw3::zone::generic::GNode;
use iw3::zone::{Asset, AssetId, AssetType, Zone};
use std::collections::HashMap;
use std::sync::Arc;

/// `snd_alias_t::flags`: the sound loops.
const LOOPING: i64 = 1;
/// ... ducks slave sounds while it plays; is ducked.
const MASTER: i64 = 2;
const SLAVE: i64 = 4;
/// `AILSOUNDINFO::format` for plain PCM.
const PCM: i64 = 1;
/// Channels (`flags >> 8 & 63`) heard without position: auto2d, menu,
/// body2d, reload2d, weapon2d, local, local2, ambient, hurt, player1/2,
/// music, musicnopause, mission, announcer and shellshock.
const CHANNELS_2D: [i64; 16] = [7, 10, 12, 14, 19, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32];

#[derive(Clone)]
pub enum SoundFile {
    /// PCM samples from the zone, already a WAV file.
    Loaded(Arc<[u8]>),
    /// A file in the iwds.
    Streamed(String),
    /// A sound in Black Ops' banks (index into its `sounds`).
    Bo1(Arc<t5::sound::SoundBank>, usize),
    /// A World at War `.wav` file from its zone ([`t4::sound::decode_wav`]).
    Waw(Arc<[u8]>),
    /// A World at War `.wav` file in its iwds.
    WawStreamed(Arc<iw3::iwd::Vfs>, String),
}

/// One variation of a sound alias.
#[derive(Clone)]
pub struct Variant {
    pub file: SoundFile,
    pub volume: (f32, f32),
    pub pitch: (f32, f32),
    /// Full volume up to the first distance, silent past the second (CoD units).
    pub dist: (f32, f32),
    pub probability: f32,
    pub looping: bool,
    /// Heard without position (music, the announcer, the player's own sounds).
    pub two_d: bool,
    /// Volume falloff knots: (fraction of the way from the near to the far
    /// distance, volume). Empty is linear.
    pub curve: Vec<[f32; 2]>,
    /// An alias played along with this one (layers).
    pub secondary: Option<String>,
    /// CoD4's ducking: while a master sound plays, slave ones drop to their
    /// percentage (`flags` 2 and 4, `slavePercentage`).
    pub master: bool,
    pub slave: Option<f32>,
}

impl Variant {
    /// Volume scale at `d` CoD units away.
    pub fn falloff(&self, d: f32) -> f32 {
        let (near, far) = self.dist;
        if d <= near {
            return 1.0;
        }
        if far <= near || d >= far {
            return 0.0;
        }
        let x = (d - near) / (far - near);
        if self.curve.len() < 2 {
            return 1.0 - x;
        }
        for w in self.curve.windows(2) {
            let ([x0, y0], [x1, y1]) = (w[0], w[1]);
            if x <= x1 {
                let t = if x1 > x0 { (x - x0) / (x1 - x0) } else { 1.0 };
                return (y0 + (y1 - y0) * t).clamp(0.0, 1.0);
            }
        }
        self.curve.last().map_or(0.0, |k| k[1].clamp(0.0, 1.0))
    }
}

/// The variations of each alias, by lower-cased name.
pub type Aliases = HashMap<String, Arc<Vec<Variant>>>;

/// Every sound alias in a zone. Aliases sharing a loaded sound share its
/// samples.
pub fn aliases(zone: &Zone) -> Vec<(String, Vec<Variant>)> {
    let mut wavs: HashMap<AssetId, Option<Arc<[u8]>>> = HashMap::new();
    zone.assets
        .iter()
        .filter_map(|a| match a {
            Asset::Generic(g) if g.ty == AssetType::Sound => Some(g),
            _ => None,
        })
        .map(|g| {
            let variants = g.root.nodes("head").iter().filter_map(|h| variant(zone, h, &mut wavs)).collect();
            (g.name.to_ascii_lowercase(), variants)
        })
        .collect()
}

fn variant(zone: &Zone, h: &GNode, wavs: &mut HashMap<AssetId, Option<Arc<[u8]>>>) -> Option<Variant> {
    let u = h.node("soundFile")?.node("u")?;
    let file = if let Some(id) = u.asset("loadSnd") {
        let wav = wavs
            .entry(id)
            .or_insert_with(|| {
                let Asset::Generic(g) = zone.get(id) else { return None };
                let sound = g.root.node("sound")?;
                let info = sound.node("info")?;
                (info.int("format") == PCM).then(|| {
                    wav(info.int("channels") as u16, info.int("rate") as u32, info.int("bits") as u16, sound.bytes("data")).into()
                })
            })
            .clone()?;
        SoundFile::Loaded(wav)
    } else {
        let s = u.node("streamSnd")?;
        SoundFile::Streamed(format!("sound/{}/{}", s.string("dir").unwrap_or(""), s.string("name")?).to_ascii_lowercase())
    };
    let flags = h.int("flags");
    let curve = h
        .asset("volumeFalloffCurve")
        .and_then(|id| match zone.get(id) {
            Asset::Generic(c) => Some(&c.root.data),
            _ => None,
        })
        .map(|d| {
            // SndCurve: name pointer, knotCount, knots[8][2].
            let f = |o: usize| d.get(o..o + 4).map_or(0.0, |b| f32::from_le_bytes(b.try_into().unwrap()));
            let n = d.get(4..8).map_or(0, |b| i32::from_le_bytes(b.try_into().unwrap())).clamp(0, 8) as usize;
            (0..n).map(|k| [f(8 + k * 8), f(12 + k * 8)]).collect()
        })
        .unwrap_or_default();
    Some(Variant {
        file,
        volume: (h.float("volMin"), h.float("volMax").max(h.float("volMin"))),
        pitch: (h.float("pitchMin"), h.float("pitchMax").max(h.float("pitchMin"))),
        dist: (h.float("distMin"), h.float("distMax")),
        probability: h.float("probability"),
        looping: flags & LOOPING != 0,
        two_d: CHANNELS_2D.contains(&((flags >> 8) & 63)),
        curve,
        secondary: h.string("secondaryAliasName").filter(|s| !s.is_empty()).map(str::to_ascii_lowercase),
        master: flags & MASTER != 0,
        slave: (flags & SLAVE != 0).then(|| h.float("slavePercentage").clamp(0.0, 1.0)),
    })
}

/// A WAV file around PCM samples.
pub fn wav(channels: u16, rate: u32, bits: u16, data: &[u8]) -> Vec<u8> {
    let block = channels * bits / 8;
    let mut out = Vec::with_capacity(44 + data.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * block as u32).to_le_bytes());
    out.extend_from_slice(&block.to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
    out
}

/// A streamed WAV as a plain PCM one (some carry extra chunks before their
/// format), or `None` if it isn't PCM.
fn plain_wav(file: &[u8]) -> Option<Vec<u8>> {
    if file.get(..4)? != b"RIFF" || file.get(8..12)? != b"WAVE" {
        return None;
    }
    let (mut fmt, mut data) = (None, None);
    let mut at = 12;
    while at + 8 <= file.len() {
        let id = &file[at..at + 4];
        let len = u32::from_le_bytes(file[at + 4..at + 8].try_into().ok()?) as usize;
        let body = file.get(at + 8..(at + 8 + len).min(file.len()))?;
        match id {
            b"fmt " => fmt = Some(body),
            b"data" => data = Some(body),
            _ => {}
        }
        at += 8 + len + (len & 1);
    }
    let fmt = fmt?;
    let le16 = |o: usize| fmt.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let (tag, channels, rate, bits) = (le16(0)?, le16(2)?, u32::from_le_bytes(fmt.get(4..8)?.try_into().ok()?), le16(14)?);
    let data = data?;
    (tag == 1 && matches!(bits, 8 | 16 | 24 | 32) && channels > 0).then(|| wav(channels, rate, bits, data))
}

/// Audio made so far, by file.
#[derive(Default)]
pub struct Sources {
    loaded: HashMap<usize, Handle<AudioSource>>,
    streamed: HashMap<String, Option<Handle<AudioSource>>>,
    bo1: HashMap<usize, Option<Handle<AudioSource>>>,
    waw: HashMap<usize, Option<Handle<AudioSource>>>,
    waw_streamed: HashMap<String, Option<Handle<AudioSource>>>,
}

impl Sources {
    pub fn handle(&mut self, file: &SoundFile, vfs: &iw3::iwd::Vfs, audio: &mut Assets<AudioSource>) -> Option<Handle<AudioSource>> {
        match file {
            SoundFile::Loaded(bytes) => {
                let key = bytes.as_ptr() as usize;
                Some(self.loaded.entry(key).or_insert_with(|| audio.add(AudioSource { bytes: bytes.clone() })).clone())
            }
            SoundFile::Streamed(path) => self
                .streamed
                .entry(path.clone())
                .or_insert_with(|| {
                    let data = vfs.read(path).ok().flatten()?;
                    let bytes: Arc<[u8]> = if path.ends_with(".wav") { plain_wav(&data)?.into() } else { data.into() };
                    Some(audio.add(AudioSource { bytes }))
                })
                .clone(),
            SoundFile::Bo1(bank, i) => self
                .bo1
                .entry(*i)
                .or_insert_with(|| {
                    let sound = bank.sounds.get(*i)?;
                    let pcm = bank.decode(sound).map_err(|e| warn!("audio: {e:#}")).ok()?;
                    debug!("audio: decoded Black Ops sound {} ({:.2}s)", sound.name, pcm.seconds());
                    Some(audio.add(AudioSource { bytes: pcm.to_wav().into() }))
                })
                .clone(),
            SoundFile::Waw(bytes) => self
                .waw
                .entry(bytes.as_ptr() as usize)
                .or_insert_with(|| {
                    let pcm = t4::sound::decode_wav(bytes).map_err(|e| warn!("audio: World at War sound: {e:#}")).ok()?;
                    Some(audio.add(AudioSource { bytes: pcm.into() }))
                })
                .clone(),
            SoundFile::WawStreamed(vfs, path) => self
                .waw_streamed
                .entry(path.clone())
                .or_insert_with(|| {
                    let data = vfs.read(path).ok().flatten()?;
                    let pcm = t4::sound::decode_wav(&data).map_err(|e| warn!("audio: {path}: {e:#}")).ok()?;
                    Some(audio.add(AudioSource { bytes: pcm.into() }))
                })
                .clone(),
        }
    }
}

/// Black Ops' weapon sound aliases (`wpn_*`, `fly_*`) as the game's.
pub fn bo1_aliases(bank: &Arc<t5::sound::SoundBank>) -> Vec<(String, Vec<Variant>)> {
    let names: Vec<String> =
        bank.alias_names().filter(|n| n.starts_with("wpn_") || n.starts_with("fly_")).map(str::to_owned).collect();
    names
        .into_iter()
        .map(|name| {
            let variants = bank
                .variants(&name)
                .unwrap_or_default()
                .iter()
                .filter_map(|a| {
                    Some(Variant {
                        file: SoundFile::Bo1(bank.clone(), a.sound?),
                        volume: (a.volume[0], a.volume[1]),
                        pitch: (a.pitch[0], a.pitch[1]),
                        dist: (a.distance[0], a.distance[1]),
                        probability: a.probability,
                        looping: a.looping(),
                        two_d: !a.spatialized(),
                        curve: Vec::new(),
                        secondary: a.secondary.clone().filter(|s| !s.is_empty()),
                        master: false,
                        slave: None,
                    })
                })
                .collect();
            (name.to_ascii_lowercase(), variants)
        })
        .collect()
}

/// World at War's aliases `names` (and the layers they name) from its zones
/// as the game's, under [`crate::waw::sound_alias`] names. A zone may only
/// refer to a loaded sound that another zone holds (`,name`), so each is
/// looked up in every zone given. Falloff curves are the mixer's.
pub fn waw_aliases(
    zones: &[t4::zone::Zone],
    mixer: Option<&t4::sound::Mixer>,
    vfs: &Arc<iw3::iwd::Vfs>,
    names: &std::collections::HashSet<String>,
) -> Vec<(String, Vec<Variant>)> {
    use t4::sound::SoundFile as F;
    let all: Vec<t4::sound::Alias> = zones.iter().flat_map(t4::sound::aliases).collect();
    let loaded: HashMap<String, Arc<[u8]>> = all
        .iter()
        .flat_map(|a| &a.variants)
        .filter_map(|v| match &v.file {
            F::Loaded { name, data: Some(d) } => Some((name.to_ascii_lowercase(), d.clone())),
            _ => None,
        })
        .collect();
    let by_name: HashMap<String, &t4::sound::Alias> = all.iter().map(|a| (a.name.to_ascii_lowercase(), a)).collect();
    // The aliases asked for and their layers.
    let mut wanted: Vec<String> = names.iter().map(|n| n.to_ascii_lowercase()).collect();
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    while let Some(name) = wanted.pop() {
        let Some(alias) = by_name.get(&name).filter(|_| seen.insert(name.clone())) else { continue };
        let variants = alias
            .variants
            .iter()
            .filter_map(|v| {
                let file = match &v.file {
                    F::Loaded { name, data } => SoundFile::Waw(data.clone().or_else(|| loaded.get(&name.to_ascii_lowercase()).cloned())?),
                    F::Streamed(path) => SoundFile::WawStreamed(vfs.clone(), path.clone()),
                };
                let secondary = v.secondary.as_ref().map(|s| s.to_ascii_lowercase());
                wanted.extend(secondary.clone());
                Some(Variant {
                    file,
                    volume: v.volume,
                    pitch: v.pitch,
                    dist: v.dist,
                    probability: v.probability,
                    looping: v.looping,
                    two_d: !v.spatial,
                    curve: mixer.and_then(|m| m.curves.get(v.curve)).map(|c| c.knots.clone()).unwrap_or_default(),
                    secondary: secondary.map(|s| crate::waw::sound_alias(&s)),
                    master: false,
                    slave: None,
                })
            })
            .collect();
        out.push((crate::waw::sound_alias(&name), variants));
    }
    out
}

/// Pick a variation by probability.
pub fn pick(variants: &[Variant]) -> Option<&Variant> {
    let total: f32 = variants.iter().map(|v| v.probability.max(0.0)).sum();
    if total <= 0.0 {
        return variants.get(rand::random_range(0..variants.len().max(1)));
    }
    let mut r = rand::random_range(0.0..total);
    for v in variants {
        r -= v.probability.max(0.0);
        if r <= 0.0 {
            return Some(v);
        }
    }
    variants.last()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(curve: Vec<[f32; 2]>) -> Variant {
        Variant {
            file: SoundFile::Streamed(String::new()),
            volume: (1.0, 1.0),
            pitch: (1.0, 1.0),
            dist: (100.0, 300.0),
            probability: 1.0,
            looping: false,
            two_d: false,
            curve,
            secondary: None,
            master: false,
            slave: None,
        }
    }

    #[test]
    fn falls_off() {
        let lin = v(Vec::new());
        assert_eq!(lin.falloff(50.0), 1.0);
        assert!((lin.falloff(200.0) - 0.5).abs() < 1e-5);
        assert_eq!(lin.falloff(400.0), 0.0);
        let curved = v(vec![[0.0, 1.0], [0.5, 0.2], [1.0, 0.0]]);
        assert!((curved.falloff(150.0) - 0.6).abs() < 1e-5);
    }

    #[test]
    fn repacks_streamed_wavs() {
        let mut f = b"RIFF\0\0\0\0WAVE".to_vec();
        f.extend_from_slice(b"LIST\x02\0\0\0ab");
        f.extend_from_slice(b"fmt \x10\0\0\0\x01\0\x01\0\x44\xac\0\0\x88\x58\x01\0\x02\0\x10\0");
        f.extend_from_slice(b"data\x04\0\0\0\x01\0\x02\0");
        let w = plain_wav(&f).unwrap();
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(&w[w.len() - 4..], &[1, 0, 2, 0]);
        f[20 + 10] = 0x11; // IMA ADPCM
        assert!(plain_wav(&f).is_none());
    }
}

//! 3D sound: each positioned sound is played through [`SpatialSound`], a
//! source that turns it into the stereo two ears would hear, from the
//! direction and distance [`spatialize`] works out every frame:
//!
//! - equal-power panning, the far ear down by up to [`FAR_EAR_DB`] at the
//!   side;
//! - the far ear hearing it up to [`MAX_ITD`] later (the inter-aural time
//!   difference) and muffled by the head (a low-pass down to
//!   [`HEAD_SHADOW_HZ`]);
//! - from behind, the highs a little cut (a high shelf), as the ears'
//!   shape does;
//! - with distance, CoD4's own falloff per alias (its min and max distance
//!   and curve) and air absorbing the highs;
//! - behind walls, quieter and muffled (a few rays a frame, kept per
//!   sound).
//!
//! The work per sample is a few one-pole filters and a short delay line,
//! so dozens of guns firing cost little; the parameters are set once a
//! frame and eased per sample so they never click.

use super::bank::Variant;
use crate::units::INCH;
use bevy::audio::{ChannelCount, Decodable, SampleRate, Source};
use bevy::prelude::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

/// How far down the far ear is at 90 degrees (dB).
pub const FAR_EAR_DB: f32 = 14.0;
/// The largest inter-aural time difference (seconds).
pub const MAX_ITD: f32 = 0.00063;
/// The far ear's low-pass at 90 degrees (Hz).
pub const HEAD_SHADOW_HZ: f32 = 2500.0;
/// From straight behind, how much of the highs (above [`SHELF_HZ`]) go.
const REAR_CUT: f32 = 0.45;
const SHELF_HZ: f32 = 4000.0;
/// Air absorption: the low-pass falls to half its frequency every this many
/// CoD units.
const AIR_HALF_DISTANCE: f32 = 1500.0;
/// Behind a wall: gain and low-pass.
const OCCLUDED_GAIN: f32 = 0.45;
const OCCLUDED_HZ: f32 = 1200.0;
/// The top of the filters' range: open (Hz).
const OPEN_HZ: f32 = 20000.0;
/// Parameters ease towards new ones over about this long (seconds).
const EASE: f32 = 0.03;
/// Occlusion rays cast a frame, among all the sounds playing.
const RAYS_PER_FRAME: usize = 8;

/// A float shared with the audio thread.
#[derive(Default)]
struct Shared(AtomicU32);

impl Shared {
    fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
    fn set(&self, v: f32) {
        self.0.store(v.to_bits(), Ordering::Relaxed);
    }
}

/// What a sound's two ears hear, set by [`spatialize`] each frame.
#[derive(Default)]
pub struct Ears {
    gain: [Shared; 2],
    /// Delay (seconds) and low-pass (Hz) per ear.
    delay: [Shared; 2],
    cutoff: [Shared; 2],
    /// How much of the highs are cut (from behind), 0..1.
    rear: Shared,
}

/// The ears' settings for a source in direction `dir` (listener space: x
/// right, z forward), `distance` away in CoD units, with `gain` and
/// `occlusion` (0 open, 1 blocked).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EarSettings {
    pub gain: [f32; 2],
    pub delay: [f32; 2],
    pub cutoff: [f32; 2],
    pub rear: f32,
}

impl EarSettings {
    pub fn new(dir: Vec3, distance: f32, gain: f32, occlusion: f32) -> EarSettings {
        let flat = Vec2::new(dir.x, dir.z);
        let (side, front) = if flat.length_squared() > 1e-8 {
            let f = flat.normalize();
            (f.x, f.y)
        } else {
            (0.0, 1.0)
        };
        // Close up, a sound fills the head rather than coming from a side.
        let near = (distance / 40.0).clamp(0.0, 1.0);
        let s = side * near;
        let far_gain = 10f32.powf(-FAR_EAR_DB * s.abs() / 20.0);
        // Equal power: the near ear takes what the far one gives up.
        let near_gain = (2.0 / (1.0 + far_gain * far_gain)).sqrt();
        let (l, r) = if s >= 0.0 { (far_gain, near_gain) } else { (near_gain, far_gain) };
        let air = OPEN_HZ * 0.5f32.powf(distance / AIR_HALF_DISTANCE);
        let shadow = OPEN_HZ * (HEAD_SHADOW_HZ / OPEN_HZ).powf(s.abs());
        let occluded = OPEN_HZ + (OCCLUDED_HZ - OPEN_HZ) * occlusion;
        let base = air.min(occluded).max(200.0);
        let far_cut = base.min(shadow);
        let gain = gain * (1.0 + (OCCLUDED_GAIN - 1.0) * occlusion);
        let itd = MAX_ITD * s.abs();
        let (cut, delay) = if s >= 0.0 { ([far_cut, base], [itd, 0.0]) } else { ([base, far_cut], [0.0, itd]) };
        EarSettings {
            gain: [l * gain, r * gain],
            delay,
            cutoff: cut,
            // Behind: front < 0.
            rear: (-front).max(0.0) * REAR_CUT * near,
        }
    }
}

impl Ears {
    pub fn set(&self, s: &EarSettings) {
        for i in 0..2 {
            self.gain[i].set(s.gain[i]);
            self.delay[i].set(s.delay[i]);
            self.cutoff[i].set(s.cutoff[i]);
        }
        self.rear.set(s.rear);
    }
}

/// A positioned sound: the alias's sound and the ears it's heard with.
///
/// A loop loops here, before its ears: Bevy loops a sound by recording its
/// first pass (`repeat_infinite`) and playing that again, which for ours
/// was the first pass as heard, so a loop kept its first place's loudness
/// and side for good, wherever the listener went (the map's ambience heard
/// "right at the ear" all over it). Played with [`PlaybackMode::Once`]:
/// it never ends.
///
/// [`PlaybackMode::Once`]: bevy::audio::PlaybackMode::Once
#[derive(Asset, TypePath, Clone)]
pub struct SpatialSound {
    pub source: AudioSource,
    pub ears: Arc<Ears>,
    pub looping: bool,
}

impl Decodable for SpatialSound {
    type Decoder = Spatializer<Box<dyn Source + Send>>;

    fn decoder(&self) -> Self::Decoder {
        let inner: Box<dyn Source + Send> =
            if self.looping { Box::new(self.source.decoder().repeat_infinite()) } else { Box::new(self.source.decoder()) };
        Spatializer::new(inner, self.ears.clone())
    }
}

/// Longest delay line needed (samples, at up to 192 kHz).
const DELAY_LEN: usize = 128;

/// The source, as heard by two ears ([`Ears`]): mono in, stereo out.
pub struct Spatializer<S: Source> {
    inner: S,
    ears: Arc<Ears>,
    in_channels: u16,
    rate: SampleRate,
    /// The right ear's sample, waiting to be returned after the left.
    pending: Option<f32>,
    /// Recent input, for each ear's delay.
    history: [f32; DELAY_LEN],
    at: usize,
    /// Eased settings, and the one-pole filters' states, per ear.
    gain: [f32; 2],
    delay: [f32; 2],
    lowpass: [f32; 2],
    shelf: [f32; 2],
    rear: f32,
    /// Coefficients, refreshed every block.
    k_low: [f32; 2],
    k_shelf: f32,
    countdown: u32,
    started: bool,
}

/// Samples between coefficient refreshes.
const BLOCK: u32 = 64;

impl<S: Source> Spatializer<S> {
    pub fn new(inner: S, ears: Arc<Ears>) -> Self {
        let in_channels = inner.channels().get();
        let rate = inner.sample_rate();
        Spatializer {
            inner,
            ears,
            in_channels,
            rate,
            pending: None,
            history: [0.0; DELAY_LEN],
            at: 0,
            gain: [0.0; 2],
            delay: [0.0; 2],
            lowpass: [0.0; 2],
            shelf: [0.0; 2],
            rear: 0.0,
            k_low: [1.0; 2],
            k_shelf: 1.0,
            countdown: 0,
            started: false,
        }
    }

    /// A one-pole low-pass's coefficient for `hz`.
    fn coefficient(&self, hz: f32) -> f32 {
        let w = std::f32::consts::TAU * hz / self.rate.get() as f32;
        (w / (1.0 + w)).clamp(0.0, 1.0)
    }

    fn refresh(&mut self) {
        let ease = (BLOCK as f32 / (EASE * self.rate.get() as f32)).min(1.0);
        let jump = !self.started;
        self.started = true;
        for i in 0..2 {
            let ease_to = |v: &mut f32, t: f32| *v = if jump { t } else { *v + (t - *v) * ease };
            ease_to(&mut self.gain[i], self.ears.gain[i].get());
            ease_to(&mut self.delay[i], self.ears.delay[i].get() * self.rate.get() as f32);
            let cut = self.ears.cutoff[i].get();
            self.k_low[i] = self.coefficient(if cut > 0.0 { cut } else { OPEN_HZ });
        }
        let rear = self.ears.rear.get();
        self.rear = if jump { rear } else { self.rear + (rear - self.rear) * ease };
        self.k_shelf = self.coefficient(SHELF_HZ);
    }

    fn ear(&mut self, i: usize) -> f32 {
        // The delayed input (linear between samples).
        let d = self.delay[i].clamp(0.0, (DELAY_LEN - 2) as f32);
        let (whole, frac) = (d.floor() as usize, d.fract());
        let a = self.history[(self.at + DELAY_LEN - whole) % DELAY_LEN];
        let b = self.history[(self.at + DELAY_LEN - whole - 1) % DELAY_LEN];
        let x = a + (b - a) * frac;
        // Head shadow, air and walls; then the rear's high shelf.
        self.lowpass[i] += self.k_low[i] * (x - self.lowpass[i]);
        let y = self.lowpass[i];
        self.shelf[i] += self.k_shelf * (y - self.shelf[i]);
        let y = y - self.rear * (y - self.shelf[i]);
        y * self.gain[i]
    }
}

impl<S: Source> Iterator for Spatializer<S> {
    type Item = bevy::audio::Sample;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(r) = self.pending.take() {
            return Some(r);
        }
        // A frame in: its channels, mixed down.
        let mut sum = self.inner.next()?;
        for _ in 1..self.in_channels {
            sum += self.inner.next().unwrap_or(0.0);
        }
        let x = sum / self.in_channels as f32;
        if self.countdown == 0 {
            self.refresh();
            self.countdown = BLOCK;
        }
        self.countdown -= 1;
        self.at = (self.at + 1) % DELAY_LEN;
        self.history[self.at] = x;
        let l = self.ear(0);
        let r = self.ear(1);
        self.pending = Some(r);
        Some(l)
    }
}

impl<S: Source> Source for Spatializer<S> {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> ChannelCount {
        ChannelCount::new(2).expect("two")
    }

    fn sample_rate(&self) -> SampleRate {
        self.rate
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
}

/// A positioned sound playing: its ears, the alias's falloff, its gain, and
/// how blocked it is (eased).
#[derive(Component)]
pub struct Emitter {
    pub alias: String,
    pub ears: Arc<Ears>,
    pub variant: Variant,
    pub gain: f32,
    occlusion: f32,
    occluded: bool,
}

impl Emitter {
    pub fn new(alias: String, ears: Arc<Ears>, variant: Variant, gain: f32) -> Self {
        Emitter { alias, ears, variant, gain, occlusion: 0.0, occluded: false }
    }
}

/// Each playing sound's ears for where it is from the listener, its
/// falloff and its occlusion: a few occlusion rays a frame, round the
/// sounds in turn.
pub fn spatialize(
    time: Res<Time>,
    listener: Query<&GlobalTransform, With<SpatialListener>>,
    mut emitters: Query<(&GlobalTransform, &mut Emitter)>,
    spatial: avian3d::prelude::SpatialQuery,
    mut next_ray: Local<usize>,
    mut next_log: Local<f32>,
) {
    // `COD4RW_AUDIOLOG`: every 5 s, the 3D sounds heard and how loud.
    let log = std::env::var_os("COD4RW_AUDIOLOG").is_some() && time.elapsed_secs() >= *next_log;
    if log {
        *next_log = time.elapsed_secs() + 5.0;
    }
    let Some(ear) = listener.iter().next() else { return };
    let (eye, to_listener) = (ear.translation(), ear.affine().inverse());
    let filter = crate::collision::sight_filter();
    let count = emitters.iter().len().max(1);
    let (first, k) = (*next_ray % count, 1.0 - (-time.delta_secs() / 0.15).exp());
    for (i, (tf, mut e)) in emitters.iter_mut().enumerate() {
        let at = tf.translation();
        let distance = eye.distance(at) / INCH;
        // Rays for a few sounds a frame, the rest keep their last answer.
        if (i + count - first) % count < RAYS_PER_FRAME
            && let Ok(dir) = Dir3::new(at - eye)
        {
            let len = eye.distance(at);
            e.occluded = len > 0.5 && spatial.cast_ray(eye, dir, len - 0.25, true, &filter).is_some();
        }
        let target = if e.occluded { 1.0 } else { 0.0 };
        e.occlusion += (target - e.occlusion) * k;
        // Listener space: Bevy's camera looks down -Z.
        let local = to_listener.transform_vector3(at - eye);
        let dir = Vec3::new(local.x, local.y, -local.z);
        let gain = e.gain * e.variant.falloff(distance) * super::audible();
        let settings = EarSettings::new(dir, distance, gain, e.occlusion);
        // (Logged as heard with sound on: before `audible()`.)
        let heard = EarSettings::new(dir, distance, e.gain * e.variant.falloff(distance), e.occlusion);
        if log && heard.gain[0].max(heard.gain[1]) > 0.005 {
            info!("audiolog: {} at {:.1} m, ear gain {:.3} (falloff {:.3})", e.alias, distance * INCH, heard.gain[0].max(heard.gain[1]), e.variant.falloff(distance));
        }
        e.ears.set(&settings);
    }
    *next_ray = (*next_ray + RAYS_PER_FRAME) % count;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test source: a fixed buffer, mono.
    struct Buffer(std::vec::IntoIter<f32>, u32);
    impl Iterator for Buffer {
        type Item = f32;
        fn next(&mut self) -> Option<f32> {
            self.0.next()
        }
    }
    impl Source for Buffer {
        fn current_span_len(&self) -> Option<usize> {
            None
        }
        fn channels(&self) -> ChannelCount {
            ChannelCount::new(1).unwrap()
        }
        fn sample_rate(&self) -> SampleRate {
            SampleRate::new(self.1).unwrap()
        }
        fn total_duration(&self) -> Option<Duration> {
            None
        }
    }

    const RATE: u32 = 48000;

    fn noise(n: usize) -> Vec<f32> {
        let mut x = 1u32;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                (x as f32 / u32::MAX as f32) * 2.0 - 1.0
            })
            .collect()
    }

    /// Render `input` heard from `azimuth` degrees (0 ahead, 90 right) at
    /// `distance` CoD units: (left, right) samples.
    fn render(input: &[f32], azimuth: f32, distance: f32, occlusion: f32) -> (Vec<f32>, Vec<f32>) {
        let ears = Arc::new(Ears::default());
        let a = azimuth.to_radians();
        ears.set(&EarSettings::new(Vec3::new(a.sin(), 0.0, a.cos()), distance, 1.0, occlusion));
        let out: Vec<f32> = Spatializer::new(Buffer(input.to_vec().into_iter(), RATE), ears).collect();
        (out.iter().step_by(2).copied().collect(), out.iter().skip(1).step_by(2).copied().collect())
    }

    fn db(x: &[f32]) -> f32 {
        let rms = (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
        20.0 * rms.max(1e-9).log10()
    }

    /// The level of the highs (a crude high-pass: the sample-to-sample change).
    fn highs_db(x: &[f32]) -> f32 {
        db(&x.windows(2).map(|w| w[1] - w[0]).collect::<Vec<_>>())
    }

    #[test]
    fn levels_by_direction() {
        let input = noise(RATE as usize);
        let mut report = String::new();
        for az in [0.0, 90.0, 180.0, 270.0] {
            let (l, r) = render(&input, az, 300.0, 0.0);
            report += &format!("{az:>5}: L {:.1} dB, R {:.1} dB, highs L {:.1} R {:.1}\n", db(&l), db(&r), highs_db(&l), highs_db(&r));
        }
        println!("{report}");
        let (l0, r0) = render(&input, 0.0, 300.0, 0.0);
        assert!((db(&l0) - db(&r0)).abs() < 0.1, "ahead: balanced");
        let (l90, r90) = render(&input, 90.0, 300.0, 0.0);
        assert!(db(&r90) - db(&l90) >= 12.0, "right: the left ear at least 12 dB down");
        let (l270, r270) = render(&input, 270.0, 300.0, 0.0);
        assert!(db(&l270) - db(&r270) >= 12.0, "left: mirrored");
        let (l180, r180) = render(&input, 180.0, 300.0, 0.0);
        assert!((db(&l180) - db(&r180)).abs() < 0.1, "behind: balanced");
        assert!(highs_db(&l180) < highs_db(&l0) - 2.0, "behind: duller than ahead");
    }

    #[test]
    fn far_ear_hears_later() {
        // A click: the far ear's arrives about MAX_ITD later.
        let mut input = vec![0.0; 4800];
        input[1000] = 1.0;
        let (l, r) = render(&input, 90.0, 300.0, 0.0);
        let peak = |x: &[f32]| x.iter().enumerate().max_by(|a, b| a.1.abs().total_cmp(&b.1.abs())).unwrap().0;
        let lag = peak(&l) as f32 - peak(&r) as f32;
        let expected = MAX_ITD * RATE as f32;
        assert!((lag - expected).abs() <= 3.0, "lag {lag} samples, expected about {expected}");
    }

    #[test]
    fn distance_and_walls_dull_and_quieten() {
        let input = noise(RATE as usize / 2);
        let (near, _) = render(&input, 0.0, 100.0, 0.0);
        let (far, _) = render(&input, 0.0, 3000.0, 0.0);
        assert!(highs_db(&far) < highs_db(&near) - 3.0, "air absorbs the highs");
        let (open, _) = render(&input, 0.0, 300.0, 0.0);
        let (blocked, _) = render(&input, 0.0, 300.0, 1.0);
        assert!(db(&blocked) < db(&open) - 6.0, "a wall quietens");
    }

    #[test]
    fn stereo_in_is_mixed_down() {
        let ears = Arc::new(Ears::default());
        ears.set(&EarSettings::new(Vec3::Z, 300.0, 1.0, 0.0));
        struct Stereo(std::vec::IntoIter<f32>);
        impl Iterator for Stereo {
            type Item = f32;
            fn next(&mut self) -> Option<f32> {
                self.0.next()
            }
        }
        impl Source for Stereo {
            fn current_span_len(&self) -> Option<usize> {
                None
            }
            fn channels(&self) -> ChannelCount {
                ChannelCount::new(2).unwrap()
            }
            fn sample_rate(&self) -> SampleRate {
                SampleRate::new(RATE).unwrap()
            }
            fn total_duration(&self) -> Option<Duration> {
                None
            }
        }
        let out: Vec<f32> = Spatializer::new(Stereo(vec![1.0, 0.0].repeat(1000).into_iter()), ears).collect();
        assert_eq!(out.len(), 2000, "one stereo frame per input frame");
        assert!((out[1999] - 0.5).abs() < 0.05);
    }

    /// With `SPATIAL_WAV_DIR` set, writes stereo WAVs of a decaying noise
    /// burst ("a shot") from each side, near and far, for listening to
    /// offline (never played by the test).
    #[test]
    fn offline_renders() {
        let Some(dir) = std::env::var_os("SPATIAL_WAV_DIR") else { return };
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        let shot: Vec<f32> = noise(RATE as usize / 2).iter().enumerate().map(|(i, v)| v * (-(i as f32) / 2400.0).exp()).collect();
        for (name, az, dist, occ) in [("ahead", 0.0, 300.0, 0.0), ("right", 90.0, 300.0, 0.0), ("behind", 180.0, 300.0, 0.0), ("left", 270.0, 300.0, 0.0), ("far_right", 60.0, 3000.0, 0.0), ("walled_ahead", 0.0, 300.0, 1.0)] {
            let (l, r) = render(&shot, az, dist, occ);
            let mut data = Vec::with_capacity(l.len() * 4);
            for (a, b) in l.iter().zip(&r) {
                for v in [a, b] {
                    data.extend(((v.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
                }
            }
            let mut wav = Vec::new();
            wav.extend(b"RIFF");
            wav.extend((36 + data.len() as u32).to_le_bytes());
            wav.extend(b"WAVEfmt ");
            wav.extend(16u32.to_le_bytes());
            wav.extend(1u16.to_le_bytes());
            wav.extend(2u16.to_le_bytes());
            wav.extend(RATE.to_le_bytes());
            wav.extend((RATE * 4).to_le_bytes());
            wav.extend(4u16.to_le_bytes());
            wav.extend(16u16.to_le_bytes());
            wav.extend(b"data");
            wav.extend((data.len() as u32).to_le_bytes());
            wav.extend(data);
            std::fs::write(dir.join(format!("{name}.wav")), wav).unwrap();
        }
    }

    /// The cost per sound: printed (release builds: `--nocapture`).
    #[test]
    fn cost_per_second_of_sound() {
        let input = noise(RATE as usize);
        let t = std::time::Instant::now();
        let (l, _) = render(&input, 45.0, 800.0, 0.3);
        let took = t.elapsed();
        assert_eq!(l.len(), input.len());
        println!("one second of 48 kHz sound spatialized in {took:.2?} ({:.1} ns a frame)", took.as_nanos() as f64 / input.len() as f64);
    }

    /// No hiss of its own: a pure tone in, the same tone out (whatever
    /// the direction, moving or not), and silence in, silence out.
    #[test]
    fn adds_no_noise() {
        let n = RATE as usize;
        let tone: Vec<f32> = (0..n).map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / RATE as f32).sin() * 0.5).collect();
        for az in [0.0, 45.0, 90.0, 135.0, 180.0, 270.0] {
            for (dist, occ) in [(30.0, 0.0), (300.0, 0.0), (3000.0, 1.0)] {
                let (l, r) = render(&tone, az, dist, occ);
                for out in [l, r] {
                    // The best fit of a 440 Hz sine; what's left is noise.
                    let (mut sc, mut cc) = (0.0f64, 0.0f64);
                    let tail = &out[n / 4..];
                    for (i, v) in tail.iter().enumerate() {
                        let ph = (i + n / 4) as f64 * 440.0 * std::f64::consts::TAU / RATE as f64;
                        sc += *v as f64 * ph.sin();
                        cc += *v as f64 * ph.cos();
                    }
                    let (a, b) = (2.0 * sc / tail.len() as f64, 2.0 * cc / tail.len() as f64);
                    let residual: f64 = tail
                        .iter()
                        .enumerate()
                        .map(|(i, v)| {
                            let ph = (i + n / 4) as f64 * 440.0 * std::f64::consts::TAU / RATE as f64;
                            (*v as f64 - a * ph.sin() - b * ph.cos()).powi(2)
                        })
                        .sum::<f64>()
                        / tail.len() as f64;
                    let signal = (a * a + b * b) / 2.0;
                    assert!(residual < signal * 1e-6 + 1e-12, "az {az} dist {dist}: noise {:.1} dB under the tone", 10.0 * (residual / signal).log10());
                }
            }
        }
        let (l, r) = render(&vec![0.0; n], 90.0, 300.0, 0.0);
        assert!(l.iter().chain(&r).all(|v| *v == 0.0), "silence stays silent");
    }

    /// A loop keeps following its ears past its first pass (Bevy's own
    /// looping replayed the first pass as heard: frozen in place).
    #[test]
    fn loops_follow_their_ears() {
        let clip = noise(4800);
        let ears = Arc::new(Ears::default());
        ears.set(&EarSettings::new(Vec3::Z, 300.0, 1.0, 0.0));
        let inner: Box<dyn Source + Send> = Box::new(Buffer(clip.clone().into_iter(), RATE).repeat_infinite());
        let mut s = Spatializer::new(inner, ears.clone());
        let first: Vec<f32> = (&mut s).take(4800 * 2 * 2).collect();
        ears.set(&EarSettings::new(Vec3::Z, 300.0, 0.1, 0.0));
        let _settle: Vec<f32> = (&mut s).take(4800 * 2).collect();
        let later: Vec<f32> = (&mut s).take(4800 * 2 * 2).collect();
        let drop = db(&first) - db(&later);
        assert!((drop - 20.0).abs() < 1.5, "a loop's third pass is 20 dB down when its ears are, got {drop:.1}");
    }
}

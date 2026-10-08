//! Baked lighting that follows the time of day (the Wet Work showcase,
//! `COD4RW_SHOWCASE=1`). The map is baked at a few hours of the day
//! (`crate::bake::tod`); here the two keyframes either side of the clock
//! are blended into the lightmaps and the light grid the world already
//! uses. Blending runs on a worker thread whenever the mix has moved by
//! [`STEP`], and the result replaces the images' data, so the world shader
//! needs nothing new.
//!
//! The clock is `atmos::climate::TimeOfDay`'s (`COD4RW_TOD`,
//! `COD4RW_TOD_SPEED`); the keyframes are lit by its sun path.

use crate::bake::{cache, tod};
use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

pub struct TodLightPlugin;

impl Plugin for TodLightPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, blend.run_if(crate::state::in_game).run_if(crate::atmos::climate::on));
    }
}


/// How far the mix between two keyframes moves before the light is blended
/// again.
const STEP: f32 = 1.0 / 32.0;

/// One keyframe's bake.
struct Keyframe {
    hour: f32,
    baked: cache::Baked,
}

/// What a blend produces: the lightmaps' data (RGBA half floats) and the
/// light grid volume's.
struct Blended {
    mix: (usize, f32),
    atlases: Vec<Option<Vec<u8>>>,
    grid: Option<Vec<u8>>,
}

#[derive(Resource)]
pub struct TodLight {
    keys: Arc<Vec<Keyframe>>,
    /// The lightmap images the world draws with, rewritten on each blend.
    live: Vec<Option<Handle<Image>>>,
    /// The keyframe pair (first's index) and mix last blended.
    shown: (usize, f32),
    pending: Option<std::sync::Mutex<Receiver<Blended>>>,
}

/// The keyframe pair around `hour` (index of the earlier one) and how far
/// towards the later one it is.
fn mix_at(keys: &[Keyframe], hour: f32) -> (usize, f32) {
    let n = keys.len();
    for i in 0..n {
        let (a, b) = (keys[i].hour, keys[(i + 1) % n].hour);
        let span = (b - a).rem_euclid(24.0).max(1e-3);
        let into = (hour - a).rem_euclid(24.0);
        if into <= span {
            return (i, into / span);
        }
    }
    (0, 0.0)
}

fn lerp_halves(a: &[u16], b: &[u16], t: f32) -> Vec<u8> {
    a.iter()
        .zip(b)
        .flat_map(|(&x, &y)| {
            let v = half::f16::from_bits(x).to_f32() * (1.0 - t) + half::f16::from_bits(y).to_f32() * t;
            half::f16::from_f32(v).to_bits().to_le_bytes()
        })
        .collect()
}

fn blend_job(keys: &[Keyframe], mix: (usize, f32), volume: Option<&crate::model_lighting::GridVolume>) -> Blended {
    let (a, b) = (&keys[mix.0].baked, &keys[(mix.0 + 1) % keys.len()].baked);
    let t = mix.1;
    let atlases = a
        .atlases
        .iter()
        .zip(&b.atlases)
        .map(|(x, y)| match (x, y) {
            (Some(x), Some(y)) if x.data.len() == y.data.len() => Some(lerp_halves(&x.data, &y.data, t)),
            _ => None,
        })
        .collect();
    let grid = volume.map(|v| {
        v.data(|p| {
            let (ca, cb) = (a.grid.get(p)?.1, b.grid.get(p)?.1);
            Some(std::array::from_fn(|f| std::array::from_fn(|c| ca[f][c] * (1.0 - t) + cb[f][c] * t)))
        })
    });
    Blended { mix, atlases, grid }
}

/// The showcase's keyframe bakes of `map`, if all are cached: the lighting
/// to start with (`crate::lightmaps::Rebaked`) and the blending state.
pub fn load(map: &str, zone_path: &std::path::Path, images: &mut Assets<Image>, hour: f32) -> Option<(crate::lightmaps::Rebaked, TodLight)> {
    let stamp = cache::source_stamp(zone_path);
    let mut keys = Vec::new();
    for key in &tod::KEYS {
        let Some(baked) = cache::dir_for(map, Some(&tod::variant(key))).and_then(|d| cache::load_in(&d, stamp)) else {
            info!("showcase lighting: no {} bake of {map} (rebake --showcase {map} makes them)", key.name);
            return None;
        };
        keys.push(Keyframe { hour: key.hour, baked });
    }
    let mix = mix_at(&keys, hour);
    let first = blend_job(&keys, mix, None);
    let live: Vec<Option<Handle<Image>>> = first
        .atlases
        .into_iter()
        .zip(&keys[0].baked.atlases)
        .map(|(data, a)| {
            let (data, a) = (data?, a.as_ref()?);
            let mut image = Image::new(
                Extent3d { width: a.w, height: a.h * 2, depth_or_array_layers: 1 },
                TextureDimension::D2,
                data,
                TextureFormat::Rgba16Float,
                // Kept in the main world too: each blend rewrites it.
                RenderAssetUsages::default(),
            );
            image.sampler = ImageSampler::linear();
            Some(images.add(image))
        })
        .collect();
    // The grid to start with, blended the same way.
    let (a, b) = (&keys[mix.0].baked, &keys[(mix.0 + 1) % keys.len()].baked);
    let grid = a
        .grid
        .iter()
        .zip(&b.grid)
        .map(|((p, ca), (_, cb))| (*p, std::array::from_fn(|f| std::array::from_fn(|c| ca[f][c] * (1.0 - mix.1) + cb[f][c] * mix.1))))
        .collect();
    info!("showcase lighting: {} keyframes of {map}, starting at {hour:.1}h ({} towards {}, {:.2})", keys.len(), tod::KEYS[mix.0].name, tod::KEYS[(mix.0 + 1) % keys.len()].name, mix.1);
    let rebaked = crate::lightmaps::Rebaked { lightmaps: live.clone(), grid };
    Some((rebaked, TodLight { keys: Arc::new(keys), live, shown: mix, pending: None }))
}

fn blend(
    clock: Res<crate::atmos::climate::TimeOfDay>,
    tod: Option<ResMut<TodLight>>,
    volume: Option<Res<crate::model_lighting::GridVolume>>,
    mut images: ResMut<Assets<Image>>,
) {
    let Some(mut tod) = tod else { return };
    // (The clock starts once the map's sun is learnt.)
    if clock.map.is_none() {
        return;
    }
    // A finished blend: into the images.
    if let Some(rx) = &tod.pending {
        let Ok(done) = rx.lock().map_err(|_| ()).and_then(|rx| rx.try_recv().map_err(|_| ())) else { return };
        for (handle, data) in tod.live.iter().zip(done.atlases) {
            if let (Some(h), Some(data)) = (handle, data) {
                if let Some(mut image) = images.get_mut(h) {
                    image.data = Some(data);
                }
            }
        }
        if let (Some(v), Some(data)) = (&volume, done.grid) {
            if let Some(mut image) = images.get_mut(&v.image) {
                image.data = Some(data);
            }
        }
        tod.shown = done.mix;
        tod.pending = None;
    }
    let mix = mix_at(&tod.keys, clock.hours);
    if mix.0 == tod.shown.0 && (mix.1 - tod.shown.1).abs() < STEP {
        return;
    }
    let keys = tod.keys.clone();
    let volume = volume.map(|v| crate::model_lighting::GridVolume::clone(&v));
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let _ = tx.send(blend_job(&keys, mix, volume.as_ref()));
    });
    tod.pending = Some(std::sync::Mutex::new(rx));
}

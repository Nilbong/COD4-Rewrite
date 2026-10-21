//! Map Lighting: Live (`r_lightmaps live`, `COD4RW_MAPLIGHTING=live`). The
//! map's sky and sun bounce solved from its geometry as it loads
//! (`crate::bake::gi`), with the whole sun drawn live by `crate::world` and
//! its shadow maps: sunlight through a window makes a bright, crisp patch
//! and the room around it is lit by what it bounces.
//!
//! A worker thread (three rayon threads at most, and it waits while
//! `target/bake.pause` exists) loads the map, traces it once (a few seconds;
//! the map's re-bake shows meanwhile), then solves whenever the light
//! changes and sends the lightmaps back, resampled to the images' size.
//! Here they're swapped into the images (the path `crate::tod_light`
//! takes), at most twice a second.
//!
//! Knobs: `gi.sky` (the sky's strength, 1.2), `gi.albedo` (bounce, 1.7: CoD4's
//! colour maps carry their own shading and run dark), `gi.floor` (an even fill,
//! 0.05 of the sky's), `gi.lamps` (the lamps' bounce, 1), `gi.grid` (models, 0.2),
//! `gi.models` (1),
//! `gi.bounces` (4), `gi.freeze` (1: stop relighting). The sun follows the
//! Map Look Editor's bearing and elevation (`sun.azimuth.<map>`,
//! `sun.elevation.<map>`) and brightness: drag it and the bounce follows.

use crate::bake::gi;
use bevy::prelude::*;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, channel};

pub struct GiLivePlugin;

impl Plugin for GiLivePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (update, steer).run_if(crate::state::in_game).run_if(resource_exists::<GiLive>))
            // The match over: the worker ends with its channel (and lets go
            // of the map), and no late solve reaches the next map's images.
            .add_systems(OnExit(crate::state::GameState::InGame), |mut commands: Commands| commands.remove_resource::<GiLive>());
    }
}

/// What the light should be, over the map's own (sent to the worker).
#[derive(Clone, Copy, PartialEq, Debug)]
struct Wanted {
    sky: f32,
    albedo: f32,
    bounces: u32,
    /// Towards the sun, CoD axes, when moved off the map's.
    to_sun: Option<[f32; 3]>,
    sun_scale: f32,
    /// The light's colour (brightest channel 1): the moon's, by night.
    sun_colour: [f32; 3],
    /// How far into night (0 day, 1 night): a bluer sky, the moon's light
    /// lifted (`gi.moon`).
    night: f32,
    grid_scale: f32,
    floor: f32,
    lamps: f32,
    lamp_range: f32,
}

/// A solve: the lightmaps (resampled) and the light grid's cubes.
type Solved = (Vec<Option<Vec<u16>>>, Vec<Option<[[f32; 3]; 6]>>);

#[derive(Resource)]
pub struct GiLive {
    images: Vec<Option<Handle<Image>>>,
    to_worker: Sender<Wanted>,
    from_worker: Mutex<Receiver<Solved>>,
    sent: Option<Wanted>,
    /// The sun's illuminance when lighting started (the map's own).
    base_lux: Option<f32>,
    last_upload: f32,
    pending: Option<Solved>,
}

/// What the sun is now (the world's "sun" light, whatever moves it: the
/// Map Look Editor's bearing, elevation, time of day or brightness, the
/// climate's clock): towards it (CoD axes), its light against the map's
/// own, and how far into night it is.
#[derive(Clone, Copy, Default)]
struct SunNow {
    to_sun: Option<[f32; 3]>,
    scale: f32,
    night: f32,
    colour: Option<[f32; 3]>,
}

fn wanted(sun: SunNow) -> Wanted {
    // (Rounded, so a running clock relights in steps, not every frame.)
    let q = |v: f32, step: f32| (v / step).round() * step;
    let to_sun = sun.to_sun.map(|d| d.map(|v| q(v, 0.004)));
    Wanted {
        // (The sky dims into night with the clock.)
        // (By night the sky keeps half: a night sky lights a street faintly,
        // blue; at 0.15 Wet Work's night went black.)
        sky: crate::tune::get("gi.sky", 1.2).max(0.0) * q(1.0 - 0.5 * sun.night, 0.02),
        albedo: crate::tune::get("gi.albedo", 1.7).clamp(0.0, 2.0),
        bounces: crate::tune::get("gi.bounces", 4.0).round().clamp(0.0, 12.0) as u32,
        to_sun,
        sun_scale: q(sun.scale, 0.02),
        sun_colour: sun.colour.map_or([0.0; 3], |c| c.map(|v| q(v, 0.02))),
        night: q(sun.night, 0.05),
        grid_scale: crate::tune::get("gi.grid", 0.2).max(0.0),
        floor: crate::tune::get("gi.floor", 0.05).clamp(0.0, 1.0),
        // (The game's lamp lights, `crate::fixture_lights`, times `gi.lamps`.)
        lamps: crate::tune::get("lamps.light", 150_000.0) * crate::tune::get("gi.lamps", 1.0).max(0.0),
        lamp_range: crate::tune::get("lamps.range", 5.0) / crate::units::INCH,
    }
}

/// Starts lighting `map` live into the re-bake's `images`.
pub fn start(commands: &mut Commands, map: &str, images: &[Option<Handle<Image>>], assets: &Assets<Image>) {
    let sizes: Vec<Option<(u32, u32)>> = images
        .iter()
        .map(|h| h.as_ref().and_then(|h| assets.get(h)).map(|i| (i.texture_descriptor.size.width, i.texture_descriptor.size.height / 2)))
        .collect();
    let (to_worker, rx) = channel::<Wanted>();
    let (tx, from_worker) = channel();
    let map = map.to_owned();
    let first = wanted(SunNow { scale: 1.0, ..default() });
    std::thread::Builder::new()
        .name("gi-live".into())
        .spawn(move || {
            let Ok(pool) = rayon::ThreadPoolBuilder::new().num_threads(3).thread_name(|i| format!("gi-live-{i}")).build() else { return };
            // (The receiver stays on this thread: the pool runs the work.)
            let rx = Mutex::new(rx);
            pool.install(|| {
                let rx = rx.lock().expect("gi-live channel");
                let t = std::time::Instant::now();
                let (tr, base) = match gi::load_and_trace(&map) {
                    Ok(v) => v,
                    Err(e) => {
                        warn!("live lighting: {map} couldn't be traced: {e}");
                        return;
                    }
                };
                info!("live lighting: {map} traced in {:.1}s ({})", t.elapsed().as_secs_f32(), tr.stats());
                let mut want = first;
                loop {
                    // The latest wish only.
                    while let Ok(w) = rx.try_recv() {
                        want = w;
                    }
                    let mut light = base;
                    light.sky = want.sky;
                    light.albedo = want.albedo;
                    light.bounces = want.bounces;
                    // The light's own colour (the moon's by night), lifted by
                    // night for the bounce (`gi.moon`: the moon alone lit
                    // next to nothing).
                    let colour = glam33::Vec3::from(want.sun_colour);
                    let colour = if colour.max_element() > 0.0 { colour / colour.max_element() } else { base.sun / base.sun.max_element().max(1e-6) };
                    let moon = 1.0 + (crate::tune::get("gi.moon", 2.0).max(0.0) - 1.0) * want.night;
                    light.sun = colour * base.sun.max_element() * want.sun_scale * moon;
                    light.sky_tint = glam33::Vec3::ONE.lerp(glam33::Vec3::new(0.62, 0.78, 1.15), want.night);
                    // A moonlit night sky (`gi.night_sky`), blue.
                    light.night_sky = glam33::Vec3::new(0.6, 0.75, 1.0) * crate::tune::get("gi.night_sky", 0.08).max(0.0) * want.night;
                    light.grid_scale = want.grid_scale;
                    light.floor = want.floor;
                    light.lamp_lumens = want.lamps;
                    light.lamp_range = want.lamp_range;
                    if let Some(d) = want.to_sun {
                        light.to_sun = glam33::Vec3A::from(d).normalize_or(base.to_sun);
                    }
                    gi::wait_if_paused();
                    // (A light that isn't finite would solve to NaN, and NaN
                    // uploaded hung the GPU: keep the last good one.)
                    let finite = light.to_sun.is_finite() && light.sun.is_finite() && light.sky.is_finite() && light.albedo.is_finite() && light.to_sun.length_squared() > 0.5;
                    if !finite {
                        warn!("live lighting: skipping a light that isn't finite: {light:?}");
                        match rx.recv() {
                            Ok(w) => want = w,
                            Err(_) => return,
                        }
                        continue;
                    }
                    info!("live lighting: solving for sun {:.3?} x{:.2}, sky x{:.2}", light.to_sun, want.sun_scale, light.sky);
                    let (atlases, mut cubes) = tr.solve(&light);
                    let mut bad = 0usize;
                    for c in cubes.iter_mut().flatten() {
                        for f in c.iter_mut() {
                            for v in f.iter_mut() {
                                if !v.is_finite() {
                                    *v = 0.0;
                                    bad += 1;
                                }
                            }
                        }
                    }
                    let out: Vec<Option<Vec<u16>>> = atlases
                        .iter()
                        .zip(&sizes)
                        .map(|(a, s)| match (a, s) {
                            (Some(a), Some((w, h))) => Some(gi::resample(a, *w, *h).into_iter().map(|v| if half::f16::from_bits(v).is_finite() { v } else { bad += 1; 0 }).collect()),
                            _ => None,
                        })
                        .collect();
                    if bad > 0 {
                        warn!("live lighting: {bad} values weren't finite (zeroed)");
                    }
                    if tx.send((out, cubes)).is_err() {
                        return;
                    }
                    // Wait for the next change (the match ending drops the
                    // sender and ends the thread).
                    match rx.recv() {
                        Ok(w) => want = w,
                        Err(_) => return,
                    }
                }
            });
        })
        .ok();
    commands.insert_resource(GiLive { images: images.to_vec(), to_worker, from_worker: Mutex::new(from_worker), sent: Some(first), base_lux: None, last_upload: 0.0, pending: None });
}

/// A finished solve into the images (twice a second at most).
fn update(
    time: Res<Time>,
    mut live: ResMut<GiLive>,
    mut images: ResMut<Assets<Image>>,
    volume: Option<Res<crate::model_lighting::GridVolume>>,
    mut lookup: Option<ResMut<crate::model_lighting::LightGridLookup>>,
) {
    let latest = live.from_worker.lock().ok().and_then(|rx| {
        let mut latest = None;
        while let Ok(v) = rx.try_recv() {
            latest = Some(v);
        }
        latest
    });
    if latest.is_some() {
        live.pending = latest;
    }
    let now = time.elapsed_secs();
    if live.pending.is_none() || now - live.last_upload < 0.5 {
        return;
    }
    let Some((atlases, cubes)) = live.pending.take() else { return };
    live.last_upload = now;
    // Models' light: the grid volume and the viewmodel's lookup (knob
    // `gi.models`, 1; 0 keeps the re-bake's).
    if crate::tune::get("gi.models", 1.0) > 0.5 {
        let cube = |p: usize| cubes.get(p).copied().flatten();
        if let Some(v) = &volume {
            let data = v.data(cube);
            if let Some(mut image) = images.get_mut(&v.image) {
                image.data = Some(data);
            }
            if let Some(l) = lookup.as_deref_mut() {
                l.set_means(v.means(cube));
            }
        }
        if let Some(l) = lookup.as_deref_mut() {
            l.set_cubes(cubes.iter().map(|c| c.unwrap_or([[0.0; 3]; 6])).collect());
        }
    }
    for (h, data) in live.images.clone().iter().zip(atlases) {
        if let (Some(h), Some(data)) = (h, data) {
            if let Some(mut image) = images.get_mut(h) {
                image.data = Some(data.iter().flat_map(|v| v.to_le_bytes()).collect());
            }
        }
    }
}

/// The knobs and the sun, to the worker when they change.
fn steer(
    mut live: ResMut<GiLive>,
    mut last_check: Local<f32>,
    time: Res<Time>,
    suns: Query<(&Name, &DirectionalLight, &GlobalTransform), Without<crate::model_lighting::ViewModelSun>>,
    tod: Option<Res<crate::atmos::climate::TimeOfDay>>,
) {
    let now = time.elapsed_secs();
    if now - *last_check < 0.25 || crate::tune::get("gi.freeze", 0.0) > 0.5 {
        return;
    }
    *last_check = now;
    let mut sun = SunNow { scale: 1.0, ..default() };
    if let Some((_, light, tf)) = suns.iter().find(|(n, _, _)| n.as_str() == "sun") {
        // (The light shines along its forward: the sun is behind it.)
        // (`to_cod` takes a point in metres: a unit vector comes back long.)
        let c = Vec3::from(crate::units::to_cod(tf.back().as_vec3())).normalize_or_zero();
        sun.to_sun = (c.length_squared() > 0.5).then_some([c.x, c.y, c.z]);
        let base = *live.base_lux.get_or_insert(light.illuminance.max(1.0));
        sun.scale = (light.illuminance / base).clamp(0.0, 10.0);
        let c = light.color.to_linear();
        let m = c.red.max(c.green).max(c.blue);
        sun.colour = (m > 1e-4).then(|| [c.red / m, c.green / m, c.blue / m]);
    }
    if crate::atmos::climate::on() {
        sun.night = tod.map_or(0.0, |t| t.night).clamp(0.0, 1.0);
    }
    let w = wanted(sun);
    if live.sent != Some(w) {
        live.sent = Some(w);
        let _ = live.to_worker.send(w);
    }
}

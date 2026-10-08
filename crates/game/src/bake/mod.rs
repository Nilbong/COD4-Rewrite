//! Re-baking a map's lighting: CoD4's lightmaps and light grid lit again,
//! on the player's PC from their own install, with a path tracer (sky,
//! bounce light from the sun and lamps over several bounces, soft
//! shadows), at a multiple of CoD4's lightmap resolution. The result goes
//! to a cache (`cache`) that the game uses instead of CoD4's lighting when
//! the setting asks for it.
//!
//! `COD4RW_BAKE=<map>` bakes and exits. `COD4RW_BAKE_SCALE` (2) sets the
//! resolution multiple, `COD4RW_BAKE_SAMPLES` (512) the final pass's rays
//! per texel, `COD4RW_BAKE_BOUNCES` (3) the bounces, `COD4RW_BAKE_SKY`
//! (`middle`; `fit`, `physical`) the sky's brightness ([`SkyLevel`]).

pub mod cache;
mod denoise;
mod dxt;
pub mod integrate;
mod lamps;
mod scene;
mod sky;
// The sun's path, shared with the live sun (`crate::atmos`): one file, so
// the baked bounce always matches it.
#[path = "../atmos/sun_path.rs"]
mod sun_path;
mod texels;
pub mod tod;

use glam33::{Vec2, Vec3};
use integrate::{AtlasLight, Baker, Directional, Grid, Lights, Split};
use iw3::zone::{ParseOptions, Zone};
use std::time::Instant;

/// `world.rs`'s live sun: `LIVE_SUN_ILLUMINANCE` lux for a live sun of 1,
/// the live sun being `sunColor * (sunLight - ambientScale) * (1 -
/// diffuseFraction)` (the rest of `sunLight` is in the lightmaps).
const LIVE_SUN_ILLUMINANCE: f32 = 12_000.0;

pub struct Options {
    pub scale: f32,
    pub samples: u32,
    pub bounces: u32,
    pub sky: SkyLevel,
    /// The darkest the bake may be against CoD4's light (`COD4RW_BAKE_FLOOR`).
    pub floor: f32,
    pub profile: Profile,
}

/// How far the bake departs from CoD4 (`COD4RW_BAKE_PROFILE`): the default
/// ("bold", the user's pick) trusts the bake (darker shade, stronger contact
/// shadows and bounce colour, full lamps) while keeping players in shade
/// readable; `safe` stays close to CoD4's brightness.
#[derive(Clone, Copy, Debug)]
pub struct Profile {
    /// The middle sky over the geometric mean.
    pub sky_lift: f32,
    pub floor: f32,
    /// The floor where CoD4 was dim (see [`floor_at`]).
    pub dark_floor: f32,
    /// Fitted lamps' brightness against the fit (under 1: the fit also
    /// explains CoD4's own bounce of them, and the bake adds its own).
    pub lamp_scale: f32,
    /// Bounce light's colour saturation.
    pub saturation: f32,
    /// Extra contact occlusion (see `Baker::gather_final`).
    pub contact: f32,
}

impl Profile {
    fn from_env() -> Profile {
        match cache::profile().as_deref() {
            Some("safe") => Profile { sky_lift: 1.2, floor: 0.55, dark_floor: 0.85, lamp_scale: 0.85, saturation: 1.0, contact: 0.0 },
            _ => Profile { sky_lift: 1.0, floor: 0.3, dark_floor: 0.5, lamp_scale: 1.0, saturation: 1.35, contact: 0.35 },
        }
    }
}

/// How bright the sky is: as CoD4 lit its shade (`Fit`), as the skybox is
/// drawn (`Physical`: about four parts sun to one of sky, real daylight's
/// contrast), or between the two (`Middle`, their geometric mean).
#[derive(Clone, Copy, Debug)]
pub enum SkyLevel {
    Fit,
    Middle,
    Physical,
}

/// `player::SKY_BRIGHTNESS`: the skybox's luminance (nits) for white.
const SKY_BRIGHTNESS: f32 = 800.0;

impl SkyLevel {
    /// The sky's scale (over its texture's linear values), given CoD4's fit.
    fn brightness(self, fit: f32, lift: f32) -> f32 {
        // Radiance in lightmap units is nits / LIGHTMAP_EXPOSURE.
        let physical = SKY_BRIGHTNESS / integrate::EXPOSURE;
        match self {
            SkyLevel::Fit => fit,
            SkyLevel::Physical => physical,
            // Lifted so shade (and soldiers standing in it, through the
            // grid) stays readable: gameplay first.
            SkyLevel::Middle => (fit * physical).sqrt() * lift,
        }
    }
}

impl Options {
    pub fn from_env() -> Options {
        let var = |k: &str, d: u32| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
        let sky = match std::env::var("COD4RW_BAKE_SKY").unwrap_or_default().to_ascii_lowercase().as_str() {
            "fit" => SkyLevel::Fit,
            "physical" => SkyLevel::Physical,
            _ => SkyLevel::Middle,
        };
        let profile = Profile::from_env();
        Options { profile, scale: std::env::var("COD4RW_BAKE_SCALE").ok().and_then(|v| v.parse().ok()).unwrap_or(2.0f32).clamp(1.0, 8.0), samples: var("COD4RW_BAKE_SAMPLES", 512).max(16), bounces: var("COD4RW_BAKE_BOUNCES", 3).clamp(1, 8), sky, floor: std::env::var("COD4RW_BAKE_FLOOR").ok().and_then(|v| v.parse().ok()).unwrap_or(profile.floor).clamp(0.0, 1.0) }
    }
}

fn secs(t: Instant) -> f32 {
    t.elapsed().as_secs_f32()
}

pub fn run(map: &str, opts: &Options) -> anyhow::Result<std::path::PathBuf> {
    Ok(run_with(map, opts, None)?.dir)
}

/// What a bake leaves for keyframes baked after it.
pub struct Out {
    pub dir: std::path::PathBuf,
    /// The lamps found in the map's lightmap.
    pub lamps: Vec<integrate::Spot>,
    pub grid_scale: f32,
    /// CoD4's sun on the time-of-day path.
    pub map_sun: sun_path::MapSun,
}

/// Bake every time-of-day keyframe of `map` ([`tod::KEYS`]), after its
/// regular bake (the map's own lighting), whose lamps and grid units they
/// reuse.
pub fn showcase(map: &str, opts: &Options) -> anyhow::Result<()> {
    let first = run_with(map, opts, None)?;
    log::info!("bake: map sun on the path at {:.2}h, turned {:.0} degrees", first.map_sun.hour, first.map_sun.azimuth_offset.to_degrees());
    for key in &tod::KEYS {
        let ovr = tod::Override::new(key, &first.map_sun, &first.lamps, first.grid_scale, 1.0 / (std::f32::consts::PI * integrate::EXPOSURE));
        log::info!("bake: keyframe {} ({}h): light {:?}, lamps {:.2} on", key.name, key.hour, ovr.light, ovr.lamps_on);
        run_with(map, opts, Some(Variant::Key(ovr)))?;
    }
    Ok(())
}

/// A bake other than the map's default one.
pub enum Variant {
    /// A time-of-day keyframe.
    Key(tod::Override),
}

pub fn run_with(map: &str, opts: &Options, variant: Option<Variant>) -> anyhow::Result<Out> {
    let ovr = match &variant {
        Some(Variant::Key(o)) => Some(o),
        None => None,
    };
    let t0 = Instant::now();
    let install = iw3::Install::locate()?;
    let vfs = iw3::iwd::Vfs::mount(&install.iwd_paths()?)?;
    let zone_path = install.zone_path(map);
    let load = |name: &str| -> anyhow::Result<Zone> { Zone::parse(&iw3::fastfile::load(&install.zone_path(name))?, ParseOptions::default()) };
    let zones = vec![load(map)?, load("common_mp")?];
    let world = zones[0].gfx_world().ok_or_else(|| anyhow::anyhow!("{map} has no GfxWorld"))?;
    log::info!("bake {map}: loaded in {:.1}s", secs(t0));

    let t = Instant::now();
    let mut scene = scene::Scene::build(&zones, &vfs);
    scene.saturation = opts.profile.saturation;
    log::info!("bake: profile {:?} {:?}", cache::profile(), opts.profile);
    log::info!("bake: scene and BVH in {:.1}s", secs(t));
    let sky_name = world.sky_image.and_then(|i| zones[0].image(i)).map(|i| i.name.clone()).unwrap_or_default();
    let own_sky;
    let sky = match ovr {
        Some(o) => &o.sky,
        None => {
            own_sky = sky::Sky::load(&vfs, &sky_name).ok_or_else(|| anyhow::anyhow!("no sky cube {sky_name}"))?;
            &own_sky
        }
    };
    let sun = &world.sun;
    let live = Vec3::from(sun.sun_color) * ((sun.sun_light - sun.ambient_scale) * (1.0 - sun.diffuse_fraction)).max(0.0);
    let primary = zones[0].com_world().map_or(&[][..], |c| &c.primary_lights[..]);
    let mut lights = Lights::new(world, primary, LIVE_SUN_ILLUMINANCE * live.max_element());
    let map_sun = sun_path::MapSun::new(lights.to_sun.to_array(), sun.sun_color, LIVE_SUN_ILLUMINANCE * live.max_element());
    if let Some(o) = ovr {
        // The keyframe's sun or moon, and the lamps at its strength.
        (lights.to_sun, lights.sun) = o.light;
        for s in lights.spots.iter_mut() {
            s.colour *= o.lamps_on;
        }
        lights.spots.extend(o.lamps.iter().cloned().map(|mut s| {
            s.colour *= o.lamps_on;
            s
        }));
    }
    let lights = lights;
    log::info!("bake: sun {:?} (lightmap units), {} lamps, sky ground {:?}", lights.sun, lights.spots.len(), sky.ground);
    let baker = Baker { scene: &scene, sky, lights: &lights };

    // The atlases to bake, with CoD4's own (for fitting the sky).
    // The resolution: `opts.scale` times CoD4's, less where that would
    // cover more than [`COVERED_BUDGET`] texels (the cache's size follows
    // the covered texels).
    let covered_at = |scale: f32| -> usize {
        world
            .lightmaps
            .iter()
            .enumerate()
            .filter_map(|(i, p)| Some((i, p.secondary.and_then(|id| zones[0].image(id))?)))
            .map(|(i, img)| {
                let (w, h) = ((img.width as f32 * scale).round() as usize, (img.height as f32 / 2.0 * scale).round() as usize);
                texels::rasterise(world, i as u8, w, h).texels.iter().flatten().count()
            })
            .sum()
    };
    let mut scale = opts.scale;
    let full = covered_at(scale);
    if full > COVERED_BUDGET {
        scale = (scale * (COVERED_BUDGET as f32 / full as f32).sqrt()).max(1.0);
        log::info!("bake: {full} texels at {}x is over budget; baking at {scale:.2}x", opts.scale);
    }
    let mut atlases = Vec::new();
    let mut originals = Vec::new();
    for (i, pair) in world.lightmaps.iter().enumerate() {
        let img = pair.secondary.and_then(|id| zones[0].image(id));
        let def = img.and_then(|img| img.load_def.as_ref().map(|d| (img, d)));
        let Some((img, def)) = def.filter(|(img, d)| d.format == 21 && d.data.len() >= img.width as usize * img.height as usize * 4) else {
            atlases.push(AtlasLight::new(texels::Atlas { w: 0, h: 0, texels: Vec::new(), dropped: Vec::new() }));
            originals.push(None);
            continue;
        };
        let (w, h) = (img.width as usize, img.height as usize / 2);
        atlases.push(AtlasLight::new(texels::rasterise(world, i as u8, (w as f32 * scale).round() as usize, (h as f32 * scale).round() as usize)));
        originals.push(Some((w, h, def.data.clone())));
    }
    let covered: usize = atlases.iter().map(|a| a.atlas.texels.iter().flatten().count()).sum();
    log::info!("bake: {} atlases at {scale:.2}x, {covered} texels", atlases.len());

    let t = Instant::now();
    baker.direct(&mut atlases);
    log::info!("bake: direct light in {:.1}s", secs(t));

    let mut grid = Grid::new(&world.light_grid);
    let grid_samples = (opts.samples / 2).max(128);
    bounces(&baker, &mut atlases, &mut grid, opts, true);

    // The sky's brightness, fitted to CoD4's own lightmaps (a keyframe's
    // sky is in lightmap units already).
    let k = if ovr.is_some() { 1.0 } else { fit_sky(&atlases, &originals) };
    let k_out = if ovr.is_some() { 1.0 } else { opts.sky.brightness(k, opts.profile.sky_lift) };
    log::info!("bake: sky brightness fitted to CoD4's x{k:.3}; baking with x{k_out:.3} ({:?})", opts.sky);

    // CoD4's lamps, found again as lights; then the light again with them.
    // Twice: the second fit counts the first lamps' bounce as the bake's
    // own, so it isn't fitted (and then bounced) a second time.
    let primary_count = lights.spots.len();
    let mut lights = lights;
    for round in 0..if ovr.is_some() { 0 } else { 2 } {
        let t = Instant::now();
        let samples = lamps::samples(&atlases, |ai, i| {
            let a = &atlases[ai];
            let orig = originals[ai].as_ref()?;
            let w = a.atlas.w;
            let uv = Vec2::new((i % w) as f32 + 0.5, (i / w) as f32 + 0.5) / Vec2::new(w as f32, a.atlas.h as f32);
            let (e, l) = original_dir_at(orig, uv);
            let g = a.gathered[i];
            // The bake's light less the fitted lamps' direct light (the
            // primary lights' stays: `lamps` minus the fitted share).
            let direct = if round == 0 { a.lamps[i] } else { a.primary[i] };
            Some(((e - (g.sky * k + g.sun + direct)).max(Vec3::ZERO), l))
        });
        let found = lamps::fit(&zones, &scene, &samples, opts.profile.lamp_scale);
        log::info!("bake: lamps fitted (round {}) in {:.1}s", round + 1, secs(t));
        lights.spots.truncate(primary_count);
        lights.spots.extend(found);
        let baker = Baker { scene: &scene, sky, lights: &lights };
        if round == 0 {
            // The primary lights alone, kept for the second round.
            for a in atlases.iter_mut() {
                a.primary = a.lamps.clone();
            }
        }
        baker.direct(&mut atlases);
        bounces(&baker, &mut atlases, &mut grid, opts, false);
    }
    let baker = Baker { scene: &scene, sky, lights: &lights };

    // (Debug: `COD4RW_BAKE_LOCAL=1` also copies in whatever CoD4 still has
    // beyond the bake, as before the lamps were fitted.)
    let local = std::env::var_os("COD4RW_BAKE_LOCAL").is_some();
    let (texels_local, points_local) = add_local(&mut atlases, &originals, &mut grid, &world.light_grid, k, local);
    if local {
        log::info!("bake: CoD4's own light kept at {texels_local} texels and {points_local} grid points");
    }
    if let Some(o) = ovr {
        grid.scale = o.grid_scale;
    }
    // CoD4's lightmap is one time of day: other keyframes take no floor
    // from it.
    let (floor, dark_floor) = if ovr.is_some() { (0.0, 0.0) } else { (opts.floor, opts.profile.dark_floor) };

    let t = Instant::now();
    let mut out_atlases = Vec::new();
    for (ai, a) in atlases.iter().enumerate() {
        if a.atlas.w == 0 {
            out_atlases.push(None);
            continue;
        }
        let mut light = baker.gather_final(&atlases, &grid, opts.samples, k_out, ai, opts.profile.contact);
        denoise::atrous(&a.atlas, &mut light, &[1, 2, 4]);
        if let Some(orig) = originals[ai].as_ref().filter(|_| ovr.is_none()) {
            let raised = floor_texels(&a.atlas, &mut light, orig, floor, dark_floor);
            log::info!("bake: {raised} texels raised to the visibility floor ({:.0}% of CoD4's)", opts.floor * 100.0);
            log_directionality(&a.atlas, &light, orig);
            if let Some(at) = std::env::var("COD4RW_BAKE_PROBE").ok().and_then(|v| {
                let v: Vec<f32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                (v.len() == 3).then(|| glam33::Vec3A::new(v[0], v[1], v[2]))
            }) {
                probe(&a.atlas, &light, orig, at);
            }
            // Texels dropped as inside walls take CoD4's light (some
            // were only near one-sided props), not their neighbours'.
            let mut atlas = texels::Atlas { w: a.atlas.w, h: a.atlas.h, texels: a.atlas.texels.clone(), dropped: Vec::new() };
            for &(i, t) in &a.atlas.dropped {
                let uv = Vec2::new((i % atlas.w) as f32 + 0.5, (i / atlas.w) as f32 + 0.5) / Vec2::new(atlas.w as f32, atlas.h as f32);
                let (e, l) = original_dir_at(orig, uv);
                light[i] = Directional::default();
                light[i].add(e, l);
                atlas.texels[i] = Some(t);
            }
            out_atlases.push(Some(pack(&atlas, &light)));
            continue;
        }
        out_atlases.push(Some(pack(&a.atlas, &light)));
    }
    log::info!("bake: final gather and denoise in {:.1}s", secs(t));
    // The grid once more, with the sky fitted, for models.
    let cubes = baker.gather_grid(&atlases, &grid, grid_samples, opts.bounces);
    fill_grid(&mut grid, cubes);
    let grid_out: Vec<_> = grid
        .points
        .iter()
        .zip(&grid.cubes)
        .zip(&grid.local)
        .zip(original_cubes(&world.light_grid))
        .map(|(((p, c), l), o)| {
            (*p, std::array::from_fn(|f| {
                let v = (c[f].sky * k_out + c[f].sun + l[f]) * grid.scale;
                // The same floor as the texels', so no one stands unseen.
                // (Grid light is ~1/5 of the lightmap's units: see `add_local`.)
                o.filter(|_| ovr.is_none()).map_or(v, |o| v.max(o[f] * floor_at(sky::luma(o[f]) / grid.scale, floor, dark_floor))).to_array()
            }))
        })
        .collect();
    log::info!("bake: grid scaled x{:.3} to CoD4's units", grid.scale);

    grid_report(&grid_out, &world.light_grid);
    let baked = cache::Baked { atlases: out_atlases, grid: grid_out };
    let total = secs(t0);
    let meta = format!(
        "{{\n  \"map\": \"{map}\",\n  \"version\": {},\n  \"scale\": {scale:.2},\n  \"samples\": {},\n  \"bounces\": {},\n  \"texels\": {covered},\n  \"grid_points\": {},\n  \"sky_fit\": {k:.4},
  \"sky\": \"{:?}\",
  \"sky_used\": {k_out:.4},\n  \"seconds\": {total:.1},\n  \"threads\": {}\n}}\n",
        cache::VERSION,
        opts.samples,
        opts.bounces,
        grid.points.len(),
        opts.sky,
        rayon::current_num_threads()
    );
    let name = match &variant {
        Some(Variant::Key(o)) => Some(o.variant.clone()),
        None => cache::profile(),
    };
    let target = cache::dir_for(map, name.as_deref()).ok_or_else(|| anyhow::anyhow!("no cache folder"))?;
    let dir = cache::save_in(target, cache::source_stamp(&zone_path), &baked, &meta)?;
    // `COD4RW_BAKE_PREVIEW`: PNGs of the atlases beside CoD4's, for looking at.
    if std::env::var_os("COD4RW_BAKE_PREVIEW").is_some() {
        preview(&dir, &baked, &originals);
    }
    log::info!("bake {map}: done in {total:.1}s, written to {}", dir.display());
    Ok(Out { dir, lamps: lights.spots[primary_count..].to_vec(), grid_scale: grid.scale, map_sun })
}

/// Take the grid's new cubes; points that were inside something take
/// their neighbours' mean (or keep the last pass's).
fn fill_grid(grid: &mut Grid, cubes: Vec<Option<[Split; 6]>>) {
    let index: std::collections::HashMap<[u32; 3], usize> = grid.points.iter().enumerate().map(|(i, p)| (*p, i)).collect();
    let old = std::mem::take(&mut grid.cubes);
    grid.cubes = cubes
        .iter()
        .enumerate()
        .map(|(i, c)| {
            c.unwrap_or_else(|| {
                let p = grid.points[i];
                let mut sum = [Split::default(); 6];
                let mut n = 0;
                for (axis, step) in [(0, -1i64), (0, 1), (1, -1), (1, 1), (2, -1), (2, 1)] {
                    let mut q = p.map(|v| v as i64);
                    q[axis] += step;
                    if let Some(c) = index.get(&q.map(|v| v.max(0) as u32)).and_then(|&j| cubes[j]) {
                        for f in 0..6 {
                            sum[f] += c[f];
                        }
                        n += 1;
                    }
                }
                if n == 0 { old[i] } else { sum.map(|s| s * (1.0 / n as f32)) }
            })
        })
        .collect();
}

/// CoD4's lightmap at a texel, as the shader lights a flat surface with it
/// (linear lightmap units).
fn original_at(orig: &(usize, usize, Vec<u8>), uv: Vec2) -> Vec3 {
    original_dir_at(orig, uv).0
}

/// CoD4's lightmap at a texel, bilinearly filtered: its light on a flat
/// surface and its direction (tangent space).
fn original_dir_at(orig: &(usize, usize, Vec<u8>), uv: Vec2) -> (Vec3, Vec3) {
    let (w, h, data) = orig;
    let (w, h) = (*w, *h);
    let fx = (uv.x * w as f32 - 0.5).clamp(0.0, w as f32 - 1.0);
    let fy = (uv.y * h as f32 - 0.5).clamp(0.0, h as f32 - 1.0);
    let (x0, y0) = (fx as usize, fy as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
    let texel = |x: usize, y: usize| {
        let p = &data[(y * w + x) * 4..][..4];
        glam33::Vec4::new(p[2] as f32, p[1] as f32, p[0] as f32, p[3] as f32) / 255.0
    };
    let bilerp = |dy: usize| {
        let a = texel(x0, y0 + dy).lerp(texel(x1, y0 + dy), tx);
        let b = texel(x0, y1 + dy).lerp(texel(x1, y1 + dy), tx);
        a.lerp(b, ty)
    };
    let (a, b) = (bilerp(0), bilerp(h));
    let l = Vec3::new(a.w * 4.08 - 2.08, b.w * 4.064_516 - 2.064_516, 1.0).normalize();
    ((a.truncate() + b.truncate() * l.z).max(Vec3::ZERO).powf(2.2), l)
}

#[allow(dead_code)]
fn original_nearest(orig: &(usize, usize, Vec<u8>), uv: Vec2) -> Vec3 {
    let (w, h, data) = orig;
    let x = ((uv.x * *w as f32) as usize).min(w - 1);
    let y = ((uv.y * *h as f32) as usize).min(h - 1);
    let px = |y: usize| {
        let p = &data[(y * w + x) * 4..][..4];
        (Vec3::new(p[2] as f32, p[1] as f32, p[0] as f32) / 255.0, p[3] as f32 / 255.0)
    };
    let (a, ax) = px(y);
    let (b, by) = px(y + h);
    let l = Vec3::new(ax * 4.08 - 2.08, by * 4.064_516 - 2.064_516, 1.0).normalize();
    (a + b * l.z).powf(2.2)
}

/// How bright the sky must be for the bake to light the map as brightly
/// as CoD4 did: the median, over texels lit mostly by the sky, of what
/// CoD4's lightmap has beyond the bake's sun and lamp light, per unit of
/// the bake's sky light.
fn fit_sky(atlases: &[AtlasLight], originals: &[Option<(usize, usize, Vec<u8>)>]) -> f32 {
    let mut ratios = Vec::new();
    for (a, orig) in atlases.iter().zip(originals) {
        let Some(orig) = orig else { continue };
        let (w, h) = (a.atlas.w, a.atlas.h);
        for (i, t) in a.atlas.texels.iter().enumerate() {
            if t.is_none() {
                continue;
            }
            let uv = Vec2::new((i % w) as f32 + 0.5, (i / w) as f32 + 0.5) / Vec2::new(w as f32, h as f32);
            let g = a.gathered[i];
            let sky = sky::luma(g.sky);
            let rest = sky::luma(g.sun + a.lamps[i]);
            if sky > 1e-4 && sky > rest {
                ratios.push(((sky::luma(original_at(orig, uv)) - rest) / sky).max(0.0));
            }
        }
    }
    if ratios.is_empty() {
        return 1.0;
    }
    ratios.sort_by(f32::total_cmp);
    ratios[ratios.len() / 2].clamp(0.05, 50.0)
}

/// Gathering passes over the texels and the grid, a bounce each. The first
/// time, texels inside walls are dropped.
fn bounces(baker: &Baker, atlases: &mut [AtlasLight], grid: &mut Grid, opts: &Options, first: bool) {
    let pass_samples = (opts.samples / 4).max(64);
    let grid_samples = (opts.samples / 2).max(128);
    for pass in 0..opts.bounces {
        let t = Instant::now();
        let gathered = baker.gather(atlases, grid, pass_samples, pass);
        let mut dropped = 0;
        for (a, (light, inside)) in atlases.iter_mut().zip(gathered) {
            a.gathered = light;
            if first && pass == 0 {
                for (i, inside) in inside.into_iter().enumerate() {
                    if let Some(t) = a.atlas.texels[i].filter(|_| inside) {
                        a.atlas.texels[i] = None;
                        a.atlas.dropped.push((i, t));
                        dropped += 1;
                    }
                }
                a.find_nearest();
            }
        }
        let cubes = baker.gather_grid(atlases, grid, grid_samples, pass);
        fill_grid(grid, cubes);
        log::info!("bake: bounce {} in {:.1}s{}", pass + 1, secs(t), if first && pass == 0 { format!(" ({dropped} texels inside walls dropped)") } else { String::new() });
    }
}

/// How much brighter than the bake CoD4's light must be before the
/// difference counts as a lamp of its own (CoD4's bounce light was
/// stronger and leakier; that isn't kept).
const LOCAL_MARGIN: f32 = 1.5;

/// Keep CoD4's lamps: the light in its lightmaps and grid beyond what the
/// bake's sky, sun and primary lights give (by [`LOCAL_MARGIN`]) becomes
/// fixed light, with CoD4's direction for it.
fn add_local(atlases: &mut [AtlasLight], originals: &[Option<(usize, usize, Vec<u8>)>], grid: &mut Grid, lg: &iw3::zone::LightGrid, k: f32, inject: bool) -> (usize, usize) {
    let mut texels = 0;
    for (a, orig) in atlases.iter_mut().zip(originals).filter(|_| inject) {
        let Some(orig) = orig else { continue };
        let (w, h) = (a.atlas.w, a.atlas.h);
        for i in 0..a.atlas.texels.len() {
            if a.atlas.texels[i].is_none() {
                continue;
            }
            let uv = Vec2::new((i % w) as f32 + 0.5, (i / w) as f32 + 0.5) / Vec2::new(w as f32, h as f32);
            let (e, l) = original_dir_at(orig, uv);
            let g = a.gathered[i];
            let est = g.sky * k + g.sun + a.lamps[i];
            let r = (e - est * LOCAL_MARGIN).max(Vec3::ZERO);
            if sky::luma(r) > 0.01 {
                a.local[i] = (r, l);
                texels += 1;
            }
        }
    }
    let originals = original_cubes(lg);
    // CoD4's grid is in units of its own: the game's model lighting is tuned
    // to them, so the bake's grid is scaled to match CoD4's overall (the
    // median over points lit mostly by the sky).
    let mut ratios: Vec<f32> = originals
        .iter()
        .zip(&grid.cubes)
        .filter_map(|(o, c)| {
            let o = (*o)?;
            let sky: f32 = c.iter().map(|f| sky::luma(f.sky * k)).sum();
            let all: f32 = c.iter().map(|f| sky::luma(f.sky * k + f.sun)).sum();
            let orig: f32 = o.iter().map(|f| sky::luma(*f)).sum();
            (all > 1e-4 && sky > 0.5 * all && orig > 1e-4).then_some(orig / all)
        })
        .collect();
    ratios.sort_by(f32::total_cmp);
    grid.scale = ratios.get(ratios.len() / 2).copied().unwrap_or(1.0).clamp(0.05, 20.0);
    let mut count = 0;
    for (i, o) in originals.iter().enumerate().filter(|_| inject) {
        let Some(o) = o else { continue };
        let mut any = false;
        for f in 0..6 {
            let c = grid.cubes[i][f];
            let r = (o[f] - (c.sky * k + c.sun) * grid.scale * LOCAL_MARGIN).max(Vec3::ZERO);
            if sky::luma(r) > 0.01 {
                // (In the bake's units.)
                grid.local[i][f] = r / grid.scale;
                any = true;
            }
        }
        count += any as usize;
    }
    (texels, count)
}

/// CoD4's light grid as ambient cubes, as `crate::model_lighting` makes
/// them (linear, CoD4's units), in `LightGrid::points` order.
fn original_cubes(lg: &iw3::zone::LightGrid) -> Vec<Option<[Vec3; 6]>> {
    let dirs = iw3::zone::LightGrid::directions();
    let face_dirs: Vec<Vec<usize>> = (0..6)
        .map(|f| {
            let (axis, sign) = (f / 2, if f % 2 == 0 { 1.0 } else { -1.0 });
            (0..dirs.len()).filter(|&d| (dirs[d][axis] - sign).abs() < 1e-3 && (0..3).all(|a| a == axis || dirs[d][a].abs() < 0.5)).collect()
        })
        .collect();
    lg.points()
        .iter()
        .map(|(_, entry)| {
            let colors = lg.colors.get(entry.colors_index as usize)?;
            Some(std::array::from_fn(|f| {
                face_dirs[f].iter().fold(Vec3::ZERO, |s, &d| {
                    let c = colors.0[d];
                    s + Vec3::new(c[0] as f32, c[1] as f32, c[2] as f32).map(|v| (v / 255.0).powf(2.2)) / face_dirs[f].len() as f32
                })
            }))
        })
        .collect()
}

/// The visibility floor for light that CoD4 had at `lum` (lightmap units):
/// `floor` of it in ordinary shade, rising to `dark` where CoD4
/// was already dim (a half-lit dark corner reads as black once the eye has
/// adjusted to the street outside).
fn floor_at(lum: f32, floor: f32, dark: f32) -> f32 {
    let t = ((lum - 0.02) / (0.15 - 0.02)).clamp(0.0, 1.0);
    let t = t * t * (3.0 - 2.0 * t);
    dark + (floor.min(dark) - dark) * t
}

/// Raise texels lit less than the floor ([`floor_at`]) of CoD4's light to it,
/// the extra coming from CoD4's direction: darker shade than CoD4's, but
/// never so dark that a player can't be made out. Returns how many.
fn floor_texels(atlas: &texels::Atlas, light: &mut [Directional], orig: &(usize, usize, Vec<u8>), floor: f32, dark: f32) -> usize {
    let (w, h) = (atlas.w, atlas.h);
    let mut raised = 0;
    for (i, t) in atlas.texels.iter().enumerate() {
        if t.is_none() {
            continue;
        }
        let uv = Vec2::new((i % w) as f32 + 0.5, (i / w) as f32 + 0.5) / Vec2::new(w as f32, h as f32);
        let (e, l) = original_dir_at(orig, uv);
        let target = e * floor_at(sky::luma(e), floor, dark);
        if sky::luma(light[i].e) < sky::luma(target) {
            light[i].add((target - light[i].e).max(Vec3::ZERO), l);
            raised += 1;
        }
    }
    raised
}

/// Debug aid (`COD4RW_BAKE_PROBE=x,y,z`): texels within 80 units of a
/// point, CoD4's light and the bake's.
fn probe(atlas: &texels::Atlas, light: &[Directional], orig: &(usize, usize, Vec<u8>), at: glam33::Vec3A) {
    let (w, h) = (atlas.w, atlas.h);
    let mut shown = 0;
    for (i, t) in atlas.texels.iter().enumerate() {
        let Some(t) = t else { continue };
        if t.pos.distance(at) > 80.0 || i % 7 != 0 {
            continue;
        }
        let uv = Vec2::new((i % w) as f32 + 0.5, (i / w) as f32 + 0.5) / Vec2::new(w as f32, h as f32);
        let (e, l) = original_dir_at(orig, uv);
        let (a, b, bl) = light[i].fit();
        log::info!("probe {:.0?} n {:.2?}: cod4 {:.3} dir {:.2?} | bake e {:.3} A {:.3} B {:.3} L {:.2?}", t.pos, t.n, sky::luma(e), l, sky::luma(light[i].e), sky::luma(a), sky::luma(b), bl);
        shown += 1;
        if shown > 40 {
            break;
        }
    }
    let dropped = atlas.dropped.iter().filter(|(_, t)| t.pos.distance(at) < 80.0).count();
    log::info!("probe: {dropped} dropped texels near");
}

/// Log how the bake's grid compares with CoD4's (mean light per point).
fn grid_report(baked: &[([u32; 3], [[f32; 3]; 6])], lg: &iw3::zone::LightGrid) {
    let mut ratios: Vec<f32> = Vec::new();
    let mut ups: Vec<f32> = Vec::new();
    for ((_, cube), (_, entry)) in baked.iter().zip(lg.points()) {
        let Some(colors) = lg.colors.get(entry.colors_index as usize) else { continue };
        let orig: f32 = colors.0.iter().map(|c| sky::luma(Vec3::new(c[0] as f32, c[1] as f32, c[2] as f32).map(|v| (v / 255.0).powf(2.2)))).sum::<f32>() / 56.0;
        let new: f32 = cube.iter().map(|f| sky::luma(Vec3::from(*f))).sum::<f32>() / 6.0;
        if orig > 1e-3 {
            ratios.push(new / orig);
            ups.push(sky::luma(Vec3::from(cube[4])) / new.max(1e-4));
        }
    }
    ratios.sort_by(f32::total_cmp);
    let q = |f: f32| ratios.get(((ratios.len() as f32 - 1.0) * f) as usize).copied().unwrap_or(0.0);
    log::info!("bake: grid vs CoD4's, bake/CoD4 mean light: 10% {:.2}, median {:.2}, 90% {:.2}", q(0.1), q(0.5), q(0.9));
}

/// How much of the light comes from one direction (`B * L.z / (A + B *
/// L.z)`, mean over texels), the bake's and CoD4's: what gives normal maps
/// their relief.
fn log_directionality(atlas: &texels::Atlas, light: &[Directional], orig: &(usize, usize, Vec<u8>)) {
    let (w, h) = (atlas.w, atlas.h);
    let (mut ours, mut theirs, mut n) = (0.0, 0.0, 0);
    for (i, t) in atlas.texels.iter().enumerate() {
        if t.is_none() {
            continue;
        }
        let (a, b, l) = light[i].fit();
        let (a, b) = (sky::luma(a), sky::luma(b) * l.z);
        if a + b > 1e-4 {
            ours += b / (a + b);
        }
        let (ow, oh, data) = orig;
        let x = (((i % w) as f32 + 0.5) / w as f32 * *ow as f32) as usize;
        let y = (((i / w) as f32 + 0.5) / h as f32 * *oh as f32) as usize;
        let px = |y: usize| {
            let p = &data[(y * ow + x) * 4..][..4];
            (sky::luma(Vec3::new(p[2] as f32, p[1] as f32, p[0] as f32) / 255.0), p[3] as f32 / 255.0)
        };
        let ((oa, ax), (ob, by)) = (px(y), px(y + oh));
        let lz = Vec3::new(ax * 4.08 - 2.08, by * 4.064_516 - 2.064_516, 1.0).normalize().z;
        if oa + ob * lz > 1e-4 {
            theirs += ob * lz / (oa + ob * lz);
        }
        n += 1;
    }
    log::info!("bake: directional share of the light: bake {:.2}, CoD4 {:.2}", ours / n.max(1) as f32, theirs / n.max(1) as f32);
}

/// The fitted light as the game's image: layer A (even light, direction x)
/// above layer B (directional light, direction y), half floats, padding
/// filled.
fn pack(atlas: &texels::Atlas, light: &[Directional]) -> cache::BakedAtlas {
    let (w, h) = (atlas.w, atlas.h);
    let mut fitted: Vec<[f32; 8]> = light
        .iter()
        .map(|d| {
            let (a, b, l) = d.fit();
            [a.x, a.y, a.z, l.x, b.x, b.y, b.z, l.y]
        })
        .collect();
    let mut covered: Vec<bool> = atlas.texels.iter().map(|t| t.is_some()).collect();
    denoise::dilate(w, h, &mut covered, &mut fitted, PADDING, |near| {
        let mut s = [0.0; 8];
        for n in near {
            for k in 0..8 {
                s[k] += n[k] / near.len() as f32;
            }
        }
        s
    });
    let mut data = vec![0u16; w * h * 2 * 4];
    for (i, f) in fitted.iter().enumerate() {
        let (x, y) = (i % w, i / w);
        for k in 0..4 {
            data[(y * w + x) * 4 + k] = half::f16::from_f32(f[k]).to_bits();
            data[((y + h) * w + x) * 4 + k] = half::f16::from_f32(f[4 + k]).to_bits();
        }
    }
    cache::BakedAtlas { w: w as u32, h: h as u32, data }
}

/// PNGs of the flat-surface light, CoD4's and the bake's, side by side at
/// the bake's size, for looking at.
fn preview(dir: &std::path::Path, baked: &cache::Baked, originals: &[Option<(usize, usize, Vec<u8>)>]) {
    for (i, (a, orig)) in baked.atlases.iter().zip(originals).enumerate() {
        let (Some(a), Some(orig)) = (a, orig) else { continue };
        let (w, h) = (a.w as usize, a.h as usize);
        let mut img = image::RgbImage::new(w as u32 * 2, h as u32);
        let tone = |c: Vec3| c.to_array().map(|v| (v.max(0.0).powf(1.0 / 2.2) * 255.0).min(255.0) as u8);
        for y in 0..h {
            for x in 0..w {
                let f = |k: usize, row: usize| half::f16::from_bits(a.data[(row * w + x) * 4 + k]).to_f32();
                let av = Vec3::new(f(0, y), f(1, y), f(2, y));
                let bv = Vec3::new(f(0, y + h), f(1, y + h), f(2, y + h));
                let (lx, ly) = (f(3, y), f(3, y + h));
                let lz = (1.0 - lx * lx - ly * ly).max(0.0).sqrt();
                let c = tone(av + bv * lz);
                img.put_pixel(w as u32 + x as u32, y as u32, image::Rgb(c));
                let uv = Vec2::new((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
                img.put_pixel(x as u32, y as u32, image::Rgb(tone(original_at(orig, uv))));
            }
        }
        let _ = img.save(dir.join(format!("preview{i}.png")));
    }
}

/// Rewrite every version-1 cache (uncompressed half floats, padding spread
/// 8 texels round charts) in the current format, with everything beyond
/// [`PADDING`] texels of a chart zeroed: the charts are found again by
/// rasterising the map's lightmap coordinates, no baking needed. Leaves the
/// old caches; returns (map, old MB, new MB) per map.
pub fn convert_v1() -> Vec<(String, f32, f32)> {
    let mut out = Vec::new();
    let Some(root) = cache::root() else { return out };
    let Ok(install) = iw3::Install::locate() else { return out };
    let Ok(maps) = std::fs::read_dir(&root) else { return out };
    for m in maps.flatten() {
        let old = m.path().join("v1");
        let map = m.file_name().to_string_lossy().into_owned();
        let Some((stamp, mut baked)) = cache::load_v1(&old.join("light.bin")) else { continue };
        let Ok(zone) = iw3::fastfile::load(&install.zone_path(&map)).and_then(|d| Zone::parse(&d, ParseOptions::default())) else { continue };
        let Some(world) = zone.gfx_world() else { continue };
        for (i, a) in baked.atlases.iter_mut().enumerate() {
            let Some(a) = a else { continue };
            let atlas = texels::rasterise(world, i as u8, a.w as usize, a.h as usize);
            if atlas.w != a.w as usize || atlas.h != a.h as usize {
                continue;
            }
            let mut covered: Vec<bool> = atlas.texels.iter().map(|t| t.is_some()).collect();
            let mut keep = vec![0u8; covered.len()];
            denoise::dilate(atlas.w, atlas.h, &mut covered, &mut keep, PADDING, |_| 0);
            let layer = atlas.w * atlas.h;
            for (t, c) in covered.iter().enumerate() {
                if !c {
                    for half in 0..2 {
                        a.data[(half * layer + t) * 4..][..4].fill(0);
                    }
                }
            }
        }
        let meta = std::fs::read_to_string(old.join("bake.json")).unwrap_or_default().replace("\"version\": 1", &format!("\"version\": {}", cache::VERSION));
        let Ok(new) = cache::save(&map, stamp, &baked, &meta) else { continue };
        let size = |p: std::path::PathBuf| std::fs::metadata(p).map_or(0.0, |m| m.len() as f32 / 1e6);
        if cache::load(&map, stamp).is_some() {
            out.push((map, size(old.join("light.bin")), size(new.join("light.bin"))));
        }
    }
    out
}

/// The most covered texels a map is baked at: 12.5-17 bytes of cache each
/// (busier maps compress worse), so at most ~15 MB.
const COVERED_BUDGET: usize = 950_000;

/// Texels of padding kept round each chart (bilinear filtering reads one
/// past its edge).
const PADDING: usize = 3;

/// `COD4RW_BAKE=<map>`: bake from the command line, logging to stderr.
pub fn cli(map: &str) -> std::process::ExitCode {
    cli_with(map, |map, opts| run(map, opts).map(|_| ()))
}

/// [`cli`] running `bake` (a whole bake: [`run`], [`showcase`]).
pub fn cli_with(map: &str, bake: impl Fn(&str, &Options) -> anyhow::Result<()>) -> std::process::ExitCode {
    struct Stderr;
    impl log::Log for Stderr {
        fn enabled(&self, m: &log::Metadata) -> bool {
            m.level() <= log::Level::Info
        }
        fn log(&self, r: &log::Record) {
            if self.enabled(r.metadata()) && r.target().contains("bake") {
                eprintln!("{} {}", r.level(), r.args());
            }
        }
        fn flush(&self) {}
    }
    let _ = log::set_logger(&Stderr).map(|()| log::set_max_level(log::LevelFilter::Info));
    match bake(map, &Options::from_env()) {
        Ok(_) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("bake {map} failed: {e:#}");
            std::process::ExitCode::FAILURE
        }
    }
}

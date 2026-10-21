//! LiveGI: the map's sky and sun bounce lit live, from geometry alone (no
//! bake on disk). At map load, rays are traced once from every lightmap
//! texel and from small patches of surface ("clusters", about 24 units
//! across, one per normal direction): each ray either reaches the sky or
//! lands on a cluster, and only that is kept. Relighting is then cheap: the
//! sun's visibility from each cluster (one ray each, re-traced when the sun
//! moves), the light bounced cluster to cluster a few times, and each
//! texel's sky and bounce gathered from its rays and fitted to IW3's
//! lightmap model (`integrate::Directional::fit`), in the same layout as a
//! re-bake (`cache::BakedAtlas`). The sun's direct light isn't in it: the
//! game draws the whole sun live with its shadow maps (`crate::world`,
//! `lightmaps::full_sun`); its bounce is.
//!
//! Units are the lightmaps' (irradiance, see `integrate::EXPOSURE`), as the
//! bake's. The rays are fixed (seeded per texel and cluster), so the result
//! doesn't shimmer; what noise there is stays put.

use super::cache::BakedAtlas;
use super::integrate::{OFFSET, basis, cosine_dir, hash, rand2, sample2, Directional, EXPOSURE};
use super::scene::Scene;
use super::sky::{luma, Sky};
use super::texels::{self, Atlas};
use glam33::{Vec3, Vec3A};
use rayon::prelude::*;
use std::collections::HashMap;

/// Rays per texel, per cluster and per light grid point (all round).
const TEXEL_RAYS: u32 = 64;
const CLUSTER_RAYS: u32 = 32;
const GRID_RAYS: u32 = 48;
/// A cluster's size (units).
const CELL: f32 = 24.0;
/// A ray that reached the sky, and one that hit the back of something (or
/// a cluster never traced): no light.
const SKY: u32 = u32::MAX;
const DARK: u32 = u32::MAX - 1;

#[derive(Clone, Copy, Default)]
struct Cluster {
    pos: Vec3A,
    n: Vec3A,
    albedo: Vec3,
    hits: u32,
    /// Its rays traced.
    traced: bool,
    /// Its position, normal and albedo averaged (later hits don't add).
    fin: bool,
}

/// A switched-on lamp (`crate::fixture_lights`' lamps): its light lands
/// on the clusters (to be bounced on); its direct light is the game's.
#[derive(Clone, Copy)]
pub struct Lamp {
    pub pos: Vec3A,
    /// Linear colour.
    pub colour: Vec3,
    /// Shines down only (a shaded lamp), else all round (a tube).
    pub down: bool,
}

/// The switched-on lamps among `world`'s static models, as
/// `crate::fixture_lights` finds them.
pub fn lamps(world: &iw3::zone::GfxWorld, zones: &[iw3::zone::Zone]) -> Vec<Lamp> {
    let mut out = Vec::new();
    for sm in &world.static_models {
        let Some(id) = sm.model else { continue };
        let (zi, id) = super::scene::resolve_xmodel(zones, 0, id);
        let Some(xm) = zones[zi].xmodel(id) else { continue };
        let name = xm.name.to_ascii_lowercase();
        let lamp = name.contains("_on") && ["light", "lamp", "bulb", "fluo", "lantern"].iter().any(|k| name.contains(k)) && !name.contains("lightning") && !name.contains("lighthouse");
        if !lamp {
            continue;
        }
        let axis = sm.axis.map(Vec3A::from);
        let local = Vec3A::new((xm.mins[0] + xm.maxs[0]) * 0.5, (xm.mins[1] + xm.maxs[1]) * 0.5, xm.mins[2] - 6.0);
        let pos = Vec3A::from(sm.origin) + (axis[0] * local.x + axis[1] * local.y + axis[2] * local.z) * sm.scale;
        let fluo = name.contains("fluo");
        out.push(Lamp { pos, colour: if fluo { Vec3::new(0.85, 0.95, 1.0) } else { Vec3::new(1.0, 0.78, 0.52) }, down: !fluo });
    }
    out
}

/// What's traced once per map.
pub struct Transport {
    pub scene: Scene,
    sky: Sky,
    atlases: Vec<Atlas>,
    /// Per atlas, per texel: the start of its rays in `texel_rays`
    /// (`u32::MAX`: no texel there, or one inside a wall).
    texel_start: Vec<Vec<u32>>,
    texel_rays: Vec<u32>,
    clusters: Vec<Cluster>,
    cluster_rays: Vec<u32>,
    /// The sky's light reaching each cluster (sky radiance scale 1).
    cluster_sky: Vec<Vec3>,
    /// The light grid's points (CoD4's, `LightGrid::points` order: models'
    /// light) and their rays; `false` for points inside walls.
    grid_points: Vec<Vec3A>,
    grid_rays: Vec<u32>,
    grid_ok: Vec<bool>,
    pub lamps: Vec<Lamp>,
}

/// What the light is.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Light {
    /// Towards the sun, CoD axes.
    pub to_sun: Vec3A,
    /// The sun's irradiance on a surface facing it (lightmap units).
    pub sun: Vec3,
    /// The sky's radiance scale over its texture.
    pub sky: f32,
    /// The sky's tint (bluer by night).
    pub sky_tint: Vec3,
    /// An even night sky's light over the sky texture's (a night map's own
    /// sky is near black: Wet Work's mean is 0.002), on every ray that
    /// reaches the sky.
    pub night_sky: Vec3,
    /// Surfaces' albedo scale (bounce strength), capped below 1.
    pub albedo: f32,
    /// Bounces between clusters.
    pub bounces: u32,
    /// The light grid's units against the lightmaps' (CoD4's grid is about
    /// a fifth, and models are tuned to it).
    pub grid_scale: f32,
    /// An even fill under everything, as a share of the sky's mean light on
    /// a surface (gameplay readability in rooms the sun never reaches).
    pub floor: f32,
    /// The lamps' power (lumens, as the game's lamp lights) and reach
    /// (units).
    pub lamp_lumens: f32,
    pub lamp_range: f32,
}

impl Light {
    /// The map's own sun, whole (its "diffuse" share included: the game
    /// draws it all live).
    pub fn of_map(world: &iw3::zone::GfxWorld) -> Light {
        let sun = &world.sun;
        let (pitch, yaw) = (sun.angles[0].to_radians(), sun.angles[1].to_radians());
        let to_sun = Vec3A::new(pitch.cos() * yaw.cos(), pitch.cos() * yaw.sin(), -pitch.sin()).normalize();
        // `crate::world`'s live sun: 12000 lux for 1.
        let lux = 12_000.0 * (Vec3::from(sun.sun_color) * (sun.sun_light - sun.ambient_scale).max(0.0)).max_element();
        let colour = Vec3::from(sun.sun_color) / Vec3::from(sun.sun_color).max_element().max(1e-3);
        Light { to_sun, sun: colour * lux / (std::f32::consts::PI * EXPOSURE), sky: 0.5, albedo: 1.0, bounces: 4, grid_scale: 0.2, floor: 0.0, sky_tint: Vec3::ONE, night_sky: Vec3::ZERO, lamp_lumens: 150_000.0, lamp_range: 5.0 / 0.0254 }
    }
}

fn cluster_key(p: Vec3A, n: Vec3A) -> (i32, i32, i32, u8) {
    let a = n.abs();
    let face = if a.x >= a.y && a.x >= a.z {
        (n.x < 0.0) as u8
    } else if a.y >= a.z {
        2 + (n.y < 0.0) as u8
    } else {
        4 + (n.z < 0.0) as u8
    };
    ((p.x / CELL).floor() as i32, (p.y / CELL).floor() as i32, (p.z / CELL).floor() as i32, face)
}

/// What a ray found: the sky, the back of something, or a surface point.
enum Found {
    Sky,
    Dark,
    Surface(Vec3A, Vec3A, Vec3),
}

fn trace(scene: &Scene, o: Vec3A, d: Vec3A) -> Found {
    let Some(h) = scene.closest(o, d, 100_000.0) else { return Found::Sky };
    if !h.front {
        return Found::Dark;
    }
    let nf = scene.info[h.tri].nf;
    Found::Surface(scene.point(&h), nf, scene.albedo(&h))
}

/// A texel's ray `j` (world space), cosine-distributed about its normal.
fn texel_dir(t: &texels::Texel, i: u32, j: u32) -> Vec3A {
    let (t1, t2) = basis(t.n);
    let s = sample2(j, TEXEL_RAYS, rand2(hash(i) ^ 0x6c1e_57a1));
    let l = cosine_dir(s.x, s.y);
    (t1 * l.x + t2 * l.y + t.n * l.z).normalize()
}

/// A grid point's ray `j`, uniform over the sphere.
fn grid_dir(i: u32, j: u32) -> Vec3A {
    let s = sample2(j, GRID_RAYS, rand2(hash(i) ^ 0x51a7_e3c9));
    let z = 1.0 - 2.0 * s.x;
    let r = (1.0 - z * z).max(0.0).sqrt();
    let phi = std::f32::consts::TAU * s.y;
    Vec3A::new(r * phi.cos(), r * phi.sin(), z)
}

fn cluster_dir(c: &Cluster, i: u32, j: u32) -> Vec3A {
    let (t1, t2) = basis(c.n);
    let s = sample2(j, CLUSTER_RAYS, rand2(hash(i) ^ 0x2f9a_0c13));
    let l = cosine_dir(s.x, s.y);
    (t1 * l.x + t2 * l.y + c.n * l.z).normalize()
}

/// Whether to wait (`target/bake.pause`, other work needs the machine).
pub fn wait_if_paused() {
    while std::path::Path::new("target/bake.pause").exists() {
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

impl Transport {
    /// The world's lightmap texels, as CoD4 laid them out, at its resolution.
    pub fn atlases(world: &iw3::zone::GfxWorld, zone: &iw3::zone::Zone) -> Vec<Atlas> {
        (0..world.lightmaps.len())
            .map(|i| {
                let img = world.lightmaps[i].secondary.and_then(|id| zone.image(id));
                match img {
                    Some(img) => texels::rasterise(world, i as u8, img.width as usize, img.height as usize / 2),
                    None => Atlas { w: 0, h: 0, texels: Vec::new(), dropped: Vec::new(), other_side: Vec::new() },
                }
            })
            .collect()
    }

    /// Traces the map: rays from `atlases`' texels and from the clusters
    /// they land on.
    pub fn build(atlases: Vec<Atlas>, grid_points: Vec<Vec3A>, scene: Scene, sky: Sky) -> Transport {
        let t0 = std::time::Instant::now();
        let covered: usize = atlases.iter().map(|a| a.texels.iter().flatten().count()).sum();
        log::info!("gi: {} atlases, {covered} texels", atlases.len());

        // Texel rays, in parallel; their hits become clusters after.
        let mut found: Vec<Vec<(u32, Vec<(u8, (i32, i32, i32, u8), Vec3A, Vec3A, Vec3)>)>> = Vec::new();
        for a in &atlases {
            wait_if_paused();
            let per: Vec<_> = (0..a.texels.len())
                .into_par_iter()
                .filter_map(|i| {
                    let t = a.texels[i]?;
                    let o = t.pos + t.nf * OFFSET;
                    let mut rays = Vec::with_capacity(TEXEL_RAYS as usize);
                    let mut back_near = 0;
                    for j in 0..TEXEL_RAYS {
                        let d = texel_dir(&t, i as u32, j);
                        if d.dot(t.nf) <= 0.0 {
                            rays.push((1u8, (0, 0, 0, 0), Vec3A::ZERO, Vec3A::ZERO, Vec3::ZERO));
                            continue;
                        }
                        match trace(&scene, o, d) {
                            Found::Sky => rays.push((0u8, (0, 0, 0, 0), Vec3A::ZERO, Vec3A::ZERO, Vec3::ZERO)),
                            Found::Dark => {
                                if let Some(h) = scene.closest(o, d, 8.0) {
                                    if !h.front {
                                        back_near += 1;
                                    }
                                }
                                rays.push((1u8, (0, 0, 0, 0), Vec3A::ZERO, Vec3A::ZERO, Vec3::ZERO));
                            }
                            Found::Surface(p, n, alb) => rays.push((2u8, cluster_key(p, n), p, n, alb)),
                        }
                    }
                    // Inside a wall: most rays see a back face right there.
                    (back_near < TEXEL_RAYS / 2).then_some((i as u32, rays))
                })
                .collect();
            found.push(per);
        }
        // Grid point rays (they land on clusters too).
        wait_if_paused();
        let grid_found: Vec<Option<Vec<(u8, (i32, i32, i32, u8), Vec3A, Vec3A, Vec3)>>> = (0..grid_points.len())
            .into_par_iter()
            .map(|gi| {
                let o = grid_points[gi];
                let mut back_near = 0;
                let rays: Vec<_> = (0..GRID_RAYS)
                    .map(|j| {
                        let d = grid_dir(gi as u32, j);
                        match scene.closest(o, d, 100_000.0) {
                            None => (0u8, (0, 0, 0, 0), Vec3A::ZERO, Vec3A::ZERO, Vec3::ZERO),
                            Some(h) if !h.front => {
                                if h.t < 32.0 {
                                    back_near += 1;
                                }
                                (1u8, (0, 0, 0, 0), Vec3A::ZERO, Vec3A::ZERO, Vec3::ZERO)
                            }
                            Some(h) => {
                                let (p, n) = (scene.point(&h), scene.info[h.tri].nf);
                                (2u8, cluster_key(p, n), p, n, scene.albedo(&h))
                            }
                        }
                    })
                    .collect();
                (back_near * 4 < GRID_RAYS).then_some(rays)
            })
            .collect();
        let mut index: HashMap<(i32, i32, i32, u8), u32> = HashMap::new();
        let mut clusters: Vec<Cluster> = Vec::new();
        let mut add_hit = |key, p: Vec3A, n: Vec3A, alb: Vec3, clusters: &mut Vec<Cluster>| -> u32 {
            let id = *index.entry(key).or_insert_with(|| {
                clusters.push(Cluster::default());
                clusters.len() as u32 - 1
            });
            let c = &mut clusters[id as usize];
            if !c.fin {
                c.pos += p;
                c.n += n;
                c.albedo += alb;
                c.hits += 1;
            }
            id
        };
        let mut texel_start: Vec<Vec<u32>> = atlases.iter().map(|a| vec![u32::MAX; a.texels.len()]).collect();
        let mut texel_rays = Vec::with_capacity(covered * TEXEL_RAYS as usize);
        for (ai, per) in found.into_iter().enumerate() {
            for (i, rays) in per {
                texel_start[ai][i as usize] = texel_rays.len() as u32;
                for (kind, key, p, n, alb) in rays {
                    texel_rays.push(match kind {
                        0 => SKY,
                        1 => DARK,
                        _ => add_hit(key, p, n, alb, &mut clusters),
                    });
                }
            }
        }
        let mut grid_rays = Vec::with_capacity(grid_points.len() * GRID_RAYS as usize);
        let mut grid_ok = Vec::with_capacity(grid_points.len());
        for rays in grid_found {
            grid_ok.push(rays.is_some());
            for (kind, key, p, n, alb) in rays.unwrap_or_else(|| vec![(1u8, (0, 0, 0, 0), Vec3A::ZERO, Vec3A::ZERO, Vec3::ZERO); GRID_RAYS as usize]) {
                grid_rays.push(match kind {
                    0 => SKY,
                    1 => DARK,
                    _ => add_hit(key, p, n, alb, &mut clusters),
                });
            }
        }
        let finish = |c: &mut Cluster| {
            if c.hits > 0 && !c.fin {
                let k = 1.0 / c.hits as f32;
                c.pos *= k;
                c.n = c.n.normalize_or(Vec3A::Z);
                c.albedo *= k;
                c.fin = true;
            }
        };
        clusters.iter_mut().for_each(finish);
        log::info!("gi: texel rays in {:.1}s, {} clusters", t0.elapsed().as_secs_f32(), clusters.len());

        // Cluster rays: rounds, as hits make new clusters (three at most;
        // later ones stay untraced and bounce nothing on).
        let mut cluster_rays = vec![DARK; 0];
        let mut cluster_start: Vec<u32> = Vec::new();
        let mut done = 0usize;
        for _round in 0..3 {
            wait_if_paused();
            let todo: Vec<usize> = (done..clusters.len()).collect();
            if todo.is_empty() {
                break;
            }
            let snapshot = clusters.clone();
            let per: Vec<Vec<(u8, (i32, i32, i32, u8), Vec3A, Vec3A, Vec3)>> = todo
                .par_iter()
                .map(|&ci| {
                    let c = &snapshot[ci];
                    let o = c.pos + c.n * (OFFSET + 1.0);
                    (0..CLUSTER_RAYS)
                        .map(|j| match trace(&scene, o, cluster_dir(c, ci as u32, j)) {
                            Found::Sky => (0u8, (0, 0, 0, 0), Vec3A::ZERO, Vec3A::ZERO, Vec3::ZERO),
                            Found::Dark => (1u8, (0, 0, 0, 0), Vec3A::ZERO, Vec3A::ZERO, Vec3::ZERO),
                            Found::Surface(p, n, alb) => (2u8, cluster_key(p, n), p, n, alb),
                        })
                        .collect()
                })
                .collect();
            let before = clusters.len();
            for (k, rays) in per.into_iter().enumerate() {
                let ci = todo[k];
                if cluster_start.len() <= ci {
                    cluster_start.resize(ci + 1, u32::MAX);
                }
                cluster_start[ci] = cluster_rays.len() as u32;
                clusters[ci].traced = true;
                for (kind, key, p, n, alb) in rays {
                    cluster_rays.push(match kind {
                        0 => SKY,
                        1 => DARK,
                        _ => add_hit(key, p, n, alb, &mut clusters),
                    });
                }
            }
            for c in clusters[before..].iter_mut() {
                finish(c);
            }
            done = todo.last().map_or(done, |l| l + 1);
        }
        // Clusters left untraced: no rays (nothing bounces on from them).
        cluster_start.resize(clusters.len(), u32::MAX);
        // Laid out per cluster in order (`cluster_rays` is per traced
        // cluster, in `cluster_start` order).
        let mut laid = vec![DARK; clusters.len() * CLUSTER_RAYS as usize];
        for (ci, &s) in cluster_start.iter().enumerate() {
            if s != u32::MAX {
                laid[ci * CLUSTER_RAYS as usize..][..CLUSTER_RAYS as usize].copy_from_slice(&cluster_rays[s as usize..][..CLUSTER_RAYS as usize]);
            }
        }
        let cluster_rays = laid;
        let mut tr = Transport { scene, sky, atlases, texel_start, texel_rays, clusters, cluster_rays, cluster_sky: Vec::new(), grid_points, grid_rays, grid_ok, lamps: Vec::new() };
        tr.cluster_sky = (0..tr.clusters.len())
            .into_par_iter()
            .map(|ci| {
                let c = &tr.clusters[ci];
                let rays = &tr.cluster_rays[ci * CLUSTER_RAYS as usize..][..CLUSTER_RAYS as usize];
                rays.iter().enumerate().filter(|(_, r)| **r == SKY).map(|(j, _)| tr.sky.radiance(cluster_dir(c, ci as u32, j as u32))).sum::<Vec3>() / CLUSTER_RAYS as f32
            })
            .collect();
        log::info!(
            "gi: traced in {:.1}s: {} clusters ({} traced), {:.0} MB of rays",
            t0.elapsed().as_secs_f32(),
            tr.clusters.len(),
            tr.clusters.iter().filter(|c| c.traced).count(),
            (tr.texel_rays.len() + tr.cluster_rays.len()) as f32 * 4.0 / 1e6
        );
        tr
    }

    /// Lights the map: the atlases (the re-bake's layout, CoD4's size) and
    /// the clusters' light (for the light grid later).
    pub fn solve(&self, light: &Light) -> (Vec<Option<BakedAtlas>>, Vec<Option<[[f32; 3]; 6]>>) {
        let t0 = std::time::Instant::now();
        let albedo_k = light.albedo;
        let sky_k = light.sky_tint * light.sky;
        let fill = self.sky.ground * sky_k * light.floor;
        // Each cluster's own light: the sun if it sees it, the sky.
        let direct: Vec<Vec3> = (0..self.clusters.len())
            .into_par_iter()
            .map(|ci| {
                let c = &self.clusters[ci];
                let cos = c.n.dot(light.to_sun);
                let o = c.pos + c.n * (OFFSET + 1.0);
                let sun = if cos > 0.0 && !self.scene.occluded(o, light.to_sun, 100_000.0) { light.sun * cos } else { Vec3::ZERO };
                // The lamps (Bevy's point light: lumens / 4 pi candela, its
                // smooth range window).
                let mut lamp = Vec3::ZERO;
                for l in &self.lamps {
                    let to = l.pos - o;
                    let dist = to.length();
                    if dist >= light.lamp_range || dist < 1.0 {
                        continue;
                    }
                    let dir = to / dist;
                    let cos = c.n.dot(dir);
                    // (A shaded lamp: the cone below it, Bevy's 1.25 rad.)
                    if cos <= 0.0 || (l.down && -dir.z < 0.315) || self.scene.occluded(o, dir, dist - 2.0) {
                        continue;
                    }
                    let m = dist * 0.0254;
                    let r = dist / light.lamp_range;
                    let window = (1.0 - r * r * r * r).clamp(0.0, 1.0).powi(2);
                    lamp += l.colour * (light.lamp_lumens / (4.0 * std::f32::consts::PI)) * window * cos / (m * m).max(0.01);
                }
                let open = self.cluster_rays[ci * CLUSTER_RAYS as usize..][..CLUSTER_RAYS as usize].iter().filter(|&&r| r == SKY).count() as f32 / CLUSTER_RAYS as f32;
                sun + self.cluster_sky[ci] * sky_k + light.night_sky * open + lamp / (std::f32::consts::PI * EXPOSURE)
            })
            .collect();
        // Light leaving each cluster (albedo times what reaches it), bounced.
        let reflect = |ci: usize, e: Vec3| (self.clusters[ci].albedo * albedo_k).min(Vec3::splat(0.95)) * e;
        let mut out: Vec<Vec3> = (0..self.clusters.len()).map(|ci| reflect(ci, direct[ci])).collect();
        for _ in 0..light.bounces {
            out = (0..self.clusters.len())
                .into_par_iter()
                .map(|ci| {
                    let rays = &self.cluster_rays[ci * CLUSTER_RAYS as usize..][..CLUSTER_RAYS as usize];
                    let bounce = rays.iter().filter(|&&r| r < DARK).map(|&r| out[r as usize]).sum::<Vec3>() / CLUSTER_RAYS as f32;
                    reflect(ci, direct[ci] + bounce)
                })
                .collect();
        }
        // Each texel: the sky through its rays, the clusters' light.
        let atlases = self
            .atlases
            .iter()
            .enumerate()
            .map(|(ai, a)| {
                if a.w == 0 {
                    return None;
                }
                let light_at: Vec<Directional> = (0..a.texels.len())
                    .into_par_iter()
                    .map(|i| {
                        let mut d = Directional::default();
                        let (Some(t), s) = (a.texels[i], self.texel_start[ai][i]) else { return d };
                        if s == u32::MAX {
                            return d;
                        }
                        d.add(fill, Vec3::Z);
                        let to_tangent = |v: Vec3A| Vec3::new(v.dot(t.t), v.dot(t.b), v.dot(t.n));
                        for (j, &r) in self.texel_rays[s as usize..][..TEXEL_RAYS as usize].iter().enumerate() {
                            let l = match r {
                                SKY => {
                                    let dir = texel_dir(&t, i as u32, j as u32);
                                    d.add((self.sky.radiance(dir) * sky_k + light.night_sky) / TEXEL_RAYS as f32, to_tangent(dir));
                                    continue;
                                }
                                DARK => continue,
                                c => out[c as usize],
                            };
                            let dir = texel_dir(&t, i as u32, j as u32);
                            d.add(l / TEXEL_RAYS as f32, to_tangent(dir));
                        }
                        d
                    })
                    .collect();
                // Texels inside walls and uncovered ones are filled from
                // their neighbours (`pack`'s dilation).
                let mut atlas = Atlas { w: a.w, h: a.h, texels: a.texels.clone(), dropped: Vec::new(), other_side: Vec::new() };
                for (i, s) in self.texel_start[ai].iter().enumerate() {
                    if *s == u32::MAX {
                        atlas.texels[i] = None;
                    }
                }
                Some(super::pack(&atlas, &light_at))
            })
            .collect();
        // The light grid: each point's six faces (irradiance from all round,
        // `value` being pi times radiance as the clusters' light is).
        const FACES: [Vec3A; 6] = [Vec3A::X, Vec3A::NEG_X, Vec3A::Y, Vec3A::NEG_Y, Vec3A::Z, Vec3A::NEG_Z];
        let cubes: Vec<Option<[[f32; 3]; 6]>> = (0..self.grid_points.len())
            .into_par_iter()
            .map(|gi| {
                if !self.grid_ok[gi] {
                    return None;
                }
                let mut cube = [Vec3::ZERO; 6];
                for (j, &r) in self.grid_rays[gi * GRID_RAYS as usize..][..GRID_RAYS as usize].iter().enumerate() {
                    let d = grid_dir(gi as u32, j as u32);
                    let v = match r {
                        SKY => self.sky.radiance(d) * sky_k + light.night_sky,
                        DARK => continue,
                        c => out[c as usize],
                    };
                    for (f, axis) in FACES.iter().enumerate() {
                        let c = d.dot(*axis);
                        if c > 0.0 {
                            cube[f] += v * c;
                        }
                    }
                }
                Some(cube.map(|c| ((c * (4.0 / GRID_RAYS as f32) + fill) * light.grid_scale).to_array()))
            })
            .collect();
        log::info!("gi: solved in {:.0} ms (sky x{}, albedo x{}, {} bounces)", t0.elapsed().as_secs_f64() * 1000.0, light.sky, light.albedo, light.bounces);
        (atlases, cubes)
    }

    /// Mean light reaching the clusters, for logs.
    pub fn stats(&self) -> String {
        let n = self.clusters.len().max(1) as f32;
        let sky: f32 = self.cluster_sky.iter().map(|c| luma(*c)).sum::<f32>() / n;
        format!("{} clusters, mean sky {sky:.3}", self.clusters.len())
    }
}

/// Resamples an atlas (A over B halves) to another size: the live images
/// keep the size of the bake they started from.
pub fn resample(a: &BakedAtlas, w: u32, h: u32) -> Vec<u16> {
    if a.w == w && a.h == h {
        return a.data.clone();
    }
    let (sw, sh) = (a.w as usize, a.h as usize);
    let (w, h) = (w as usize, h as usize);
    let mut out = vec![0u16; w * h * 2 * 4];
    let px = |x: usize, y: usize, k: usize| half::f16::from_bits(a.data[(y * sw + x) * 4 + k]).to_f32();
    for half_ in 0..2 {
        for y in 0..h {
            let fy = ((y as f32 + 0.5) / h as f32 * sh as f32 - 0.5).clamp(0.0, sh as f32 - 1.0);
            let (y0, ty) = (fy as usize, fy.fract());
            let y1 = (y0 + 1).min(sh - 1);
            for x in 0..w {
                let fx = ((x as f32 + 0.5) / w as f32 * sw as f32 - 0.5).clamp(0.0, sw as f32 - 1.0);
                let (x0, tx) = (fx as usize, fx.fract());
                let x1 = (x0 + 1).min(sw - 1);
                for k in 0..4 {
                    let (o0, o1) = (y0 + half_ * sh, y1 + half_ * sh);
                    let v = (px(x0, o0, k) * (1.0 - tx) + px(x1, o0, k) * tx) * (1.0 - ty) + (px(x0, o1, k) * (1.0 - tx) + px(x1, o1, k) * tx) * ty;
                    out[((y + half_ * h) * w + x) * 4 + k] = half::f16::from_f32(v).to_bits();
                }
            }
        }
    }
    out
}

/// Loads `map`, traces it and lights it with its own sun (a run of the
/// whole thing, for `rebake --gi-proto` and the game's worker): the
/// transport, the first solve and the light grid of the map's re-bake if
/// it has one (models' light, until LiveGI lights them too).
pub fn load_and_trace(map: &str) -> anyhow::Result<(Transport, Light)> {
    use iw3::zone::{ParseOptions, Zone};
    let t0 = std::time::Instant::now();
    let install = iw3::Install::locate()?;
    let vfs = iw3::iwd::Vfs::mount(&install.iwd_paths()?)?;
    let load = |name: &str| -> anyhow::Result<Zone> { Zone::parse(&iw3::fastfile::load(&install.zone_path(name))?, ParseOptions::default()) };
    let zones = vec![load(map)?, load("common_mp")?];
    let world = zones[0].gfx_world().ok_or_else(|| anyhow::anyhow!("{map} has no GfxWorld"))?;
    wait_if_paused();
    let layout = super::layout::Layout::load(map);
    let scene = Scene::build(&zones, &vfs, &layout);
    let sky_name = world.sky_image.and_then(|i| zones[0].image(i)).map(|i| i.name.clone()).unwrap_or_default();
    let sky = Sky::load(&vfs, &sky_name).ok_or_else(|| anyhow::anyhow!("no sky cube {sky_name}"))?;
    log::info!("gi {map}: loaded in {:.1}s", t0.elapsed().as_secs_f32());
    let atlases = Transport::atlases(world, &zones[0]);
    let grid_points: Vec<Vec3A> = super::integrate::Grid::new(&world.light_grid).points.iter().map(|p| super::integrate::Grid::pos(*p)).collect();
    let light = Light::of_map(world);
    let lamp_list = lamps(world, &zones);
    // (The zones go before the rays are traced: they're most of the RAM.)
    drop(zones);
    let mut tr = Transport::build(atlases, grid_points, scene, sky);
    log::info!("gi: {} lamps bounce", lamp_list.len());
    tr.lamps = lamp_list;
    Ok((tr, light))
}

/// `rebake --gi-proto <map>`: a LiveGI solve saved as the cache variant
/// `gi-proto` (`COD4RW_BAKE_PROFILE=gi-proto` shows it; the game's own
/// Live mode solves at load instead).
pub fn proto(map: &str) -> anyhow::Result<std::path::PathBuf> {
    let (tr, mut light) = load_and_trace(map)?;
    if let Some(k) = std::env::var("COD4RW_GI_SKY").ok().and_then(|v| v.parse().ok()) {
        light.sky = k;
    }
    let (atlases, _cubes) = tr.solve(&light);
    let install = iw3::Install::locate()?;
    let stamp = super::cache::source_stamp(&install.zone_path(map));
    // The re-bake's light grid, for models (`load_in` checks its stamp).
    let grid = super::cache::dir(map).and_then(|d| super::cache::load_in(&d, stamp)).map(|b| b.grid).unwrap_or_default();
    let dir = super::cache::dir_for(map, Some("gi-proto")).ok_or_else(|| anyhow::anyhow!("no cache folder"))?;
    let meta = format!("{{\n  \"map\": \"{map}\",\n  \"gi\": \"{}\"\n}}\n", tr.stats());
    // Previews: each atlas's flat-surface light (A + B L.z), gamma.
    for (i, a) in atlases.iter().enumerate() {
        let Some(a) = a else { continue };
        let (w, h) = (a.w as usize, a.h as usize);
        let mut img = image::RgbImage::new(w as u32, h as u32);
        for y in 0..h {
            for x in 0..w {
                let f = |k: usize, row: usize| half::f16::from_bits(a.data[(row * w + x) * 4 + k]).to_f32();
                let lz = (1.0 - f(3, y).powi(2) - f(3, y + h).powi(2)).max(0.0).sqrt();
                let c = [0, 1, 2].map(|k| ((f(k, y) + f(k, y + h) * lz).max(0.0).powf(1.0 / 2.2) * 255.0).min(255.0) as u8);
                img.put_pixel(x as u32, y as u32, image::Rgb(c));
            }
        }
        let _ = std::fs::create_dir_all(&dir);
        let _ = img.save(dir.join(format!("gi_preview{i}.png")));
    }
    let out = super::cache::save_in(dir, stamp, &super::cache::Baked { atlases, grid }, &meta)?;
    log::info!("gi {map}: written to {}", out.display());
    Ok(out)
}

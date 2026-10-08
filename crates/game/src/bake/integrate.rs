//! The light transport: lightmap texels and light grid points gather the
//! light arriving over their hemisphere (sphere for grid points) by tracing
//! rays. A ray that escapes sees the sky; one that hits a surface sees that
//! surface's colour times the light the previous pass found there (its
//! lightmap texel, or for models the nearest grid point), plus the sun's
//! direct light on it. Each pass adds a bounce.
//!
//! Units: "lightmap units", in which a value of 1 lights a surface as a
//! lightmap value of 1 does once linearised (`crate::lightmaps`): an
//! irradiance of `pi * LIGHTMAP_EXPOSURE` lux. A surface of albedo `a` under
//! irradiance `e` then has radiance `a * e`, and a texel's irradiance is the
//! mean radiance of cosine-distributed samples.
//!
//! The sun's direct light is never in the result (the game adds the live sun
//! with shadow maps); only its bounce is. Light from the sky and from the
//! sun and lamps is kept apart until the sky's brightness is fitted.

use super::scene::{Hit, NO_LIGHTMAP, Scene};
use super::sky::{Sky, luma};
use super::texels::Atlas;
use glam33::{Vec2, Vec3, Vec3A};
use iw3::zone::{LightGrid, PrimaryLight};
use rayon::prelude::*;
use std::collections::HashMap;
use std::f32::consts::{PI, TAU};

/// Rays start this far off the surface (CoD units, inches).
const OFFSET: f32 = 0.1;
/// Surfaces this near (units) count for contact occlusion.
pub const CONTACT_RANGE: f32 = 40.0;
/// Rays travel at most this far.
const FAR: f32 = 100_000.0;

/// Logs how far a long parallel loop has got, every tenth.
pub struct Progress {
    label: String,
    total: usize,
    done: std::sync::atomic::AtomicUsize,
    start: std::time::Instant,
}

impl Progress {
    pub fn new(label: impl Into<String>, total: usize) -> Progress {
        Progress { label: label.into(), total: total.max(1), done: Default::default(), start: std::time::Instant::now() }
    }

    pub fn tick(&self) {
        let n = self.done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        let step = (self.total / 10).max(1);
        if n % step == 0 && n < self.total {
            let f = n as f32 / self.total as f32;
            let t = self.start.elapsed().as_secs_f32();
            // (Short loops log nothing.)
            if t < 2.0 {
                return;
            }
            log::info!("bake: {} {:.0}% ({:.0}s, ~{:.0}s left)", self.label, f * 100.0, t, t / f - t);
        }
    }
}

pub fn cosine_dir(u: f32, v: f32) -> Vec3 {
    let r = u.sqrt();
    let phi = TAU * v;
    Vec3::new(r * phi.cos(), r * phi.sin(), (1.0 - u).max(0.0).sqrt())
}

fn radical_inverse(i: u32) -> f32 {
    i.reverse_bits() as f32 * 2.328_306_4e-10
}

fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^ (x >> 16)
}

fn rand2(seed: u32) -> Vec2 {
    let a = hash(seed);
    let b = hash(a ^ 0x9e37_79b9);
    Vec2::new(a as f32 / 4_294_967_296.0, b as f32 / 4_294_967_296.0)
}

/// Sample `j` of `n` of a 2D low-discrepancy set, rotated by `rot`.
fn sample2(j: u32, n: u32, rot: Vec2) -> Vec2 {
    let s = Vec2::new((j as f32 + 0.5) / n as f32, radical_inverse(j)) + rot;
    s - s.floor()
}

fn basis(n: Vec3A) -> (Vec3A, Vec3A) {
    let t = n.any_orthonormal_vector();
    (t, n.cross(t))
}

#[derive(Clone)]
pub struct Spot {
    pub origin: Vec3A,
    /// Towards the light from what it lights (the spot shines along -axis).
    pub axis: Vec3A,
    pub colour: Vec3,
    pub radius: f32,
    /// Cone (cosines); `None` for an omni light.
    pub cone: Option<(f32, f32)>,
    pub quadratic: bool,
}

pub struct Lights {
    pub to_sun: Vec3A,
    /// The live sun's irradiance (lightmap units) on a surface facing it.
    pub sun: Vec3,
    pub spots: Vec<Spot>,
}

/// The sun disc's angular radius for soft shadows (radians), a little over
/// the real sun's for the haze.
const SUN_RADIUS: f32 = 0.01;

impl Lights {
    pub fn new(world: &iw3::zone::GfxWorld, primary: &[PrimaryLight], live_sun_lux: f32) -> Lights {
        let sun = &world.sun;
        let (pitch, yaw) = (sun.angles[0].to_radians(), sun.angles[1].to_radians());
        let to_sun = Vec3A::new(pitch.cos() * yaw.cos(), pitch.cos() * yaw.sin(), -pitch.sin()).normalize();
        let lux_to_units = 1.0 / (PI * crate_exposure());
        let spots = primary
            .iter()
            .filter(|l| (l.kind == 2 || l.kind == 3) && l.radius > 0.0)
            .map(|l| Spot {
                origin: Vec3A::from(l.origin),
                axis: Vec3A::from(l.dir).normalize_or(Vec3A::Z),
                colour: Vec3::from(l.color),
                radius: l.radius,
                cone: (l.kind == 2 && l.cos_half_fov_inner > l.cos_half_fov_outer).then_some((l.cos_half_fov_outer, l.cos_half_fov_inner)),
                quadratic: l.def_name.as_deref().is_some_and(|d| d.contains("quadratic")),
            })
            .collect();
        Lights { to_sun, sun: Vec3::from(sun.sun_color) * live_sun_lux * lux_to_units, spots }
    }

    /// Direct light from the lamps at `p` (normal `n` for the cosine, `nf`
    /// to start shadow rays off the surface): per lamp its irradiance (on
    /// a surface facing it) and direction.
    fn lamps(&self, scene: &Scene, p: Vec3A, nf: Vec3A, seed: u32, mut each: impl FnMut(Vec3, Vec3A)) {
        let o = p + nf * OFFSET;
        for (k, s) in self.spots.iter().enumerate() {
            let to = s.origin - o;
            let dist = to.length();
            if dist >= s.radius || dist < 1e-3 {
                continue;
            }
            let l = to / dist;
            let mut f = 1.0 - dist / s.radius;
            if s.quadratic {
                f *= f;
            }
            if let Some((outer, inner)) = s.cone {
                let c = l.dot(s.axis);
                f *= ((c - outer) / (inner - outer)).clamp(0.0, 1.0);
            }
            if f <= 0.0 || (nf != Vec3A::ZERO && l.dot(nf) <= 0.0) {
                continue;
            }
            // Soft shadows from a lamp a few inches across.
            const N: u32 = 8;
            let (t1, t2) = basis(l);
            let rot = rand2(seed ^ (k as u32 * 7919));
            let mut lit = 0;
            for j in 0..N {
                let r = sample2(j, N, rot);
                let (a, b) = ((r.x - 0.5) * 4.0, (r.y - 0.5) * 4.0);
                let target = s.origin + t1 * a + t2 * b;
                let d = target - o;
                let len = d.length();
                if !scene.occluded(o, d / len, len - 1.0) {
                    lit += 1;
                }
            }
            if lit > 0 {
                each(s.colour * f * lit as f32 / N as f32, l);
            }
        }
    }

    /// The sun's direct light at `p` (irradiance on a surface facing it,
    /// times how much of the disc is seen), soft-shadowed.
    fn sun_seen(&self, scene: &Scene, p: Vec3A, nf: Vec3A, samples: u32, seed: u32) -> f32 {
        if self.to_sun.dot(nf) <= 0.0 {
            return 0.0;
        }
        let o = p + nf * OFFSET;
        let (t1, t2) = basis(self.to_sun);
        let rot = rand2(seed);
        let mut lit = 0;
        for j in 0..samples {
            let r = sample2(j, samples, rot);
            let (rad, phi) = (r.x.sqrt() * SUN_RADIUS, r.y * TAU);
            let d = (self.to_sun + t1 * rad * phi.cos() + t2 * rad * phi.sin()).normalize();
            if !scene.occluded(o, d, FAR) {
                lit += 1;
            }
        }
        lit as f32 / samples as f32
    }
}

/// `crate::lightmaps::LIGHTMAP_EXPOSURE` (kept here so the baker builds on
/// its own).
pub const EXPOSURE: f32 = 3_000.0;
fn crate_exposure() -> f32 {
    EXPOSURE
}

/// Light split into what came from the sky and what from the sun and lamps.
#[derive(Clone, Copy, Default)]
pub struct Split {
    pub sky: Vec3,
    pub sun: Vec3,
}

impl std::ops::AddAssign for Split {
    fn add_assign(&mut self, o: Split) {
        self.sky += o.sky;
        self.sun += o.sun;
    }
}

impl std::ops::Mul<f32> for Split {
    type Output = Split;
    fn mul(self, k: f32) -> Split {
        Split { sky: self.sky * k, sun: self.sun * k }
    }
}

/// One atlas's lighting: fixed direct light and the latest pass's bounce.
pub struct AtlasLight {
    pub atlas: Atlas,
    /// For every texel, the nearest covered texel (for looking up hits that
    /// land in padding): `u32::MAX` if none near.
    pub nearest: Vec<u32>,
    /// The live sun's direct light at each texel (only bounced).
    pub sun: Vec<Vec3>,
    /// The lamps' direct light at each texel.
    pub lamps: Vec<Vec3>,
    /// The primary lights' share of `lamps` (once lamps are fitted).
    pub primary: Vec<Vec3>,
    /// Light CoD4's lightmap has that the sky, sun and primary lights don't
    /// explain: its lamps, which the compiled map no longer lists. Kept as
    /// a fixed light (and its direction, tangent space) that bounces.
    pub local: Vec<(Vec3, Vec3)>,
    /// Gathered light from the last pass.
    pub gathered: Vec<Split>,
}

impl AtlasLight {
    pub fn new(atlas: Atlas) -> AtlasLight {
        let n = atlas.texels.len();
        let mut a = AtlasLight { atlas, nearest: Vec::new(), sun: vec![Vec3::ZERO; n], lamps: vec![Vec3::ZERO; n], primary: vec![Vec3::ZERO; n], local: vec![(Vec3::ZERO, Vec3::Z); n], gathered: vec![Split::default(); n] };
        a.find_nearest();
        a
    }

    /// Recompute `nearest`: covered texels themselves, padding within a
    /// few texels the closest covered one.
    pub fn find_nearest(&mut self) {
        let (w, h) = (self.atlas.w, self.atlas.h);
        let mut nearest: Vec<u32> = (0..w * h).map(|i| if self.atlas.texels[i].is_some() { i as u32 } else { u32::MAX }).collect();
        for _ in 0..4 {
            let prev = nearest.clone();
            for y in 0..h {
                for x in 0..w {
                    if prev[y * w + x] != u32::MAX {
                        continue;
                    }
                    for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1), (-1, -1), (1, 1), (-1, 1), (1, -1)] {
                        let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                        if nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h && prev[ny as usize * w + nx as usize] != u32::MAX {
                            nearest[y * w + x] = prev[ny as usize * w + nx as usize];
                            break;
                        }
                    }
                }
            }
        }
        self.nearest = nearest;
    }

    fn at(&self, uv: Vec2) -> Option<usize> {
        let i = self.atlas.index(uv)?;
        let n = self.nearest[i];
        (n != u32::MAX).then_some(n as usize)
    }
}

/// The light grid: CoD4's points, each an ambient cube (light arriving on
/// a surface facing +x, -x, +y, -y, +z, -z).
pub struct Grid {
    pub points: Vec<[u32; 3]>,
    index: HashMap<[u32; 3], u32>,
    pub cubes: Vec<[Split; 6]>,
    /// As [`AtlasLight::local`]: CoD4's grid light beyond the bake's.
    pub local: Vec<[Vec3; 6]>,
    /// CoD4's grid units per bake unit (see `bake::add_local`).
    pub scale: f32,
}

const FACES: [Vec3A; 6] = [Vec3A::X, Vec3A::NEG_X, Vec3A::Y, Vec3A::NEG_Y, Vec3A::Z, Vec3A::NEG_Z];

impl Grid {
    pub fn new(grid: &LightGrid) -> Grid {
        let points: Vec<[u32; 3]> = grid.points().into_iter().map(|(p, _)| p).collect();
        let index = points.iter().enumerate().map(|(i, p)| (*p, i as u32)).collect();
        let cubes = vec![[Split::default(); 6]; points.len()];
        let local = vec![[Vec3::ZERO; 6]; points.len()];
        Grid { points, index, cubes, local, scale: 1.0 }
    }

    pub fn pos(p: [u32; 3]) -> Vec3A {
        Vec3A::from(LightGrid::world_pos(p))
    }

    /// The light on a surface at `p` facing `n`, from the nearest point.
    fn eval(&self, p: Vec3A, n: Vec3A) -> Option<Split> {
        let sp = LightGrid::SPACING;
        let q: [i64; 3] = std::array::from_fn(|i| ((p[i] + 131_072.0) / sp[i]).round() as i64);
        let mut found = None;
        'search: for r in 0..=2i64 {
            for dz in -r..=r {
                for dy in -r..=r {
                    for dx in -r..=r {
                        if dx.abs().max(dy.abs()).max(dz.abs()) != r {
                            continue;
                        }
                        let c = [q[0] + dx, q[1] + dy, q[2] + dz];
                        if c.iter().any(|&v| v < 0) {
                            continue;
                        }
                        if let Some(&i) = self.index.get(&c.map(|v| v as u32)) {
                            found = Some(i as usize);
                            break 'search;
                        }
                    }
                }
            }
        }
        let i = found?;
        let cube = &self.cubes[i];
        let mut out = Split::default();
        let n2 = n * n;
        for (axis, w) in [n2.x, n2.y, n2.z].into_iter().enumerate() {
            let face = if n[axis] >= 0.0 { axis * 2 } else { axis * 2 + 1 };
            out += cube[face] * w;
            out.sun += self.local[i][face] * w;
        }
        Some(out)
    }
}

pub struct Baker<'a> {
    pub scene: &'a Scene,
    pub sky: &'a Sky,
    pub lights: &'a Lights,
}

/// What one ray brings back.
enum Seen {
    /// The light, and how far away its surface is (infinite for the sky).
    Light(Split, f32),
    /// The back of a one-sided surface, this far away: near, the ray
    /// started inside something; far, it blocks the light.
    Back(f32),
}

impl Baker<'_> {
    fn trace(&self, atlases: &[AtlasLight], grid: &Grid, o: Vec3A, d: Vec3A) -> Seen {
        let Some(hit) = self.scene.closest(o, d, FAR) else {
            return Seen::Light(Split { sky: self.sky.radiance(d), sun: Vec3::ZERO }, f32::INFINITY);
        };
        if !hit.front {
            return Seen::Back(hit.t);
        }
        Seen::Light(self.surface(atlases, grid, &hit, d), hit.t)
    }

    /// The light leaving a hit surface towards the ray.
    fn surface(&self, atlases: &[AtlasLight], grid: &Grid, hit: &Hit, d: Vec3A) -> Split {
        let scene = self.scene;
        let albedo = scene.albedo(hit);
        let info = &scene.info[hit.tri];
        if info.lightmap != NO_LIGHTMAP {
            if let Some(a) = atlases.get(info.lightmap as usize) {
                if let Some(i) = a.at(scene.lightmap_uv(hit)) {
                    let g = a.gathered[i];
                    return Split { sky: albedo * g.sky, sun: albedo * (g.sun + a.sun[i] + a.lamps[i] + a.local[i].0) };
                }
            }
        }
        // Models (and the odd surface without a lightmap): the grid's light
        // and the sun, shadowed.
        let p = scene.point(hit);
        let n = if info.nf.dot(d) > 0.0 { -info.nf } else { info.nf };
        let mut e = grid.eval(p, n).unwrap_or(Split { sky: self.sky.ground * 0.5, sun: Vec3::ZERO });
        let c = n.dot(self.lights.to_sun);
        if c > 0.0 && !scene.occluded(p + n * OFFSET, self.lights.to_sun, FAR) {
            e.sun += self.lights.sun * c;
        }
        Split { sky: albedo * e.sky, sun: albedo * e.sun }
    }

    /// The fixed direct light at every texel: the sun (for bouncing) and
    /// the lamps.
    pub fn direct(&self, atlases: &mut [AtlasLight]) {
        for a in atlases.iter_mut() {
            let texels = &a.atlas.texels;
            let out: Vec<(Vec3, Vec3)> = (0..texels.len())
                .into_par_iter()
                .map(|i| {
                    let Some(t) = texels[i] else { return (Vec3::ZERO, Vec3::ZERO) };
                    let seed = hash(i as u32 ^ 0x51ed);
                    let sun = self.lights.sun * t.n.dot(self.lights.to_sun).max(0.0) * self.lights.sun_seen(self.scene, t.pos, t.nf, 8, seed);
                    let mut lamps = Vec3::ZERO;
                    self.lights.lamps(self.scene, t.pos, t.nf, seed, |e, l| lamps += e * t.n.dot(l).max(0.0));
                    (sun, lamps)
                })
                .collect();
            a.sun = out.iter().map(|o| o.0).collect();
            a.lamps = out.iter().map(|o| o.1).collect();
        }
    }

    /// One gathering pass over every texel: the light arriving at each.
    /// Texels that mostly see back faces (inside walls) are dropped.
    pub fn gather(&self, atlases: &[AtlasLight], grid: &Grid, samples: u32, pass: u32) -> Vec<(Vec<Split>, Vec<bool>)> {
        atlases
            .iter()
            .map(|a| {
                let texels = &a.atlas.texels;
                let progress = Progress::new(format!("bounce {} texels", pass + 1), texels.len());
                (0..texels.len())
                    .into_par_iter()
                    .map(|i| {
                        progress.tick();
                        let Some(t) = texels[i] else { return (Split::default(), false) };
                        let (t1, t2) = basis(t.n);
                        let o = t.pos + t.nf * OFFSET;
                        let rot = rand2(hash(i as u32) ^ pass.wrapping_mul(0x632b_e5ab));
                        let mut sum = Split::default();
                        let mut back = 0;
                        // Inside a wall, the back faces are right there; a one-sided prop a
                        // little way off only blocks light.
                        let near = (t.size * 0.5).clamp(2.0, 8.0);
                        for j in 0..samples {
                            let s = sample2(j, samples, rot);
                            let l = cosine_dir(s.x, s.y);
                            let d = (t1 * l.x + t2 * l.y + t.n * l.z).normalize();
                            if d.dot(t.nf) <= 0.0 {
                                continue;
                            }
                            match self.trace(atlases, grid, o, d) {
                                Seen::Light(l, _) => sum += l,
                                Seen::Back(t) if t < near => back += 1,
                                Seen::Back(_) => {}
                            }
                        }
                        (sum * (1.0 / samples as f32), back * 4 > samples)
                    })
                    .unzip()
            })
            .collect()
    }

    /// The final pass: the light arriving at every texel, with the sky at
    /// its fitted brightness `k`, as its irradiance `e` and its mean
    /// direction per colour channel `d` (tangent space), lamps included.
    ///
    /// `contact` darkens light arriving past nearby surfaces beyond what
    /// tracing alone gives (0: none, 1: by all of the share of rays that hit
    /// something within [`CONTACT_RANGE`]), for stronger contact shadows.
    pub fn gather_final(&self, atlases: &[AtlasLight], grid: &Grid, samples: u32, k: f32, ai: usize, contact: f32) -> Vec<Directional> {
        let a = &atlases[ai];
        let texels = &a.atlas.texels;
        let progress = Progress::new("final gather", texels.len());
        (0..texels.len())
            .into_par_iter()
            .map(|i| {
                progress.tick();
                let Some(t) = texels[i] else { return Directional::default() };
                let (t1, t2) = basis(t.n);
                let o = t.pos + t.nf * OFFSET;
                let rot = rand2(hash(i as u32) ^ 0xfeed_beef);
                let to_tangent = |d: Vec3A| Vec3::new(d.dot(t.t), d.dot(t.b), d.dot(t.n));
                let mut out = Directional::default();
                let mut near = 0u32;
                for j in 0..samples {
                    let s = sample2(j, samples, rot);
                    let l = cosine_dir(s.x, s.y);
                    let d = (t1 * l.x + t2 * l.y + t.n * l.z).normalize();
                    if d.dot(t.nf) <= 0.0 {
                        continue;
                    }
                    match self.trace(atlases, grid, o, d) {
                        Seen::Light(l, t) => {
                            out.add(l.sky * k + l.sun, to_tangent(d));
                            near += (t < CONTACT_RANGE) as u32;
                        }
                        Seen::Back(t) => near += (t < CONTACT_RANGE) as u32,
                    }
                }
                out.scale((1.0 - contact * near as f32 / samples as f32).max(0.0) / samples as f32);
                let (local, dir) = a.local[i];
                out.add(local, dir);
                self.lights.lamps(self.scene, t.pos, t.nf, hash(i as u32 ^ 0x51ed), |e, l| {
                    let c = t.n.dot(l).max(0.0);
                    out.add(e * c, to_tangent(l));
                });
                out
            })
            .collect()
    }

    /// One pass over the light grid's points.
    pub fn gather_grid(&self, atlases: &[AtlasLight], grid: &Grid, samples: u32, pass: u32) -> Vec<Option<[Split; 6]>> {
        let progress = Progress::new(format!("grid pass {}", pass + 1), grid.points.len());
        (0..grid.points.len())
            .into_par_iter()
            .map(|i| {
                progress.tick();
                let p = Grid::pos(grid.points[i]);
                let rot = rand2(hash(i as u32 ^ 0xabcd) ^ pass.wrapping_mul(0x632b_e5ab));
                let mut cube = [Split::default(); 6];
                let mut back = 0;
                for j in 0..samples {
                    let s = sample2(j, samples, rot);
                    let z = 1.0 - 2.0 * s.x;
                    let r = (1.0 - z * z).max(0.0).sqrt();
                    let d = Vec3A::new(r * (TAU * s.y).cos(), r * (TAU * s.y).sin(), z);
                    match self.trace(atlases, grid, p, d) {
                        Seen::Light(l, _) => {
                            for (f, axis) in FACES.iter().enumerate() {
                                let c = d.dot(*axis);
                                if c > 0.0 {
                                    cube[f] += l * c;
                                }
                            }
                        }
                        Seen::Back(t) if t < 32.0 => back += 1,
                        Seen::Back(_) => {}
                    }
                }
                if back * 4 > samples {
                    return None;
                }
                let k = 4.0 / samples as f32;
                for f in &mut cube {
                    *f = *f * k;
                }
                self.lights.lamps(self.scene, p, Vec3A::ZERO, hash(i as u32), |e, l| {
                    for (f, axis) in FACES.iter().enumerate() {
                        cube[f].sun += e * l.dot(*axis).max(0.0);
                    }
                });
                Some(cube)
            })
            .collect()
    }
}

/// Light arriving at a texel: irradiance and, per channel, the mean
/// direction weighted by it (tangent space).
#[derive(Clone, Copy, Default)]
pub struct Directional {
    pub e: Vec3,
    pub d: [Vec3; 3],
}

impl Directional {
    pub fn add(&mut self, l: Vec3, dir: Vec3) {
        self.e += l;
        self.d[0] += dir * l.x;
        self.d[1] += dir * l.y;
        self.d[2] += dir * l.z;
    }

    fn scale(&mut self, k: f32) {
        self.e *= k;
        for d in &mut self.d {
            *d *= k;
        }
    }

    pub fn lerp_add(&mut self, o: &Directional, w: f32) {
        self.e += o.e * w;
        for c in 0..3 {
            self.d[c] += o.d[c] * w;
        }
    }

    /// IW3's lightmap model, `A * n.z + B * max(0, dot(n, L))`, fitted:
    /// the light as an even part (A) plus light from one direction (B, L).
    /// An even hemisphere's mean direction is (0, 0, 2/3); the directional
    /// part's is L.
    pub fn fit(&self) -> (Vec3, Vec3, Vec3) {
        let e = self.e.max(Vec3::ZERO);
        let lum = |v: Vec3| luma(v);
        let d_lum = Vec3::new(lum(Vec3::new(self.d[0].x, self.d[1].x, self.d[2].x)), lum(Vec3::new(self.d[0].y, self.d[1].y, self.d[2].y)), lum(Vec3::new(self.d[0].z, self.d[1].z, self.d[2].z)));
        let e_lum = lum(e);
        if e_lum <= 1e-8 {
            return (Vec3::ZERO, Vec3::ZERO, Vec3::Z);
        }
        // Direction of the directional part: the mean direction less the
        // even part's, refined a few times.
        let mut l = d_lum.normalize_or(Vec3::Z);
        for _ in 0..4 {
            let q = ((d_lum.dot(l) - 2.0 / 3.0 * l.z * e_lum) / (1.0 - 2.0 / 3.0 * l.z).max(1e-3)).clamp(0.0, e_lum);
            let even = e_lum - q;
            l = (d_lum - Vec3::Z * (2.0 / 3.0 * even)).normalize_or(Vec3::Z);
            if l.z < 0.05 {
                l = Vec3::new(l.x, l.y, 0.05).normalize();
            }
        }
        let lz = l.z.max(0.25);
        let mut a = Vec3::ZERO;
        let mut b = Vec3::ZERO;
        for c in 0..3 {
            let q = ((self.d[c].dot(l) - 2.0 / 3.0 * l.z * e[c]) / (1.0 - 2.0 / 3.0 * l.z).max(1e-3)).clamp(0.0, e[c]);
            a[c] = e[c] - q;
            b[c] = q / lz;
        }
        (a, b, l)
    }
}

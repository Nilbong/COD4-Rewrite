//! CoD4's lamps, recovered. The compiled map keeps only its primary lights;
//! the lamps that lit its rooms survive only in the lightmap. Here they're
//! found again as point lights, so the bake can light with them properly
//! (sharp, shadowed, bouncing) instead of copying CoD4's blurry result:
//!
//! - candidates: light fixture models placed in the map, and the points
//!   where CoD4's lightmap directions converge (in a texel lit by a lamp,
//!   the lightmap's direction points at it);
//! - each candidate, at a few falloff radii, is fitted to the light CoD4's
//!   lightmap has beyond the bake's sky, sun and primary lights, by
//!   non-negative least squares per colour channel, with shadow rays.

use super::integrate::{AtlasLight, Spot};
use super::scene::Scene;
use super::sky::luma;
use glam33::{Vec3, Vec3A};
use iw3::zone::Zone;
use rayon::prelude::*;
use std::collections::HashMap;

/// Falloffs tried for each candidate: radius (CoD units) and whether it
/// falls off quadratically (a tight pool) or linearly (CoD4's
/// `light_point_linear`, a broad wash).
const RADII: [(f32, bool); 6] = [(120.0, false), (250.0, false), (450.0, false), (250.0, true), (450.0, true), (700.0, true)];
/// Voting voxel size (units).
const VOXEL: f32 = 24.0;
/// Candidates closer than this are one lamp.
const MERGE: f32 = 64.0;
/// The most candidates from voting.
const MAX_VOTED: usize = 96;

/// One residual sample: a texel's position and normals, and the light CoD4
/// has there beyond the bake's (and its direction, world space).
pub struct Sample {
    pub pos: Vec3A,
    pub n: Vec3A,
    pub nf: Vec3A,
    pub residual: Vec3,
    pub dir: Vec3A,
}

/// Collect residual samples (every other texel each way).
pub fn samples(atlases: &[AtlasLight], residual: impl Fn(usize, usize) -> Option<(Vec3, Vec3)>) -> Vec<Sample> {
    let mut out = Vec::new();
    for (ai, a) in atlases.iter().enumerate() {
        let w = a.atlas.w;
        for (i, t) in a.atlas.texels.iter().enumerate() {
            let Some(t) = t else { continue };
            if (i % w) % 2 != 0 || (i / w) % 2 != 0 {
                continue;
            }
            let Some((r, l)) = residual(ai, i) else { continue };
            let dir = (t.t * l.x + t.b * l.y + t.n * l.z).normalize_or(t.n);
            out.push(Sample { pos: t.pos, n: t.n, nf: t.nf, residual: r, dir });
        }
    }
    out
}

/// Light fixture models: where the lamp is (the bottom of the model's
/// bounds, a little below).
fn fixtures(zones: &[Zone]) -> Vec<Vec3A> {
    let world = zones[0].gfx_world().expect("gfxworld");
    let mut out = Vec::new();
    for sm in &world.static_models {
        let Some(id) = sm.model else { continue };
        let (zi, id) = super::scene::resolve_xmodel(zones, 0, id);
        let Some(xm) = zones[zi].xmodel(id) else { continue };
        let name = xm.name.to_ascii_lowercase();
        let lamp = ["light", "lamp", "bulb", "fluo", "lantern", "sconce", "chandelier"].iter().any(|k| name.contains(k));
        if !lamp || name.contains("lightning") || name.contains("lighthouse") {
            continue;
        }
        let axis = sm.axis.map(Vec3A::from);
        let local = Vec3A::new((xm.mins[0] + xm.maxs[0]) * 0.5, (xm.mins[1] + xm.maxs[1]) * 0.5, xm.mins[2]);
        let p = Vec3A::from(sm.origin) + (axis[0] * local.x + axis[1] * local.y + axis[2] * local.z) * sm.scale;
        out.push(p - Vec3A::Z * 4.0);
    }
    out
}

/// Where the residual light's directions converge.
fn voted(samples: &[Sample]) -> Vec<Vec3A> {
    let mut votes: HashMap<[i32; 3], (f32, Vec3A)> = HashMap::new();
    for s in samples {
        let w = luma(s.residual);
        if w < 0.03 || s.dir.dot(s.n) < 0.2 {
            continue;
        }
        let mut d = 16.0;
        while d < 600.0 {
            let p = s.pos + s.dir * d;
            let key = (p / VOXEL).floor().as_ivec3().to_array();
            let e = votes.entry(key).or_insert((0.0, Vec3A::ZERO));
            // Near voxels are crossed by more rays from each texel: weight
            // by distance so far lamps aren't drowned out.
            e.0 += w * d / 100.0;
            e.1 += p * w * d / 100.0;
            d += VOXEL * 0.5;
        }
    }
    let mut peaks: Vec<(f32, Vec3A)> = votes
        .iter()
        .filter(|(k, (v, _))| {
            (-1..=1).all(|dz| {
                (-1..=1).all(|dy| (-1..=1).all(|dx| votes.get(&[k[0] + dx, k[1] + dy, k[2] + dz]).is_none_or(|(o, _)| o <= v)))
            })
        })
        .map(|(_, (v, c))| (*v, *c / *v))
        .collect();
    peaks.sort_by(|a, b| b.0.total_cmp(&a.0));
    let top = peaks.first().map_or(0.0, |p| p.0);
    peaks.into_iter().filter(|p| p.0 > top * 0.02).take(MAX_VOTED).map(|p| p.1).collect()
}

/// Fit lamps to `samples`; returns them as lights for the bake.
pub fn fit(zones: &[Zone], scene: &Scene, samples: &[Sample], scale: f32) -> Vec<Spot> {
    let mut candidates: Vec<Vec3A> = Vec::new();
    let fixture = fixtures(zones);
    let vote = voted(samples);
    for p in fixture.iter().chain(&vote) {
        if candidates.iter().all(|c| c.distance(*p) > MERGE) {
            candidates.push(*p);
        }
    }
    log::info!("bake: lamp candidates: {} fixtures, {} from the lightmap's directions, {} after merging", fixture.len(), vote.len(), candidates.len());

    // Each sample's response to each candidate (unit brightness, before
    // falloff): the cosine if it sees it.
    let k = candidates.len();
    let bases = k * RADII.len();
    let max_r = RADII.iter().fold(0.0f32, |m, r| m.max(r.0));
    let rows: Vec<Vec<(u32, f32)>> = samples
        .par_iter()
        .map(|s| {
            let o = s.pos + s.nf * 0.1;
            let mut row = Vec::new();
            for (c, p) in candidates.iter().enumerate() {
                let to = *p - o;
                let d = to.length();
                if d >= max_r || d < 1.0 {
                    continue;
                }
                let l = to / d;
                let cos = s.n.dot(l);
                if cos <= 0.0 || l.dot(s.nf) <= 0.0 || scene.occluded(o, l, d - 2.0) {
                    continue;
                }
                for (ri, &(r, quadratic)) in RADII.iter().enumerate() {
                    if d < r {
                        let f = 1.0 - d / r;
                        row.push(((c * RADII.len() + ri) as u32, cos * if quadratic { f * f } else { f }));
                    }
                }
            }
            row
        })
        .collect();
    // Normal equations, then non-negative coordinate descent per channel.
    let mut gram = vec![0.0f64; bases * bases];
    let mut rhs = vec![[0.0f64; 3]; bases];
    for (row, s) in rows.iter().zip(samples) {
        for &(a, va) in row {
            for c in 0..3 {
                rhs[a as usize][c] += va as f64 * s.residual[c] as f64;
            }
            for &(b, vb) in row {
                gram[a as usize * bases + b as usize] += va as f64 * vb as f64;
            }
        }
    }
    let mut x = vec![[0.0f64; 3]; bases];
    for _ in 0..300 {
        for j in 0..bases {
            let g = gram[j * bases + j];
            if g <= 0.0 {
                continue;
            }
            for c in 0..3 {
                let mut sum = rhs[j][c];
                for i in 0..bases {
                    if i != j {
                        sum -= gram[j * bases + i] * x[i][c];
                    }
                }
                x[j][c] = (sum / g).max(0.0);
            }
        }
    }
    // How much of the residual the lamps explain.
    let (mut before, mut after) = (0.0f64, 0.0f64);
    for (row, s) in rows.iter().zip(samples) {
        let mut fit = [0.0f64; 3];
        for &(j, v) in row {
            for c in 0..3 {
                fit[c] += v as f64 * x[j as usize][c];
            }
        }
        for c in 0..3 {
            before += (s.residual[c] as f64).powi(2);
            after += (s.residual[c] as f64 - fit[c]).powi(2);
        }
    }
    let mut lamps = Vec::new();
    for (j, v) in x.iter().enumerate() {
        let colour = Vec3::new(v[0] as f32, v[1] as f32, v[2] as f32) * scale;
        if luma(colour) < 0.02 {
            continue;
        }
        let (radius, quadratic) = RADII[j % RADII.len()];
        lamps.push(Spot { origin: candidates[j / RADII.len()], axis: Vec3A::Z, colour, radius, cone: None, quadratic });
    }
    log::info!(
        "bake: {} lamps fitted at {} spots; they explain {:.0}% of the light CoD4 has beyond the bake's",
        lamps.len(),
        lamps.iter().map(|l| l.origin.to_array().map(|v| v as i32)).collect::<std::collections::HashSet<_>>().len(),
        100.0 * (1.0 - after / before.max(1e-12))
    );
    lamps
}

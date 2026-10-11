//! Lightmap texels: where each texel of a (scaled-up) lightmap atlas lies on
//! the map, found by drawing every lightmapped triangle into the atlas by
//! its lightmap coordinates.

use super::scene::unit;
use glam33::{Vec2, Vec3A};
use iw3::zone::GfxWorld;

#[derive(Clone, Copy)]
pub struct Texel {
    pub pos: Vec3A,
    /// Smooth (vertex) normal.
    pub n: Vec3A,
    /// Face normal, for offsetting rays off the surface.
    pub nf: Vec3A,
    /// Tangent frame of the shader's tangent space (IW3's: binormal =
    /// cross(normal, tangent) * sign).
    pub t: Vec3A,
    pub b: Vec3A,
    /// How far the texel's centre is outside its triangle (texels): 0
    /// inside. Texels just past an edge still light that edge.
    pub outside: f32,
    /// World size of a texel (units), for spacing samples.
    pub size: f32,
    /// The world surface (index in `GfxWorld::surfaces`).
    pub surf: u32,
}

pub struct Atlas {
    /// Size of one half (the A or B layer) at the bake's resolution.
    pub w: usize,
    pub h: usize,
    pub texels: Vec<Option<Texel>>,
    /// Texels dropped from the bake as inside walls (they take CoD4's
    /// light): index and texel.
    pub dropped: Vec<(usize, Texel)>,
    /// Texels a thin wall's two sides share (CoD4 lit them for one): the
    /// side not kept in `texels`, to bake too and choose between.
    pub other_side: Vec<(usize, Texel)>,
}

impl Atlas {
    pub fn index(&self, uv: Vec2) -> Option<usize> {
        let x = (uv.x * self.w as f32).floor();
        let y = (uv.y * self.h as f32).floor();
        (x >= 0.0 && y >= 0.0 && (x as usize) < self.w && (y as usize) < self.h).then(|| y as usize * self.w + x as usize)
    }
}

/// How far outside a triangle a texel centre may be and still take its
/// light (texels): a texel that the triangle touches.
const CONSERVATIVE: f32 = 0.75;

fn bary(p: Vec2, a: Vec2, b: Vec2, c: Vec2) -> Option<[f32; 3]> {
    let v0 = b - a;
    let v1 = c - a;
    let v2 = p - a;
    let d = v0.perp_dot(v1);
    if d.abs() < 1e-12 {
        return None;
    }
    let u = v2.perp_dot(v1) / d;
    let v = v0.perp_dot(v2) / d;
    Some([1.0 - u - v, u, v])
}

fn closest_on_segment(p: Vec2, a: Vec2, b: Vec2) -> Vec2 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-12)).clamp(0.0, 1.0);
    a + ab * t
}

/// Rasterise every lightmapped world triangle of lightmap `index` into a
/// `w` x `h` atlas layer (its lightmap coordinates span 0..1 whatever the
/// size).
pub fn rasterise(world: &GfxWorld, index: u8, w: usize, h: usize) -> Atlas {
    let mut atlas = Atlas { w, h, texels: vec![None; w * h], dropped: Vec::new(), other_side: Vec::new() };
    // Texels two surfaces both lie over (inside both): facing apart, CoD4
    // shared them between a thin wall's sides.
    let mut shared = 0usize;
    let mut shared_facing_apart = 0usize;
    for (si, s) in world.surfaces.iter().enumerate().filter(|(_, s)| s.lightmap_index == index && s.material.is_some()) {
        let first = s.first_vertex.max(0) as usize;
        let start = s.base_index.max(0) as usize;
        let end = start + s.tri_count as usize * 3;
        if end > world.indices.len() {
            continue;
        }
        for t in world.indices[start..end].chunks_exact(3) {
            let idx = [first + t[0] as usize, first + t[1] as usize, first + t[2] as usize];
            if idx.iter().any(|&i| i >= world.vertices.len()) {
                continue;
            }
            let vs = idx.map(|i| &world.vertices[i]);
            let px = vs.map(|v| Vec2::new(v.lmap_coord[0] * w as f32, v.lmap_coord[1] * h as f32));
            let pos = vs.map(|v| Vec3A::from(v.xyz));
            let mut nf = (pos[1] - pos[0]).cross(pos[2] - pos[0]);
            let area_uv = (px[1] - px[0]).perp_dot(px[2] - px[0]).abs();
            if nf.length_squared() < 1e-12 || area_uv < 1e-8 {
                continue;
            }
            let ns = vs.map(|v| unit(v.normal));
            nf = nf.normalize();
            if nf.dot(ns[0] + ns[1] + ns[2]) < 0.0 {
                nf = -nf;
            }
            let size = ((pos[1] - pos[0]).cross(pos[2] - pos[0]).length() / area_uv).sqrt();
            let lo = px.iter().fold(Vec2::splat(f32::MAX), |a, p| a.min(*p)) - CONSERVATIVE;
            let hi = px.iter().fold(Vec2::splat(f32::MIN), |a, p| a.max(*p)) + CONSERVATIVE;
            for y in (lo.y.floor().max(0.0) as usize)..(hi.y.ceil() as usize).min(h) {
                for x in (lo.x.floor().max(0.0) as usize)..(hi.x.ceil() as usize).min(w) {
                    let c = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                    let Some(mut b) = bary(c, px[0], px[1], px[2]) else { continue };
                    let mut outside = 0.0;
                    if b.iter().any(|&v| v < 0.0) {
                        // The nearest point on the triangle.
                        let q = [(0, 1), (1, 2), (2, 0)]
                            .map(|(i, j)| closest_on_segment(c, px[i], px[j]))
                            .into_iter()
                            .min_by(|a, b| a.distance_squared(c).total_cmp(&b.distance_squared(c)))
                            .unwrap();
                        outside = q.distance(c);
                        if outside > CONSERVATIVE {
                            continue;
                        }
                        b = bary(q, px[0], px[1], px[2]).unwrap_or([1.0 / 3.0; 3]).map(|v| v.clamp(0.0, 1.0));
                    }
                    let slot = &mut atlas.texels[y * w + x];
                    if let Some(t) = slot.as_ref().filter(|t| t.outside == 0.0 && outside == 0.0 && t.surf != si as u32) {
                        shared += 1;
                        if t.nf.dot(nf) < -0.5 {
                            shared_facing_apart += 1;
                        }
                    }
                    let new = Texel { pos: Vec3A::ZERO, n: nf, nf, t: nf, b: nf, outside, size, surf: si as u32 };
                    if let Some(t) = slot.filter(|t| t.outside <= outside) {
                        // A side facing the other way under the same texel:
                        // kept to choose between once both are lit.
                        if outside == 0.0 && t.nf.dot(nf) < -0.5 && t.surf != si as u32 {
                            let p = pos[0] * b[0] + pos[1] * b[1] + pos[2] * b[2];
                            let n = (ns[0] * b[0] + ns[1] * b[1] + ns[2] * b[2]).normalize_or(nf);
                            let tg = vs.map(|v| unit(v.tangent));
                            let tan = tg[0] * b[0] + tg[1] * b[1] + tg[2] * b[2];
                            let tan = (tan - n * n.dot(tan)).normalize_or(n.any_orthonormal_vector());
                            let sign = if vs[0].binormal_sign < 0.0 { -1.0 } else { 1.0 };
                            atlas.other_side.push((y * w + x, Texel { pos: p, n, nf, t: tan, b: n.cross(tan) * sign, ..new }));
                        }
                        continue;
                    }
                    let p = pos[0] * b[0] + pos[1] * b[1] + pos[2] * b[2];
                    let n = (ns[0] * b[0] + ns[1] * b[1] + ns[2] * b[2]).normalize_or(nf);
                    let tg = vs.map(|v| unit(v.tangent));
                    let tan = tg[0] * b[0] + tg[1] * b[1] + tg[2] * b[2];
                    let tan = (tan - n * n.dot(tan)).normalize_or(n.any_orthonormal_vector());
                    let sign = if vs[0].binormal_sign < 0.0 { -1.0 } else { 1.0 };
                    *slot = Some(Texel { pos: p, n, nf, t: tan, b: n.cross(tan) * sign, outside, size, surf: si as u32 });
                }
            }
        }
    }
    if shared > 0 {
        log::info!("bake: atlas {index}: {shared} texels lie under two surfaces ({shared_facing_apart} facing apart)");
    }
    atlas
}

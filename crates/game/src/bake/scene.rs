//! The map as the baker sees it: every triangle that blocks or bounces light
//! (world surfaces and static models, CoD axes and units), each with its
//! material's colour and alpha, in a BVH for tracing.

use super::dxt;
use glam33::{Vec2, Vec3, Vec3A};
use iw3::zone::{AssetId, GfxWorld, Zone};
use obvhs::cwbvh::CwBvh;
use obvhs::ray::{Ray, RayHit};
use obvhs::triangle::Triangle;
use std::collections::HashMap;

/// No lightmap.
pub const NO_LIGHTMAP: u8 = 255;

/// Textures are read at most this big (the bounce only needs their colour;
/// alpha-tested cut-outs keep their shape at this size).
const MAX_TEXTURE: usize = 256;

pub struct Texture {
    w: usize,
    h: usize,
    /// sRGB colour and alpha, as stored (decoded per sample: four floats a
    /// texel took ~250 MB on mp_crash).
    texels: Vec<[u8; 4]>,
}

impl Texture {
    fn sample(&self, uv: Vec2) -> [f32; 4] {
        let x = ((uv.x - uv.x.floor()) * self.w as f32) as usize % self.w;
        let y = ((uv.y - uv.y.floor()) * self.h as f32) as usize % self.h;
        let p = self.texels[y * self.w + x];
        [SRGB[p[0] as usize], SRGB[p[1] as usize], SRGB[p[2] as usize], p[3] as f32 / 255.0]
    }
}

pub struct Material {
    pub name: String,
    /// Alpha below this lets light through (alpha-tested foliage, fences).
    pub alpha_test: Option<f32>,
    pub two_sided: bool,
    texture: Option<Texture>,
}

#[derive(Clone, Copy)]
pub struct TriInfo {
    pub mat: u32,
    pub uv: [Vec2; 3],
    /// Lightmap coordinates (0..1 over one half of the atlas).
    pub lm: [Vec2; 3],
    pub lightmap: u8,
    /// Linear vertex colour (the tri's mean), multiplying the texture.
    pub colour: Vec3,
    /// Front face normal.
    pub nf: Vec3A,
}

pub struct Scene {
    pub tris: Vec<Triangle>,
    pub info: Vec<TriInfo>,
    pub mats: Vec<Material>,
    bvh: CwBvh,
    /// Colour saturation of bounced light (1: the textures' own).
    pub saturation: f32,
}

pub struct Hit {
    pub tri: usize,
    pub t: f32,
    /// Barycentric weights of v1 and v2.
    pub bary: Vec2,
    pub front: bool,
}

/// A world vertex's normal, CoD axes.
pub fn unit(packed: u32) -> Vec3A {
    Vec3A::from(iw3::unpack::unit_vec(packed)).normalize_or_zero()
}

/// sRGB byte to linear.
static SRGB: std::sync::LazyLock<[f32; 256]> = std::sync::LazyLock::new(|| {
    std::array::from_fn(|i| {
        let c = i as f32 / 255.0;
        if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    })
});

/// A material's render state bits (its lit technique's, or the first).
pub fn state_bits(mat: &iw3::zone::Material) -> Option<[u32; 2]> {
    [iw3::zone::TECHNIQUE_LIT, 8, iw3::zone::TECHNIQUE_UNLIT, iw3::zone::TECHNIQUE_EMISSIVE]
        .into_iter()
        .find_map(|t| mat.state_bits_for(t))
        .or_else(|| mat.state_bits.first().copied())
}

/// How a material takes part: `None` lets light through (sky, glass,
/// decals, additive beams).
fn classify(zone: &Zone, id: AssetId) -> Option<(Option<f32>, bool)> {
    let mat = zone.material(id)?;
    let techset = mat.technique_set.and_then(|t| zone.technique_set(t)).map_or("", |t| t.name.as_str());
    if techset.contains("sky") || techset.starts_with("wc_water") || techset.contains("effect") || techset.contains("falloff") {
        return None;
    }
    let [b0, b1] = state_bits(mat).unwrap_or([0, 0]);
    let (src, dst, blend_op) = (b0 & 0xf, (b0 >> 4) & 0xf, (b0 >> 8) & 0x7);
    if blend_op != 0 && !(src == 2 && dst == 1) {
        return None;
    }
    // Polygon offset: a decal lying on another surface.
    if (b1 >> 4) & 3 != 0 {
        return None;
    }
    let atest = b0 & 0x3000;
    let alpha = (b0 & 0x800 == 0 && atest != 0).then_some(if atest == 0x1000 { 0.01 } else { 0.5 });
    Some((alpha, b0 & 0xc000 == 0x4000))
}

struct Loader<'a> {
    zones: &'a [Zone],
    vfs: &'a iw3::iwd::Vfs,
    mats: Vec<Material>,
    by_id: HashMap<(usize, AssetId), Option<u32>>,
}

impl Loader<'_> {
    /// A material `,name` is defined in another zone.
    fn resolve(&self, zi: usize, id: AssetId) -> (usize, AssetId) {
        let Some(name) = self.zones[zi].material(id).and_then(|m| m.name.strip_prefix(',')) else { return (zi, id) };
        self.zones
            .iter()
            .enumerate()
            .filter(|&(z, _)| z != zi)
            .find_map(|(z, zone)| zone.find(name).filter(|&rid| zone.material(rid).is_some()).map(|rid| (z, rid)))
            .unwrap_or((zi, id))
    }

    fn material(&mut self, zi: usize, id: AssetId) -> Option<u32> {
        let (zi, id) = self.resolve(zi, id);
        if let Some(m) = self.by_id.get(&(zi, id)) {
            return *m;
        }
        let zone = &self.zones[zi];
        let m = classify(zone, id).map(|(alpha_test, two_sided)| {
            let mat = zone.material(id).unwrap();
            let image = mat
                .textures
                .iter()
                .find(|t| t.semantic == iw3::zone::TextureSemantic::Color)
                .or(mat.textures.first())
                .and_then(|t| t.image)
                .and_then(|i| zone.image(i))
                .map(|i| i.name.trim_start_matches(',').to_string());
            let texture = image.filter(|n| !n.starts_with('$')).and_then(|n| self.texture(&n));
            self.mats.push(Material { name: mat.name.clone(), alpha_test, two_sided, texture });
            self.mats.len() as u32 - 1
        });
        self.by_id.insert((zi, id), m);
        m
    }

    fn texture(&self, name: &str) -> Option<Texture> {
        let bytes = self.vfs.read(&format!("images/{name}.iwi")).ok()??;
        let iwi = iw3::iwi::Iwi::parse(&bytes).ok()?;
        let mut level = 0;
        while level + 1 < iwi.levels.len() && (iwi.width.max(iwi.height) as usize >> level) > MAX_TEXTURE {
            level += 1;
        }
        let (w, h, px) = dxt::decode(&iwi, level, 0)?;
        Some(Texture { w, h, texels: px })
    }
}

impl Scene {
    /// World surfaces and static models of `zones[0]` (`zones[1]` is
    /// common_mp, for models and materials kept there).
    pub fn build(zones: &[Zone], vfs: &iw3::iwd::Vfs) -> Scene {
        let world: &GfxWorld = zones[0].gfx_world().expect("zone has no GfxWorld");
        let mut loader = Loader { zones, vfs, mats: Vec::new(), by_id: HashMap::new() };
        let mut tris = Vec::new();
        let mut info = Vec::new();
        let push = |v: [Vec3A; 3], n: [Vec3A; 3], tri: TriInfo, tris: &mut Vec<Triangle>, info: &mut Vec<TriInfo>| {
            let mut nf = (v[1] - v[0]).cross(v[2] - v[0]);
            if nf.length_squared() < 1e-12 {
                return;
            }
            nf = nf.normalize();
            // CoD winds front faces clockwise; check against the normals.
            if nf.dot(n[0] + n[1] + n[2]) < 0.0 {
                nf = -nf;
            }
            tris.push(Triangle { v0: v[0], v1: v[1], v2: v[2] });
            info.push(TriInfo { nf, ..tri });
        };

        for s in &world.surfaces {
            let Some(mat) = s.material.and_then(|m| loader.material(0, m)) else { continue };
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
                let colour = vs.iter().fold(Vec3::ZERO, |a, v| {
                    let [r, g, b, _] = iw3::unpack::color(v.color);
                    a + Vec3::new(r, g, b).powf(2.2) / 3.0
                });
                let tri = TriInfo {
                    mat,
                    uv: vs.map(|v| Vec2::from(v.tex_coord)),
                    lm: vs.map(|v| Vec2::from(v.lmap_coord)),
                    lightmap: if (s.lightmap_index as usize) < world.lightmaps.len() { s.lightmap_index } else { NO_LIGHTMAP },
                    colour,
                    nf: Vec3A::ZERO,
                };
                push(vs.map(|v| Vec3A::from(v.xyz)), vs.map(|v| unit(v.normal)), tri, &mut tris, &mut info);
            }
        }
        let world_tris = tris.len();

        for sm in &world.static_models {
            let Some(model_id) = sm.model else { continue };
            let (zi, model_id) = resolve_xmodel(zones, 0, model_id);
            let Some(xm) = zones[zi].xmodel(model_id) else { continue };
            let Some(lod) = xm.lods.first() else { continue };
            let axis = sm.axis.map(Vec3A::from);
            let origin = Vec3A::from(sm.origin);
            let place = |p: [f32; 3]| origin + (axis[0] * p[0] + axis[1] * p[1] + axis[2] * p[2]) * sm.scale;
            let turn = |n: Vec3A| (axis[0] * n.x + axis[1] * n.y + axis[2] * n.z).normalize_or_zero();
            for surf in lod.surf_index as usize..(lod.surf_index + lod.num_surfs) as usize {
                let Some(mat) = xm.materials.get(surf).copied().flatten().and_then(|m| loader.material(zi, m)) else { continue };
                let Some(s) = xm.surfs.get(surf) else { continue };
                for t in &s.tris {
                    let Some(vs) = t.iter().map(|&i| s.verts.get(i as usize)).collect::<Option<Vec<_>>>() else { continue };
                    let tri = TriInfo {
                        mat,
                        uv: std::array::from_fn(|k| Vec2::from(iw3::unpack::tex_coords(vs[k].tex_coord))),
                        lm: [Vec2::ZERO; 3],
                        lightmap: NO_LIGHTMAP,
                        colour: Vec3::ONE,
                        nf: Vec3A::ZERO,
                    };
                    push(
                        std::array::from_fn(|k| place(vs[k].xyz)),
                        std::array::from_fn(|k| turn(unit(vs[k].normal))),
                        tri,
                        &mut tris,
                        &mut info,
                    );
                }
            }
        }
        log::info!(
            "bake scene: {world_tris} world and {} model triangles, {} materials ({} textured, {} alpha-tested)",
            tris.len() - world_tris,
            loader.mats.len(),
            loader.mats.iter().filter(|m| m.texture.is_some()).count(),
            loader.mats.iter().filter(|m| m.alpha_test.is_some()).count()
        );

        let bvh = obvhs::cwbvh::builder::build_cwbvh_from_tris(&tris, obvhs::BvhBuildParams::medium_build(), &mut Default::default());
        // Lay the triangles out in the BVH's order.
        let tris = bvh.primitive_indices.iter().map(|&i| tris[i as usize]).collect();
        let info = bvh.primitive_indices.iter().map(|&i| info[i as usize]).collect();
        Scene { tris, info, mats: loader.mats, bvh, saturation: 1.0 }
    }

    /// `t`, and barycentrics of v1 and v2, if `ray` hits triangle `i`.
    fn intersect(&self, ray: &Ray, i: usize) -> Option<(f32, f32, f32)> {
        let tri = &self.tris[i];
        let e1 = tri.v1 - tri.v0;
        let e2 = tri.v2 - tri.v0;
        let p = ray.direction.cross(e2);
        let det = e1.dot(p);
        if det.abs() < 1e-12 {
            return None;
        }
        let inv = 1.0 / det;
        let s = ray.origin - tri.v0;
        let u = s.dot(p) * inv;
        if !(0.0..=1.0).contains(&u) {
            return None;
        }
        let q = s.cross(e1);
        let v = ray.direction.dot(q) * inv;
        if v < 0.0 || u + v > 1.0 {
            return None;
        }
        let t = e2.dot(q) * inv;
        (t > ray.tmin && t < ray.tmax).then_some((t, u, v))
    }

    /// Whether the hit lets light through (an alpha-tested cut-out).
    fn see_through(&self, i: usize, u: f32, v: f32) -> bool {
        let info = &self.info[i];
        let mat = &self.mats[info.mat as usize];
        let (Some(threshold), Some(tex)) = (mat.alpha_test, &mat.texture) else { return false };
        let uv = info.uv[0] * (1.0 - u - v) + info.uv[1] * u + info.uv[2] * v;
        tex.sample(uv)[3] < threshold
    }

    fn test(&self, ray: &Ray, i: usize) -> f32 {
        match self.intersect(ray, i) {
            Some((t, u, v)) if !self.see_through(i, u, v) => t,
            _ => f32::INFINITY,
        }
    }

    pub fn closest(&self, origin: Vec3A, dir: Vec3A, tmax: f32) -> Option<Hit> {
        let ray = Ray::new(origin, dir, 0.0, tmax);
        let mut hit = RayHit::none();
        if !self.bvh.ray_traverse(ray, &mut hit, |r, i| self.test(r, i)) {
            return None;
        }
        let tri = hit.primitive_id as usize;
        let (t, u, v) = self.intersect(&Ray::new(origin, dir, 0.0, hit.t * 1.0001 + 1e-4), tri)?;
        let front = self.info[tri].nf.dot(dir) < 0.0 || self.mats[self.info[tri].mat as usize].two_sided;
        Some(Hit { tri, t, bary: Vec2::new(u, v), front })
    }

    pub fn occluded(&self, origin: Vec3A, dir: Vec3A, tmax: f32) -> bool {
        let ray = Ray::new(origin, dir, 0.0, tmax);
        !self.bvh.ray_traverse_miss(ray, |r, i| self.test(r, i))
    }

    /// The surface colour at a hit (linear albedo, clamped below 0.9 as no
    /// real paint reflects more).
    pub fn albedo(&self, hit: &Hit) -> Vec3 {
        let info = &self.info[hit.tri];
        let mat = &self.mats[info.mat as usize];
        let c = match &mat.texture {
            Some(tex) => {
                let (u, v) = (hit.bary.x, hit.bary.y);
                let uv = info.uv[0] * (1.0 - u - v) + info.uv[1] * u + info.uv[2] * v;
                let s = tex.sample(uv);
                Vec3::new(s[0], s[1], s[2])
            }
            None => Vec3::splat(0.35),
        };
        let c = c * info.colour;
        let grey = Vec3::splat(c.dot(Vec3::new(0.2126, 0.7152, 0.0722)));
        (grey + (c - grey) * self.saturation).clamp(Vec3::ZERO, Vec3::splat(0.9))
    }

    pub fn point(&self, hit: &Hit) -> Vec3A {
        let t = &self.tris[hit.tri];
        t.v0 * (1.0 - hit.bary.x - hit.bary.y) + t.v1 * hit.bary.x + t.v2 * hit.bary.y
    }

    pub fn lightmap_uv(&self, hit: &Hit) -> Vec2 {
        let info = &self.info[hit.tri];
        info.lm[0] * (1.0 - hit.bary.x - hit.bary.y) + info.lm[1] * hit.bary.x + info.lm[2] * hit.bary.y
    }
}

/// A model `,name` is defined in another zone.
pub fn resolve_xmodel(zones: &[Zone], zi: usize, id: AssetId) -> (usize, AssetId) {
    let Some(name) = zones[zi].xmodel(id).and_then(|m| m.name.strip_prefix(',')) else { return (zi, id) };
    zones
        .iter()
        .enumerate()
        .filter(|&(z, _)| z != zi)
        .find_map(|(z, zone)| zone.find(name).filter(|&rid| zone.xmodel(rid).is_some_and(|m| !m.lods.is_empty())).map(|rid| (z, rid)))
        .unwrap_or((zi, id))
}

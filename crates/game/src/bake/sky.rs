//! The map's sky as a light source: its cube map, looked up by direction
//! (CoD axes, as the skybox is drawn), with the painted sun clipped out
//! (the sun is a light of its own).

use super::dxt;
use glam33::{Vec3, Vec3A};

/// Each face is averaged down to this many texels a side.
const RES: usize = 32;

pub struct Sky {
    faces: [Vec<Vec3>; 6],
    /// Mean radiance over the upper hemisphere's cosine (what open, flat
    /// ground receives), before scaling.
    pub ground: Vec3,
}

impl Sky {
    pub fn load(vfs: &iw3::iwd::Vfs, name: &str) -> Option<Sky> {
        let bytes = vfs.read(&format!("images/{}.iwi", name.trim_start_matches(','))).ok()??;
        let iwi = iw3::iwi::Iwi::parse(&bytes).ok()?;
        if !iwi.is_cube() {
            return None;
        }
        let faces: Vec<Vec<Vec3>> = (0..6)
            .map(|f| {
                let (w, h, px) = dxt::decode(&iwi, 0, f)?;
                let mut out = vec![Vec3::ZERO; RES * RES];
                for y in 0..h {
                    for x in 0..w {
                        let p = px[y * w + x];
                        let c = Vec3::new(p[0] as f32, p[1] as f32, p[2] as f32) / 255.0;
                        out[(y * RES / h) * RES + x * RES / w] += c.powf(2.2);
                    }
                }
                let n = (w * h / (RES * RES)).max(1) as f32;
                Some(out.into_iter().map(|c| c / n).collect())
            })
            .collect::<Option<_>>()?;
        let mut faces: [Vec<Vec3>; 6] = faces.try_into().ok()?;
        // Clip the painted sun and its glare: nothing above the 98th
        // percentile's brightness.
        let mut lum: Vec<f32> = faces.iter().flatten().map(|c| luma(*c)).collect();
        lum.sort_by(f32::total_cmp);
        let cap = lum[lum.len() * 98 / 100].max(1e-4);
        for c in faces.iter_mut().flatten() {
            let l = luma(*c);
            if l > cap {
                *c *= cap / l;
            }
        }
        Some(Sky::from_faces(faces))
    }

    /// A sky of plain gradients (for times of day the map's own sky cube,
    /// painted for one time, doesn't show): `zenith` overhead, `horizon`
    /// at the horizon, `ground` below it (radiance, lightmap units).
    pub fn gradient(zenith: Vec3, horizon: Vec3, ground: Vec3) -> Sky {
        let faces: [Vec<Vec3>; 6] = std::array::from_fn(|f| {
            (0..RES * RES)
                .map(|i| {
                    let (sc, tc) = ((i % RES) as f32 + 0.5, (i / RES) as f32 + 0.5);
                    let (sc, tc) = (sc / RES as f32 * 2.0 - 1.0, tc / RES as f32 * 2.0 - 1.0);
                    // The inverse of `radiance`'s face mapping.
                    let d = match f {
                        0 => Vec3::new(1.0, -tc, -sc),
                        1 => Vec3::new(-1.0, -tc, sc),
                        2 => Vec3::new(sc, 1.0, tc),
                        3 => Vec3::new(sc, -1.0, -tc),
                        4 => Vec3::new(sc, -tc, 1.0),
                        _ => Vec3::new(-sc, -tc, -1.0),
                    }
                    .normalize();
                    if d.z >= 0.0 { horizon.lerp(zenith, d.z.sqrt()) } else { horizon.lerp(ground, (-d.z).sqrt().min(1.0)) }
                })
                .collect()
        });
        Sky::from_faces(faces)
    }

    fn from_faces(faces: [Vec<Vec3>; 6]) -> Sky {
        let mut sky = Sky { faces, ground: Vec3::ZERO };
        // Cosine-weighted mean over the upper hemisphere.
        let n = 64;
        let mut sum = Vec3::ZERO;
        for i in 0..n {
            for j in 0..n {
                let d = super::integrate::cosine_dir((i as f32 + 0.5) / n as f32, (j as f32 + 0.5) / n as f32);
                sum += sky.radiance(Vec3A::new(d.x, d.y, d.z));
            }
        }
        sky.ground = sum / (n * n) as f32;
        sky
    }


    /// Radiance towards `dir` (D3D cube face order and orientation).
    pub fn radiance(&self, d: Vec3A) -> Vec3 {
        let a = d.abs();
        let (face, sc, tc, ma) = if a.x >= a.y && a.x >= a.z {
            if d.x > 0.0 { (0, -d.z, -d.y, a.x) } else { (1, d.z, -d.y, a.x) }
        } else if a.y >= a.z {
            if d.y > 0.0 { (2, d.x, d.z, a.y) } else { (3, d.x, -d.z, a.y) }
        } else if d.z > 0.0 {
            (4, d.x, -d.y, a.z)
        } else {
            (5, -d.x, -d.y, a.z)
        };
        let u = ((sc / ma + 1.0) * 0.5 * RES as f32).clamp(0.0, RES as f32 - 1.0) as usize;
        let v = ((tc / ma + 1.0) * 0.5 * RES as f32).clamp(0.0, RES as f32 - 1.0) as usize;
        self.faces[face][v * RES + u]
    }
}

pub fn luma(c: Vec3) -> f32 {
    0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z
}

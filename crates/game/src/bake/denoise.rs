//! Smoothing the traced light and filling the atlas padding.

use super::integrate::Directional;
use super::sky::luma;
use super::texels::Atlas;

/// Edge-aware à-trous filtering of the gathered light within the atlas:
/// neighbours count only if they lie on the same surface (normal and
/// plane) and are lit alike, so shadow edges and corners stay sharp.
pub fn atrous(atlas: &Atlas, data: &mut [Directional], steps: &[usize]) {
    const H: [f32; 5] = [1.0 / 16.0, 1.0 / 4.0, 3.0 / 8.0, 1.0 / 4.0, 1.0 / 16.0];
    let (w, h) = (atlas.w, atlas.h);
    for &step in steps {
        let src = data.to_vec();
        use rayon::prelude::*;
        data.par_iter_mut().enumerate().for_each(|(i, out)| {
            let Some(t) = atlas.texels[i] else { return };
            let (x, y) = ((i % w) as i64, (i / w) as i64);
            let l0 = luma(src[i].e);
            let mut sum = Directional::default();
            let mut wsum = 0.0;
            for (ky, hy) in H.iter().enumerate() {
                for (kx, hx) in H.iter().enumerate() {
                    let (nx, ny) = (x + (kx as i64 - 2) * step as i64, y + (ky as i64 - 2) * step as i64);
                    if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                        continue;
                    }
                    let j = ny as usize * w + nx as usize;
                    let Some(u) = atlas.texels[j] else { continue };
                    let wn = t.n.dot(u.n).max(0.0).powi(16);
                    let plane = t.n.dot(u.pos - t.pos).abs() / t.size.max(1e-3);
                    let dist = u.pos.distance(t.pos) / (t.size.max(1e-3) * step as f32 * 3.0);
                    let wp = (-plane * plane - dist * dist).exp();
                    let l1 = luma(src[j].e);
                    let wl = (-(l0 - l1).abs() / (0.3 * l0.max(l1) + 1e-4)).exp();
                    let k = hx * hy * wn * wp * wl;
                    if k > 0.0 {
                        sum.lerp_add(&src[j], k);
                        wsum += k;
                    }
                }
            }
            if wsum > 0.0 {
                let mut r = Directional::default();
                r.lerp_add(&sum, 1.0 / wsum);
                *out = r;
            }
        });
    }
}

/// Spread covered texels' values into the padding around them (`rounds`
/// texels deep), so filtering at chart edges never reads black.
pub fn dilate<T: Copy + Default>(w: usize, h: usize, covered: &mut [bool], data: &mut [T], rounds: usize, mix: impl Fn(&[T]) -> T) {
    for _ in 0..rounds {
        let prev_cov = covered.to_vec();
        let prev = data.to_vec();
        let mut changed = false;
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                if prev_cov[i] {
                    continue;
                }
                let mut near = Vec::new();
                for (dx, dy) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1), (-1, -1), (1, 1), (-1, 1), (1, -1)] {
                    let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                    if nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h {
                        let j = ny as usize * w + nx as usize;
                        if prev_cov[j] {
                            near.push(prev[j]);
                        }
                    }
                }
                if !near.is_empty() {
                    data[i] = mix(&near);
                    covered[i] = true;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
}

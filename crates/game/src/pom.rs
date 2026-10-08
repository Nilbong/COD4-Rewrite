//! Surface depth for the showcase (`COD4RW_SHOWCASE=1`): height maps for
//! parallax occlusion mapping on world surfaces with a normal map
//! (`shaders/world.wgsl`'s `relief_uv`).
//!
//! CoD4's normal maps carry no height, so it's worked out from them when a
//! map loads: the slopes they store (DXT5, x in alpha, y in green, as
//! `iw3_slope` reads them) integrated into heights (the Poisson equation,
//! wrapping round as the textures tile), at up to 256 texels a side, then
//! stretched to 0..1. Each texture's is made once. `COD4RW_POM=0` makes
//! none, to compare.

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use std::collections::HashMap;
use std::sync::Mutex;

/// Height maps by normal map name (none where it couldn't be read).
static MADE: Mutex<Option<HashMap<String, Option<Handle<Image>>>>> = Mutex::new(None);

/// Largest side worked on (bigger ones are averaged down).
const MAX_SIDE: usize = 256;
/// Relaxation sweeps (red-black, over-relaxed).
const SWEEPS: usize = 160;

/// The height map for a normal map, made the first time it's asked for.
pub fn height_map(vfs: &iw3::iwd::Vfs, normal_name: &str, images: &mut Assets<Image>) -> Option<Handle<Image>> {
    // `COD4RW_POM=0`: none (to compare).
    if std::env::var("COD4RW_POM").is_ok_and(|v| v == "0") {
        return None;
    }
    let name = normal_name.trim_start_matches(',');
    let mut made = MADE.lock().ok()?;
    let made = made.get_or_insert_with(HashMap::new);
    if let Some(h) = made.get(name) {
        return h.clone();
    }
    let handle = slopes(vfs, name).map(|(w, h, gx, gy)| images.add(image(w, h, &integrate(w, h, &gx, &gy))));
    made.insert(name.to_owned(), handle.clone());
    handle
}

/// A normal map's slopes (height change per texel along x and y),
/// averaged down to at most `MAX_SIDE` a side.
fn slopes(vfs: &iw3::iwd::Vfs, name: &str) -> Option<(usize, usize, Vec<f32>, Vec<f32>)> {
    let data = vfs.read(&format!("images/{name}.iwi")).ok()??;
    let iwi = iw3::iwi::Iwi::parse(&data).ok()?;
    if iwi.format != iw3::iwi::Format::Dxt5 || iwi.width % 4 != 0 || iwi.height % 4 != 0 {
        return None;
    }
    let (w, h) = (iwi.width as usize, iwi.height as usize);
    let (mut sx, mut sy) = (vec![0f32; w * h], vec![0f32; w * h]);
    for (b, block) in iwi.levels[0].chunks_exact(16).enumerate().take((w / 4) * (h / 4)) {
        let (bx, by) = ((b % (w / 4)) * 4, (b / (w / 4)) * 4);
        let alpha = dxt5_alpha(block);
        let green = dxt_green(&block[8..]);
        for i in 0..16 {
            let p = (by + i / 4) * w + bx + i % 4;
            // The tangent-space normal is (x, y, 1): the surface falls
            // by x along u and y along v.
            sx[p] = -(alpha[i] as f32 / 255.0 * 4.08 - 2.08);
            sy[p] = -(green[i] as f32 / 255.0 * 4.064_516 - 2.064_516);
        }
    }
    let k = (w.max(h) / MAX_SIDE).max(1);
    let (dw, dh) = ((w / k).max(1), (h / k).max(1));
    let down = |s: &[f32]| -> Vec<f32> {
        let mut out = vec![0f32; dw * dh];
        for y in 0..dh {
            for x in 0..dw {
                let mut sum = 0.0;
                for yy in 0..k {
                    for xx in 0..k {
                        sum += s[(y * k + yy) * w + x * k + xx];
                    }
                }
                // Per (bigger) texel.
                out[y * dw + x] = sum / k as f32;
            }
        }
        out
    };
    Some((dw, dh, down(&sx), down(&sy)))
}

/// Heights whose differences best match the slopes, wrapping at the edges,
/// stretched to 0..1 (1 highest).
fn integrate(w: usize, h: usize, gx: &[f32], gy: &[f32]) -> Vec<f32> {
    let at = |x: isize, y: isize| (y.rem_euclid(h as isize) as usize) * w + x.rem_euclid(w as isize) as usize;
    // The slopes' divergence.
    let mut div = vec![0f32; w * h];
    for y in 0..h as isize {
        for x in 0..w as isize {
            div[at(x, y)] = (gx[at(x + 1, y)] - gx[at(x - 1, y)] + gy[at(x, y + 1)] - gy[at(x, y - 1)]) * 0.5;
        }
    }
    let mut hgt = vec![0f32; w * h];
    let omega = 1.9;
    for _ in 0..SWEEPS {
        for colour in 0..2 {
            for y in 0..h as isize {
                for x in 0..w as isize {
                    if (x + y) as usize % 2 != colour {
                        continue;
                    }
                    let i = at(x, y);
                    let target = (hgt[at(x - 1, y)] + hgt[at(x + 1, y)] + hgt[at(x, y - 1)] + hgt[at(x, y + 1)] - div[i]) * 0.25;
                    hgt[i] += omega * (target - hgt[i]);
                }
            }
        }
    }
    // Stretched between the 1st and 99th percentiles.
    let mut sorted = hgt.clone();
    sorted.sort_by(f32::total_cmp);
    let (lo, hi) = (sorted[sorted.len() / 100], sorted[sorted.len() * 99 / 100]);
    let span = (hi - lo).max(1e-6);
    hgt.iter().map(|v| ((v - lo) / span).clamp(0.0, 1.0)).collect()
}

/// A DXT5 block's 16 alpha values (as `crate::wardrobe`'s).
fn dxt5_alpha(block: &[u8]) -> [u8; 16] {
    let (a0, a1) = (block[0] as u32, block[1] as u32);
    let palette: [u32; 8] = if a0 > a1 {
        std::array::from_fn(|i| match i {
            0 => a0,
            1 => a1,
            _ => ((8 - i as u32) * a0 + (i as u32 - 1) * a1) / 7,
        })
    } else {
        std::array::from_fn(|i| match i {
            0 => a0,
            1 => a1,
            6 => 0,
            7 => 255,
            _ => ((6 - i as u32) * a0 + (i as u32 - 1) * a1) / 5,
        })
    };
    let bits = block[2..8].iter().rev().fold(0u64, |acc, &b| (acc << 8) | b as u64);
    std::array::from_fn(|i| palette[((bits >> (3 * i)) & 7) as usize] as u8)
}

/// A DXT colour block's 16 green values.
fn dxt_green(block: &[u8]) -> [u8; 16] {
    let g = |c: u16| (((c >> 5) & 0x3f) as u32 * 255 + 31) / 63;
    let (c0, c1) = (g(u16::from_le_bytes([block[0], block[1]])), g(u16::from_le_bytes([block[2], block[3]])));
    let palette = [c0, c1, (2 * c0 + c1) / 3, (c0 + 2 * c1) / 3];
    let bits = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
    std::array::from_fn(|i| palette[((bits >> (2 * i)) & 3) as usize] as u8)
}

/// An R8 image of the heights with its mip chain (box-filtered), so it
/// can be sampled at any distance without shimmering.
fn image(w: usize, h: usize, heights: &[f32]) -> Image {
    let mut levels: Vec<(usize, usize, Vec<f32>)> = vec![(w, h, heights.to_vec())];
    while let Some((lw, lh, last)) = levels.last().filter(|(lw, lh, _)| *lw > 1 || *lh > 1) {
        let (nw, nh) = ((lw / 2).max(1), (lh / 2).max(1));
        let (lw, lh) = (*lw, *lh);
        let mut next = vec![0f32; nw * nh];
        for y in 0..nh {
            for x in 0..nw {
                let mut sum = 0.0;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    sum += last[((y * 2 + dy).min(lh - 1)) * lw + (x * 2 + dx).min(lw - 1)];
                }
                next[y * nw + x] = sum * 0.25;
            }
        }
        levels.push((nw, nh, next));
    }
    let data: Vec<u8> = levels.iter().flat_map(|(_, _, l)| l.iter().map(|v| (v * 255.0).round() as u8)).collect();
    let mut image = Image::new(
        Extent3d { width: w as u32, height: h as u32, depth_or_array_layers: 1 },
        TextureDimension::D2,
        data,
        TextureFormat::R8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_descriptor.mip_level_count = levels.len() as u32;
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        ..default()
    });
    image
}

#[cfg(test)]
mod tests {
    #[test]
    fn integrates_a_ramp_bump() {
        // A bump: slopes of a cosine hill along x (wrapping), flat along y.
        let (w, h) = (32usize, 4usize);
        let height = |x: usize| (x as f32 / w as f32 * std::f32::consts::TAU).cos();
        let gx: Vec<f32> = (0..w * h).map(|i| {
            let x = i % w;
            (height((x + 1) % w) - height((x + w - 1) % w)) * 0.5
        }).collect();
        let gy = vec![0f32; w * h];
        let out = super::integrate(w, h, &gx, &gy);
        // Highest at x = 0, lowest at the middle.
        assert!(out[0] > 0.9 && out[w / 2] < 0.1, "{} {}", out[0], out[w / 2]);
    }
}

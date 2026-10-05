//! Map lightmaps and reflection probes: CoD4's baked indirect light (sky,
//! bounce light, lamps) and environment reflections, under our real-time sun.
//!
//! Each `lightmapN_secondary` image is two stacked BGRA halves, A on top and
//! B below. IW3's world shaders (`lm_*_sm3`) light a pixel with
//! `A * N.z + B * saturate(dot(N, L))`, where `N` is the tangent-space normal
//! and `L` a tangent-space light direction whose x and y are packed in the two
//! halves' alpha. `shaders/world.wgsl` evaluates that per pixel, so the
//! images are uploaded as they are. The `primary` image is baked sun
//! visibility, which the real-time sun and its shadow maps replace.

use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::render_resource::{
    Extent3d, TextureDimension, TextureFormat, TextureViewDescriptor, TextureViewDimension,
};
use iw3::zone::{GfxWorld, Zone};

/// `D3DFMT_A8R8G8B8`.
const FORMAT_ARGB8: u32 = 21;

/// Scale from lightmap values (after IW3's gamma-space lighting is
/// linearised) to Bevy light units, tuned by eye against the sun's
/// illuminance on mp_killhouse and mp_crash.
pub const LIGHTMAP_EXPOSURE: f32 = 3_000.0;

/// Upload every reflection probe of `world`, indexed like
/// `GfxSurface::reflection_probe_index`.
///
/// Probes are small A8R8G8B8 cube maps, stored face by face with each
/// face's full mip chain. IW3 reads them as `rgb * a` in gamma space; alpha
/// is an overbright intensity scale that peaks around 0.5 (see
/// `shaders/world.wgsl`).
pub fn load_probes(zone: &Zone, world: &GfxWorld, images: &mut Assets<Image>) -> Vec<Option<Handle<Image>>> {
    world
        .reflection_probes
        .iter()
        .enumerate()
        .map(|(i, probe)| {
            let img = probe.image.and_then(|id| zone.image(id))?;
            let def = img.load_def.as_ref().filter(|d| d.format == FORMAT_ARGB8)?;
            let size = img.width as u32;
            let mips = size.max(1).ilog2() + 1;
            let face_bytes: usize = (0..mips).map(|m| ((size >> m).max(1) as usize).pow(2) * 4).sum();
            if def.data.len() < face_bytes * 6 {
                warn!("reflection probe {i}: {} bytes, expected {}", def.data.len(), face_bytes * 6);
                return None;
            }
            let mut image = Image::new_uninit(
                Extent3d { width: size, height: size, depth_or_array_layers: 6 },
                TextureDimension::D2,
                TextureFormat::Bgra8Unorm,
                RenderAssetUsages::RENDER_WORLD,
            );
            // Face-major ("layer-major"), Bevy's default data order.
            image.data = Some(def.data[..face_bytes * 6].to_vec());
            image.texture_descriptor.mip_level_count = mips;
            image.texture_view_descriptor =
                Some(TextureViewDescriptor { dimension: Some(TextureViewDimension::Cube), ..default() });
            image.sampler = ImageSampler::linear();
            Some(images.add(image))
        })
        .collect()
}

/// Upload every secondary lightmap of `world`, indexed like
/// `GfxSurface::lightmap_index`.
pub fn load(zone: &Zone, world: &GfxWorld, images: &mut Assets<Image>) -> Vec<Option<Handle<Image>>> {
    world
        .lightmaps
        .iter()
        .enumerate()
        .map(|(i, pair)| {
            let img = pair.secondary.and_then(|id| zone.image(id))?;
            let def = img.load_def.as_ref().filter(|d| d.format == FORMAT_ARGB8)?;
            let (w, h) = (img.width as u32, img.height as u32);
            let size = (w * h * 4) as usize;
            if def.data.len() < size {
                warn!("lightmap {i}: {} bytes, expected {size}", def.data.len());
                return None;
            }
            // A8R8G8B8 is B, G, R, A in memory. The halves are combined in
            // gamma space by the shader, so this is not an sRGB format.
            let mut image = Image::new(
                Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                TextureDimension::D2,
                def.data[..size].to_vec(),
                TextureFormat::Bgra8Unorm,
                RenderAssetUsages::RENDER_WORLD,
            );
            image.sampler = ImageSampler::linear();
            Some(images.add(image))
        })
        .collect()
}

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

/// Map lighting: CoD4's own lightmaps and grid (the default), or the
/// re-baked ones (`crate::bake`) where a bake of the map is cached. Set from
/// the settings (`r_maplighting`); `COD4RW_MAPLIGHTING=rebaked|original`
/// overrides. Takes effect from the next map load.
static REBAKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_rebaked(on: bool) {
    REBAKED.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn rebaked_wanted() -> bool {
    // Ray-traced lighting reads CoD4's (`shaders/world_deferred.wgsl`).
    if crate::ui::lighting() != crate::rtgi::Lighting::Baked {
        return false;
    }
    match std::env::var("COD4RW_MAPLIGHTING") {
        Ok(v) => v.eq_ignore_ascii_case("rebaked"),
        Err(_) => REBAKED.load(std::sync::atomic::Ordering::Relaxed),
    }
}

/// The re-baked lighting in use for a map: lightmaps indexed like
/// `GfxWorld::lightmaps` (linear half floats, see `crate::bake::cache`)
/// and the light grid's cubes.
pub struct Rebaked {
    pub lightmaps: Vec<Option<Handle<Image>>>,
    /// Grid point -> light on surfaces facing CoD +x, -x, +y, -y, +z, -z.
    pub grid: std::collections::HashMap<[u32; 3], [[f32; 3]; 6]>,
}

/// The map's cached bake, if wanted and made from this install's zone.
///
/// With the showcase on (`crate::tod_light`), the map's time-of-day
/// keyframes instead, blended for the clock, when it has them.
pub fn load_rebaked(commands: &mut Commands, map: &str, zone_path: &std::path::Path, images: &mut Assets<Image>) -> Option<Rebaked> {
    if crate::atmos::climate::showcase() && crate::ui::lighting() == crate::rtgi::Lighting::Baked {
        // The clock's start (`COD4RW_TOD`); without it, noon until the
        // clock is set to the map's hour a frame or two in.
        let hour = std::env::var("COD4RW_TOD").ok().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(12.0).rem_euclid(24.0);
        if let Some((rebaked, tod)) = crate::tod_light::load(map, zone_path, images, hour) {
            commands.insert_resource(tod);
            return Some(rebaked);
        }
    }
    if !rebaked_wanted() {
        return None;
    }
    let t = std::time::Instant::now();
    let Some(baked) = crate::bake::cache::load(map, crate::bake::cache::source_stamp(zone_path)) else {
        info!("rebaked lighting: no bake of {map} (COD4RW_BAKE={map} makes one)");
        return None;
    };
    let lightmaps = baked
        .atlases
        .into_iter()
        .map(|a| {
            let a = a?;
            let bytes: Vec<u8> = a.data.iter().flat_map(|v| v.to_le_bytes()).collect();
            let mut image = Image::new(
                Extent3d { width: a.w, height: a.h * 2, depth_or_array_layers: 1 },
                TextureDimension::D2,
                bytes,
                TextureFormat::Rgba16Float,
                RenderAssetUsages::RENDER_WORLD,
            );
            image.sampler = ImageSampler::linear();
            Some(images.add(image))
        })
        .collect();
    info!("rebaked lighting: {map} loaded in {:?}", t.elapsed());
    Some(Rebaked { lightmaps, grid: baked.grid.into_iter().collect() })
}

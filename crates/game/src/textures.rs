//! CoD4 images -> Bevy images.
//!
//! DXT textures are uploaded as BC1/2/3 with their full mip chain; the GPU
//! decodes them, which keeps memory close to the original game.

use bevy::asset::RenderAssetUsages;
use bevy::image::{Image, ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_resource::{
    Extent3d, TextureDataOrder, TextureDimension, TextureFormat, TextureViewDescriptor, TextureViewDimension,
};
use iw3::iwd::Vfs;
use iw3::iwi::{Format, Iwi};
use std::collections::HashMap;

#[derive(Default)]
pub struct TextureCache {
    map: HashMap<(String, bool), Option<Handle<Image>>>,
}

impl TextureCache {
    /// Load `images/<name>.iwi`. `srgb` should be true for colour maps only.
    pub fn get(&mut self, name: &str, srgb: bool, vfs: &Vfs, images: &mut Assets<Image>) -> Option<Handle<Image>> {
        // A leading comma marks a reference to an asset defined in a shared zone.
        let name = name.trim_start_matches(',');
        let key = (name.to_owned(), srgb);
        if let Some(h) = self.map.get(&key) {
            return h.clone();
        }
        let handle = load(name, srgb, vfs).map(|img| images.add(img));
        if handle.is_none() {
            debug!("texture {name} not found");
        }
        self.map.insert(key, handle.clone());
        handle
    }
}

impl TextureCache {
    /// Load `images/<name>.iwi` as a cube map (sRGB colour), e.g. a sky.
    pub fn get_cube(&mut self, name: &str, vfs: &Vfs, images: &mut Assets<Image>) -> Option<Handle<Image>> {
        let name = name.trim_start_matches(',');
        let bytes = vfs.read(&format!("images/{name}.iwi")).ok()??;
        let iwi = Iwi::parse(&bytes).inspect_err(|e| warn!("bad iwi {name}: {e}")).ok()?;
        let image = cube_image(&iwi)?;
        Some(images.add(image))
    }
}

/// A cube iwi (six faces per mip level) as a Bevy cube texture.
fn cube_image(iwi: &Iwi) -> Option<Image> {
    if !iwi.is_cube() || !iwi.format.is_compressed() {
        return None;
    }
    let format = match iwi.format {
        Format::Dxt1 => TextureFormat::Bc1RgbaUnormSrgb,
        Format::Dxt3 => TextureFormat::Bc2RgbaUnormSrgb,
        _ => TextureFormat::Bc3RgbaUnormSrgb,
    };
    let size = Extent3d { width: iwi.width, height: iwi.height, depth_or_array_layers: 6 };
    let mut image = Image::new_uninit(size, TextureDimension::D2, format, RenderAssetUsages::RENDER_WORLD);
    // iwi levels are mip-major: each level holds the six faces in D3D order.
    image.data = Some(iwi.levels.concat());
    image.data_order = TextureDataOrder::MipMajor;
    image.texture_descriptor.mip_level_count = iwi.levels.len() as u32;
    image.texture_view_descriptor =
        Some(TextureViewDescriptor { dimension: Some(TextureViewDimension::Cube), ..default() });
    image.sampler = ImageSampler::linear();
    Some(image)
}

fn load(name: &str, srgb: bool, vfs: &Vfs) -> Option<Image> {
    if let Some(code) = name.strip_prefix('$') {
        return code_image(code);
    }
    let bytes = vfs.read(&format!("images/{name}.iwi")).ok()??;
    let iwi = match Iwi::parse(&bytes) {
        Ok(i) => i,
        Err(e) => {
            warn!("bad iwi {name}: {e}");
            return None;
        }
    };
    if iwi.is_cube() || iwi.depth > 1 {
        return None;
    }
    to_image(&iwi, srgb)
}

/// The settings' texture filtering: 0 bilinear, 1 trilinear, else the
/// anisotropy (from the next map's textures).
static ANISOTROPY: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(16);

pub fn set_anisotropy(v: u16) {
    ANISOTROPY.store(v, std::sync::atomic::Ordering::Relaxed);
}

pub fn to_image(iwi: &Iwi, srgb: bool) -> Option<Image> {
    let size = Extent3d { width: iwi.width, height: iwi.height, depth_or_array_layers: 1 };
    let (format, data, mips) = if iwi.format.is_compressed() {
        let format = match (iwi.format, srgb) {
            (Format::Dxt1, true) => TextureFormat::Bc1RgbaUnormSrgb,
            (Format::Dxt1, false) => TextureFormat::Bc1RgbaUnorm,
            (Format::Dxt3, true) => TextureFormat::Bc2RgbaUnormSrgb,
            (Format::Dxt3, false) => TextureFormat::Bc2RgbaUnorm,
            (Format::Dxt5, true) => TextureFormat::Bc3RgbaUnormSrgb,
            _ => TextureFormat::Bc3RgbaUnorm,
        };
        // Block-compressed textures need a base size that is a multiple of 4.
        if iwi.width % 4 != 0 || iwi.height % 4 != 0 {
            return None;
        }
        let data: Vec<u8> = iwi.levels.concat();
        (format, data, iwi.levels.len() as u32)
    } else {
        let format = if srgb { TextureFormat::Rgba8UnormSrgb } else { TextureFormat::Rgba8Unorm };
        (format, iwi.to_rgba8(0)?, 1)
    };
    let mut image = Image::new_uninit(size, TextureDimension::D2, format, RenderAssetUsages::RENDER_WORLD);
    image.data = Some(data);
    image.texture_descriptor.mip_level_count = mips;
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: match ANISOTROPY.load(std::sync::atomic::Ordering::Relaxed) {
            0 => ImageFilterMode::Nearest,
            _ => ImageFilterMode::Linear,
        },
        anisotropy_clamp: ANISOTROPY.load(std::sync::atomic::Ordering::Relaxed).max(1),
        ..default()
    });
    Some(image)
}

/// The engine's built-in `$name` images.
fn code_image(name: &str) -> Option<Image> {
    let rgba = match name {
        "white" => [255, 255, 255, 255],
        "black" => [0, 0, 0, 255],
        "identitynormalmap" => [127, 127, 255, 255],
        _ => return None,
    };
    Some(Image::new(
        Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        TextureDimension::D2,
        rgba.to_vec(),
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    ))
}

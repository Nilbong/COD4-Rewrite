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
        self.get_cube_mirrored(name, false, vfs, images)
    }

    /// [`Self::get_cube`], mirrored north to south (`mirror_y`: CoD's y
    /// flipped) when asked: MW2's skies are drawn that way round (their
    /// painted sun stood mirrored from the real one).
    pub fn get_cube_mirrored(&mut self, name: &str, mirror_y: bool, vfs: &Vfs, images: &mut Assets<Image>) -> Option<Handle<Image>> {
        let name = name.trim_start_matches(',');
        let bytes = vfs.read(&format!("images/{name}.iwi")).ok()??;
        let mut iwi = Iwi::parse(&bytes).inspect_err(|e| warn!("bad iwi {name}: {e}")).ok()?;
        if mirror_y {
            mirror_cube_y(&mut iwi);
        }
        let image = cube_image(&iwi)?;
        Some(images.add(image))
    }
}

/// Mirror a block-compressed cube iwi across CoD's y. The skybox shows the
/// cube turned so its faces' y is CoD's -y: every face flips top to bottom,
/// and the +y and -y faces trade places.
fn mirror_cube_y(iwi: &mut Iwi) {
    let block = match iwi.format {
        Format::Dxt1 => 8,
        Format::Dxt3 | Format::Dxt5 => 16,
        _ => return,
    };
    for (level, data) in iwi.levels.iter_mut().enumerate() {
        let size = (iwi.width >> level).max(1) as usize;
        let blocks = size.div_ceil(4);
        let face = blocks * blocks * block;
        if data.len() < face * 6 {
            continue;
        }
        let mut faces: Vec<Vec<u8>> = data.chunks_exact(face).take(6).map(|f| flip_blocks_vertically(f, blocks, block, iwi.format)).collect();
        faces.swap(2, 3);
        data[..face * 6].copy_from_slice(&faces.concat());
    }
}

/// One face of 4x4 blocks turned upside down: rows of blocks reversed, and
/// each block's own rows.
fn flip_blocks_vertically(face: &[u8], blocks: usize, block: usize, format: Format) -> Vec<u8> {
    let row = blocks * block;
    let mut out = Vec::with_capacity(face.len());
    for r in (0..blocks).rev() {
        for b in face[r * row..(r + 1) * row].chunks_exact(block) {
            let mut b = b.to_vec();
            let colour = block - 8;
            // Colour: 4 bytes of endpoints, then a byte of indices per row.
            b[colour + 4..colour + 8].reverse();
            match format {
                // Explicit alpha: two bytes per row.
                Format::Dxt3 => {
                    let rows: Vec<[u8; 2]> = b[..8].chunks_exact(2).rev().map(|c| [c[0], c[1]]).collect();
                    b[..8].copy_from_slice(&rows.concat());
                }
                // Interpolated alpha: two endpoints, then 12 bits per row.
                Format::Dxt5 => {
                    let mut bits = 0u64;
                    for (i, v) in b[2..8].iter().enumerate() {
                        bits |= (*v as u64) << (8 * i);
                    }
                    let mut flipped = 0u64;
                    for r in 0..4 {
                        flipped |= ((bits >> (12 * r)) & 0xfff) << (12 * (3 - r));
                    }
                    for i in 0..6 {
                        b[2 + i] = (flipped >> (8 * i)) as u8;
                    }
                }
                _ => {}
            }
            out.extend_from_slice(&b);
        }
    }
    out
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

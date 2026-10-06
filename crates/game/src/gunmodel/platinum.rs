//! Mastery finishes on CoD4's original gun surfaces. Native camo materials
//! define the coverage; pistols and the RPG use explicit body materials.

use super::{CamoDetail, CamoMaterial};
use crate::content::Content;
use bevy::asset::RenderAssetUsages;
use bevy::image::{
    CompressedImageFormats, ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor, ImageType,
};
use bevy::prelude::*;
use std::collections::HashSet;

pub(crate) const CAMO: usize = 200;
pub(crate) const DIAMOND: usize = 201;
const TEXTURE: &[u8] = include_bytes!("../../assets/camos/ak47-platinum.png");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Finish {
    Gold,
    Platinum,
    Diamond,
}

impl Finish {
    pub(super) fn selected(weapon: &str, camo: usize) -> Option<Self> {
        if !matches!(
            weapon,
            "ak47"
                | "m14"
                | "mp44"
                | "g3"
                | "g36c"
                | "m16"
                | "m4"
                | "mp5"
                | "skorpion"
                | "uzi"
                | "ak74u"
                | "p90"
                | "rpd"
                | "saw"
                | "m60e4"
                | "m1014"
                | "winchester1200"
                | "dragunov"
                | "m40a3"
                | "barrett"
                | "remington700"
                | "m21"
                | "beretta"
                | "colt45"
                | "usp"
                | "deserteagle"
                | "deserteaglegold"
                | "rpg"
        ) {
            return None;
        }
        match camo {
            6 => Some(Self::Gold),
            CAMO => Some(Self::Platinum),
            // A test feature (`--features diamond`); otherwise a plain gun.
            DIAMOND if cfg!(feature = "diamond") => Some(Self::Diamond),
            _ => None,
        }
    }

    pub(super) fn native_gold(weapon: &str) -> bool {
        matches!(weapon, "ak47" | "uzi" | "dragunov" | "m1014" | "m60e4" | "deserteaglegold")
    }

    pub(super) fn model_camo(self, weapon: &str) -> usize {
        if self == Self::Gold && Self::native_gold(weapon) { 6 } else { 1 }
    }

    pub(super) fn cache_key(self) -> &'static str {
        match self {
            Self::Gold => "cod4rw/gold-v2",
            Self::Platinum => "cod4rw/platinum-v2",
            Self::Diamond => "cod4rw/diamond-v1",
        }
    }
}

/// Resolve by material name rather than surface number: both the original
/// first-person and world models use it, while sights and misc parts don't.
pub(super) fn surfaces(
    content: &mut Content,
    model: &str,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
) -> HashSet<AssetId<StandardMaterial>> {
    let Some((zi, id)) = content.find(model) else { return HashSet::new() };
    let zone = &content.zones[zi];
    let Some(xm) = zone.xmodel(id) else { return HashSet::new() };
    let Some(lod) = xm.lods.first() else { return HashSet::new() };
    let ids: Vec<_> = (lod.surf_index as usize..(lod.surf_index + lod.num_surfs) as usize)
        .filter_map(|i| xm.materials.get(i).copied().flatten())
        .filter(|&i| zone.material(i).is_some_and(|m| {
            let name = m.name.trim_start_matches(',');
            if !name.starts_with("mc/mtl_weapon_") || [
                "misc_nocamo", "scope", "lens", "reticle", "reflex", "acog", "ironsight",
                "sights", "bullet", "suppressor", "foregrip", "peq", "trinium",
            ].iter().any(|part| name.contains(part)) {
                return false;
            }
            m.textures.iter().any(|t| t.name_hash == super::DETAIL_MAP)
                || matches!(name, "mc/mtl_weapon_beretta" | "mc/mtl_weapon_usp" | "mc/mtl_weapon_colt1911"
                    | "mc/mtl_weapon_desert_eagle_silver" | "mc/mtl_weapon_desert_eagle_gold" | "mc/mtl_weapon_rpg7")
        }))
        .collect();
    ids.into_iter().filter_map(|id| content.material(zi, id, materials, images)).map(|m| m.handle.id()).collect()
}

fn image() -> Result<Image, bevy::image::TextureError> {
    Image::from_buffer(
        TEXTURE,
        ImageType::Extension("png"),
        CompressedImageFormats::NONE,
        true,
        ImageSampler::Descriptor(ImageSamplerDescriptor {
            address_mode_u: ImageAddressMode::Repeat,
            address_mode_v: ImageAddressMode::Repeat,
            mag_filter: ImageFilterMode::Linear,
            min_filter: ImageFilterMode::Linear,
            ..default()
        }),
        RenderAssetUsages::RENDER_WORLD,
    )
}

pub(crate) fn texture(images: &mut Assets<Image>) -> Option<Handle<Image>> {
    match image() {
        Ok(image) => Some(images.add(image)),
        Err(e) => {
            error!("Mastery finish texture could not be decoded: {e}");
            None
        }
    }
}

/// A small menu swatch matching the real octagonal crowns on gold backing.
/// The in-game studs are meshes; this icon is generated once for the UI.
pub(crate) fn diamond_texture(images: &mut Assets<Image>) -> Option<Handle<Image>> {
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
    const SIZE: u32 = 128;
    let mut pixels = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let p = Vec2::new((x % 16) as f32 - 7.5, (y % 16) as f32 - 7.5);
            let r = p.length();
            let color = if r < 6.0 {
                let a = p.y.atan2(p.x);
                let facet = ((a / std::f32::consts::FRAC_PI_4).floor() as i32).rem_euclid(8);
                let value: u8 = if r < 2.7 { 231 } else { [188, 242, 219, 161, 116, 174, 207, 255][facet as usize] };
                [value, value, value.saturating_add(5), 255]
            } else {
                [143, 99, 31, 255]
            };
            pixels.extend_from_slice(&color);
        }
    }
    Some(images.add(Image::new(
        Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
        TextureDimension::D2,
        pixels,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    )))
}

/// Keep the original UVs, normal map, engraved details and specular wear.
/// Mode 3 is platinum, 4 is gold and 5 is diamond on gold backing.
/// A cloned material keeps other camos, hands and optics unchanged.
pub(super) fn material(
    mut base: StandardMaterial,
    detail: Handle<Image>,
    specular: Option<Handle<Image>>,
    finish: Finish,
) -> CamoMaterial {
    let color = base.base_color_texture.clone();
    base.metallic = 1.0;
    base.perceptual_roughness = if finish == Finish::Platinum { 0.26 } else { 0.23 };
    base.base_color = Color::WHITE;
    if finish == Finish::Diamond {
        // Bevy specialises its lighting shader from the CPU material.
        base.clearcoat = 1.0;
        base.clearcoat_perceptual_roughness = 0.05;
    }
    let mode = match finish {
        Finish::Platinum => 3.0,
        Finish::Gold => 4.0,
        Finish::Diamond => 5.0,
    };
    CamoMaterial {
        base,
        extension: CamoDetail {
            scale: Vec4::new(3.0, 3.0, mode, 1.0),
            detail: Some(detail),
            specular,
            probe: None,
            env: Vec4::new(3.0, 4.5, 5.0, 0.625),
            shine: Vec4::new(
                (std::env::var_os("COD4RW_NOSHINE").is_none()) as u32 as f32,
                0.0,
                0.0,
                crate::lightmaps::LIGHTMAP_EXPOSURE,
            ),
            look: Vec4::new(1.0, 1.0, 0.0, 1.4),
            color,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mastery_finishes_cover_cod4_guns_and_exclude_other_games() {
        for weapon in ["ak47", "m4", "ak74u", "beretta", "deserteaglegold", "rpg"] {
            assert_eq!(Finish::selected(weapon, CAMO), Some(Finish::Platinum));
            assert_eq!(Finish::selected(weapon, DIAMOND), cfg!(feature = "diamond").then_some(Finish::Diamond));
            assert_eq!(Finish::selected(weapon, 6), Some(Finish::Gold));
        }
        for weapon in ["t5_ak47", "t4_thompson", "knife", "frag_grenade"] {
            assert_eq!(Finish::selected(weapon, CAMO), None);
            assert_eq!(Finish::selected(weapon, DIAMOND), None);
        }
        for camo in [0, 1, 2, 3, 4, 5, 7, 100, 115] {
            assert_eq!(Finish::selected("ak47", camo), None);
        }
    }

    #[test]
    fn embedded_platinum_texture_decodes_as_repeating_srgb() {
        let texture = image().expect("embedded generated PNG must decode");
        assert!(texture.width() >= 1024 && texture.height() >= 1024);
        assert_eq!(texture.width(), texture.height());
        assert!(texture.texture_descriptor.format.is_srgb());
        let ImageSampler::Descriptor(sampler) = texture.sampler else { panic!("repeat sampler required") };
        assert_eq!(sampler.address_mode_u, ImageAddressMode::Repeat);
        assert_eq!(sampler.address_mode_v, ImageAddressMode::Repeat);
    }
}

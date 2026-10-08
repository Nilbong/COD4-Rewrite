//! Custom camos ([`crate::custom_camos`]) on any game's gun: which surfaces
//! take the paint, and their material (mode 6 of `ui/camo.wgsl`).

use super::{CamoDetail, CamoMaterial, DEFAULT_ENV, GunLook};
use crate::content::Content;
use crate::custom_camos::CustomCamo;
use bevy::prelude::*;
use std::collections::HashMap;

/// The paint's tiling over a surface's UVs when it has no camo of its own
/// to go by (mastery finishes use the same).
const TILING: Vec2 = Vec2::splat(3.0);

/// Parts never painted: optics, the dot, bullets, attachments' own.
const UNPAINTED: [&str; 14] = [
    "misc_nocamo", "scope", "lens", "reticle", "reflex", "acog", "ironsight", "sights", "bullet", "suppressor",
    "foregrip", "peq", "trinium", "glass",
];

/// The camo model to build a custom-camo gun from: CoD4's woodland
/// variant, whose surfaces mark where camo goes (as the mastery finishes).
pub(super) fn model_camo(bo1: bool, waw: bool) -> usize {
    if bo1 || waw { 0 } else { 1 }
}

/// The surfaces of `model` that take the paint: its standard material ->
/// (tiling, masked by the colour map's alpha as Black Ops' camos are).
pub(super) fn surfaces(
    content: &mut Content,
    model: &str,
    weapon: &str,
    game: (bool, bool),
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
) -> HashMap<AssetId<StandardMaterial>, (Vec2, bool)> {
    match game {
        (true, _) => {
            // Black Ops: the surfaces its own camos go on.
            let stat = crate::bo1::camo_stat(1);
            super::bo1_camo_surfaces(content, model, weapon, stat, materials, images)
                .into_iter()
                .map(|(id, (_, s))| (id, (Vec2::new(s.x, s.y), true)))
                .collect()
        }
        (_, true) => named(content, model, materials, images),
        _ => {
            let tiling = super::camo_surfaces(content, model, materials, images);
            super::platinum::surfaces(content, model, materials, images)
                .into_iter()
                .map(|id| (id, (tiling.get(&id).map_or(TILING, |(_, s)| Vec2::new(s.x, s.y)), false)))
                .collect()
        }
    }
}

/// World at War has no camos to go by: its gun materials, by name.
fn named(
    content: &mut Content,
    model: &str,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
) -> HashMap<AssetId<StandardMaterial>, (Vec2, bool)> {
    let Some((zi, id)) = content.find(model) else { return HashMap::new() };
    let zone = &content.zones[zi];
    let Some(xm) = zone.xmodel(id) else { return HashMap::new() };
    let Some(lod) = xm.lods.first() else { return HashMap::new() };
    let ids: Vec<_> = (lod.surf_index as usize..(lod.surf_index + lod.num_surfs) as usize)
        .filter_map(|i| xm.materials.get(i).copied().flatten())
        .filter(|&i| {
            zone.material(i).is_some_and(|m| {
                let name = m.name.trim_start_matches(',').to_ascii_lowercase();
                name.contains("weapon") && !UNPAINTED.iter().any(|part| name.contains(part))
            })
        })
        .collect();
    ids.into_iter()
        .filter_map(|id| content.material(zi, id, materials, images))
        .map(|m| (m.handle.id(), (TILING, false)))
        .collect()
}

/// A painted surface's material: `base` (the surface's own, for its colour
/// map's wear and its normal map) with `paint` over it.
pub(super) fn material(
    mut base: StandardMaterial,
    def: &CustomCamo,
    paint: Handle<Image>,
    specular: Option<(Handle<Image>, Vec4)>,
    (tiling, masked): (Vec2, bool),
) -> CamoMaterial {
    let color = base.base_color_texture.clone();
    let (turn, gloss) = def.transform();
    base.metallic = 0.0;
    base.perceptual_roughness = 0.85 - 0.55 * gloss;
    let on = specular.is_some() && std::env::var_os("COD4RW_NOSHINE").is_none();
    let (specular, env) = specular.map_or((None, DEFAULT_ENV), |(s, e)| (Some(s), e));
    let sheen = GunLook::get().uniform().w;
    CamoMaterial {
        base,
        extension: CamoDetail {
            // Mode 6 paints; 7 only where the colour map's alpha says.
            scale: Vec4::new(tiling.x, tiling.y, if masked { 7.0 } else { 6.0 }, 1.0),
            detail: Some(paint),
            specular,
            probe: None,
            env,
            shine: Vec4::new(on as u32 as f32, 0.0, 0.0, crate::lightmaps::LIGHTMAP_EXPOSURE),
            // x, y: the paint's turn and scale; z: its gloss.
            look: Vec4::new(turn.x, turn.y, gloss, sheen),
            color,
        },
    }
}

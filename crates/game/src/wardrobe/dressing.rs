//! Characters' models as worn: the shared model materials leave out normal
//! maps, so characters get theirs here. CoD4's gain their creases and folds,
//! and Black Ops' (whose colour maps carry little baked detail) stop looking
//! flat. A Black Ops eye's cornea layer is left out: a shader makes it
//! see-through, so drawn plainly it covers the iris.

use crate::content::{Content, PreparedBone, PreparedModel};
use bevy::asset::RenderAssetUsages;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use iw3::iwd::Vfs;
use std::collections::HashMap;
use std::sync::Arc;

/// One content's models, dressed. Keep one per [`Content`]: names are only
/// unique within one.
#[derive(Default)]
pub struct Dressing {
    /// Normal maps by image name.
    normals: HashMap<String, Option<Handle<Image>>>,
    models: HashMap<String, Arc<PreparedModel>>,
}

impl Dressing {
    /// `content`'s model `name`, normal-mapped and without any cornea.
    pub fn model(
        &mut self,
        content: &mut Content,
        name: &str,
        meshes: &mut Assets<Mesh>,
        materials: &mut Assets<StandardMaterial>,
        images: &mut Assets<Image>,
        bindposes: &mut Assets<SkinnedMeshInverseBindposes>,
    ) -> Option<Arc<PreparedModel>> {
        if let Some(m) = self.models.get(name) {
            return Some(m.clone());
        }
        let m = content.model(name, meshes, materials, images, bindposes)?;
        let dressed = self.dress(content, name, &m, meshes, materials, images).map_or(m, Arc::new);
        self.models.insert(name.to_owned(), dressed.clone());
        Some(dressed)
    }

    /// `m`, `content`'s model `name`, normal-mapped (its surfaces in the
    /// same order), not cached: for callers keeping their own (the guns,
    /// whose models come from more than one content).
    pub fn dressed(
        &mut self,
        content: &Content,
        name: &str,
        m: &PreparedModel,
        meshes: &mut Assets<Mesh>,
        materials: &mut Assets<StandardMaterial>,
        images: &mut Assets<Image>,
    ) -> Option<PreparedModel> {
        self.dress(content, name, m, meshes, materials, images).filter(|d| d.surfaces.len() == m.surfaces.len())
    }

    fn dress(
        &mut self,
        content: &Content,
        name: &str,
        m: &PreparedModel,
        meshes: &mut Assets<Mesh>,
        materials: &mut Assets<StandardMaterial>,
        images: &mut Assets<Image>,
    ) -> Option<PreparedModel> {
        let (zi, id) = content.find(name)?;
        let xm = content.zones[zi].xmodel(id)?;
        let lod = xm.lods.first()?;
        // The surfaces `Content::model` kept, in order.
        let kept: Vec<usize> = (lod.surf_index as usize..(lod.surf_index + lod.num_surfs) as usize)
            .filter(|&i| xm.surfs.get(i).is_some_and(|s| !s.verts.is_empty() && !s.tris.is_empty()))
            .filter(|&i| xm.materials.get(i).copied().flatten().is_some())
            .collect();
        if kept.len() != m.surfaces.len() {
            return None;
        }
        let surfaces = kept
            .iter()
            .zip(&m.surfaces)
            .filter_map(|(&surf, s)| self.dress_surface(content, zi, xm, surf, s, meshes, materials, images))
            .collect();
        let bones = m
            .bones
            .iter()
            .map(|b| PreparedBone { name: b.name.clone(), parent: b.parent, bind_local: b.bind_local, anim_base: b.anim_base })
            .collect();
        Some(PreparedModel { name: m.name.clone(), bones, surfaces, inverse_bindposes: m.inverse_bindposes.clone(), extent: m.extent, bounds: m.bounds })
    }

    /// One surface: `None` leaves it out (the cornea); otherwise its mesh
    /// with tangents and its material with the normal map, or as they were.
    #[allow(clippy::too_many_arguments)]
    fn dress_surface(
        &mut self,
        content: &Content,
        zi: usize,
        xm: &iw3::zone::XModel,
        surf: usize,
        (mesh, material): &(Handle<Mesh>, Handle<StandardMaterial>),
        meshes: &mut Assets<Mesh>,
        materials: &mut Assets<StandardMaterial>,
        images: &mut Assets<Image>,
    ) -> Option<(Handle<Mesh>, Handle<StandardMaterial>)> {
        let unchanged = Some((mesh.clone(), material.clone()));
        let Some(mid) = xm.materials.get(surf).copied().flatten() else { return unchanged };
        // `,name` is defined in another of the content's zones.
        let (zone, mat) = {
            let zone = &content.zones[zi];
            let Some(mat) = zone.material(mid) else { return unchanged };
            match mat.name.strip_prefix(',') {
                Some(real) => match content.zones.iter().find_map(|z| z.find(real).and_then(|i| z.material(i)).map(|m| (z, m))) {
                    Some(found) => found,
                    None => return unchanged,
                },
                None => (zone, mat),
            }
        };
        let techset = mat.technique_set.and_then(|t| zone.technique_set(t)).map_or("", |t| t.name.as_str());
        if techset.contains("cornea") {
            return None;
        }
        // `normalMap` (not `detailNormalMap`): named n...p.
        let normal = mat
            .textures
            .iter()
            .find(|t| t.semantic == iw3::zone::TextureSemantic::Normal && t.name_start == b'n' && t.name_end == b'p')
            .and_then(|t| zone.image(t.image?))
            .map(|i| i.name.trim_start_matches(',').to_owned());
        let Some(normal) = normal else { return unchanged };
        let image = self.normals.entry(normal.clone()).or_insert_with(|| normal_map(&content.vfs, &normal).map(|i| images.add(i))).clone();
        let Some(image) = image else { return unchanged };
        let Some(old_material) = materials.get(material) else { return unchanged };
        // From the model, not the mesh asset: once that's on the GPU its
        // vertices can't be read (a killcam dressing the gun again would
        // panic).
        let Some(mut new_mesh) = crate::content::surface_mesh(xm, surf, true) else { return unchanged };
        let tangents: Vec<[f32; 4]> = xm.surfs[surf]
            .verts
            .iter()
            .map(|v| {
                let t = crate::units::dir(iw3::unpack::unit_vec(v.tangent)).normalize_or(Vec3::X);
                [t.x, t.y, t.z, if v.binormal_sign < 0.0 { -1.0 } else { 1.0 }]
            })
            .collect();
        new_mesh.insert_attribute(Mesh::ATTRIBUTE_TANGENT, tangents);
        let mut new_material = old_material.clone();
        new_material.normal_map_texture = Some(image);
        Some((crate::mesh_bounds::add(meshes, new_mesh), materials.add(new_material)))
    }
}

/// A CoD normal map (DXT5, X in alpha and Y in green, as slopes) as a
/// tangent-space normal map Bevy's standard material reads.
fn normal_map(vfs: &Vfs, name: &str) -> Option<Image> {
    let data = vfs.read(&format!("images/{name}.iwi")).ok()??;
    let iwi = iw3::iwi::Iwi::parse(&data).ok()?;
    if iwi.format != iw3::iwi::Format::Dxt5 || iwi.width % 4 != 0 || iwi.height % 4 != 0 {
        return None;
    }
    let (w, h) = (iwi.width as usize, iwi.height as usize);
    let blocks = &iwi.levels[0];
    let mut out = vec![0u8; w * h * 4];
    for (b, block) in blocks.chunks_exact(16).enumerate().take((w / 4) * (h / 4)) {
        let (bx, by) = ((b % (w / 4)) * 4, (b / (w / 4)) * 4);
        let alpha = dxt5_alpha(block);
        let green = dxt_green(&block[8..]);
        for i in 0..16 {
            // IW3's slope encoding (the world shader's `iw3_slope`).
            let x = alpha[i] as f32 / 255.0 * 4.08 - 2.08;
            let y = green[i] as f32 / 255.0 * 4.064_516 - 2.064_516;
            let n = Vec3::new(x, y, 1.0).normalize();
            let p = ((by + i / 4) * w + bx + i % 4) * 4;
            out[p..p + 4].copy_from_slice(&[enc(n.x), enc(n.y), enc(n.z), 255]);
        }
    }
    let mut image = Image::new(
        Extent3d { width: w as u32, height: h as u32, depth_or_array_layers: 1 },
        TextureDimension::D2,
        out,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = bevy::image::ImageSampler::linear();
    Some(image)
}

fn enc(v: f32) -> u8 {
    ((v * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u8
}

/// A DXT5 block's 16 alpha values.
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

/// A DXT colour block's 16 green values (always four-colour in DXT5).
fn dxt_green(block: &[u8]) -> [u8; 16] {
    let g = |c: u16| (((c >> 5) & 0x3f) as u32 * 255 + 31) / 63;
    let (c0, c1) = (g(u16::from_le_bytes([block[0], block[1]])), g(u16::from_le_bytes([block[2], block[3]])));
    let palette = [c0, c1, (2 * c0 + c1) / 3, (c0 + 2 * c1) / 3];
    let bits = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
    std::array::from_fn(|i| palette[((bits >> (2 * i)) & 3) as usize] as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dxt5_blocks() {
        // Alpha 255..0 interpolated, index 0 everywhere: all 255.
        let mut block = [0u8; 16];
        block[0] = 255;
        assert_eq!(dxt5_alpha(&block), [255; 16]);
        // Colour block with c0 pure green, index 1 (c1 = black) everywhere.
        let c = [0xe0, 0x07, 0, 0, 0x55, 0x55, 0x55, 0x55];
        assert_eq!(dxt_green(&c), [0; 16]);
        let c = [0xe0, 0x07, 0, 0, 0, 0, 0, 0];
        assert_eq!(dxt_green(&c), [255; 16]);
    }
}

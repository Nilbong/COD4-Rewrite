//! Game content loaded from the CoD4 install: the map zone, `common_mp`, the
//! iwd filesystem, and caches of everything converted for Bevy.

use crate::textures::TextureCache;
use crate::units;
use bevy::asset::RenderAssetUsages;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::*;
use iw3::xanim::XAnim;
use iw3::zone::{self, generic::GNode, Asset, AssetId, AssetType, TextureSemantic, Zone};
use std::collections::HashMap;
use std::sync::Arc;

/// Zone indices in [`Content::zones`].
pub const MAP_ZONE: usize = 0;
pub const COMMON_ZONE: usize = 1;

#[derive(Clone)]
pub struct MatInfo {
    pub handle: Handle<StandardMaterial>,
    pub sky: bool,
}

pub struct PreparedBone {
    pub name: String,
    pub parent: Option<usize>,
    /// Bind pose relative to the parent bone (or model root).
    pub bind_local: Transform,
    /// The model's own local translation (`XModel::trans`), which XAnim
    /// translations are added to. Equals the bind translation for most
    /// models, but viewhands store zero and take everything from the anim.
    pub anim_base: Vec3,
}

/// An xmodel converted for rendering: skinned surfaces of its first LOD plus
/// the skeleton needed to pose them.
pub struct PreparedModel {
    pub name: String,
    pub bones: Vec<PreparedBone>,
    pub surfaces: Vec<(Handle<Mesh>, Handle<StandardMaterial>)>,
    pub inverse_bindposes: Handle<SkinnedMeshInverseBindposes>,
}

#[derive(Resource)]
pub struct Content {
    pub zones: Vec<Zone>,
    pub vfs: Arc<iw3::iwd::Vfs>,
    textures: TextureCache,
    materials: HashMap<(usize, AssetId), Option<MatInfo>>,
    static_meshes: HashMap<(usize, AssetId, usize), Option<Handle<Mesh>>>,
    models: HashMap<String, Option<Arc<PreparedModel>>>,
    anims: HashMap<String, Option<Arc<XAnim>>>,
}

/// CoD quaternion (x, y, z, w) in CoD axes -> Bevy rotation.
pub fn quat(q: [f32; 4]) -> Quat {
    Quat::from_xyzw(q[0], q[2], -q[1], q[3]).normalize()
}

impl Content {
    pub fn new(zones: Vec<Zone>, vfs: Arc<iw3::iwd::Vfs>) -> Content {
        Content {
            zones,
            vfs,
            textures: TextureCache::default(),
            materials: HashMap::new(),
            static_meshes: HashMap::new(),
            models: HashMap::new(),
            anims: HashMap::new(),
        }
    }

    pub fn map(&self) -> &Zone {
        &self.zones[MAP_ZONE]
    }

    /// Find an asset by name, map zone first.
    pub fn find(&self, name: &str) -> Option<(usize, AssetId)> {
        self.zones.iter().enumerate().find_map(|(zi, z)| z.find(name).map(|id| (zi, id)))
    }

    /// A generically loaded asset (weapons, anims, fx, ...) by type and name.
    pub fn generic(&self, ty: AssetType, name: &str) -> Option<(usize, &GNode)> {
        self.zones.iter().enumerate().find_map(|(zi, z)| {
            z.assets.iter().find_map(|a| match a {
                Asset::Generic(g) if g.ty == ty && g.name == name => Some((zi, &g.root)),
                _ => None,
            })
        })
    }

    pub fn material(&mut self, zi: usize, id: AssetId, materials: &mut Assets<StandardMaterial>, images: &mut Assets<Image>) -> Option<MatInfo> {
        let (zi, id) = self.resolve_material(zi, id);
        if let Some(m) = self.materials.get(&(zi, id)) {
            return m.clone();
        }
        let info = self.build_material(zi, id, materials, images);
        self.materials.insert((zi, id), info.clone());
        info
    }

    /// A material named `,name` is a reference to one defined in another
    /// zone (e.g. viewhands materials in common_mp); resolve it there.
    fn resolve_material(&self, zi: usize, id: AssetId) -> (usize, AssetId) {
        let Some(name) = self.zones[zi].material(id).and_then(|m| m.name.strip_prefix(',')) else { return (zi, id) };
        self.zones
            .iter()
            .enumerate()
            .filter(|&(z, _)| z != zi)
            .find_map(|(z, zone)| zone.find(name).filter(|&rid| zone.material(rid).is_some()).map(|rid| (z, rid)))
            .unwrap_or((zi, id))
    }

    /// A model named `,name` is a reference to one defined in another zone
    /// (rubble bricks, flags, bombs placed in a map but kept in common_mp);
    /// resolve it there.
    pub fn resolve_xmodel(&self, zi: usize, id: AssetId) -> (usize, AssetId) {
        let Some(name) = self.zones[zi].xmodel(id).and_then(|m| m.name.strip_prefix(',')) else { return (zi, id) };
        self.zones
            .iter()
            .enumerate()
            .filter(|&(z, _)| z != zi)
            .find_map(|(z, zone)| zone.find(name).filter(|&rid| zone.xmodel(rid).is_some_and(|m| !m.lods.is_empty())).map(|rid| (z, rid)))
            .unwrap_or((zi, id))
    }

    pub fn zone(&self, zi: usize) -> &Zone {
        &self.zones[zi]
    }

    /// The map's sky as a cube texture.
    pub fn sky_cube(&mut self, images: &mut Assets<Image>) -> Option<Handle<Image>> {
        let zone = &self.zones[MAP_ZONE];
        let name = zone.gfx_world()?.sky_image.and_then(|i| zone.image(i))?.name.clone();
        self.textures.get_cube(&name, &self.vfs, images)
    }

    /// The image name behind one of a material's textures, e.g. its normal map.
    pub fn material_texture_name(&self, zi: usize, id: AssetId, semantic: TextureSemantic) -> Option<String> {
        let zone = &self.zones[zi];
        let image = zone.material(id)?.textures.iter().find(|t| t.semantic == semantic)?.image?;
        zone.image(image).map(|i| i.name.clone())
    }

    /// One of a material's textures by semantic, e.g. its normal map.
    pub fn material_texture(
        &mut self,
        zi: usize,
        id: AssetId,
        semantic: TextureSemantic,
        srgb: bool,
        images: &mut Assets<Image>,
    ) -> Option<Handle<Image>> {
        let name = self.material_texture_name(zi, id, semantic)?;
        self.textures.get(&name, srgb, &self.vfs, images)
    }

    fn build_material(&mut self, zi: usize, id: AssetId, materials: &mut Assets<StandardMaterial>, images: &mut Assets<Image>) -> Option<MatInfo> {
        let zone = &self.zones[zi];
        let mat = zone.material(id)?;
        let techset = mat.technique_set.and_then(|t| zone.technique_set(t)).map(|t| t.name.as_str()).unwrap_or("");
        let sky = techset.contains("sky");
        let image_name = |sem: TextureSemantic| {
            mat.textures.iter().find(|t| t.semantic == sem).and_then(|t| t.image).and_then(|i| zone.image(i)).map(|i| i.name.clone())
        };
        let color = image_name(TextureSemantic::Color).or_else(|| {
            // Effects and some decals only have a "2D" texture.
            mat.textures.first().and_then(|t| t.image).and_then(|i| zone.image(i)).map(|i| i.name.clone())
        });
        let bits = state_bits(mat);
        let base_color_texture = color.and_then(|n| self.textures.get(&n, true, &self.vfs, images));

        let mut m = StandardMaterial { base_color_texture, perceptual_roughness: 0.85, reflectance: 0.2, ..default() };
        // Render state comes from the lit technique's state bits.
        if let Some([b0, b1]) = bits {
            let src = b0 & 0xf;
            let dst = (b0 >> 4) & 0xf;
            let blend_op = (b0 >> 8) & 0x7;
            let atest = b0 & 0x3000;
            m.alpha_mode = if blend_op != 0 && !(src == 2 && dst == 1) {
                match (src, dst) {
                    // ONE/ONE, SRCALPHA/ONE and INVDESTCOLOR/ONE (a screen:
                    // fake light beams and flares, as in `crate::fx`) add.
                    // The screen blended by alpha drew lamps' beams as dark
                    // boxes.
                    (2, 2) | (5, 2) | (10, 2) => AlphaMode::Add,
                    // DESTCOLOR/SRCCOLOR is a 2x multiply (HDR portals, which
                    // the world shader doubles; see `crate::world`).
                    (9, 1) | (1, 3) | (9, 3) => AlphaMode::Multiply,
                    _ => AlphaMode::Blend,
                }
            } else if b0 & 0x800 == 0 && atest != 0 {
                AlphaMode::Mask(if atest == 0x1000 { 0.01 } else { 0.5 })
            } else {
                AlphaMode::Opaque
            };
            match b0 & 0xc000 {
                0x4000 => {
                    m.double_sided = true;
                    m.cull_mode = None;
                }
                0xc000 => m.cull_mode = Some(bevy::render::render_resource::Face::Front),
                _ => {}
            }
            // Polygon offset marks decals.
            if (b1 >> 4) & 3 != 0 {
                m.depth_bias = 2.0;
            }
            if matches!(m.alpha_mode, AlphaMode::Add) || (src, dst) == (9, 3) {
                m.unlit = true;
            }
        }
        if techset.contains("nofog") || techset.contains("distfalloff") {
            m.fog_enabled = false;
        }
        Some(MatInfo { handle: materials.add(m), sky })
    }

    /// Unskinned mesh for one surface (used for static world models).
    pub fn static_mesh(&mut self, zi: usize, model: AssetId, surf: usize, meshes: &mut Assets<Mesh>) -> Option<Handle<Mesh>> {
        if let Some(h) = self.static_meshes.get(&(zi, model, surf)) {
            return h.clone();
        }
        let zone = &self.zones[zi];
        let h = zone
            .xmodel(model)
            .and_then(|xm| {
                let mut mesh = surface_mesh(xm, surf, false)?;
                let effect = xm.materials.get(surf).copied().flatten().is_some_and(|m| self.is_effect(zi, m));
                if effect {
                    // In gamma space, as the world's (`crate::world`).
                    let colors: Vec<[f32; 4]> = xm.surfs[surf]
                        .verts
                        .iter()
                        .map(|v| {
                            let [r, g, b, a] = iw3::unpack::color(v.color);
                            [r.powf(2.2), g.powf(2.2), b.powf(2.2), a]
                        })
                        .collect();
                    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
                }
                Some(mesh)
            })
            .map(|m| meshes.add(m));
        self.static_meshes.insert((zi, model, surf), h.clone());
        h
    }

    /// A material's technique set name (`""` without one).
    pub fn technique_set(&self, zi: usize, id: AssetId) -> &str {
        let zone = &self.zones[zi];
        zone.material(id).and_then(|m| m.technique_set).and_then(|t| zone.technique_set(t)).map_or("", |t| t.name.as_str())
    }

    /// Effect materials on models (`mc_effect*`, `*_falloff_*`: lamps' fake
    /// beams and flares), whose vertex colours shade them.
    pub fn is_effect(&self, zi: usize, id: AssetId) -> bool {
        let (zi, id) = self.resolve_material(zi, id);
        let ts = self.technique_set(zi, id);
        ts.contains("effect") || ts.contains("falloff")
    }

    /// Prepare an xmodel by name for skinned rendering.
    pub fn model(
        &mut self,
        name: &str,
        meshes: &mut Assets<Mesh>,
        materials: &mut Assets<StandardMaterial>,
        images: &mut Assets<Image>,
        bindposes: &mut Assets<SkinnedMeshInverseBindposes>,
    ) -> Option<Arc<PreparedModel>> {
        if let Some(m) = self.models.get(name) {
            return m.clone();
        }
        let prepared = self.prepare_model(name, meshes, materials, images, bindposes).map(Arc::new);
        if prepared.is_none() {
            warn!("model {name} not found");
        }
        self.models.insert(name.to_owned(), prepared.clone());
        prepared
    }

    fn prepare_model(
        &mut self,
        name: &str,
        meshes: &mut Assets<Mesh>,
        materials: &mut Assets<StandardMaterial>,
        images: &mut Assets<Image>,
        bindposes: &mut Assets<SkinnedMeshInverseBindposes>,
    ) -> Option<PreparedModel> {
        let (zi, id) = self.find(name)?;
        let xm = self.zones[zi].xmodel(id)?;
        let n = xm.num_bones as usize;
        let roots = xm.num_root_bones as usize;
        // Bind pose: model-space matrices from baseMat.
        let globals: Vec<Mat4> = (0..n)
            .map(|i| {
                let b = xm.base_mat.get(i);
                let q = b.map_or(Quat::IDENTITY, |b| quat(b.quat));
                let t = b.map_or(Vec3::ZERO, |b| units::pos(b.trans));
                Mat4::from_rotation_translation(if q.is_finite() { q } else { Quat::IDENTITY }, t)
            })
            .collect();
        let bones: Vec<PreparedBone> = (0..n)
            .map(|i| {
                let parent = (i >= roots).then(|| i - xm.parent_list.get(i - roots).copied().unwrap_or(0) as usize);
                let local = match parent {
                    Some(p) if p < n => globals[p].inverse() * globals[i],
                    _ => globals[i],
                };
                let bind_local = Transform::from_matrix(local);
                let anim_base = match i.checked_sub(roots).and_then(|k| xm.trans.get(k)) {
                    Some(&t) => units::pos(t),
                    None => bind_local.translation,
                };
                PreparedBone {
                    name: xm.bone_names.get(i).cloned().unwrap_or_default(),
                    parent: parent.filter(|&p| p < n),
                    bind_local,
                    anim_base,
                }
            })
            .collect();
        let inverse_bindposes = bindposes.add(SkinnedMeshInverseBindposes::from(globals.iter().map(|g| g.inverse()).collect::<Vec<_>>()));

        let lod = xm.lods.first().copied()?;
        let surf_range = lod.surf_index as usize..(lod.surf_index + lod.num_surfs) as usize;
        let mats: Vec<Option<AssetId>> = surf_range.clone().map(|s| xm.materials.get(s).copied().flatten()).collect();
        let surf_meshes: Vec<Option<Mesh>> = surf_range.clone().map(|s| surface_mesh(xm, s, true)).collect();
        let mut surfaces = Vec::new();
        for (mesh, mat) in surf_meshes.into_iter().zip(mats) {
            let (Some(mesh), Some(mat)) = (mesh, mat) else { continue };
            let Some(info) = self.material(zi, mat, materials, images) else { continue };
            surfaces.push((meshes.add(mesh), info.handle));
        }
        Some(PreparedModel { name: name.to_owned(), bones, surfaces, inverse_bindposes })
    }

    /// Decode an animation by name.
    pub fn anim(&mut self, name: &str) -> Option<Arc<XAnim>> {
        if let Some(a) = self.anims.get(name) {
            return a.clone();
        }
        let decoded = self.zones.iter().find_map(|z| {
            z.assets.iter().find_map(|a| match a {
                Asset::Generic(g) if g.ty == AssetType::XAnimParts && g.name.eq_ignore_ascii_case(name) => {
                    Some(XAnim::from_node(name, &g.root, &z.script_strings))
                }
                _ => None,
            })
        });
        let anim = match decoded {
            Some(Ok(a)) => Some(Arc::new(a)),
            Some(Err(e)) => {
                warn!("anim {name}: {e}");
                None
            }
            None => {
                warn!("anim {name} not found");
                None
            }
        };
        self.anims.insert(name.to_owned(), anim.clone());
        anim
    }
}

/// Per-vertex (joints, weights) for a surface.
fn skin_weights(s: &zone::XSurface, num_bones: usize) -> Vec<([u16; 4], [f32; 4])> {
    let n = s.verts.len();
    let mut out = vec![([0u16; 4], [1.0, 0.0, 0.0, 0.0]); n];
    let bone = |raw: u16| -> u16 {
        // Bone references are byte offsets into 64-byte skeleton matrices.
        let i = raw / 64;
        if (i as usize) < num_bones { i } else { 0 }
    };
    if !s.vert_lists.is_empty() {
        let mut v = 0usize;
        for vl in &s.vert_lists {
            for slot in out.iter_mut().skip(v).take(vl.vert_count as usize) {
                slot.0[0] = bone(vl.bone_offset);
            }
            v += vl.vert_count as usize;
        }
        return out;
    }
    let b = &s.verts_blend;
    let (mut v, mut k) = (0usize, 0usize);
    for (influences, &count) in s.blend_counts.iter().enumerate() {
        for _ in 0..count.max(0) {
            if v >= n {
                break;
            }
            let fields = 1 + influences * 2;
            let Some(e) = b.get(k..k + fields) else { break };
            let mut joints = [bone(e[0]), 0, 0, 0];
            let mut weights = [0.0f32; 4];
            let mut rest = 1.0;
            for j in 0..influences {
                joints[j + 1] = bone(e[1 + j * 2]);
                weights[j + 1] = e[2 + j * 2] as f32 / 65536.0;
                rest -= weights[j + 1];
            }
            weights[0] = rest.max(0.0);
            out[v] = (joints, weights);
            v += 1;
            k += fields;
        }
    }
    out
}

/// Mesh for one XModel surface, in model space (Bevy axes). Vertices are
/// stored in model space for both rigid and skinned surfaces.
/// A material's render state bits: its lit technique's, or the first
/// that has any.
pub fn state_bits(mat: &zone::Material) -> Option<[u32; 2]> {
    [zone::TECHNIQUE_LIT, 8, zone::TECHNIQUE_UNLIT, zone::TECHNIQUE_EMISSIVE]
        .into_iter()
        .find_map(|t| mat.state_bits_for(t))
        .or_else(|| mat.state_bits.first().copied())
}

pub fn surface_mesh(xm: &zone::XModel, surf: usize, skinned: bool) -> Option<Mesh> {
    let s = xm.surfs.get(surf)?;
    if s.verts.is_empty() || s.tris.is_empty() {
        return None;
    }
    let positions: Vec<[f32; 3]> = s.verts.iter().map(|v| units::pos(v.xyz).to_array()).collect();
    let normals: Vec<[f32; 3]> = s
        .verts
        .iter()
        .map(|v| units::dir(iw3::unpack::unit_vec(v.normal)).normalize_or(Vec3::Y).to_array())
        .collect();
    let uvs: Vec<[f32; 2]> = s.verts.iter().map(|v| iw3::unpack::tex_coords(v.tex_coord)).collect();
    // Clockwise -> counter-clockwise, as for world geometry.
    let indices: Vec<u32> = s.tris.iter().flat_map(|t| [t[0] as u32, t[2] as u32, t[1] as u32]).collect();
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    if skinned {
        let w = skin_weights(s, xm.num_bones as usize);
        mesh.insert_attribute(Mesh::ATTRIBUTE_JOINT_INDEX, VertexAttributeValues::Uint16x4(w.iter().map(|x| x.0).collect()));
        mesh.insert_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, w.iter().map(|x| x.1).collect::<Vec<[f32; 4]>>());
    }
    mesh.insert_indices(Indices::U32(indices));
    Some(mesh)
}

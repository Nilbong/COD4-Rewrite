//! World at War models, animations, materials, technique sets and images as
//! `iw3` assets.
//!
//! World at War is CoD4's engine one step on: animations, images, the packed
//! vertex, rigid vertex lists and render state bits are laid out exactly as
//! in CoD4; models add physics/collision pointers, surfaces add buffer
//! handles, and materials have more techniques (fade and charred variants).
//! [`to_iw3`] builds an `iw3::zone::Zone` holding just these assets, so code
//! written for CoD4 zones (the game's model pipeline and `iw3::xanim`) can
//! use World at War ones unchanged. Images are streamed from the `.iwd`
//! archives by name, like CoD4's.

use crate::zone::schema::schema;
use crate::zone::{AssetType, GNode, GVal, Zone};
use iw3::zone as z3;

/// World at War technique index for each CoD4 one. World at War inserts
/// `emissive_reflected`, a `_fade` variant after each lit technique, the
/// `_charred` lit techniques and charred lights.
fn t4_technique(iw3_index: usize) -> Option<usize> {
    Some(match iw3_index {
        0..=6 => iw3_index,
        // lit, lit_sun, lit_sun_shadow, lit_spot, lit_spot_shadow, lit_omni, lit_omni_shadow
        7..=13 => 8 + (iw3_index - 7) * 2,
        // lit_instanced.. and light_spot, light_omni, light_spot_shadow
        14..=23 => iw3_index + 22,
        // fakelight_normal .. debug_bumpmap_instanced
        24..=33 => iw3_index + 25,
        _ => return None,
    })
}

/// The bytes of a loaded array, whether it came back as primitives or as
/// pointer-free records.
fn raw(n: &GNode, field: &str) -> Vec<u8> {
    match n.field(field) {
        Some(GVal::Bytes(b)) => b.clone(),
        Some(GVal::Nodes(nodes)) => nodes.iter().flat_map(|c| c.data.iter().copied()).collect(),
        _ => Vec::new(),
    }
}

fn u16s(b: &[u8]) -> impl Iterator<Item = u16> + '_ {
    b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]]))
}

fn f32s(b: &[u8]) -> impl Iterator<Item = f32> + '_ {
    b.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap()))
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    b.get(o..o + 4).map_or(0, |c| u32::from_le_bytes(c.try_into().unwrap()))
}

/// A `vec3_t` member read from a struct image (the path reader indexes
/// whole members, not their elements).
fn vec3(n: &GNode, member: &str) -> [f32; 3] {
    let offset = schema().types[&n.ty].members.iter().find(|m| m.name.as_deref() == Some(member)).map_or(0, |m| m.offset as usize);
    std::array::from_fn(|k| f32::from_bits(u32_at(&n.data, offset + k * 4)))
}

/// Models, animations, materials, technique sets and images of a World at
/// War zone as a CoD4 zone. Asset ids are renumbered; references between
/// them follow.
pub fn to_iw3(zone: &Zone) -> z3::Zone {
    let kept = |ty: AssetType| {
        matches!(ty, AssetType::XModel | AssetType::XAnimParts | AssetType::Material | AssetType::TechniqueSet | AssetType::Image)
    };
    let mut map = vec![None; zone.assets.len()];
    let mut order = Vec::new();
    for (i, a) in zone.assets.iter().enumerate() {
        if kept(a.ty) {
            map[i] = Some(order.len());
            order.push(i);
        }
    }
    let remap = |id: Option<usize>| id.and_then(|i| map.get(i).copied().flatten());
    let assets = order
        .iter()
        .map(|&i| {
            let a = &zone.assets[i];
            match a.ty {
                AssetType::XModel => z3::Asset::XModel(xmodel(zone, &a.root, &remap)),
                // CoD4's layout exactly.
                AssetType::XAnimParts => z3::Asset::Generic(z3::GenericAsset {
                    ty: z3::AssetType::XAnimParts,
                    name: a.name.clone(),
                    root: gnode(&a.root),
                }),
                AssetType::Material => z3::Asset::Material(material(&a.root, &remap)),
                AssetType::TechniqueSet => z3::Asset::TechniqueSet(z3::TechniqueSet {
                    name: a.name.clone(),
                    world_vert_format: a.root.int("worldVertFormat") as u8,
                    techniques: Vec::new(),
                }),
                _ => z3::Asset::Image(image(&a.root)),
            }
        })
        .collect();
    z3::Zone { script_strings: zone.script_strings.clone(), assets, top_level: Vec::new(), stats: Default::default() }
}

pub fn xmodel(zone: &Zone, m: &GNode, remap: &dyn Fn(Option<usize>) -> Option<usize>) -> z3::XModel {
    let quats = raw(m, "quats").chunks_exact(8).map(|q| std::array::from_fn(|k| i16::from_le_bytes([q[k * 2], q[k * 2 + 1]]))).collect();
    // Packed vec3s, though the allocation is sized for vec4s.
    let non_root = (m.int("numBones") - m.int("numRootBones")).max(0) as usize;
    let trans: Vec<f32> = f32s(&raw(m, "trans")).collect();
    let trans = trans.chunks_exact(3).take(non_root).map(|t| [t[0], t[1], t[2]]).collect();
    let base_mat = raw(m, "baseMat")
        .chunks_exact(32)
        .map(|b| {
            let f: Vec<f32> = f32s(b).collect();
            z3::BoneMat { quat: [f[0], f[1], f[2], f[3]], trans: [f[4], f[5], f[6]] }
        })
        .collect();
    let lods = (0..m.int("numLods").clamp(0, 4))
        .map(|i| z3::XModelLod {
            dist: m.float(&format!("lodInfo[{i}]::dist")),
            num_surfs: m.int(&format!("lodInfo[{i}]::numsurfs")) as u16,
            surf_index: m.int(&format!("lodInfo[{i}]::surfIndex")) as u16,
        })
        .collect();
    z3::XModel {
        name: m.string("name").unwrap_or_default().to_owned(),
        num_bones: m.int("numBones") as u8,
        num_root_bones: m.int("numRootBones") as u8,
        bone_names: u16s(&raw(m, "boneNames")).map(|i| zone.script_strings.get(i as usize).cloned().unwrap_or_default()).collect(),
        parent_list: raw(m, "parentList"),
        quats,
        trans,
        base_mat,
        surfs: m.nodes("surfs").iter().map(surface).collect(),
        materials: m.assets("materialHandles").into_iter().map(remap).collect(),
        lods,
        radius: m.float("radius"),
        mins: vec3(m, "mins"),
        maxs: vec3(m, "maxs"),
        contents: m.int("contents") as i32,
    }
}

fn surface(s: &GNode) -> z3::XSurface {
    let info = s.node("vertInfo");
    let blend_counts = std::array::from_fn(|k| info.map_or(0, |i| i.int(&format!("vertCount[{k}]")) as i16));
    let verts = raw(s, "verts0")
        .chunks_exact(32)
        .map(|v| z3::PackedVertex {
            xyz: std::array::from_fn(|k| f32::from_bits(u32_at(v, k * 4))),
            binormal_sign: f32::from_bits(u32_at(v, 12)),
            color: u32_at(v, 16),
            tex_coord: u32_at(v, 20),
            normal: u32_at(v, 24),
            tangent: u32_at(v, 28),
        })
        .collect();
    let tris = u16s(&raw(s, "triIndices")).collect::<Vec<_>>().chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect();
    let vert_lists = s
        .nodes("vertList")
        .iter()
        .map(|l| z3::RigidVertList {
            bone_offset: l.int("boneOffset") as u16,
            vert_count: l.int("vertCount") as u16,
            tri_offset: l.int("triOffset") as u16,
            tri_count: l.int("triCount") as u16,
        })
        .collect();
    z3::XSurface {
        tile_mode: s.int("tileMode") as u8,
        deformed: s.int("deformed") != 0,
        vert_count: s.int("vertCount") as u16,
        tri_count: s.int("triCount") as u16,
        base_tri_index: s.int("baseTriIndex") as u16,
        base_vert_index: s.int("baseVertIndex") as u16,
        blend_counts,
        verts_blend: info.map(|i| u16s(&raw(i, "vertsBlend")).collect()).unwrap_or_default(),
        verts,
        tris,
        vert_lists,
    }
}

pub fn material(m: &GNode, remap: &dyn Fn(Option<usize>) -> Option<usize>) -> z3::Material {
    let info = m.node("info");
    let entries: Vec<u8> = (0..59).map(|i| m.int(&format!("stateBitsEntry[{i}]")) as u8).collect();
    let state_bits_entry = std::array::from_fn(|i| t4_technique(i).and_then(|t| entries.get(t).copied()).unwrap_or(0xff));
    let textures = m
        .nodes("textureTable")
        .iter()
        .map(|t| z3::MaterialTexture {
            name_hash: t.int("nameHash") as u32,
            name_start: t.int("nameStart") as u8,
            name_end: t.int("nameEnd") as u8,
            sampler_state: t.data.get(6).copied().unwrap_or(0),
            semantic: z3::TextureSemantic::from_u8(t.int("semantic") as u8),
            image: remap(t.node("u").and_then(|u| u.asset("image"))),
        })
        .collect();
    let constants = raw(m, "constantTable")
        .chunks_exact(32)
        .map(|c| z3::MaterialConstant {
            name_hash: u32_at(c, 0),
            name: String::from_utf8_lossy(&c[4..16]).trim_end_matches('\0').to_owned(),
            literal: std::array::from_fn(|k| f32::from_bits(u32_at(c, 16 + k * 4))),
        })
        .collect();
    let state_bits = raw(m, "stateBitsTable").chunks_exact(8).map(|b| [u32_at(b, 0), u32_at(b, 4)]).collect();
    z3::Material {
        name: info.and_then(|i| i.string("name")).unwrap_or_default().to_owned(),
        game_flags: info.map_or(0, |i| i.int("gameFlags") as u8),
        sort_key: info.map_or(0, |i| i.int("sortKey") as u8),
        surface_type_bits: info.map_or(0, |i| i.int("surfaceTypeBits") as u32),
        state_bits_entry,
        state_flags: m.int("stateFlags") as u8,
        camera_region: m.int("cameraRegion") as u8,
        technique_set: remap(m.asset("techniqueSet")),
        textures,
        constants,
        state_bits,
    }
}

pub fn image(i: &GNode) -> z3::Image {
    let (width, height, depth) = (i.int("width") as u16, i.int("height") as u16, i.int("depth") as u16);
    let load_def = i.node("texture").and_then(|t| t.node("loadDef")).map(|l| z3::ImageLoadDef {
        level_count: l.int("levelCount") as u8,
        flags: l.int("flags") as u8,
        dimensions: [width, height, depth],
        format: l.int("format") as u32,
        data: l.data.get(16..).unwrap_or_default().to_vec(),
    });
    z3::Image {
        name: i.string("name").unwrap_or_default().to_owned(),
        map_type: match i.int("mapType") {
            3 => z3::MapType::TwoD,
            4 => z3::MapType::ThreeD,
            5 => z3::MapType::Cube,
            o => z3::MapType::Other(o as u32),
        },
        semantic: i.int("semantic") as u8,
        category: i.int("category") as u8,
        width,
        height,
        depth,
        load_def,
    }
}

/// A World at War node tree as an `iw3` one, for structs laid out the same.
fn gnode(n: &GNode) -> z3::generic::GNode {
    z3::generic::GNode { ty: n.ty.clone(), data: n.data.clone(), fields: n.fields.iter().map(|(k, v)| (k.clone(), gval(v))).collect() }
}

fn gval(v: &GVal) -> z3::generic::GVal {
    use z3::generic::GVal as V;
    match v {
        GVal::Nodes(n) => V::Nodes(n.iter().map(gnode).collect()),
        GVal::Bytes(b) => V::Bytes(b.clone()),
        GVal::Str(s) => V::Str(s.clone()),
        GVal::Strs(s) => V::Strs(s.clone()),
        // Animations hold no asset references.
        GVal::Asset(_) => V::Asset(None),
        GVal::Assets(a) => V::Assets(vec![None; a.len()]),
    }
}

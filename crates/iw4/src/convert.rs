//! Modern Warfare 2 map zones as `iw3` (CoD4) assets.
//!
//! [`to_iw3`] builds an `iw3::zone::Zone` holding a map zone's world (draw
//! surfaces, static models, lightmaps, light grid, reflection probes), its
//! collision (brushes, terrain triangles, brush entity trees, static and
//! dynamic models), primary lights, entities, and the materials, technique
//! sets, images, models and physics presets they use, so the game's CoD4
//! map pipeline can load it unchanged.
//!
//! The formats are close relatives. Where MW2 differs:
//! - bounds are a middle and half size, not mins and maxs;
//! - model surfaces hang off each LOD (`XModelSurfs`), not the model;
//! - collision brushes point at their sides and the sides at their planes
//!   (indices here, worked out from where the arrays start);
//! - materials have 48 techniques to CoD4's 34 (their render states are
//!   looked up through CoD4's numbering);
//! - the sun is a primary light, not a `SunLightParseParams`.
//!
//! Images are inline (lightmaps, probes) or streamed by name from MW2's
//! `.iwd` archives (version 8 `.iwi`, which `iw3::iwi` reads).

use crate::zone::{AssetType, GNode, GVal, Zone};
use iw3::zone as z3;

// Animations convert on their own (maps don't use them): `common_mp`'s
// viewmodel anims as a CoD4 zone of `XAnimParts`.
pub use crate::xanim::to_iw3_anims;

/// An asset's name without MW2's leading comma (marking a stand-in for an
/// asset another zone defines).
fn bare(name: &str) -> String {
    name.trim_start_matches(',').to_owned()
}

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

fn u32_at(b: &[u8], o: usize) -> u32 {
    b.get(o..o + 4).map_or(0, |c| u32::from_le_bytes(c.try_into().unwrap()))
}

fn i16_at(b: &[u8], o: usize) -> i16 {
    b.get(o..o + 2).map_or(0, |c| i16::from_le_bytes(c.try_into().unwrap()))
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    i16_at(b, o) as u16
}

fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_bits(u32_at(b, o))
}

fn vec3_at(b: &[u8], o: usize) -> [f32; 3] {
    std::array::from_fn(|k| f32_at(b, o + k * 4))
}

/// `Bounds` (middle, half size) at `o` as mins and maxs.
fn bounds_at(b: &[u8], o: usize) -> [[f32; 3]; 2] {
    let (mid, half) = (vec3_at(b, o), vec3_at(b, o + 12));
    [std::array::from_fn(|k| mid[k] - half[k]), std::array::from_fn(|k| mid[k] + half[k])]
}

/// The element index of a reference pointer (as stored in the stream) into
/// an array starting at `base` (a [`GNode::locs`] key) of `size`-byte
/// elements.
fn index_into(raw_ptr: u32, base: Option<u32>, size: u32) -> Option<u32> {
    let base = base?;
    if raw_ptr == 0 || raw_ptr >= 0xFFFF_FFFE {
        return None;
    }
    let at = raw_ptr - 1;
    (at >> 28 == base >> 28 && at >= base && (at - base) % size == 0).then(|| (at - base) / size)
}

fn loc(n: &GNode, field: &str) -> Option<u32> {
    n.locs.iter().find(|(k, _)| k == field).map(|(_, v)| *v)
}

/// MW2's technique for each of CoD4's, where there is one: the first six
/// match; then emissive shadow, and the lit techniques skip MW2's
/// distance-fog variants.
fn iw4_technique(iw3_index: usize) -> Option<usize> {
    Some(match iw3_index {
        0..=5 => iw3_index,
        6 => 7,
        7 => 9,
        8 => 11,
        9 => 13,
        10 => 15,
        11 => 17,
        12 => 19,
        13 => 21,
        _ => return None,
    })
}

/// What a converted asset is: an id in the map zone or in MW2's common zone.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Src {
    Map(usize),
    Common(usize),
}

fn kept(ty: AssetType) -> bool {
    matches!(
        ty,
        AssetType::XModel
            | AssetType::Material
            | AssetType::TechniqueSet
            | AssetType::Image
            | AssetType::PhysPreset
            | AssetType::ComWorld
            | AssetType::MapEnts
            | AssetType::GameWorldMp
            | AssetType::GfxWorld
            | AssetType::ClipMapMp
            | AssetType::ClipMapSp
            | AssetType::RawFile
    )
}

/// Every asset a node refers to, however deep.
fn refs(n: &GNode, out: &mut Vec<usize>) {
    for (_, v) in &n.fields {
        match v {
            GVal::Asset(Some(a)) => out.push(*a),
            GVal::Assets(a) => out.extend(a.iter().flatten()),
            GVal::Nodes(ns) => ns.iter().for_each(|c| refs(c, out)),
            _ => {}
        }
    }
}

/// A map zone's assets as a CoD4 zone. Asset ids are renumbered; references
/// between them follow. Assets the map only names (stand-ins, `,name`:
/// models, materials and their images that MW2 keeps in `common_mp`) come
/// from `common` when given, with everything they use.
pub fn to_iw3(zone: &Zone, common: Option<&Zone>) -> z3::Zone {
    to_iw3_with_glass(zone, common).0
}

/// A pane of MW2's breakable glass (its FxWorld glass system): a flat
/// polygon in CoD space, drawn with `material` until it breaks.
#[derive(Debug, Clone)]
pub struct GlassPane {
    pub corners: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// The plane's normal (CoD space).
    pub normal: [f32; 3],
    pub half_thickness: f32,
    /// In the converted zone.
    pub material: Option<usize>,
    pub shattered_material: Option<usize>,
}

/// [`to_iw3`], and the map's breakable glass.
pub fn to_iw3_with_glass(zone: &Zone, common: Option<&Zone>) -> (z3::Zone, Vec<GlassPane>) {
    let mut order: Vec<Src> = Vec::new();
    let mut index: std::collections::HashMap<Src, usize> = std::collections::HashMap::new();
    // A stand-in's place takes the common asset of its name, which then
    // answers to both.
    let by_name: std::collections::HashMap<(AssetType, &str), usize> = common
        .map(|c| c.assets.iter().enumerate().filter(|(_, a)| kept(a.ty) && !a.name.starts_with(',')).map(|(i, a)| ((a.ty, a.name.as_str()), i)).collect())
        .unwrap_or_default();
    let mut pending: Vec<usize> = Vec::new();
    for (i, a) in zone.assets.iter().enumerate() {
        if !kept(a.ty) {
            continue;
        }
        // (A model can be whole but for its surfaces, which are stand-ins.)
        let surfs_named = || {
            a.root.nodes("lodInfo").iter().any(|l| l.nodes("modelSurfs").first().is_some_and(|s| s.string("name").is_some_and(|n| n.starts_with(','))))
        };
        let name = a.name.strip_prefix(',').or_else(|| (a.ty == AssetType::XModel && surfs_named()).then_some(a.name.as_str()));
        let stand_in = name.and_then(|n| by_name.get(&(a.ty, n)).copied());
        let at = order.len();
        index.insert(Src::Map(i), at);
        match stand_in {
            Some(c) if !index.contains_key(&Src::Common(c)) => {
                index.insert(Src::Common(c), at);
                order.push(Src::Common(c));
                pending.push(c);
            }
            // (Two stand-ins for one asset: both answer to the first.)
            Some(c) => {
                index.insert(Src::Map(i), index[&Src::Common(c)]);
            }
            None => order.push(Src::Map(i)),
        }
    }
    // What the common assets use, in turn.
    if let Some(c) = common {
        while let Some(id) = pending.pop() {
            let mut used = Vec::new();
            refs(&c.assets[id].root, &mut used);
            for u in used {
                if kept(c.assets[u].ty) && !index.contains_key(&Src::Common(u)) {
                    index.insert(Src::Common(u), order.len());
                    order.push(Src::Common(u));
                    pending.push(u);
                }
            }
        }
    }
    let map_remap = |id: Option<usize>| id.and_then(|i| index.get(&Src::Map(i)).copied());
    let common_remap = |id: Option<usize>| id.and_then(|i| index.get(&Src::Common(i)).copied());
    // The sun (for the world's `SunParse`): worldspawn's keys, as CoD4's
    // compiler read them, else MW2's sun primary light.
    let sun = worldspawn_sun(zone).or_else(|| {
        zone.assets.iter().find(|a| a.ty == AssetType::ComWorld).and_then(|c| {
            c.root.nodes("primaryLights").iter().find(|l| l.data.first() == Some(&1)).map(|l| {
                // (Its direction points at the sun; its colour includes the light.)
                let (color, to_sun) = (vec3_at(&l.data, 4), vec3_at(&l.data, 16));
                let light = color.iter().cloned().fold(0.0f32, f32::max).max(1e-3);
                let pitch = -to_sun[2].clamp(-1.0, 1.0).asin().to_degrees();
                let yaw = to_sun[1].atan2(to_sun[0]).to_degrees();
                Sun { color: color.map(|c| c / light), light, angles: [pitch, yaw, 0.0], ambient: 0.0, ambient_color: [1.0; 3] }
            })
        })
    });
    let assets = order
        .iter()
        .map(|&src| {
            let (from, a, remap): (&Zone, _, &dyn Fn(Option<usize>) -> Option<usize>) = match src {
                Src::Map(i) => (zone, &zone.assets[i], &map_remap),
                Src::Common(i) => (common.expect("common zone"), &common.expect("common zone").assets[i], &common_remap),
            };
            match a.ty {
                AssetType::XModel => z3::Asset::XModel(xmodel(from, &a.root, remap)),
                AssetType::Material => z3::Asset::Material(material(&a.root, remap)),
                AssetType::TechniqueSet => z3::Asset::TechniqueSet(z3::TechniqueSet {
                    name: bare(&a.name),
                    world_vert_format: a.root.int("worldVertFormat") as u8,
                    techniques: Vec::new(),
                }),
                AssetType::Image => z3::Asset::Image(image(&a.root)),
                AssetType::PhysPreset => z3::Asset::PhysPreset(phys_preset(&a.root)),
                AssetType::ComWorld => z3::Asset::ComWorld(com_world(&a.root)),
                AssetType::MapEnts => z3::Asset::MapEnts(z3::MapEnts {
                    name: a.name.clone(),
                    // (A counted char array here, not a string.)
                    entity_string: String::from_utf8_lossy(&raw(&a.root, "entityString")).trim_end_matches('\0').to_owned(),
                }),
                AssetType::GameWorldMp => z3::Asset::GameWorldMp(a.name.clone()),
                AssetType::RawFile => z3::Asset::RawFile(raw_file(&a.root, &a.name)),
                AssetType::GfxWorld => z3::Asset::GfxWorld(Box::new(gfx_world(&a.root, remap, sun))),
                _ => z3::Asset::ClipMap(Box::new(clip_map(&a.root, remap))),
            }
        })
        .collect();
    let glass = zone.assets.iter().find(|a| a.ty == AssetType::FxWorld).and_then(|f| f.root.node("glassSys")).map(|g| glass(g, &map_remap)).unwrap_or_default();
    (z3::Zone { script_strings: zone.script_strings.clone(), assets, top_level: Vec::new(), stats: Default::default() }, glass)
}

/// A raw file (the map's scripts: the minimap's material is named in
/// them), inflated when MW2 stored it compressed.
fn raw_file(n: &GNode, name: &str) -> z3::RawFile {
    // (`data` is a union: the compressed buffer or the plain one.)
    let data = n
        .node("data")
        .map(|d| ["compressedBuffer", "buffer"].iter().map(|f| raw(d, f)).find(|b| !b.is_empty()).unwrap_or_default())
        .unwrap_or_default();
    let data = if n.int("compressedLen") > 0 {
        let mut out = Vec::new();
        use std::io::Read;
        match flate2::read::ZlibDecoder::new(&data[..]).read_to_end(&mut out) {
            Ok(_) => out,
            Err(_) => Vec::new(),
        }
    } else {
        data
    };
    z3::RawFile { name: bare(name), data }
}

/// The glass system's panes as they start: each piece's polygon (16-bit
/// vertices in 1/32 units on its frame's x-y plane), placed by its frame
/// (quaternion x, y, z, w and origin), textured by its kind's texture
/// vectors from its texture origin.
fn glass(g: &GNode, remap: &dyn Fn(Option<usize>) -> Option<usize>) -> Vec<GlassPane> {
    let defs = g.nodes("defs");
    let geo = raw(g, "initGeoData");
    let mut at = 0usize;
    let mut out = Vec::new();
    for p in g.nodes("initPieceStates") {
        let b = &p.data;
        let (count, fans) = (b[49] as usize, b[50] as usize);
        let verts: Vec<[f32; 2]> = (0..count)
            .filter_map(|k| {
                let o = (at + k) * 4;
                Some([i16_at(&geo, o) as f32, geo.get(o + 2..o + 4).map(|_| i16_at(&geo, o + 2) as f32)?])
            })
            .collect();
        at += count + fans;
        let Some(def) = defs.get(b[48] as usize) else { continue };
        if verts.len() < 3 {
            continue;
        }
        let [x, y, z, w] = std::array::from_fn(|k| f32_at(b, k * 4));
        // Rotate (v, 0) by the quaternion.
        let rotate = |v: [f32; 3]| {
            let (u, s) = ([x, y, z], w);
            let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
            let cross = |a: [f32; 3], b: [f32; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
            let (uv, uu) = (dot(u, v), dot(u, u));
            let c = cross(u, v);
            std::array::from_fn(|k| 2.0 * uv * u[k] + (s * s - uu) * v[k] + 2.0 * s * c[k])
        };
        let origin = vec3_at(b, 16);
        let tex = [[f32_at(&def.data, 4), f32_at(&def.data, 8)], [f32_at(&def.data, 12), f32_at(&def.data, 16)]];
        let tc = [f32_at(b, 32), f32_at(b, 36)];
        let corners = verts
            .iter()
            .map(|v| {
                let r = rotate([v[0] / 32.0, v[1] / 32.0, 0.0]);
                [origin[0] + r[0], origin[1] + r[1], origin[2] + r[2]]
            })
            .collect();
        let uvs = verts
            .iter()
            .map(|v| [tc[0] + tex[0][0] * v[0] + tex[0][1] * v[1], tc[1] + tex[1][0] * v[0] + tex[1][1] * v[1]])
            .collect();
        out.push(GlassPane {
            corners,
            uvs,
            normal: rotate([0.0, 0.0, 1.0]),
            half_thickness: f32_at(&def.data, 0),
            material: remap(def.asset("material")),
            shattered_material: remap(def.asset("materialShattered")),
        });
    }
    out
}

pub fn xmodel(zone: &Zone, m: &GNode, remap: &dyn Fn(Option<usize>) -> Option<usize>) -> z3::XModel {
    let quats = raw(m, "quats").chunks_exact(8).map(|q| std::array::from_fn(|k| i16::from_le_bytes([q[k * 2], q[k * 2 + 1]]))).collect();
    let non_root = (m.int("numBones") - m.int("numRootBones")).max(0) as usize;
    let trans: Vec<f32> = raw(m, "trans").chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect();
    let trans = trans.chunks_exact(3).take(non_root).map(|t| [t[0], t[1], t[2]]).collect();
    let base_mat = raw(m, "baseMat")
        .chunks_exact(32)
        .map(|b| z3::BoneMat { quat: std::array::from_fn(|k| f32_at(b, k * 4)), trans: vec3_at(b, 16) })
        .collect();
    // Each LOD's surfaces are its `XModelSurfs`; `surfIndex` places them in
    // the model's list (its materials are per surface of that list).
    let lod_nodes = m.nodes("lodInfo");
    let num_lods = (m.int("numLods").clamp(0, 4) as usize).min(lod_nodes.len());
    let mut lods = Vec::new();
    let mut surfs: Vec<(u16, Vec<z3::XSurface>)> = Vec::new();
    for l in &lod_nodes[..num_lods] {
        let (num, index) = (u16_at(&l.data, 4), u16_at(&l.data, 6));
        lods.push(z3::XModelLod { dist: f32_at(&l.data, 0), num_surfs: num, surf_index: index });
        if surfs.iter().any(|s| s.0 == index) {
            continue;
        }
        let list = l.nodes("modelSurfs").first().map(|ms| ms.nodes("surfs").iter().map(surface).collect()).unwrap_or_default();
        surfs.push((index, list));
    }
    surfs.sort_by_key(|s| s.0);
    let mut all = Vec::new();
    for (index, list) in surfs {
        // (Gaps: an LOD whose surfaces didn't load keeps its places.)
        while all.len() < index as usize {
            all.push(empty_surface());
        }
        all.extend(list);
    }
    let coll_boxes = m
        .nodes("collSurfs")
        .iter()
        .map(|c| {
            let [mins, maxs] = bounds_at(&c.data, 8);
            let tris = c
                .nodes("collTris")
                .iter()
                .filter_map(|t| {
                    let v = |o: usize| std::array::from_fn(|k| f32_at(&t.data, o + k * 4));
                    z3::coll_tri_corners(v(0), v(16), v(32))
                })
                .collect();
            z3::CollBox { mins, maxs, contents: u32_at(&c.data, 36) as i32, surf_flags: u32_at(&c.data, 40) as i32, tris }
        })
        .collect();
    let [mins, maxs] = bounds_at(&m.data, 264);
    z3::XModel {
        name: bare(m.string("name").unwrap_or_default()),
        num_bones: m.int("numBones") as u8,
        num_root_bones: m.int("numRootBones") as u8,
        bone_names: u16s(&raw(m, "boneNames")).map(|i| zone.script_strings.get(i as usize).cloned().unwrap_or_default()).collect(),
        parent_list: raw(m, "parentList"),
        quats,
        trans,
        base_mat,
        surfs: all,
        materials: m.assets("materialHandles").into_iter().map(remap).collect(),
        lods,
        radius: m.float("radius"),
        mins,
        maxs,
        contents: m.int("contents") as i32,
        coll_boxes,
    }
}

fn empty_surface() -> z3::XSurface {
    z3::XSurface {
        tile_mode: 0,
        deformed: false,
        vert_count: 0,
        tri_count: 0,
        base_tri_index: 0,
        base_vert_index: 0,
        blend_counts: [0; 4],
        verts_blend: Vec::new(),
        verts: Vec::new(),
        tris: Vec::new(),
        vert_lists: Vec::new(),
    }
}

fn surface(s: &GNode) -> z3::XSurface {
    let info = s.node("vertInfo");
    let blend_counts = std::array::from_fn(|k| info.map_or(0, |i| i16_at(&i.data, k * 2)));
    let verts = raw(s, "verts0")
        .chunks_exact(32)
        .map(|v| z3::PackedVertex {
            xyz: vec3_at(v, 0),
            binormal_sign: f32_at(v, 12),
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
            bone_offset: u16_at(&l.data, 0),
            vert_count: u16_at(&l.data, 2),
            tri_offset: u16_at(&l.data, 4),
            tri_count: u16_at(&l.data, 6),
        })
        .collect();
    z3::XSurface {
        tile_mode: s.data[0],
        deformed: s.data[1] != 0,
        vert_count: u16_at(&s.data, 2),
        tri_count: u16_at(&s.data, 4),
        base_tri_index: u16_at(&s.data, 8),
        base_vert_index: u16_at(&s.data, 10),
        blend_counts,
        verts_blend: info.map(|i| u16s(&raw(i, "vertsBlend")).collect()).unwrap_or_default(),
        verts,
        tris,
        vert_lists,
    }
}

pub fn material(m: &GNode, remap: &dyn Fn(Option<usize>) -> Option<usize>) -> z3::Material {
    let info = m.node("info");
    let entries = &m.data[24..72];
    let state_bits_entry = std::array::from_fn(|i| iw4_technique(i).map_or(0xff, |t| entries[t]));
    // MW2's detail normal maps (`q0` techniques: a small tiled bump over
    // the normal map) share CoD4's colour detail map's name, `detailMap`:
    // left out, or they'd be drawn into the colour as blotches.
    const DETAIL_MAP: u32 = 0xeb52_9b4d;
    let textures = m
        .nodes("textureTable")
        .iter()
        .filter(|t| !(u32_at(&t.data, 0) == DETAIL_MAP && t.data[7] == 5))
        .map(|t| z3::MaterialTexture {
            name_hash: u32_at(&t.data, 0),
            name_start: t.data[4],
            name_end: t.data[5],
            sampler_state: t.data[6],
            semantic: z3::TextureSemantic::from_u8(t.data[7]),
            image: remap(t.node("u").and_then(|u| u.asset("image"))),
        })
        .collect();
    let constants = raw(m, "constantTable")
        .chunks_exact(32)
        .map(|c| z3::MaterialConstant {
            name_hash: u32_at(c, 0),
            name: String::from_utf8_lossy(&c[4..16]).trim_end_matches('\0').to_owned(),
            literal: std::array::from_fn(|k| f32_at(c, 16 + k * 4)),
        })
        .collect();
    let state_bits = raw(m, "stateBitsTable").chunks_exact(8).map(|b| [u32_at(b, 0), u32_at(b, 4)]).collect();
    z3::Material {
        name: bare(info.and_then(|i| i.string("name")).unwrap_or_default()),
        game_flags: info.map_or(0, |i| i.data[4]),
        sort_key: info.map_or(0, |i| i.data[5]),
        surface_type_bits: info.map_or(0, |i| u32_at(&i.data, 16)),
        state_bits_entry,
        state_flags: m.data[75],
        camera_region: m.data[76],
        technique_set: remap(m.asset("techniqueSet")),
        textures,
        constants,
        state_bits,
    }
}

pub fn image(i: &GNode) -> z3::Image {
    let (width, height, depth) = (u16_at(&i.data, 20), u16_at(&i.data, 22), u16_at(&i.data, 24));
    // (Inline only for lightmaps, probes and the like: the rest are
    // streamed by name.)
    let load_def = i.node("texture").and_then(|t| t.node("loadDef")).filter(|l| u32_at(&l.data, 12) > 0).map(|l| {
        let size = u32_at(&l.data, 12) as usize;
        z3::ImageLoadDef {
            level_count: l.data[0],
            flags: l.data[4],
            dimensions: [width, height, depth],
            format: u32_at(&l.data, 8),
            data: l.data.get(16..16 + size).unwrap_or_default().to_vec(),
        }
    });
    // MW2's reflection probes are plain colour: their alpha is noise, where
    // CoD4's is a brightness scale (rgb * a * 2). Neutral (0.5) instead.
    let name = bare(i.string("name").unwrap_or_default());
    let load_def = load_def.map(|mut d| {
        if name.starts_with("*reflection_probe") && d.format == 21 {
            for px in d.data.chunks_exact_mut(4) {
                px[3] = 128;
            }
        }
        d
    });
    z3::Image {
        name: bare(i.string("name").unwrap_or_default()),
        map_type: match i.data[4] {
            3 => z3::MapType::TwoD,
            4 => z3::MapType::ThreeD,
            5 => z3::MapType::Cube,
            o => z3::MapType::Other(o as u32),
        },
        semantic: i.data[5],
        category: i.data[6],
        width,
        height,
        depth,
        load_def,
    }
}

fn phys_preset(p: &GNode) -> z3::PhysPreset {
    z3::PhysPreset {
        name: p.string("name").unwrap_or_default().to_owned(),
        mass: f32_at(&p.data, 8),
        bounce: f32_at(&p.data, 12),
        friction: f32_at(&p.data, 16),
        bullet_force_scale: f32_at(&p.data, 20),
        explosive_force_scale: f32_at(&p.data, 24),
        pieces_spread_fraction: f32_at(&p.data, 32),
        pieces_upward_velocity: f32_at(&p.data, 36),
        sound_prefix: p.string("sndAliasPrefix").unwrap_or_default().to_owned(),
    }
}

fn com_world(c: &GNode) -> z3::ComWorld {
    let primary_lights = c
        .nodes("primaryLights")
        .iter()
        .map(|l| z3::PrimaryLight {
            kind: l.data[0],
            color: vec3_at(&l.data, 4),
            dir: vec3_at(&l.data, 16),
            origin: vec3_at(&l.data, 28),
            radius: f32_at(&l.data, 40),
            cos_half_fov_outer: f32_at(&l.data, 44),
            cos_half_fov_inner: f32_at(&l.data, 48),
            def_name: l.string("defName").map(str::to_owned),
        })
        .collect();
    z3::ComWorld { name: c.string("name").unwrap_or_default().to_owned(), primary_lights }
}

/// The sun's parameters, as worldspawn gives them.
#[derive(Clone, Copy)]
struct Sun {
    color: [f32; 3],
    light: f32,
    /// Pitch, yaw toward the sun (CoD degrees).
    angles: [f32; 3],
    ambient: f32,
    ambient_color: [f32; 3],
}

fn worldspawn_sun(zone: &Zone) -> Option<Sun> {
    let ents = zone.assets.iter().find(|a| a.ty == AssetType::MapEnts)?;
    let text = String::from_utf8_lossy(&raw(&ents.root, "entityString")).into_owned();
    let ws = iw3::ents::parse(&text).into_iter().find(|e| e.classname() == "worldspawn")?;
    let vec = |k: &str| -> Option<[f32; 3]> {
        let v: Vec<f32> = ws.get(k)?.split_whitespace().filter_map(|x| x.parse().ok()).collect();
        (v.len() >= 3).then(|| [v[0], v[1], v[2]])
    };
    let num = |k: &str| ws.get(k).and_then(|v| v.trim().parse::<f32>().ok());
    Some(Sun {
        color: vec("suncolor")?,
        light: num("sunlight")?,
        angles: vec("sundirection")?,
        ambient: num("ambient").unwrap_or(0.0),
        ambient_color: vec("_color").unwrap_or([1.0; 3]),
    })
}

fn gfx_world(w: &GNode, remap: &dyn Fn(Option<usize>) -> Option<usize>, sun: Option<Sun>) -> z3::GfxWorld {
    let d = &w.data;
    let draw = w.node("draw");
    let dpvs = w.node("dpvs");
    let indices = draw.map(|dr| u16s(&raw(dr, "indices")).collect()).unwrap_or_default();
    let vertices = draw
        .and_then(|dr| dr.node("vd"))
        .map(|vd| {
            raw(vd, "vertices")
                .chunks_exact(44)
                .map(|v| z3::WorldVertex {
                    xyz: vec3_at(v, 0),
                    binormal_sign: f32_at(v, 12),
                    color: u32_at(v, 16),
                    tex_coord: [f32_at(v, 20), f32_at(v, 24)],
                    lmap_coord: [f32_at(v, 28), f32_at(v, 32)],
                    normal: u32_at(v, 36),
                    tangent: u32_at(v, 40),
                })
                .collect()
        })
        .unwrap_or_default();
    let bounds: Vec<[[f32; 3]; 2]> = dpvs.map(|p| p.nodes("surfacesBounds").iter().map(|b| bounds_at(&b.data, 0)).collect()).unwrap_or_default();
    let surfaces = dpvs
        .map(|p| {
            p.nodes("surfaces")
                .iter()
                .enumerate()
                .map(|(i, s)| z3::GfxSurface {
                    first_vertex: u32_at(&s.data, 4) as i32,
                    vertex_count: u16_at(&s.data, 8),
                    tri_count: u16_at(&s.data, 10),
                    base_index: u32_at(&s.data, 12) as i32,
                    material: remap(s.asset("material")),
                    lightmap_index: s.data[20],
                    reflection_probe_index: s.data[21],
                    primary_light_index: s.data[22],
                    flags: s.data[23],
                    bounds: bounds.get(i).copied().unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default();
    let static_models = dpvs
        .map(|p| {
            p.nodes("smodelDrawInsts")
                .iter()
                .map(|s| z3::StaticModel {
                    model: remap(s.asset("model")),
                    origin: vec3_at(&s.data, 0),
                    axis: [vec3_at(&s.data, 12), vec3_at(&s.data, 24), vec3_at(&s.data, 36)],
                    scale: f32_at(&s.data, 48),
                    cull_dist: u16_at(&s.data, 56) as f32,
                    flags: s.data[62],
                })
                .collect()
        })
        .unwrap_or_default();
    let sky = w.nodes("skies").first();
    let sky_start_surfs = sky.map(|s| raw(s, "skyStartSurfs").chunks_exact(4).map(|c| u32_at(c, 0) as i32).collect()).unwrap_or_default();
    let lightmaps = draw
        .map(|dr| {
            dr.nodes("lightmaps").iter().map(|l| z3::LightmapPair { primary: remap(l.asset("primary")), secondary: remap(l.asset("secondary")) }).collect()
        })
        .unwrap_or_default();
    let reflection_probes = draw
        .map(|dr| {
            let images = dr.assets("reflectionProbes");
            dr.nodes("reflectionProbeOrigins")
                .iter()
                .enumerate()
                .map(|(i, o)| z3::ReflectionProbe { origin: vec3_at(&o.data, 0), image: remap(images.get(i).copied().flatten()) })
                .collect()
        })
        .unwrap_or_default();
    let range = |o: usize| dpvs.map_or(0..0, |p| u32_at(&p.data, o)..u32_at(&p.data, o + 4));
    let [mins, maxs] = bounds_at(d, 216);
    let models = w
        .nodes("models")
        .iter()
        .map(|m| z3::GfxBrushModel { bounds: bounds_at(&m.data, 24), surface_count: u16_at(&m.data, 52), start_surf_index: u16_at(&m.data, 54) })
        .collect();
    let light_grid = w.node("lightGrid").map(light_grid).unwrap_or_default();
    let sf = w.node("sun");
    let sun_flare = sf
        .map(|s| {
            let b = &s.data;
            z3::SunFlare {
                valid: b[0] != 0,
                sprite: remap(s.asset("spriteMaterial")),
                flare: remap(s.asset("flareMaterial")),
                sprite_size: f32_at(b, 12),
                flare_min_size: f32_at(b, 16),
                flare_min_dot: f32_at(b, 20),
                flare_max_size: f32_at(b, 24),
                flare_max_dot: f32_at(b, 28),
                flare_max_alpha: f32_at(b, 32),
                flare_fade_in: u32_at(b, 36) as i32,
                flare_fade_out: u32_at(b, 40) as i32,
                blind_min_dot: f32_at(b, 44),
                blind_max_dot: f32_at(b, 48),
                blind_max_darken: f32_at(b, 52),
                blind_fade_in: u32_at(b, 56) as i32,
                blind_fade_out: u32_at(b, 60) as i32,
                glare_min_dot: f32_at(b, 64),
                glare_max_dot: f32_at(b, 68),
                glare_max_lighten: f32_at(b, 72),
                glare_fade_in: u32_at(b, 76) as i32,
                glare_fade_out: u32_at(b, 80) as i32,
                fx_position: vec3_at(b, 84),
            }
        })
        .unwrap_or_default();
    let sun = sun.unwrap_or(Sun { color: [1.0; 3], light: 1.0, angles: [-60.0, 0.0, 0.0], ambient: 0.0, ambient_color: [1.0; 3] });
    z3::GfxWorld {
        name: w.string("name").unwrap_or_default().to_owned(),
        base_name: w.string("baseName").unwrap_or_default().to_owned(),
        indices,
        vertices,
        surfaces,
        static_models,
        sky_start_surfs,
        sky_image: remap(sky.and_then(|s| s.asset("skyImage"))),
        sun: z3::SunParse {
            name: String::from("sun"),
            ambient_scale: sun.ambient,
            ambient_color: sun.ambient_color,
            diffuse_fraction: 0.0,
            sun_light: sun.light,
            sun_color: sun.color,
            diffuse_color: [0.0; 3],
            angles: sun.angles,
        },
        sun_flare,
        sun_color_from_bsp: sun.color.map(|c| c * sun.light),
        lightmaps,
        reflection_probes,
        mins,
        maxs,
        lit_surfs: range(12),
        decal_surfs: range(20),
        emissive_surfs: range(36),
        models,
        light_grid,
    }
}

fn light_grid(g: &GNode) -> z3::LightGrid {
    let d = &g.data;
    z3::LightGrid {
        mins: std::array::from_fn(|k| u16_at(d, 8 + k * 2)),
        maxs: std::array::from_fn(|k| u16_at(d, 14 + k * 2)),
        row_axis: u32_at(d, 20),
        col_axis: u32_at(d, 24),
        row_data_start: u16s(&raw(g, "rowDataStart")).collect(),
        raw_row_data: raw(g, "rawRowData"),
        entries: raw(g, "entries")
            .chunks_exact(4)
            .map(|e| z3::LightGridEntry { colors_index: u16_at(e, 0), primary_light_index: e[2], needs_trace: e[3] })
            .collect(),
        colors: raw(g, "colors")
            .chunks_exact(168)
            .map(|c| z3::LightGridColors(std::array::from_fn(|k| [c[k * 3], c[k * 3 + 1], c[k * 3 + 2]])))
            .collect(),
    }
}

fn clip_map(c: &GNode, remap: &dyn Fn(Option<usize>) -> Option<usize>) -> z3::ClipMap {
    let d = &c.data;
    // The planes (often the world's, loaded with it): where they start, for
    // the sides' and nodes' pointers into them.
    let planes_raw = u32_at(d, 12);
    let planes_base = loc(c, "planes").or_else(|| (planes_raw != 0 && planes_raw < 0xFFFF_FFFE).then(|| planes_raw - 1));
    let planes = raw(c, "planes").chunks_exact(20).map(|p| z3::Plane { normal: vec3_at(p, 0), dist: f32_at(p, 12) }).collect();
    let static_models = c
        .nodes("staticModelList")
        .iter()
        .map(|s| {
            let [absmin, absmax] = bounds_at(&s.data, 52);
            z3::ClipStaticModel {
                model: remap(s.asset("xmodel")),
                origin: vec3_at(&s.data, 4),
                inv_scaled_axis: [vec3_at(&s.data, 16), vec3_at(&s.data, 28), vec3_at(&s.data, 40)],
                absmin,
                absmax,
            }
        })
        .collect();
    let materials = c
        .nodes("materials")
        .iter()
        .map(|m| z3::ClipMaterial { name: m.string("name").unwrap_or_default().to_owned(), surface_flags: u32_at(&m.data, 4) as i32, content_flags: u32_at(&m.data, 8) as i32 })
        .collect();
    let sides = c.nodes("brushsides");
    let sides_base = loc(c, "brushsides");
    let bounds = c.nodes("brushBounds");
    let contents = raw(c, "brushContents");
    let brushes = c
        .nodes("brushes")
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let [mins, maxs] = bounds.get(i).map_or([[0.0; 3]; 2], |n| bounds_at(&n.data, 0));
            let n = u16_at(&b.data, 0) as usize;
            let first = index_into(u32_at(&b.data, 4), sides_base, 8);
            if std::env::var_os("IW4_TRACE_BRUSHES").is_some() && n > 0 && first.is_none() {
                eprintln!("brush {i}: {n} sides, side pointer {:#x} not into brushsides {:?}", u32_at(&b.data, 4), sides_base.map(|v| format!("{v:#x}")));
            }
            let (mut side_planes, mut side_materials) = (Vec::new(), Vec::new());
            if let Some(first) = first {
                for side in sides.iter().skip(first as usize).take(n) {
                    let Some(plane) = index_into(u32_at(&side.data, 0), planes_base, 20) else {
                        if std::env::var_os("IW4_TRACE_BRUSHES").is_some() {
                            eprintln!("brush {i}: side plane {:#x} not into planes {:?}", u32_at(&side.data, 0), planes_base.map(|v| format!("{v:#x}")));
                        }
                        continue;
                    };
                    side_planes.push(plane);
                    side_materials.push(u16_at(&side.data, 4) as u32);
                }
            }
            z3::Brush {
                mins,
                maxs,
                contents: u32_at(&contents, i * 4) as i32,
                side_planes,
                side_materials,
                axial_materials: [
                    [i16_at(&b.data, 12), i16_at(&b.data, 14), i16_at(&b.data, 16)],
                    [i16_at(&b.data, 18), i16_at(&b.data, 20), i16_at(&b.data, 22)],
                ],
            }
        })
        .collect();
    let verts = raw(c, "verts").chunks_exact(12).map(|v| vec3_at(v, 0)).collect();
    let partitions = c.nodes("partitions").iter().map(|p| z3::CollisionPartition { tri_count: p.data[0], first_tri: u32_at(&p.data, 4) as i32 }).collect();
    let aabb_trees = c
        .nodes("aabbTrees")
        .iter()
        .map(|a| z3::CollisionAabbTree {
            origin: vec3_at(&a.data, 0),
            half_size: vec3_at(&a.data, 16),
            material_index: u16_at(&a.data, 12),
            child_count: u16_at(&a.data, 14),
            index: u32_at(&a.data, 28) as i32,
        })
        .collect();
    let cmodels = c
        .nodes("cmodels")
        .iter()
        .map(|m| {
            let [mins, maxs] = bounds_at(&m.data, 0);
            // `leaf` (cLeaf_t) at 28: firstCollAabbIndex, collAabbCount, ...,
            // leafBrushNode at +36.
            z3::CModel {
                mins,
                maxs,
                radius: f32_at(&m.data, 24),
                leaf_brush_node: u32_at(&m.data, 28 + 36) as i32,
                first_coll_aabb: u16_at(&m.data, 28),
                coll_aabb_count: u16_at(&m.data, 30),
            }
        })
        .collect();
    // Leaf brush lists point into `leafbrushes` (u16 brush indices).
    let leaf_brushes: Vec<u16> = u16s(&raw(c, "leafbrushes")).collect();
    let leaf_base = loc(c, "leafbrushes");
    let leaf_brush_nodes = c
        .nodes("leafbrushNodes")
        .iter()
        .map(|n| {
            let count = i16_at(&n.data, 2);
            let brushes = if count > 0 {
                index_into(u32_at(&n.data, 8), leaf_base, 2)
                    .map(|first| leaf_brushes.iter().skip(first as usize).take(count as usize).copied().collect())
                    .or_else(|| {
                        // Loaded inline with the node instead.
                        let inline = n.node("data").and_then(|d| d.node("leaf")).map(|l| u16s(&raw(l, "brushes")).collect::<Vec<_>>());
                        inline.filter(|v| !v.is_empty())
                    })
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            z3::LeafBrushNode { leaf_brush_count: count, brushes, child_offsets: [u16_at(&n.data, 16), u16_at(&n.data, 18)] }
        })
        .collect();
    let dyn_ent_counts = [u16_at(d, 168), u16_at(d, 170)];
    let dyn_ents = c
        .field("dynEntDefList")
        .map(|v| match v {
            GVal::Nodes(n) => n.clone(),
            _ => Vec::new(),
        })
        .unwrap_or_default()
        .iter()
        .map(|e| z3::DynEntDef {
            kind: u32_at(&e.data, 0) as i32,
            quat: std::array::from_fn(|k| f32_at(&e.data, 4 + k * 4)),
            origin: vec3_at(&e.data, 20),
            model: remap(e.asset("xModel")),
            brush_model: u16_at(&e.data, 36),
            physics_brush_model: u16_at(&e.data, 38),
            destroy_fx: None,
            destroy_pieces: None,
            phys_preset: remap(e.asset("physPreset")),
            health: u32_at(&e.data, 48) as i32,
            contents: u32_at(&e.data, 88) as i32,
        })
        .collect();
    z3::ClipMap {
        name: c.string("name").unwrap_or_default().to_owned(),
        planes,
        static_models,
        materials,
        brushes,
        verts,
        tri_indices: u16s(&raw(c, "triIndices")).collect(),
        partitions,
        aabb_trees,
        cmodels,
        map_ents: remap(c.asset("mapEnts")),
        dyn_ent_counts,
        dyn_ents,
        leaf_brush_nodes,
    }
}

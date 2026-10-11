//! Modern Warfare 2 animations (`XAnimParts`) as CoD4's, so `iw3::xanim`
//! (and the game's viewmodel code) can play them unchanged.
//!
//! The keyframe streams (`dataByte` .. `indices`), the part types they are
//! grouped by, the bone names and the notetracks are laid out as CoD4's.
//! What differs:
//! * the header: CoD4's `bLoop` and `bDelta` bools became one `flags` byte
//!   (`ANIM_LOOP` 1, `ANIM_DELTA` 2, `ANIM_DELTA_3D` 4), moving `boneCount`,
//!   `notifyCount`, `assetType` and `isDefault` back a byte;
//! * the root motion: `XAnimDeltaPart` gained a full quaternion track
//!   (`quat`, for `ANIM_DELTA_3D`) beside CoD4's yaw-only one, which it
//!   renamed `quat2`. CoD4 has nowhere for the full one, so it is dropped.

use crate::zone::{AssetType, GNode, GVal, Zone};
use iw3::zone as z3;
use iw3::zone::generic as g3;

const ANIM_LOOP: u8 = 1;
const ANIM_DELTA: u8 = 2;

/// One MW2 `XAnimParts` node as CoD4's.
pub fn to_iw3(a: &GNode) -> g3::GNode {
    let d = &a.data;
    let mut h = vec![0u8; 88];
    if d.len() >= 88 {
        let flags = d[16];
        h[..16].copy_from_slice(&d[..16]); // name, data counts, numframes
        h[16] = (flags & ANIM_LOOP != 0) as u8;
        h[17] = (flags & ANIM_DELTA != 0) as u8;
        h[18..31].copy_from_slice(&d[17..30]); // boneCount[10], notifyCount, assetType, isDefault
        h[32..88].copy_from_slice(&d[32..88]); // counts, framerate, frequency, pointers
    }
    let fields = a
        .fields
        .iter()
        .map(|(k, v)| match (k.as_str(), v) {
            ("deltaPart", GVal::Nodes(n)) => (k.clone(), g3::GVal::Nodes(n.iter().map(delta_part).collect())),
            _ => (k.clone(), gval(v)),
        })
        .collect();
    g3::GNode { ty: a.ty.clone(), data: h, fields }
}

/// `XAnimDeltaPart` {trans, quat2, quat} as CoD4's {trans, quat}.
fn delta_part(p: &GNode) -> g3::GNode {
    let mut data = vec![0u8; 8];
    // (Pointer images: only their being set matters downstream.)
    if p.data.len() >= 8 {
        data.copy_from_slice(&p.data[..8]);
    }
    let fields = p
        .fields
        .iter()
        .filter_map(|(k, v)| match k.as_str() {
            "trans" => Some((k.clone(), gval(v))),
            "quat2" => Some(("quat".to_owned(), gval(v))),
            _ => None,
        })
        .collect();
    g3::GNode { ty: p.ty.clone(), data, fields }
}

/// CoD4's name for a struct MW2 split in two (the yaw-only delta quaternion
/// kept CoD4's layout under a `2` suffix).
fn iw3_type(ty: &str) -> String {
    match ty {
        "XAnimDeltaPartQuat2" | "XAnimDeltaPartQuatData2" | "XAnimDeltaPartQuatDataFrames2" | "XAnimDynamicIndicesQuat2" => ty[..ty.len() - 1].to_owned(),
        _ => ty.to_owned(),
    }
}

fn gnode(n: &GNode) -> g3::GNode {
    g3::GNode { ty: iw3_type(&n.ty), data: n.data.clone(), fields: n.fields.iter().map(|(k, v)| (k.clone(), gval(v))).collect() }
}

fn gval(v: &GVal) -> g3::GVal {
    use g3::GVal as V;
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

/// The animations of a MW2 zone (`common_mp` holds the guns') as a CoD4
/// zone of `XAnimParts`, for `iw3::xanim::XAnim::from_node` with its script
/// strings. Kept apart from the map conversion, which has no use for them.
pub fn to_iw3_anims(zone: &Zone) -> z3::Zone {
    let assets = zone
        .assets
        .iter()
        .filter(|a| a.ty == AssetType::XAnimParts && !a.name.starts_with(','))
        .map(|a| z3::Asset::Generic(z3::GenericAsset { ty: z3::AssetType::XAnimParts, name: a.name.clone(), root: to_iw3(&a.root) }))
        .collect();
    z3::Zone { script_strings: zone.script_strings.clone(), assets, top_level: Vec::new(), stats: Default::default() }
}

//! Modern Warfare 2's multiplayer weapons, as their compiled definitions in
//! `common_mp` hold them (MW2 ships no weapon files): every value of a
//! weapon's `WeaponCompleteDef` and `WeaponDef` by member name, with the
//! names of the models, animations, sounds, materials and tags they point
//! at.
//!
//! MW2 compiles each attachment combination as a weapon of its own
//! (`m4_mp`, `m4_reflex_mp`, `m4_acog_silencer_mp`); they share one view
//! model carrying every attachment, each showing its own parts by the tags
//! it hides, as CoD4's do. Camos are whole models: `gunXModel[camo]` and
//! `worldModel[camo]` (0 none, 1 woodland, ... in `mp/camotable.csv`'s
//! order).
//!
//! Values keep MW2's units: times are integer milliseconds (`iFireTime`),
//! view kicks degrees.

use crate::zone::generic::{GNode, GVal};
use crate::zone::schema::schema;
use crate::zone::{AssetType, Zone};
use std::collections::HashMap;

/// One compiled weapon (`m4_reflex_mp`).
#[derive(Clone, Debug, Default)]
pub struct Weapon {
    pub name: String,
    /// Every scalar, string, asset and sound member of the weapon's
    /// `WeaponCompleteDef` and `WeaponDef` by member name (`iFireTime`,
    /// `fAdsZoomFov`, `fireSoundPlayer`, `killIcon`), as text; float arrays
    /// space separated. The complete def's (the variant's own) win.
    pub values: HashMap<String, String>,
    /// View models by camo (empty where it has none).
    pub gun_models: Vec<String>,
    /// World models by camo.
    pub world_models: Vec<String>,
    pub hand_model: String,
    /// `szXAnims`, by CoD's animation slot (1 idle, 3 fire, 9 reload...).
    pub anims: Vec<String>,
    pub hide_tags: Vec<String>,
    /// Notetrack -> sound alias.
    pub notetrack_sounds: Vec<(String, String)>,
}

impl Weapon {
    /// A value as text ("" when unset).
    pub fn get(&self, key: &str) -> &str {
        self.values.get(key).map_or("", String::as_str)
    }

    pub fn f(&self, key: &str) -> Option<f32> {
        self.get(key).split_whitespace().next()?.parse().ok()
    }

    /// A time member (milliseconds) in seconds.
    pub fn secs(&self, key: &str) -> Option<f32> {
        self.f(key).map(|ms| ms / 1000.0)
    }

    pub fn flag(&self, key: &str) -> bool {
        self.f(key).is_some_and(|v| v != 0.0)
    }

    /// An animation slot's animation, if it has one.
    pub fn anim(&self, slot: usize) -> Option<&str> {
        self.anims.get(slot).map(String::as_str).filter(|a| !a.is_empty())
    }

    /// The view model with camo `camo` (0 none), falling back to none.
    pub fn gun_model(&self, camo: usize) -> &str {
        self.gun_models.get(camo).filter(|m| !m.is_empty()).or(self.gun_models.first()).map_or("", String::as_str)
    }

    pub fn world_model(&self, camo: usize) -> &str {
        self.world_models.get(camo).filter(|m| !m.is_empty()).or(self.world_models.first()).map_or("", String::as_str)
    }
}

/// The multiplayer weapons of a parsed zone (`common_mp`).
pub fn weapons(zone: &Zone) -> Vec<Weapon> {
    zone.of_type(AssetType::Weapon).map(|(_, a)| weapon(zone, &a.root)).collect()
}

fn weapon(zone: &Zone, root: &GNode) -> Weapon {
    let asset_name = |id: Option<usize>| id.map_or(String::new(), |i| zone.assets[i].name.trim_start_matches(',').to_owned());
    let script = |s: u16| zone.script_strings.get(s as usize).cloned().unwrap_or_default();
    let u16s = |b: &[u8]| -> Vec<u16> { b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect() };
    let mut w = Weapon { name: root.string("szInternalName").unwrap_or("").to_owned(), ..Default::default() };
    let def = root.node("weapDef");
    // The def's values first, then the complete def's over them.
    for node in def.into_iter().chain(std::iter::once(root)) {
        values(zone, node, &mut w.values);
    }
    if let Some(def) = def {
        w.gun_models = def.assets("gunXModel").into_iter().map(asset_name).collect();
        w.world_models = def.assets("worldModel").into_iter().map(asset_name).collect();
        w.hand_model = asset_name(def.asset("handXModel"));
        let keys = u16s(def.bytes("notetrackSoundMapKeys"));
        let vals = u16s(def.bytes("notetrackSoundMapValues"));
        w.notetrack_sounds = keys.iter().zip(&vals).filter(|(k, _)| **k != 0).map(|(k, v)| (script(*k), script(*v))).collect();
    }
    w.anims = root.strings("szXAnims").into_iter().map(|a| a.unwrap_or("").to_owned()).collect();
    w.hide_tags = u16s(root.bytes("hideTags")).into_iter().filter(|&s| s != 0).map(script).collect();
    w
}

/// A struct's members as text by name.
fn values(zone: &Zone, node: &GNode, out: &mut HashMap<String, String>) {
    let s = schema();
    let Some(t) = s.types.get(&node.ty) else { return };
    for m in &t.members {
        let Some(name) = m.name.as_deref() else { continue };
        let r = s.resolve(&m.ty);
        let text = if r.ptr + m.ptr == 0 && !s.is_record(&r.base) {
            // Scalars and float arrays (`vStandMove`), from the raw bytes.
            let n: u32 = m.arr.iter().chain(&r.arr).product::<u32>().max(1);
            let size = s.size_align(&r.base).0;
            let at = m.offset as usize;
            (0..n as usize)
                .filter_map(|i| scalar(&r.base, node.data.get(at + i * size as usize..at + (i + 1) * size as usize)?))
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            match node.field(name) {
                Some(GVal::Str(v)) => v.clone().unwrap_or_default(),
                Some(GVal::Asset(id)) => id.map_or(String::new(), |i| zone.assets[i].name.trim_start_matches(',').to_owned()),
                // A sound: `SndAliasCustom` -> its alias name.
                Some(GVal::Nodes(ns)) if m.ty == "SndAliasCustom" => ns
                    .first()
                    .and_then(|n| n.node("name"))
                    .and_then(|n| n.string("soundName"))
                    .unwrap_or("")
                    .to_owned(),
                _ => continue,
            }
        };
        out.insert(name.to_owned(), text);
    }
}

/// A primitive (or enum) value as text.
fn scalar(base: &str, b: &[u8]) -> Option<String> {
    Some(match (base, b.len()) {
        ("float", 4) => f32::from_le_bytes(b.try_into().ok()?).to_string(),
        ("bool" | "char" | "unsigned char" | "byte", 1) => b[0].to_string(),
        (_, 1) => b[0].to_string(),
        (_, 2) => u16::from_le_bytes(b.try_into().ok()?).to_string(),
        (_, 4) => i32::from_le_bytes(b.try_into().ok()?).to_string(),
        _ => return None,
    })
}

/// A 2D material's picture: its image streamed from the iwds by name, or
/// held in the zone (most of MW2's menu pictures and kill icons are).
#[derive(Debug)]
pub enum Picture {
    Streamed(String),
    Inline(iw3::iwi::Iwi),
}

/// The pictures of a zone's materials whose names pass `keep`, by lower-cased
/// material name.
pub fn pictures(zone: &Zone, keep: impl Fn(&str) -> bool) -> HashMap<String, Picture> {
    zone.of_type(AssetType::Material)
        .filter(|(_, a)| !a.name.starts_with(',') && keep(&a.name))
        .filter_map(|(_, a)| {
            let image = a.root.nodes("textureTable").first()?.node("u")?.asset("image")?;
            let img = crate::convert::image(&zone.assets[image].root);
            let picture = match img.load_def.as_ref().and_then(|d| inline_iwi(d)) {
                Some(iwi) => Picture::Inline(iwi),
                None => Picture::Streamed(img.name),
            };
            Some((a.name.to_ascii_lowercase(), picture))
        })
        .collect()
}

/// An in-zone image (D3D format, mips largest first) as an iwi.
fn inline_iwi(d: &iw3::zone::ImageLoadDef) -> Option<iw3::iwi::Iwi> {
    use iw3::iwi::Format;
    let format = match d.format {
        0x3154_5844 => Format::Dxt1,
        0x3354_5844 => Format::Dxt3,
        0x3554_5844 => Format::Dxt5,
        21 | 22 => Format::Argb32,
        50 => Format::L8,
        28 => Format::A8,
        _ => return None,
    };
    let [w, h, _] = d.dimensions.map(u32::from);
    let mut levels = Vec::new();
    let mut at = 0;
    for level in 0..d.level_count.max(1) as u32 {
        let size = format.level_size(w >> level, h >> level);
        levels.push(d.data.get(at..at + size)?.to_vec());
        at += size;
    }
    Some(iw3::iwi::Iwi { format, flags: 0, width: w, height: h, depth: 1, levels })
}

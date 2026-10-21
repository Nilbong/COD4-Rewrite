//! The object palette: every model in the loaded zones (the map's own and
//! CoD4's shared ones) that can stand on its own, sorted into categories by
//! its name. Guns, viewmodels, characters and effects' markers are left out.

use crate::content::Content;
use iw3::zone::Asset;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Category {
    All,
    Props,
    Vehicles,
    Military,
    Furniture,
    Plants,
    Lights,
    Characters,
    Effects,
    Spawns,
}

pub const CATEGORIES: [Category; 10] = [
    Category::All,
    Category::Props,
    Category::Vehicles,
    Category::Military,
    Category::Furniture,
    Category::Plants,
    Category::Lights,
    Category::Characters,
    Category::Effects,
    Category::Spawns,
];

impl Category {
    pub fn label(self) -> &'static str {
        match self {
            Category::All => "All",
            Category::Props => "Props",
            Category::Vehicles => "Vehicles",
            Category::Military => "Military",
            Category::Furniture => "Furniture",
            Category::Plants => "Plants",
            Category::Lights => "Lights",
            Category::Characters => "Characters",
            Category::Effects => "FX",
            Category::Spawns => "Spawns",
        }
    }
}

/// A model's category, by its name.
pub fn category(name: &str) -> Category {
    if name == super::layout::LIGHT {
        return Category::Lights;
    } else if name.starts_with("@fx:") {
        return Category::Effects;
    } else if name.starts_with("@spawn:") {
        return Category::Spawns;
    } else if name.starts_with(super::actors::PREFIX) {
        return Category::Characters;
    }
    let n = name.to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| n.contains(w));
    if has(&["foliage", "tree", "bush", "grass", "plant", "shrub", "palm", "fern", "flower", "weed", "hedge", "ivy", "leaves", "cactus"]) {
        Category::Plants
    } else if has(&["vehicle", "_car", "car_", "truck", "bus_", "tank", "bmp", "humvee", "tractor", "sedan", "pickup", "van_", "heli", "boat", "jeep", "uaz", "bike", "blackhawk", "ch46", "hind", "cobra", "mig29", "harrier", "zpu", "bm21"]) {
        Category::Vehicles
    } else if has(&[
        "mil_", "military", "sandbag", "barrier", "hesco", "jersey", "barricade", "ammo", "missile", "mortar", "bunker", "camo_net", "cammo", "plasticcase", "tent", "radio", "foxhole",
        "tank_trap", "hedgehog", "razor", "concertina", "barbed", "roadblock", "rocket", "bomb", "javelin", "stinger", "launcher", "antenna", "army", "ordnance", "satchel", "flak",
    ]) {
        Category::Military
    } else if has(&[
        "chair", "table", "bed", "desk", "shelf", "couch", "cabinet", "bench", "sofa", "locker", "stool", "mattress", "dresser", "bookcase", "tv_", "television", "computer", "monitor",
        "filing", "fridge", "refrigerator", "stove", "sink", "toilet", "bathtub", "rug", "carpet", "curtain", "piano", "clock", "picture", "painting", "laptop", "phone", "fan_", "kitchen",
    ]) {
        Category::Furniture
    } else if has(&["lamp", "light", "lantern", "bulb", "flood"]) {
        Category::Lights
    } else {
        Category::Props
    }
}

/// Whether a model is a placeable prop rather than a gun, a hand, a body,
/// a destroyed piece or an effect's marker.
fn placeable(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    const SKIP: &[&str] = &[
        "viewmodel", "weapon_", "worldmodel", "body_", "head_", "playerbody", "viewhands", "gib", "tag_", "fx", "_d_", "_dam", "destroyed", "chunk",
        "piece", "shard", "projectile", "shell", "clip_", "_mag", "hardpoint", "flag", "c4_", "claymore", "grenade", "mp_body", "char_", "ghillie",
        "default", "defaultactor", "defaultvehicle",
    ];
    // A vehicle's lamps, wrecks, cheap and thermal copies.
    const VEHICLE_SKIP: &[&str] = &["_light_", "dest", "dstry", "dsty", "thermal", "_glow", "drawing", "_test", "tankcrush", "damaged", "_window", "_hub", "_axle", "animated"];
    let vehicle_part = n.starts_with("vehicle_") && (VEHICLE_SKIP.iter().any(|s| n.contains(s)) || n.ends_with("_low") || n.ends_with("_dam"));
    !n.starts_with('*') && !n.starts_with("w_") && !n.starts_with("t5_") && !n.starts_with("iw4_") && !SKIP.iter().any(|s| n.contains(s)) && !vehicle_part
}

/// Every placeable model: the loaded zones' (sorted, each once), then CoD4's
/// other levels' ([`super::stream`]), the characters, lights, the zones'
/// scenery effects and spawn points.
pub fn models(content: &Content) -> Vec<String> {
    let mut out: Vec<String> = content
        .zones
        .iter()
        .flat_map(|z| z.assets.iter())
        .filter_map(|a| match a {
            Asset::XModel(m) if !m.lods.is_empty() && placeable(&m.name) => Some(m.name.clone()),
            _ => None,
        })
        .collect();
    out.sort();
    out.dedup();
    let here: std::collections::HashSet<String> = out.iter().map(|m| m.to_ascii_lowercase()).collect();
    let idx = super::stream::index();
    let mut more: Vec<String> = idx
        .models
        .iter()
        .filter(|(name, e)| {
            // (Nothing huge: a level's terrain or a building's shell.)
            let size = e.max - e.min;
            !here.contains(*name) && placeable(name) && size.max_element() < 40.0 && size.max_element() > 0.05 && (e.bones < 2 || category(name) == Category::Vehicles)
        })
        .map(|(n, _)| n.clone())
        .collect();
    more.sort();
    out.extend(more);
    out.extend(super::actors::characters().into_iter().map(|c| format!("{}{}", super::actors::PREFIX, c.id)));
    // Lights, the zones' looping scenery effects, and spawn points.
    out.push(super::layout::LIGHT.to_owned());
    let mut fx: Vec<String> = content
        .zones
        .iter()
        .flat_map(|z| z.assets.iter())
        .filter_map(|a| match a {
            Asset::Generic(g) if g.ty == iw3::zone::AssetType::Fx && scenery_fx(&g.name) => Some(format!("@fx:{}", g.name)),
            _ => None,
        })
        .collect();
    fx.sort();
    fx.dedup();
    out.extend(fx);
    out.extend(["@spawn:tdm", "@spawn:allies", "@spawn:axis", "@spawn:dm"].map(String::from));
    out
}

/// The loaded zones' models' names, lower case.
pub fn loaded(content: &Content) -> std::collections::HashSet<String> {
    content
        .zones
        .iter()
        .flat_map(|z| z.assets.iter())
        .filter_map(|a| match a {
            Asset::XModel(m) if !m.lods.is_empty() => Some(m.name.to_ascii_lowercase()),
            _ => None,
        })
        .collect()
}

/// Effects worth placing as scenery (fires, smoke, steam, sparks, drips),
/// not impacts, muzzle flashes or explosions.
fn scenery_fx(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    const WANT: &[&str] = &["fire", "smoke", "steam", "spark", "drip", "leak", "dust", "ember", "ash", "insect", "bird", "light", "glow", "water", "rain", "flies", "trash", "paper", "leaves"];
    const SKIP: &[&str] = &["impact", "muzzle", "flash", "explo", "shell", "tracer", "blood", "grenade", "rocket", "fog", "mist", "haze"];
    WANT.iter().any(|w| n.contains(w)) && !SKIP.iter().any(|w| n.contains(w))
}

/// A shorter name to show: without the usual prefixes.
pub fn label(name: &str) -> String {
    if name == super::layout::LIGHT {
        return "light".into();
    }
    if let Some(f) = name.strip_prefix("@fx:") {
        return f.rsplit('/').next().unwrap_or(f).replace('_', " ");
    }
    if let Some(k) = name.strip_prefix("@spawn:") {
        return format!("spawn: {}", if k == "tdm" { "any team" } else { k });
    }
    if let Some(spec) = super::actors::Spec::parse(name) {
        return spec.character().map_or(spec.character.clone(), |c| c.name.to_owned());
    }
    let mut s = name;
    for p in ["com_", "ch_", "me_", "bc_", "mil_", "prop_", "foliage_", "vehicle_"] {
        if let Some(rest) = s.strip_prefix(p) {
            s = rest;
            break;
        }
    }
    s.replace('_', " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories_by_name() {
        assert_eq!(category("com_barrel_blue"), Category::Props);
        assert_eq!(category("vehicle_80s_sedan1_yel"), Category::Vehicles);
        assert_eq!(category("vehicle_ch46e"), Category::Vehicles);
        assert_eq!(category("foliage_tree_palm_bushy_1"), Category::Plants);
        assert_eq!(category("mil_sandbag_desert_long"), Category::Military);
        assert_eq!(category("com_plasticcase_beige_big"), Category::Military);
        assert_eq!(category("com_bottle4"), Category::Props);
        assert_eq!(category("@actor:gaz"), Category::Characters);
        assert!(!placeable("vehicle_80s_sedan1_yel_light_lb"));
        assert!(!placeable("vehicle_bmp_dsty"));
        assert!(placeable("vehicle_humvee_camo"));
        assert!(!placeable("viewmodel_m4"));
        assert!(!placeable("weapon_ak47"));
        assert!(placeable("com_barrel_blue"));
    }
}

/// Whether `name` (or its label) has every word of `query`.
pub fn matches(name: &str, query: &str) -> bool {
    let hay = format!("{} {}", name.to_ascii_lowercase(), label(name).to_ascii_lowercase());
    query.split_whitespace().all(|w| hay.contains(&w.to_ascii_lowercase()))
}

//! What Black Ops brings, ready to use: the multiplayer guns with their
//! attachments and the view hands (all in `common_mp`), and each faction's
//! character bodies and heads (in the map zones, see [`CHARACTER_ZONES`]).

use crate::weapons::WeaponFile;
use std::collections::BTreeMap;

/// A multiplayer gun and the attachments it can take.
#[derive(Clone, Debug)]
pub struct Gun {
    /// `ak47` (its weapon file is `ak47_mp`).
    pub name: String,
    /// Localization key, e.g. `WEAPON_AK47`.
    pub display_name: String,
    /// `rifle`, `smg`, `mg`, `spread`, `pistol` or `rocketlauncher`.
    pub class: String,
    pub view_model: String,
    pub world_model: String,
    pub hands: String,
    /// Attachment -> its variant's weapon file, e.g. `reflex` ->
    /// `ak47_reflex_mp`. Dual wield (`dw`) is the `<gun>dw_mp` pair.
    pub attachments: BTreeMap<String, String>,
}

/// The zone with the view hands' materials (`common_mp` refers to them).
pub const HANDS_ZONE: &str = "code_post_gfx_mp";

/// Maps that between them carry every multiplayer faction: CIA and
/// Spetsnaz, their winter gear, SOG and NVA, and the Cuban rebels and Tropas.
pub const CHARACTER_ZONES: [&str; 4] = ["mp_nuked", "mp_array", "mp_cracked", "mp_firingrange"];

/// A team look: its bodies and heads (`c_usa_cia_mp_body_flak`, ...).
#[derive(Clone, Debug, Default)]
pub struct Faction {
    /// `usa_cia`, `rus_spet`, ...
    pub name: String,
    /// The zone its models are in.
    pub zone: String,
    pub bodies: Vec<String>,
    pub heads: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Catalog {
    pub guns: Vec<Gun>,
    pub factions: Vec<Faction>,
    pub view_hands: Vec<String>,
}

const GUN_CLASSES: &[&str] = &["rifle", "smg", "mg", "spread", "pistol", "rocketlauncher"];
/// Carried killstreaks and the fallback weapon aren't loadout guns.
const NOT_GUNS: &[&str] = &["defaultweapon", "minigun"];

impl Catalog {
    /// Build from the multiplayer weapon files, the models in `common_mp`
    /// and those of the character zones, `(zone, models)`. Weapon files
    /// name models in any case; the catalog uses the zone's spelling.
    pub fn build(weapons: &[WeaponFile], common: &[&str], maps: &[(&str, Vec<&str>)]) -> Catalog {
        let by_name: BTreeMap<&str, &WeaponFile> = weapons.iter().map(|w| (w.name.as_str(), w)).collect();
        let known: BTreeMap<String, &str> = common.iter().map(|m| (m.to_ascii_lowercase(), *m)).collect();
        let model = |name: &str| known.get(&name.to_ascii_lowercase()).map(|m| m.to_string());
        let mut guns = Vec::new();
        for w in weapons {
            let Some(base) = w.name.strip_suffix("_mp").filter(|b| !b.contains('_')) else { continue };
            let gun_like = w.get("inventoryType") == "primary"
                && matches!(w.get("weaponType"), "bullet" | "projectile")
                && GUN_CLASSES.contains(&w.get("weaponClass"));
            // Dual wield halves (`aspdw`, `asplh`) belong to their gun.
            let half = ["dw", "lh"].iter().any(|s| base.strip_suffix(s).is_some_and(|b| by_name.contains_key(format!("{b}_mp").as_str())));
            if !gun_like || half || NOT_GUNS.contains(&base) {
                continue;
            }
            // Some files are leftovers without models (`ks23`, `mp40`).
            let (Some(view_model), Some(world_model)) = (model(w.gun_model()), model(w.world_model())) else { continue };
            let mut attachments = BTreeMap::new();
            for v in weapons {
                let Some(att) = v.name.strip_prefix(&format!("{base}_")).and_then(|r| r.strip_suffix("_mp")) else { continue };
                // Same gun model with different tags hidden: an attachment.
                if !att.contains('_') && v.gun_model() == w.gun_model() {
                    attachments.insert(att.to_owned(), v.name.clone());
                }
            }
            if by_name.contains_key(format!("{base}dw_mp").as_str()) {
                attachments.insert("dw".to_owned(), format!("{base}dw_mp"));
            }
            guns.push(Gun {
                name: base.to_owned(),
                display_name: w.get("displayName").to_owned(),
                class: w.get("weaponClass").to_owned(),
                view_model,
                world_model,
                hands: model(w.hand_model()).unwrap_or_else(|| w.hand_model().to_owned()),
                attachments,
            });
        }

        // Characters: `c_<country>_<unit>_mp_body_<kind>` and `..._mp_head_<n>`.
        let mut factions: BTreeMap<String, Faction> = BTreeMap::new();
        for (zone, models) in maps {
            for &m in models {
                let Some(rest) = m.strip_prefix("c_") else { continue };
                let Some((faction, part)) = rest.split_once("_mp_") else { continue };
                let f = factions.entry(faction.to_owned()).or_insert_with(|| Faction {
                    name: faction.to_owned(),
                    zone: zone.to_string(),
                    ..Default::default()
                });
                let list = if part.starts_with("body") {
                    &mut f.bodies
                } else if part.starts_with("head") {
                    &mut f.heads
                } else {
                    continue;
                };
                if f.zone == *zone && !list.iter().any(|x| x == m) {
                    list.push(m.to_owned());
                }
            }
        }
        let mut factions: Vec<Faction> = factions.into_values().filter(|f| !f.bodies.is_empty()).collect();
        for f in &mut factions {
            f.bodies.sort();
            f.heads.sort();
        }
        let view_hands = common.iter().filter(|m| m.starts_with("viewhands")).map(|m| m.to_string()).collect();
        Catalog { guns, factions, view_hands }
    }

    /// Every model the catalog names.
    pub fn models(&self) -> impl Iterator<Item = &str> {
        let guns = self.guns.iter().flat_map(|g| [g.view_model.as_str(), g.world_model.as_str()]);
        let people = self.factions.iter().flat_map(|f| f.bodies.iter().chain(&f.heads).map(String::as_str));
        guns.chain(people).chain(self.view_hands.iter().map(String::as_str))
    }
}

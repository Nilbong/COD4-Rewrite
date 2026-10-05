//! What World at War brings, ready to use: the multiplayer guns with their
//! attachments and the equipment (all in `common_mp`), and the four multiplayer factions'
//! characters — body, head and first-person arms per class — which live in
//! the per-map team zones (see [`CHARACTER_ZONES`]).
//!
//! Unlike CoD4 and Black Ops, World at War's weapon files name no view
//! hands (`viewmodel_hands_no_model`): the arms belong to the character
//! (`setViewmodel` in its script), so they are listed on [`Character`].

use crate::weapons::WeaponFile;
use std::collections::BTreeMap;

/// A multiplayer gun and the attachments it can take.
#[derive(Clone, Debug)]
pub struct Gun {
    /// `thompson` (its weapon file is `thompson_mp`).
    pub name: String,
    /// Localization key, e.g. `WEAPON_THOMPSON`.
    pub display_name: String,
    /// `rifle`, `smg`, `mg`, `spread`, `pistol` or `rocketlauncher`.
    pub class: String,
    pub view_model: String,
    pub world_model: String,
    /// Attachment -> its variant's weapon file, e.g. `aperture` ->
    /// `thompson_aperture_mp`. A variant shows its parts by hiding the
    /// others' tags on the shared view model; some (`silenced`, `bayonet`,
    /// `flash`) also have their own world model, named in that file.
    pub attachments: BTreeMap<String, String>,
}

/// A non-primary weapon with a model: grenades, the bazooka and the
/// flamethrower (World at War's perk-slot weapons), satchel charges, mines.
#[derive(Clone, Debug)]
pub struct Equipment {
    /// `bazooka` (its weapon file is `bazooka_mp`).
    pub name: String,
    pub display_name: String,
    /// `grenade`, `rocketlauncher`, `gas`, `smoke`, ...
    pub class: String,
    pub view_model: String,
    pub world_model: String,
}

/// The team zones for each pairing: Marines (Raiders) against the Imperial
/// Japanese Army on the Pacific maps, the Red Army against the Wehrmacht on
/// the European ones. Every Pacific / European map's team zone has the same
/// characters; these are the smallest.
pub const CHARACTER_ZONES: [&str; 2] = ["localized_mp_makin", "localized_mp_dome"];

/// One character as its script (`character/char_usa_raider_player_rifle.gsc`)
/// puts it together.
#[derive(Clone, Debug)]
pub struct Character {
    /// `usa_raider_rifle`.
    pub name: String,
    /// `usa_raider`, `jap_impinf`, `rus_guard` or `ger_hnrgd`.
    pub faction: String,
    /// The class look: `rifle`, `cqb`, `assault`, `lmg` or `smg`.
    pub class: String,
    pub body: String,
    pub head: String,
    /// First-person arms, e.g. `viewmodel_usa_marine_arms`.
    pub view_arms: String,
    /// The zone holding the models.
    pub zone: String,
}

#[derive(Clone, Debug, Default)]
pub struct Catalog {
    pub guns: Vec<Gun>,
    pub equipment: Vec<Equipment>,
    pub characters: Vec<Character>,
}

const GUN_CLASSES: &[&str] = &["rifle", "smg", "mg", "spread", "pistol", "rocketlauncher"];
/// The fallback weapon isn't a loadout gun.
const NOT_GUNS: &[&str] = &["defaultweapon"];
const NOT_EQUIPMENT: &[&str] = &["napalmblob", "frag_grenade_short"];

impl Catalog {
    /// Build from the multiplayer weapon files, the models in `common_mp`,
    /// the character scripts `(name, text)` (in `common_mp`) and the models
    /// of the character zones `(zone, models)`. Weapon files name models in
    /// any case; the catalog uses the zone's spelling.
    pub fn build(weapons: &[WeaponFile], common: &[&str], scripts: &[(&str, &str)], maps: &[(&str, Vec<&str>)]) -> Catalog {
        let known: BTreeMap<String, &str> = common.iter().map(|m| (m.to_ascii_lowercase(), *m)).collect();
        let model = |name: &str| known.get(&name.to_ascii_lowercase()).map(|m| m.to_string());
        let mut guns = Vec::new();
        let mut equipment = Vec::new();
        for w in weapons {
            let Some(base) = w.name.strip_suffix("_mp") else { continue };
            if matches!(w.get("inventoryType"), "offhand" | "item") {
                // Killstreak radios have no view model; the flamethrower's
                // fire and the cooked grenade aren't equipment of their own.
                let real = !w.gun_model().ends_with("no_model") && !NOT_EQUIPMENT.contains(&base);
                if let (true, Some(view_model), Some(world_model)) = (real, model(w.gun_model()), model(w.world_model())) {
                    equipment.push(Equipment {
                        name: base.to_owned(),
                        display_name: w.get("displayName").to_owned(),
                        class: w.get("weaponClass").to_owned(),
                        view_model,
                        world_model,
                    });
                }
                continue;
            }
            if base.contains('_') {
                continue;
            }
            let gun_like = w.get("inventoryType") == "primary"
                && matches!(w.get("weaponType"), "bullet" | "projectile")
                && GUN_CLASSES.contains(&w.get("weaponClass"));
            if !gun_like || NOT_GUNS.contains(&base) {
                continue;
            }
            let (Some(view_model), Some(world_model)) = (model(w.gun_model()), model(w.world_model())) else { continue };
            let mut attachments = BTreeMap::new();
            for v in weapons {
                let Some(att) = v.name.strip_prefix(&format!("{base}_")).and_then(|r| r.strip_suffix("_mp")) else { continue };
                // Same gun model with other tags hidden: an attachment.
                // (`bipod_stand` and friends are the deployed states.)
                if !att.contains('_') && v.gun_model().eq_ignore_ascii_case(w.gun_model()) {
                    attachments.insert(att.to_owned(), v.name.clone());
                }
            }
            guns.push(Gun {
                name: base.to_owned(),
                display_name: w.get("displayName").to_owned(),
                class: w.get("weaponClass").to_owned(),
                view_model,
                world_model,
                attachments,
            });
        }

        // `character/char_<faction>_player_<class>.gsc`; its models must be
        // in one of the character zones.
        let mut characters = Vec::new();
        for (path, text) in scripts {
            let Some(name) = path.strip_prefix("character/char_").and_then(|n| n.strip_suffix(".gsc")) else { continue };
            let Some((faction, class)) = name.split_once("_player_") else { continue };
            let (Some(body), Some(head), Some(view_arms)) =
                (quoted_after(text, "setModel("), quoted_after(text, "self.headModel ="), quoted_after(text, "setViewmodel("))
            else {
                continue;
            };
            let Some((zone, _)) = maps.iter().find(|(_, models)| [&body, &head, &view_arms].iter().all(|m| models.contains(&m.as_str()))) else {
                continue;
            };
            characters.push(Character {
                name: format!("{faction}_{class}"),
                faction: faction.to_owned(),
                class: class.to_owned(),
                body,
                head,
                view_arms,
                zone: zone.to_string(),
            });
        }
        characters.sort_by(|a, b| a.name.cmp(&b.name));
        Catalog { guns, equipment, characters }
    }

    /// Every model the catalog names.
    pub fn models(&self) -> impl Iterator<Item = &str> {
        let guns = self.guns.iter().flat_map(|g| [g.view_model.as_str(), g.world_model.as_str()]);
        let equipment = self.equipment.iter().flat_map(|e| [e.view_model.as_str(), e.world_model.as_str()]);
        let people = self.characters.iter().flat_map(|c| [c.body.as_str(), c.head.as_str(), c.view_arms.as_str()]);
        guns.chain(equipment).chain(people)
    }
}

/// The first `"..."` after `key` in a script.
fn quoted_after(text: &str, key: &str) -> Option<String> {
    let rest = &text[text.find(key)? + key.len()..];
    let start = rest.find('"')? + 1;
    let len = rest[start..].find('"')?;
    Some(rest[start..start + len].to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_character_scripts() {
        let script = "main()\n{\n\tself setModel(\"body_a\");\n\tself.headModel = \"head_a\";\n\tself attach(self.headModel, \"\", true);\n\tself setViewmodel(\"arms_a\");\n}";
        let maps = [("zone_a", vec!["body_a", "head_a", "arms_a"])];
        let c = Catalog::build(&[], &[], &[("character/char_usa_raider_player_rifle.gsc", script)], &maps);
        let c = &c.characters[0];
        assert_eq!((c.faction.as_str(), c.class.as_str(), c.body.as_str(), c.head.as_str()), ("usa_raider", "rifle", "body_a", "head_a"));
        assert_eq!((c.view_arms.as_str(), c.zone.as_str()), ("arms_a", "zone_a"));
    }
}

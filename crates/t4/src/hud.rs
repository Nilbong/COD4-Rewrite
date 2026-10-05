//! World at War's HUD art, sorted by what it is for: crosshairs, the hit
//! marker, the minimap and its pings, damage indicators, weapon and kill
//! icons, ammo counters, stances, perks, ranks, team insignia, objective
//! markers and load screens.
//!
//! They are materials like any other (2D ones; their images are in the
//! iwds): most are in `localized_common_mp` and `common_mp`, the rank icons
//! and menu art in `code_post_gfx_mp`, and each map's own minimap
//! ([`minimap`]) in the map's zone. A weapon's crosshair pieces and icons
//! are named in its weapon file ([`Crosshair`]).

use crate::weapons::WeaponFile;
use std::collections::BTreeMap;

/// The zones holding the shared HUD art.
pub const HUD_ZONES: [&str; 3] = ["localized_common_mp", "common_mp", "code_post_gfx_mp"];

/// Groups and the material name prefixes that belong to them. The longest
/// matching prefix decides (`hud_icon_bomb` is an objective, not a weapon
/// icon), then the earlier group.
pub const GROUPS: &[(&str, &[&str])] = &[
    ("crosshair", &["reticle_", "hud_flamethrower_reticle"]),
    ("hitmarker", &["damage_feedback"]),
    ("damage", &["hit_direction", "overlay_low_health"]),
    ("scope", &["scope_overlay"]),
    (
        "minimap",
        &[
            "minimap_",
            "compass_map_",
            "compass_overlay_map_",
            "compass_radarline",
            "compassping_",
            "compass_flag_",
            "compass_noflag_",
            "compass_objpoint_",
            "compass_squad_",
            "compass_waypoint_",
        ],
    ),
    ("grenade", &["hud_grenade", "hud_us_grenade", "hud_us_smokegrenade"]),
    ("weapon_icons", &["hud_icon_", "weapon_"]),
    ("kill_icons", &["killicon"]),
    ("ammo", &["ammo_counter_", "hud_bullets_"]),
    ("stance", &["stance_"]),
    ("perks", &["specialty_"]),
    ("ranks", &["rank_"]),
    ("teams", &["faction_", "hudicon_", "headicon", "mpflag_", "scorebar_", "score_bar_"]),
    ("objectives", &["waypoint_", "objpoint_", "objective", "hud_icon_bomb", "hud_suitcase_bomb"]),
    ("killstreaks", &["hud_dog_", "hud_dpad_", "map_artillery_selector", "map_squad_command", "compass_objpoint_dogs"]),
    ("momentum", &["hud_momentum", "hud_blitzkrieg", "hud_xpticker"]),
    ("load_screens", &["loadscreen_"]),
    ("menu", &["menu_", "ui_", "logo", "gradient", "line_", "progress_bar_"]),
];

/// The HUD materials of some zones, grouped by [`GROUPS`].
#[derive(Clone, Debug, Default)]
pub struct Hud {
    pub groups: BTreeMap<&'static str, Vec<String>>,
}

impl Hud {
    /// Group material names (anything that matches no group is left out).
    /// `,name` references to another zone's copy are skipped.
    pub fn build<'a>(materials: impl IntoIterator<Item = &'a str>) -> Hud {
        let mut groups: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
        for m in materials {
            if m.starts_with(',') {
                continue;
            }
            let found = GROUPS.iter().filter(|(_, prefixes)| prefixes.iter().any(|p| m.starts_with(p))).min_by_key(|(g, prefixes)| {
                let specific = prefixes.iter().filter(|p| m.starts_with(**p)).map(|p| p.len()).max().unwrap_or(0);
                (std::cmp::Reverse(specific), GROUPS.iter().position(|(n, _)| n == g))
            });
            if let Some((group, _)) = found {
                let list = groups.entry(group).or_default();
                if !list.iter().any(|x| x == m) {
                    list.push(m.to_owned());
                }
            }
        }
        for list in groups.values_mut() {
            list.sort();
        }
        Hud { groups }
    }

    pub fn group(&self, name: &str) -> &[String] {
        self.groups.get(name).map_or(&[], Vec::as_slice)
    }
}

/// A map's minimap material (in the map's own zone), e.g.
/// `compass_map_mp_airfield`.
pub fn minimap(map: &str) -> String {
    format!("compass_map_{map}")
}

/// A weapon's crosshair and icons, from its weapon file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Crosshair {
    /// Drawn at the centre (grenades and launchers: `reticle_center_cross`).
    pub center: Option<String>,
    /// Drawn four times around the centre, spread by accuracy (guns:
    /// `reticle_side_small`).
    pub side: Option<String>,
    /// Sizes in 640x480 screen units.
    pub center_size: f32,
    pub side_size: f32,
    /// The side pieces' smallest distance from the centre.
    pub min_offset: f32,
    /// A full-screen overlay while aiming down the sights (`scope_overlay_mp`).
    pub ads_overlay: Option<String>,
    pub hud_icon: Option<String>,
    pub kill_icon: Option<String>,
}

impl Crosshair {
    pub fn of(w: &WeaponFile) -> Crosshair {
        let name = |k: &str| Some(w.get(k).trim()).filter(|v| !v.is_empty() && *v != "none").map(str::to_owned);
        Crosshair {
            center: name("reticleCenter"),
            side: name("reticleSide"),
            center_size: w.float("reticleCenterSize"),
            side_size: w.float("reticleSideSize"),
            min_offset: w.float("reticleMinOfs"),
            ads_overlay: name("adsOverlayShader"),
            hud_icon: name("hudIcon"),
            kill_icon: name("killIcon"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_materials() {
        let hud = Hud::build(["damage_feedback", "compass_map_mp_airfield", "hud_icon_bomb", "hud_icon_thompson", ",white", "sun"]);
        assert_eq!(hud.group("hitmarker"), ["damage_feedback"]);
        assert_eq!(hud.group("minimap"), ["compass_map_mp_airfield"]);
        assert_eq!(hud.group("objectives"), ["hud_icon_bomb"]);
        assert_eq!(hud.group("weapon_icons"), ["hud_icon_thompson"]);
        assert_eq!(hud.groups.values().map(Vec::len).sum::<usize>(), 4);
    }

    #[test]
    fn reads_crosshairs() {
        let w = WeaponFile::parse("x_mp", "WEAPONFILE\\reticleSide\\reticle_side_small\\reticleSideSize\\8\\adsOverlayShader\\none\\hudIcon\\hud_icon_x").unwrap();
        let c = Crosshair::of(&w);
        assert_eq!((c.side.as_deref(), c.center, c.side_size, c.ads_overlay), (Some("reticle_side_small"), None, 8.0, None));
        assert_eq!(c.hud_icon.as_deref(), Some("hud_icon_x"));
    }
}

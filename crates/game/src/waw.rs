//! World at War (WaW) guns in the game: Create a Class lists them next to
//! CoD4's and Black Ops', and matches use them like any other gun.
//!
//! Everything comes from the user's World at War install ([`t4`]): the guns,
//! their attachments, names, pictures and attribute bars from its own
//! `mp/statstable.csv`, `mp/attachmenttable.csv` and `mp/attributestable.csv`
//! (in `code_post_gfx_mp`, laid out like CoD4's) and English strings
//! (`localized_code_post_gfx_mp`, `localized_common_mp`); stats, models,
//! animations and sounds from the plain-text weapon files
//! (`weapons/mp/thompson_aperture_mp`); models and animations from its
//! `common_mp` ([`load_content`], a [`Content`] of its own, like Black Ops').
//!
//! In the game a WaW gun is `t4_<name>` (`t4_thompson:aperture`), with a
//! weapon index from [`FIRST_INDEX`] in CoD4's stats table. A WaW gun takes
//! one attachment, as in WaW; each is a variant weapon file of its own
//! (`thompson_aperture_mp`) on the same view model, showing its part by the
//! tags it hides, and some (bayonet, flash hider, suppressor, scope, bipod)
//! with a world model of their own.
//!
//! WaW's multiplayer has no weapon camos: its attachment table still carries
//! CoD4's camo rows (renamed, with blank descriptions), but no gun names a
//! camo model, no camo textures or pictures ship, and its class menu has no
//! camo choice. So WaW guns have none here either.

use crate::content::Content;
use crate::explosives::ExplosiveDef;
use crate::weapons::{AdsOverlay, Reticle, WeaponDef, WeaponSounds};
use bevy::prelude::*;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use t4::weapons::WeaponFile;

/// WaW guns' names in the game start with this.
pub const PREFIX: &str = "t4_";
/// Their weapon indices in CoD4's stats table start here (CoD4's end at
/// 149, Black Ops' are 400..600); weapon `i` keeps its unlock bits in stat
/// `3000 + i`.
pub const FIRST_INDEX: i32 = 600;
/// Material names the UI looks up in WaW's own zones and iwds
/// ([`Data::material_image`]): its pictures and kill icons.
pub const MATERIAL_PREFIX: &str = "t4/";

/// Is this stats table index a WaW gun's?
pub fn is_index(index: i32) -> bool {
    (FIRST_INDEX..FIRST_INDEX + 200).contains(&index)
}

/// Is this a WaW gun (`t4_thompson`, `t4_thompson:aperture`)?
pub fn is_waw(weapon: &str) -> bool {
    weapon.starts_with(PREFIX)
}

/// World at War's sound aliases go into the bank under `t4/<alias>`, apart
/// from CoD4's (both games name some sounds alike, `weap_raise_plr`).
pub fn sound_alias(name: &str) -> String {
    format!("t4/{}", name.to_ascii_lowercase())
}

/// WaW's weapon groups (`mp/statstable.csv` column 2) that matches can use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Group {
    BoltAction,
    Rifle,
    Smg,
    Shotgun,
    Lmg,
    Pistol,
}

impl Group {
    fn from_table(group: &str) -> Option<Group> {
        Some(match group {
            "weapon_sniper" => Group::BoltAction,
            "weapon_assault" => Group::Rifle,
            "weapon_smg" => Group::Smg,
            "weapon_shotgun" => Group::Shotgun,
            // The heavy machine guns (M1919, MG42) are in the machine gun
            // list.
            "weapon_lmg" | "weapon_hmg" => Group::Lmg,
            "weapon_pistol" => Group::Pistol,
            _ => return None,
        })
    }

    /// The primary weapon groups, in WaW's menu order.
    pub const PRIMARY: [Group; 5] = [Group::BoltAction, Group::Rifle, Group::Smg, Group::Shotgun, Group::Lmg];

    /// The group name (stats table column 2; WaW's are CoD4's) and WaW's
    /// string for its heading.
    pub fn table(self) -> (&'static str, &'static str) {
        match self {
            Group::BoltAction => ("weapon_sniper", "MPUI_BOLT_ACTION_RIFLES"),
            Group::Rifle => ("weapon_assault", "MPUI_RIFLES"),
            Group::Smg => ("weapon_smg", "MPUI_SUB_MACHINE_GUNS"),
            Group::Shotgun => ("weapon_shotgun", "MPUI_SHOTGUNS"),
            Group::Lmg => ("weapon_lmg", "MPUI_LIGHT_MACHINE_GUNS"),
            Group::Pistol => ("weapon_pistol", "MPUI_PISTOLS"),
        }
    }

    /// A CoD4 gun of the same kind, whose sounds fill any a WaW gun lacks.
    fn sound_standin(self) -> &'static str {
        match self {
            Group::BoltAction => "m40a3_mp",
            Group::Rifle => "m14_mp",
            Group::Smg => "mp5_mp",
            Group::Shotgun => "winchester1200_mp",
            Group::Lmg => "rpd_mp",
            Group::Pistol => "colt45_mp",
        }
    }

    /// CoD4's `weapClass` (WaW's files call the M1919 and FG42 SMGs).
    fn class(self) -> i32 {
        match self {
            Group::Lmg => 1,
            Group::Smg => 2,
            Group::Shotgun => 3,
            Group::Pistol => 4,
            Group::BoltAction | Group::Rifle => 0,
        }
    }
}

/// A WaW gun.
#[derive(Clone, Debug)]
pub struct Gun {
    pub index: i32,
    /// WaW's name, `thompson` (the game's is [`Gun::id`]).
    pub name: String,
    pub group: Group,
    /// "Thompson", from WaW's strings, and its key (`WEAPON_THOMPSON`).
    pub display: String,
    pub key: String,
    pub description_key: String,
    /// WaW's picture material (`weapon_thompson`).
    pub picture: String,
    /// The attachments it takes, in WaW's order, that have a weapon file.
    pub attachments: Vec<String>,
}

impl Gun {
    /// The game's name for it: `t4_thompson`.
    pub fn id(&self) -> String {
        format!("{PREFIX}{}", self.name)
    }
}

/// A WaW attachment (`mp/attachmenttable.csv`).
#[derive(Clone, Debug)]
pub struct Attachment {
    pub name: String,
    /// "Aperture Sight", and its key (`MPUI_APERTURE`).
    pub display: String,
    pub key: String,
    /// Its description's key (`PERKS_APERTURE`).
    pub description_key: String,
    pub picture: String,
}

pub struct Data {
    pub install: t4::Install,
    pub vfs: Arc<iw3::iwd::Vfs>,
    pub guns: Vec<Gun>,
    pub attachments: Vec<Attachment>,
    /// Weapon files by variant name (`thompson_aperture_mp`).
    weapons: HashMap<String, WeaponFile>,
    /// WaW's English strings by key.
    pub strings: HashMap<String, String>,
    /// WaW's attribute bars (`mp/attributestable.csv`): (group, name, [accuracy,
    /// damage, range, fire rate, mobility]) for its guns and gun_attachment
    /// changes.
    pub attributes: Vec<(String, String, [i32; 5])>,
    /// Picture materials (`localized_common_mp`) -> their images.
    pictures: HashMap<String, String>,
    /// HUD materials (kill icons, in `common_mp`) -> their images, once the
    /// guns' content has loaded.
    hud: Mutex<HashMap<String, String>>,
}

static DATA: OnceLock<Option<Data>> = OnceLock::new();

/// WaW's data, loaded on first use (`None` without a World at War install).
pub fn data() -> Option<&'static Data> {
    DATA.get_or_init(|| match Data::load() {
        Ok(d) => {
            log::info!("waw: {} guns, {} attachments", d.guns.len(), d.attachments.len());
            Some(d)
        }
        Err(e) => {
            log::info!("waw: World at War guns unavailable: {e:#}");
            None
        }
    })
    .as_ref()
}

/// Start loading WaW's data in the background, then its guns' sounds (so
/// they are ready when a match starts).
pub fn preload() {
    std::thread::spawn(|| {
        if let Some(d) = data() {
            let _ = d.sound_aliases();
        }
    });
}

/// WaW's gun sounds, once read ([`Data::sound_aliases`]).
static SOUNDS: OnceLock<Vec<(String, Vec<crate::audio::bank::Variant>)>> = OnceLock::new();

/// A string table's rows from a parsed WaW zone.
fn table(zone: &t4::zone::Zone, name: &str) -> Vec<Vec<String>> {
    let Some((_, a)) = zone.of_type(t4::zone::AssetType::StringTable).find(|(_, a)| a.name.eq_ignore_ascii_case(name)) else {
        return Vec::new();
    };
    let cols = a.root.int("columnCount").max(0) as usize;
    let rows = a.root.int("rowCount").max(0) as usize;
    let cells: Vec<String> = a.root.strings("values").into_iter().map(|s| s.unwrap_or("").to_owned()).collect();
    (0..rows).map(|r| (0..cols).map(|c| cells.get(r * cols + c).cloned().unwrap_or_default()).collect()).collect()
}

/// Each 2D material of a converted zone -> its image.
fn material_images(zone: &iw3::zone::Zone, keep: impl Fn(&str) -> bool) -> HashMap<String, String> {
    zone.assets
        .iter()
        .filter_map(|a| match a {
            iw3::zone::Asset::Material(m) if !m.name.starts_with(',') && keep(&m.name) => {
                let image = m.textures.first()?.image.and_then(|i| zone.image(i))?;
                Some((m.name.to_ascii_lowercase(), image.name.clone()))
            }
            _ => None,
        })
        .collect()
}

impl Data {
    fn load() -> anyhow::Result<Data> {
        let t0 = std::time::Instant::now();
        let install = t4::Install::locate()?;
        let vfs = Arc::new(install.vfs()?);
        let weapons: HashMap<String, WeaponFile> =
            t4::weapons::mp_weapons(&vfs).into_iter().map(|w| (w.name.to_ascii_lowercase(), w)).collect();
        let parse = |name: &str| -> anyhow::Result<t4::zone::Zone> {
            Ok(t4::zone::Zone::parse(&t4::fastfile::load(&install.zone_path(name))?, Default::default())?)
        };
        let gfx = parse("code_post_gfx_mp")?;
        let localized = parse("localized_common_mp")?;
        let mut strings = HashMap::new();
        for zone in [parse("localized_code_post_gfx_mp").ok().as_ref(), Some(&localized)].into_iter().flatten() {
            for (_, a) in zone.of_type(t4::zone::AssetType::LocalizeEntry) {
                strings.insert(a.name.to_ascii_uppercase(), a.root.string("value").unwrap_or("").to_owned());
            }
        }
        let text = |key: &str| strings.get(&key.to_ascii_uppercase()).cloned().unwrap_or_else(|| key.to_owned());
        // The menus' pictures (guns' and attachments').
        let pictures = material_images(&t4::convert::to_iw3(&localized), |m| m.starts_with("weapon_"));

        // [index, unlock stat, group, name key, name, -, picture, description
        // key, attachments, ...], like CoD4's. The heavy machine guns come
        // after the light ones (and the DP-28, in both, once).
        let mut rows: Vec<(bool, i32, Vec<String>)> = table(&gfx, "mp/statstable.csv")
            .into_iter()
            .filter_map(|row| Some((row.get(2)? == "weapon_hmg", row.first()?.parse().ok()?, row)))
            .collect();
        rows.sort_by_key(|(hmg, index, _)| (*hmg, *index));
        let mut guns: Vec<Gun> = Vec::new();
        for (_, waw_index, row) in rows {
            let cell = |c: usize| row.get(c).map_or("", String::as_str);
            let (Some(group), name) = (Group::from_table(cell(2)), cell(4)) else { continue };
            if name.is_empty() || !weapons.contains_key(&format!("{name}_mp")) || guns.iter().any(|g| g.name == name) {
                continue;
            }
            let attachments = cell(8)
                .split_whitespace()
                .map(str::to_ascii_lowercase)
                .filter(|a| weapons.contains_key(&format!("{name}_{a}_mp")))
                .collect();
            guns.push(Gun {
                index: FIRST_INDEX + waw_index,
                name: name.to_owned(),
                group,
                display: text(cell(3)),
                key: cell(3).to_owned(),
                description_key: cell(7).to_owned(),
                picture: cell(6).to_owned(),
                attachments,
            });
        }

        // [index, -, type, name key, name, -, picture, description, ...].
        let mut attachments = Vec::new();
        for row in table(&gfx, "mp/attachmenttable.csv") {
            let cell = |c: usize| row.get(c).map_or("", String::as_str);
            if cell(2).starts_with("attachment_") && !cell(4).is_empty() && cell(4) != "none" {
                attachments.push(Attachment {
                    name: cell(4).to_ascii_lowercase(),
                    display: text(cell(3)),
                    key: cell(3).to_owned(),
                    description_key: format!("PERKS_{}", cell(7)),
                    picture: cell(6).to_owned(),
                });
            }
        }

        // [group, name, accuracy, damage, range, fire rate, mobility].
        let attributes = table(&gfx, "mp/attributestable.csv")
            .into_iter()
            .filter_map(|row| {
                let values: Vec<i32> = row.get(2..7)?.iter().map(|v| v.trim().parse().ok()).collect::<Option<_>>()?;
                let name = row.get(1).filter(|n| !n.is_empty())?;
                Some((row[0].clone(), name.to_ascii_lowercase(), values.try_into().ok()?))
            })
            .collect();
        log::info!("waw: tables and weapon files read in {:.2?}", t0.elapsed());
        Ok(Data { install, vfs, guns, attachments, weapons, strings, attributes, pictures, hud: Mutex::new(HashMap::new()) })
    }

    /// A gun by the game's name (`t4_thompson`).
    pub fn gun(&self, id: &str) -> Option<&Gun> {
        let name = id.strip_prefix(PREFIX)?;
        self.guns.iter().find(|g| g.name.eq_ignore_ascii_case(name))
    }

    pub fn attachment(&self, name: &str) -> Option<&Attachment> {
        self.attachments.iter().find(|a| a.name.eq_ignore_ascii_case(name))
    }

    /// A weapon file by variant name (`thompson_aperture_mp`).
    pub fn weapon(&self, variant: &str) -> Option<&WeaponFile> {
        self.weapons.get(&variant.to_ascii_lowercase())
    }

    /// The image behind a WaW material the UI draws (`t4/weapon_thompson`
    /// without its prefix): a picture or, once the guns' content has
    /// loaded, a kill icon.
    pub fn material_image(&self, material: &str) -> Option<String> {
        let key = material.to_ascii_lowercase();
        self.pictures.get(&key).cloned().or_else(|| self.hud.lock().ok()?.get(&key).cloned())
    }

    /// The gun an id belongs to: a rifle's grenade launcher
    /// (`t4_gl_kar98k`) belongs to the rifle.
    fn owner(&self, id: &str) -> Option<&Gun> {
        match id.strip_prefix(PREFIX).and_then(|n| n.strip_prefix("gl_")) {
            Some(rifle) => self.gun(&format!("{PREFIX}{rifle}")),
            None => self.gun(id),
        }
    }

    /// A gun's base weapon file and those of its attachments. A launcher's
    /// is its own (`gl_kar98k_mp`: the rifle's viewmodel with the grenade
    /// on, and its own animations).
    fn files(&self, id: &str, attachments: &[&str]) -> Option<(&Gun, &WeaponFile, Vec<&WeaponFile>)> {
        if let Some(rifle) = id.strip_prefix(PREFIX).and_then(|n| n.strip_prefix("gl_")) {
            return Some((self.owner(id)?, self.weapon(&format!("gl_{rifle}_mp"))?, Vec::new()));
        }
        let gun = self.gun(id)?;
        let base = self.weapon(&format!("{}_mp", gun.name))?;
        let variants = attachments.iter().filter_map(|a| self.weapon(&format!("{}_{a}_mp", gun.name))).collect();
        Some((gun, base, variants))
    }

    /// Stats of a WaW gun with its attachment (`t4_thompson`, ["aperture"]):
    /// the attachment's own weapon file (WaW guns take one), or with several,
    /// the base file with each one's changes on top, like CoD4 combinations
    /// (see `loadout`).
    pub fn weapon_def(&self, id: &str, attachments: &[&str]) -> Option<WeaponDef> {
        let (gun, base_file, variants) = self.files(id, attachments)?;
        let base = def_from_file(base_file, gun.group);
        let mut out = base.clone();
        for file in variants {
            crate::loadout::merge(&mut out, &base, &def_from_file(file, gun.group));
        }
        out.name = format!("{}_{}mp", id, attachments.iter().map(|a| format!("{a}_")).collect::<String>());
        out.display_key = gun.display.clone();
        Some(out)
    }

    /// A WaW gun's view (or world) model and the tags it hides: its base
    /// weapon file's, less the parts each attachment's variant shows, plus
    /// what they hide (the iron sight under a scope, the magazine under a
    /// drum). A variant with a world model of its own (bayonet, flash hider,
    /// suppressor, scope, bipod) has its part built in there.
    pub fn model(&self, id: &str, attachments: &[&str], world: bool) -> Option<(String, BTreeSet<String>)> {
        let (_, base, variants) = self.files(id, attachments)?;
        let tags = |w: &WeaponFile| -> BTreeSet<String> { w.hide_tags().iter().map(|t| t.to_ascii_lowercase()).collect() };
        let base_hide = tags(base);
        let (mut shown, mut hidden) = (BTreeSet::new(), BTreeSet::new());
        let mut model = if world { base.world_model() } else { base.gun_model() };
        for variant in variants {
            let variant_hide = tags(variant);
            shown.extend(base_hide.difference(&variant_hide).cloned());
            hidden.extend(variant_hide.difference(&base_hide).cloned());
            let own = if world { variant.world_model() } else { variant.gun_model() };
            if !own.is_empty() {
                model = own;
            }
        }
        let hide = base_hide.difference(&shown).cloned().collect::<BTreeSet<_>>().union(&hidden).cloned().collect();
        Some((model.to_owned(), hide))
    }

    /// The CoD4 gun whose sounds fill any a WaW gun lacks.
    pub fn sound_standin(&self, id: &str) -> Option<&'static str> {
        self.owner(id).map(|g| g.group.sound_standin())
    }

    /// The rifle grenades (each rifle's `gl` attachment): the launcher's
    /// weapon name (`t4_gl_kar98k_mp`) and its projectile, with `standin`'s
    /// trail, explosion and sound (WaW's effects aren't loaded).
    pub fn rifle_grenades(&self, standin: &ExplosiveDef) -> Vec<(String, ExplosiveDef)> {
        self.guns
            .iter()
            .filter(|g| g.attachments.iter().any(|a| a == "gl"))
            .filter_map(|g| {
                let alt = self.weapon(&format!("{}_gl_mp", g.name))?.get("altWeapon").trim().to_owned();
                let w = self.weapon(&alt)?;
                let f = |k: &str| w.get(k).trim().parse::<f32>().unwrap_or(0.0);
                let name = |k: &str| Some(w.get(k).trim()).filter(|v| !v.is_empty() && *v != "none").map(str::to_owned);
                let def = ExplosiveDef {
                    behaviour: crate::explosives::Behaviour::Launched,
                    speed: f("projectileSpeed"),
                    speed_up: f("projectileSpeedUp"),
                    delay: f("fireDelay"),
                    arming: f("projectileActivateDist"),
                    impact_damage: f("damage"),
                    radius: f("explosionRadius"),
                    inner_damage: f("explosionInnerDamage"),
                    outer_damage: f("explosionOuterDamage"),
                    model: name("projectileModel").map_or_else(String::new, |m| format!("{MATERIAL_PREFIX}{m}")),
                    display: "Rifle Grenade",
                    icon: name("killIcon").map_or_else(|| standin.icon.clone(), |i| format!("{MATERIAL_PREFIX}{i}")),
                    ..standin.clone()
                };
                Some((format!("{PREFIX}{alt}"), def))
            })
            .collect()
    }

    /// The sound aliases WaW's guns use (fire, dry fire, raise, put away,
    /// third-person reloads, and the reload foley their animations' notes
    /// name), with the layers they play along, as the game's (under
    /// [`sound_alias`] names). The first call reads them (a few seconds;
    /// [`preload`] does it early), later ones share the sounds.
    pub fn sound_aliases(&self) -> anyhow::Result<Vec<(String, Vec<crate::audio::bank::Variant>)>> {
        if let Some(list) = SOUNDS.get() {
            return Ok(list.clone());
        }
        let list = self.read_sound_aliases()?;
        Ok(SOUNDS.get_or_init(|| list).clone())
    }

    /// Read the guns' sound aliases from `common_mp` (the sounds) and
    /// `code_post_gfx_mp` (the mixer's falloff curves).
    fn read_sound_aliases(&self) -> anyhow::Result<Vec<(String, Vec<crate::audio::bank::Variant>)>> {
        let t0 = std::time::Instant::now();
        let parse = |name: &str| -> anyhow::Result<t4::zone::Zone> {
            Ok(t4::zone::Zone::parse(&t4::fastfile::load(&self.install.zone_path(name))?, Default::default())?)
        };
        let mixer = t4::sound::Mixer::from_zone(&parse("code_post_gfx_mp")?);
        let common = parse("common_mp")?;
        let mut names = HashSet::new();
        for gun in &self.guns {
            let files = std::iter::once(format!("{}_mp", gun.name)).chain(gun.attachments.iter().map(|a| format!("{}_{a}_mp", gun.name)));
            for w in files.filter_map(|f| self.weapon(&f)) {
                for key in SOUND_FIELDS {
                    names.insert(w.get(key).trim().to_ascii_lowercase());
                }
                // `notetrackSoundMap`: note and alias pairs, a pair a line.
                names.extend(w.get("notetrackSoundMap").split_whitespace().map(str::to_ascii_lowercase));
            }
        }
        names.remove("");
        let list = crate::audio::bank::waw_aliases(std::slice::from_ref(&common), mixer.as_ref(), &self.vfs, &names);
        log::info!("waw: {} gun sound aliases in {:.2?}", list.len(), t0.elapsed());
        Ok(list)
    }
}

/// The weapon file fields naming sounds the game plays.
const SOUND_FIELDS: [&str; 10] = [
    "fireSound",
    "fireSoundPlayer",
    "lastShotSound",
    "lastShotSoundPlayer",
    "emptyFireSound",
    "emptyFireSoundPlayer",
    "reloadSound",
    "raiseSoundPlayer",
    "putawaySoundPlayer",
    "rechamberSoundPlayer",
];

/// A [`WeaponDef`] from a WaW weapon file. Like Black Ops', WaW gives times
/// in seconds and view kick in hundredths of a degree. Bolt actions (and the
/// trench gun's pump) work the action after each shot, so a shot takes the
/// rechamber time too; the trench gun loads a round at a time (a segmented
/// reload).
fn def_from_file(w: &WeaponFile, group: Group) -> WeaponDef {
    let fb = WeaponDef::fallback();
    let f = |k: &str| w.get(k).trim().parse::<f32>().ok();
    let fo = |k: &str, d: f32| f(k).unwrap_or(d);
    let kick = |a: &str, b: &str| {
        let (x, y) = (fo(a, 0.0) / 100.0, fo(b, 0.0) / 100.0);
        (x.min(y), x.max(y))
    };
    let clip = w.int("clipSize").max(1) as u32;
    let bolt = w.get("boltAction").trim() == "1";
    let fire_time = fo("fireTime", fb.fire_time) + if bolt { fo("rechamberTime", 0.0) } else { 0.0 };
    let name = |k: &str| Some(w.get(k).trim()).filter(|v| !v.is_empty() && *v != "none").unwrap_or("").to_owned();
    let sound = |k: &str| Some(w.get(k).trim()).filter(|v| !v.is_empty()).map_or_else(String::new, sound_alias);
    // CoD4's `szXAnims` slots (see `viewmodel::anim_slot`).
    const ANIMS: [(usize, &str); 22] = [
        (1, "idleAnim"),
        (2, "emptyIdleAnim"),
        (3, "fireAnim"),
        (5, "lastShotAnim"),
        (6, "rechamberAnim"),
        (7, "meleeAnim"),
        (9, "reloadAnim"),
        (10, "reloadEmptyAnim"),
        (11, "reloadStartAnim"),
        (12, "reloadEndAnim"),
        (13, "raiseAnim"),
        (14, "firstRaiseAnim"),
        (15, "dropAnim"),
        (18, "quickRaiseAnim"),
        (19, "quickDropAnim"),
        (22, "sprintInAnim"),
        (23, "sprintLoopAnim"),
        (24, "sprintOutAnim"),
        (28, "adsFireAnim"),
        (29, "adsLastShotAnim"),
        (31, "adsUpAnim"),
        (32, "adsDownAnim"),
    ];
    let mut xanims = vec![None; 33];
    for (slot, key) in ANIMS {
        xanims[slot] = w.anim(key).map(str::to_owned);
    }
    // The game plays one animation a shot: a bolt action's (or pump's) is
    // working the action, its sounds and all.
    if bolt {
        let rechamber = w.anim("rechamberAnim");
        xanims[3] = rechamber.or(w.anim("fireAnim")).map(str::to_owned);
        xanims[28] = w.anim("adsRechamberAnim").or(rechamber).or(w.anim("adsFireAnim")).map(str::to_owned);
    }
    WeaponDef {
        name: w.name.clone(),
        display_name: "",
        damage: fo("damage", fb.damage),
        min_damage: fo("minDamage", fb.min_damage),
        max_damage_range: fo("maxDamageRange", fb.max_damage_range),
        min_damage_range: fo("minDamageRange", fb.min_damage_range),
        fire_time: fire_time.max(0.01),
        fire_type: crate::weapons::fire_type_named(w.get("fireType")),
        // Already in the fire time (its rechamber animation is its fire one).
        rechamber_time: 0.0,
        clip_size: clip,
        max_ammo: w.int("maxAmmo").max(0) as u32,
        reload_time: fo("reloadTime", fb.reload_time),
        reload_empty_time: fo("reloadEmptyTime", fo("reloadTime", fb.reload_empty_time)),
        segmented_reload: w.get("segmentedReload").trim() == "1",
        reload_start_time: fo("reloadStartTime", 0.0),
        reload_start_add_time: fo("reloadStartAddTime", 0.0),
        reload_add_time: fo("reloadAddTime", 0.0),
        reload_end_time: fo("reloadEndTime", 0.0),
        reload_start_add: w.int("reloadStartAdd").max(0) as u32,
        reload_ammo_add: w.int("reloadAmmoAdd").max(1) as u32,
        ads_trans_in: fo("adsTransInTime", fb.ads_trans_in),
        ads_trans_out: fo("adsTransOutTime", fb.ads_trans_out),
        sprint_in_time: fo("sprintInTime", fb.sprint_in_time),
        sprint_loop_time: fo("sprintLoopTime", fb.sprint_loop_time),
        sprint_out_time: fo("sprintOutTime", fb.sprint_out_time),
        drop_time: fo("dropTime", fb.drop_time),
        raise_time: fo("raiseTime", fb.raise_time),
        first_raise_time: fo("firstRaiseTime", fb.first_raise_time),
        quick_drop_time: fo("quickDropTime", fb.quick_drop_time),
        quick_raise_time: fo("quickRaiseTime", fb.quick_raise_time),
        hip_spread_min: [fo("hipSpreadStandMin", 3.0), fo("hipSpreadDuckedMin", 2.5), fo("hipSpreadProneMin", 2.0)],
        hip_spread_max: [fo("hipSpreadMax", 7.0), fo("hipSpreadDuckedMax", 6.0), fo("hipSpreadProneMax", 5.0)],
        hip_spread_fire_add: fo("hipSpreadFireAdd", fb.hip_spread_fire_add),
        hip_spread_move_add: fo("hipSpreadMoveAdd", fb.hip_spread_move_add),
        hip_spread_decay: fo("hipSpreadDecayRate", fb.hip_spread_decay),
        ads_spread: fo("adsSpread", 0.0),
        kick_pitch: kick("hipViewKickPitchMin", "hipViewKickPitchMax"),
        kick_yaw: kick("hipViewKickYawMin", "hipViewKickYawMax"),
        ads_kick_pitch: kick("adsViewKickPitchMin", "adsViewKickPitchMax"),
        ads_kick_yaw: kick("adsViewKickYawMin", "adsViewKickYawMax"),
        move_speed_scale: fo("moveSpeedScale", 1.0),
        ads_move_speed_scale: fo("adsMoveSpeedScale", 1.0),
        ads_fov: fo("adsZoomFov", 50.0),
        ads_view_bob_mult: fo("adsViewBobMult", fb.ads_view_bob_mult),
        ads_bob_factor: fo("adsBobFactor", fb.ads_bob_factor),
        hip_idle_amount: fo("hipIdleAmount", fb.hip_idle_amount),
        hip_idle_speed: fo("hipIdleSpeed", fb.hip_idle_speed),
        ads_idle_amount: fo("adsIdleAmount", fb.ads_idle_amount),
        ads_idle_speed: fo("adsIdleSpeed", fb.ads_idle_speed),
        idle_crouch_factor: fo("idleCrouchFactor", 1.0),
        idle_prone_factor: fo("idleProneFactor", 0.4),
        viewmodel: w.gun_model().to_owned(),
        world_model: w.world_model().to_owned(),
        xanims,
        // WaW's own (from its zones, see `Data::sound_aliases`); a CoD4
        // stand-in fills any gap (see `Data::sound_standin`).
        sounds: WeaponSounds {
            fire: sound("fireSound"),
            fire_player: sound("fireSoundPlayer"),
            fire_last: sound("lastShotSound"),
            fire_last_player: sound("lastShotSoundPlayer"),
            empty: sound("emptyFireSound"),
            empty_player: sound("emptyFireSoundPlayer"),
            reload: sound("reloadSound"),
            raise_player: sound("raiseSoundPlayer"),
            putaway_player: sound("putawaySoundPlayer"),
        },
        penetrate_type: crate::weapons::penetrate_type_named(w.get("penetrateType")),
        impact_type: match w.get("impactType") {
            "bullet_small" => 1,
            "bullet_ap" => 3,
            "shotgun" | "shotgun_ap" => 4,
            _ => 2,
        },
        class: group.class(),
        // Drawn from WaW's own materials (see `Data::material_image`).
        kill_icon: Some(name("killIcon")).filter(|n| !n.is_empty()).map_or_else(String::new, |n| format!("{MATERIAL_PREFIX}{n}")),
        kill_icon_ratio: w.get("killIconRatio").split(':').next().and_then(|r| r.trim().parse().ok()).unwrap_or(1),
        ammo_counter: match w.get("ammoCounterClip").to_ascii_lowercase().as_str() {
            "shortmagazine" => 2,
            "shotgun" => 3,
            "rocket" => 4,
            "beltfed" => 5,
            _ => 1,
        },
        // CoD4's own crosshair pieces (WaW's guns name CoD4's).
        reticle: Reticle {
            side: name("reticleSide"),
            side_size: fo("reticleSideSize", 8.0),
            min_ofs: fo("reticleMinOfs", 0.0),
            center: name("reticleCenter"),
            center_size: fo("reticleCenterSize", 0.0),
        },
        display_key: w.get("displayName").to_owned(),
        // The scoped rifles' scope (WaW's own material).
        ads_overlay: Some(name("adsOverlayShader")).filter(|n| !n.is_empty()).map(|n| AdsOverlay {
            material: format!("{MATERIAL_PREFIX}{n}"),
            width: fo("adsOverlayWidth", 480.0),
            height: fo("adsOverlayHeight", 480.0),
        }),
    }
}

/// How a WaW gun is carried at a sprint: WaW has no sprint animations for
/// its multiplayer guns, but moves the gun into a pose and sways it while
/// sprinting, from its weapon file.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SprintPose {
    /// `sprintRotP/Y/R`: degrees of pitch (down), yaw (left) and roll.
    pub rot: Vec3,
    /// `sprintOfsF/R/U`: inches forward, along the view's `axis[1]` (left)
    /// and up.
    pub ofs: Vec3,
    /// `sprintBobH/V`: the most the sway reaches across and up and down,
    /// degrees.
    pub bob: Vec2,
}

/// A WaW gun's sprint pose (its weapon def's name, `t4_thompson_mp`): the
/// variant's own weapon file (an attachment's can differ, as the rifle
/// grenade's does), else the gun's.
pub fn sprint_pose(def_name: &str) -> Option<SprintPose> {
    let rest = def_name.strip_prefix(PREFIX)?;
    let data = data()?;
    let w = data.weapon(rest).or_else(|| {
        let gun = data.guns.iter().filter(|g| rest.starts_with(&format!("{}_", g.name))).max_by_key(|g| g.name.len())?;
        data.weapon(&format!("{}_mp", gun.name))
    })?;
    let f = |k: &str| w.get(k).trim().parse::<f32>().unwrap_or(0.0);
    Some(SprintPose {
        rot: Vec3::new(f("sprintRotP"), f("sprintRotY"), f("sprintRotR")),
        ofs: Vec3::new(f("sprintOfsF"), f("sprintOfsR"), f("sprintOfsU")),
        bob: Vec2::new(f("sprintBobH"), f("sprintBobV")),
    })
}

/// WaW's gun content: `common_mp` (guns and their animations) as an `iw3`
/// zone, with WaW's iwds. Its kill icons and scope pictures become drawable
/// ([`Data::material_image`]). Takes a few seconds; run it off the main
/// thread.
pub fn load_content() -> anyhow::Result<Content> {
    let data = data().ok_or_else(|| anyhow::anyhow!("no World at War install"))?;
    let t0 = std::time::Instant::now();
    let zone = t4::load_iw3(&data.install, "common_mp")?;
    let icons =
        material_images(&zone, |m| m.starts_with("hud_icon_") || m.starts_with("killicon") || m.starts_with("scope_overlay"));
    if let Ok(mut hud) = data.hud.lock() {
        hud.extend(icons);
    }
    log::info!("waw: gun content loaded in {:.2?}", t0.elapsed());
    Ok(Content::new(vec![zone], data.vfs.clone()))
}

/// A WaW model by its weapon file's name: weapon files spell names in any
/// case, the zone in its own.
pub fn model_name(content: &Content, name: &str) -> Option<String> {
    if content.find(name).is_some() {
        return Some(name.to_owned());
    }
    content.zones.iter().find_map(|z| {
        z.assets.iter().find_map(|a| match a {
            iw3::zone::Asset::XModel(x) if x.name.eq_ignore_ascii_case(name) => Some(x.name.clone()),
            _ => None,
        })
    })
}

/// WaW's gun content for matches: loaded in the background when a match
/// starts (if World at War is installed), kept for later matches.
#[derive(Resource, Default)]
pub enum MatchContent {
    #[default]
    NotStarted,
    Loading(std::thread::JoinHandle<anyhow::Result<Content>>),
    Ready(Box<Content>),
    Failed,
}

impl MatchContent {
    /// Start loading, unless it has been already.
    pub fn start(&mut self) {
        if matches!(self, MatchContent::NotStarted) && data().is_some() {
            *self = MatchContent::Loading(std::thread::spawn(load_content));
        }
    }

    /// The content once loaded (starting the load if needed).
    pub fn get(&mut self) -> Option<&mut Content> {
        self.start();
        if let MatchContent::Loading(task) = self {
            if task.is_finished() {
                let MatchContent::Loading(task) = std::mem::replace(self, MatchContent::Failed) else { unreachable!() };
                match task.join() {
                    Ok(Ok(c)) => *self = MatchContent::Ready(Box::new(c)),
                    Ok(Err(e)) => log::warn!("waw: gun content unavailable: {e:#}"),
                    Err(_) => log::warn!("waw: loading gun content panicked"),
                }
            }
        }
        match self {
            MatchContent::Ready(c) => Some(c),
            _ => None,
        }
    }

    /// Still loading.
    pub fn busy(&self) -> bool {
        matches!(self, MatchContent::Loading(t) if !t.is_finished())
    }
}

pub struct WawPlugin;

impl Plugin for WawPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MatchContent>()
            .add_systems(OnEnter(crate::state::GameState::InGame), |mut c: ResMut<MatchContent>| c.start())
            .add_systems(OnEnter(crate::state::GameState::InGame), add_rifle_grenades.in_set(crate::state::Setup::Spawn));
    }
}

/// A rifle's grenade launcher (its `gl` attachment): the launcher's weapon
/// (`t4_gl_kar98k_mp`, the alternate weapon on 5) and how long the rifle
/// takes to switch to it.
pub fn launcher(weapon: &str) -> Option<(String, f32)> {
    let data = data()?;
    let gun = data.gun(weapon)?;
    let alt = data.weapon(&format!("{}_gl_mp", gun.name))?.get("altWeapon").trim().to_owned();
    let raise = data.weapon(&alt)?.get("altRaiseTime").trim().parse::<f32>().ok().filter(|&t| t > 0.0).unwrap_or(0.6);
    Some((format!("{PREFIX}{alt}"), raise))
}

/// The rifle grenades fire through [`crate::explosives`], as CoD4's M203
/// does (after its own launchers are known, `Setup::Content`).
fn add_rifle_grenades(mut defs: ResMut<crate::explosives::ExplosiveDefs>) {
    let (Some(data), Some(standin)) = (data(), defs.get("gl_m16_mp").cloned()) else { return };
    let grenades = data.rifle_grenades(&standin);
    log::info!("waw: {} rifle grenades", grenades.len());
    for (weapon, def) in grenades {
        defs.insert(weapon, def);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indices() {
        assert!(is_index(610) && !is_index(410) && !is_index(10));
        assert!(is_waw("t4_thompson") && !is_waw("t5_ak47"));
        assert_eq!(sound_alias("Weap_Thompson_Fire_Plr"), "t4/weap_thompson_fire_plr");
    }

    /// Against the local install: `cargo test -p game waw -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn reads_install() {
        let d = data().expect("World at War install");
        for g in &d.guns {
            let def = d.weapon_def(&g.id(), &[]).expect("weapon def");
            println!(
                "{:>3} {:<22} {:?} {:<18} {} dmg, {:.0} rpm, clip {} / {}, {:?}",
                g.index, g.name, g.group, g.display, def.damage, 60.0 / def.fire_time, def.clip_size, def.max_ammo, g.attachments
            );
        }
        println!("attachments: {:?}", d.attachments.iter().map(|a| format!("{}={}", a.name, a.display)).collect::<Vec<_>>());
        assert!(d.guns.len() > 20);
    }
}

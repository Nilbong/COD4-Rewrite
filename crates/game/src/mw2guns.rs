//! Modern Warfare 2 (MW2) guns in the game: Create a Class lists them next
//! to CoD4's, Black Ops' and World at War's, and matches use them like any
//! other gun.
//!
//! Everything comes from the user's MW2 install ([`iw4`]): the guns, their
//! attachments, camos, names, pictures and attribute bars from its own
//! `mp/statstable.csv`, `mp/attachmenttable.csv`, `mp/attachmentcombos.csv`,
//! `mp/camotable.csv` and `mp/attributestable.csv` (in `code_post_gfx_mp`,
//! laid out like CoD4's) and English strings (`localized_code_post_gfx_mp`);
//! stats, models, animations and sound names from the compiled weapons in
//! `common_mp` ([`iw4::weapons`]), whose models, materials and animations
//! become a [`Content`] of their own ([`load_content`]), like Black Ops'.
//!
//! In the game an MW2 gun is `iw4_<name>` (`iw4_m4:reflex silencer`), with
//! a weapon index from [`FIRST_INDEX`] in CoD4's stats table. MW2 compiles
//! every pair of attachments it allows as a weapon of its own
//! (`m4_reflex_silencer_mp`, names in order), all on one view model whose
//! hidden tags pick the parts, so a gun with its attachments is exactly one
//! of those. Camos are model variants: `gunXModel[camo]`, camo `n` of
//! `mp/camotable.csv` (the class's camo stat holds [`CAMO_BASE`] + n).

use crate::content::Content;
use crate::weapons::{AdsOverlay, Reticle, WeaponDef, WeaponSounds};
use bevy::prelude::*;
use iw4::weapons::{Picture, Weapon};
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, OnceLock};

/// MW2 guns' names in the game start with this.
pub const PREFIX: &str = "iw4_";
/// Their weapon indices in CoD4's stats table start here (Black Ops' are
/// 400..600, World at War's 600..800); weapon `i` keeps its unlock bits in
/// stat `3000 + i`.
pub const FIRST_INDEX: i32 = 800;
/// Material names the UI looks up in MW2's own pictures
/// ([`Data::picture`]): its menu pictures, camo swatches and kill icons.
pub const MATERIAL_PREFIX: &str = "iw4/";
/// An MW2 camo's value in the class's camo stat: this plus its
/// `mp/camotable.csv` index (1 woodland ... 8 orange fall).
pub const CAMO_BASE: usize = 200;

/// Is this stats table index an MW2 gun's?
pub fn is_index(index: i32) -> bool {
    (FIRST_INDEX..FIRST_INDEX + 200).contains(&index)
}

/// Is this an MW2 gun (`iw4_m4`, `iw4_m4:reflex`)?
pub fn is_mw2(weapon: &str) -> bool {
    weapon.starts_with(PREFIX)
}

/// MW2's camo model index of a class's camo stat value, if it's MW2's.
pub fn camo_model(camo: usize) -> Option<usize> {
    (CAMO_BASE + 1..CAMO_BASE + 16).contains(&camo).then(|| camo - CAMO_BASE)
}

/// MW2's sound aliases go into the bank under `iw4/<alias>`, apart from
/// CoD4's (both games name some sounds alike).
pub fn sound_alias(name: &str) -> String {
    format!("iw4/{}", name.to_ascii_lowercase())
}

/// MW2's weapon groups (`mp/statstable.csv` column 2) that matches use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Group {
    Assault,
    Smg,
    Lmg,
    Sniper,
    /// Secondaries: shotguns, machine pistols, pistols.
    Shotgun,
    MachinePistol,
    Pistol,
}

impl Group {
    fn from_table(group: &str) -> Option<Group> {
        Some(match group {
            "weapon_assault" => Group::Assault,
            "weapon_smg" => Group::Smg,
            "weapon_lmg" => Group::Lmg,
            "weapon_sniper" => Group::Sniper,
            "weapon_shotgun" => Group::Shotgun,
            "weapon_machine_pistol" => Group::MachinePistol,
            "weapon_pistol" => Group::Pistol,
            _ => return None,
        })
    }

    /// The primary groups, in MW2's menu order.
    pub const PRIMARY: [Group; 4] = [Group::Assault, Group::Smg, Group::Lmg, Group::Sniper];
    /// The secondary groups, in MW2's menu order.
    pub const SECONDARY: [Group; 3] = [Group::MachinePistol, Group::Shotgun, Group::Pistol];

    /// The group name (stats table column 2, CoD4's where it has one) and
    /// MW2's string for its heading.
    pub fn table(self) -> (&'static str, &'static str) {
        match self {
            Group::Assault => ("weapon_assault", "MPUI_ASSAULT_RIFLES"),
            Group::Smg => ("weapon_smg", "MPUI_SUB_MACHINE_GUNS"),
            Group::Lmg => ("weapon_lmg", "MPUI_LIGHT_MACHINE_GUNS"),
            Group::Sniper => ("weapon_sniper", "MPUI_SNIPER_RIFLES"),
            Group::Shotgun => ("weapon_shotgun", "MPUI_SHOTGUNS"),
            Group::MachinePistol => ("weapon_pistol", "MPUI_MACHINE_PISTOLS"),
            Group::Pistol => ("weapon_pistol", "MPUI_HANDGUNS"),
        }
    }

    /// A CoD4 gun of the same kind, whose sounds fill any an MW2 gun lacks.
    fn sound_standin(self) -> &'static str {
        match self {
            Group::Assault => "m4_mp",
            Group::Smg => "mp5_mp",
            Group::Lmg => "saw_mp",
            Group::Sniper => "m40a3_mp",
            Group::Shotgun => "winchester1200_mp",
            Group::MachinePistol | Group::Pistol => "beretta_mp",
        }
    }

    pub fn secondary(self) -> bool {
        Group::SECONDARY.contains(&self)
    }
}

/// An MW2 gun.
#[derive(Clone, Debug)]
pub struct Gun {
    pub index: i32,
    /// MW2's name, `m4` (the game's is [`Gun::id`]).
    pub name: String,
    pub group: Group,
    /// "M4A1", from MW2's strings, and its key (`WEAPON_M4`).
    pub display: String,
    pub key: String,
    pub description_key: String,
    /// MW2's picture material (`weapon_m4_short`).
    pub picture: String,
    /// The attachments it takes, in MW2's order, that it has a weapon for.
    pub attachments: Vec<String>,
    /// The camos it has models for (`mp/camotable.csv` indices).
    pub camos: Vec<usize>,
}

impl Gun {
    /// The game's name for it: `iw4_m4`.
    pub fn id(&self) -> String {
        format!("{PREFIX}{}", self.name)
    }
}

/// An MW2 attachment (`mp/attachmenttable.csv`).
#[derive(Clone, Debug)]
pub struct Attachment {
    pub name: String,
    /// "Red Dot Sight", and its key (`MPUI_RED_DOT_SIGHT`).
    pub display: String,
    pub key: String,
    pub description_key: String,
    pub picture: String,
}

/// An MW2 camo (`mp/camotable.csv`).
#[derive(Clone, Debug)]
pub struct Camo {
    pub index: usize,
    pub name: String,
    pub key: String,
    pub description_key: String,
    pub picture: String,
}

pub struct Data {
    pub install: iw4::Install,
    pub vfs: Arc<iw3::iwd::Vfs>,
    pub guns: Vec<Gun>,
    pub attachments: Vec<Attachment>,
    pub camos: Vec<Camo>,
    /// Pairs of attachments MW2 doesn't allow together.
    excluded: BTreeSet<(String, String)>,
    /// Compiled weapons by name (`m4_reflex_silencer_mp`).
    weapons: HashMap<String, Weapon>,
    /// MW2's English strings by key.
    pub strings: HashMap<String, String>,
    /// Attribute bars (`mp/attributestable.csv`): (group, name, [accuracy,
    /// damage, range, fire rate, mobility]).
    pub attributes: Vec<(String, String, [i32; 5])>,
    /// Menu pictures, camo swatches and kill icons by material name.
    pictures: HashMap<String, Picture>,
}

static DATA: OnceLock<Option<Data>> = OnceLock::new();

/// MW2's data, loaded on first use (`None` without an MW2 install).
pub fn data() -> Option<&'static Data> {
    DATA.get_or_init(|| match Data::load() {
        Ok(d) => {
            log::info!("mw2 guns: {} guns, {} attachments, {} camos", d.guns.len(), d.attachments.len(), d.camos.len());
            Some(d)
        }
        Err(e) => {
            log::info!("mw2 guns: Modern Warfare 2 guns unavailable: {e:#}");
            None
        }
    })
    .as_ref()
}

/// Start loading MW2's data in the background, then its guns' sounds.
pub fn preload() {
    std::thread::spawn(|| {
        if let Some(d) = data() {
            let _ = d.sound_aliases();
        }
    });
}

/// A string table's rows from a parsed MW2 zone.
fn table(zone: &iw4::zone::Zone, name: &str) -> Vec<Vec<String>> {
    let Some((_, a)) = zone.of_type(iw4::zone::AssetType::StringTable).find(|(_, a)| a.name.eq_ignore_ascii_case(name)) else {
        return Vec::new();
    };
    let cols = a.root.int("columnCount").max(1) as usize;
    let cells: Vec<String> = a.root.nodes("values").iter().map(|c| c.string("string").unwrap_or("").to_owned()).collect();
    cells.chunks(cols).map(<[String]>::to_vec).collect()
}

fn parse(install: &iw4::Install, name: &str) -> anyhow::Result<iw4::zone::Zone> {
    Ok(iw4::zone::Zone::parse(&iw4::fastfile::load(&install.zone_path(name))?, Default::default())?)
}

impl Data {
    fn load() -> anyhow::Result<Data> {
        let t0 = std::time::Instant::now();
        let install = iw4::Install::locate()?;
        let vfs = Arc::new(install.vfs()?);
        // (The zones in parallel: `common_mp` alone takes seconds.)
        let (common, gfx, strings) = std::thread::scope(|s| {
            let common = s.spawn(|| {
                let common = parse(&install, "common_mp")?;
                let weapons: HashMap<String, Weapon> = iw4::weapons::weapons(&common).into_iter().map(|w| (w.name.to_ascii_lowercase(), w)).collect();
                let pictures = iw4::weapons::pictures(&common, |m| {
                    ["weapon_", "ui_camoskin_", "hud_icon_", "killicon", "scope_overlay", "reticle_"].iter().any(|p| m.starts_with(p))
                });
                log::info!("mw2 guns: weapons read in {:.2?}", t0.elapsed());
                anyhow::Ok((weapons, pictures))
            });
            let gfx = s.spawn(|| parse(&install, "code_post_gfx_mp"));
            let strings = s.spawn(|| {
                let mut strings = HashMap::new();
                // (Only its strings: the sounds are read later, apart.)
                if let Ok(zone) = parse(&install, "localized_code_post_gfx_mp") {
                    for (_, a) in zone.of_type(iw4::zone::AssetType::LocalizeEntry) {
                        strings.insert(a.name.to_ascii_uppercase(), a.root.string("value").unwrap_or("").to_owned());
                    }
                }
                log::info!("mw2 guns: strings read in {:.2?}", t0.elapsed());
                strings
            });
            (common.join(), gfx.join(), strings.join())
        });
        let ((weapons, pictures), gfx, strings) = (
            common.map_err(|_| anyhow::anyhow!("reading common_mp panicked"))??,
            gfx.map_err(|_| anyhow::anyhow!("reading code_post_gfx_mp panicked"))??,
            strings.map_err(|_| anyhow::anyhow!("reading strings panicked"))?,
        );
        let text = |key: &str| strings.get(&key.to_ascii_uppercase()).cloned().unwrap_or_else(|| key.to_owned());

        // [index, unlock stat, group, name key, name, -, picture, description
        // key, -, stow model, -, attachments (11..21)...], like CoD4's.
        let mut guns: Vec<Gun> = Vec::new();
        for row in table(&gfx, "mp/statstable.csv") {
            let cell = |c: usize| row.get(c).map_or("", String::as_str);
            let (Some(group), name, Ok(index)) = (Group::from_table(cell(2)), cell(4), cell(0).parse::<i32>()) else { continue };
            let Some(base) = weapons.get(&format!("{name}_mp")) else { continue };
            if name.is_empty() || guns.iter().any(|g| g.name == name) {
                continue;
            }
            let attachments =
                (11..22).map(cell).filter(|a| !a.is_empty()).map(str::to_ascii_lowercase).filter(|a| weapons.contains_key(&format!("{name}_{a}_mp"))).collect();
            let camos = (1..base.gun_models.len()).filter(|&c| !base.gun_models[c].is_empty()).collect();
            guns.push(Gun {
                index: FIRST_INDEX + index,
                name: name.to_owned(),
                group,
                display: text(cell(3)),
                key: cell(3).to_owned(),
                description_key: cell(7).to_owned(),
                picture: cell(6).to_owned(),
                attachments,
                camos,
            });
        }

        // [index, -, slot, name key, name, -, picture, description key, ...].
        let mut attachments = Vec::new();
        for row in table(&gfx, "mp/attachmenttable.csv") {
            let cell = |c: usize| row.get(c).map_or("", String::as_str);
            if !cell(4).is_empty() && !matches!(cell(4), "none" | "done") && cell(0).parse::<i32>().is_ok() {
                attachments.push(Attachment {
                    name: cell(4).to_ascii_lowercase(),
                    display: text(cell(3)),
                    key: cell(3).to_owned(),
                    description_key: cell(7).to_owned(),
                    picture: cell(6).to_owned(),
                });
            }
        }

        // A grid: row and column heads are attachments, "no" where the two
        // can't go together.
        let combos = table(&gfx, "mp/attachmentcombos.csv");
        let mut excluded = BTreeSet::new();
        if let Some(head) = combos.first() {
            for row in &combos[1..] {
                for (c, cell) in row.iter().enumerate().skip(1) {
                    if cell == "no" {
                        if let (Some(a), Some(b)) = (row.first(), head.get(c)) {
                            excluded.insert((a.to_ascii_lowercase(), b.to_ascii_lowercase()));
                        }
                    }
                }
            }
        }

        // [index, name, name key, description key, picture, unlock].
        let camos = table(&gfx, "mp/camotable.csv")
            .into_iter()
            .filter_map(|row| {
                let index: usize = row.first()?.parse().ok().filter(|&i| i > 0)?;
                Some(Camo {
                    index,
                    name: row.get(1)?.clone(),
                    key: row.get(2)?.clone(),
                    description_key: row.get(3)?.clone(),
                    picture: row.get(4)?.clone(),
                })
            })
            .collect();

        // [group, name, accuracy, damage, range, fire rate, mobility].
        let attributes = table(&gfx, "mp/attributestable.csv")
            .into_iter()
            .filter_map(|row| {
                let values: Vec<i32> = row.get(2..7)?.iter().map(|v| v.trim().parse().ok()).collect::<Option<_>>()?;
                let name = row.get(1).filter(|n| !n.is_empty())?;
                Some((row[0].clone(), name.to_ascii_lowercase(), values.try_into().ok()?))
            })
            .collect();
        log::info!("mw2 guns: tables and weapons read in {:.2?}", t0.elapsed());
        Ok(Data { install, vfs, guns, attachments, camos, excluded, weapons, strings, attributes, pictures })
    }

    /// A gun by the game's name (`iw4_m4`).
    pub fn gun(&self, id: &str) -> Option<&Gun> {
        let name = id.strip_prefix(PREFIX)?;
        self.guns.iter().find(|g| g.name.eq_ignore_ascii_case(name))
    }

    pub fn attachment(&self, name: &str) -> Option<&Attachment> {
        self.attachments.iter().find(|a| a.name.eq_ignore_ascii_case(name))
    }

    /// Whether MW2 lets `a` and `b` go on a gun together.
    pub fn compatible(&self, a: &str, b: &str) -> bool {
        a != b && !self.excluded.contains(&(a.to_owned(), b.to_owned())) && !self.excluded.contains(&(b.to_owned(), a.to_owned()))
    }

    /// The compiled weapon of a gun with its attachments (two at most, in
    /// name order): `m4_reflex_silencer_mp`, or the nearest MW2 has.
    fn variant(&self, gun: &Gun, attachments: &[&str]) -> Option<&Weapon> {
        let mut atts: Vec<&str> = attachments.iter().copied().filter(|a| gun.attachments.iter().any(|x| x == a)).take(2).collect();
        atts.sort_unstable();
        let name = |atts: &[&str]| format!("{}_{}mp", gun.name, atts.iter().map(|a| format!("{a}_")).collect::<String>());
        self.weapons
            .get(&name(&atts))
            .or_else(|| atts.first().and_then(|a| self.weapons.get(&name(&[a]))))
            .or_else(|| self.weapons.get(&name(&[])))
    }

    /// A compiled weapon by name (`m4_reflex_mp`).
    pub fn weapon(&self, name: &str) -> Option<&Weapon> {
        self.weapons.get(&name.to_ascii_lowercase())
    }

    /// Stats of an MW2 gun with its attachments (`iw4_m4`, ["reflex"]).
    pub fn weapon_def(&self, id: &str, attachments: &[&str]) -> Option<WeaponDef> {
        let gun = self.gun(id)?;
        let w = self.variant(gun, attachments)?;
        let mut def = def_from_weapon(w, gun.group);
        def.name = format!("{}_{}mp", id, attachments.iter().map(|a| format!("{a}_")).collect::<String>());
        def.display_key = gun.display.clone();
        Some(def)
    }

    /// An MW2 gun's view (or world) model with camo `camo` (0 none) and the
    /// tags its attachments' variant hides.
    pub fn model(&self, id: &str, attachments: &[&str], world: bool, camo: usize) -> Option<(String, BTreeSet<String>)> {
        let gun = self.gun(id)?;
        let w = self.variant(gun, attachments)?;
        let model = if world { w.world_model(camo) } else { w.gun_model(camo) };
        Some((model.to_owned(), w.hide_tags.iter().map(|t| t.to_ascii_lowercase()).collect()))
    }

    /// The CoD4 gun whose sounds fill any an MW2 gun lacks.
    pub fn sound_standin(&self, id: &str) -> Option<&'static str> {
        self.gun(id).map(|g| g.group.sound_standin())
    }

    /// The picture behind an MW2 material the UI draws (`iw4/weapon_m4_short`
    /// without its prefix).
    pub fn picture(&self, material: &str) -> Option<&Picture> {
        self.pictures.get(&material.to_ascii_lowercase())
    }

    /// The sound aliases MW2's guns use, as the game's (under
    /// [`sound_alias`] names). Read once (a few seconds; [`preload`] does it
    /// early), shared after.
    pub fn sound_aliases(&self) -> anyhow::Result<Vec<(String, Vec<crate::audio::bank::Variant>)>> {
        static SOUNDS: OnceLock<Vec<(String, Vec<crate::audio::bank::Variant>)>> = OnceLock::new();
        if let Some(list) = SOUNDS.get() {
            return Ok(list.clone());
        }
        let list = self.read_sound_aliases()?;
        Ok(SOUNDS.get_or_init(|| list).clone())
    }

    /// The aliases the guns' weapons name (and their reload notes'), from
    /// `localized_common_mp` and `common_mp`.
    fn read_sound_aliases(&self) -> anyhow::Result<Vec<(String, Vec<crate::audio::bank::Variant>)>> {
        let t0 = std::time::Instant::now();
        let zones: Vec<iw4::zone::Zone> =
            ["localized_common_mp", "common_mp", "code_post_gfx_mp"].iter().filter_map(|z| parse(&self.install, z).ok()).collect();
        let channels = zones.iter().find_map(iw4::sound::channels);
        let list = crate::audio::bank::mw2_aliases(&zones, channels.as_deref(), &self.sound_names());
        log::info!("mw2 guns: {} gun sound aliases in {:.2?}", list.len(), t0.elapsed());
        Ok(list)
    }

    /// Every sound alias name the guns use.
    pub fn sound_names(&self) -> std::collections::HashSet<String> {
        let mut names = std::collections::HashSet::new();
        for gun in &self.guns {
            for w in self.weapons.iter().filter(|(n, _)| n.starts_with(&format!("{}_", gun.name))).map(|(_, w)| w) {
                for key in SOUND_FIELDS {
                    names.insert(w.get(key).trim().to_ascii_lowercase());
                }
                names.extend(w.notetrack_sounds.iter().map(|(_, s)| s.to_ascii_lowercase()));
            }
        }
        names.extend(STREAK_SOUNDS.iter().map(|s| s.to_string()));
        for voice in ["us", "uk", "ab", "ru"] {
            for line in ["achieve_carepackage", "use_carepackage", "enemy_carepackage", "achieve_sentrygun", "enemy_sentrygun", "sentry_destroyed", "sentry_gone"] {
                names.insert(format!("{voice}_1mc_{line}"));
            }
        }
        names.remove("");
        names
    }
}

/// MW2's kill streak sounds the care package and the sentry gun play
/// ([`crate::killstreaks`]); the announcer's lines are added per voice.
const STREAK_SOUNDS: [&str; 13] = [
    "mp_killstreak_carepackage",
    "mp_killstreak_sentrygun",
    "ammo_crate_use",
    "sentry_drop",
    "sentry_gun_plant",
    "sentry_gun_beep",
    "sentry_explode",
    "sentry_explode_smoke",
    "sentry_minigun_fire",
    "sentry_minigun_cooldown",
    "sentry_minigun_spin",
    "sentry_minigun_spinup1",
    "sentry_minigun_spindown1",
];

/// The weapon members naming sounds the game plays.
const SOUND_FIELDS: [&str; 14] = [
    "fireSound",
    "fireSoundPlayer",
    "fireLastSound",
    "fireLastSoundPlayer",
    "emptyFireSound",
    "emptyFireSoundPlayer",
    "reloadSound",
    "raiseSoundPlayer",
    "putawaySoundPlayer",
    "rechamberSoundPlayer",
    "rechamberSound",
    "pullbackSound",
    "pullbackSoundPlayer",
    "firstRaiseSoundPlayer",
];

/// MW2's animation slots (`weapAnimFiles_t`, 37 of them) -> CoD4's
/// (`viewmodel::anim_slot`'s 33): MW2 added melee charge, breach and
/// empty raises, stun, night vision and detonate animations in between.
const ANIM_SLOTS: [(usize, usize); 23] = [
    (1, 1),
    (2, 2),
    (3, 3),
    (5, 5),
    (6, 6),
    (7, 7),
    (9, 9),
    (10, 10),
    (11, 11),
    (12, 12),
    (13, 13),
    (14, 14),
    (16, 15),
    (19, 18),
    (20, 19),
    (23, 22),
    (24, 23),
    (25, 24),
    (32, 28),
    (33, 29),
    (34, 30),
    (35, 31),
    (36, 32),
];

/// A [`WeaponDef`] from a compiled MW2 weapon: CoD4's own members, mostly
/// (MW2 kept CoD4's names and units: times in ms, view kicks in hundredths
/// of a degree).
fn def_from_weapon(w: &Weapon, group: Group) -> WeaponDef {
    let fb = WeaponDef::fallback();
    let fo = |k: &str, d: f32| w.f(k).unwrap_or(d);
    let ms = |k: &str, d: f32| w.secs(k).unwrap_or(d);
    let kick = |a: &str, b: &str| {
        let (x, y) = (fo(a, 0.0) / 100.0, fo(b, 0.0) / 100.0);
        (x.min(y), x.max(y))
    };
    let sound = |k: &str| Some(w.get(k).trim()).filter(|v| !v.is_empty()).map_or_else(String::new, sound_alias);
    let material = |k: &str| Some(w.get(k).trim()).filter(|v| !v.is_empty()).map_or_else(String::new, |v| format!("{MATERIAL_PREFIX}{v}"));
    let mut xanims = vec![None; 33];
    for (from, to) in ANIM_SLOTS {
        // (The zone names them in lower case.)
        xanims[to] = w.anim(from).map(str::to_ascii_lowercase);
    }
    let bolt = w.flag("bBoltAction");
    WeaponDef {
        name: w.name.clone(),
        display_name: "",
        damage: fo("damage", fb.damage),
        min_damage: fo("minDamage", fb.min_damage),
        max_damage_range: fo("fMaxDamageRange", fb.max_damage_range),
        min_damage_range: fo("fMinDamageRange", fb.min_damage_range),
        fire_time: ms("iFireTime", fb.fire_time).max(0.01),
        fire_type: fo("fireType", 0.0) as i32,
        rechamber_time: if bolt { ms("iRechamberTime", 0.0) } else { 0.0 },
        clip_size: fo("iClipSize", 30.0).max(1.0) as u32,
        max_ammo: fo("iMaxAmmo", 0.0).max(0.0) as u32,
        start_ammo: fo("iStartAmmo", 0.0).max(0.0) as u32,
        reload_time: ms("iReloadTime", fb.reload_time),
        reload_empty_time: ms("iReloadEmptyTime", ms("iReloadTime", fb.reload_empty_time)),
        segmented_reload: w.flag("bSegmentedReload"),
        reload_start_time: ms("iReloadStartTime", 0.0),
        reload_start_add_time: ms("iReloadStartAddTime", 0.0),
        reload_add_time: ms("iReloadAddTime", 0.0),
        reload_end_time: ms("iReloadEndTime", 0.0),
        reload_start_add: fo("iReloadStartAdd", 0.0).max(0.0) as u32,
        reload_ammo_add: fo("iReloadAmmoAdd", 1.0).max(1.0) as u32,
        ads_trans_in: ms("iAdsTransInTime", fb.ads_trans_in),
        ads_trans_out: ms("iAdsTransOutTime", fb.ads_trans_out),
        sprint_in_time: ms("sprintInTime", fb.sprint_in_time),
        sprint_loop_time: ms("sprintLoopTime", fb.sprint_loop_time),
        sprint_out_time: ms("sprintOutTime", fb.sprint_out_time),
        drop_time: ms("iDropTime", fb.drop_time),
        raise_time: ms("iRaiseTime", fb.raise_time),
        first_raise_time: ms("iFirstRaiseTime", fb.first_raise_time),
        quick_drop_time: ms("quickDropTime", fb.quick_drop_time),
        quick_raise_time: ms("quickRaiseTime", fb.quick_raise_time),
        hip_spread_min: [fo("fHipSpreadStandMin", 3.0), fo("fHipSpreadDuckedMin", 2.5), fo("fHipSpreadProneMin", 2.0)],
        hip_spread_max: [fo("hipSpreadStandMax", 7.0), fo("hipSpreadDuckedMax", 6.0), fo("hipSpreadProneMax", 5.0)],
        hip_spread_fire_add: fo("fHipSpreadFireAdd", fb.hip_spread_fire_add),
        hip_spread_move_add: fo("fHipSpreadMoveAdd", fb.hip_spread_move_add),
        hip_spread_decay: fo("fHipSpreadDecayRate", fb.hip_spread_decay),
        ads_spread: fo("fAdsSpread", 0.0),
        kick_pitch: kick("fHipViewKickPitchMin", "fHipViewKickPitchMax"),
        kick_yaw: kick("fHipViewKickYawMin", "fHipViewKickYawMax"),
        ads_kick_pitch: kick("fAdsViewKickPitchMin", "fAdsViewKickPitchMax"),
        ads_kick_yaw: kick("fAdsViewKickYawMin", "fAdsViewKickYawMax"),
        move_speed_scale: fo("moveSpeedScale", 1.0),
        ads_move_speed_scale: fo("adsMoveSpeedScale", 1.0),
        ads_fov: fo("fAdsZoomFov", 50.0),
        ads_view_bob_mult: fo("fAdsViewBobMult", fb.ads_view_bob_mult),
        ads_bob_factor: fo("fAdsBobFactor", fb.ads_bob_factor),
        hip_idle_amount: fo("fHipIdleAmount", fb.hip_idle_amount),
        hip_idle_speed: fo("hipIdleSpeed", fb.hip_idle_speed),
        ads_idle_amount: fo("fAdsIdleAmount", fb.ads_idle_amount),
        ads_idle_speed: fo("adsIdleSpeed", fb.ads_idle_speed),
        idle_crouch_factor: fo("fIdleCrouchFactor", 1.0),
        idle_prone_factor: fo("fIdleProneFactor", 0.4),
        viewmodel: w.gun_model(0).to_owned(),
        world_model: w.world_model(0).to_owned(),
        xanims,
        // MW2's own (see `Data::sound_aliases`); a CoD4 stand-in fills any
        // gap (see `Data::sound_standin`).
        sounds: WeaponSounds {
            fire: sound("fireSound"),
            fire_player: sound("fireSoundPlayer"),
            fire_last: sound("fireLastSound"),
            fire_last_player: sound("fireLastSoundPlayer"),
            empty: sound("emptyFireSound"),
            empty_player: sound("emptyFireSoundPlayer"),
            reload: sound("reloadSound"),
            raise_player: sound("raiseSoundPlayer"),
            putaway_player: sound("putawaySoundPlayer"),
            pullback: sound("pullbackSound"),
            pullback_player: sound("pullbackSoundPlayer"),
            rechamber: sound("rechamberSound"),
        },
        impact_type: fo("impactType", 2.0) as i32,
        penetrate_type: fo("penetrateType", 1.0).clamp(0.0, 3.0) as u8,
        class: match group {
            Group::Lmg => 1,
            Group::Smg | Group::MachinePistol => 2,
            Group::Shotgun => 3,
            Group::Pistol => 4,
            Group::Assault | Group::Sniper => 0,
        },
        player_anim: fo("playerAnimType", 0.0).clamp(0.0, crate::weapons::PLAYER_ANIM_TYPES.len() as f32 - 1.0) as u8,
        kill_icon: material("killIcon"),
        kill_icon_ratio: fo("killIconRatio", 0.0) as i32,
        ammo_counter: fo("ammoCounterClip", 1.0) as i32,
        // CoD4's own crosshair pieces (MW2's guns name CoD4's).
        reticle: Reticle {
            side: w.get("reticleSide").to_owned(),
            side_size: fo("iReticleSideSize", 8.0),
            min_ofs: fo("iReticleMinOfs", 0.0),
            center: w.get("reticleCenter").to_owned(),
            center_size: fo("iReticleCenterSize", 0.0),
        },
        display_key: w.get("szDisplayName").to_owned(),
        ads_overlay: Some(w.get("overlayMaterial")).filter(|n| !n.is_empty()).map(|n| AdsOverlay {
            material: format!("{MATERIAL_PREFIX}{n}"),
            width: fo("overlayWidth", 480.0),
            height: fo("overlayHeight", 480.0),
        }),
        gunplay: crate::weapons::Gunplay {
            location_mult: [1.4, 1.0, 1.0, 1.0, 1.0],
            kick_center: (fo("fHipViewKickCenterSpeed", 1500.0), fo("fAdsViewKickCenterSpeed", 1500.0)),
            reduced_kick: [
                (fo("hipGunKickReducedKickBullets", 0.0), fo("hipGunKickReducedKickPercent", 0.0)),
                (fo("adsGunKickReducedKickBullets", 0.0), fo("adsGunKickReducedKickPercent", 0.0)),
            ],
            spread_decay_stance: (fo("fHipSpreadDuckedDecay", 1.0), fo("fHipSpreadProneDecay", 1.0)),
            spread_turn_add: fo("fHipSpreadTurnAdd", 0.0),
            fire_delay: ms("iFireDelay", 0.0),
            gun_kick: [
                crate::weapons::GunKick::read(&|k| fo(&format!("f{k}"), 0.0), "Hip"),
                crate::weapons::GunKick::read(&|k| fo(&format!("f{k}"), 0.0), "Ads"),
            ],
            gun_max: (fo("fGunMaxPitch", 6.0), fo("fGunMaxYaw", 6.0)),
        },
    }
}

/// MW2's gun content: `common_mp`'s models, materials and images as an `iw3`
/// zone, and its animations as another, with MW2's iwds. Takes several
/// seconds; run it off the main thread.
pub fn load_content() -> anyhow::Result<Content> {
    let data = data().ok_or_else(|| anyhow::anyhow!("no Modern Warfare 2 install"))?;
    let t0 = std::time::Instant::now();
    let common = parse(&data.install, "common_mp")?;
    let anims = iw4::convert::to_iw3_anims(&common);
    let zone = iw4::convert::to_iw3(&common, None);
    let mut zones = vec![zone, anims];
    zones.extend(crate_models(&data.install));
    log::info!("mw2 guns: gun content loaded in {:.2?}", t0.elapsed());
    Ok(Content::new(zones, data.vfs.clone()))
}

/// The care package's crates, one per team: MW2's big plastic cases (its
/// team crates, `com_plasticcase_taskforce141` and `_arab`, borrow their
/// mesh from a zone of another map), taken out of Rust's zone with what
/// they use (MW2 keeps them in each map's zone, not `common_mp`).
pub const CRATE_MODELS: [&str; 2] = ["com_plasticcase_green_big_us_dirt", "com_plasticcase_beige_big"];

fn crate_models(install: &iw4::Install) -> Option<iw3::zone::Zone> {
    let map = parse(install, "mp_rust").map_err(|e| log::warn!("mw2 guns: no crate models: {e:#}")).ok()?;
    let empty = || iw4::zone::GNode { ty: String::new(), data: Vec::new(), fields: Vec::new(), locs: Vec::new() };
    // Stand-ins for them, filled from the map's zone.
    let wanted = iw4::zone::Zone {
        script_strings: Vec::new(),
        assets: CRATE_MODELS.iter().map(|n| iw4::zone::Asset { ty: iw4::zone::AssetType::XModel, name: format!(",{n}"), root: empty() }).collect(),
        top_level: Vec::new(),
        stats: Default::default(),
        block_sizes: [0; iw4::zone::reader::BLOCK_COUNT],
    };
    Some(iw4::convert::to_iw3(&wanted, Some(&map)))
}

/// MW2's gun content for matches: loaded in the background when a match
/// starts (if MW2 is installed), kept for later matches.
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
                    Ok(Err(e)) => log::warn!("mw2 guns: gun content unavailable: {e:#}"),
                    Err(_) => log::warn!("mw2 guns: loading gun content panicked"),
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

pub struct Mw2GunsPlugin;

impl Plugin for Mw2GunsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MatchContent>()
            .add_systems(OnEnter(crate::state::GameState::InGame), |mut c: ResMut<MatchContent>| c.start());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indices() {
        assert!(is_index(820) && !is_index(610) && !is_index(10));
        assert!(is_mw2("iw4_m4") && !is_mw2("t4_thompson"));
        assert_eq!(camo_model(201), Some(1));
        assert_eq!(camo_model(5), None);
        assert_eq!(sound_alias("Weap_M4carbine_Fire_Plr"), "iw4/weap_m4carbine_fire_plr");
    }

    /// Against the local install: `cargo test --release -p game mw2guns -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn reads_install() {
        let d = data().expect("Modern Warfare 2 install");
        for g in &d.guns {
            let def = d.weapon_def(&g.id(), &[]).expect("weapon def");
            println!(
                "{:>4} {:<14} {:?} {:<16} {} dmg, {:.0} rpm, clip {} / {}, {:?} camos {:?}",
                g.index, g.name, g.group, g.display, def.damage, 60.0 / def.fire_time, def.clip_size, def.max_ammo, g.attachments, g.camos
            );
        }
        println!("attachments: {:?}", d.attachments.iter().map(|a| format!("{}={}", a.name, a.display)).collect::<Vec<_>>());
        println!("camos: {:?}", d.camos.iter().map(|c| format!("{}={}", c.index, c.name)).collect::<Vec<_>>());
        let def = d.weapon_def("iw4_m4", &["silencer", "reflex"]).unwrap();
        println!("m4 reflex silencer: {} {:?}", def.name, d.model("iw4_m4", &["silencer", "reflex"], false, 3));
        assert!(d.guns.len() > 30);
    }
}

//! Black Ops (BO1) guns in the game: Create a Class lists them next to
//! CoD4's, and matches use them like any other gun.
//!
//! Everything comes from the user's Black Ops install ([`t5`]): the guns,
//! their attachments and BO1's own names from its `mp/statstable.csv` and
//! `mp/attachmenttable.csv` (in `code_post_gfx_mp`) and English strings
//! (`en_code_post_gfx_mp`); stats, models and animations from the plain-text
//! weapon files (`weapons/mp/ak47_reflex_mp`); models and animations from its
//! `common_mp` ([`load_content`], a [`Content`] of its own, since BO1 reuses
//! CoD4's names for different models and animations).
//!
//! In the game a BO1 gun is `t5_<name>` (`t5_ak47:reflex+extclip`), with a
//! weapon index from [`FIRST_INDEX`] in CoD4's stats table, so Create a Class
//! and the class menus handle it like CoD4's guns.
//!
//! Camos (from `mp/weaponoptions.csv`): each gun material has a colour
//! detail texture (`colorDetailMap`), added onto the colour map like CoD4's
//! camo (`color + detail - 0.5`, gamma space) but only where the colour
//! map's alpha says so. A gun lists which of its materials take the metal
//! and which the furniture camo; a camo swaps each base texture
//! (`cammo_gunmetal`, `cammo_wood_tile_red`, ...) for its own. With no camo
//! the base textures still apply: they are the guns' metal and wood grain.

use crate::content::Content;
use crate::weapons::{AdsOverlay, Reticle, WeaponDef, WeaponSounds};
use bevy::prelude::*;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use t5::weapons::WeaponFile;

/// BO1 guns' names in the game start with this.
pub const PREFIX: &str = "t5_";
/// Their weapon indices in CoD4's stats table start here (CoD4's end at
/// 149); weapon `i` keeps its unlock bits in stat `3000 + i`.
pub const FIRST_INDEX: i32 = 400;
/// A class's camo stat holds BO1 camo `n` as `CAMO_STAT_BASE + n` (0 is
/// none), apart from CoD4's camo numbers.
pub const CAMO_STAT_BASE: usize = 100;

/// Is this stats table index a BO1 gun's (up to World at War's)?
pub fn is_index(index: i32) -> bool {
    (FIRST_INDEX..crate::waw::FIRST_INDEX).contains(&index)
}

/// The camo stat for BO1 camo `n`.
pub fn camo_stat(n: usize) -> usize {
    if n == 0 { 0 } else { CAMO_STAT_BASE + n }
}

/// The BO1 camo of a camo stat (none for CoD4's camo numbers).
pub fn camo_of_stat(stat: usize) -> usize {
    stat.checked_sub(CAMO_STAT_BASE).unwrap_or(0)
}


/// Black Ops' gold camo colour map (nearly black) and specular map (gold).
pub const GOLD_CAMO_TEXTURE: &str = "camo_gold_c";
pub const GOLD_CAMO_SPECULAR: &str = "camo_gold_spec";

/// Is this a BO1 gun (`t5_ak47`, `t5_ak47:reflex`)?
pub fn is_bo1(weapon: &str) -> bool {
    weapon.starts_with(PREFIX)
}

/// BO1's weapon groups (`mp/statstable.csv` column 2) that matches can use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Group {
    Assault,
    Smg,
    Lmg,
    Sniper,
    Shotgun,
    Pistol,
}

impl Group {
    fn from_table(group: &str) -> Option<Group> {
        Some(match group {
            "weapon_assault" => Group::Assault,
            "weapon_smg" => Group::Smg,
            "weapon_lmg" => Group::Lmg,
            "weapon_sniper" => Group::Sniper,
            "weapon_cqb" => Group::Shotgun,
            "weapon_pistol" => Group::Pistol,
            _ => return None,
        })
    }

    /// The primary weapon groups, in BO1's menu order.
    pub const PRIMARY: [Group; 5] = [Group::Assault, Group::Smg, Group::Lmg, Group::Shotgun, Group::Sniper];

    /// CoD4's group name (stats table column 2) and string for its heading.
    pub fn cod4(self) -> (&'static str, &'static str) {
        match self {
            Group::Assault => ("weapon_assault", "@MPUI_ASSAULT_RIFLES"),
            Group::Smg => ("weapon_smg", "@MPUI_SUB_MACHINE_GUNS"),
            Group::Lmg => ("weapon_lmg", "@MPUI_LIGHT_MACHINE_GUNS"),
            Group::Sniper => ("weapon_sniper", "@MPUI_SNIPER_RIFLES"),
            Group::Shotgun => ("weapon_shotgun", "@MPUI_SHOTGUNS"),
            Group::Pistol => ("weapon_pistol", "@MPUI_PISTOLS"),
        }
    }

    /// A CoD4 gun of the same kind, whose sounds a BO1 gun borrows (BO1's
    /// sounds are in its own sound bank format).
    fn sound_standin(self) -> &'static str {
        match self {
            Group::Assault => "ak47_mp",
            Group::Smg => "mp5_mp",
            Group::Lmg => "rpd_mp",
            Group::Sniper => "m40a3_mp",
            Group::Shotgun => "winchester1200_mp",
            Group::Pistol => "colt45_mp",
        }
    }
}

/// A BO1 gun.
#[derive(Clone, Debug)]
pub struct Gun {
    pub index: i32,
    /// BO1's name, `ak47` (the game's is [`Gun::id`]).
    pub name: String,
    pub group: Group,
    /// "AK47", from BO1's strings, and its key (`WEAPON_AK47`).
    pub display: String,
    pub key: String,
    pub description_key: String,
    /// BO1's picture material (`menu_mp_weapons_ak47`).
    pub picture: String,
    /// The attachments it takes, in BO1's order, that have a weapon file.
    pub attachments: Vec<String>,
}

impl Gun {
    /// The game's name for it: `t5_ak47`.
    pub fn id(&self) -> String {
        format!("{PREFIX}{}", self.name)
    }
}

/// A BO1 attachment (`mp/attachmenttable.csv`).
#[derive(Clone, Debug)]
pub struct Attachment {
    pub name: String,
    /// "Red Dot Sight", and its key (`MPUI_ELBIT`).
    pub display: String,
    pub key: String,
    /// Where it goes: `top`, `bottom`, `muzzle` or `trigger`. One of each.
    pub slot: String,
    pub picture: String,
}

/// A camo (`mp/weaponoptions.csv`).
#[derive(Clone, Debug)]
pub struct Camo {
    pub index: usize,
    /// Its name's key (`MPUI_CAMO_DUSTY`, "Dusty").
    pub key: String,
    pub picture: String,
}

pub struct Data {
    pub install: t5::Install,
    pub vfs: Arc<iw3::iwd::Vfs>,
    pub guns: Vec<Gun>,
    pub attachments: Vec<Attachment>,
    /// Camos in BO1's order; 0 is none.
    pub camos: Vec<Camo>,
    /// Weapon files by variant name (`ak47_reflex_mp`).
    weapons: HashMap<String, WeaponFile>,
    /// BO1's English strings by key.
    pub strings: HashMap<String, String>,
    /// `mp/weaponoptions.csv`: the base camo textures (row 0), each camo's
    /// replacements (rows 1..), and each gun's camo materials.
    camo_base: Vec<String>,
    camo_rows: Vec<Vec<String>>,
    camo_guns: HashMap<String, Vec<String>>,
    /// Scope pictures' images by material, once the guns' content has
    /// loaded ([`load_content`]).
    overlays: std::sync::Mutex<HashMap<String, String>>,
}

/// BO1 materials the UI draws (scope pictures) are named with this prefix.
pub const MATERIAL_PREFIX: &str = "t5/";

static DATA: OnceLock<Option<Data>> = OnceLock::new();

/// BO1's data, loaded on first use (`None` without a Black Ops install).
pub fn data() -> Option<&'static Data> {
    DATA.get_or_init(|| match Data::load() {
        Ok(d) => {
            log::info!("bo1: {} guns, {} attachments, {} camos", d.guns.len(), d.attachments.len(), d.camos.len());
            Some(d)
        }
        Err(e) => {
            log::info!("bo1: Black Ops guns unavailable: {e:#}");
            None
        }
    })
    .as_ref()
}

/// Start loading BO1's data in the background.
pub fn preload() {
    std::thread::spawn(|| {
        data();
    });
}

/// A string table's rows from a parsed BO1 zone.
fn table(zone: &t5::zone::Zone, name: &str) -> Vec<Vec<String>> {
    let Some((_, a)) = zone.of_type(t5::zone::AssetType::StringTable).find(|(_, a)| a.name.eq_ignore_ascii_case(name)) else {
        return Vec::new();
    };
    let cols = a.root.int("columnCount").max(0) as usize;
    let rows = a.root.int("rowCount").max(0) as usize;
    let cells: Vec<String> = match a.root.nodes("values") {
        [] => a.root.strings("values").into_iter().map(|s| s.unwrap_or("").to_owned()).collect(),
        nodes => nodes.iter().map(|n| n.string("string").unwrap_or("").to_owned()).collect(),
    };
    (0..rows).map(|r| (0..cols).map(|c| cells.get(r * cols + c).cloned().unwrap_or_default()).collect()).collect()
}

impl Data {
    fn load() -> anyhow::Result<Data> {
        let t0 = std::time::Instant::now();
        let install = t5::Install::locate()?;
        let vfs = Arc::new(install.vfs()?);
        let weapons: HashMap<String, WeaponFile> =
            t5::weapons::mp_weapons(&vfs).into_iter().map(|w| (w.name.to_ascii_lowercase(), w)).collect();
        let parse = |name: &str| -> anyhow::Result<t5::zone::Zone> {
            Ok(t5::zone::Zone::parse(&t5::fastfile::load(&install.zone_path(name))?, Default::default())?)
        };
        let gfx = parse("code_post_gfx_mp")?;
        let strings: HashMap<String, String> = parse("en_code_post_gfx_mp")
            .map(|z| {
                z.of_type(t5::zone::AssetType::LocalizeEntry)
                    .map(|(_, a)| (a.name.to_ascii_uppercase(), a.root.string("value").unwrap_or("").to_owned()))
                    .collect()
            })
            .unwrap_or_default();
        let text = |key: &str| strings.get(&key.to_ascii_uppercase()).cloned().unwrap_or_else(|| key.to_owned());

        let mut guns = Vec::new();
        for row in table(&gfx, "mp/statstable.csv") {
            let cell = |c: usize| row.get(c).map_or("", String::as_str);
            let Ok(bo_index) = cell(0).parse::<i32>() else { continue };
            let (Some(group), name) = (Group::from_table(cell(2)), cell(4)) else { continue };
            // Dual wield pairs are their guns' `dw` attachment.
            if name.is_empty() || name.ends_with("dw") || !weapons.contains_key(&format!("{name}_mp")) {
                continue;
            }
            let attachments = cell(8)
                .split_whitespace()
                .filter(|a| *a != "dw" && weapons.contains_key(&format!("{name}_{a}_mp")))
                .map(str::to_owned)
                .collect();
            guns.push(Gun {
                index: FIRST_INDEX + bo_index,
                name: name.to_owned(),
                group,
                display: text(cell(3)),
                key: cell(3).to_owned(),
                description_key: cell(7).to_owned(),
                picture: cell(6).to_owned(),
                attachments,
            });
        }

        let (mut attachments, mut camos) = (Vec::new(), Vec::new());
        for row in table(&gfx, "mp/attachmenttable.csv") {
            let cell = |c: usize| row.get(c).map_or("", String::as_str);
            match (cell(2), cell(1)) {
                ("attachment", slot) if cell(4) != "none" && !cell(4).is_empty() => attachments.push(Attachment {
                    name: cell(4).to_owned(),
                    display: text(cell(3)),
                    key: cell(3).to_owned(),
                    slot: slot.to_owned(),
                    picture: cell(6).to_owned(),
                }),
                ("weaponoption", "camo") => {
                    if let Ok(index) = cell(0).parse() {
                        camos.push(Camo { index, key: cell(3).to_owned(), picture: cell(6).to_owned() });
                    }
                }
                _ => {}
            }
        }
        camos.sort_by_key(|c| c.index);

        let (mut camo_base, mut camo_rows, mut camo_guns) = (Vec::new(), Vec::new(), HashMap::new());
        for row in table(&gfx, "mp/weaponoptions.csv") {
            match (row.first().map(String::as_str), row.get(1).map(String::as_str)) {
                (Some(i), Some("camo")) => {
                    let Ok(i) = i.parse::<usize>() else { continue };
                    if i == 0 {
                        camo_base = row.clone();
                    }
                    if camo_rows.len() <= i {
                        camo_rows.resize(i + 1, Vec::new());
                    }
                    camo_rows[i] = row;
                }
                (_, Some("weapon")) => {
                    if let Some(name) = row.get(2) {
                        camo_guns.insert(name.to_ascii_lowercase(), row.clone());
                    }
                }
                _ => {}
            }
        }
        log::info!("bo1: tables and weapon files read in {:.2?}", t0.elapsed());
        Ok(Data {
            install,
            vfs,
            guns,
            attachments,
            camos,
            weapons,
            strings,
            camo_base,
            camo_rows,
            camo_guns,
            overlays: Default::default(),
        })
    }

    /// A gun by the game's name (`t5_ak47`) or its index.
    pub fn gun(&self, id: &str) -> Option<&Gun> {
        let name = id.strip_prefix(PREFIX)?;
        self.guns.iter().find(|g| g.name.eq_ignore_ascii_case(name))
    }

    pub fn attachment(&self, name: &str) -> Option<&Attachment> {
        self.attachments.iter().find(|a| a.name.eq_ignore_ascii_case(name))
    }

    /// A weapon file by variant name (`ak47_reflex_mp`).
    pub fn weapon(&self, variant: &str) -> Option<&WeaponFile> {
        self.weapons.get(&variant.to_ascii_lowercase())
    }

    /// The image behind a BO1 scope picture's material (without its
    /// prefix), once the guns' content has loaded.
    pub fn material_image(&self, material: &str) -> Option<String> {
        self.overlays.lock().ok()?.get(&material.to_ascii_lowercase()).cloned()
    }

    /// The colour detail texture a material of gun `name` (BO1's name) gets
    /// with camo `camo` (0: none): its own (`own`) unless the gun lists the
    /// material, then the camo's texture for the material's base texture.
    pub fn camo_detail(&self, name: &str, material: &str, camo: usize, own: Option<&str>) -> Option<String> {
        let material = material.trim_start_matches(',').trim_start_matches("mc/");
        // [index, "weapon", gun, metal base, furniture base, materials...]:
        // columns 5..9 and the scopes (18..) are metal, the rest furniture.
        let listed = self.camo_guns.get(&name.to_ascii_lowercase()).and_then(|row| {
            let col = row.iter().skip(5).position(|m| !m.is_empty() && m.eq_ignore_ascii_case(material))? + 5;
            Some(row.get(if col < 9 || col >= 18 { 3 } else { 4 })?.clone())
        });
        let Some(base) = listed else { return own.map(str::to_owned) };
        if camo == 0 {
            return Some(base);
        }
        let column = self.camo_base.iter().position(|b| b.eq_ignore_ascii_case(&base));
        let swapped = column.and_then(|c| self.camo_rows.get(camo)?.get(c)).filter(|t| !t.is_empty());
        match swapped.map(String::as_str) {
            // Gold has a material set of its own: its colour map stands in,
            // and the gun model shines it (see `gunmodel::CamoDetail`).
            Some("gold") => Some(GOLD_CAMO_TEXTURE.into()),
            Some(t) => Some(t.to_owned()),
            None => Some(base),
        }
    }

    /// Stats of a BO1 gun with attachments (`t5_ak47`, ["reflex",
    /// "extclip"]): the base weapon file with each attachment's changes on
    /// top, like CoD4 combinations (see `loadout`).
    pub fn weapon_def(&self, id: &str, attachments: &[&str]) -> Option<WeaponDef> {
        let gun = self.gun(id)?;
        let base_file = self.weapon(&format!("{}_mp", gun.name))?;
        let base = def_from_file(base_file, gun.group);
        let mut out = base.clone();
        // Sights last, so their aiming values win.
        let mut order = attachments.to_vec();
        order.sort_by_key(|a| self.attachment(a).is_some_and(|x| x.slot == "top"));
        for att in order {
            if let Some(file) = self.weapon(&format!("{}_{att}_mp", gun.name)) {
                crate::loadout::merge(&mut out, &base, &def_from_file(file, gun.group));
            }
        }
        out.name = format!("{}_{}mp", id, attachments.iter().map(|a| format!("{a}_")).collect::<String>());
        out.display_key = gun.display.clone();
        Some(out)
    }

    /// The CoD4 gun whose sounds a BO1 gun borrows.
    pub fn sound_standin(&self, id: &str) -> Option<&'static str> {
        self.gun(id).map(|g| g.group.sound_standin())
    }
}

/// A [`WeaponDef`] from a BO1 weapon file. BO1 gives times in seconds
/// (CoD4's binary defs: milliseconds) and view kick in hundredths of a
/// degree like CoD4.
fn def_from_file(w: &WeaponFile, group: Group) -> WeaponDef {
    let fb = WeaponDef::fallback();
    let f = |k: &str| w.get(k).trim().parse::<f32>().ok();
    let fo = |k: &str, d: f32| f(k).unwrap_or(d);
    let kick = |a: &str, b: &str| {
        let (x, y) = (fo(a, 0.0) / 100.0, fo(b, 0.0) / 100.0);
        (x.min(y), x.max(y))
    };
    let clip = w.int("clipSize").max(1) as u32;
    // Base files count magazines, variants rounds.
    let max_ammo = match w.int("maxAmmo").max(0) as u32 {
        m if m <= 20 => m * clip,
        m => m,
    };
    let name = |k: &str| Some(w.get(k).trim()).filter(|v| !v.is_empty() && *v != "none").unwrap_or("").to_owned();
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
    WeaponDef {
        name: w.name.clone(),
        display_name: "",
        damage: fo("damage", fb.damage),
        min_damage: fo("minDamage", fb.min_damage),
        max_damage_range: fo("maxDamageRange", fb.max_damage_range),
        min_damage_range: fo("minDamageRange", fb.min_damage_range),
        fire_time: fo("fireTime", fb.fire_time).max(0.01),
        fire_type: crate::weapons::fire_type_named(w.get("fireType")),
        rechamber_time: if w.get("boltAction").trim() == "1" { fo("rechamberTime", 0.0) } else { 0.0 },
        clip_size: clip,
        max_ammo,
        reload_time: fo("reloadTime", fb.reload_time),
        reload_empty_time: fo("reloadEmptyTime", fb.reload_empty_time),
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
        ads_fov: fo("adsZoomFov1", fo("adsZoomFov", 50.0)),
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
        // Black Ops' own (`t5::sound`); a CoD4 stand-in fills any gap (see
        // `Data::sound_standin`).
        sounds: WeaponSounds {
            fire: w.get("fireSound").to_owned(),
            fire_player: w.get("fireSoundPlayer").to_owned(),
            fire_last: w.get("fireLastSound").to_owned(),
            fire_last_player: w.get("fireLastSoundPlayer").to_owned(),
            empty: w.get("emptyFireSound").to_owned(),
            empty_player: w.get("emptyFireSoundPlayer").to_owned(),
            reload: w.get("reloadSound").to_owned(),
            raise_player: w.get("raiseSoundPlayer").to_owned(),
            putaway_player: w.get("putawaySoundPlayer").to_owned(),
        },
        penetrate_type: crate::weapons::penetrate_type_named(w.get("penetrateType")),
        impact_type: match w.get("impactType") {
            "bullet_small" => 1,
            "bullet_ap" => 3,
            "shotgun" | "shotgun_ap" => 4,
            _ => 2,
        },
        class: match (w.get("weaponClass"), group) {
            ("mg", _) => 1,
            ("smg", _) => 2,
            ("spread", _) => 3,
            ("pistol", _) => 4,
            _ => 0,
        },
        kill_icon: name("killIcon"),
        kill_icon_ratio: w.get("killIconRatio").split(':').next().and_then(|r| r.trim().parse().ok()).unwrap_or(1),
        ammo_counter: match w.get("ammoCounterClip").to_ascii_lowercase().as_str() {
            "shortmagazine" => 2,
            "shotgun" => 3,
            "rocket" => 4,
            "beltfed" => 5,
            _ => 1,
        },
        reticle: Reticle {
            side: name("reticleSide"),
            side_size: fo("reticleSideSize", 8.0),
            min_ofs: fo("reticleMinOfs", 0.0),
            center: name("reticleCenter"),
            center_size: fo("reticleCenterSize", 0.0),
        },
        display_key: w.get("displayName").to_owned(),
        // Sniper and infrared scopes (BO1's own material).
        ads_overlay: Some(name("adsOverlayShader")).filter(|n| !n.is_empty()).map(|n| AdsOverlay {
            material: format!("{MATERIAL_PREFIX}{n}"),
            width: fo("adsOverlayWidth", 480.0),
            height: fo("adsOverlayHeight", 480.0),
        }),
    }
}

/// BO1's gun content: `common_mp` (guns, hands, animations) and
/// `code_post_gfx_mp` (the hands' materials) as `iw3` zones, with BO1's
/// iwds. Takes a few seconds; run it off the main thread.
pub fn load_content() -> anyhow::Result<Content> {
    let data = data().ok_or_else(|| anyhow::anyhow!("no Black Ops install"))?;
    let t0 = std::time::Instant::now();
    let zones = ["common_mp", "code_post_gfx_mp"]
        .into_iter()
        .map(|z| t5::load_iw3(&data.install, z))
        .collect::<anyhow::Result<Vec<_>>>()?;
    // The scopes' pictures become drawable (`ui::assets`).
    if let Ok(mut overlays) = data.overlays.lock() {
        for zone in &zones {
            for asset in &zone.assets {
                let iw3::zone::Asset::Material(m) = asset else { continue };
                if m.name.contains("_overlay") && !m.name.starts_with(',') {
                    if let Some(image) = m.textures.first().and_then(|t| t.image).and_then(|i| zone.image(i)) {
                        overlays.insert(m.name.to_ascii_lowercase(), image.name.clone());
                    }
                }
            }
        }
    }
    log::info!("bo1: gun content loaded in {:.2?}", t0.elapsed());
    Ok(Content::new(zones, data.vfs.clone()))
}

/// BO1's gun content for matches: loaded in the background when a match
/// starts (if Black Ops is installed), kept for later matches.
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
                    Ok(Err(e)) => log::warn!("bo1: gun content unavailable: {e:#}"),
                    Err(_) => log::warn!("bo1: loading gun content panicked"),
                }
            }
        }
        match self {
            MatchContent::Ready(c) => Some(c),
            _ => None,
        }
    }
}

pub struct Bo1Plugin;

impl Plugin for Bo1Plugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MatchContent>()
            .add_systems(OnEnter(crate::state::GameState::InGame), |mut c: ResMut<MatchContent>| c.start());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camo_details() {
        let row = |cells: &[&str]| cells.iter().map(|c| c.to_string()).collect::<Vec<_>>();
        let gun = row(&[
            "1", "weapon", "ak47", "cammo_gunmetal", "cammo_wood_tile_red", "t5_weapon_mtl_ak47_gunset", "", "", "",
            "t5_weapon_mtl_ak47_gunset_plastic",
        ]);
        let d = Data {
            install: t5::Install { root: Default::default() },
            vfs: Arc::new(iw3::iwd::Vfs::mount(&[]).unwrap()),
            guns: Vec::new(),
            attachments: Vec::new(),
            camos: Vec::new(),
            weapons: HashMap::new(),
            strings: HashMap::new(),
            camo_base: row(&["0", "camo", "cammo_gunmetal", "cammo_gunplastic", "cammo_wood_tile_red"]),
            camo_rows: vec![
                row(&["0", "camo", "cammo_gunmetal", "cammo_gunplastic", "cammo_wood_tile_red"]),
                row(&["1", "camo", "solid_camo_dusty", "solid_camo_dusty", "solid_camo_dusty"]),
                row(&["2", "camo", "camo_desert_nevada", "solid_camo_desert_nevada", "solid_camo_desert_nevada"]),
            ],
            camo_guns: HashMap::from([("ak47".to_owned(), gun)]),
            overlays: Default::default(),
        };
        // Listed metal: the gunmetal base, or the camo's pattern.
        assert_eq!(d.camo_detail("ak47", "mc/t5_weapon_mtl_ak47_gunset", 0, Some("weapon_camo_off")).as_deref(), Some("cammo_gunmetal"));
        assert_eq!(d.camo_detail("ak47", "mc/t5_weapon_mtl_ak47_gunset", 2, None).as_deref(), Some("camo_desert_nevada"));
        // Listed furniture: the wood base, or the camo's solid colour.
        assert_eq!(d.camo_detail("ak47", "mc/t5_weapon_mtl_ak47_gunset_plastic", 0, None).as_deref(), Some("cammo_wood_tile_red"));
        assert_eq!(d.camo_detail("ak47", "mc/t5_weapon_mtl_ak47_gunset_plastic", 2, None).as_deref(), Some("solid_camo_desert_nevada"));
        // Unlisted: its own.
        assert_eq!(d.camo_detail("ak47", "mc/other", 2, Some("cammo_gunmetal")).as_deref(), Some("cammo_gunmetal"));
    }

    /// Against the local install: `cargo test -p game bo1 -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn reads_install() {
        let d = data().expect("Black Ops install");
        for g in &d.guns {
            let def = d.weapon_def(&g.id(), &[]).expect("weapon def");
            println!(
                "{:>3} {:<12} {:?} {:<14} {} dmg, {:.0} rpm, clip {} / {}, {:?}",
                g.index, g.name, g.group, g.display, def.damage, 60.0 / def.fire_time, def.clip_size, def.max_ammo, g.attachments
            );
        }
        println!("attachments: {:?}", d.attachments.iter().map(|a| format!("{}={}({})", a.name, a.display, a.slot)).collect::<Vec<_>>());
        println!("camos: {:?}", d.camos.iter().map(|c| d.strings.get(&c.key).map_or("?", String::as_str)).collect::<Vec<_>>());
        assert!(d.guns.len() > 30);
    }
}

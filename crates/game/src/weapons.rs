//! Hitscan weapons: fire rate, spread, recoil, ADS, reloads, damage
//! falloff and bullet penetration. Their muzzle flashes and impacts are CoD4's effects
//! ([`crate::fx`]).

use crate::collision;
use crate::combat::{Damage, Dead, HitLocation, Hitbox};
use crate::movement::{Mover, Stance, ViewAngles};
use crate::units::u;
use avian3d::prelude::*;
use bevy::prelude::*;
use rand::Rng;

pub struct WeaponsPlugin;

impl Plugin for WeaponsPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<ShotFired>()
            .add_message::<HitConfirmed>()
            .add_message::<Penetrated>()
            .init_resource::<Penetration>()
            .add_systems(
                OnEnter(crate::state::GameState::InGame),
                (load_weapon, load_penetration).after(crate::world::load_map).in_set(crate::state::Setup::Content),
            )
            .add_systems(
                Update,
                update_weapons.in_set(WeaponSet).after(crate::movement::MovementSet).run_if(crate::state::in_game),
            );
    }
}

/// Weapon stats, read from the game's `WeaponDef` (see [`WeaponDef::from_node`]).
#[derive(Debug, Clone)]
pub struct WeaponDef {
    pub name: String,
    pub display_name: &'static str,
    pub damage: f32,
    pub min_damage: f32,
    /// Full damage up to here (CoD units), falling to `min_damage` at `min_damage_range`.
    pub max_damage_range: f32,
    pub min_damage_range: f32,
    /// Seconds between shots.
    pub fire_time: f32,
    /// `fireType`: 0 full auto, 1 single shot (a trigger pull per shot),
    /// 2-4 a burst of that many per pull.
    pub fire_type: i32,
    /// Bolt actions (and the W1200's pump): working the action after each
    /// shot (`iRechamberTime`), 0 for everything else.
    pub rechamber_time: f32,
    pub clip_size: u32,
    pub max_ammo: u32,
    /// Spare rounds at spawn (`iStartAmmo`; 0: three magazines).
    pub start_ammo: u32,
    pub reload_time: f32,
    pub reload_empty_time: f32,
    /// Segmented reloads (shotguns, bolt actions: `bSegmentedReload`): a
    /// start that loads `reload_start_add` rounds `reload_start_add_time`
    /// in, then a `reload_time` loop loading `reload_ammo_add` rounds
    /// `reload_add_time` into each cycle until the clip is full (or the
    /// reserve is out), then an end.
    pub segmented_reload: bool,
    pub reload_start_time: f32,
    pub reload_start_add_time: f32,
    pub reload_add_time: f32,
    pub reload_end_time: f32,
    pub reload_start_add: u32,
    pub reload_ammo_add: u32,
    pub ads_trans_in: f32,
    pub ads_trans_out: f32,
    /// Sprint in/loop/out durations the viewmodel anims are played to.
    pub sprint_in_time: f32,
    pub sprint_loop_time: f32,
    pub sprint_out_time: f32,
    /// Putting the weapon away and raising it (switching weapons), and its
    /// first raise after spawning.
    pub drop_time: f32,
    pub raise_time: f32,
    pub first_raise_time: f32,
    /// Putting it away for a grenade and raising it after
    /// (`quickDropTime`, `quickRaiseTime`).
    pub quick_drop_time: f32,
    pub quick_raise_time: f32,
    /// Hip spread in degrees: (stand, crouch, prone) minimum and maximum.
    pub hip_spread_min: [f32; 3],
    pub hip_spread_max: [f32; 3],
    pub hip_spread_fire_add: f32,
    pub hip_spread_move_add: f32,
    pub hip_spread_decay: f32,
    pub ads_spread: f32,
    /// View kick per shot in degrees.
    pub kick_pitch: (f32, f32),
    pub kick_yaw: (f32, f32),
    pub ads_kick_pitch: (f32, f32),
    pub ads_kick_yaw: (f32, f32),
    pub move_speed_scale: f32,
    pub ads_move_speed_scale: f32,
    pub ads_fov: f32,
    /// View angle bob scale while fully aimed down sights.
    pub ads_view_bob_mult: f32,
    /// Weapon bob scale while fully aimed down sights.
    pub ads_bob_factor: f32,
    /// Idle sway amount (hundredths of a degree) and speed, hip and ADS.
    pub hip_idle_amount: f32,
    pub hip_idle_speed: f32,
    pub ads_idle_amount: f32,
    pub ads_idle_speed: f32,
    pub idle_crouch_factor: f32,
    pub idle_prone_factor: f32,
    pub viewmodel: String,
    pub world_model: String,
    /// `szXAnims`, indexed by `crate::viewmodel::anim_slot`.
    pub xanims: Vec<Option<String>>,
    pub sounds: WeaponSounds,
    /// `impactType`: 1 small bullets, 2 large, 3 armour piercing, 4 shotgun.
    pub impact_type: i32,
    /// `penetrateType`: 0 none, 1 small, 2 medium, 3 large (see
    /// [`Penetration`]).
    pub penetrate_type: u8,
    /// `weapClass`: 0 rifle, 1 machine gun, 2 SMG, 3 shotgun, 4 pistol, ...
    pub class: i32,
    /// `playerAnimType`, an index into [`PLAYER_ANIM_TYPES`]: how other
    /// players are seen holding, firing and reloading it.
    pub player_anim: u8,
    /// HUD: the kill feed icon and its width:height ratio (1 or 2), and the
    /// ammo counter's bullet style (`ammoCounterClip`: 1 magazine, 2 short
    /// magazine, 3 shotgun, 4 rocket, 5 belt).
    pub kill_icon: String,
    pub kill_icon_ratio: i32,
    pub ammo_counter: i32,
    /// The crosshair, and the name shown (`szDisplayName`, a string key).
    pub reticle: Reticle,
    pub display_key: String,
    /// A scope's full-screen picture once fully aimed (`ui::scope`).
    pub ads_overlay: Option<AdsOverlay>,
    /// The rest of CoD4's gunplay numbers ([`Gunplay`]).
    pub gunplay: Gunplay,
}

/// CoD4's gunplay numbers past the basics: hit locations, how the view
/// kick settles, the first shots' reduced kick and how stance and turning
/// change the hip spread.
#[derive(Debug, Clone, PartialEq)]
pub struct Gunplay {
    /// `locationDamageMultipliers` for the head, neck, upper torso, lower
    /// torso and upper legs (the hitboxes there are).
    pub location_mult: [f32; 5],
    /// `hipViewKickCenterSpeed`, `adsViewKickCenterSpeed`: how fast the
    /// view kick is pulled back (degrees/s²).
    pub kick_center: (f32, f32),
    /// `*GunKickReducedKickBullets` and `*Percent`, hip then ADS: the first
    /// shots of a burst kick this much (`weaponRestrictKickTime`).
    pub reduced_kick: [(f32, f32); 2],
    /// `fHipSpreadDuckedDecay`, `fHipSpreadProneDecay`: the spread decay
    /// rate's scale crouched and prone.
    pub spread_decay_stance: (f32, f32),
    /// `fHipSpreadTurnAdd`: spread from turning.
    pub spread_turn_add: f32,
    /// `iFireDelay` (s): the first shot's wait.
    pub fire_delay: f32,
    /// How the gun in hand kicks ([`GunKick`]): hip, then fully aimed.
    pub gun_kick: [GunKick; 2],
    /// `fGunMaxPitch`, `fGunMaxYaw`: how far it can kick (degrees).
    pub gun_max: (f32, f32),
}

/// The gun's own kick in the hands (`BG_CalculateWeaponPosition_GunRecoil`):
/// each shot adds a speed (degrees/s; CoD's pitch is down), a spring pulls
/// the gun back.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GunKick {
    pub pitch: (f32, f32),
    pub yaw: (f32, f32),
    pub accel: f32,
    pub speed_max: f32,
    pub speed_decay: f32,
    pub static_decay: f32,
}

impl GunKick {
    /// Read from a weapon's fields, `name(key)` giving `hip`/`ads` keys
    /// spelt as the game does.
    pub fn read(f: &dyn Fn(&str) -> f32, prefix: &str) -> GunKick {
        GunKick {
            pitch: (f(&format!("{prefix}GunKickPitchMin")), f(&format!("{prefix}GunKickPitchMax"))),
            yaw: (f(&format!("{prefix}GunKickYawMin")), f(&format!("{prefix}GunKickYawMax"))),
            accel: f(&format!("{prefix}GunKickAccel")),
            speed_max: f(&format!("{prefix}GunKickSpeedMax")),
            speed_decay: f(&format!("{prefix}GunKickSpeedDecay")),
            static_decay: f(&format!("{prefix}GunKickStaticDecay")),
        }
    }

    /// Blended between hip and fully aimed.
    fn lerp(&self, ads: &GunKick, t: f32) -> GunKick {
        let l = |a: f32, b: f32| a + (b - a) * t;
        GunKick {
            pitch: self.pitch,
            yaw: self.yaw,
            accel: l(self.accel, ads.accel),
            speed_max: l(self.speed_max, ads.speed_max),
            speed_decay: l(self.speed_decay, ads.speed_decay),
            static_decay: l(self.static_decay, ads.static_decay),
        }
    }
}

impl Default for Gunplay {
    fn default() -> Self {
        Gunplay {
            location_mult: [1.4, 1.0, 1.0, 1.0, 1.0],
            kick_center: (1500.0, 1500.0),
            reduced_kick: [(0.0, 0.0), (0.0, 0.0)],
            spread_decay_stance: (1.0, 1.0),
            spread_turn_add: 0.0,
            fire_delay: 0.0,
            gun_kick: [GunKick::default(), GunKick::default()],
            gun_max: (0.0, 0.0),
        }
    }
}

/// A weapon's crosshair: side pieces `side_size` virtual pixels square at
/// least `min_ofs` from the centre, and an optional centre piece.
#[derive(Debug, Clone, PartialEq)]
pub struct Reticle {
    pub side: String,
    pub side_size: f32,
    pub min_ofs: f32,
    pub center: String,
    pub center_size: f32,
}

/// A scope's picture while aiming (`overlayMaterial`, `adsOverlayShader`),
/// `width` by `height` virtual pixels at the centre of the screen.
#[derive(Debug, Clone, PartialEq)]
pub struct AdsOverlay {
    pub material: String,
    pub width: f32,
    pub height: f32,
}

/// A weapon's sound aliases (`weap_ak47_fire_plr`, ...); empty where it has
/// none. `_player` ones are what its user hears, the others everyone else.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WeaponSounds {
    pub fire: String,
    pub fire_player: String,
    pub fire_last: String,
    pub fire_last_player: String,
    pub empty: String,
    pub empty_player: String,
    pub reload: String,
    pub raise_player: String,
    pub putaway_player: String,
    /// A grenade's pin (`pullbackSound`).
    pub pullback: String,
    pub pullback_player: String,
    /// A bolt being worked (`rechamberSound`).
    pub rechamber: String,
}

/// `mp/playeranimtypes.txt`: the `playerAnimType`s, in order.
pub const PLAYER_ANIM_TYPES: [&str; 15] =
    ["none", "other", "pistol", "smg", "autorifle", "mg", "sniper", "rocketlauncher", "explosive", "grenade", "turret", "c4", "m203", "hold", "briefcase"];

/// A weapon file's `playerAnimType` by name (a rifle's if unknown).
pub fn player_anim_named(name: &str) -> u8 {
    PLAYER_ANIM_TYPES.iter().position(|t| t.eq_ignore_ascii_case(name.trim())).unwrap_or(4) as u8
}

impl WeaponDef {
    /// Shots per pull for burst weapons, 0 otherwise.
    pub fn burst(&self) -> u32 {
        if (2..=4).contains(&self.fire_type) { self.fire_type as u32 } else { 0 }
    }

    /// Fallback used before game data is loaded (and in tests).
    pub fn fallback() -> WeaponDef {
        WeaponDef {
            name: "ak47_mp".into(),
            display_name: "AK-47",
            damage: 40.0,
            min_damage: 30.0,
            max_damage_range: 1500.0,
            min_damage_range: 3000.0,
            fire_time: 0.085,
            fire_type: 0,
            rechamber_time: 0.0,
            clip_size: 30,
            max_ammo: 180,
            start_ammo: 90,
            reload_time: 2.5,
            reload_empty_time: 3.25,
            segmented_reload: false,
            reload_start_time: 0.0,
            reload_start_add_time: 0.0,
            reload_add_time: 0.0,
            reload_end_time: 0.0,
            reload_start_add: 0,
            reload_ammo_add: 1,
            ads_trans_in: 0.25,
            ads_trans_out: 0.25,
            ads_view_bob_mult: 0.2,
            ads_bob_factor: 1.0,
            hip_idle_amount: 30.0,
            hip_idle_speed: 1.0,
            ads_idle_amount: 40.0,
            ads_idle_speed: 0.8,
            idle_crouch_factor: 1.0,
            idle_prone_factor: 0.4,
            sprint_in_time: 0.3,
            sprint_loop_time: 0.7,
            sprint_out_time: 0.3,
            drop_time: 0.6,
            raise_time: 0.95,
            first_raise_time: 1.4,
            quick_drop_time: 0.25,
            quick_raise_time: 0.75,
            hip_spread_min: [3.0, 2.5, 2.0],
            hip_spread_max: [7.0, 6.0, 5.0],
            hip_spread_fire_add: 0.6,
            hip_spread_move_add: 5.0,
            hip_spread_decay: 4.0,
            ads_spread: 0.0,
            kick_pitch: (-0.3, 0.6),
            kick_yaw: (-0.6, 0.6),
            ads_kick_pitch: (-0.3, 0.6),
            ads_kick_yaw: (-0.6, 0.6),
            move_speed_scale: 1.0,
            ads_move_speed_scale: 1.0,
            ads_fov: 50.0,
            viewmodel: "viewmodel_ak47_mp".into(),
            world_model: "weapon_ak47".into(),
            xanims: Vec::new(),
            sounds: WeaponSounds::default(),
            impact_type: 2,
            penetrate_type: 2,
            class: 0,
            player_anim: 4,
            kill_icon: "hud_icon_ak47".into(),
            kill_icon_ratio: 2,
            ammo_counter: 1,
            reticle: Reticle { side: "reticle_side_small".into(), side_size: 8.0, min_ofs: 0.0, center: String::new(), center_size: 0.0 },
            display_key: "WEAPON_AK47".into(),
            ads_overlay: None,
            gunplay: Gunplay::default(),
        }
    }

    /// Build from a loaded `WeaponDef` asset.
    pub fn from_node(zone: &iw3::zone::Zone, w: &iw3::zone::generic::GNode) -> WeaponDef {
        let ms = |f: &str| w.int(f) as f32 / 1000.0;
        let model = |id: Option<usize>| id.map(|i| zone.get(i).name().to_owned()).unwrap_or_default();
        // View kick values are stored in hundredths of a degree.
        let kick = |a: &str, b: &str| {
            let (x, y) = (w.float(a) / 100.0, w.float(b) / 100.0);
            (x.min(y), x.max(y))
        };
        WeaponDef {
            name: w.string("szInternalName").unwrap_or("weapon").to_owned(),
            display_name: "AK-47",
            damage: w.int("damage") as f32,
            min_damage: w.int("minDamage") as f32,
            max_damage_range: w.float("fMaxDamageRange"),
            min_damage_range: w.float("fMinDamageRange"),
            fire_time: ms("iFireTime").max(0.01),
            fire_type: w.int("fireType") as i32,
            rechamber_time: if w.int("bBoltAction") != 0 { ms("iRechamberTime") } else { 0.0 },
            clip_size: w.int("iClipSize").max(1) as u32,
            max_ammo: w.int("iMaxAmmo").max(0) as u32,
            start_ammo: w.int("iStartAmmo").max(0) as u32,
            reload_time: ms("iReloadTime"),
            reload_empty_time: ms("iReloadEmptyTime"),
            segmented_reload: w.int("bSegmentedReload") != 0,
            reload_start_time: ms("iReloadStartTime"),
            reload_start_add_time: ms("iReloadStartAddTime"),
            reload_add_time: ms("iReloadAddTime"),
            reload_end_time: ms("iReloadEndTime"),
            reload_start_add: w.int("iReloadStartAdd").max(0) as u32,
            reload_ammo_add: w.int("iReloadAmmoAdd").max(1) as u32,
            ads_trans_in: ms("iAdsTransInTime"),
            ads_trans_out: ms("iAdsTransOutTime"),
            ads_view_bob_mult: w.float("fAdsViewBobMult"),
            ads_bob_factor: w.float("fAdsBobFactor"),
            hip_idle_amount: w.float("fHipIdleAmount"),
            hip_idle_speed: w.float("hipIdleSpeed"),
            ads_idle_amount: w.float("fAdsIdleAmount"),
            ads_idle_speed: w.float("adsIdleSpeed"),
            idle_crouch_factor: w.float("fIdleCrouchFactor"),
            idle_prone_factor: w.float("fIdleProneFactor"),
            sprint_in_time: ms("sprintInTime"),
            sprint_loop_time: ms("sprintLoopTime"),
            sprint_out_time: ms("sprintOutTime"),
            drop_time: ms("iDropTime"),
            raise_time: ms("iRaiseTime"),
            first_raise_time: ms("iFirstRaiseTime"),
            quick_drop_time: ms("quickDropTime"),
            quick_raise_time: ms("quickRaiseTime"),
            hip_spread_min: [w.float("fHipSpreadStandMin"), w.float("fHipSpreadDuckedMin"), w.float("fHipSpreadProneMin")],
            hip_spread_max: [w.float("hipSpreadStandMax"), w.float("hipSpreadDuckedMax"), w.float("hipSpreadProneMax")],
            hip_spread_fire_add: w.float("fHipSpreadFireAdd"),
            hip_spread_move_add: w.float("fHipSpreadMoveAdd"),
            hip_spread_decay: w.float("fHipSpreadDecayRate"),
            ads_spread: w.float("fAdsSpread"),
            kick_pitch: kick("fHipViewKickPitchMin", "fHipViewKickPitchMax"),
            kick_yaw: kick("fHipViewKickYawMin", "fHipViewKickYawMax"),
            ads_kick_pitch: kick("fAdsViewKickPitchMin", "fAdsViewKickPitchMax"),
            ads_kick_yaw: kick("fAdsViewKickYawMin", "fAdsViewKickYawMax"),
            move_speed_scale: w.float("moveSpeedScale"),
            ads_move_speed_scale: w.float("adsMoveSpeedScale"),
            ads_fov: w.float("fAdsZoomFov"),
            viewmodel: model(w.assets("gunXModel").first().copied().flatten()),
            world_model: model(w.assets("worldModel").first().copied().flatten()),
            xanims: w.strings("szXAnims").into_iter().map(|s| s.map(str::to_owned)).collect(),
            sounds: WeaponSounds {
                fire: sound(w, "fireSound"),
                fire_player: sound(w, "fireSoundPlayer"),
                fire_last: sound(w, "lastShotSound"),
                fire_last_player: sound(w, "lastShotSoundPlayer"),
                empty: sound(w, "emptyFireSound"),
                empty_player: sound(w, "emptyFireSoundPlayer"),
                reload: sound(w, "reloadSound"),
                raise_player: sound(w, "raiseSoundPlayer"),
                putaway_player: sound(w, "putawaySoundPlayer"),
                pullback: sound(w, "pullbackSound"),
                pullback_player: sound(w, "pullbackSoundPlayer"),
                rechamber: sound(w, "rechamberSound"),
            },
            impact_type: w.int("impactType") as i32,
            penetrate_type: w.int("penetrateType").clamp(0, 3) as u8,
            class: w.int("weapClass") as i32,
            player_anim: w.int("playerAnimType").clamp(0, PLAYER_ANIM_TYPES.len() as i64 - 1) as u8,
            kill_icon: w.asset("killIcon").map(|i| zone.get(i).name().trim_start_matches(',').to_owned()).unwrap_or_default(),
            kill_icon_ratio: w.int("killIconRatio") as i32,
            ammo_counter: w.int("ammoCounterClip") as i32,
            reticle: Reticle {
                side: w.asset("reticleSide").map(|i| zone.get(i).name().trim_start_matches(',').to_owned()).unwrap_or_default(),
                side_size: w.int("iReticleSideSize") as f32,
                min_ofs: w.int("iReticleMinOfs") as f32,
                center: w.asset("reticleCenter").map(|i| zone.get(i).name().trim_start_matches(',').to_owned()).unwrap_or_default(),
                center_size: w.int("iReticleCenterSize") as f32,
            },
            display_key: w.string("szDisplayName").unwrap_or("").to_owned(),
            ads_overlay: w
                .asset("overlayMaterial")
                .map(|i| zone.get(i).name().trim_start_matches(',').to_owned())
                .filter(|m| !m.is_empty())
                .map(|material| AdsOverlay { material, width: w.float("overlayWidth"), height: w.float("overlayHeight") }),
            gunplay: Gunplay {
                // `hitLocation_t`: 2 head, 3 neck, 4 upper torso, 5 lower
                // torso, 12 upper right leg.
                location_mult: [2, 3, 4, 5, 12].map(|i| w.float(&format!("locationDamageMultipliers[{i}]"))),
                kick_center: (w.float("fHipViewKickCenterSpeed"), w.float("fAdsViewKickCenterSpeed")),
                reduced_kick: [
                    (w.float("hipGunKickReducedKickBullets"), w.float("hipGunKickReducedKickPercent")),
                    (w.float("adsGunKickReducedKickBullets"), w.float("adsGunKickReducedKickPercent")),
                ],
                spread_decay_stance: (w.float("fHipSpreadDuckedDecay"), w.float("fHipSpreadProneDecay")),
                spread_turn_add: w.float("fHipSpreadTurnAdd"),
                fire_delay: ms("iFireDelay"),
                gun_kick: [GunKick::read(&|k| w.float(&format!("f{k}")), "Hip"), GunKick::read(&|k| w.float(&format!("f{k}")), "Ads")],
                gun_max: (w.float("fGunMaxPitch"), w.float("fGunMaxYaw")),
            },
        }
    }
}

/// A weapon file's `penetrateType` name (Black Ops, World at War) as CoD4's
/// number.
pub fn penetrate_type_named(name: &str) -> u8 {
    match name.trim().to_ascii_lowercase().as_str() {
        "small" => 1,
        "medium" => 2,
        "large" => 3,
        _ => 0,
    }
}

/// How deep bullets go through each surface type ([`collision::SURFACE_NAMES`]),
/// in CoD units, for small, medium and large bullets
/// (`info/bullet_penetration_mp`). A bullet passes through a surface no
/// thicker than that, losing the fraction of the depth it used from its
/// damage, up to five times (`Bullet_Fire`); Deep Impact doubles the depths
/// (`perk_bulletPenetrationMultiplier`).
#[derive(Resource, Default)]
pub struct Penetration {
    depths: [[f32; 29]; 3],
}

/// `perk_bulletPenetrationMultiplier`.
const DEEP_IMPACT: f32 = 2.0;
/// `Bullet_Fire`'s most surfaces a bullet passes through.
const MAX_PENETRATIONS: usize = 5;

impl Penetration {
    /// The depth (CoD units) a `penetrate_type` bullet goes through surface
    /// type `surface`.
    pub fn depth(&self, penetrate_type: u8, surface: usize) -> f32 {
        match penetrate_type {
            1..=3 => self.depths[penetrate_type as usize - 1].get(surface).copied().unwrap_or(0.0),
            _ => 0.0,
        }
    }

    /// `BULLET_PEN_TABLE\small_bark\20\small_brick\6\...`.
    pub fn parse(text: &str) -> Penetration {
        let mut table = Penetration::default();
        let mut parts = text.trim_end_matches('\0').split('\\').skip(1);
        while let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            let Some((size, surface)) = key.split_once('_') else { continue };
            let row = match size {
                "small" => 0,
                "medium" => 1,
                "large" => 2,
                _ => continue,
            };
            if let (Some(s), Ok(v)) = (collision::SURFACE_NAMES.iter().position(|n| *n == surface), value.trim().parse()) {
                table.depths[row][s] = v;
            }
        }
        table
    }
}

fn load_penetration(mut table: ResMut<Penetration>, content: Res<crate::content::Content>) {
    let text = content.zones.iter().find_map(|z| {
        z.assets.iter().find_map(|a| match a {
            iw3::zone::Asset::RawFile(r) if r.name.eq_ignore_ascii_case("info/bullet_penetration_mp") => {
                Some(String::from_utf8_lossy(&r.data).into_owned())
            }
            _ => None,
        })
    });
    match text {
        Some(t) => {
            *table = Penetration::parse(&t);
            info!("bullet penetration: concrete {}/{}/{}, wood {}/{}/{}", table.depth(1, 5), table.depth(2, 5), table.depth(3, 5), table.depth(1, 21), table.depth(2, 21), table.depth(3, 21));
        }
        None => warn!("no info/bullet_penetration_mp: bullets won't go through anything"),
    }
}

/// A weapon def's sound alias (`SndAliasCustom`: the alias by name).
fn sound(w: &iw3::zone::generic::GNode, field: &str) -> String {
    w.node(field).and_then(|n| n.node("name")).and_then(|n| n.string("soundName")).unwrap_or_default().to_owned()
}

static WEAPON: std::sync::OnceLock<WeaponDef> = std::sync::OnceLock::new();

/// The weapon everyone carries (the AK-47 from `common_mp`).
pub fn weapon_def() -> &'static WeaponDef {
    WEAPON.get_or_init(WeaponDef::fallback)
}

fn load_weapon(content: Res<crate::content::Content>) {
    if let Some((zi, node)) = content.generic(iw3::zone::AssetType::Weapon, "ak47_mp") {
        let def = WeaponDef::from_node(&content.zones[zi], node);
        info!(
            "weapon {}: {} dmg, {:.0} rpm, {} rounds, view {}, world {}",
            def.name,
            def.damage,
            60.0 / def.fire_time,
            def.clip_size,
            def.viewmodel,
            def.world_model
        );
        let _ = WEAPON.set(def);
    } else {
        warn!("ak47_mp weapon def not found; using fallback stats");
    }
}

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct WeaponSet;

/// What the pawn wants its weapon to do this frame.
#[derive(Component, Default, Clone, Copy, Debug)]
pub struct WeaponInput {
    pub fire: bool,
    pub ads: bool,
    pub reload: bool,
    /// Look the gun over ([`crate::viewmodel`]; the local player's only).
    pub inspect: bool,
}

#[derive(Component, Clone, Debug)]
pub struct WeaponState {
    pub def: &'static WeaponDef,
    pub clip: u32,
    pub reserve: u32,
    pub next_fire: f32,
    /// While reloading: when the current part of the reload ends.
    pub reload_until: Option<f32>,
    /// Which part that is, and when its rounds go in (`None` once they
    /// have).
    pub reload_phase: ReloadPhase,
    pub reload_add_at: Option<f32>,
    /// The trigger was held last frame (a fresh pull cuts a segmented
    /// reload short).
    pub fire_held: bool,
    /// 0 = hip, 1 = fully aimed down sights.
    pub ads: f32,
    /// CoD4's `aimSpreadScale` (0 to 1): where the spread is between its
    /// minimum and maximum ([`WeaponState::spread`]).
    pub bloom: f32,
    /// The view kick's speed (pitch up, yaw left; degrees/s), and its
    /// angle now (CoD4's `kickAVel`, `kickAngles`: the aim is the view plus
    /// this, so it's moved along with it).
    pub pending_kick: Vec2,
    pub kick_angles: Vec2,
    /// The first shots' reduced kick lasts until this runs out (s).
    pub restrict_kick: f32,
    /// No firing until then: sprinting's end (`sprintOutTime`).
    pub sprint_out_until: f32,
    /// A pull's first shot waits for the weapon's `iFireDelay`: until then.
    pub first_shot_at: Option<f32>,
    /// The gun's kick in the hands (CoD angles: pitch down, yaw left;
    /// degrees) and its speed (`vGunOffset`, `vGunSpeed`).
    pub gun_offset: Vec2,
    pub gun_speed: Vec2,
    /// Last frame's view (pitch, yaw; radians), for the spread from turning.
    pub last_view: Option<Vec2>,
    pub shots_fired: u32,
    /// Monotonic shot counter (drives fire animations).
    pub shots_fired_total: u32,
    /// A deployed bipod's stats ([`crate::bipod`]): fire rate, spread, kick
    /// and damage come from here while set; the gun in hand stays `def`.
    pub mounted: Option<&'static WeaponDef>,
}

impl Default for WeaponState {
    fn default() -> Self {
        let def = weapon_def();
        WeaponState {
            def,
            clip: def.clip_size,
            reserve: def.max_ammo.min(def.clip_size * 3),
            next_fire: 0.0,
            reload_until: None,
            reload_phase: ReloadPhase::Whole,
            reload_add_at: None,
            fire_held: false,
            ads: 0.0,
            bloom: 0.0,
            pending_kick: Vec2::ZERO,
            kick_angles: Vec2::ZERO,
            restrict_kick: 0.0,
            sprint_out_until: 0.0,
            first_shot_at: None,
            gun_offset: Vec2::ZERO,
            gun_speed: Vec2::ZERO,
            last_view: None,
            shots_fired: 0,
            shots_fired_total: 0,
            mounted: None,
        }
    }
}

/// The parts of a reload ([`WeaponState::reload_phase`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReloadPhase {
    /// The whole clip at once, at the end.
    #[default]
    Whole,
    /// A segmented reload's start, one cycle of its loop, and its end.
    Start,
    Loop,
    End,
}

impl WeaponState {
    pub fn reloading(&self) -> bool {
        self.reload_until.is_some()
    }

    /// Start, advance and finish reloads (`PM_Weapon_CheckForReload` and
    /// the reload states): the whole clip at the end of `reload_time`
    /// (`reload_empty_time` when empty), or a segmented reload's start, a
    /// loop cycle per round and end. A fresh pull of the trigger cuts a
    /// segmented reload to its end once there's a round to fire.
    fn update_reload(&mut self, reload: bool, fire: bool, now: f32) {
        let def = self.def;
        let fresh_pull = fire && !self.fire_held;
        self.fire_held = fire;
        let Some(until) = self.reload_until else {
            if (reload || (self.clip == 0 && fire)) && self.clip < def.clip_size && self.reserve > 0 {
                self.shots_fired = 0;
                if !def.segmented_reload {
                    let t = if self.clip == 0 { def.reload_empty_time } else { def.reload_time };
                    self.reload_part(ReloadPhase::Whole, now, t, None);
                } else if def.reload_start_time > 0.0 {
                    self.reload_part(ReloadPhase::Start, now, def.reload_start_time, Some(def.reload_start_add_time));
                } else {
                    self.reload_part(ReloadPhase::Loop, now, def.reload_time.max(0.05), Some(def.reload_add_time));
                }
            }
            return;
        };
        // Rounds go in partway through a part.
        if self.reload_add_at.is_some_and(|at| now >= at) {
            self.reload_add_at = None;
            match self.reload_phase {
                ReloadPhase::Start => self.load(def.reload_start_add),
                ReloadPhase::Loop => self.load(def.reload_ammo_add.max(1)),
                _ => {}
            }
        }
        if fresh_pull && self.clip > 0 && matches!(self.reload_phase, ReloadPhase::Start | ReloadPhase::Loop) {
            self.reload_part(ReloadPhase::End, now, def.reload_end_time, None);
            return;
        }
        if now < until {
            return;
        }
        // Each part follows on from the last one's end.
        match self.reload_phase {
            ReloadPhase::Whole => {
                self.load(def.clip_size);
                self.reload_until = None;
            }
            ReloadPhase::Start | ReloadPhase::Loop if self.clip < def.clip_size && self.reserve > 0 => {
                self.reload_part(ReloadPhase::Loop, until, def.reload_time.max(0.05), Some(def.reload_add_time));
            }
            ReloadPhase::Start | ReloadPhase::Loop => self.reload_part(ReloadPhase::End, until, def.reload_end_time, None),
            ReloadPhase::End => self.reload_until = None,
        }
    }

    /// Begin a part of a reload `length` long from `from`, its rounds going
    /// in `add` into it (or at its end, when that's 0 or past it).
    fn reload_part(&mut self, phase: ReloadPhase, from: f32, length: f32, add: Option<f32>) {
        self.reload_phase = phase;
        self.reload_until = Some(from + length);
        self.reload_add_at = add.map(|a| from + if a > 0.0 && a < length { a } else { length });
    }

    /// Up to `rounds` from the reserve into the clip.
    fn load(&mut self, rounds: u32) {
        let take = rounds.min(self.def.clip_size.saturating_sub(self.clip)).min(self.reserve);
        self.clip += take;
        self.reserve -= take;
    }

    /// Current cone half-angle in degrees (`Weapon_Fire`): fully aimed, from
    /// the ADS spread towards the stance's maximum; otherwise from the
    /// stance's minimum towards its maximum; by the aim spread scale.
    pub fn spread(&self, mover: &Mover) -> f32 {
        let d = self.mounted.unwrap_or(self.def);
        let i = match mover.stance {
            Stance::Stand => 0,
            Stance::Crouch => 1,
            Stance::Prone => 2,
        };
        let (min, max) = (d.hip_spread_min[i], d.hip_spread_max[i]);
        if self.ads >= 1.0 { d.ads_spread + (max - d.ads_spread) * self.bloom } else { min + (max - min) * self.bloom }
    }

    /// `PM_AdjustAimSpreadScale`: the scale grows while not fully aimed
    /// (moving, turning, in the air), else decays by the weapon's rate
    /// (scaled by stance; halved in the air).
    fn adjust_spread(&mut self, mover: &Mover, view: Vec2, dt: f32) {
        let d = self.mounted.unwrap_or(self.def);
        let turned = self.last_view.map_or(Vec2::ZERO, |l| (view - l).abs());
        self.last_view = Some(view);
        if d.hip_spread_decay == 0.0 {
            self.bloom = 0.0;
            return;
        }
        let mut rate = d.hip_spread_decay;
        if !mover.on_ground {
            rate *= 0.5;
        } else if mover.stance == Stance::Prone {
            rate *= d.gunplay.spread_decay_stance.1;
        } else if mover.stance == Stance::Crouch {
            rate *= d.gunplay.spread_decay_stance.0;
        }
        let mut increase = 0.0;
        if self.ads < 1.0 {
            // Turning: a hundredth per degree, times the weapon's add.
            increase += (turned.x.to_degrees() + turned.y.to_degrees()) * 0.01 * d.gunplay.spread_turn_add;
            let speed = mover.horizontal_speed();
            if d.hip_spread_move_add != 0.0 && speed > u(SPREAD_MOVE_THRESHOLD) {
                increase += d.hip_spread_move_add * (speed / crate::movement::RUN_SPEED) * dt;
            }
            if !mover.on_ground {
                increase += 2.0 * 1.28 * dt;
            }
        }
        self.bloom = if increase > 0.0 { self.bloom + increase } else { self.bloom - rate * dt }.clamp(0.0, 1.0);
    }

    /// `BG_CalculateWeaponPosition_GunRecoil`: the gun's kick in 5 ms
    /// steps per axis: capped, sprung back by `accel`, its speed decaying
    /// in proportion and by a constant, never faster than `speed_max`;
    /// stopped once small.
    fn settle_gun(&mut self, dt: f32) {
        let d = self.mounted.unwrap_or(self.def);
        let g = &d.gunplay;
        let k = g.gun_kick[0].lerp(&g.gun_kick[1], self.ads);
        let caps = [g.gun_max.0, g.gun_max.1];
        let mut left = dt;
        while left > 0.0 {
            let ft = left.min(0.005);
            left -= ft;
            for i in 0..2 {
                let (mut o, mut s) = (self.gun_offset[i], self.gun_speed[i]);
                if o.abs() < 0.25 && s.abs() < 1.0 {
                    self.gun_offset[i] = 0.0;
                    self.gun_speed[i] = 0.0;
                    continue;
                }
                o += s * ft;
                if o > caps[i] {
                    o = caps[i];
                    s = s.min(0.0);
                } else if o < -caps[i] {
                    o = -caps[i];
                    s = s.max(0.0);
                }
                if o > 0.0 {
                    s -= k.accel * ft;
                } else if o < 0.0 {
                    s += k.accel * ft;
                }
                s -= s * k.speed_decay * ft;
                s = if s <= 0.0 { (s + k.static_decay * ft).min(0.0) } else { (s - k.static_decay * ft).max(0.0) };
                s = s.clamp(-k.speed_max, k.speed_max);
                self.gun_offset[i] = o;
                self.gun_speed[i] = s;
            }
        }
    }

    /// `CG_KickAngles`: the view kick in 5 ms steps, per axis: pulled back
    /// to centre at the weapon's centre speed (hip below half aimed), only
    /// 6% as fast on the way back, never past 10 degrees.
    fn settle_kick(&mut self, dt: f32) {
        let d = self.mounted.unwrap_or(self.def);
        let center = if self.ads > 0.5 { d.gunplay.kick_center.1 } else { d.gunplay.kick_center.0 };
        let mut left = dt;
        while left > 0.0 {
            let ft = left.min(0.005);
            left -= ft;
            for i in 0..2 {
                let (mut angle, mut speed) = (self.kick_angles[i], self.pending_kick[i]);
                if speed == 0.0 && angle == 0.0 {
                    continue;
                }
                if angle != 0.0 {
                    speed += if angle <= 0.0 { center } else { -center } * ft;
                }
                let mut change = speed * ft;
                if angle * change < 0.0 {
                    change *= 0.06;
                }
                if (angle + change) * angle < 0.0 {
                    angle = 0.0;
                    speed = 0.0;
                } else {
                    angle += change;
                    if angle == 0.0 {
                        speed = 0.0;
                    } else if angle.abs() > MAX_KICK {
                        angle = MAX_KICK.copysign(angle);
                        speed = 0.0;
                    }
                }
                self.kick_angles[i] = angle;
                self.pending_kick[i] = speed;
            }
        }
    }

    /// The weapon's scale on movement speed. Aiming down sights is CoD4's
    /// "walking" (`PM_CmdScale_Walk`): 0.4 of the speed, times the weapon's
    /// `adsMoveSpeedScale` in place of its `moveSpeedScale` (1 for rifles,
    /// so 40%; 2 for SMGs and pistols, so 80%). Blended over the move into
    /// sights.
    /// The pull has fired all it will: a single shot fired, or a burst
    /// finished. Let go to fire again.
    pub fn trigger_spent(&self) -> bool {
        let burst = self.def.burst();
        (self.def.fire_type == 1 && self.shots_fired >= 1) || (burst > 0 && self.shots_fired >= burst)
    }

    pub fn speed_scale(&self) -> f32 {
        let d = self.def;
        let ads = if d.ads_move_speed_scale > 0.0 { ADS_WALK_SCALE * d.ads_move_speed_scale } else { ADS_WALK_SCALE };
        d.move_speed_scale + (ads - d.move_speed_scale) * self.ads
    }
}

/// Stopping Power's and Juggernaut's scales (`perk_bulletDamage` 40,
/// `perk_armorVest` 75), as `crate::loadout` and `crate::combat` apply them.
const STOPPING_POWER: f32 = 1.4;
const JUGGERNAUT: f32 = 0.75;

/// A bullet's damage (`Bullet_GetDamage`, `G_GetWeaponHitLocationMultiplier`):
/// full to `max_damage_range` units, down to `min_damage` at
/// `min_damage_range`, in whole points; times what's left of it after
/// walls (whole points), times the weapon's multiplier where it hit.
pub fn bullet_damage(def: &WeaponDef, dist_units: f32, strength: f32, location: HitLocation) -> f32 {
    let range = def.min_damage_range - def.max_damage_range;
    let base = if def.damage == def.min_damage || range == 0.0 || dist_units <= def.max_damage_range {
        def.damage
    } else if dist_units >= def.min_damage_range {
        def.min_damage
    } else {
        let t = (dist_units - def.max_damage_range) / range;
        (def.min_damage * t + def.damage * (1.0 - t)).trunc()
    };
    let after_walls = (base * strength).trunc();
    let i = match location {
        HitLocation::Head => 0,
        HitLocation::Neck => 1,
        HitLocation::Torso => 2,
        HitLocation::TorsoLower => 3,
        HitLocation::Legs => 4,
    };
    after_walls * def.gunplay.location_mult[i]
}

/// `bg_aimSpreadMoveSpeedThreshold`: slower than this (units/s), moving
/// doesn't spread the aim.
const SPREAD_MOVE_THRESHOLD: f32 = 11.0;
/// The view kick goes no further than this (degrees).
const MAX_KICK: f32 = 10.0;

/// `player_burstFireCooldown`: the pause after a burst.
const BURST_COOLDOWN: f32 = 0.2;

/// A weapon file's `fireType` name (Black Ops, World at War) as CoD4's
/// number: "Single Shot" 1, "3-Round Burst" 3, anything else full auto.
pub fn fire_type_named(name: &str) -> i32 {
    let n = name.to_ascii_lowercase();
    if n.contains("single") {
        1
    } else {
        (2..=4).find(|k| n.starts_with(&k.to_string()) && n.contains("burst")).unwrap_or(0)
    }
}

/// CoD4's speed scale while aiming down sights, before the weapon's own.
const ADS_WALK_SCALE: f32 = 0.4;

/// Free aim (Bodycam gunplay): shots leave along the weapon's own barrel
/// rather than the centre of the view. Set each frame by [`crate::bodycam`].
#[derive(Component, Clone, Copy, Debug)]
pub struct FreeAim {
    pub origin: Vec3,
    pub rotation: Quat,
    /// Cone half-angle in degrees, replacing the hip/ADS spread.
    pub spread: f32,
    /// Fraction of the weapon's view kick that moves the view; the weapon
    /// itself takes the rest.
    pub view_kick: f32,
    /// The muzzle is pressed against a wall: no firing or aiming.
    pub blocked: bool,
}

#[derive(Message, Clone, Copy)]
pub struct ShotFired {
    pub shooter: Entity,
    /// Captured at firing time for weapon records; synthetic HUD events
    /// have no weapon and must not add to a player's weapon totals.
    pub weapon: Option<&'static WeaponDef>,
    pub from: Vec3,
    pub to: Vec3,
    pub hit_pawn: bool,
    pub normal: Vec3,
    pub hit_world: bool,
}

/// Sent to the shooter's HUD when a shot hurts a live enemy (not a teammate,
/// with friendly fire off, nor a body).
#[derive(Message, Clone, Copy)]
pub struct HitConfirmed {
    pub shooter: Entity,
    pub headshot: bool,
}

/// A bullet hit a pawn after going through a wall (for the challenges).
#[derive(Message, Clone, Copy, Debug)]
pub struct Penetrated {
    pub shooter: Entity,
    pub target: Entity,
}

fn update_weapons(
    time: Res<Time>,
    spatial: SpatialQuery,
    mut shooters: Query<
        (
            Entity,
            &Transform,
            &Mover,
            &mut ViewAngles,
            &mut WeaponState,
            &WeaponInput,
            Option<&FreeAim>,
            Has<crate::grenades::Offhand>,
            Option<&crate::loadout::Loadout>,
            Has<crate::splitscreen::LocalSlot>,
        ),
        Without<Dead>,
    >,
    hitboxes: Query<&Hitbox>,
    pawns: Query<(&crate::combat::Pawn, Has<Dead>)>,
    loadouts: Query<&crate::loadout::Loadout>,
    mut damage: MessageWriter<Damage>,
    mut shots: MessageWriter<ShotFired>,
    mut hits: MessageWriter<HitConfirmed>,
    mut penetrated: MessageWriter<Penetrated>,
    explosives: Res<crate::explosives::ExplosiveDefs>,
    mut launches: MessageWriter<crate::explosives::Launch>,
    penetration: Res<Penetration>,
    surfaces: Query<&collision::Surfaces>,
) {
    let now = time.elapsed_secs();
    let dt = time.delta_secs();
    let filter = collision::bullet_filter();
    let mut rng = rand::rng();
    let idle = WeaponInput::default();
    for (shooter, tf, mover, mut view, mut w, input, free_aim, throwing, loadout, player) in &mut shooters {
        // Throwing a grenade: the gun is down.
        let input = if throwing { &idle } else { input };
        // C4 in a player's hand: aim (`+speed_throw`) throws it, fire is the
        // detonator ([`crate::explosives`]).
        let c4_throw;
        let input = if player && w.def.name == "c4_mp" {
            c4_throw = WeaponInput { fire: input.ads, ads: false, ..*input };
            &c4_throw
        } else {
            input
        };
        // On a deployed bipod, its stats (the same gun otherwise).
        let def = w.mounted.unwrap_or(w.def);
        let blocked = free_aim.is_some_and(|a| a.blocked);
        w.update_reload(input.reload, input.fire, now);

        // Nothing to aim with C4 or claymores (no zoom).
        let wants_ads = input.ads && def.ads_fov > 0.0 && !w.reloading() && !mover.sprinting && !blocked;
        w.ads = if wants_ads {
            (w.ads + dt / def.ads_trans_in.max(0.01)).min(1.0)
        } else {
            (w.ads - dt / def.ads_trans_out.max(0.01)).max(0.0)
        };
        w.adjust_spread(mover, Vec2::new(view.pitch, view.yaw), dt);
        w.restrict_kick = (w.restrict_kick - dt).max(0.0);

        // The view kick: the view moves with it as it settles.
        w.settle_gun(dt);
        let before = w.kick_angles;
        w.settle_kick(dt);
        let moved = w.kick_angles - before;
        view.pitch = (view.pitch + moved.x.to_radians()).clamp(-1.5, 1.5);
        view.yaw += moved.y.to_radians();
        // Back from sprinting, the gun comes up before it fires.
        if mover.sprinting {
            w.sprint_out_until = now + def.sprint_out_time;
        }

        // CoD4's fire types (`ShotLimitReached`, `BurstFirePending`): a
        // single shot per pull, or a burst that finishes even if the trigger
        // is let go; holding on afterwards fires nothing more.
        let burst = def.burst();
        let mid_burst = burst > 0 && w.shots_fired > 0 && w.shots_fired < burst;
        if !input.fire && !mid_burst {
            w.shots_fired = 0;
        }
        let pulled = (input.fire || mid_burst) && !w.trigger_spent();
        // `PM_Weapon_StartFiring`: a fresh pull fires after `iFireDelay`.
        if !pulled || w.shots_fired > 0 {
            w.first_shot_at = None;
        } else if w.first_shot_at.is_none() {
            w.first_shot_at = Some(now + def.gunplay.fire_delay);
        }
        let delayed = w.first_shot_at.is_some_and(|at| now < at);
        let can_fire = pulled && w.clip > 0 && !w.reloading() && !mover.sprinting && !blocked && now >= w.next_fire && now >= w.sprint_out_until && !delayed;
        if !can_fire {
            continue;
        }
        w.clip -= 1;
        w.next_fire = now + def.fire_time + def.rechamber_time;
        w.shots_fired += 1;
        if burst > 0 && w.shots_fired == burst {
            w.next_fire += BURST_COOLDOWN;
        }
        w.shots_fired_total += 1;

        // Spread: a random direction within the cone, its distance from the
        // middle uniform (`Bullet_RandomDir`: more shots near the middle).
        let spread = free_aim.map_or_else(|| w.spread(mover), |a| a.spread).to_radians();
        let r = spread * rng.random::<f32>();
        let theta = rng.random_range(0.0..std::f32::consts::TAU);
        let (eye, base) = free_aim.map_or_else(|| (mover.eye(tf.translation), view.rotation()), |a| (a.origin, a.rotation));
        let aim = base * Quat::from_euler(EulerRot::YXZ, r * theta.cos(), r * theta.sin(), 0.0);
        let dir = aim * Vec3::NEG_Z;

        // `BG_WeaponFireRecoil`: the kick's speed, set afresh each shot
        // (the weapon's numbers are degrees/s; stored in hundredths), less
        // over a burst's first shots.
        let aimed = w.ads >= 1.0;
        if w.shots_fired == 1 {
            let (bullets, _) = def.gunplay.reduced_kick[aimed as usize];
            w.restrict_kick = def.gunplay.fire_delay + bullets * def.fire_time;
        }
        let reduce = if w.restrict_kick > 0.0 { def.gunplay.reduced_kick[aimed as usize].1 * 0.01 } else { 1.0 };
        let (kp, ky) = if aimed { (def.ads_kick_pitch, def.ads_kick_yaw) } else { (def.kick_pitch, def.kick_yaw) };
        // `random() * (max - min) + min`: either way round.
        let mut range = |r: (f32, f32)| r.0 + rng.random::<f32>() * (r.1 - r.0);
        let kick = Vec2::new(range(kp), range(ky)) * 100.0 * reduce;
        w.pending_kick = kick * free_aim.map_or(1.0, |a| a.view_kick);
        // The gun in hand kicks too (hip until fully aimed).
        let gk = &def.gunplay.gun_kick[(w.ads > 0.0) as usize];
        w.gun_speed += Vec2::new(range(gk.pitch), range(gk.yaw)) * reduce;
        // `PM_Weapon_AddFiringAimSpreadScale`: not when fully aimed.
        if !aimed {
            w.bloom = (w.bloom + def.hip_spread_fire_add).min(1.0);
        }

        // An explosive weapon launches its projectile along the aim
        // ([`crate::explosives`]).
        if explosives.get(&def.name).is_some() {
            launches.write(crate::explosives::Launch {
                shooter,
                weapon: def.name.clone(),
                from: eye,
                dir: base * Vec3::NEG_Z,
                yaw: view.yaw,
            });
            shots.write(ShotFired { shooter, weapon: Some(def), from: eye, to: eye, hit_pawn: false, normal: Vec3::Y, hit_world: false });
            continue;
        }

        // The bullet: through players and thin surfaces, damage dwindling by
        // the depth each uses, each player hit once ([`Penetration`]).
        let max_dist = u(8192.0);
        let deep = if loadout.is_some_and(|l| l.class.perks.iter().any(|p| p == "specialty_bulletpenetration")) { DEEP_IMPACT } else { 1.0 };
        let mut passed: Vec<Entity> = vec![shooter];
        let (mut origin, mut travelled, mut strength) = (eye, 0.0f32, 1.0f32);
        let mut first: Option<(Vec3, Vec3, bool, bool)> = None;
        let mut walls = 0;
        let Ok(ray) = Dir3::new(dir) else { continue };
        for _ in 0..=MAX_PENETRATIONS {
            let skip = |e: Entity| hitboxes.get(e).map_or(true, |h| !passed.contains(&h.owner));
            let Some(h) = spatial.cast_ray_predicate(origin, ray, max_dist - travelled, true, &filter, &skip) else { break };
            let point = origin + dir * h.distance;
            travelled += h.distance;
            let hitbox = hitboxes.get(h.entity).ok();
            first.get_or_insert((point, h.normal, hitbox.is_some(), hitbox.is_none()));
            let surface = match hitbox {
                Some(hb) => {
                    let amount = bullet_damage(def, travelled / crate::units::INCH, strength, hb.location);
                    // Stopping Power against Juggernaut: they cancel
                    // (`cac_modified_damage`), where they'd otherwise
                    // make 1.4 x 0.75.
                    let attacker_power = loadout.is_some_and(|l| crate::perks::has(Some(l), "specialty_bulletdamage"));
                    let armoured = crate::perks::has(loadouts.get(hb.owner).ok(), "specialty_armorvest");
                    let amount = if attacker_power && armoured { amount / (STOPPING_POWER * JUGGERNAUT) } else { amount };
                    damage.write(Damage {
                        target: hb.owner,
                        attacker: Some(shooter),
                        amount,
                        location: hb.location,
                        weapon: def.display_name,
                    });
                    let enemy = matches!((pawns.get(shooter), pawns.get(hb.owner)), (Ok((a, _)), Ok((t, false))) if crate::combat::hostile(a, t));
                    if enemy {
                        hits.write(HitConfirmed { shooter, headshot: hb.location == HitLocation::Head });
                    }
                    if walls > 0 {
                        penetrated.write(Penetrated { shooter, target: hb.owner });
                    }
                    passed.push(hb.owner);
                    collision::SURFACE_NAMES.iter().position(|n| *n == "flesh").unwrap_or(0)
                }
                None => {
                    walls += 1;
                    let name = surfaces.get(h.entity).map_or("default", |s| s.facing(h.normal));
                    collision::SURFACE_NAMES.iter().position(|n| *n == name).unwrap_or(0)
                }
            };
            // Out the far side within the depth: traced back from there.
            let depth = penetration.depth(def.penetrate_type, surface) * deep;
            if depth <= 0.0 {
                break;
            }
            let beyond = point + dir * u(depth);
            let entry = h.entity;
            let back = spatial.cast_ray_predicate(beyond, -ray, u(depth), true, &filter, &|e| e == entry);
            let Some(exit) = back.filter(|b| b.distance > 0.0).map(|b| beyond - dir * b.distance) else { break };
            let thickness = (exit - point).length() / crate::units::INCH;
            strength -= thickness / depth;
            debug!("bullet: through {} ({thickness:.1} of {depth} units), {:.0}% left", collision::SURFACE_NAMES[surface], strength.max(0.0) * 100.0);
            if strength <= 0.0 {
                break;
            }
            travelled += (exit - point).length() + u(0.5);
            origin = exit + dir * u(0.5);
        }
        let (to, normal, hit_pawn, hit_world) = first.unwrap_or((eye + dir * max_dist, Vec3::Y, false, false));
        shots.write(ShotFired { shooter, weapon: Some(def), from: eye, to, hit_pawn, normal, hit_world });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A W1200-like gun: 4 rounds, a start loading one, 0.5 s a round, an end.
    fn shotgun() -> &'static WeaponDef {
        Box::leak(Box::new(WeaponDef {
            segmented_reload: true,
            clip_size: 4,
            reload_start_time: 1.0,
            reload_start_add_time: 0.6,
            reload_time: 0.5,
            reload_add_time: 0.25,
            reload_end_time: 0.4,
            reload_start_add: 1,
            reload_ammo_add: 1,
            ..WeaponDef::fallback()
        }))
    }

    /// Run reloads in 10 ms steps from `t` until `until`, pulling the trigger
    /// over `fire` (times).
    fn run(w: &mut WeaponState, t: &mut f32, until: f32, reload: bool, fire: std::ops::Range<f32>) {
        while *t < until {
            w.update_reload(reload, fire.contains(t), *t);
            *t += 0.01;
        }
    }

    #[test]
    fn segmented_reloads_load_a_round_at_a_time() {
        let mut w = WeaponState { def: shotgun(), clip: 0, reserve: 10, ..default() };
        let mut t = 0.0;
        run(&mut w, &mut t, 0.02, true, 0.0..0.0);
        assert_eq!(w.reload_phase, ReloadPhase::Start);
        run(&mut w, &mut t, 0.7, false, 0.0..0.0);
        assert_eq!(w.clip, 1, "the start loads one");
        run(&mut w, &mut t, 1.3, false, 0.0..0.0);
        assert_eq!((w.reload_phase, w.clip), (ReloadPhase::Loop, 2));
        // Start, three rounds, end: 1 + 3 * 0.5 + 0.4 s.
        run(&mut w, &mut t, 2.85, false, 0.0..0.0);
        assert!(w.reloading());
        assert_eq!((w.reload_phase, w.clip), (ReloadPhase::End, 4));
        run(&mut w, &mut t, 2.95, false, 0.0..0.0);
        assert!(!w.reloading());
        assert_eq!((w.clip, w.reserve), (4, 6));
    }

    #[test]
    fn segmented_reloads_stop_short() {
        // The reserve runs out first.
        let mut w = WeaponState { def: shotgun(), clip: 1, reserve: 2, ..default() };
        let mut t = 0.0;
        run(&mut w, &mut t, 3.0, true, 0.0..0.0);
        assert!(!w.reloading());
        assert_eq!((w.clip, w.reserve), (3, 0));
        // A fresh pull once a round is in goes to the end; holding the
        // trigger from an empty clip doesn't.
        let mut w = WeaponState { def: shotgun(), clip: 0, reserve: 10, ..default() };
        let mut t = 0.0;
        run(&mut w, &mut t, 1.15, false, 0.0..1.15);
        assert_eq!((w.reload_phase, w.clip), (ReloadPhase::Loop, 1));
        // Pulled before the loop's round (at 1.25 s) goes in.
        run(&mut w, &mut t, 1.3, false, 1.18..1.3);
        assert_eq!(w.reload_phase, ReloadPhase::End);
        run(&mut w, &mut t, 1.75, false, 0.0..0.0);
        assert!(!w.reloading());
        assert_eq!(w.clip, 1);
    }

    #[test]
    fn bullet_damage_follows_cod4() {
        // The M4: 30 to 1500 units, 20 from 2000, head 1.4.
        let m4 = WeaponDef {
            damage: 30.0,
            min_damage: 20.0,
            max_damage_range: 1500.0,
            min_damage_range: 2000.0,
            gunplay: Gunplay { location_mult: [1.4, 1.0, 1.0, 1.0, 1.0], ..Gunplay::default() },
            ..WeaponDef::fallback()
        };
        assert_eq!(bullet_damage(&m4, 1000.0, 1.0, HitLocation::Torso), 30.0);
        assert_eq!(bullet_damage(&m4, 1750.0, 1.0, HitLocation::Torso), 25.0);
        assert_eq!(bullet_damage(&m4, 1800.0, 1.0, HitLocation::Torso), 24.0, "whole points");
        assert_eq!(bullet_damage(&m4, 5000.0, 1.0, HitLocation::Legs), 20.0);
        assert!((bullet_damage(&m4, 100.0, 1.0, HitLocation::Head) - 42.0).abs() < 1e-4);
        // Through a wall at 60%: 18 of 30.
        assert_eq!(bullet_damage(&m4, 100.0, 0.6, HitLocation::Torso), 18.0);
    }

    #[test]
    fn view_kick_rises_then_settles() {
        let mut w = WeaponState { pending_kick: Vec2::new(60.0, 0.0), ..default() };
        // An AK's hardest hip kick, 60 degrees/s, against 1500: up about
        // 1.2-1.4 degrees in 40 ms (in 5 ms steps).
        w.settle_kick(0.04);
        assert!((1.1..1.5).contains(&w.kick_angles.x), "{:?}", w.kick_angles);
        let peak = w.kick_angles.x;
        // Then back down: a tenth of a second later it is lower (back in
        // about 0.17 s), and in the end centred.
        w.settle_kick(0.1);
        assert!(w.kick_angles.x < peak && w.kick_angles.x > 0.0);
        w.settle_kick(3.0);
        assert_eq!(w.kick_angles, Vec2::ZERO);
        assert_eq!(w.pending_kick, Vec2::ZERO);
    }

    #[test]
    fn penetration_table() {
        let t = Penetration::parse(r"BULLET_PEN_TABLE\small_bark\20\medium_wood\16\large_concrete\12\large_nothing\5");
        assert_eq!(t.depth(1, 1), 20.0);
        assert_eq!(t.depth(2, 21), 16.0);
        assert_eq!(t.depth(3, 5), 12.0);
        assert_eq!(t.depth(0, 5), 0.0, "no penetration type");
        assert_eq!(t.depth(1, 5), 0.0, "not listed");
    }

    #[test]
    fn whole_clip_reloads_are_unchanged() {
        let mut w = WeaponState { def: weapon_def(), clip: 5, reserve: 100, ..default() };
        let mut t = 0.0;
        run(&mut w, &mut t, 2.4, true, 0.0..0.0);
        assert_eq!((w.reload_phase, w.clip), (ReloadPhase::Whole, 5));
        run(&mut w, &mut t, 2.6, false, 0.0..2.6);
        assert!(!w.reloading());
        assert_eq!(w.clip, 30);
    }
}

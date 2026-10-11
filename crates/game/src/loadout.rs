//! Classes in matches: the local player's two weapons (attachments, camo
//! and all), its equipment (C4, claymores or an RPG-7) and perks from a
//! Create a Class class or one of CoD4's default classes, and switching
//! between the weapons (1, 2 or the mouse wheel; 5 for the equipment, or for
//! the primary's grenade launcher, its alternate mode).
//!
//! The class is picked in CoD4's in-game class menu ([`crate::ui`]) and
//! comes into effect at the next spawn. Attachment combinations CoD4 has no
//! weapon for (`ak47:reflex+silencer`) get the base weapon's stats with each
//! single-attachment variant's changes on top. The perks that change
//! something this game models work: Stopping Power, Juggernaut, Sleight of
//! Hand, Double Tap, Steady Aim, Bandolier and Extreme Conditioning (and
//! Overkill, through the class's second weapon).

use crate::combat::{DamageScale, Dead};
use crate::content::Content;
use crate::movement::Mover;
use crate::player::LocalPlayer;
use crate::weapons::{WeaponDef, WeaponInput, WeaponState};

use bevy::prelude::*;

use iw3::zone::AssetType;
use std::collections::HashMap;
use std::sync::Mutex;

pub struct LoadoutPlugin;

impl Plugin for LoadoutPlugin {
    fn build(&self, app: &mut App) {
        // Debug aid: `COD4RW_LOADOUT=<gun spec>[@camo]` (`ak47@6`,
        // `t5_ak47:reflex@15`, `t4_thompson:aperture`) starts the local
        // player with that primary.
        // `COD4RW_INVENTORY=rpg_mp|c4_mp|claymore_mp` gives it that
        // equipment, `COD4RW_PERKS` those perks.
        let choice = std::env::var("COD4RW_LOADOUT").ok().map(|v| {
            let (spec, camo) = v.split_once('@').map_or((v.as_str(), 0), |(s, c)| (s, c.parse().unwrap_or(0)));
            let gun =
                |spec: &str, camo| Gun { spec: spec.into(), camo, name: spec.to_ascii_uppercase(), variant: None };
            let inventory = std::env::var("COD4RW_INVENTORY").ok();
            // `COD4RW_PERKS=specialty_a,specialty_b`: and those perks.
            let perks = std::env::var("COD4RW_PERKS")
                .map_or_else(|_| Vec::new(), |p| p.split(',').map(str::to_owned).collect());
            let inventory_camo = std::env::var("COD4RW_INVENTORY_CAMO").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
            ClassLoadout {
                name: "Debug".into(),
                guns: vec![gun(spec, camo), gun("beretta", 0)],
                perks,
                special: None,
                inventory,
                inventory_camo,
            }
        });
        // The debug class is everyone's (splitscreen players too).
        app.insert_resource(ClassChoice { next: std::array::from_fn(|_| choice.clone()) }).add_systems(
            Update,
            (local_class, equip, local_switch_keys, switch_weapons)
                .chain()
                .after(crate::player::InputSet)
                .before(crate::weapons::WeaponSet)
                .run_if(crate::state::in_game),
        );
    }
}

/// One of a class's weapons.
#[derive(Clone, Debug, PartialEq)]
pub struct Gun {
    /// `weapon:attachment+attachment` (see [`crate::gunmodel`]).
    pub spec: String,
    pub camo: usize,
    /// Its name on the HUD.
    pub name: String,
    /// A supply drop variant's id ([`crate::supply`]).
    pub variant: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ClassLoadout {
    /// What the class menu calls it.
    pub name: String,
    /// Primary, then secondary.
    pub guns: Vec<Gun>,
    /// `specialty_*`.
    pub perks: Vec<String>,
    /// The special grenade (`flash_grenade`, `concussion_grenade`,
    /// `smoke_grenade`), if any.
    pub special: Option<String>,
    /// The perk-1 equipment (`c4_mp`, `claymore_mp`, `rpg_mp`), if any.
    pub inventory: Option<String>,
    /// The equipped RPG's camo. Other equipment stays unchanged.
    pub inventory_camo: usize,
}

/// The class picked in the menu for each local player's next spawn (by
/// their place: Player 1's first).
#[derive(Resource, Default)]
pub struct ClassChoice {
    pub next: [Option<ClassLoadout>; crate::splitscreen::MAX_PLAYERS],
}

/// The local player hasn't picked a class yet and waits to spawn.
#[derive(Component)]
pub struct AwaitingClass;

/// A pawn's class as carried: the weapons and which is in hand.
#[derive(Component)]
pub struct Loadout {
    pub class: ClassLoadout,
    /// Every weapon carried: the class's, then its equipment or its
    /// primary's grenade launcher (the extra slot, on 5).
    guns: Vec<Gun>,
    defs: Vec<&'static WeaponDef>,
    pub current: usize,
    /// The weapons not in hand, with their ammo (none for the one in hand).
    stowed: Vec<Option<WeaponState>>,
    pub switching: Option<Switching>,
    extra: Option<Extra>,
    /// The slot to go back to from the extra one.
    previous: usize,
}

/// The slot on 5: equipment, or the primary's grenade launcher, switched
/// to as the rifle's alternate mode (in `iAltRaiseTime`, with no drop).
#[derive(Clone, Copy, Debug)]
struct Extra {
    slot: usize,
    alt_raise: Option<f32>,
}

impl Loadout {
    pub fn gun(&self) -> &Gun {
        &self.guns[self.current]
    }

    /// The class gun a weapon def was made for.
    pub fn gun_for(&self, def: &WeaponDef) -> Option<&Gun> {
        self.defs.iter().position(|d| std::ptr::eq(*d, def)).map(|i| &self.guns[i])
    }

    /// Last Stand ([`crate::perks`]): the class's pistol in hand at once (a
    /// Beretta if it has none), with all its ammo (`giveMaxAmmo`).
    pub fn take_pistol(&mut self, weapon: &mut WeaponState, content: &Content, now: f32) {
        let pistol = (0..self.class.guns.len().min(self.defs.len())).find(|&i| self.defs[i].class == 4);
        let slot = match pistol {
            Some(i) => i,
            None => {
                let gun = Gun { spec: "beretta:".into(), camo: 0, name: "M9".into(), variant: None };
                let Some(d) = weapon_def(content, &gun, &self.class.perks) else { return };
                self.guns.push(gun);
                self.defs.push(d);
                self.stowed.push(Some(fresh(d, false)));
                self.defs.len() - 1
            }
        };
        if slot != self.current {
            let Some(next) = self.stowed[slot].take() else { return };
            let current = self.current;
            self.stowed[current] = Some(std::mem::replace(weapon, next));
            self.current = slot;
        }
        // A full magazine and all the spare rounds (`giveMaxAmmo`).
        weapon.clip = weapon.def.clip_size;
        weapon.reserve = weapon.def.max_ammo;
        weapon.reload_until = None;
        weapon.ads = 0.0;
        self.switching = Some(Switching { to: slot, raising: true, alt: false, started: now, until: now + weapon.def.raise_time.max(0.05) });
    }

    /// Every weapon carried: its slot, def and clip and reserve (the one in
    /// hand's from `in_hand`).
    pub fn carried(&self, in_hand: &WeaponState) -> Vec<(usize, &'static WeaponDef, u32, u32)> {
        (0..self.defs.len())
            .filter_map(|i| match (i == self.current, &self.stowed[i]) {
                (true, _) => Some((i, in_hand.def, in_hand.clip, in_hand.reserve)),
                (false, Some(w)) => Some((i, w.def, w.clip, w.reserve)),
                (false, None) => None,
            })
            .collect()
    }

    /// Walking over a dropped copy of a weapon carried
    /// ([`crate::pickups`]): up to `amount` more spare rounds for it (in
    /// hand or not), as many as it can carry. How many it took.
    pub fn give_ammo(&mut self, in_hand: &mut WeaponState, def: &WeaponDef, amount: u32) -> u32 {
        let w = if std::ptr::eq(in_hand.def, def) {
            in_hand
        } else {
            match self.stowed.iter_mut().flatten().find(|w| std::ptr::eq(w.def, def)) {
                Some(w) => w,
                None => return 0,
            }
        };
        let took = amount.min(w.def.max_ammo.saturating_sub(w.reserve));
        w.reserve += took;
        took
    }

    /// Picking up a dropped weapon ([`crate::pickups`]): it takes the place
    /// of the one in hand, with its ammo, and comes up (its raise, no
    /// putting away). What was in hand, to drop: its gun, def, clip and
    /// reserve. Not from the extra slot, while switching, or with a grenade
    /// in hand (the caller's to check: [`Loadout::may_swap`]).
    pub fn swap_in_hand(
        &mut self,
        weapon: &mut WeaponState,
        gun: Gun,
        def: &'static WeaponDef,
        clip: u32,
        reserve: u32,
        now: f32,
    ) -> (Gun, &'static WeaponDef, u32, u32) {
        let slot = self.current;
        let old_gun = std::mem::replace(&mut self.guns[slot], gun);
        self.defs[slot] = def;
        let old = (old_gun, weapon.def, weapon.clip, weapon.reserve);
        *weapon = WeaponState { def, clip: clip.min(def.clip_size), reserve: reserve.min(def.max_ammo), ..WeaponState::default() };
        // A grenade launcher went with the rifle (`itemRemoveAmmoFromAltModes`):
        // the slot stays, empty.
        if let Some(x) = self.extra.filter(|x| x.alt_raise.is_some() && slot == 0) {
            if let Some(w) = self.stowed[x.slot].as_mut() {
                (w.clip, w.reserve) = (0, 0);
            }
        }
        self.switching = Some(Switching { to: slot, raising: true, alt: false, started: now, until: now + def.raise_time.max(0.05) });
        old
    }

    /// May the weapon in hand be swapped for a pickup now? Not the extra
    /// slot's, not while switching (or with a grenade in hand: the caller's).
    pub fn may_swap(&self) -> bool {
        self.switching.is_none() && self.extra_slot() != Some(self.current)
    }

    /// The slot on 5: the equipment, or the primary's grenade launcher.
    pub fn extra_slot(&self) -> Option<usize> {
        self.extra.map(|x| x.slot)
    }

    /// The extra slot's weapon (equipment or launcher) and the rounds it has
    /// left: the weapon in hand's when it's that one.
    pub fn equipment(&self, in_hand: &WeaponState) -> Option<(&'static WeaponDef, u32)> {
        let x = self.extra?;
        let ammo = match (&self.stowed[x.slot], x.slot == self.current) {
            (_, true) => in_hand.clip + in_hand.reserve,
            (Some(w), false) => w.clip + w.reserve,
            (None, false) => 0,
        };
        Some((self.defs[x.slot], ammo))
    }
}

/// Putting one weapon away, then raising the other; `alt` for a rifle's
/// grenade launcher (its alternate drop and raise animations).
#[derive(Clone, Copy, Debug)]
pub struct Switching {
    pub to: usize,
    pub raising: bool,
    pub alt: bool,
    pub started: f32,
    pub until: f32,
}

// CoD4's perk dvars.
/// `perk_bulletDamage` 40%.
const STOPPING_POWER: f32 = 1.4;
/// `perk_armorVest` 75%.
const JUGGERNAUT: f32 = 0.75;
/// `perk_weapReloadMultiplier`.
const SLEIGHT_OF_HAND: f32 = 0.5;
/// `perk_weapRateMultiplier`.
const DOUBLE_TAP: f32 = 0.75;
/// `perk_weapSpreadMultiplier`.
const STEADY_AIM: f32 = 0.65;
/// `perk_sprintMultiplier`.
const EXTREME_CONDITIONING: f32 = 2.0;
/// Spare magazines at spawn when a weapon doesn't give its `iStartAmmo`.
const MAGAZINES: u32 = 3;

fn has(perks: &[String], perk: &str) -> bool {
    perks.iter().any(|p| p.eq_ignore_ascii_case(perk))
}

/// Copy the fields `variant` changes from `base` into `out`.
macro_rules! take_changes {
    ($out:ident, $base:ident, $variant:ident: $($field:ident),* $(,)?) => {
        $(if $variant.$field != $base.$field {
            $out.$field = $variant.$field.clone();
        })*
    };
}

pub(crate) fn merge(out: &mut WeaponDef, base: &WeaponDef, variant: &WeaponDef) {
    take_changes!(out, base, variant:
        damage, min_damage, max_damage_range, min_damage_range, fire_time, fire_type, rechamber_time, clip_size, max_ammo,
        reload_time, reload_empty_time, segmented_reload, reload_start_time, reload_start_add_time, reload_add_time,
        reload_end_time, reload_start_add, reload_ammo_add, ads_trans_in, ads_trans_out, sprint_in_time, sprint_loop_time,
        sprint_out_time, drop_time, raise_time, first_raise_time, hip_spread_min, hip_spread_max,
        hip_spread_fire_add, hip_spread_move_add, hip_spread_decay, ads_spread, kick_pitch, kick_yaw,
        ads_kick_pitch, ads_kick_yaw, move_speed_scale, ads_move_speed_scale, ads_fov, ads_view_bob_mult,
        ads_bob_factor, hip_idle_amount, hip_idle_speed, ads_idle_amount, ads_idle_speed,
        idle_crouch_factor, idle_prone_factor, xanims, sounds, impact_type, class, kill_icon, kill_icon_ratio,
        ammo_counter, reticle, display_key, ads_overlay, gunplay,
    );
}

/// Weapon defs made so far, by gun and perks. They live for the whole run
/// (weapon states hold `&'static` defs), and there are only so many guns.
static DEFS: Mutex<Option<HashMap<String, &'static WeaponDef>>> = Mutex::new(None);

/// The weapon def for a gun with its attachments and the class's perks.
fn weapon_def(content: &Content, gun: &Gun, perks: &[String]) -> Option<&'static WeaponDef> {
    let key = format!("{} {} {}", gun.spec, gun.variant.as_deref().unwrap_or("-"), perks.join(" "));
    let mut cache = DEFS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(d) = cache.get_or_insert_default().get(&key) {
        return Some(d);
    }
    let def = |name: &str| content.generic(AssetType::Weapon, name).map(|(zi, n)| WeaponDef::from_node(&content.zones[zi], n));
    let (weapon, attachments) = crate::gunmodel::parse(&gun.spec);
    // Black Ops' and World at War's guns, from their own weapon files.
    let other_game = if crate::bo1::is_bo1(weapon) {
        let bo1 = crate::bo1::data()?;
        Some((bo1.weapon_def(weapon, &attachments)?, bo1.sound_standin(weapon)))
    } else if crate::waw::is_waw(weapon) {
        let waw = crate::waw::data()?;
        Some((waw.weapon_def(weapon, &attachments)?, waw.sound_standin(weapon)))
    } else if crate::mw2guns::is_mw2(weapon) {
        let mw2 = crate::mw2guns::data()?;
        Some((mw2.weapon_def(weapon, &attachments)?, mw2.sound_standin(weapon)))
    } else {
        None
    };
    let mut d = if let Some((mut d, standin)) = other_game {
        // Their own sounds (Black Ops' banks, World at War's zones); a CoD4
        // gun of the same kind fills any they don't name.
        if let Some(standin) = standin.and_then(def) {
            let s = &mut d.sounds;
            for (mine, theirs) in [
                (&mut s.fire, standin.sounds.fire),
                (&mut s.fire_player, standin.sounds.fire_player),
                (&mut s.fire_last, standin.sounds.fire_last),
                (&mut s.fire_last_player, standin.sounds.fire_last_player),
                (&mut s.empty, standin.sounds.empty),
                (&mut s.empty_player, standin.sounds.empty_player),
                (&mut s.reload, standin.sounds.reload),
                (&mut s.raise_player, standin.sounds.raise_player),
                (&mut s.putaway_player, standin.sounds.putaway_player),
            ] {
                if mine.is_empty() {
                    *mine = theirs;
                }
            }
        }
        d
    } else {
        let gl = attachments.contains(&"gl");
        let base = def(&format!("{weapon}_{}mp", if gl { "gl_" } else { "" }))?;
        let mut d = base.clone();
        if !gl {
            // Sights last, so their aiming values win.
            let mut order = attachments.clone();
            order.sort_by_key(|a| matches!(*a, "reflex" | "acog"));
            for att in order {
                if let Some(variant) = def(&format!("{weapon}_{att}_mp")) {
                    merge(&mut d, &base, &variant);
                }
            }
        }
        d.name = format!("{weapon}_{}mp", attachments.iter().map(|a| format!("{a}_")).collect::<String>());
        d
    };
    d.display_name = Box::leak(gun.name.clone().into_boxed_str());
    // A variant is a different gun; perks then work on it like any other.
    if let Some(v) = gun.variant.as_deref().and_then(crate::supply::variant) {
        crate::supply::apply(&mut d, v);
    }
    if has(perks, "specialty_bulletdamage") {
        d.damage *= STOPPING_POWER;
        d.min_damage *= STOPPING_POWER;
    }
    if has(perks, "specialty_fastreload") {
        for t in [
            &mut d.reload_time,
            &mut d.reload_empty_time,
            &mut d.reload_start_time,
            &mut d.reload_start_add_time,
            &mut d.reload_add_time,
            &mut d.reload_end_time,
        ] {
            *t *= SLEIGHT_OF_HAND;
        }
    }
    // Firing and rechambering (`BG_WeaponFireRate`'s states).
    if has(perks, "specialty_rof") {
        d.fire_time *= DOUBLE_TAP;
        d.rechamber_time *= DOUBLE_TAP;
    }
    if has(perks, "specialty_bulletaccuracy") {
        d.hip_spread_min = d.hip_spread_min.map(|s| s * STEADY_AIM);
        d.hip_spread_max = d.hip_spread_max.map(|s| s * STEADY_AIM);
    }
    let d: &'static WeaponDef = Box::leak(Box::new(d));
    cache.get_or_insert_default().insert(key, d);
    Some(d)
}

/// A bot's gun: `spec` with no perks (see [`weapon_def`]).
pub fn bot_weapon(content: &Content, spec: &str) -> Option<&'static WeaponDef> {
    weapon_def(content, &Gun { spec: spec.into(), camo: 0, name: spec.to_ascii_uppercase(), variant: None }, &[])
}

/// Equipment and launchers fresh from spawning: a full clip and the rest of
/// `iMaxAmmo` (two rockets, two C4, two claymores, two launcher grenades).
fn fresh_equipment(def: &'static WeaponDef) -> WeaponState {
    WeaponState { def, clip: def.clip_size, reserve: def.max_ammo.saturating_sub(def.clip_size), ..WeaponState::default() }
}

/// The equipment's name on the HUD.
fn equipment_name(weapon: &str) -> &'static str {
    match weapon {
        "rpg_mp" => "RPG-7",
        "c4_mp" => "C4",
        "claymore_mp" => "Claymore",
        "gl_ak47_mp" => "GP-25",
        _ => "M203",
    }
}

/// A weapon fresh from spawning: a full magazine and `iStartAmmo` spare
/// (`GiveWeapon`), or with Bandolier all it can carry (`giveMaxAmmo`).
pub fn fresh(def: &'static WeaponDef, bandolier: bool) -> WeaponState {
    let start = if def.start_ammo > 0 { def.start_ammo } else { def.clip_size * MAGAZINES };
    let reserve = if bandolier { def.max_ammo } else { start.min(def.max_ammo) };
    WeaponState { def, clip: def.clip_size, reserve, ..WeaponState::default() }
}

/// A pawn's class (any pawn: the player's from the class menu, bots'
/// from wherever they choose): what it's equipped with ([`Loadout`]) each
/// time it spawns.
#[derive(Component, Clone, Debug)]
pub struct PawnClass(pub ClassLoadout);

/// Which slot a pawn wants in hand: 0 its primary, 1 its secondary, and the
/// extra slot (equipment or grenade launcher, [`Loadout::extra_slot`]);
/// cleared once taken. The player's keys set it ([`local_switch_keys`]).
#[derive(Component, Default, Clone, Copy, Debug)]
pub struct SwitchInput {
    pub to: Option<usize>,
}

/// The class picked in the menu becomes the player's, for its next spawn;
/// waiting to spawn, it spawns now.
fn local_class(
    mut commands: Commands,
    time: Res<Time>,
    mut choice: ResMut<ClassChoice>,
    mut players: Query<(
        Entity,
        &crate::splitscreen::LocalSlot,
        &crate::combat::Pawn,
        Option<&mut Dead>,
        Has<AwaitingClass>,
        (&crate::combat::Health, &WeaponState),
    )>,
) {
    for (entity, slot, pawn, dead, awaiting, (health, weapon)) in &mut players {
        let Some(class) = choice.next.get_mut(slot.0).and_then(Option::take) else { continue };
        commands.entity(entity).insert(PawnClass(class));
        // Early in the match, before any fighting (`_menus.gsc`'s
        // `menuClass` in the grace period): the new class now.
        let fought = health.current < crate::combat::max_health() || weapon.shots_fired_total > 0;
        if dead.is_none() && !fought && crate::tdm::in_grace_period(time.elapsed_secs()) {
            commands.entity(entity).remove::<Loadout>();
        }
        if let (Some(mut dead), true) = (dead, awaiting) {
            // Mid-round in Search and Destroy (or while the side holds the
            // HQ) they wait with the rest of the dead.
            if !crate::modes::respawn_locked(pawn.team) {
                dead.respawn_at = time.elapsed_secs();
            }
            commands.entity(entity).remove::<AwaitingClass>();
        }
    }
}

/// Give each pawn with a class its weapons, perks and equipment when it
/// spawns (respawning resets it to the default weapon), and the first time.
#[allow(clippy::type_complexity)]
fn equip(
    mut commands: Commands,
    content: Res<Content>,
    mut respawned: RemovedComponents<Dead>,
    mut pawns: Query<(Entity, &PawnClass, &mut WeaponState, &mut Mover, Has<Loadout>, Has<LocalPlayer>), Without<Dead>>,
) {
    let respawned: Vec<Entity> = respawned.read().collect();
    for (entity, class, mut weapon, mut mover, equipped, local) in &mut pawns {
        if equipped && !respawned.contains(&entity) {
            continue;
        }
        let mut class = class.0.clone();
        // Without Overkill the second weapon is a pistol (`_class.gsc`): a
        // CoD4 class with another gun there gets the Beretta.
        if !has(&class.perks, "specialty_twoprimaries") {
            if let Some(second) = class.guns.get_mut(1) {
                let gun = crate::gunmodel::parse(&second.spec).0;
                let cod4 = !crate::bo1::is_bo1(gun) && !crate::waw::is_waw(gun) && !crate::mw2guns::is_mw2(gun);
                if cod4 && weapon_def(&content, second, &class.perks).is_some_and(|d| d.class != 4) {
                    *second = Gun { spec: "beretta:".into(), camo: 0, name: "M9".into(), variant: None };
                }
            }
        }
        let (guns, defs): (Vec<Gun>, Vec<&'static WeaponDef>) =
            class.guns.iter().filter_map(|g| Some((g.clone(), weapon_def(&content, g, &class.perks)?))).unzip();
        if defs.is_empty() {
            warn!("class {}: none of its weapons were found", class.name);
            continue;
        }
        let perks = &class.perks;
        let bandolier = has(perks, "specialty_extraammo");
        *weapon = fresh(defs[0], bandolier);
        mover.sprint_time_scale = if has(perks, "specialty_longersprint") { EXTREME_CONDITIONING } else { 1.0 };
        let armor = if has(perks, "specialty_armorvest") { JUGGERNAUT } else { 1.0 };
        if local {
            info!(
                "class {}: {:?} perks {:?}, {:?}, {:?}",
                class.name,
                guns.iter().map(|g| (&g.spec, g.camo, &g.variant)).collect::<Vec<_>>(),
                perks,
                class.special,
                class.inventory
            );
        }
        let mut stowed: Vec<Option<WeaponState>> = defs.iter().map(|&d| Some(fresh(d, bandolier))).collect();
        stowed[0] = None;
        // The equipment, or the primary's grenade launcher (its alternate
        // weapon, `szAltWeaponName`).
        let (mut guns, mut defs) = (guns, defs);
        let class = ClassLoadout { guns: guns.clone(), ..class };
        let launcher = || {
            let (weapon, attachments) = crate::gunmodel::parse(&guns[0].spec);
            // (MW2's launcher isn't wired up: its grenade launcher is only
            // the part on the gun.)
            if !attachments.contains(&"gl") || crate::bo1::is_bo1(weapon) || crate::mw2guns::is_mw2(weapon) {
                return None;
            }
            // World at War's rifle grenades, from its weapon files.
            if crate::waw::is_waw(weapon) {
                return crate::waw::launcher(weapon).map(|(alt, raise)| (alt, Some(raise)));
            }
            let (_, rifle) = content.generic(AssetType::Weapon, &format!("{weapon}_gl_mp"))?;
            let alt = rifle.string("szAltWeaponName").filter(|n| !n.is_empty())?.to_owned();
            let raise = content.generic(AssetType::Weapon, &alt).map_or(0.6, |(_, w)| w.int("iAltRaiseTime") as f32 / 1000.0);
            Some((alt, Some(raise)))
        };
        let extra_weapon = class.inventory.clone().map(|i| (i, None)).or_else(launcher);
        let mut extra = None;
        if let Some((weapon, alt_raise)) = extra_weapon {
            let camo = if weapon == "rpg_mp" { class.inventory_camo } else { 0 };
            let gun = Gun {
                spec: format!("{}:", weapon.trim_end_matches("_mp")),
                camo,
                name: equipment_name(&weapon).into(),
                variant: None,
            };
            if let Some(d) = weapon_def(&content, &gun, &[]) {
                extra = Some(Extra { slot: defs.len(), alt_raise });
                guns.push(gun);
                defs.push(d);
                stowed.push(Some(fresh_equipment(d)));
            }
        }
        commands.entity(entity).insert((
            Loadout { class, guns, defs, current: 0, stowed, switching: None, extra, previous: 0 },
            DamageScale(armor),
            SwitchInput::default(),
        ));
    }
}

/// The player's keys for switching: 1 and 2 or the mouse wheel, 5 for the
/// equipment or grenade launcher (and back).
fn local_switch_keys(mut players: Query<(&crate::splitscreen::PlayerInput, &Loadout, &mut SwitchInput)>) {
    for (player, loadout, mut input) in &mut players {
        if !player.live {
            continue;
        }
        let keys = &player.keys;
        let count = loadout.class.guns.len().min(loadout.defs.len());
        let on_extra = loadout.extra_slot() == Some(loadout.current);
        let wanted = if keys.just_pressed(KeyCode::Digit1) {
            Some(0)
        } else if keys.just_pressed(KeyCode::Digit2) && count > 1 {
            Some(1)
        } else if player.scroll != 0.0 && count > 1 {
            // Mid-switch, "next" counts from the gun on its way up (so a
            // quick double tap goes back to the one you had).
            let from = loadout.switching.map_or(loadout.current, |s| s.to);
            Some(if on_extra { loadout.previous } else { (from + 1) % count })
        } else if keys.just_pressed(KeyCode::Digit3) {
            loadout.extra_slot().map(|x| if on_extra { loadout.previous } else { x })
        } else {
            None
        };
        if wanted.is_some() {
            input.to = wanted;
        }
    }
}

/// Switch weapons for every pawn with a class, as its [`SwitchInput`] asks:
/// put the one in hand away, then raise the other. Neither can fire, aim or
/// reload meanwhile.
#[allow(clippy::type_complexity)]
fn switch_weapons(
    time: Res<Time>,
    mut pawns: Query<
        (&mut WeaponState, &mut WeaponInput, &mut Loadout, &mut SwitchInput, Has<crate::grenades::Offhand>, Has<crate::perks::Downed>),
        Without<Dead>,
    >,
) {
    let now = time.elapsed_secs();
    for (mut weapon, mut input, mut loadout, mut switch, throwing, downed) in &mut pawns {
        // Downed in Last Stand: the pistol only.
        if downed {
            switch.to = None;
        }
        let extra = loadout.extra;
        let on_extra = extra.is_some_and(|x| x.slot == loadout.current);
        // Out of claymores: back to the gun (C4 keeps its detonator).
        let spent = on_extra && weapon.clip + weapon.reserve == 0 && now >= weapon.next_fire && weapon.def.name == "claymore_mp";
        let wanted = switch.to.take().or(spent.then_some(loadout.previous));
        // Mid-switch presses, as in CoD4 (the "quick swap"): while a gun
        // comes up, another switch puts it straight back down from where it
        // is; while one goes down, asking for it again brings it back up from
        // there, and asking for a third gun just retargets the swap.
        if let (Some(s), Some(to)) = (loadout.switching, wanted.filter(|&to| to < loadout.defs.len())) {
            if !throwing && !s.alt {
                let span = (s.until - s.started).max(1e-3);
                let done = ((now - s.started) / span).clamp(0.0, 1.0);
                if s.raising && to != loadout.current {
                    loadout.previous = loadout.current;
                    let drop = weapon.def.drop_time.max(0.05) * done;
                    loadout.switching = Some(Switching { to, raising: false, alt: false, started: now - (weapon.def.drop_time.max(0.05) - drop), until: now + drop });
                } else if !s.raising && to == loadout.current {
                    let raise = weapon.def.raise_time.max(0.05);
                    loadout.switching = Some(Switching { to, raising: true, alt: false, started: now - raise * (1.0 - done), until: now + raise * done });
                } else if !s.raising && to != s.to {
                    loadout.switching = Some(Switching { to, ..s });
                }
            }
        }
        if loadout.switching.is_none() && !throwing {
            if let Some(to) = wanted.filter(|&to| to != loadout.current && to < loadout.defs.len()) {
                if !on_extra {
                    loadout.previous = loadout.current;
                }
                // A rifle to its launcher and back: no drop.
                let alt = extra.is_some_and(|x| x.alt_raise.is_some() && (to == x.slot || on_extra) && (to == 0 || loadout.current == 0));
                let drop = if alt { 0.05 } else { weapon.def.drop_time.max(0.05) };
                loadout.switching = Some(Switching { to, raising: false, alt, started: now, until: now + drop });
                weapon.reload_until = None;
            }
        }
        let Some(s) = loadout.switching else { continue };
        *input = WeaponInput::default();
        if now < s.until {
            continue;
        }
        if s.raising {
            loadout.switching = None;
            continue;
        }
        let Some(mut next) = loadout.stowed.get_mut(s.to).and_then(Option::take) else {
            loadout.switching = None;
            continue;
        };
        next.ads = 0.0;
        next.bloom = 0.0;
        next.pending_kick = Vec2::ZERO;
        next.reload_until = None;
        let raise = match (s.alt, loadout.extra.and_then(|x| x.alt_raise)) {
            (true, Some(alt)) => alt,
            _ => next.def.raise_time,
        }
        .max(0.05);
        let current = loadout.current;
        loadout.stowed[current] = Some(std::mem::replace(&mut *weapon, next));
        loadout.current = s.to;
        loadout.switching = Some(Switching { to: s.to, raising: true, alt: s.alt, started: now, until: now + raise });
    }
}

//! Dropped weapons you can pick up, as CoD4 MP has them (`_weapons.gsc`'s
//! `dropWeaponForDeath`, the engine's `Touch_Item` and weapon swap).
//!
//! - Dying (or going down in Last Stand, then not again) drops the last
//!   weapon held that may be dropped (not the extra slot's equipment or
//!   launcher), if it still has rounds in the magazine; not in a match's
//!   first 15 seconds. It's gone after a minute.
//! - Walking over a gun you carry takes its rounds, as many as you can
//!   carry, and the gun is gone (`WeaponPickup_LeechFromWeaponEnt`); its
//!   dropper can't for a second.
//! - Looking at a gun you don't carry, within 128 units and in sight, shows
//!   "Press [Use] to swap for ..." (`PLATFORM_SWAPWEAPONS`); pressing it
//!   takes it in place of the one in hand, which drops where it lay (not
//!   sprinting, mantling, in Last Stand, with a grenade in hand, or for
//!   the extra slot).
//! - At most 16 lie about (`g_maxDroppedWeapons`); the one furthest from
//!   anyone goes first.
//!
//! Dropped guns fall and tumble as clutter does ([`crate::clutter`]).

use crate::combat::{Dead, Pawn};
use crate::content::Content;
use crate::loadout::{Gun, Loadout};
use crate::models::Skeleton;
use crate::movement::{Mover, ViewAngles};
use crate::units::u;
use crate::weapons::{WeaponDef, WeaponState};
use avian3d::prelude::SpatialQuery;
use bevy::ecs::system::SystemParam;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;
use std::collections::{HashMap, HashSet};

pub struct PickupsPlugin;

impl Plugin for PickupsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (give_input, drop_on_death, touch, swap, expire).chain().after(crate::weapons::WeaponSet).run_if(crate::state::in_game),
        );
        if std::env::var_os("COD4RW_PICKUPTEST").is_some() {
            app.add_systems(Update, test.before(swap).after(give_input).run_if(crate::state::in_game));
        }
    }
}

/// A dropped weapon: what it is (its class gun: attachments and camo) and
/// its rounds, who dropped it and when, and when it goes (a death's drop).
#[derive(Component, Clone, Debug)]
pub struct Pickup {
    pub def: &'static WeaponDef,
    pub gun: Gun,
    pub clip: u32,
    pub reserve: u32,
    dropper: Option<Entity>,
    dropped: f32,
    expires: Option<f32>,
}

/// A pawn wants to swap for the weapon it's looking at (the player's Use;
/// bots set it). Cleared once read.
#[derive(Component, Default, Clone, Copy, Debug)]
pub struct PickupInput {
    pub swap: bool,
}

/// The local player's: the weapon a Use would swap for (its name's string
/// key), for the HUD.
#[derive(Component, Default, Clone, Debug)]
pub struct PickupHint(pub Option<String>);

/// `dropWeaponForDeath` isn't in the grace period (s into a match).
const GRACE: f32 = 15.0;
/// `deletePickupAfterAWhile`.
const LIFETIME: f32 = 60.0;
/// `g_maxDroppedWeapons`.
const MAX_DROPPED: usize = 16;
/// `BG_PlayerTouchesItem`: within 36 units across, the item from 88 below
/// the feet to 18 above (CoD units).
const TOUCH_ACROSS: f32 = 36.0;
const TOUCH_BELOW: f32 = 88.0;
const TOUCH_ABOVE: f32 = 18.0;
/// The dropper can't touch it this long (`DroppedItemClearOwner`).
const OWNER_BLOCK: f32 = 1.0;
/// `Player_GetUseList`: items in reach of the eye.
const USE_REACH: f32 = 128.0;
/// `Drop_Item`: forward 10 u/s, ±100 across, 10 ± 5 up.
const DROP_FORWARD: f32 = 10.0;
const DROP_SPREAD: f32 = 100.0;
const DROP_UP: (f32, f32) = (10.0, 5.0);
/// A dropped gun's box: half its height and width; its half length by
/// class (CoD units).
const THICK: (f32, f32) = (3.0, 1.5);

fn half_length(def: &WeaponDef) -> f32 {
    match def.class {
        4 => 6.0,
        2 => 12.0,
        _ => 18.0,
    }
}

/// May this weapon be dropped (`mayDropWeapon`)? Not the extra slot's
/// (equipment, the launcher).
fn droppable(loadout: &Loadout) -> bool {
    loadout.extra_slot() != Some(loadout.current)
}

/// Does a pawn carry `def`?
fn carries(loadout: &Loadout, weapon: &WeaponState, def: &WeaponDef) -> bool {
    loadout.carried(weapon).iter().any(|c| std::ptr::eq(c.1, def))
}

/// For the bots: would walking over `item` give this pawn rounds?
#[allow(dead_code)] // (until the bots use it)
pub fn would_take_ammo(item: &Pickup, loadout: &Loadout, weapon: &WeaponState) -> bool {
    loadout.carried(weapon).iter().any(|&(_, d, _, reserve)| std::ptr::eq(d, item.def) && reserve < d.max_ammo) && item.clip + item.reserve > 0
}

/// For the bots: could this pawn swap for `item` (it doesn't carry it, and
/// the weapon in hand may go)?
#[allow(dead_code)] // (until the bots use it)
pub fn may_swap_for(item: &Pickup, loadout: &Loadout, weapon: &WeaponState) -> bool {
    !carries(loadout, weapon, item.def) && loadout.may_swap() && droppable(loadout)
}

fn give_input(
    mut commands: Commands,
    pawns: Query<Entity, (With<Pawn>, Without<PickupInput>)>,
    locals: Query<Entity, (With<crate::player::LocalPlayer>, Without<PickupHint>)>,
) {
    for e in &pawns {
        commands.entity(e).insert(PickupInput::default());
    }
    for e in &locals {
        commands.entity(e).insert(PickupHint::default());
    }
}

/// What spawning a dropped gun's model takes.
#[derive(SystemParam)]
pub struct GunMaker<'w, 's> {
    commands: Commands<'w, 's>,
    content: ResMut<'w, Content>,
    bo1: ResMut<'w, crate::bo1::MatchContent>,
    waw: ResMut<'w, crate::waw::MatchContent>,
    mw2: ResMut<'w, crate::mw2guns::MatchContent>,
    meshes: ResMut<'w, Assets<Mesh>>,
    materials: ResMut<'w, Assets<StandardMaterial>>,
    images: ResMut<'w, Assets<Image>>,
    bindposes: ResMut<'w, Assets<SkinnedMeshInverseBindposes>>,
    camo_materials: ResMut<'w, Assets<crate::gunmodel::CamoMaterial>>,
    camos: ResMut<'w, crate::gunmodel::WorldCamos>,
}

impl GunMaker<'_, '_> {
    /// A dropped gun at `at`, moving at `velocity`.
    #[allow(clippy::too_many_arguments)]
    fn drop(&mut self, def: &'static WeaponDef, gun: Gun, clip: u32, reserve: u32, at: Transform, velocity: Vec3, dropper: Option<Entity>, now: f32, expires: Option<f32>) -> Option<Entity> {
        let name = crate::gunmodel::parse(&gun.spec).0;
        let (bo1, waw, mw2) = (crate::bo1::is_bo1(name), crate::waw::is_waw(name), crate::mw2guns::is_mw2(name));
        let content: &mut Content = match (self.bo1.get().filter(|_| bo1), self.waw.get().filter(|_| waw), self.mw2.get().filter(|_| mw2)) {
            (Some(c), ..) | (_, Some(c), _) | (.., Some(c)) => c,
            _ if bo1 || waw || mw2 => return None,
            _ => &mut self.content,
        };
        let root = self.commands.spawn((Name::new("dropped weapon"), at, Visibility::default())).id();
        let mut skeleton = Skeleton::default();
        let mut assets = crate::gunmodel::GunAssets {
            meshes: &mut self.meshes,
            materials: &mut self.materials,
            images: &mut self.images,
            bindposes: &mut self.bindposes,
            camo_materials: &mut self.camo_materials,
        };
        let target = crate::gunmodel::GunTarget { owner: root, attach_to: None, layers: None };
        if crate::gunmodel::spawn_held_gun(&mut self.commands, content, &mut self.camos.0, &mut assets, &mut skeleton, &gun.spec, gun.camo, target).is_none() {
            self.commands.entity(root).despawn();
            return None;
        }
        let half = Vec3::new(u(half_length(def)), u(THICK.0), u(THICK.1));
        let spin = Vec3::new(rand::random_range(-2.0..2.0), rand::random_range(-2.0..2.0), rand::random_range(-2.0..2.0));
        self.commands.entity(root).insert((
            skeleton,
            crate::clutter::Body::loose(Vec3::X * half.x * 0.4, half, 3.0, "physics_weapon"),
            crate::clutter::Moving::thrown(velocity, spin),
            Pickup { def, gun, clip, reserve, dropper, dropped: now, expires },
        ));
        Some(root)
    }
}

/// Make room: the drop furthest from anyone goes (`GetFreeDropCueIdx`).
fn make_room(commands: &mut Commands, items: &Query<(Entity, &Pickup, &Transform)>, pawns: &[Vec3], adding: usize) {
    let mut all: Vec<(Entity, f32)> = items
        .iter()
        .map(|(e, _, tf)| (e, pawns.iter().map(|p| p.distance(tf.translation)).fold(f32::MAX, f32::min)))
        .collect();
    let over = (all.len() + adding).saturating_sub(MAX_DROPPED);
    all.sort_by(|a, b| b.1.total_cmp(&a.1));
    for (e, _) in all.into_iter().take(over) {
        commands.entity(e).try_despawn();
    }
}

/// `dropWeaponForDeath`: on dying, or going down in Last Stand (and not
/// again when it dies).
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn drop_on_death(
    time: Res<Time>,
    mut maker: GunMaker,
    state: Option<Res<crate::tdm::MatchState>>,
    pawns: Query<(Entity, &Transform, &Mover, &ViewAngles, &WeaponState, Option<&Loadout>, Has<Dead>, Has<crate::perks::Downed>, Option<&crate::thirdperson::Body>)>,
    places: Query<&GlobalTransform>,
    skeletons: Query<&Skeleton>,
    items: Query<(Entity, &Pickup, &Transform)>,
    // The last droppable weapon each held, with its gun.
    mut last: Local<HashMap<Entity, (&'static WeaponDef, Gun)>>,
    mut dropped: Local<HashSet<Entity>>,
) {
    let now = time.elapsed_secs();
    let grace = state.as_ref().is_some_and(|s| now - s.started < GRACE);
    let positions: Vec<Vec3> = pawns.iter().filter(|p| !p.6).map(|p| p.1.translation).collect();
    for (e, tf, mover, view, weapon, loadout, dead, downed, body) in &pawns {
        let Some(loadout) = loadout else { continue };
        if !dead && !downed {
            dropped.remove(&e);
            if droppable(loadout) {
                // The gun of the weapon in hand, not of the current slot:
                // mid-switch those differ, and a bot killed switching from
                // its AK-47 dropped an AK-47 that looked like its shotgun.
                last.insert(e, (weapon.def, loadout.gun_for(weapon.def).unwrap_or(loadout.gun()).clone()));
            }
            continue;
        }
        if !dropped.insert(e) || grace {
            continue;
        }
        let Some((def, gun)) = last.get(&e).cloned() else { continue };
        // Its rounds, if it still has it; none in the magazine, no drop.
        let Some(&(_, _, clip, reserve)) = loadout.carried(weapon).iter().find(|c| std::ptr::eq(c.1, def)) else { continue };
        if clip == 0 {
            continue;
        }
        // From the hand (or half the player's height), turned only to the
        // view's yaw (`Drop_Item`; CoD models face +X), thrown a little.
        let from = body
            .and_then(|b| skeletons.get(b.0).ok())
            .and_then(|s| s.joint("tag_weapon_right"))
            .and_then(|j| places.get(j).ok())
            .map_or(tf.translation + Vec3::Y * mover.eye_height * 0.5, |g| g.translation());
        let hand = Transform::from_translation(from).with_rotation(Quat::from_rotation_y(view.yaw + std::f32::consts::FRAC_PI_2));
        let forward = Quat::from_rotation_y(view.yaw) * Vec3::NEG_Z;
        let r = || rand::random_range(-1.0f32..1.0);
        let velocity = mover.velocity
            + (forward * DROP_FORWARD + Vec3::new(r() * DROP_SPREAD, DROP_UP.0 + r() * DROP_UP.1, r() * DROP_SPREAD)) * u(1.0);
        make_room(&mut maker.commands, &items, &positions, 1);
        let reserve = reserve.min(def.max_ammo);
        maker.drop(def, gun, clip, reserve, hand, velocity, Some(e), now, Some(now + LIFETIME));
    }
    let alive: HashSet<Entity> = pawns.iter().map(|p| p.0).collect();
    last.retain(|e, _| alive.contains(e));
    dropped.retain(|e| alive.contains(e));
}

/// Walking over a gun carried takes its rounds (`Touch_Item`).
#[allow(clippy::type_complexity)]
fn touch(
    mut commands: Commands,
    time: Res<Time>,
    mut sfx: ResMut<crate::audio::Sfx>,
    mut pawns: Query<(Entity, &Transform, &mut WeaponState, &mut Loadout, Has<crate::player::LocalPlayer>), Without<Dead>>,
    items: Query<(Entity, &Pickup, &Transform)>,
) {
    let now = time.elapsed_secs();
    let mut taken = HashSet::new();
    for (e, tf, mut weapon, mut loadout, local) in &mut pawns {
        for (item, p, at) in &items {
            if taken.contains(&item) || (p.dropper == Some(e) && now - p.dropped < OWNER_BLOCK) {
                continue;
            }
            let d = (tf.translation - at.translation) / u(1.0);
            if d.x.abs() > TOUCH_ACROSS || d.z.abs() > TOUCH_ACROSS || !(-TOUCH_ABOVE..=TOUCH_BELOW).contains(&d.y) {
                continue;
            }
            if !carries(&loadout, &weapon, p.def) {
                continue;
            }
            // Its spare rounds and the magazine's, as spare.
            let took = loadout.give_ammo(&mut weapon, p.def, p.clip + p.reserve);
            if took > 0 {
                taken.insert(item);
                commands.entity(item).try_despawn();
                sfx.play(if local { "weap_pickup" } else { "weap_pickup_npc" }, Some(at.translation));
            }
        }
    }
}

/// Use on a gun not carried swaps it for the one in hand.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn swap(
    time: Res<Time>,
    spatial: SpatialQuery,
    mut maker: GunMaker,
    mut sfx: ResMut<crate::audio::Sfx>,
    mut pawns: Query<
        (
            Entity,
            &Transform,
            &Mover,
            &ViewAngles,
            &mut WeaponState,
            &mut Loadout,
            &mut PickupInput,
            Option<&mut PickupHint>,
            Option<&crate::splitscreen::PlayerInput>,
            Has<crate::grenades::Offhand>,
            Has<crate::perks::Downed>,
        ),
        Without<Dead>,
    >,
    items: Query<(Entity, &Pickup, &Transform)>,
    mut held: Local<HashMap<Entity, bool>>,
) {
    let now = time.elapsed_secs();
    let filter = crate::collision::sight_filter();
    let positions: Vec<Vec3> = pawns.iter().map(|p| p.1.translation).collect();
    let mut gone = HashSet::new();
    for (e, tf, mover, view, mut weapon, mut loadout, mut input, hint, player, offhand, downed) in &mut pawns {
        // The player's Use (F, a pad's interact), as a press.
        if let Some(p) = player {
            let down = p.live && (p.keys.pressed(KeyCode::KeyF) || p.pad.interact);
            let was = held.insert(e, down).unwrap_or(false);
            input.swap |= down && !was;
        }
        let want = std::mem::take(&mut input.swap);
        let able = !mover.sprinting && mover.mantle.is_none() && !offhand && !downed && loadout.may_swap() && droppable(&loadout);
        // `Player_GetUseList`: in reach and in sight, the one looked at most
        // squarely (then the nearest).
        let eye = mover.eye(tf.translation);
        let look = view.forward();
        let target = items
            .iter()
            .filter(|(i, p, _)| !gone.contains(i) && !carries(&loadout, &weapon, p.def))
            .filter_map(|(i, p, at)| {
                let to = at.translation - eye;
                let dist = to.length();
                if dist > u(USE_REACH) {
                    return None;
                }
                let dir = Dir3::new(to).ok()?;
                if spatial.cast_ray(eye, dir, (dist - u(2.0)).max(0.0), true, &filter).is_some() {
                    return None;
                }
                let score = (1.0 - (look.dot(*dir) + 1.0) / 2.0) * 256.0 + dist / u(1.0);
                Some((score, i, p.clone(), *at))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0));
        if let Some(mut h) = hint {
            h.0 = target.as_ref().filter(|_| able).map(|t| t.2.def.display_key.clone());
        }
        let (Some((_, item, p, at)), true, true) = (target, want, able) else { continue };
        let (old_gun, old_def, clip, reserve) = loadout.swap_in_hand(&mut weapon, p.gun.clone(), p.def, p.clip, p.reserve, now);
        gone.insert(item);
        maker.commands.entity(item).try_despawn();
        sfx.play(if player.is_some() { "weap_pickup" } else { "weap_pickup_npc" }, Some(at.translation));
        // What was in hand lies where the pickup did (none if it was empty).
        if clip + reserve > 0 {
            make_room(&mut maker.commands, &items, &positions, 1);
            maker.drop(old_def, old_gun, clip, reserve, at, Vec3::ZERO, Some(e), now, None);
        }
    }
}

/// Deaths' drops go after a minute.
fn expire(mut commands: Commands, time: Res<Time>, items: Query<(Entity, &Pickup)>) {
    let now = time.elapsed_secs();
    for (e, p) in &items {
        if p.expires.is_some_and(|t| now >= t) {
            commands.entity(e).try_despawn();
        }
    }
}

/// Debug aid: with `COD4RW_PICKUPTEST=<dir>` (and a class:
/// `COD4RW_LOADOUT=m4`), at 16 s (past the grace
/// period) a bot is put 120 units in front of the player and shot dead, the
/// view on where its gun lands; a screenshot once it has (the hint), then
/// Use swaps for it, and screenshots of the new gun in hand and the old one
/// on the ground; then exit.
#[allow(clippy::type_complexity)]
pub(crate) fn test(
    mut commands: Commands,
    time: Res<Time>,
    mut player: Query<(Entity, &Transform, &Mover, &mut ViewAngles, &WeaponState, &mut PickupInput, Option<&PickupHint>), With<crate::player::LocalPlayer>>,
    mut bots: Query<(Entity, &mut Transform), (With<Pawn>, Without<crate::player::LocalPlayer>, Without<Pickup>)>,
    items: Query<(&Pickup, &Transform), Without<Pawn>>,
    mut damage: MessageWriter<crate::combat::Damage>,
    mut dead: Query<&mut Dead>,
    mut step: Local<(usize, f32)>,
    mut exit: MessageWriter<AppExit>,
) {
    use bevy::render::view::screenshot::{Screenshot, save_to_disk};
    let Ok(dir) = std::env::var("COD4RW_PICKUPTEST") else { return };
    // The victim stays dead (it would come back and shoot).
    for mut d in &mut dead {
        if step.0 >= 1 {
            d.respawn_at = f32::INFINITY;
        }
    }
    let t = time.elapsed_secs();
    let Ok((me, at, mover, mut view, weapon, mut input, hint)) = player.single_mut() else { return };
    // The player can't be shot meanwhile.
    commands.entity(me).insert(crate::combat::DamageScale(0.0));
    let mut shot = |name: &str| {
        std::fs::create_dir_all(&dir).ok();
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(std::path::Path::new(&dir).join(format!("pickup_{name}.png"))));
    };
    // Look at the nearest dropped gun.
    if step.0 >= 1 {
        if let Some((_, it)) = items.iter().min_by(|a, b| a.1.translation.distance(at.translation).total_cmp(&b.1.translation.distance(at.translation))) {
            let d = it.translation - mover.eye(at.translation);
            view.yaw = (-d.x).atan2(-d.z);
            view.pitch = d.y.atan2(d.with_y(0.0).length());
        }
    }
    match step.0 {
        0 if t >= 16.0 => {
            let forward = (Quat::from_rotation_y(view.yaw) * Vec3::NEG_Z).with_y(0.0).normalize_or_zero();
            let Some((victim, mut tf)) = bots.iter_mut().next() else { return };
            tf.translation = at.translation + forward * u(70.0);
            damage.write(crate::combat::Damage { target: victim, attacker: Some(me), amount: 1000.0, location: crate::combat::HitLocation::Torso, weapon: "m16_mp" });
            info!("pickup test: holding {}, killed {victim:?}", weapon.def.name);
            *step = (1, t);
        }
        1 if t - step.1 > 2.5 => {
            let dropped: Vec<String> = items
                .iter()
                .map(|(p, it)| format!("{} (model {}) {}+{} at {:.0} u from the eye, {:.0} u above the feet", p.def.name, p.gun.spec, p.clip, p.reserve, it.translation.distance(mover.eye(at.translation)) / u(1.0), (it.translation.y - at.translation.y) / u(1.0)))
                .collect();
            info!("pickup test: dropped {dropped:?}, hint {:?}", hint.and_then(|h| h.0.clone()));
            shot("1_hint");
            *step = (2, t);
        }
        2 if t - step.1 > 0.5 => {
            input.swap = true;
            *step = (3, t);
        }
        3 if t - step.1 > 1.5 => {
            let dropped: Vec<String> = items.iter().map(|(p, _)| format!("{} {}+{}", p.def.name, p.clip, p.reserve)).collect();
            info!("pickup test: now holding {} {}+{}, on the ground {dropped:?}", weapon.def.name, weapon.clip, weapon.reserve);
            shot("2_swapped");
            *step = (4, t);
        }
        4 if t - step.1 > 1.0 => {
            exit.write(AppExit::Success);
        }
        _ => {}
    }
}

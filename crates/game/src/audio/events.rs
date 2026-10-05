//! Sounds of the fight: gunfire (and the player's brass), bullet impacts by
//! surface, near misses, the hit marker, footsteps and landings by surface,
//! pain and death, hurt breathing and sprint gasps, reload foley from the
//! viewmodel animations' notetracks, other players' reloads, weapon
//! switches and dry fire.

use super::{Bank, Sfx, Sides};
use crate::collision::{self, Surfaces};
use crate::combat::{Dead, Health, Killed, Pawn};
use crate::loadout::Loadout;
use crate::models::AnimPlayer;
use crate::movement::{Landed, Mover, Stance};
use crate::player::LocalPlayer;
use crate::units::u;
use crate::viewmodel::ViewModelRoot;
use crate::weapons::{HitConfirmed, ShotFired, WeaponInput, WeaponState};
use avian3d::prelude::*;
use bevy::prelude::*;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub(super) fn build(app: &mut App) {
    app.add_systems(
        Update,
        (
            gunfire,
            hit_marker,
            footsteps,
            landings,
            pain_and_death,
            breathing,
            viewmodel_notes,
            other_reloads,
            dry_fire,
            weapon_switch,
            turret_impacts,
        )
            .after(crate::weapons::WeaponSet)
            .after(crate::movement::MovementSet)
            .run_if(crate::state::in_game),
    );
}

/// The impact sounds' prefix for a weapon's `impactType`.
fn impact_kind(impact_type: i32) -> &'static str {
    match impact_type {
        1 => "bullet_small",
        3 => "bullet_ap",
        4 => "bulletspray_small",
        _ => "bullet_large",
    }
}

/// The surface a bullet from `from` hit at `to`, by the brush face there.
fn shot_surface(spatial: &SpatialQuery, surfaces: &Query<&Surfaces>, from: Vec3, to: Vec3) -> &'static str {
    let dir = (to - from).normalize_or_zero();
    Dir3::new(dir)
        .ok()
        .and_then(|d| spatial.cast_ray(to - dir * u(4.0), d, u(8.0), true, &collision::sight_filter()))
        .map_or("default", |h| surfaces.get(h.entity).map_or("default", |sf| sf.facing(h.normal)))
}

/// Where a helicopter's rounds land ([`crate::fx::BulletImpact`]).
fn turret_impacts(
    mut sfx: ResMut<Sfx>,
    mut impacts: MessageReader<crate::fx::BulletImpact>,
    spatial: SpatialQuery,
    surfaces: Query<&Surfaces>,
) {
    use crate::fx::BulletHit;
    for b in impacts.read() {
        let surface = match b.hit {
            BulletHit::World => shot_surface(&spatial, &surfaces, b.from, b.to),
            BulletHit::Flesh { .. } => "flesh",
            BulletHit::Metal => "metal",
        };
        sfx.play(format!("{}_{surface}", impact_kind(b.impact_type)), Some(b.to));
    }
}

/// Head height for voices, in CoD units.
const VOICE_HEIGHT: f32 = 60.0;

fn listener_pos(listener: &Query<&GlobalTransform, With<SpatialListener>>) -> Option<Vec3> {
    listener.iter().next().map(GlobalTransform::translation)
}

/// The surface the ground under `feet` is made of.
fn ground_surface(spatial: &SpatialQuery, surfaces: &Query<&Surfaces>, feet: Vec3) -> &'static str {
    spatial
        .cast_ray(feet + Vec3::Y * u(8.0), Dir3::NEG_Y, u(40.0), true, &collision::movement_filter())
        .map_or("default", |h| surfaces.get(h.entity).map_or("default", |s| s.facing(h.normal)))
}

#[allow(clippy::too_many_arguments)]
fn gunfire(
    time: Res<Time>,
    mut sfx: ResMut<Sfx>,
    mut shots: MessageReader<ShotFired>,
    shooters: Query<(&WeaponState, Has<LocalPlayer>)>,
    spatial: SpatialQuery,
    surfaces: Query<&Surfaces>,
    listener: Query<&GlobalTransform, With<SpatialListener>>,
    mut last_whizby: Local<f32>,
    hitboxes: Query<&crate::combat::Hitbox>,
    pawns: Query<(), With<Pawn>>,
) {
    let now = time.elapsed_secs();
    let ear = listener_pos(&listener);
    for s in shots.read() {
        let Ok((w, local)) = shooters.get(s.shooter) else { continue };
        let snd = &w.def.sounds;
        let last = w.clip == 0;
        if local {
            let alias = if last && !snd.fire_last_player.is_empty() { &snd.fire_last_player } else { &snd.fire_player };
            sfx.play(alias.clone(), None);
            // The spent case hitting the floor.
            let brass = if w.def.class == 4 { "shell_eject_pistol" } else { "shell_eject_rifle" };
            sfx.play_later(brass, None, now, 0.45);
        } else {
            let alias = if last && !snd.fire_last.is_empty() { &snd.fire_last } else { &snd.fire };
            sfx.play(alias.clone(), Some(s.from));
        }

        let kind = impact_kind(w.def.impact_type);
        if s.hit_pawn {
            // A hitbox that isn't a pawn's (a helicopter's) rings as metal.
            let dir = (s.to - s.from).normalize_or_zero();
            let owner = Dir3::new(dir)
                .ok()
                .and_then(|d| spatial.cast_ray(s.to - dir * u(4.0), d, u(8.0), true, &SpatialQueryFilter::from_mask(collision::Layer::Hitbox)))
                .and_then(|h| hitboxes.get(h.entity).ok());
            let metal = owner.is_some_and(|h| pawns.get(h.owner).is_err());
            sfx.play(format!("{kind}_{}", if metal { "metal" } else { "flesh" }), Some(s.to));
        } else if s.hit_world {
            // Which face of which brush: a short ray onto the hit point.
            sfx.play(format!("{kind}_{}", shot_surface(&spatial, &surfaces, s.from, s.to)), Some(s.to));
        }

        // Someone else's bullet passing close by.
        if let Some(ear) = ear.filter(|_| !local) {
            let seg = s.to - s.from;
            let t = ((ear - s.from).dot(seg) / seg.length_squared().max(1e-6)).clamp(0.0, 1.0);
            let closest = s.from + seg * t;
            if t > 0.05 && t < 0.98 && closest.distance(ear) < u(80.0) && now - *last_whizby > 0.12 {
                *last_whizby = now;
                sfx.play("whizby", Some(closest));
            }
        }
    }
}

fn hit_marker(mut sfx: ResMut<Sfx>, mut hits: MessageReader<HitConfirmed>, local: Query<(), With<crate::splitscreen::LocalSlot>>) {
    for h in hits.read() {
        if local.contains(h.shooter) {
            sfx.play("mp_hit_alert", None);
        }
    }
}

/// CoD4 steps when the bob cycle passes a quarter or three quarters.
fn footsteps(
    mut sfx: ResMut<Sfx>,
    pawns: Query<(Entity, &Mover, &Transform, Has<LocalPlayer>, Option<&crate::loadout::Loadout>), Without<Dead>>,
    spatial: SpatialQuery,
    surfaces: Query<&Surfaces>,
    mut last: Local<HashMap<Entity, f32>>,
) {
    let half = |c: f32| ((c + 0.25) * 2.0).floor() as i32 % 2;
    let mut seen = HashSet::new();
    for (e, m, tf, local, loadout) in &pawns {
        seen.insert(e);
        let prev = last.insert(e, m.bob_cycle).unwrap_or(m.bob_cycle);
        if !m.on_ground || half(prev) == half(m.bob_cycle) || m.horizontal_speed() < u(20.0) {
            continue;
        }
        // Dead Silence (`specialty_quieter`): no footsteps for anyone else.
        if !local && crate::perks::has(loadout, "specialty_quieter") {
            continue;
        }
        let kind = if m.sprinting {
            "step_sprint"
        } else if m.stance == Stance::Prone {
            "step_prone"
        } else if m.stance == Stance::Crouch {
            "qstep_run"
        } else if m.horizontal_speed() < u(110.0) {
            "step_walk"
        } else {
            "step_run"
        };
        let surface = ground_surface(&spatial, &surfaces, tf.translation);
        if local {
            sfx.play(format!("{kind}_plr_{surface}"), None);
        } else {
            sfx.play(format!("{kind}_{surface}"), Some(tf.translation));
        }
    }
    last.retain(|e, _| seen.contains(e));
}

fn landings(
    mut sfx: ResMut<Sfx>,
    mut landed: MessageReader<Landed>,
    pawns: Query<(&Transform, Has<LocalPlayer>)>,
    spatial: SpatialQuery,
    surfaces: Query<&Surfaces>,
) {
    for l in landed.read() {
        let Ok((tf, local)) = pawns.get(l.entity) else { continue };
        let kind = if l.fall_height < 40.0 { "qland" } else { "land" };
        let surface = ground_surface(&spatial, &surfaces, tf.translation);
        if local {
            sfx.play(format!("{kind}_plr_{surface}"), None);
        } else {
            sfx.play(format!("{kind}_{surface}"), Some(tf.translation));
        }
    }
}

/// Others cry out when hurt (the player hears their own breathing instead);
/// everyone has a death cry and a body fall.
#[allow(clippy::too_many_arguments)]
fn pain_and_death(
    time: Res<Time>,
    mut sfx: ResMut<Sfx>,
    sides: Option<Res<Sides>>,
    mut killed: MessageReader<Killed>,
    pawns: Query<(Entity, &Pawn, &Health, &Transform, Has<LocalPlayer>, Has<Dead>)>,
    mut last_health: Local<HashMap<Entity, f32>>,
    mut last_pain: Local<HashMap<Entity, f32>>,
) {
    let Some(sides) = sides else { return };
    let now = time.elapsed_secs();
    let voice = |tf: &Transform| tf.translation + Vec3::Y * u(VOICE_HEIGHT);
    for (e, p, h, tf, local, dead) in &pawns {
        let before = last_health.insert(e, h.current).unwrap_or(h.current);
        let hurt = h.current < before - 1.0 && h.current > 0.0 && !dead;
        if hurt && !local && now - last_pain.get(&e).copied().unwrap_or(-10.0) > 0.6 {
            last_pain.insert(e, now);
            let nationality = sides.of(p.team).nationality;
            sfx.play(format!("generic_pain_{nationality}_{}", rand::random_range(1..=8)), Some(voice(tf)));
        }
    }
    for k in killed.read() {
        let Ok((_, p, _, tf, _, _)) = pawns.get(k.victim) else { continue };
        let nationality = sides.of(p.team).nationality;
        sfx.play(format!("generic_death_{nationality}_{}", rand::random_range(1..=8)), Some(voice(tf)));
        sfx.play_later("bodyfall_flesh_large", Some(tf.translation), now, 0.8);
    }
}

/// Hurt breathing while badly wounded, a sigh once healed, and a gasp when
/// sprinting runs out.
fn breathing(
    time: Res<Time>,
    mut sfx: ResMut<Sfx>,
    player: Query<(&Health, &Mover, Has<Dead>), With<LocalPlayer>>,
    mut hurt: Local<bool>,
    mut next: Local<f32>,
    mut was_sprinting: Local<bool>,
) {
    let Ok((h, m, dead)) = player.single() else { return };
    let now = time.elapsed_secs();
    if !dead && h.current < crate::combat::max_health() * 0.5 {
        if now >= *next {
            sfx.play("breathing_hurt", None);
            *next = now + 1.8;
        }
        *hurt = true;
    } else if *hurt && (dead || h.current >= crate::combat::max_health()) {
        if !dead {
            sfx.play("breathing_better", None);
        }
        *hurt = false;
    }
    if *was_sprinting && !m.sprinting && m.sprint_left < 0.2 && !dead {
        sfx.play("sprint_gasp", None);
    }
    *was_sprinting = m.sprinting;
}

/// The viewmodel animations' notetracks name their sounds
/// (`weap_ak47_clipout_plr` at 23% of the reload). World at War's guns'
/// notes name World at War's sounds (see [`crate::waw::sound_alias`]).
fn viewmodel_notes(
    mut sfx: ResMut<Sfx>,
    bank: Option<Res<Bank>>,
    vm: Query<(&AnimPlayer, &crate::viewmodel::ViewModelSlot), With<ViewModelRoot>>,
    weapon: Query<&WeaponState, With<LocalPlayer>>,
    mut last: Local<Option<(usize, f32)>>,
) {
    // Player 1's gun: theirs is the sound heard.
    let (Some(bank), Some((player, _))) = (bank, vm.iter().find(|v| v.1.0 == 0)) else { return };
    let waw = weapon.single().is_ok_and(|w| crate::waw::is_waw(&w.def.name));
    let Some(anim) = &player.anim else { return };
    let key = Arc::as_ptr(anim) as usize;
    let duration = anim.duration().max(1e-3);
    // A new (or restarted) animation plays its notes from the start.
    let from = match *last {
        Some((k, t)) if k == key && t <= player.time => t,
        _ => -1.0,
    };
    for n in &anim.notifies {
        let at = n.time * duration;
        // Black Ops marks its sound notes `sndnt#<alias>`.
        let name = n.name.strip_prefix("sndnt#").unwrap_or(&n.name);
        let name: std::borrow::Cow<str> = if waw { crate::waw::sound_alias(name).into() } else { name.into() };
        if at > from && at <= player.time && bank.has(&name) {
            sfx.play(name.into_owned(), None);
        }
    }
    *last = Some((key, player.time));
}

/// Other players' reloads (the player's own come from the notetracks).
fn other_reloads(
    mut sfx: ResMut<Sfx>,
    pawns: Query<(Entity, &WeaponState, &Transform), (Without<LocalPlayer>, Without<Dead>)>,
    mut reloading: Local<HashSet<Entity>>,
) {
    for (e, w, tf) in &pawns {
        if !w.reloading() {
            reloading.remove(&e);
        } else if reloading.insert(e) && !w.def.sounds.reload.is_empty() {
            sfx.play(w.def.sounds.reload.clone(), Some(tf.translation + Vec3::Y * u(48.0)));
        }
    }
}

/// Pulling the trigger on an empty weapon.
fn dry_fire(
    mut sfx: ResMut<Sfx>,
    player: Query<(&WeaponState, &WeaponInput), (With<LocalPlayer>, Without<Dead>)>,
    mut held: Local<bool>,
) {
    let Ok((w, input)) = player.single() else { return };
    if input.fire && !*held && w.clip == 0 && w.reserve == 0 && !w.def.sounds.empty_player.is_empty() {
        sfx.play(w.def.sounds.empty_player.clone(), None);
    }
    *held = input.fire;
}

/// Putting a weapon away and raising the next.
fn weapon_switch(
    mut sfx: ResMut<Sfx>,
    player: Query<(&Loadout, &WeaponState), With<LocalPlayer>>,
    mut phase: Local<Option<bool>>,
) {
    let Ok((l, w)) = player.single() else { return };
    let now = l.switching.map(|s| s.raising);
    if now != *phase {
        let alias = match now {
            Some(false) => &w.def.sounds.putaway_player,
            Some(true) => &w.def.sounds.raise_player,
            None => &String::new(),
        };
        if !alias.is_empty() {
            sfx.play(alias.clone(), None);
        }
        *phase = now;
    }
}

//! Modern Warfare 2's sentry gun (`_autosentry.gsc`), the 6 kill streak.
//! Called in, it's carried: a ghost of it stands on the ground in front of
//! its owner, green where it can go and red where it can't (on a steep
//! slope, in a wall, out of sight), the gun put away meanwhile. Fire, Use or
//! its key again plants it there (`sentry_setPlaced`); dying while carrying
//! it keeps it held for later. Planted, it watches 55° either side of the
//! way it faces for its owner's enemies within 1800 units, turns its gun to
//! the nearest it can see, beeps three times and fires bursts of 20 to 120
//! rounds (`sentry_minigun_mp`: 10 a round to players, 20 rounds a second)
//! with short pauses, overheating after 8 s of fire. It's solid, takes 1000
//! damage from its owner's enemies (bullets and blasts) before it blows up,
//! and goes after 90 s (`SENTRY_TIME_OUT`).

use super::models::StreakModels;
use super::{Hardpoint, Killstreak, PAD_PICK};
use crate::collision::{self, Layer};
use crate::combat::{Damage, Dead, HitLocation, Hitbox, Pawn, Team, hostile};
use crate::explosives::Exploded;
use crate::fx::{Anchor, BulletHit, BulletImpact, Effects, Frame, FxLayer};
use crate::movement::{Mover, ViewAngles};
use crate::units::{self, INCH, u};
use crate::weapons::HitConfirmed;
use avian3d::prelude::*;
use bevy::prelude::*;
use rand::Rng;

/// The sentry's weapon, as kills name it.
pub const WEAPON: &str = "sentry_minigun_mp";

/// `SENTRY_TIME_OUT`, and `self.health`.
const TIME_OUT: f32 = 90.0;
const HEALTH: f32 = 1000.0;
/// `sentry_minigun_mp`: `iFireTime`, `playerDamage`, `maxRange`, the arcs
/// (`leftArc`/`rightArc`, `topArc`, `bottomArc`) and `aiSpread` (degrees).
const FIRE_TIME: f32 = 0.05;
const DAMAGE: f32 = 10.0;
const RANGE: f32 = 1800.0;
const SIDE_ARC: f32 = 55.0;
const TOP_ARC: f32 = 45.0;
const BOTTOM_ARC: f32 = 20.0;
const SPREAD: f32 = 0.6;
/// How fast the gun turns (degrees a second; the turret's `maxTurnSpeed`).
const YAW_SPEED: f32 = 200.0;
const PITCH_SPEED: f32 = 60.0;
/// Fires once aimed this near (degrees).
const AIMED: f32 = 6.0;
/// `level.sentrySettings`: bursts, pauses between (s).
const BURST: (u32, u32) = (20, 120);
const PAUSE: (f32, f32) = (0.15, 0.35);
/// `sentry_targetLockSound` before the first round, and
/// `SENTRY_OVERHEAT_TIME` of firing before it has to cool.
const LOCK_TIME: f32 = 0.3;
const OVERHEAT: f32 = 8.0;
/// Looks for targets this often (s).
const SCAN: f32 = 0.1;
/// The gun's pivot above the feet, and the muzzle in front of it (CoD
/// units, the model's `tag_aim` and `tag_flash`).
const PIVOT: f32 = 48.5;
const MUZZLE: f32 = 37.0;
/// Planted this far in front of its carrier (CoD units), on ground no
/// steeper than this (the normal's up part), with this much room.
const CARRY_AHEAD: f32 = 40.0;
const FLAT: f32 = 0.82;
const ROOM_RADIUS: f32 = 14.0;
const ROOM_HEIGHT: f32 = 52.0;
/// Its body for bullets and blasts (a capsule, CoD units), and for walking
/// into.
const BODY: (f32, f32, f32) = (8.0, 50.0, 18.0);
/// The wreck lies smoking this long.
const WRECK_TIME: f32 = 6.0;
/// `sentry_beepSounds`.
const BEEP_EVERY: f32 = 3.0;

pub(super) fn build(app: &mut App) {
    app.init_resource::<Calls>()
        .add_systems(OnEnter(crate::state::GameState::InGame), clear)
        .add_systems(
            Update,
            (start_carrying, carry, take_damage, think, wrecks).chain().after(super::use_hardpoints).run_if(crate::state::in_game),
        )
        .add_systems(Update, hold_gun.after(crate::player::InputSet).before(crate::weapons::WeaponSet).run_if(crate::state::in_game));
}

/// Pawns who've called in a sentry and are to carry it out.
#[derive(Resource, Default)]
pub struct Calls(pub Vec<Entity>);

/// A pawn carrying its sentry out: the ghost, where it would go, whether
/// it can.
#[derive(Component)]
pub struct Carrying {
    ghost: Entity,
    good: Option<Entity>,
    bad: Option<Entity>,
    spot: Transform,
    valid: bool,
    since: f32,
}

/// Plant the carried sentry now if it can go (the streak test's press).
#[derive(Component)]
pub(super) struct PlaceNow;

/// A planted sentry gun.
#[derive(Component)]
pub struct Sentry {
    pub owner: Entity,
    pub team: Team,
    damage_taken: f32,
    until: f32,
    /// The way it was planted facing (CoD yaw, radians) and where the gun
    /// points from there (radians: yaw left of it, pitch up).
    base_yaw: f32,
    yaw: f32,
    pitch: f32,
    target: Option<Entity>,
    next_scan: f32,
    /// Since when it has had its target (the lock beeps first).
    locked: Option<f32>,
    burst: u32,
    next_shot: f32,
    heat: f32,
    overheated: bool,
    next_beep: f32,
    /// The model's holder, and its `tag_aim` (the gun) and that joint's
    /// bind pose, when the model has one.
    body: Entity,
    aim: Option<(Entity, Transform)>,
    /// Its firing loop's carrier, while a burst lasts.
    loop_sound: Option<Entity>,
}

/// A destroyed sentry, smoking until then.
#[derive(Component)]
pub struct Wreck {
    until: f32,
    next_puff: f32,
}

impl Wreck {
    fn new(now: f32) -> Wreck {
        Wreck { until: now + WRECK_TIME, next_puff: now + 1.5 }
    }
}

impl Sentry {
    /// Shooting now (for the HUD's compass).
    pub fn firing(&self) -> bool {
        self.loop_sound.is_some()
    }
}

fn clear(mut calls: ResMut<Calls>) {
    calls.0.clear();
}

/// The model's names: MW2's, then CoD4's mounted SAW (on some maps), then
/// the SAW itself.
const MODEL: [&str; 3] = ["sentry_minigun", "weapon_saw_mg_setup", "weapon_saw"];
const WRECK: [&str; 1] = ["sentry_minigun_destroyed"];

/// Hand each caller the ghost to carry (or, dead by now, the sentry back).
fn start_carrying(
    mut commands: Commands,
    time: Res<Time>,
    mut calls: ResMut<Calls>,
    mut pawns: Query<(Has<Dead>, Has<Carrying>, &mut Killstreak, &Transform)>,
    mut models: StreakModels,
) {
    let now = time.elapsed_secs();
    for e in std::mem::take(&mut calls.0) {
        let Ok((dead, carrying, mut streak, tf)) = pawns.get_mut(e) else { continue };
        if dead || carrying {
            streak.held.add(Hardpoint::Sentry);
            continue;
        }
        let ghost = commands.spawn((Name::new("sentry ghost"), Transform::from_translation(tf.translation), Visibility::default())).id();
        // The sentry itself, see-through green where it can go and red
        // where it can't (MW2's `_obj` models are that glow, which isn't
        // drawn here).
        let part = |commands: &mut Commands, models: &mut StreakModels, color: Color, shown: bool| {
            let holder = commands
                .spawn((Transform::default(), if shown { Visibility::Inherited } else { Visibility::Hidden }, ChildOf(ghost)))
                .id();
            let m = models.spawn(commands, &MODEL, holder, false)?;
            let material = models.ghost_material(color);
            for s in m.surfaces {
                commands.entity(s).insert((MeshMaterial3d(material.clone()), bevy::light::NotShadowCaster));
            }
            Some(holder)
        };
        let good = part(&mut commands, &mut models, Color::srgba(0.3, 1.0, 0.35, 0.45), true);
        let bad = part(&mut commands, &mut models, Color::srgba(1.0, 0.25, 0.2, 0.45), false);
        commands.entity(e).insert(Carrying { ghost, good, bad, spot: Transform::from_translation(tf.translation), valid: false, since: now });
    }
}

/// Where a sentry carried by someone at `feet` facing `forward` would
/// go, and whether it can: on the ground there, not too steep, with room
/// for it, and in the carrier's sight.
fn placement(spatial: &SpatialQuery, feet: Vec3, eye: Vec3, forward: Vec3) -> (Transform, bool) {
    let flat = forward.with_y(0.0).normalize_or(Vec3::NEG_Z);
    let yaw = (-flat.z).atan2(flat.x);
    let ahead = feet + flat * u(CARRY_AHEAD);
    let top = ahead + Vec3::Y * u(30.0);
    let filter = collision::movement_filter();
    let ground = spatial.cast_ray(top, Dir3::NEG_Y, u(80.0), true, &filter);
    let at = ground.map_or(ahead, |h| top - Vec3::Y * h.distance);
    let spot = Transform::from_translation(at).with_rotation(Quat::from_rotation_y(yaw));
    let Some(ground) = ground.filter(|h| h.distance > 0.0) else { return (spot, false) };
    if ground.normal.y < FLAT {
        return (spot, false);
    }
    // Room: nothing solid where it would stand (a little off the ground).
    let shape = Collider::cylinder(u(ROOM_RADIUS), u(ROOM_HEIGHT - 6.0));
    let middle = at + Vec3::Y * u(ROOM_HEIGHT * 0.5 + 3.0);
    if !spatial.shape_intersections(&shape, middle, Quat::IDENTITY, &filter).is_empty() {
        return (spot, false);
    }
    // Seen from the eye (not round a corner or through a thin wall).
    let look = middle - eye;
    let seen = Dir3::new(look).ok().is_none_or(|d| spatial.cast_ray(eye, d, look.length(), true, &collision::sight_filter()).is_none());
    (spot, seen)
}

/// Carry the ghost about, and plant it: a player with Fire, Use or the
/// sentry's key (a pad's picker button), a bot as soon as it can (trying
/// round itself).
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn carry(
    mut commands: Commands,
    time: Res<Time>,
    spatial: SpatialQuery,
    mut carriers: Query<(
        Entity,
        &Pawn,
        &Transform,
        &Mover,
        &ViewAngles,
        &mut Carrying,
        &mut Killstreak,
        Has<Dead>,
        Option<&crate::splitscreen::PlayerInput>,
        Option<&crate::splitscreen::LocalSlot>,
        Has<PlaceNow>,
    )>,
    mut ghosts: Query<(&mut Transform, &mut Visibility), Without<Pawn>>,
    mut models: StreakModels,
    mut sfx: ResMut<crate::audio::Sfx>,
) {
    let now = time.elapsed_secs();
    for (e, pawn, tf, mover, view, mut c, mut streak, dead, player, slot, place_now) in &mut carriers {
        let slot = slot.map(|s| s.0);
        // Dying with it: it's kept for later.
        if dead {
            streak.held.add(Hardpoint::Sentry);
            commands.entity(c.ghost).despawn();
            commands.entity(e).remove::<Carrying>();
            if let Some(s) = slot {
                super::PROMPTS.set(s, None);
            }
            continue;
        }
        let eye = mover.eye(tf.translation);
        let forward = view.forward();
        let (spot, valid) = if player.is_some() {
            placement(&spatial, tf.translation, eye, forward)
        } else {
            // A bot tries the way it looks, then round itself.
            (0..8)
                .map(|k| Quat::from_rotation_y(k as f32 * std::f32::consts::FRAC_PI_4) * forward)
                .map(|f| placement(&spatial, tf.translation, eye, f))
                .find(|(_, ok)| *ok)
                .unwrap_or_else(|| placement(&spatial, tf.translation, eye, forward))
        };
        c.spot = spot;
        c.valid = valid && mover.on_ground;
        if let Ok((mut g, _)) = ghosts.get_mut(c.ghost) {
            *g = spot;
        }
        // Green or red.
        if let (Some(good), Some(bad)) = (c.good, c.bad) {
            for (part, shown) in [(good, c.valid), (bad, !c.valid)] {
                if let Ok((_, mut v)) = ghosts.get_mut(part) {
                    v.set_if_neq(if shown { Visibility::Inherited } else { Visibility::Hidden });
                }
            }
        }
        let place = place_now || match player {
            Some(p) => {
                let keys = &p.keys;
                p.live
                    && now - c.since > 0.25
                    && (p.mouse.just_pressed(MouseButton::Left)
                        || keys.just_pressed(KeyCode::KeyF)
                        || p.pad.interact
                        || keys.just_pressed(Hardpoint::Sentry.key())
                        || keys.just_pressed(PAD_PICK))
            }
            // Bots: a moment after calling it in, or wherever after a while.
            None => now - c.since > 0.6 && (c.valid || now - c.since > 6.0),
        };
        if let Some(s) = slot {
            let text = if c.valid {
                let key = player.map_or_else(|| "F".to_owned(), |p| p.use_key());
                format!("Press [{}] or [{key}] to place the Sentry Gun", crate::bindings::key_name(crate::bindings::Action::Fire))
            } else {
                "Can't place the Sentry Gun here".to_owned()
            };
            super::PROMPTS.set(s, Some((text, None)));
        }
        if !place || !(c.valid || player.is_none()) {
            continue;
        }
        commands.entity(c.ghost).despawn();
        commands.entity(e).remove::<(Carrying, PlaceNow)>();
        if let Some(s) = slot {
            super::PROMPTS.set(s, None);
        }
        plant(&mut commands, &mut models, &mut sfx, e, pawn.team, spot, now);
    }
}

/// `sentry_setPlaced`: the sentry where it was carried to.
fn plant(commands: &mut Commands, models: &mut StreakModels, sfx: &mut crate::audio::Sfx, owner: Entity, team: Team, spot: Transform, now: f32) {
    let base_yaw = {
        let f = spot.rotation * Vec3::X;
        (-f.z).atan2(f.x)
    };
    let sentry = commands.spawn((Name::new("sentry gun"), spot, Visibility::default())).id();
    let body = commands.spawn((Transform::default(), Visibility::default(), ChildOf(sentry))).id();
    let model = models.spawn(commands, &MODEL, body, true);
    let aim = model.as_ref().and_then(|m| {
        let e = m.skeleton.joint("tag_aim")?;
        m.skeleton.joints.iter().find(|j| j.entity == e).map(|j| (e, j.bind))
    });
    info!("sentry: planted ({}) at CoD {:?}", model.as_ref().map_or("no model", |m| m.name.as_str()), units::to_cod(spot.translation).map(f32::round));
    let (a, b, r) = BODY;
    commands.spawn((
        Hitbox { owner: sentry, location: HitLocation::Torso },
        Collider::capsule_endpoints(u(r), units::pos([0.0, 0.0, a]), units::pos([0.0, 0.0, b])),
        CollisionLayers::new(Layer::Hitbox, LayerMask::NONE),
        Transform::default(),
        ChildOf(sentry),
    ));
    // Solid to walk into (not to bullets, which its hitbox takes).
    commands.spawn((
        RigidBody::Static,
        Collider::cylinder(u(ROOM_RADIUS), u(b)),
        CollisionLayers::new(Layer::PlayerClip, LayerMask::NONE),
        Transform::from_translation(Vec3::Y * u(b * 0.5)),
        ChildOf(sentry),
    ));
    commands.entity(sentry).insert(Sentry {
        owner,
        team,
        damage_taken: 0.0,
        until: now + TIME_OUT,
        base_yaw,
        yaw: 0.0,
        pitch: 0.0,
        target: None,
        next_scan: now,
        locked: None,
        burst: 0,
        next_shot: now,
        heat: 0.0,
        overheated: false,
        next_beep: now + BEEP_EVERY,
        body,
        aim,
        loop_sound: None,
    });
    sfx.play("iw4/sentry_gun_plant", Some(spot.translation));
}

/// No firing or aiming while carrying a sentry: the hands carry it.
fn hold_gun(mut pawns: Query<&mut crate::weapons::WeaponInput, With<Carrying>>) {
    for mut input in &mut pawns {
        input.fire = false;
        input.ads = false;
        input.reload = false;
    }
}

/// `sentry_handleDamage`: its owner's enemies hurt it, bullets and blasts
/// alike (a blast by how near it goes off); past its health it blows up.
#[allow(clippy::too_many_arguments)]
fn take_damage(
    mut commands: Commands,
    time: Res<Time>,
    mut damage: MessageReader<Damage>,
    mut blasts: MessageReader<Exploded>,
    mut sentries: Query<(Entity, &mut Sentry, &Transform)>,
    pawns: Query<&Pawn>,
    mut hits: MessageWriter<HitConfirmed>,
    mut models: StreakModels,
    mut fx: Option<ResMut<Effects>>,
    mut sfx: ResMut<crate::audio::Sfx>,
    sides: Option<Res<crate::audio::Sides>>,
    locals: Query<(), With<crate::player::LocalPlayer>>,
) {
    let now = time.elapsed_secs();
    let mut hurt: Vec<(Entity, Entity, f32)> = damage.read().map(|d| (d.target, d.attacker.unwrap_or(Entity::PLACEHOLDER), d.amount)).collect();
    for b in blasts.read() {
        for (e, _, tf) in &sentries {
            let dist = (b.at.distance(tf.translation + Vec3::Y * u(30.0)) / INCH - BODY.2).max(0.0);
            if dist < b.radius && b.inner + b.outer > 0.0 {
                hurt.push((e, b.owner, b.inner + (b.outer - b.inner) * dist / b.radius.max(1.0)));
            }
        }
    }
    let mut destroyed = Vec::new();
    for (target, attacker, amount) in hurt {
        let Ok((e, mut s, _)) = sentries.get_mut(target) else { continue };
        if s.damage_taken >= HEALTH || attacker == s.owner {
            continue;
        }
        let Ok(by) = pawns.get(attacker) else { continue };
        let enemy = match pawns.get(s.owner) {
            Ok(owner) => hostile(owner, by),
            Err(_) => by.team != s.team,
        };
        if !enemy {
            continue;
        }
        hits.write(HitConfirmed { shooter: attacker, headshot: false });
        s.damage_taken += amount;
        if s.damage_taken >= HEALTH {
            info!("sentry: destroyed by {}", by.name);
            destroyed.push(e);
        }
    }
    // Run out of time too.
    for (e, s, _) in &sentries {
        if now >= s.until && !destroyed.contains(&e) {
            info!("sentry: timed out");
            destroyed.push(e);
        }
    }
    for e in destroyed {
        let Ok((_, s, tf)) = sentries.get(e) else { continue };
        if let Some(l) = s.loop_sound {
            commands.entity(l).despawn();
        }
        // `sentry_handleDeath`: the wreck, a blast and smoke.
        let wreck = commands.spawn((Name::new("sentry wreck"), *tf, Visibility::default(), Wreck::new(now))).id();
        if models.spawn(&mut commands, &WRECK, wreck, true).is_none() {
            // (No wreck model: the gun itself, tipped over.)
            models.spawn(&mut commands, &MODEL, wreck, true);
            commands.entity(wreck).insert(tf.with_rotation(tf.rotation * Quat::from_rotation_x(1.3)));
        }
        let at = tf.translation + Vec3::Y * u(PIVOT);
        if let Some(fx) = fx.as_deref_mut() {
            fx.play("explosions/small_vehicle_explosion", Anchor::Fixed(Frame::facing(at, Vec3::Y, 0.0)), FxLayer::World);
        }
        sfx.play("iw4/sentry_explode", Some(at));
        sfx.play_later("iw4/sentry_explode_smoke", Some(at), now, 1.5);
        // "Sentry destroyed" / "Sentry gone" to its owner.
        if let (Some(sides), true) = (&sides, locals.contains(s.owner)) {
            let line = if s.damage_taken >= HEALTH { "sentry_destroyed" } else { "sentry_gone" };
            sfx.play(format!("iw4/{}_1mc_{line}", sides.of(s.team).voice.to_ascii_lowercase()), None);
        }
        commands.entity(e).despawn();
    }
}

/// The wrecks smoke a while, then go.
fn wrecks(mut commands: Commands, time: Res<Time>, mut wrecks: Query<(Entity, &mut Wreck, &Transform)>, mut fx: Option<ResMut<Effects>>) {
    let now = time.elapsed_secs();
    for (e, mut w, tf) in &mut wrecks {
        if now >= w.until {
            commands.entity(e).despawn();
            continue;
        }
        if now >= w.next_puff {
            w.next_puff = now + 0.4;
            if let Some(fx) = fx.as_deref_mut() {
                fx.play("smoke/damaged_vehicle_smoke", Anchor::Fixed(Frame::facing(tf.translation + Vec3::Y * u(PIVOT), Vec3::Y, 0.0)), FxLayer::World);
            }
        }
    }
}

/// Turn towards a direction (Bevy), within its arcs: the yaw left of its
/// facing and the pitch up, radians.
fn aim_angles(base_yaw: f32, dir: Vec3) -> (f32, f32) {
    let yaw = (-dir.z).atan2(dir.x);
    let rel = (yaw - base_yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
    let pitch = dir.y.clamp(-1.0, 1.0).asin();
    (rel, pitch)
}

fn in_arcs(rel: f32, pitch: f32) -> bool {
    rel.abs() <= SIDE_ARC.to_radians() && pitch <= TOP_ARC.to_radians() && pitch >= -BOTTOM_ARC.to_radians()
}

/// `sentry_attackTargets`: pick the nearest enemy in its arcs and sight,
/// turn to them, beep, and fire bursts (overheating as it goes).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn think(
    mut commands: Commands,
    time: Res<Time>,
    spatial: SpatialQuery,
    mut sentries: Query<(Entity, &mut Sentry, &Transform)>,
    pawns: Query<(Entity, &Pawn, &Transform, &Mover, Has<Dead>)>,
    hitboxes: Query<&Hitbox>,
    mut joints: Query<&mut Transform, (Without<Sentry>, Without<Pawn>)>,
    parents: Query<&ChildOf>,
    globals: Query<&GlobalTransform>,
    mut damage: MessageWriter<Damage>,
    mut impacts: MessageWriter<BulletImpact>,
    mut fx: Option<ResMut<Effects>>,
    mut sfx: ResMut<crate::audio::Sfx>,
) {
    let (now, dt) = (time.elapsed_secs(), time.delta_secs());
    let mut rng = rand::rng();
    for (e, mut s, tf) in &mut sentries {
        let pivot = tf.translation + Vec3::Y * u(PIVOT);
        let owner = pawns.get(s.owner).ok().map(|p| p.1.clone());
        let (team, base_yaw) = (s.team, s.base_yaw);
        let enemy = |p: &Pawn| owner.as_ref().map_or(p.team != team, |o| hostile(o, p));
        let chest = |ptf: &Transform| ptf.translation + Vec3::Y * u(40.0);
        let visible = |to: Vec3| {
            let d = to - pivot;
            Dir3::new(d).ok().is_none_or(|dir| spatial.cast_ray(pivot, dir, d.length(), true, &collision::ai_sight_filter()).is_none())
        };
        let can_shoot = |ptf: &Transform| {
            let to = chest(ptf) - pivot;
            let (rel, pitch) = aim_angles(base_yaw, to.normalize_or_zero());
            to.length() <= u(RANGE) && in_arcs(rel, pitch) && visible(chest(ptf))
        };
        // Keep a target while it can be shot; look again every so often.
        let keep = s.target.and_then(|t| pawns.get(t).ok()).is_some_and(|(_, p, ptf, _, dead)| !dead && enemy(p) && can_shoot(ptf));
        if !keep {
            s.target = None;
            s.locked = None;
        }
        if now >= s.next_scan {
            s.next_scan = now + SCAN;
            let best = pawns
                .iter()
                .filter(|(_, p, ptf, _, dead)| !dead && enemy(p) && can_shoot(ptf))
                .min_by(|a, b| a.2.translation.distance_squared(pivot).total_cmp(&b.2.translation.distance_squared(pivot)))
                .map(|(t, ..)| t);
            if best.is_some() && best != s.target {
                if s.target.is_none() {
                    // `sentry_targetLockSound`.
                    for k in 0..3 {
                        sfx.play_later("iw4/sentry_gun_beep", Some(pivot), now, k as f32 * 0.1);
                    }
                    s.locked = Some(now);
                }
                s.target = best;
            }
        }
        // Turn: towards the target, else back to its facing.
        let want = match s.target.and_then(|t| pawns.get(t).ok()) {
            Some((_, _, ptf, mover, _)) => {
                let aim_at = chest(ptf).lerp(mover.eye(ptf.translation), 0.3);
                aim_angles(s.base_yaw, (aim_at - pivot).normalize_or_zero())
            }
            None => (0.0, 0.0),
        };
        let step = |from: f32, to: f32, speed: f32| from + (to - from).clamp(-speed.to_radians() * dt, speed.to_radians() * dt);
        s.yaw = step(s.yaw, want.0.clamp(-SIDE_ARC.to_radians(), SIDE_ARC.to_radians()), YAW_SPEED);
        s.pitch = step(s.pitch, want.1.clamp(-BOTTOM_ARC.to_radians(), TOP_ARC.to_radians()), PITCH_SPEED);
        let turn = Quat::from_rotation_y(s.base_yaw + s.yaw) * Quat::from_rotation_z(s.pitch);
        let dir = turn * Vec3::X;
        // The gun on the model: turned about its pivot, in the sentry's
        // own frame (its parent doesn't move).
        if let Some((joint, bind)) = s.aim {
            let parent = parents.get(joint).ok().and_then(|p| globals.get(p.parent()).ok());
            if let (Some(parent), Ok(mut jt)) = (parent, joints.get_mut(joint)) {
                let p = parent.rotation();
                let local_turn = Quat::from_rotation_y(s.yaw) * Quat::from_rotation_z(s.pitch);
                let world_turn = tf.rotation * local_turn * tf.rotation.inverse();
                jt.rotation = (p.inverse() * world_turn * p * bind.rotation).normalize();
            }
        } else if let Ok(mut b) = joints.get_mut(s.body) {
            // (A model without the joint turns whole.)
            b.rotation = Quat::from_rotation_y(s.yaw);
        }
        // Heat wears off when it's not firing.
        let firing_now = s.target.is_some()
            && !s.overheated
            && s.locked.is_some_and(|l| now - l >= LOCK_TIME)
            && (want.0 - s.yaw).abs() < AIMED.to_radians()
            && (want.1 - s.pitch).abs() < AIMED.to_radians() * 2.0;
        if s.overheated {
            s.heat = (s.heat - dt).max(0.0);
            if s.heat <= 0.0 {
                s.overheated = false;
            }
        }
        if !firing_now {
            if !s.overheated {
                s.heat = (s.heat - dt).max(0.0);
            }
            if let Some(l) = s.loop_sound.take() {
                commands.entity(l).despawn();
                sfx.play("iw4/sentry_minigun_cooldown", Some(pivot));
            }
            s.burst = 0;
            if now >= s.next_beep {
                s.next_beep = now + BEEP_EVERY;
                sfx.play("iw4/sentry_gun_beep", Some(pivot));
            }
            continue;
        }
        if s.loop_sound.is_none() {
            let carrier = commands.spawn((Transform::from_translation(Vec3::Y * u(PIVOT)), Visibility::default(), ChildOf(e))).id();
            sfx.play_on("iw4/sentry_minigun_fire", carrier);
            s.loop_sound = Some(carrier);
        }
        // Rounds due this frame (20 a second), in bursts.
        let muzzle = pivot + dir * u(MUZZLE);
        let mut shots = 0;
        while s.next_shot <= now && shots < 4 {
            shots += 1;
            if s.burst == 0 {
                s.burst = rng.random_range(BURST.0..=BURST.1);
            }
            s.burst -= 1;
            s.next_shot = if s.burst == 0 { now + rng.random_range(PAUSE.0..PAUSE.1) } else { s.next_shot.max(now - FIRE_TIME) + FIRE_TIME };
            s.heat += FIRE_TIME;
            shoot(e, s.owner, muzzle, dir, &spatial, &hitboxes, &pawns, &mut damage, &mut impacts, &mut rng);
        }
        if shots > 0 {
            if let Some(fx) = fx.as_deref_mut() {
                fx.play("muzzleflashes/saw_flash_wv", Anchor::Fixed(Frame::facing(muzzle, dir, 0.0)), FxLayer::World);
            }
        }
        if s.heat > OVERHEAT {
            s.overheated = true;
        }
    }
}

/// One round from the muzzle along the gun, a little spread.
#[allow(clippy::too_many_arguments)]
fn shoot(
    sentry: Entity,
    owner: Entity,
    from: Vec3,
    aim: Vec3,
    spatial: &SpatialQuery,
    hitboxes: &Query<&Hitbox>,
    pawns: &Query<(Entity, &Pawn, &Transform, &Mover, Has<Dead>)>,
    damage: &mut MessageWriter<Damage>,
    impacts: &mut MessageWriter<BulletImpact>,
    rng: &mut impl Rng,
) {
    let r = SPREAD.to_radians() * rng.random::<f32>().sqrt();
    let theta = rng.random_range(0.0..std::f32::consts::TAU);
    let side = aim.any_orthonormal_vector();
    let dir = (Quat::from_axis_angle(aim, theta) * Quat::from_axis_angle(side, r) * aim).normalize();
    let not_self = |e: Entity| hitboxes.get(e).map_or(true, |hb| hb.owner != sentry);
    let Ok(d) = Dir3::new(dir) else { return };
    let Some(hit) = spatial.cast_ray_predicate(from, d, u(RANGE * 1.5), true, &collision::bullet_filter(), &not_self) else { return };
    let to = from + dir * hit.distance;
    let what = match hitboxes.get(hit.entity) {
        Ok(hb) if pawns.get(hb.owner).is_ok() => {
            if hb.owner != owner {
                damage.write(Damage { target: hb.owner, attacker: Some(owner), amount: DAMAGE, location: hb.location, weapon: WEAPON });
            }
            BulletHit::Flesh { head: hb.location == HitLocation::Head }
        }
        Ok(hb) => {
            // Another sentry, a helicopter.
            damage.write(Damage { target: hb.owner, attacker: Some(owner), amount: DAMAGE * 2.0, location: hb.location, weapon: WEAPON });
            BulletHit::Metal
        }
        Err(_) => BulletHit::World,
    };
    impacts.write(BulletImpact { from, to, normal: hit.normal, impact_type: 2, hit: what });
}

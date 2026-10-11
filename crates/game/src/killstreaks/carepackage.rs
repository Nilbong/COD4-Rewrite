//! Modern Warfare 2's care package (`_airdrop.gsc`), the 4 kill streak.
//! Called in, its owner throws a marker (a smoke canister, thrown as the
//! smoke grenade is: [`crate::grenades::How::Marker`]). Once it lies
//! smoking, a Little Bird flies in over it, hovers, lets a crate go and
//! flies off; the crate falls, bounces and settles as clutter does
//! ([`crate::clutter`]), then stands solid for 90 s. Holding Use on it
//! opens it: its owner in half a second, a teammate in two, an enemy in
//! three (`crateUseTime`, and others' longer capture). Inside is a kill
//! streak (`getRandomCrateType`, weighted: UAV and the sentry gun often,
//! the helicopter rarely; never another care package) or ammo, decided when
//! it drops and named on its prompt.

use super::models::StreakModels;
use super::{Hardpoint, Killstreak, StreakNotice};
use crate::clutter::{Body, Moving};
use crate::collision::Layer;
use crate::combat::{Dead, Pawn, Team, hostile};
use crate::grenades::{How, LiveGrenade, Offhand};
use crate::movement::{Mover, ViewAngles};
use crate::units::{self, u};
use avian3d::prelude::*;
use bevy::prelude::*;
use rand::Rng;
use std::collections::HashSet;
use std::sync::Mutex;

/// The Little Bird: in from this far away (CoD units), this far above the
/// marker, at this speed (u/s); it hovers this long and drops the crate
/// this far into it.
const APPROACH: f32 = 7000.0;
const HEIGHT: f32 = 800.0;
const SPEED: f32 = 1400.0;
const HOVER: f32 = 1.6;
const DROP_AT: f32 = 0.6;
/// The crate hangs this far under it.
const SLUNG: f32 = 90.0;
/// A landed crate stays this long (s).
const CRATE_TIME: f32 = 90.0;
/// Opening it: its owner, a teammate, an enemy (s).
const USE_OWNER: f32 = 0.5;
const USE_TEAM: f32 = 2.0;
const USE_ENEMY: f32 = 3.0;
/// Within reach of it (CoD units from its middle, as `Player_GetUseList`).
const REACH: f32 = 100.0;
/// Bots go for their own crate, and after this long anyone's near them
/// (CoD units).
const BOT_OWN: f32 = 4000.0;
const BOT_ANY: (f32, f32) = (8.0, 1200.0);
/// Its mass for the fall (clutter's units: CoD's `PhysPreset` mass).
const MASS: f32 = 60.0;
/// A marker still rolling this long after its fuse calls the drop anyway.
const MARKER_ROLL: f32 = 1.5;
/// Waiting this long for a marker to be thrown before giving it back.
const THROW_WAIT: f32 = 10.0;

pub(super) fn build(app: &mut App) {
    app.init_resource::<Calls>()
        .add_systems(OnEnter(crate::state::GameState::InGame), clear)
        .add_systems(Update, (throw_markers, watch_markers, fly, land, open).chain().after(super::use_hardpoints).run_if(crate::state::in_game));
}

/// Pawns who've called in a care package and are to throw its marker.
#[derive(Resource, Default)]
pub struct Calls(pub Vec<Entity>);

/// A bot wants to open the crate it's at.
#[derive(Component, Default, Clone, Copy, Debug)]
pub struct UseCrate(pub bool);

/// What's in a crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Contents {
    Ammo,
    Streak(Hardpoint),
}

impl Contents {
    pub fn name(self) -> &'static str {
        match self {
            Contents::Ammo => "Ammo",
            Contents::Streak(h) => h.name(),
        }
    }

    /// `getRandomCrateType`: by weight.
    fn roll(rng: &mut impl Rng, helicopter: bool) -> Contents {
        let table = [
            (Contents::Streak(Hardpoint::Uav), 30),
            (Contents::Ammo, 20),
            (Contents::Streak(Hardpoint::Airstrike), 20),
            (Contents::Streak(Hardpoint::Sentry), 22),
            (Contents::Streak(Hardpoint::Helicopter), if helicopter { 8 } else { 0 }),
        ];
        let total: u32 = table.iter().map(|t| t.1).sum();
        let mut pick = rng.random_range(0..total);
        for (c, w) in table {
            if pick < w {
                return c;
            }
            pick -= w;
        }
        Contents::Ammo
    }
}

/// The Little Bird bringing a crate.
#[derive(Component)]
pub struct Courier {
    from: Vec3,
    over: Vec3,
    away: Vec3,
    start: f32,
    /// When it gets there, and leaves.
    arrive: f32,
    leave: f32,
    dropped: bool,
    /// The crate, slung under it until it's let go.
    load: Option<Entity>,
}

/// A care package's crate.
#[derive(Component)]
pub struct CarePackage {
    pub owner: Entity,
    pub team: Team,
    pub contents: Contents,
    landed: Option<f32>,
    /// Its box (the model's space, metres: middle and half size).
    centre: Vec3,
    half: Vec3,
    /// Who's opening it, since when.
    opening: Option<(Entity, f32)>,
}

impl CarePackage {
    /// Down and openable.
    pub fn landed(&self) -> bool {
        self.landed.is_some()
    }
}

/// Bots' crates to go and open: the bot, where.
static BOT_GOALS: Mutex<Vec<(Entity, Vec3)>> = Mutex::new(Vec::new());

/// The crate `bot` should go and open, if any.
pub fn bot_goal(bot: Entity) -> Option<Vec3> {
    BOT_GOALS.lock().ok()?.iter().find(|g| g.0 == bot).map(|g| g.1)
}

fn clear(mut calls: ResMut<Calls>) {
    calls.0.clear();
    if let Ok(mut g) = BOT_GOALS.lock() {
        g.clear();
    }
}

/// Start each caller's marker throw once their hands are free; dead
/// callers (or none to throw it as, after a while) get it back.
#[allow(clippy::type_complexity)]
fn throw_markers(
    mut commands: Commands,
    time: Res<Time>,
    mut calls: ResMut<Calls>,
    defs: Res<crate::grenades::GrenadeDefs>,
    mut pawns: Query<(&crate::weapons::WeaponState, &mut Killstreak, Has<Dead>, Has<Offhand>, Has<super::sentry::Carrying>, Option<&crate::loadout::Loadout>)>,
    mut waiting: Local<Vec<(Entity, f32)>>,
) {
    let now = time.elapsed_secs();
    for e in std::mem::take(&mut calls.0) {
        waiting.push((e, now));
    }
    waiting.retain(|&(e, since)| {
        let Ok((weapon, mut streak, dead, busy, carrying, loadout)) = pawns.get_mut(e) else { return false };
        let switching = loadout.is_some_and(|l| l.switching.is_some());
        if dead || now - since > THROW_WAIT {
            streak.held.add(Hardpoint::CarePackage);
            return false;
        }
        if busy || carrying || switching {
            return true;
        }
        match crate::grenades::marker_throw(&defs, weapon, now) {
            Some(o) => {
                commands.entity(e).insert(o);
                false
            }
            None => true,
        }
    });
}

/// A marker smoking: send the Little Bird.
#[allow(clippy::too_many_arguments)]
fn watch_markers(
    mut commands: Commands,
    time: Res<Time>,
    markers: Query<(Entity, &LiveGrenade, &Transform)>,
    pawns: Query<&Pawn>,
    heli_paths: Res<super::helicopter::HeliPaths>,
    mut models: StreakModels,
    mut sfx: ResMut<crate::audio::Sfx>,
    mut sent: Local<HashSet<Entity>>,
) {
    let now = time.elapsed_secs();
    sent.retain(|e| markers.contains(*e));
    let mut rng = rand::rng();
    for (e, g, tf) in &markers {
        // Smoking, or still rolling a while after its fuse: the helicopter
        // comes for where it is.
        if g.how != How::Marker || !(g.popped() || now > g.explode_at + MARKER_ROLL) || sent.contains(&e) {
            continue;
        }
        sent.insert(e);
        let team = pawns.get(g.thrower).map_or(Team::Allies, |p| p.team);
        let target = tf.translation;
        let over = target + Vec3::Y * u(HEIGHT);
        let a = rng.random_range(0.0..std::f32::consts::TAU);
        let way = Vec3::new(a.cos(), 0.0, a.sin());
        let from = over - way * u(APPROACH) + Vec3::Y * u(300.0);
        let away = over + way * u(APPROACH) + Vec3::Y * u(600.0);
        let arrive = now + from.distance(over) / u(SPEED);
        let yaw = (-way.z).atan2(way.x);
        let heli = commands
            .spawn((Name::new("care package helicopter"), Transform::from_translation(from).with_rotation(Quat::from_rotation_y(yaw)), Visibility::default()))
            .id();
        let model = models.spawn(&mut commands, &["vehicle_little_bird_armed", "vehicle_cobra_helicopter_fly"], heli, true);
        // The crate, slung under it.
        let contents = Contents::roll(&mut rng, heli_paths.available());
        let load = spawn_crate(&mut commands, &mut models, g.thrower, team, contents, heli);
        commands.entity(heli).insert(Courier {
            from,
            over,
            away,
            start: now,
            arrive,
            leave: arrive + HOVER,
            dropped: false,
            load: Some(load),
        });
        sfx.play_on("mp_cobra_helicopter", heli);
        info!(
            "care package: {} ({:?}) to CoD {:?}, {contents:?}",
            model.as_ref().map_or("no model", |m| m.name.as_str()),
            team,
            units::to_cod(target).map(f32::round)
        );
    }
}

/// The crate model for a team: MW2's (Task Force 141's for the allies,
/// the militia's for the axis), else CoD4's plastic case.
fn spawn_crate(commands: &mut Commands, models: &mut StreakModels, owner: Entity, team: Team, contents: Contents, under: Entity) -> Entity {
    let [ours, theirs] = crate::mw2guns::CRATE_MODELS;
    let first = if team == Team::Allies { ours } else { theirs };
    let names = [first, if team == Team::Allies { theirs } else { ours }, "com_plasticcase_beige_big"];
    let e = commands.spawn((Name::new("care package"), Transform::from_translation(-Vec3::Y * u(SLUNG)), Visibility::default(), ChildOf(under))).id();
    let (centre, half) = match models.spawn(commands, &names, e, true) {
        Some(m) if (m.bounds.1 - m.bounds.0).max_element() < 4.0 => {
            ((m.bounds.0 + m.bounds.1) * 0.5, ((m.bounds.1 - m.bounds.0) * 0.5).max(Vec3::splat(u(4.0))))
        }
        _ => (Vec3::ZERO, units::dir([30.0, 16.0, 15.0]).abs() * units::INCH),
    };
    commands.entity(e).insert(CarePackage { owner, team, contents, landed: None, centre, half, opening: None });
    e
}

/// Fly in, hover, let the crate go, fly away.
fn fly(
    mut commands: Commands,
    time: Res<Time>,
    mut couriers: Query<(Entity, &mut Courier, &mut Transform)>,
    globals: Query<&GlobalTransform>,
    crates: Query<&CarePackage>,
    mut sfx: ResMut<crate::audio::Sfx>,
) {
    let now = time.elapsed_secs();
    for (e, mut c, mut tf) in &mut couriers {
        let ease = |t: f32| t * t * (3.0 - 2.0 * t);
        let (at, velocity) = if now < c.arrive {
            // Slowing into the hover.
            let t = ((now - c.start) / (c.arrive - c.start).max(0.01)).clamp(0.0, 1.0);
            let k = 1.0 - (1.0 - t) * (1.0 - t);
            (c.from.lerp(c.over, k), (c.over - c.from) * 2.0 * (1.0 - t) / (c.arrive - c.start).max(0.01))
        } else if now < c.leave {
            let bob = (now * 2.0).sin() * u(6.0);
            (c.over + Vec3::Y * bob, Vec3::ZERO)
        } else {
            let span = c.away.distance(c.over) / u(SPEED);
            let t = ((now - c.leave) / span).clamp(0.0, 1.0);
            if t >= 1.0 {
                commands.entity(e).despawn();
                continue;
            }
            (c.over.lerp(c.away, ease(t) * 0.5 + t * 0.5), (c.away - c.over) / span)
        };
        // Leaning into its speed.
        let flat = velocity.with_y(0.0);
        let way = (c.away - c.from).with_y(0.0).normalize_or(Vec3::X);
        let yaw = (-way.z).atan2(way.x);
        let lean = (flat.dot(way) / u(SPEED)).clamp(-1.0, 1.0) * 0.25;
        tf.translation = at;
        tf.rotation = Quat::from_rotation_y(yaw) * Quat::from_rotation_z(-lean);
        // `dropTheCrate`.
        if !c.dropped && now >= c.arrive + DROP_AT {
            c.dropped = true;
            if let Some(load) = c.load.take() {
                let g = globals.get(load).map(|g| g.compute_transform()).unwrap_or(Transform::from_translation(at - Vec3::Y * u(SLUNG)));
                let spin = Vec3::new(rand::random_range(-0.6..0.6), rand::random_range(-0.4..0.4), rand::random_range(-0.6..0.6));
                let (centre, half) = crates.get(load).map_or((Vec3::ZERO, Vec3::splat(0.3)), |cp| (cp.centre, cp.half));
                commands
                    .entity(load)
                    .remove::<ChildOf>()
                    .insert((g, MovingCrate, Body::loose(centre, half, MASS, "physics_wood"), Moving::thrown(velocity * 0.5 - Vec3::Y * 0.5, spin)));
                sfx.play("iw4/sentry_drop", Some(at));
            }
        }
    }
}

/// A crate let go, still to land.
#[derive(Component)]
struct MovingCrate;

/// A falling crate's weapon, as kills name it.
pub const CRUSH: &str = "care_package";

/// Crates once they've settled: solid, and openable; and gone in time. A
/// falling one crushes whoever it comes down on (`crateDamage`).
#[allow(clippy::type_complexity)]
fn land(
    mut commands: Commands,
    time: Res<Time>,
    mut crates: Query<(Entity, &mut CarePackage, &Transform, Has<Moving>, Has<MovingCrate>)>,
    pawns: Query<(Entity, &Transform), (With<Pawn>, Without<Dead>, Without<CarePackage>)>,
    mut damage: MessageWriter<crate::combat::Damage>,
    mut crushed: Local<HashSet<(Entity, Entity)>>,
) {
    let now = time.elapsed_secs();
    for (e, cp, tf, moving, falling) in &crates {
        if !(falling && moving) {
            continue;
        }
        let middle = tf.transform_point(cp.centre);
        let reach = cp.half.x.max(cp.half.z) + u(12.0);
        for (p, ptf) in &pawns {
            let feet = ptf.translation;
            let over = (middle - feet).with_y(0.0).length() < reach;
            let height = middle.y - cp.half.y < feet.y + u(70.0) && middle.y + cp.half.y > feet.y;
            if over && height && crushed.insert((e, p)) {
                info!("care package: crushed a pawn at CoD {:?} (crate at {:?}, half {:?})", units::to_cod(feet).map(f32::round), units::to_cod(middle).map(f32::round), cp.half);
                damage.write(crate::combat::Damage { target: p, attacker: Some(cp.owner), amount: 1000.0, location: crate::combat::HitLocation::Head, weapon: CRUSH });
            }
        }
    }
    crushed.retain(|(c, _)| crates.contains(*c));
    for (e, mut cp, _, moving, falling) in &mut crates {
        if falling && !moving {
            commands.entity(e).remove::<MovingCrate>();
            cp.landed = Some(now);
            // Solid now: players walk round it, bullets stop on it.
            commands.spawn((
                RigidBody::Static,
                Collider::cuboid(cp.half.x * 2.0, cp.half.y * 2.0, cp.half.z * 2.0),
                CollisionLayers::new(Layer::World, LayerMask::NONE),
                Transform::from_translation(cp.centre),
                ChildOf(e),
            ));
            continue;
        }
        if cp.landed.is_some_and(|t| now - t > CRATE_TIME) {
            commands.entity(e).despawn();
        }
    }
}

/// Holding Use on a landed crate opens it; and the bots' crates to go for.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn open(
    mut commands: Commands,
    time: Res<Time>,
    mut crates: Query<(Entity, &mut CarePackage, &GlobalTransform)>,
    mut pawns: Query<
        (
            Entity,
            &Pawn,
            &Transform,
            &Mover,
            &ViewAngles,
            &mut Killstreak,
            &mut crate::weapons::WeaponState,
            Option<&mut crate::grenades::Grenades>,
            Option<&crate::splitscreen::PlayerInput>,
            Option<&crate::splitscreen::LocalSlot>,
            Option<&UseCrate>,
            Has<crate::bots::Bot>,
        ),
        Without<Dead>,
    >,
    mut notices: MessageWriter<StreakNotice>,
    mut sfx: ResMut<crate::audio::Sfx>,
    sides: Option<Res<crate::audio::Sides>>,
    locals: Query<(), With<crate::player::LocalPlayer>>,
) {
    let now = time.elapsed_secs();
    let dt = time.delta_secs();
    let mut prompts: [Option<(String, Option<f32>)>; 4] = Default::default();
    let mut goals = Vec::new();
    let mut opened = Vec::new();
    for (ce, mut cp, g) in &mut crates {
        let Some(landed) = cp.landed else { continue };
        let middle = g.transform_point(cp.centre);
        let mut user = None;
        for (pe, pawn, tf, mover, view, .., player, slot, bot_use, is_bot) in &pawns {
            let eye = mover.eye(tf.translation);
            let near = (middle - tf.translation).with_y(0.0).length() < u(REACH) && (middle.y - tf.translation.y).abs() < u(90.0);
            let looking = player.is_none() || view.forward().dot((middle - eye).normalize_or_zero()) > 0.6;
            let owner_side = pawns.get(cp.owner).map(|o| !hostile(o.1, pawn)).unwrap_or(pawn.team == cp.team);
            let time_needed = if pe == cp.owner {
                USE_OWNER
            } else if owner_side {
                USE_TEAM
            } else {
                USE_ENEMY
            };
            // Bots: their own crate, or after a while anyone's near them.
            if is_bot {
                let mine = pe == cp.owner && tf.translation.distance(middle) < u(BOT_OWN);
                let nearby = now - landed > BOT_ANY.0 && tf.translation.distance(middle) < u(BOT_ANY.1);
                if mine || nearby {
                    goals.push((pe, middle, tf.translation.distance(middle)));
                }
            }
            if !(near && looking) {
                continue;
            }
            // (A bot's, or the streak test's for the player.)
            let holding = player.is_some_and(|p| p.live && (p.keys.pressed(KeyCode::KeyF) || p.pad.interact)) || bot_use.is_some_and(|u| u.0);
            if let Some(s) = slot.map(|s| s.0).filter(|&s| s < 4) {
                let key = player.map_or_else(|| "F".to_owned(), |p| p.use_key());
                let progress = cp.opening.filter(|o| o.0 == pe).map(|o| (o.1 / time_needed).min(1.0));
                prompts[s] = Some((format!("Hold [{key}] for {}", cp.contents.name()), progress));
            }
            if holding && user.is_none() && cp.opening.is_none_or(|o| o.0 == pe) {
                user = Some((pe, time_needed));
            }
        }
        match user {
            Some((pe, needed)) => {
                let t = cp.opening.filter(|o| o.0 == pe).map_or(0.0, |o| o.1) + dt;
                cp.opening = Some((pe, t));
                if t >= needed {
                    opened.push((ce, pe, cp.contents, cp.owner));
                }
            }
            None => cp.opening = None,
        }
    }
    for (ce, pe, contents, owner) in opened {
        commands.entity(ce).despawn();
        let Ok((_, pawn, tf, _, _, mut streak, mut weapon, grenades, ..)) = pawns.get_mut(pe) else { continue };
        info!("care package: {} opened {owner:?}'s: {}", pawn.name, contents.name());
        sfx.play("iw4/ammo_crate_use", Some(tf.translation));
        let got = match contents {
            Contents::Streak(h) if !streak.held.has(h) => {
                streak.held.add(h);
                Some(h)
            }
            // Already held (or ammo): a refill.
            _ => {
                weapon.reserve = weapon.reserve.max(weapon.def.max_ammo);
                if let Some(mut g) = grenades {
                    g.frags = g.frags.max(1);
                    if g.special.is_some() {
                        g.specials = g.specials.max(1);
                    }
                }
                None
            }
        };
        if locals.contains(pe) {
            notices.write(StreakNotice::Opened { item: got });
            if let (Some(h), Some(sides)) = (got, &sides) {
                sfx.play_later(super::Hardpoint::leader(h, &sides.of(pawn.team).voice), None, now, 0.5);
            }
        }
    }
    if let Ok(mut g) = BOT_GOALS.lock() {
        // The nearest crate each bot is after.
        goals.sort_by(|a, b| a.2.total_cmp(&b.2));
        g.clear();
        for (bot, at, _) in goals {
            if !g.iter().any(|x| x.0 == bot) {
                g.push((bot, at));
            }
        }
    }
    for (s, p) in prompts.into_iter().enumerate() {
        super::CRATE_PROMPTS.set(s, p);
    }
}

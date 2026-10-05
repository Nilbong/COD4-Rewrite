//! Explosive weapons: the class's perk-1 equipment (the RPG-7, C4 and
//! claymores, on 5) and rifles' grenade launchers (the M203 and GP-25, the
//! rifle's alternate mode, also on 5). Firing one ([`crate::weapons`])
//! launches a projectile from its weapon file instead of a bullet:
//!
//! - an RPG-7 rocket flies straight at `iProjectileSpeed` and goes off where
//!   it hits;
//! - a launcher's grenade flies at `iProjectileSpeed` plus
//!   `iProjectileSpeedUp`, falls, and goes off where it hits once it has
//!   flown `iProjectileActivateDist`; before that it's a dud that hurts
//!   whoever it hits (its `damage`) and bounces away inert;
//! - C4 is thrown (leaving the hand `iFireDelay` in), sticks where it lands
//!   and goes off when its owner detonates: aiming with C4 in hand, or
//!   double-tapping F (`watchC4AltDetonation`);
//! - a claymore is set down facing the way its owner faces, and goes off
//!   0.75 s after an enemy moves into its 70° cone within 192 units (and at
//!   least 20 in front; `claymoreDetonation`, `shouldAffectClaymore`).
//!
//! They hurt everyone in sight within `iExplosionRadius`, from
//! `iExplosionInnerDamage` to `iExplosionOuterDamage` at the edge, through
//! [`Damage`] (teammates spared, the owner not). Other games' launchers can
//! register their projectiles in [`ExplosiveDefs`].

use crate::collision;
use crate::combat::{Damage, Dead, HitLocation, Hitbox, Pawn};
use crate::content::Content;
use crate::fx::{Anchor, Effects, Frame, FxLayer};
use crate::models::{Skeleton, SpawnModel, spawn_model};
use crate::movement::{Mover, ViewAngles};
use crate::player::LocalPlayer;
use crate::units::u;
use crate::weapons::{WeaponInput, WeaponState};
use avian3d::prelude::*;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use iw3::zone::{Asset, AssetType};
use std::collections::HashMap;

pub struct ExplosivesPlugin;

impl Plugin for ExplosivesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ExplosiveDefs>()
            .init_resource::<Blasts>()
            .add_message::<Launch>()
            .add_message::<Exploded>()
            .add_systems(OnEnter(crate::state::GameState::InGame), load_defs.after(crate::world::load_map).in_set(crate::state::Setup::Content))
            .add_systems(
                Update,
                (launch, fly, detonate, claymores, explode).chain().after(crate::weapons::WeaponSet).run_if(crate::state::in_game),
            );
        if let Ok(dir) = std::env::var("COD4RW_EQUIPTEST") {
            app.insert_resource(TestDir(dir.into()))
                .add_systems(PreUpdate, test.after(bevy::input::InputSystems).run_if(crate::state::in_game));
        }
    }
}

/// How a weapon's projectile behaves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Behaviour {
    /// Flies straight and goes off where it hits (the RPG-7).
    Rocket,
    /// Falls, and goes off where it hits once armed (launchers).
    Launched,
    /// Sticks where it lands until detonated (C4).
    Sticky,
    /// Set down facing its owner's way, set off by enemies in front.
    Claymore,
}

/// A weapon's projectile, from its weapon file.
#[derive(Clone, Debug)]
pub struct ExplosiveDef {
    pub behaviour: Behaviour,
    /// CoD units a second, along the aim and upward.
    pub speed: f32,
    pub speed_up: f32,
    /// Seconds into the fire animation it leaves the hand.
    pub delay: f32,
    /// How far (CoD units) a launched grenade flies before it can go off,
    /// and what it does to whoever it hits before then.
    pub arming: f32,
    pub impact_damage: f32,
    pub radius: f32,
    pub inner_damage: f32,
    pub outer_damage: f32,
    pub model: String,
    pub trail: String,
    pub effect: String,
    pub sound: String,
    /// The kill feed's name for it, and its icon there.
    pub display: &'static str,
    pub icon: String,
}

/// Explosive weapons by weapon name (`rpg_mp`, `gl_m16_mp`).
#[derive(Resource, Default)]
pub struct ExplosiveDefs(HashMap<String, ExplosiveDef>);

impl ExplosiveDefs {
    pub fn get(&self, weapon: &str) -> Option<&ExplosiveDef> {
        self.0.get(weapon)
    }

    /// Another game's launcher: its weapon (`WeaponDef::name`) fires this.
    pub fn insert(&mut self, weapon: impl Into<String>, def: ExplosiveDef) {
        self.0.insert(weapon.into(), def);
    }

    /// The kill feed icon for a [`Damage::weapon`] that's one of these.
    pub fn kill_icon(&self, weapon: &str) -> Option<&str> {
        self.0.values().find(|d| d.display == weapon).map(|d| d.icon.as_str())
    }
}

/// Is this [`Damage::weapon`] one of these (CoD4's, or World at War's
/// rifle grenades)?
pub fn is_explosive(weapon: &str) -> bool {
    matches!(weapon, "RPG-7" | "C4" | "Claymore" | "M203" | "GP-25" | "Rifle Grenade")
}

/// Sonic Boom (`specialty_explosivedamage`): explosives hurt 25% more
/// (`perk_explosiveDamage`).
pub const SONIC_BOOM: f32 = 1.25;

/// CoD4's equipment and launchers.
fn load_defs(mut defs: ResMut<ExplosiveDefs>, content: Res<Content>) {
    let zone = &content.zones[crate::content::COMMON_ZONE];
    let names: Vec<String> = zone
        .assets
        .iter()
        .filter_map(|a| match a {
            Asset::Generic(g) if g.ty == AssetType::Weapon => Some(g.name.clone()),
            _ => None,
        })
        .filter(|n| matches!(n.as_str(), "rpg_mp" | "c4_mp" | "claymore_mp") || n.starts_with("gl_"))
        .collect();
    for name in names {
        let Some((zi, w)) = content.generic(AssetType::Weapon, &name) else { continue };
        let zone = &content.zones[zi];
        let asset = |f: &str| w.asset(f).map(|i| zone.get(i).name().trim_start_matches(',').to_owned());
        let sound = |f: &str| w.node(f).and_then(|n| n.node("name")).and_then(|n| n.string("soundName")).map(str::to_owned);
        let (behaviour, display, default_sound) = match name.as_str() {
            "rpg_mp" => (Behaviour::Rocket, "RPG-7", "rocket_explode_default"),
            "c4_mp" => (Behaviour::Sticky, "C4", "detpack_explo_default"),
            "claymore_mp" => (Behaviour::Claymore, "Claymore", "detpack_explo_main"),
            "gl_ak47_mp" => (Behaviour::Launched, "GP-25", "grenade_explode_default"),
            _ => (Behaviour::Launched, "M203", "grenade_explode_default"),
        };
        let def = ExplosiveDef {
            behaviour,
            speed: w.int("iProjectileSpeed") as f32,
            speed_up: w.int("iProjectileSpeedUp") as f32,
            delay: w.int("iFireDelay") as f32 / 1000.0,
            arming: w.int("iProjectileActivateDist") as f32,
            impact_damage: w.int("damage") as f32,
            radius: w.int("iExplosionRadius") as f32,
            inner_damage: w.int("iExplosionInnerDamage") as f32,
            outer_damage: w.int("iExplosionOuterDamage") as f32,
            model: asset("projectileModel").unwrap_or_default(),
            trail: asset("projTrailEffect").unwrap_or_default(),
            effect: asset("projExplosionEffect").unwrap_or_else(|| "explosions/grenadeexp_default".into()),
            sound: sound("projExplosionSound").unwrap_or_else(|| default_sound.into()),
            display,
            icon: asset("killIcon").unwrap_or_default(),
        };
        debug!("explosives: {name}: {:?}, radius {}, {}..{}", def.behaviour, def.radius, def.inner_damage, def.outer_damage);
        defs.0.insert(name, def);
    }
    info!("explosives: {} weapons", defs.0.len());
}

/// Something went off: a grenade (`frag_grenade_mp`, ...) or an explosive
/// weapon's projectile, at `at`, hurting within `radius` (CoD units) from
/// `inner` to `outer` damage, with its effect and sound (for whoever else
/// takes damage or replays it: helicopters, the killcam).
#[derive(Message, Clone, Debug)]
pub struct Exploded {
    pub weapon: String,
    pub owner: Entity,
    pub at: Vec3,
    pub radius: f32,
    pub inner: f32,
    pub outer: f32,
    pub effect: String,
    pub sound: String,
}

/// Fired from an explosive weapon ([`crate::weapons`]): its projectile
/// leaves along `dir` (the aim, without spread).
#[derive(Message, Clone, Debug)]
pub struct Launch {
    pub shooter: Entity,
    pub weapon: String,
    pub from: Vec3,
    pub dir: Vec3,
    pub yaw: f32,
}

/// A projectile in the world.
#[derive(Component, Debug)]
pub struct Explosive {
    pub weapon: String,
    pub owner: Entity,
    pub velocity: Vec3,
    /// How far it has flown, CoD units.
    pub travelled: f32,
    pub state: State,
    /// Its trail starts once it's in the world (a frame after it spawns).
    trail: bool,
    born: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum State {
    /// On its way (or, for C4 and claymores, still landing).
    Flying,
    /// C4 stuck where it landed; a claymore set down.
    Stuck,
    /// A claymore set off: goes off then.
    Triggered(f32),
    /// A launcher's grenade that hit before arming, lying inert until then.
    Dud(f32),
}

/// Projectiles to set off this frame.
#[derive(Resource, Default)]
struct Blasts(Vec<(String, Entity, Vec3)>);

/// CoD4's gravity, `g_gravity`.
const GRAVITY: f32 = 800.0;
/// The longest a rocket flies before it goes off anyway, and how long a dud
/// lies about.
const ROCKET_LIFE: f32 = 10.0;
const DUD_LIFE: f32 = 10.0;
/// `level.claymoreDetonateRadius`, `claymoreDetectionConeAngle` (70°),
/// `claymoreDetectionMinDist`, `claymoreDetectionGracePeriod`.
const CLAYMORE_RADIUS: f32 = 192.0;
const CLAYMORE_DOT: f32 = 0.342_020_14;
const CLAYMORE_MIN_DIST: f32 = 20.0;
const CLAYMORE_DELAY: f32 = 0.75;
/// Two presses of F within this detonate C4 (`watchC4AltDetonation`).
const DOUBLE_TAP: f32 = 0.5;

/// Projectiles leaving their weapons, after their fire delay.
#[allow(clippy::too_many_arguments)]
fn launch(
    mut commands: Commands,
    time: Res<Time>,
    defs: Res<ExplosiveDefs>,
    mut launches: MessageReader<Launch>,
    mut waiting: Local<Vec<(f32, Launch)>>,
    mut content: ResMut<Content>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut bindposes: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    mut sfx: ResMut<crate::audio::Sfx>,
    mut waw: ResMut<crate::waw::MatchContent>,
) {
    let now = time.elapsed_secs();
    for l in launches.read() {
        let delay = defs.get(&l.weapon).map_or(0.0, |d| d.delay);
        waiting.push((now + delay, l.clone()));
    }
    let (due, later): (Vec<_>, Vec<_>) = std::mem::take(&mut *waiting).into_iter().partition(|(at, _)| *at <= now);
    *waiting = later;
    for (_, l) in due {
        let Some(def) = defs.get(&l.weapon) else { continue };
        let velocity = match def.behaviour {
            Behaviour::Rocket => l.dir * u(def.speed),
            Behaviour::Launched | Behaviour::Sticky => l.dir * u(def.speed) + Vec3::Y * u(def.speed_up),
            // Set down a little ahead.
            Behaviour::Claymore => Quat::from_rotation_y(l.yaw) * Vec3::NEG_Z * u(80.0),
        };
        // Models face +X; along the way they're thrown.
        let facing = match def.behaviour {
            Behaviour::Rocket | Behaviour::Launched => Transform::IDENTITY.looking_to(l.dir, Vec3::Y).rotation * Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            _ => Quat::from_rotation_y(l.yaw + std::f32::consts::FRAC_PI_2),
        };
        let e = commands
            .spawn((
                Name::new(l.weapon.clone()),
                Explosive { weapon: l.weapon.clone(), owner: l.shooter, velocity, travelled: 0.0, state: State::Flying, trail: def.trail.is_empty(), born: now },
                Transform::from_translation(l.from).with_rotation(facing),
                Visibility::default(),
            ))
            .id();
        // World at War's projectiles (`t4/<model>`) come from its own content.
        let model = match def.model.strip_prefix(crate::waw::MATERIAL_PREFIX) {
            Some(name) => waw.get().and_then(|c| {
                let name = crate::waw::model_name(c, name)?;
                c.model(&name, &mut meshes, &mut materials, &mut images, &mut bindposes)
            }),
            None => content.model(&def.model, &mut meshes, &mut materials, &mut images, &mut bindposes),
        };
        if let Some(m) = model {
            spawn_model(&mut commands, &mut Skeleton::default(), SpawnModel { model: &m, owner: e, attach_to: None, layers: None, shadows: true });
        }
        if def.behaviour == Behaviour::Claymore {
            sfx.play("claymore_plant", Some(l.from));
        }
    }
}

/// Fly projectiles: rockets straight, the rest falling; go off, stick,
/// settle or dud where they hit.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn fly(
    mut commands: Commands,
    time: Res<Time>,
    defs: Res<ExplosiveDefs>,
    spatial: SpatialQuery,
    hitboxes: Query<&Hitbox>,
    mut projectiles: Query<(Entity, &mut Explosive, &mut Transform)>,
    mut effects: ResMut<Effects>,
    mut damage: MessageWriter<Damage>,
    mut blasts: ResMut<Blasts>,
) {
    let (now, dt) = (time.elapsed_secs(), time.delta_secs());
    for (e, mut p, mut tf) in &mut projectiles {
        let Some(def) = defs.get(&p.weapon) else { continue };
        if !p.trail {
            p.trail = true;
            effects.play(&def.trail, Anchor::Bolted(e), FxLayer::World);
        }
        match p.state {
            State::Dud(until) if now >= until => {
                commands.entity(e).despawn();
                continue;
            }
            State::Flying => {}
            _ => continue,
        }
        if def.behaviour == Behaviour::Rocket && now - p.born > ROCKET_LIFE {
            blasts.0.push((p.weapon.clone(), p.owner, tf.translation));
            commands.entity(e).despawn();
            continue;
        }
        if def.behaviour != Behaviour::Rocket {
            p.velocity.y -= u(GRAVITY) * dt;
        }
        let step = p.velocity * dt;
        let Ok(dir) = Dir3::new(step) else { continue };
        // Rockets and launched grenades hit players too; C4 and claymores
        // only the world. Never their owner.
        let owner = p.owner;
        let not_owner = |h: Entity| hitboxes.get(h).map_or(true, |hb| hb.owner != owner);
        let filter = match def.behaviour {
            Behaviour::Rocket | Behaviour::Launched => collision::bullet_filter(),
            _ => collision::sight_filter(),
        };
        let Some(hit) = spatial.cast_ray_predicate(tf.translation, dir, step.length(), true, &filter, &not_owner) else {
            tf.translation += step;
            p.travelled += step.length() / crate::units::INCH;
            if matches!(def.behaviour, Behaviour::Rocket | Behaviour::Launched) {
                tf.rotation = Transform::IDENTITY.looking_to(p.velocity, Vec3::Y).rotation * Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
            }
            continue;
        };
        let at = tf.translation + dir * hit.distance;
        p.travelled += hit.distance / crate::units::INCH;
        let pawn = hitboxes.get(hit.entity).ok().map(|h| h.owner);
        match def.behaviour {
            Behaviour::Launched if p.travelled < def.arming => {
                // A dud: it hurts whoever it hits, then bounces off inert.
                if let Some(target) = pawn {
                    damage.write(Damage { target, attacker: Some(owner), amount: def.impact_damage, location: HitLocation::Torso, weapon: def.display });
                }
                let n = hit.normal;
                p.velocity = (p.velocity - 2.0 * p.velocity.dot(n) * n) * 0.3;
                tf.translation = at + n * u(1.0);
                p.state = State::Dud(now + DUD_LIFE);
            }
            Behaviour::Rocket | Behaviour::Launched => {
                blasts.0.push((p.weapon.clone(), owner, at - dir * u(4.0)));
                commands.entity(e).despawn();
            }
            Behaviour::Sticky => {
                tf.translation = at + hit.normal * u(1.0);
                p.state = State::Stuck;
            }
            // A claymore stands on the floor; off a wall, it drops.
            Behaviour::Claymore if hit.normal.y > 0.7 => {
                tf.translation = at;
                p.state = State::Stuck;
            }
            Behaviour::Claymore => {
                tf.translation = at + hit.normal * u(2.0);
                p.velocity = Vec3::new(0.0, p.velocity.y.min(0.0), 0.0);
            }
        }
    }
}

/// Owners detonate their C4: aiming with C4 in hand, or a double tap of F
/// (the player).
#[allow(clippy::type_complexity)]
fn detonate(
    time: Res<Time>,
    defs: Res<ExplosiveDefs>,
    pawns: Query<(Entity, &WeaponState, &WeaponInput, Option<&crate::splitscreen::PlayerInput>), Without<Dead>>,
    projectiles: Query<(Entity, &Explosive, &Transform)>,
    mut commands: Commands,
    mut held: Local<HashMap<Entity, bool>>,
    mut last_f: Local<HashMap<Entity, f32>>,
    mut blasts: ResMut<Blasts>,
) {
    let now = time.elapsed_secs();
    let mut detonating = Vec::new();
    for (e, w, input, local) in &pawns {
        let c4_in_hand = defs.get(&w.def.name).is_some_and(|d| d.behaviour == Behaviour::Sticky);
        let was = held.insert(e, input.ads).unwrap_or(false);
        let mut go = c4_in_hand && input.ads && !was;
        if local.is_some_and(|p| p.live && p.keys.just_pressed(KeyCode::KeyF)) {
            let last = last_f.insert(e, now).unwrap_or(f32::NEG_INFINITY);
            go |= now - last < DOUBLE_TAP;
        }
        if go {
            detonating.push(e);
        }
    }
    for (pe, p, tf) in &projectiles {
        let sticky = defs.get(&p.weapon).is_some_and(|d| d.behaviour == Behaviour::Sticky);
        if sticky && detonating.contains(&p.owner) {
            blasts.0.push((p.weapon.clone(), p.owner, tf.translation));
            commands.entity(pe).despawn();
        }
    }
}

/// Claymores set off by enemies moving in front of them, a moment before
/// they go off.
#[allow(clippy::type_complexity)]
fn claymores(
    mut commands: Commands,
    time: Res<Time>,
    defs: Res<ExplosiveDefs>,
    spatial: SpatialQuery,
    pawns: Query<(Entity, &Transform, &Mover, &Pawn), Without<Dead>>,
    mut projectiles: Query<(Entity, &mut Explosive, &Transform)>,
    mut sfx: ResMut<crate::audio::Sfx>,
    mut blasts: ResMut<Blasts>,
) {
    let now = time.elapsed_secs();
    for (e, mut p, tf) in &mut projectiles {
        if defs.get(&p.weapon).is_none_or(|d| d.behaviour != Behaviour::Claymore) {
            continue;
        }
        match p.state {
            State::Triggered(at) if now >= at => {
                blasts.0.push((p.weapon.clone(), p.owner, tf.translation + Vec3::Y * u(4.0)));
                commands.entity(e).despawn();
                continue;
            }
            State::Stuck => {}
            _ => continue,
        }
        let owner_pawn = pawns.get(p.owner).ok().map(|o| o.3);
        // The model faces +X: its front.
        let forward = tf.rotation * Vec3::X;
        let origin = tf.translation + Vec3::Y * u(4.0);
        let tripped = pawns.iter().any(|(pe, ptf, mover, pawn)| {
            if pe == p.owner || owner_pawn.is_some_and(|o| !crate::combat::hostile(o, pawn)) || mover.velocity.length_squared() < u(3.0).powi(2) {
                return false;
            }
            let pos = ptf.translation + Vec3::Y * u(32.0);
            let to = pos - origin;
            let flat = Vec2::new(to.x, to.z).length() / crate::units::INCH;
            if flat > CLAYMORE_RADIUS || (to.y / crate::units::INCH).abs() > CLAYMORE_RADIUS {
                return false;
            }
            let ahead = to.dot(forward) / crate::units::INCH;
            if ahead < CLAYMORE_MIN_DIST || to.normalize_or_zero().dot(forward) <= CLAYMORE_DOT {
                return false;
            }
            Dir3::new(to).ok().is_some_and(|d| spatial.cast_ray(origin, d, to.length(), true, &collision::sight_filter()).is_none())
        });
        if tripped {
            p.state = State::Triggered(now + CLAYMORE_DELAY);
            sfx.play("claymore_activated", Some(tf.translation));
        }
    }
}

/// Set projectiles off: everyone in sight within the radius is hurt, from
/// the inner to the outer damage, with the effect and sound.
#[allow(clippy::too_many_arguments)]
fn explode(
    mut blasts: ResMut<Blasts>,
    defs: Res<ExplosiveDefs>,
    spatial: SpatialQuery,
    pawns: Query<(Entity, &Transform, &Mover, &Pawn), Without<Dead>>,
    loadouts: Query<&crate::loadout::Loadout>,
    mut damage: MessageWriter<Damage>,
    mut effects: ResMut<Effects>,
    mut sfx: ResMut<crate::audio::Sfx>,
    mut exploded: MessageWriter<Exploded>,
) {
    for (weapon, owner, at) in std::mem::take(&mut blasts.0) {
        let Some(def) = defs.get(&weapon) else { continue };
        let boom = if crate::perks::has(loadouts.get(owner).ok(), "specialty_explosivedamage") { SONIC_BOOM } else { 1.0 };
        let from = at + Vec3::Y * u(4.0);
        let seen = |to: Vec3| {
            let d = to - from;
            Dir3::new(d).ok().is_none_or(|dir| spatial.cast_ray(from, dir, d.length(), true, &collision::sight_filter()).is_none())
        };
        let mut hurt = Vec::new();
        for (target, tf, mover, pawn) in &pawns {
            let (centre, eye) = (tf.translation + Vec3::Y * u(32.0), mover.eye(tf.translation));
            let dist = ((centre - at).length() / crate::units::INCH - 16.0).max(0.0);
            if dist > def.radius || !(seen(centre) || seen(eye)) {
                continue;
            }
            let amount = (def.inner_damage + (def.outer_damage - def.inner_damage) * (dist / def.radius.max(1.0))) * boom;
            damage.write(Damage { target, attacker: Some(owner), amount, location: HitLocation::Torso, weapon: def.display });
            hurt.push(format!("{} {amount:.0}", pawn.name));
        }
        debug!("explosives: {weapon} went off at CoD {:?}, reaching {hurt:?}", crate::units::to_cod(at).map(f32::round));
        effects.play(&def.effect, Anchor::Fixed(Frame::facing(at, Vec3::Y, 0.0)), FxLayer::World);
        sfx.play(def.sound.clone(), Some(at));
        exploded.write(Exploded {
            radius: def.radius,
            inner: def.inner_damage * boom,
            outer: def.outer_damage * boom,
            effect: def.effect.clone(),
            sound: def.sound.clone(),
            weapon,
            owner,
            at,
        });
    }
}

#[derive(Resource)]
struct TestDir(std::path::PathBuf);

/// Debug aid: with `COD4RW_EQUIPTEST=<dir>` (and `COD4RW_LOADOUT` /
/// `COD4RW_INVENTORY` for the class), press 5 for the equipment or
/// launcher at 5.5 s, fire it at 7 s a little downwards, detonate C4 at 9 s,
/// and screenshot each step; then exit.
fn test(
    mut commands: Commands,
    time: Res<Time>,
    dir: Res<TestDir>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    mut player: Query<&mut ViewAngles, With<LocalPlayer>>,
    mut cursor: Single<&mut CursorOptions, With<PrimaryWindow>>,
    mut step: Local<usize>,
    mut exit: MessageWriter<AppExit>,
) {
    use bevy::render::view::screenshot::{Screenshot, save_to_disk};
    let t = time.elapsed_secs();
    // The game reads the keys only while it has the mouse.
    if cursor.grab_mode == CursorGrabMode::None {
        cursor.grab_mode = CursorGrabMode::Locked;
    }
    let held = |from: f32| (from..from + 0.1).contains(&t);
    for (key, at) in [(KeyCode::Digit5, 5.5)] {
        if held(at) { keys.press(key) } else { keys.release(key) }
    }
    for (button, at) in [(MouseButton::Left, 7.0), (MouseButton::Right, 9.0)] {
        if held(at) { mouse.press(button) } else { mouse.release(button) }
    }
    if let Ok(mut view) = player.single_mut() {
        if (5.0..7.2).contains(&t) {
            view.pitch = -0.3;
        }
    }
    const SHOTS: [(f32, &str); 6] = [(5.4, "before"), (6.6, "equipped"), (7.15, "fired"), (7.6, "flight"), (9.12, "detonated"), (10.5, "after")];
    match SHOTS.get(*step) {
        Some(&(at, name)) if t >= at => {
            std::fs::create_dir_all(&dir.0).ok();
            commands.spawn(Screenshot::primary_window()).observe(save_to_disk(dir.0.join(format!("equip_{}_{name}.png", *step))));
            *step += 1;
        }
        None if t > 11.5 => {
            exit.write(AppExit::Success);
        }
        _ => {}
    }
}

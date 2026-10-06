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
                (launch, fly, detonate, damaged, claymores, explode).chain().after(crate::weapons::WeaponSet).run_if(crate::state::in_game),
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
    /// CoD units a second, along the aim, upward and level forward
    /// (`Weapon_Throw_Grenade`: a claymore goes down and ahead at once).
    pub speed: f32,
    pub speed_up: f32,
    pub speed_forward: f32,
    /// Seconds into the fire animation it leaves the hand.
    pub delay: f32,
    /// How far (CoD units) a launched grenade flies before it can go off,
    /// and what it does to whoever it hits before then.
    pub arming: f32,
    pub impact_damage: f32,
    pub radius: f32,
    pub inner_damage: f32,
    pub outer_damage: f32,
    /// How long it flies before going off anyway (`projLifetime`, s).
    pub lifetime: f32,
    /// The blast reaches this far round its front (`damageConeAngle`,
    /// degrees: a claymore's 60; 180 all round).
    pub cone: f32,
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

/// An explosion's damage scale from its owner's perks against the target's
/// (`cac_modified_damage`): Sonic Boom's 1.25, except that against
/// Juggernaut the two cancel out (Juggernaut's own 0.75 is applied later,
/// to everything, by `crate::combat`, so this undoes it).
pub fn blast_scale(attacker: Option<&crate::loadout::Loadout>, target: Option<&crate::loadout::Loadout>) -> f32 {
    if !crate::perks::has(attacker, "specialty_explosivedamage") {
        1.0
    } else if crate::perks::has(target, "specialty_armorvest") {
        1.0 / 0.75
    } else {
        SONIC_BOOM
    }
}

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
            speed_forward: w.int("iProjectileSpeedForward") as f32,
            delay: w.int("iFireDelay") as f32 / 1000.0,
            arming: w.int("iProjectileActivateDist") as f32,
            impact_damage: w.int("damage") as f32,
            radius: w.int("iExplosionRadius") as f32,
            inner_damage: w.int("iExplosionInnerDamage") as f32,
            outer_damage: w.int("iExplosionOuterDamage") as f32,
            lifetime: match w.float("projLifetime") {
                l if l > 0.0 => l,
                _ => ROCKET_LIFE,
            },
            cone: match w.float("damageConeAngle") {
                c if c > 0.0 => c,
                _ => 180.0,
            },
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

/// Projectiles to set off this frame: the weapon, its owner, where, and
/// which way it faced (a claymore's blast goes forward).
#[derive(Resource, Default)]
struct Blasts(Vec<(String, Entity, Vec3, Vec3)>);

/// CoD4's gravity, `g_gravity`.
const GRAVITY: f32 = 800.0;
/// A projectile's flight when its weapon gives none, and how long a dud lies
/// about.
const ROCKET_LIFE: f32 = 10.0;
const DUD_LIFE: f32 = 10.0;
/// C4 goes off this long after the detonator (`waitAndDetonate( 0.1 )`).
const C4_DELAY: f32 = 0.1;
/// Equipment hit by a blast for this much goes off, after a moment
/// (`claymoreDetonation`'s damage check, 0.1..0.5 s when others went too).
const CHAIN_DAMAGE: f32 = 5.0;
const CHAIN_DELAY: (f32, f32) = (0.1, 0.5);
/// A placed C4 or claymore's middle above its origin, and how near it a
/// bullet hits it (inches).
const EQUIPMENT_HEIGHT: f32 = 4.0;
const EQUIPMENT_SIZE: f32 = 6.0;
/// `level.claymoreDetonateRadius`, `claymoreDetectionConeAngle` (70°),
/// `claymoreDetectionMinDist`, `claymoreDetectionGracePeriod`.
const CLAYMORE_RADIUS: f32 = 192.0;
const CLAYMORE_DOT: f32 = 0.342_020_14;
const CLAYMORE_MIN_DIST: f32 = 20.0;
const CLAYMORE_DELAY: f32 = 0.75;
/// `watchC4AltDetonation`: a press of F held no longer than this, then
/// another within this of letting go, sets off the C4 (not with it in hand).
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
    movers: Query<&Mover>,
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
        // With the shooter's own speed: all of it for a rocket (`gunVel`),
        // its part along the throw for the rest (`G_GrenadeLaunch`).
        let own = movers.get(l.shooter).map_or(Vec3::ZERO, |m| m.velocity);
        let velocity = match def.behaviour {
            Behaviour::Rocket => l.dir * u(def.speed) + own,
            Behaviour::Launched | Behaviour::Sticky | Behaviour::Claymore => {
                let level = Quat::from_rotation_y(l.yaw) * Vec3::NEG_Z;
                let thrown = l.dir * u(def.speed) + Vec3::Y * u(def.speed_up) + level * u(def.speed_forward);
                // A claymore is set down: none of its owner's speed.
                if def.behaviour == Behaviour::Claymore { thrown } else { thrown + l.dir * own.dot(l.dir).max(0.0) }
            }
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
        if matches!(def.behaviour, Behaviour::Rocket | Behaviour::Launched) && now - p.born > def.lifetime {
            blasts.0.push((p.weapon.clone(), p.owner, tf.translation, Vec3::Y));
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
                    let location = hitboxes.get(hit.entity).map_or(HitLocation::Torso, |h| h.location);
                    damage.write(Damage { target, attacker: Some(owner), amount: def.impact_damage, location, weapon: def.display });
                }
                let n = hit.normal;
                p.velocity = (p.velocity - 2.0 * p.velocity.dot(n) * n) * 0.3;
                tf.translation = at + n * u(1.0);
                p.state = State::Dud(now + DUD_LIFE);
            }
            Behaviour::Rocket | Behaviour::Launched => {
                blasts.0.push((p.weapon.clone(), owner, at - dir * u(4.0), Vec3::Y));
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
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn detonate(
    time: Res<Time>,
    defs: Res<ExplosiveDefs>,
    pawns: Query<(Entity, &WeaponState, &WeaponInput, Option<&crate::splitscreen::PlayerInput>), Without<Dead>>,
    mut projectiles: Query<(Entity, &mut Explosive, &Transform)>,
    mut commands: Commands,
    mut held: Local<HashMap<Entity, bool>>,
    // F: when the last press began and ended.
    mut presses: Local<HashMap<Entity, (f32, f32)>>,
    mut respawned: RemovedComponents<Dead>,
    mut blasts: ResMut<Blasts>,
) {
    let now = time.elapsed_secs();
    let mut detonating = Vec::new();
    for (e, w, input, local) in &pawns {
        let c4_in_hand = defs.get(&w.def.name).is_some_and(|d| d.behaviour == Behaviour::Sticky);
        // With C4 in hand the player's fire button is the detonator (aim
        // throws: see `crate::weapons`); a bot's aim is its detonator.
        let button = if local.is_some() { input.fire } else { input.ads };
        let was = held.insert(e, button).unwrap_or(false);
        let mut go = c4_in_hand && button && !was;
        if let Some(p) = local.filter(|p| p.live) {
            let f = p.keys.pressed(KeyCode::KeyF) || p.pad.interact;
            let press = presses.entry(e).or_insert((f32::NEG_INFINITY, f32::NEG_INFINITY));
            if p.keys.just_pressed(KeyCode::KeyF) {
                // A quick press, let go, and pressed again soon after.
                let quick = press.1 - press.0 <= DOUBLE_TAP && press.1 >= press.0;
                go |= !c4_in_hand && quick && now - press.1 <= DOUBLE_TAP;
                press.0 = now;
            } else if p.keys.just_released(KeyCode::KeyF) || (!f && press.1 < press.0) {
                press.1 = now;
            }
        }
        if go {
            detonating.push(e);
        }
    }
    // Respawning, an owner's claymores and C4 go (`deleteExplosivesOnSpawn`);
    // so do those whose owner has left.
    let respawned: Vec<Entity> = respawned.read().collect();
    for (pe, mut p, tf) in &mut projectiles {
        let Some(def) = defs.get(&p.weapon) else { continue };
        let placed = matches!(def.behaviour, Behaviour::Sticky | Behaviour::Claymore);
        if placed && (respawned.contains(&p.owner) || pawns.get(p.owner).is_err() && commands.get_entity(p.owner).is_err()) {
            commands.entity(pe).despawn();
            continue;
        }
        if def.behaviour != Behaviour::Sticky {
            continue;
        }
        if detonating.contains(&p.owner) && !matches!(p.state, State::Triggered(_)) {
            p.state = State::Triggered(now + C4_DELAY);
        }
        if let State::Triggered(at) = p.state {
            if now >= at {
                blasts.0.push((p.weapon.clone(), p.owner, tf.translation, Vec3::Y));
                commands.entity(pe).despawn();
            }
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
                blasts.0.push((p.weapon.clone(), p.owner, tf.translation + Vec3::Y * u(4.0), tf.rotation * Vec3::X));
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

/// Placed C4 and claymores hit by a player's bullet, or caught in a blast
/// for at least [`CHAIN_DAMAGE`], go off a moment later, for whoever set
/// them off (`c4Damage`: not a teammate's, friendly fire off).
#[allow(clippy::type_complexity)]
fn damaged(
    time: Res<Time>,
    defs: Res<ExplosiveDefs>,
    mut shots: MessageReader<crate::weapons::ShotFired>,
    mut exploded: MessageReader<Exploded>,
    pawns: Query<&Pawn>,
    mut equipment: Query<(&mut Explosive, &Transform)>,
    mut last: Local<f32>,
) {
    let now = time.elapsed_secs();
    // Who did it and where: bullets as segments, blasts as points with
    // their damage reaching out.
    let mut hits: Vec<(Entity, Vec3, Vec3, Option<(f32, f32, f32)>)> = Vec::new();
    for s in shots.read().filter(|s| s.weapon.is_some()) {
        hits.push((s.shooter, s.from, s.to, None));
    }
    for x in exploded.read().filter(|x| x.radius > 0.0 && x.inner.max(x.outer) >= CHAIN_DAMAGE) {
        hits.push((x.owner, x.at, x.at, Some((x.radius, x.inner, x.outer))));
    }
    if hits.is_empty() {
        return;
    }
    for (mut p, tf) in &mut equipment {
        if p.state != State::Stuck || defs.get(&p.weapon).is_none_or(|d| !matches!(d.behaviour, Behaviour::Sticky | Behaviour::Claymore)) {
            continue;
        }
        let middle = tf.translation + Vec3::Y * u(EQUIPMENT_HEIGHT);
        let by = hits.iter().find(|&&(attacker, from, to, blast)| {
            let allowed = attacker == p.owner
                || match (pawns.get(attacker), pawns.get(p.owner)) {
                    (Ok(a), Ok(b)) => crate::combat::hostile(a, b),
                    // Not a player's, no harm; an owner gone, anyone's.
                    (Err(_), _) => false,
                    (_, Err(_)) => true,
                };
            allowed
                && match blast {
                    Some((radius, inner, outer)) => {
                        let dist = middle.distance(from) / crate::units::INCH;
                        dist <= radius && inner + (outer - inner) * (dist / radius) >= CHAIN_DAMAGE
                    }
                    None => {
                        let along = to - from;
                        let t = ((middle - from).dot(along) / along.length_squared().max(1e-6)).clamp(0.0, 1.0);
                        (from + along * t).distance(middle) <= u(EQUIPMENT_SIZE)
                    }
                }
        });
        if let Some(&(attacker, ..)) = by {
            // A moment longer when another just went (`c4explodethisframe`).
            let wait = if now - *last < 0.05 { rand::random_range(CHAIN_DELAY.0..CHAIN_DELAY.1) } else { 0.05 };
            *last = now;
            p.state = State::Triggered(now + wait);
            p.owner = attacker;
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
    spawned: Query<&crate::combat::Spawned>,
    time: Res<Time>,
    mut damage: MessageWriter<Damage>,
    mut effects: ResMut<Effects>,
    mut sfx: ResMut<crate::audio::Sfx>,
    mut exploded: MessageWriter<Exploded>,
) {
    for (weapon, owner, at, front) in std::mem::take(&mut blasts.0) {
        let Some(def) = defs.get(&weapon) else { continue };
        let boom = if crate::perks::has(loadouts.get(owner).ok(), "specialty_explosivedamage") { SONIC_BOOM } else { 1.0 };
        let from = at + Vec3::Y * u(4.0);
        let cone = def.cone.to_radians().cos();
        let mut hurt = Vec::new();
        for (target, tf, mover, pawn) in &pawns {
            // From the blast to the feet (the player's origin).
            let dist = (tf.translation - at).length() / crate::units::INCH;
            if dist > def.radius {
                continue;
            }
            // A claymore's blast goes forward only.
            if def.cone < 180.0 && (tf.translation + Vec3::Y * u(32.0) - at).normalize_or_zero().dot(front) < cone {
                continue;
            }
            let cover = exposure(&spatial, from, tf.translation, mover.eye(tf.translation));
            if cover <= 0.0 {
                continue;
            }
            // Grenades (not rockets) spare players just spawned near them.
            if def.behaviour != Behaviour::Rocket && crate::combat::spawn_protected(spawned.get(target).ok(), at, time.elapsed_secs()) {
                continue;
            }
            let scale = blast_scale(loadouts.get(owner).ok(), loadouts.get(target).ok()) * cover;
            let amount = (def.inner_damage + (def.outer_damage - def.inner_damage) * (dist / def.radius.max(1.0))) * scale;
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

/// How much of a body a blast at `from` reaches (`G_GetDamageScale`'s
/// five traces): its middle, head and feet and either side, a third each,
/// at most whole.
pub fn exposure(spatial: &SpatialQuery, from: Vec3, feet: Vec3, eye: Vec3) -> f32 {
    let centre = feet + Vec3::Y * u(32.0);
    let side = (centre - from).with_y(0.0).normalize_or(Vec3::X).cross(Vec3::Y) * u(15.0);
    let filter = collision::sight_filter();
    let clear = |to: Vec3| {
        let d = to - from;
        Dir3::new(d).ok().is_none_or(|dir| spatial.cast_ray(from, dir, d.length(), true, &filter).is_none())
    };
    let hits = [centre, eye, feet + Vec3::Y * u(8.0), centre + side, centre - side].into_iter().filter(|&p| clear(p)).count();
    (hits as f32 / 3.0).min(1.0)
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
    // C4 is thrown with aim and set off with fire; the rest fire.
    let c4 = std::env::var("COD4RW_INVENTORY").is_ok_and(|i| i == "c4_mp");
    let (throw, set_off) = if c4 { (MouseButton::Right, MouseButton::Left) } else { (MouseButton::Left, MouseButton::Right) };
    for (button, at) in [(throw, 7.0), (set_off, 9.0)] {
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

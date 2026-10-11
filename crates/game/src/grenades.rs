//! Grenades: CoD4's frag, thrown with G (held to cook), and the class's
//! special grenade on 4: a flashbang, a stun (concussion) grenade or smoke.
//! Any pawn throws through its [`GrenadeInput`], aimed along its view,
//! which is how bots will too.
//!
//! A throw follows CoD4's offhand states: the gun in hand goes down
//! (its `quickDropTime`), the pin comes out (the grenade's
//! `iHoldFireTime`, with its pullpin animation), the grenade is held for as
//! long as the button is, then thrown (`iFireTime`; it leaves the hand
//! `iFireDelay` in) and the gun comes back up (`quickRaiseTime`). A frag's
//! fuse (`fuseTime`, 3.5 s) starts with the pin: held too long, it goes off
//! in hand; whoever dies holding a live one drops it. It flies at
//! `iProjectileSpeed` along the view plus `iProjectileSpeedUp` upward, falls
//! with CoD4's gravity and bounces off the world by the surface's
//! `parallelBounce`/`perpendicularBounce`. It hurts everyone in sight within
//! `iExplosionRadius`, from `iExplosionInnerDamage` at the centre to
//! `iExplosionOuterDamage` at the edge, through [`Damage`] (so teammates are
//! spared and the thrower isn't).
//!
//! The specials go off a fuse after they're thrown. A flashbang
//! (`_flashgrenades.gsc`) blinds and deafens whoever sees it, for longer the
//! nearer they are (inside `iExplosionRadiusMin`, fully) and the more
//! directly they look at it; a stun grenade (`Callback_PlayerDamage`) slows
//! moving and turning for 2 s plus up to 4 s more the nearer they are. Both
//! spare the thrower's teammates (not the thrower), as CoD4 does with
//! friendly fire off, and mark who they caught ([`Flashed`], [`Stunned`]).
//! Smoke pours out of its canister for a while ([`SmokeCloud`]).
//!
//! Martyrdom (`specialty_grenadepulldeath`) drops a live frag where its
//! owner dies, on `frag_grenade_short_mp`'s shorter fuse.

use crate::collision::{self, SURFACE_NAMES, Surfaces};
use crate::combat::{Damage, Dead, HitLocation, Pawn};
use crate::content::Content;
use crate::fx::{Anchor, Effects, FxLayer, Frame};
use crate::loadout::Loadout;
use crate::models::{Skeleton, SpawnModel, spawn_model};
use crate::movement::{Mover, ViewAngles};
use crate::player::LocalPlayer;
use crate::units::u;
use crate::weapons::{WeaponDef, WeaponState};
use avian3d::prelude::*;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;

use iw3::zone::AssetType;
use std::collections::HashMap;

pub struct GrenadesPlugin;

impl Plugin for GrenadesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GrenadeDefs>()
            .add_message::<WentOff>()
            .init_resource::<Pending>()
            .init_resource::<LastView>()
            .add_systems(OnEnter(crate::state::GameState::InGame), load_defs.after(crate::world::load_map).in_set(crate::state::Setup::Content))
            .add_systems(
                Update,
                (
                    keys.in_set(crate::player::InputSet),
                    (throw, spawn_grenades).chain().after(crate::player::InputSet).before(crate::weapons::WeaponSet),
                    (stock, martyrdom, fly, explode, wear_off).chain().after(crate::weapons::WeaponSet),
                    stun_controls.after(crate::player::InputSet).before(crate::movement::MovementSet),
                    stun_controls_done.after(crate::weapons::WeaponSet),
                )
                    .run_if(crate::state::in_game),
            );
        if let Ok(dir) = std::env::var("COD4RW_GRENADETEST") {
            app.insert_resource(TestDir(dir.into())).add_systems(Update, test.after(keys).before(throw).run_if(crate::state::in_game));
        }
    }
}

/// What a pawn wants to do with its grenades this frame: hold `frag` to
/// pull a frag's pin (and cook it), let go to throw; `special` the same for
/// the class's special grenade.
#[derive(Component, Default, Clone, Copy, Debug)]
pub struct GrenadeInput {
    pub frag: bool,
    pub special: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Frag,
    Flash,
    Stun,
    Smoke,
}

const KINDS: [Kind; 4] = [Kind::Frag, Kind::Flash, Kind::Stun, Kind::Smoke];

impl Kind {
    /// For the network: its place in the list.
    pub fn index(self) -> u8 {
        KINDS.iter().position(|k| *k == self).unwrap_or(0) as u8
    }
    pub fn from_index(i: u8) -> Option<Kind> {
        KINDS.get(i as usize).copied()
    }
}

impl Pending {
    /// A grenade into the world next frame (an online guest's copy of the
    /// host's, [`crate::netplay`]).
    pub(crate) fn queue_throw(&mut self, kind: Kind, thrower: Entity, at: Vec3, velocity: Vec3, explode_at: f32) {
        self.throws.push((kind, thrower, at, velocity, explode_at, How::Thrown));
    }
}

impl Kind {
    /// Its weapon (`<name>_mp`), as classes name it.
    fn weapon(self) -> &'static str {
        match self {
            Kind::Frag => "frag_grenade",
            Kind::Flash => "flash_grenade",
            Kind::Stun => "concussion_grenade",
            Kind::Smoke => "smoke_grenade",
        }
    }

    /// A class's special grenade.
    pub fn special(name: &str) -> Option<Kind> {
        [Kind::Flash, Kind::Stun, Kind::Smoke].into_iter().find(|k| name.eq_ignore_ascii_case(k.weapon()))
    }

    /// The kill feed's name for it, and its icon there and on the HUD.
    pub fn display(self) -> &'static str {
        match self {
            Kind::Frag => "Frag Grenade",
            Kind::Flash => "Flashbang",
            Kind::Stun => "Stun Grenade",
            Kind::Smoke => "Smoke Grenade",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Kind::Frag => "hud_us_grenade",
            Kind::Flash => "hud_us_flashgrenade",
            Kind::Stun => "hud_us_stungrenade",
            Kind::Smoke => "hud_us_smokegrenade",
        }
    }
}

/// The kill feed icon for a [`Damage::weapon`] that's a grenade.
pub fn kill_icon(weapon: &str) -> Option<&'static str> {
    KINDS.into_iter().find(|k| k.display() == weapon).map(Kind::icon)
}

/// A grenade's weapon file, as far as throwing and exploding go.
pub struct Stats {
    /// Its viewmodel and animations.
    pub def: &'static WeaponDef,
    pub fuse: f32,
    /// The fuse starts with the pin (`bCookOffHold`), else when thrown.
    pub cook: bool,
    pub pullback: f32,
    pub release: f32,
    pub radius: f32,
    /// A flashbang's full-strength radius.
    pub radius_min: f32,
    pub inner_damage: f32,
    pub outer_damage: f32,
    pub speed: f32,
    pub speed_up: f32,
    pub model: String,
    pub effect: String,
    pub sound: String,
    /// Bounce by surface type ([`SURFACE_NAMES`]).
    pub parallel: [f32; 29],
    pub perpendicular: [f32; 29],
}

/// The grenades' weapon files, from `common_mp`, and Martyrdom's fuse.
#[derive(Resource, Default)]
pub struct GrenadeDefs(HashMap<Kind, Stats>, f32);

impl GrenadeDefs {
    pub fn get(&self, kind: Kind) -> Option<&Stats> {
        self.0.get(&kind)
    }
}

fn load_defs(mut defs: ResMut<GrenadeDefs>, content: Res<Content>) {
    defs.0.clear();
    defs.1 = content.generic(AssetType::Weapon, "frag_grenade_short_mp").map_or(2.5, |(_, w)| w.int("fuseTime") as f32 / 1000.0);
    for kind in KINDS {
        let name = format!("{}_mp", kind.weapon());
        let (Some((zi, w)), Some(def)) =
            (content.generic(AssetType::Weapon, &name), crate::loadout::bot_weapon(&content, &format!("{}:", kind.weapon())))
        else {
            warn!("grenades: no {name}");
            continue;
        };
        let zone = &content.zones[zi];
        let ms = |f: &str| w.int(f) as f32 / 1000.0;
        let asset = |f: &str| w.asset(f).map(|i| zone.get(i).name().trim_start_matches(',').to_owned());
        let sound = |f: &str| w.node(f).and_then(|n| n.node("name")).and_then(|n| n.string("soundName")).map(str::to_owned);
        let bounce = |f: &str| std::array::from_fn(|i| w.float(&format!("{f}[{i}]")));
        let stats = Stats {
            def,
            fuse: ms("fuseTime"),
            cook: w.int("bCookOffHold") != 0,
            pullback: ms("iHoldFireTime"),
            release: ms("iFireDelay"),
            radius: w.int("iExplosionRadius") as f32,
            radius_min: w.int("iExplosionRadiusMin") as f32,
            inner_damage: w.int("iExplosionInnerDamage") as f32,
            outer_damage: w.int("iExplosionOuterDamage") as f32,
            speed: w.int("iProjectileSpeed") as f32,
            speed_up: w.int("iProjectileSpeedUp") as f32,
            model: asset("projectileModel").unwrap_or_default(),
            // A frag names none: CoD4 plays its default grenade explosion.
            effect: asset("projExplosionEffect").unwrap_or_else(|| "explosions/grenadeexp_default".into()),
            sound: sound("projExplosionSound").unwrap_or_else(|| "grenade_explode_default".into()),
            parallel: bounce("parallelBounce"),
            perpendicular: bounce("perpendicularBounce"),
        };
        info!(
            "grenades: {name}: fuse {}s (cook {}), radius {}, {}..{} damage, {} up {}",
            stats.fuse, stats.cook, stats.radius, stats.inner_damage, stats.outer_damage, stats.speed, stats.speed_up
        );
        defs.0.insert(kind, stats);
    }
}

/// A pawn's grenades.
#[derive(Component, Default, Debug)]
pub struct Grenades {
    pub frags: u32,
    /// The special grenade and how many.
    pub special: Option<Kind>,
    pub specials: u32,
    /// The inputs last frame: a throw starts on a fresh press.
    held: (bool, bool),
    /// Restock at the next chance (after spawning, once the class is on).
    restock: bool,
}

/// CoD4's frags and special grenades at spawn (`iStartAmmo`), and with
/// Frag x3 or Special Grenades x3.
const FRAGS: u32 = 1;
const FRAGS_PERK: u32 = 3;
const SPECIALS: u32 = 1;
const SPECIALS_PERK: u32 = 3;
/// Without a class (bots, `--map` matches): a flashbang.
const DEFAULT_SPECIAL: Kind = Kind::Flash;

/// The parts of a throw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// The gun in hand going down.
    Drop,
    /// Pulling the pin.
    Pullback,
    /// Holding it (cooking a frag).
    Hold,
    Throw,
    /// The gun coming back up.
    Raise,
}

/// A grenade throw in progress: no firing, aiming or reloading meanwhile.
#[derive(Component, Debug)]
pub struct Offhand {
    pub kind: Kind,
    /// The grenade's weapon (its viewmodel and animations).
    pub def: &'static WeaponDef,
    pub phase: Phase,
    pub started: f32,
    pub until: f32,
    /// When a cooked one goes off.
    pub explode_at: Option<f32>,
    thrown: bool,
    /// A care package's marker, not one of the pawn's grenades.
    marker: bool,
}

/// Throw a care package's marker ([`How::Marker`]): the smoke grenade's
/// throw, without holding it, from no grenade of the pawn's. None while
/// there's no smoke grenade to throw it as.
pub fn marker_throw(defs: &GrenadeDefs, weapon: &WeaponState, now: f32) -> Option<Offhand> {
    let stats = defs.get(Kind::Smoke)?;
    Some(Offhand {
        kind: Kind::Smoke,
        def: stats.def,
        phase: Phase::Drop,
        started: now,
        until: now + weapon.def.quick_drop_time.max(0.05),
        explode_at: None,
        thrown: false,
        marker: true,
    })
}

impl Offhand {
    /// Is the grenade (rather than the gun) in hand?
    pub fn grenade_in_hand(&self) -> bool {
        matches!(self.phase, Phase::Pullback | Phase::Hold | Phase::Throw)
    }
}

/// Grenades to put in the world, and to set off, this frame.
#[derive(Resource, Default)]
pub(crate) struct Pending {
    /// Kind, thrower, where, velocity, when it goes off, how.
    throws: Vec<(Kind, Entity, Vec3, Vec3, f32, How)>,
    blasts: Vec<(Kind, Entity, Vec3, How)>,
}

/// How a grenade came to go off (for the challenges).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum How {
    Thrown,
    /// Thrown after holding it a second or more with the pin out
    /// (`_weapons.gsc`'s `isCooked`).
    Cooked,
    /// Dropped by Martyrdom.
    Martyrdom,
    /// Dropped by whoever died holding it.
    Dropped,
    /// Held until it went off.
    InHand,
    /// A care package's marker ([`crate::killstreaks::carepackage`]): a
    /// smoke canister whose smoke calls the drop in, and hides no one.
    Marker,
}

/// How long the pin must be out for a throw to count as cooked.
const COOKED: f32 = 1.0;

/// A grenade went off.
#[derive(Message, Clone, Copy, Debug)]
pub struct WentOff {
    pub kind: Kind,
    pub thrower: Entity,
    pub how: How,
}

/// A grenade in the world.
#[derive(Component)]
pub struct LiveGrenade {
    pub kind: Kind,
    pub thrower: Entity,
    pub velocity: Vec3,
    pub explode_at: f32,
    pub resting: bool,
    pub how: How,
    spin: Vec3,
    /// A smoke grenade has gone off (it lies smoking until `explode_at`).
    popped: bool,
}

/// Caught by a flashbang: blind and deaf, fading out over the last
/// [`FLASH_FADE`] seconds.
#[derive(Component, Clone, Copy, Debug)]
pub struct Flashed {
    pub until: f32,
    pub duration: f32,
}

/// Caught by a stun grenade: moving and turning slowed until `until`.
#[derive(Component, Clone, Copy, Debug)]
pub struct Stunned {
    pub until: f32,
    pub duration: f32,
}

/// Smoke from a smoke grenade: a cloud this wide, until then.
#[derive(Component, Clone, Copy, Debug)]
pub struct SmokeCloud {
    pub radius: f32,
    pub until: f32,
}

/// The longest a flashbang blinds (at the centre, looking at it:
/// `_flashgrenades.gsc`'s `percent_distance * percent_angle * 6`), and how
/// long the white takes to wear off at the end (`flashbang.shock`'s
/// `screenFlashWhiteFadeTime`).
const FLASH_MAX: f32 = 6.0;
pub const FLASH_FADE: f32 = 3.5;
/// A stun's slowing (`concussion_grenade_mp.shock`, `PM_CmdScale`): moving
/// at 0.4, the mouse at a tenth and turning at most 35°/s, easing off over
/// the last 2 s.
const STUN_MOVE: f32 = 0.4;
const STUN_TURN: f32 = 0.1;
const STUN_MAX_TURN: f32 = 35.0;
const STUN_FADE: f32 = 2.0;
/// Martyrdom's grenade (`G_PlayerDie`): from 40 units up, tossed up to 160
/// u/s each way.
const MARTYRDOM_HEIGHT: f32 = 40.0;
const MARTYRDOM_TOSS: f32 = 160.0;
/// How long smoke pours out (`smoke_grenade_11sec_mp`, then it thins) and
/// how wide it spreads, in CoD units.
const SMOKE_TIME: f32 = 15.0;
const SMOKE_RADIUS: f32 = 220.0;

impl LiveGrenade {
    /// A smoke grenade that's already smoking.
    pub fn popped(&self) -> bool {
        self.popped
    }
}

impl Flashed {
    /// How blinded (1 white, fading to 0).
    pub fn strength(&self, now: f32) -> f32 {
        ((self.until - now) / FLASH_FADE.min(self.duration).max(0.01)).clamp(0.0, 1.0)
    }
}

impl Stunned {
    /// How stunned (1, easing to 0 over the last [`STUN_FADE`] seconds).
    pub fn strength(&self, now: f32) -> f32 {
        ((self.until - now) / STUN_FADE).clamp(0.0, 1.0)
    }
}

/// G: the frag. The same gating as the other keys: only while the game has
/// the mouse.
fn keys(test: Option<Res<TestDir>>, mut players: Query<(&crate::splitscreen::PlayerInput, &mut GrenadeInput)>) {
    if test.is_some() {
        return;
    }
    for (player, mut input) in &mut players {
        let live = player.live;
        *input = GrenadeInput { frag: live && player.keys.pressed(KeyCode::KeyG), special: live && player.keys.pressed(KeyCode::Digit4) };
    }
}

/// Give every pawn its grenade input and stock, and restock them each
/// spawn: one frag (three with Frag x3) and one of the class's special
/// grenade (three with Special Grenades x3).
fn stock(
    mut commands: Commands,
    mut pawns: Query<(Entity, Option<&mut Grenades>, Option<&Loadout>), (With<Pawn>, Without<Dead>)>,
    mut respawned: RemovedComponents<Dead>,
) {
    for e in respawned.read() {
        if let Ok((_, Some(mut g), _)) = pawns.get_mut(e) {
            g.restock = true;
        }
    }
    for (e, g, loadout) in &mut pawns {
        let perk = |name: &str| loadout.is_some_and(|l| l.class.perks.iter().any(|p| p.eq_ignore_ascii_case(name)));
        let frags = if perk("specialty_fraggrenade") { FRAGS_PERK } else { FRAGS };
        let special = match loadout {
            Some(l) => l.class.special.as_deref().and_then(Kind::special),
            None => Some(DEFAULT_SPECIAL),
        };
        // Smoke doesn't come three at a time: Special Grenades x3 makes it
        // flashbangs (`_class.gsc`).
        let special = match special {
            Some(Kind::Smoke) if perk("specialty_specialgrenade") => Some(Kind::Flash),
            s => s,
        };
        let specials = if special.is_none() { 0 } else if perk("specialty_specialgrenade") { SPECIALS_PERK } else { SPECIALS };
        match g {
            None => {
                commands.entity(e).insert((Grenades { frags, special, specials, ..default() }, GrenadeInput::default()));
            }
            Some(mut g) if g.restock => {
                (g.frags, g.special, g.specials) = (frags, special, specials);
                g.restock = false;
            }
            Some(_) => {}
        }
    }
}

/// Martyrdom: a live frag where its owner dies.
fn martyrdom(
    time: Res<Time>,
    defs: Res<GrenadeDefs>,
    died: Query<(Entity, &Transform, &Loadout), Added<Dead>>,
    mut pending: ResMut<Pending>,
) {
    for (e, tf, loadout) in &died {
        if loadout.class.perks.iter().any(|p| p.eq_ignore_ascii_case("specialty_grenadepulldeath")) {
            let toss = Vec3::new(rand::random_range(-1.0..1.0), rand::random_range(-1.0..1.0), rand::random_range(-1.0..1.0)) * u(MARTYRDOM_TOSS);
            pending.throws.push((Kind::Frag, e, tf.translation + Vec3::Y * u(MARTYRDOM_HEIGHT), toss, time.elapsed_secs() + defs.1, How::Martyrdom));
        }
    }
}

/// Start, run and finish throws, and drop the live grenade of whoever dies
/// holding one.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn throw(
    mut commands: Commands,
    time: Res<Time>,
    defs: Res<GrenadeDefs>,
    spatial: SpatialQuery,
    mut pawns: Query<(
        Entity,
        &Transform,
        &Mover,
        &ViewAngles,
        &GrenadeInput,
        &mut Grenades,
        &mut WeaponState,
        Option<&mut Offhand>,
        Option<&Loadout>,
        Has<Dead>,
    )>,
    mut pending: ResMut<Pending>,
) {
    let now = time.elapsed_secs();
    for (e, tf, mover, view, input, mut g, mut weapon, offhand, loadout, dead) in &mut pawns {
        let fresh = (input.frag && !g.held.0, input.special && !g.held.1);
        g.held = (input.frag, input.special);
        let Some(mut o) = offhand else {
            let switching = loadout.is_some_and(|l| l.switching.is_some());
            if dead || switching {
                continue;
            }
            let kind = match fresh {
                (true, _) if g.frags > 0 => Kind::Frag,
                (_, true) if g.specials > 0 => match g.special {
                    Some(k) => k,
                    None => continue,
                },
                _ => continue,
            };
            let Some(stats) = defs.get(kind) else { continue };
            if kind == Kind::Frag {
                g.frags -= 1;
            } else {
                g.specials -= 1;
            }
            weapon.reload_until = None;
            let drop = weapon.def.quick_drop_time.max(0.05);
            commands.entity(e).insert(Offhand {
                kind,
                def: stats.def,
                phase: Phase::Drop,
                started: now,
                until: now + drop,
                explode_at: None,
                thrown: false,
                marker: false,
            });
            continue;
        };
        let Some(stats) = defs.get(o.kind) else {
            commands.entity(e).remove::<Offhand>();
            continue;
        };
        let hand = mover.eye(tf.translation) - Vec3::Y * u(8.0);
        // Dying with it in hand drops it, live if the pin is out.
        if dead {
            if let (Some(at), false) = (o.explode_at, o.thrown) {
                pending.throws.push((o.kind, e, hand, DROPPED, at, How::Dropped));
            }
            commands.entity(e).remove::<Offhand>();
            continue;
        }
        // Held too long.
        if let (Some(at), false) = (o.explode_at, o.thrown) {
            if now >= at {
                pending.blasts.push((o.kind, e, hand, How::InHand));
                o.thrown = true;
                set_phase(&mut o, Phase::Raise, now, weapon.def.quick_raise_time);
                continue;
            }
        }
        if o.phase == Phase::Throw && !o.thrown && now >= o.started + stats.release {
            o.thrown = true;
            let forward = view.rotation() * Vec3::NEG_Z;
            let eye = mover.eye(tf.translation);
            // Out in front of the eye, short of any wall there.
            let reach = Dir3::new(forward)
                .ok()
                .and_then(|d| spatial.cast_ray(eye, d, u(16.0), true, &collision::sight_filter()))
                .map_or(u(16.0), |h| (h.distance - u(4.0)).max(0.0));
            // And the thrower's own way along the throw (`G_GrenadeLaunch`).
            let carried = forward * mover.velocity.dot(forward).max(0.0);
            let velocity = forward * u(stats.speed) + Vec3::Y * u(stats.speed_up) + carried;
            let explode_at = o.explode_at.unwrap_or(now + stats.fuse);
            let how = if o.marker {
                How::Marker
            } else if explode_at - now <= stats.fuse - COOKED {
                How::Cooked
            } else {
                How::Thrown
            };
            pending.throws.push((o.kind, e, eye + forward * reach, velocity, explode_at, how));
        }
        if now < o.until {
            continue;
        }
        match o.phase {
            Phase::Drop => {
                if stats.cook {
                    o.explode_at = Some(now + stats.fuse);
                }
                set_phase(&mut o, Phase::Pullback, now, stats.pullback);
            }
            Phase::Pullback | Phase::Hold if !o.marker && ((input.frag && o.kind == Kind::Frag) || (input.special && o.kind != Kind::Frag)) => {
                o.phase = Phase::Hold;
                o.until = now;
            }
            Phase::Pullback | Phase::Hold => set_phase(&mut o, Phase::Throw, now, stats.def.fire_time.max(stats.release)),
            Phase::Throw => set_phase(&mut o, Phase::Raise, now, weapon.def.quick_raise_time),
            Phase::Raise => {
                commands.entity(e).remove::<Offhand>();
            }
        }
    }
}

fn set_phase(o: &mut Offhand, phase: Phase, now: f32, length: f32) {
    o.phase = phase;
    o.started = now;
    o.until = now + length.max(0.05);
}

/// Thrown (and dropped) grenades, with their models.
fn spawn_grenades(
    mut commands: Commands,
    mut pending: ResMut<Pending>,
    defs: Res<GrenadeDefs>,
    mut content: ResMut<Content>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut bindposes: ResMut<Assets<SkinnedMeshInverseBindposes>>,
) {
    for (kind, thrower, at, velocity, explode_at, how) in std::mem::take(&mut pending.throws) {
        let still = velocity == Vec3::ZERO;
        let spin = if still { Vec3::ZERO } else { Vec3::new(9.0, 2.0, 5.0) };
        let grenade = commands
            .spawn((
                Name::new("grenade"),
                LiveGrenade { kind, thrower, velocity, explode_at, resting: still, how, spin, popped: false },
                Transform::from_translation(at),
                Visibility::default(),
            ))
            .id();
        let model = defs.get(kind).and_then(|s| content.model(&s.model, &mut meshes, &mut materials, &mut images, &mut bindposes));
        if let Some(m) = model {
            spawn_model(&mut commands, &mut Skeleton::default(), SpawnModel { model: &m, owner: grenade, attach_to: None, layers: None, shadows: true });
        }
    }
}

/// A dropped grenade's start: falling, not thrown.
const DROPPED: Vec3 = Vec3::new(0.0, -0.1, 0.0);

/// CoD4's gravity, `g_gravity`.
const GRAVITY: f32 = 800.0;
/// A grenade's size, for touching walls.
const RADIUS: f32 = 2.0;
/// Bouncing slower than this off the floor, it settles (`G_BounceMissile`).
const SETTLE_SPEED: f32 = 20.0;
/// Smoke pours out once the canister lies still, trying this long
/// (`G_RunMissile`'s 60 s of 50 ms retries).
const SMOKE_WAIT: f32 = 60.0;

/// Fly, bounce and settle grenades, and set them off.
#[allow(clippy::too_many_arguments)]
fn fly(
    mut commands: Commands,
    time: Res<Time>,
    spatial: SpatialQuery,
    surfaces: Query<&Surfaces>,
    defs: Res<GrenadeDefs>,
    mut grenades: Query<(Entity, &mut LiveGrenade, &mut Transform)>,
    mut sfx: ResMut<crate::audio::Sfx>,
    bank: Option<Res<crate::audio::Bank>>,
    mut pending: ResMut<Pending>,
) {
    let (now, dt) = (time.elapsed_secs(), time.delta_secs());
    for (e, mut g, mut tf) in &mut grenades {
        if now >= g.explode_at {
            // Smoke pours out of the canister, which stays, once it's down.
            if g.kind == Kind::Smoke && !g.popped && !g.resting && now < g.explode_at + SMOKE_WAIT {
                // (Still rolling: it waits.)
            } else if g.kind == Kind::Smoke && !g.popped {
                pending.blasts.push((g.kind, g.thrower, tf.translation, g.how));
                g.popped = true;
                g.explode_at = now + SMOKE_TIME;
            } else {
                if !g.popped {
                    pending.blasts.push((g.kind, g.thrower, tf.translation, g.how));
                }
                commands.entity(e).despawn();
                continue;
            }
        }
        if g.resting {
            continue;
        }
        let Some(stats) = defs.get(g.kind) else { continue };
        g.velocity.y -= u(GRAVITY) * dt;
        let step = g.velocity * dt;
        let spin = g.spin * dt;
        tf.rotation = Quat::from_euler(EulerRot::XYZ, spin.x, spin.y, spin.z) * tf.rotation;
        let Ok(dir) = Dir3::new(step) else { continue };
        let Some(hit) = spatial.cast_ray(tf.translation, dir, step.length() + u(RADIUS), true, &collision::sight_filter()) else {
            tf.translation += step;
            continue;
        };
        // Bounce: the part along the surface kept by `parallelBounce`, the
        // part into it reflected and kept by `perpendicularBounce`.
        let n = hit.normal;
        let name = surfaces.get(hit.entity).map_or("default", |s| s.facing(n));
        let surface = SURFACE_NAMES.iter().position(|s| *s == name).unwrap_or(0);
        let into = g.velocity.dot(n);
        let along = g.velocity - n * into;
        let speed = g.velocity.length();
        g.velocity = along * stats.parallel[surface] - n * into * stats.perpendicular[surface];
        tf.translation += dir * (hit.distance - u(RADIUS)).max(0.0) + n * u(0.5);
        if speed > u(60.0) {
            let alias = format!("grenade_bounce_{name}");
            let alias = if bank.as_ref().is_some_and(|b| b.has(&alias)) { alias } else { "grenade_bounce_default".into() };
            sfx.play(alias, Some(tf.translation));
        }
        if n.y > 0.7 && g.velocity.length() < u(SETTLE_SPEED) {
            g.resting = true;
            g.velocity = Vec3::ZERO;
            g.spin = Vec3::ZERO;
        }
    }
}

/// Set grenades off: a frag hurts everyone in sight within its radius
/// (from the inner to the outer damage), a flashbang blinds and a stun
/// grenade slows whoever it catches, smoke pours out; each with its effect
/// and sound.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn explode(
    mut commands: Commands,
    time: Res<Time>,
    mut pending: ResMut<Pending>,
    defs: Res<GrenadeDefs>,
    spatial: SpatialQuery,
    pawns: Query<(Entity, &Transform, &Mover, &ViewAngles, &Pawn, Option<&Flashed>), Without<Dead>>,
    loadouts: Query<&Loadout>,
    spawned: Query<&crate::combat::Spawned>,
    mut damage: MessageWriter<Damage>,
    mut exploded: MessageWriter<crate::explosives::Exploded>,
    mut went_off: MessageWriter<WentOff>,
    mut effects: ResMut<Effects>,
    mut sfx: ResMut<crate::audio::Sfx>,
) {
    let now = time.elapsed_secs();
    for (kind, thrower, at, how) in std::mem::take(&mut pending.blasts) {
        went_off.write(WentOff { kind, thrower, how });
        let Some(stats) = defs.get(kind) else { continue };
        let from = at + Vec3::Y * u(4.0);
        // In sight of the blast.
        let seen = |to: Vec3| {
            let d = to - from;
            Dir3::new(d).ok().is_none_or(|dir| spatial.cast_ray(from, dir, d.length(), true, &collision::sight_filter()).is_none())
        };
        let thrower_pawn = pawns.get(thrower).ok().map(|p| p.4);
        let mut caught = Vec::new();
        for (target, tf, mover, view, pawn, flashed) in &pawns {
            let (centre, eye) = (tf.translation + Vec3::Y * u(32.0), mover.eye(tf.translation));
            // From the blast to the feet (the player's origin).
            let dist = (tf.translation - at).length() / crate::units::INCH;
            if dist > stats.radius {
                continue;
            }
            // The thrower's teammates are spared (the thrower isn't).
            let teammate = target != thrower && thrower_pawn.is_some_and(|t| !crate::combat::hostile(t, pawn));
            match kind {
                Kind::Frag | Kind::Stun if !(seen(centre) || seen(eye)) => {}
                // Not a player just spawned near it.
                Kind::Frag if crate::combat::spawn_protected(spawned.get(target).ok(), at, now) => {}
                Kind::Frag => {
                    // Partly behind cover, partly hurt.
                    let cover = crate::explosives::exposure(&spatial, from, tf.translation, eye);
                    let boom = crate::explosives::blast_scale(loadouts.get(thrower).ok(), loadouts.get(target).ok()) * cover;
                    let amount = (stats.inner_damage + (stats.outer_damage - stats.inner_damage) * (dist / stats.radius.max(1.0))) * boom;
                    damage.write(Damage { target, attacker: Some(thrower), amount, location: HitLocation::Torso, weapon: kind.display() });
                    caught.push(format!("{} {amount:.0}", pawn.name));
                }
                Kind::Stun if !teammate => {
                    let time = 2.0 + 4.0 * (1.0 - dist / stats.radius.max(1.0));
                    commands.entity(target).insert(Stunned { until: now + time, duration: time });
                    caught.push(format!("{} {time:.1}s", pawn.name));
                }
                // Blinding takes seeing it: nearer, and looking at it.
                Kind::Flash if !teammate && seen(eye) => {
                    let near = 1.0 - ((dist - stats.radius_min) / (stats.radius - stats.radius_min).max(1.0)).clamp(0.0, 1.0);
                    let look = (view.rotation() * Vec3::NEG_Z).dot((at - eye).normalize_or_zero());
                    let facing = match (look + 1.0) * 0.5 {
                        a if a < 0.5 => 0.5,
                        a if a > 0.8 => 1.0,
                        a => a,
                    };
                    let time = FLASH_MAX * near * facing;
                    if time >= 0.25 && flashed.is_none_or(|f| f.until < now + time) {
                        commands.entity(target).insert(Flashed { until: now + time, duration: time });
                        caught.push(format!("{} {time:.1}s", pawn.name));
                    }
                }
                _ => {}
            }
        }
        // A marker's smoke only marks the spot.
        if how == How::Marker {
            effects.play(&stats.effect, Anchor::Fixed(Frame::facing(at, Vec3::Y, 0.0)), FxLayer::World);
            sfx.play(stats.sound.clone(), Some(at));
            continue;
        }
        if kind == Kind::Smoke {
            commands.spawn((Name::new("smoke"), SmokeCloud { radius: u(SMOKE_RADIUS), until: now + SMOKE_TIME }, Transform::from_translation(at)));
        }
        debug!("grenades: {kind:?} went off at CoD {:?}, catching {caught:?}", crate::units::to_cod(at).map(f32::round));
        effects.play(&stats.effect, Anchor::Fixed(Frame::facing(at, Vec3::Y, 0.0)), FxLayer::World);
        sfx.play(stats.sound.clone(), Some(at));
        let damaging = kind == Kind::Frag;
        exploded.write(crate::explosives::Exploded {
            weapon: format!("{}_mp", kind.weapon()),
            owner: thrower,
            at,
            radius: stats.radius,
            inner: if damaging { stats.inner_damage } else { 0.0 },
            outer: if damaging { stats.outer_damage } else { 0.0 },
            effect: stats.effect.clone(),
            sound: stats.sound.clone(),
        });
    }
}

/// Flashes, stuns and smoke wear off; the player's flash muffles their
/// hearing meanwhile.
fn wear_off(
    mut commands: Commands,
    time: Res<Time>,
    flashed: Query<(Entity, &Flashed, Has<LocalPlayer>)>,
    stunned: Query<(Entity, &Stunned)>,
    smoke: Query<(Entity, &SmokeCloud)>,
    mut sfx: ResMut<crate::audio::Sfx>,
) {
    let now = time.elapsed_secs();
    let mut deaf = 0.0f32;
    for (e, f, local) in &flashed {
        if now >= f.until {
            commands.entity(e).remove::<Flashed>();
        } else if local {
            deaf = f.strength(now) * 0.9;
        }
    }
    sfx.deafness = deaf;
    for (e, s) in &stunned {
        if now >= s.until {
            commands.entity(e).remove::<Stunned>();
        }
    }
    for (e, s) in &smoke {
        if now >= s.until {
            commands.entity(e).despawn();
        }
    }
}

/// A stunned player moves slower and turns slower: their moves are scaled
/// and their turning held back from where it was at the end of last frame.
fn stun_controls(
    time: Res<Time>,
    mut players: Query<(Entity, &mut crate::movement::MoveInput, &mut ViewAngles, Option<&Stunned>), With<crate::splitscreen::LocalSlot>>,
    last: Res<LastView>,
) {
    for (e, mut mv, mut view, stunned) in &mut players {
        let Some(s) = stunned.map(|s| s.strength(time.elapsed_secs())).filter(|s| *s > 0.0) else { continue };
        mv.speed_scale *= 1.0 - (1.0 - STUN_MOVE) * s;
        if let Some(&(yaw, pitch)) = last.0.get(&e) {
            // The mouse at a tenth, and no faster than 35°/s.
            let turn = 1.0 - (1.0 - STUN_TURN) * s;
            let most = (STUN_MAX_TURN / s).to_radians() * time.delta_secs();
            view.yaw = yaw + ((view.yaw - yaw) * turn).clamp(-most, most);
            view.pitch = pitch + ((view.pitch - pitch) * turn).clamp(-most, most);
        }
    }
}

/// Each local player's view at the end of the frame (after recoil), for
/// [`stun_controls`].
#[derive(Resource, Default)]
struct LastView(HashMap<Entity, (f32, f32)>);

fn stun_controls_done(mut last: ResMut<LastView>, players: Query<(Entity, &ViewAngles), With<crate::splitscreen::LocalSlot>>) {
    last.0 = players.iter().map(|(e, v)| (e, (v.yaw, v.pitch))).collect();
}

#[derive(Resource)]
struct TestDir(std::path::PathBuf);

/// Debug aid: with `COD4RW_GRENADETEST=<dir>`, the player cooks a frag from
/// 6 s, throws it at 6.9 s a little downwards, and a screenshot is saved at
/// each part of the throw and as it goes off; then the game exits.
fn test(
    mut commands: Commands,
    time: Res<Time>,
    dir: Res<TestDir>,
    mut player: Query<(&mut GrenadeInput, &mut ViewAngles, Option<&mut Grenades>), With<LocalPlayer>>,
    grenades: Query<&LiveGrenade>,
    mut step: Local<usize>,
    mut gone_at: Local<Option<f32>>,
    mut exit: MessageWriter<AppExit>,
) {
    use bevy::render::view::screenshot::{Screenshot, save_to_disk};
    let t = time.elapsed_secs();
    let Ok((mut input, mut view, stock)) = player.single_mut() else { return };
    // `COD4RW_GRENADETEST_SPECIAL=flash|stun|smoke`: that special grenade
    // instead.
    let special = std::env::var("COD4RW_GRENADETEST_SPECIAL").ok();
    if let (Some(kind), Some(mut g)) = (special.as_deref().map(|k| match k {
        "stun" => Kind::Stun,
        "smoke" => Kind::Smoke,
        _ => Kind::Flash,
    }), stock) {
        if t < 5.9 {
            (g.special, g.specials) = (Some(kind), 1);
        }
    }
    let special = special.is_some();
    let held = (6.0..6.9).contains(&t);
    (input.frag, input.special) = (held && !special, held && special);
    // `COD4RW_GRENADETEST_PITCH=<radians>`: aimed that far up (down
    // negative) instead.
    if (5.5..6.9).contains(&t) {
        view.pitch = std::env::var("COD4RW_GRENADETEST_PITCH").ok().and_then(|p| p.parse().ok()).unwrap_or(-0.25);
    }
    // Drop, pin, cooking, thrown, flying; then just after it goes off.
    const SHOTS: [(f32, &str); 6] = [(5.8, "ready"), (6.12, "drop"), (6.5, "pin"), (6.85, "cook"), (7.05, "throw"), (7.6, "raise")];
    let n = *step;
    let mut shot = |name: &str| {
        std::fs::create_dir_all(&dir.0).ok();
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(dir.0.join(format!("grenade_{n}_{name}.png"))));
        *step += 1;
    };
    if let Some(&(at, name)) = SHOTS.get(n) {
        if t >= at {
            shot(name);
        }
        return;
    }
    // Gone off: none left, or smoke pouring out (shown a while in).
    let smoking = grenades.iter().any(|g| g.popped);
    if t > 8.0 && (grenades.is_empty() || smoking) {
        let since = *gone_at.get_or_insert(t);
        let wait = if smoking { 3.0 } else { 0.12 };
        if n == SHOTS.len() && t - since > wait {
            shot("blast");
        } else if t - since > wait + 1.5 {
            if n == SHOTS.len() + 1 {
                shot("after");
            } else {
                exit.write(AppExit::Success);
            }
        }
    }
}

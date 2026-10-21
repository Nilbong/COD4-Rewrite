//! Live hands: the first-person hands working the gun, on top of CoD4's
//! viewmodel animations (which still play, untouched: reloads, raises and
//! the rest).
//!
//! Each frame, after the animation has posed the viewmodel:
//! - the gun (`tag_weapon`, which the hands' rig carries apart from the
//!   arms) is moved against the hands by springs: it climbs and twists
//!   with each shot, by the weapon's class and weight (a pistol snaps, a
//!   machine gun or sniper rifle heaves), and the hands haul it back down;
//! - both arms are bent (two-bone IK, shoulder and elbow) so each wrist
//!   stays on its grip, where the animation put it on the gun this frame:
//!   the hands never leave the gun however it moves;
//! - the wrists absorb the kick and twitch a little all the time
//!   (readjusting), more when hurt or just out of a close fight;
//! - the fingers curl and open by springs: the support hand clamps with
//!   each shot, the trigger finger rests on the frame between bursts.
//!
//! Fidgets on timers that never repeat exactly, and when something
//! prompts one (a reload done, a sprint over, a long burst, standing
//! still): the support hand slides along the handguard and regrips, the
//! trigger finger comes off the trigger and back, the thumbs shift, the gun
//! rolls a touch and is recentred.
//!
//! Weight in the transitions (the whole gun, added beside
//! [`super::feel`]'s): raising and aiming in lock in with an overshoot, a
//! hard landing braces the hands and dips the gun, coming out of a sprint
//! settles, a wall close ahead pulls the gun back and tilts it instead of
//! sinking it into the wall, and after a sprint the breathing heaves, then
//! calms.
//!
//! Live tuned: `livehands.*` in `tuning.txt`. Switch: Options > Game > Live
//! Hands (`COD4RW_FP_HANDS=0|1` for test runs). Test aid:
//! `COD4RW_LIVEHANDS_AB=<dir>` (see [`ab_test`]).

use crate::models::Skeleton;
use crate::movement::{Mover, ViewAngles};
use crate::units::u;
use crate::weapons::WeaponState;
use avian3d::prelude::SpatialQuery;
use bevy::math::Affine3A;
use bevy::prelude::*;

pub(super) fn build(app: &mut App) {
    app.add_systems(
        PostUpdate,
        (attach, update.before(crate::viewmodel::weapon_angles), pose.after(crate::viewmodel::weapon_angles))
            .chain()
            .after(crate::models::animate_skeletons)
            .before(bevy::transform::TransformSystems::Propagate)
            .run_if(crate::state::in_game),
    );
    if let Ok(dir) = std::env::var("COD4RW_LIVEHANDS_AB") {
        app.insert_resource(AbDir(dir.into()))
            .add_systems(PreUpdate, ab_test.after(bevy::input::InputSystems).run_if(crate::state::in_game));
    }
}

fn knob(name: &str, default: f32) -> f32 {
    crate::tune::get(&format!("livehands.{name}"), default)
}

/// The fingers, each a chain of joints: `j_<name>_<side>_<n>`.
const FINGERS: [(&str, [u8; 3]); 5] = [("index", [1, 2, 3]), ("mid", [1, 2, 3]), ("ring", [1, 2, 3]), ("pinky", [1, 2, 3]), ("thumb", [1, 2, 3])];
const INDEX: usize = 0;
const THUMB: usize = 4;
/// Sides: the trigger hand (right) and the support hand (left).
const RIGHT: usize = 0;
const LEFT: usize = 1;
const SIDES: [&str; 2] = ["ri", "le"];

/// What a local player's live hands add this frame: the whole gun (CoD
/// degrees: pitch down, yaw left, roll right side down; inches: forward,
/// left, up), for [`crate::viewmodel::weapon_angles`], and the rest for
/// [`pose`].
#[derive(Component, Default)]
pub struct LiveHands {
    pub gun_angles: Vec3,
    pub gun_shift: Vec3,
    s: State,
}

/// A value on a spring, and its speed.
#[derive(Default, Clone, Copy)]
struct Spring3 {
    x: Vec3,
    v: Vec3,
}

impl Spring3 {
    fn step(&mut self, target: Vec3, omega: f32, zeta: f32, dt: f32) {
        let k = omega * omega;
        let c = 2.0 * zeta * omega;
        let steps = ((dt / 0.004).ceil() as usize).clamp(1, 16);
        let h = dt / steps as f32;
        for _ in 0..steps {
            self.v += (-(self.x - target) * k - self.v * c) * h;
            self.x += self.v * h;
        }
    }
}

#[derive(Default, Clone, Copy)]
struct Spring1 {
    x: f32,
    v: f32,
}

impl Spring1 {
    fn step(&mut self, target: f32, omega: f32, zeta: f32, dt: f32) {
        let mut s = Spring3 { x: Vec3::X * self.x, v: Vec3::X * self.v };
        s.step(Vec3::X * target, omega, zeta, dt);
        self.x = s.x.x;
        self.v = s.v.x;
    }
}

/// A fidget under way: what, and until when.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Fidget {
    /// The support hand slides along the handguard (inches) and regrips.
    Slide(f32),
    /// The trigger finger off the trigger, along the frame.
    TriggerOff,
    /// A thumb shifts (which hand, how far).
    Thumb(usize, f32),
    /// The gun rolled a touch, then recentred.
    Roll(f32),
    /// Both hands regrip: fingers open and close.
    Regrip,
}

#[derive(Default)]
struct State {
    /// The gun against the hands: turned (pitch down, yaw left, roll) and
    /// moved (forward, left, up).
    rel_rot: Spring3,
    rel_pos: Spring3,
    /// The whole gun.
    rot: Spring3,
    pos: Spring3,
    /// Each wrist's turn (pitch, yaw, roll degrees).
    wrist: [Spring3; 2],
    /// Each finger's curl (radians, by side and finger), and the support
    /// hand's slide along the gun (inches).
    curl: [[Spring1; 5]; 2],
    slide: Spring1,
    /// The fidget going (and when it started, when it ends), when the next
    /// is due, and the kind of the last (never the same twice running).
    fidget: Option<(Fidget, f32)>,
    fidget_from: f32,
    next_fidget: f32,
    last_kind: u8,
    /// Where the support hand rests along the gun now (inches): moved by
    /// a slide fidget and left there, till a reload or a raise.
    rest: f32,
    /// The trigger finger: the trigger held last frame.
    trigger_held: bool,
    /// The viewmodel animation's notes read so far (which animation, how
    /// far), for the reload's beats.
    last_note: Option<(usize, f32)>,
    /// When it left the ground (for the air pose).
    left_ground: f32,
    /// The burst's climb (slow) beside the per-shot punch (`rel_rot`, fast),
    /// and the sideways drift.
    climb: Spring3,
    drift: f32,
    /// The wall in front: how much the gun is tucked (0..1), held until; and
    /// the gun's reach from the eye (inches, measured in [`pose`]).
    tuck: Spring1,
    tucked_until: f32,
    wall_since: Option<f32>,
    reach: f32,
    /// What happened lately: shots fired by which gun (a switch brings
    /// another's count, which isn't new shots).
    last_shots: Option<(usize, u32)>,
    burst: u32,
    last_shot: f32,
    was_reloading: bool,
    was_sprinting: bool,
    was_on_ground: bool,
    last_vy: f32,
    last_ads: f32,
    ads_was: f32,
    was_raising: bool,
    still_since: f32,
    fatigue: f32,
    breath: f32,
    tremor: Vec3,
    /// The active idle: a slow drift the arms can't quite hold, caught by a
    /// quick correction now and then (when it's strayed, or on its timer).
    idle: Vec3,
    idle_fix: Spring3,
    next_fix: f32,
    seed: u32,
    started: bool,
}

impl State {
    fn rand(&mut self) -> f32 {
        // xorshift: a different run of fidgets each life.
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        (self.seed % 10_000) as f32 / 10_000.0
    }
}

/// A gun's heft, from its own weapon file (CoD4's, World at War's, Black
/// Ops', MW2's: they all have these): how heavy it handles (its aim-in and
/// raise times, which the weapon designers made longer for heavier guns)
/// over its class's base, and how hard it kicks (its aimed kick against its
/// class's usual).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Heft {
    /// 0.5 (a light pistol) .. 1.8 (a machine gun).
    pub mass: f32,
    /// How hard a shot is (about 1 for its class).
    pub energy: f32,
    /// The muzzle's flip per shot about the hands (degrees), the pull back
    /// (inches), and the climb over a burst (degrees a shot, most).
    pub flip: f32,
    pub back: f32,
    pub climb: f32,
    pub climb_max: f32,
    /// The punch's spring (it recovers between shots) and the climb's.
    pub punch_omega: f32,
    pub punch_zeta: f32,
    pub climb_zeta: f32,
}

/// The class's name for knobs and its base numbers: (mass base, median
/// aimed kick, flip, back, climb, climb most, punch damping, climb damping).
fn class_of(def: &crate::weapons::WeaponDef) -> (&'static str, [f32; 8]) {
    if def.ads_overlay.is_some() {
        return ("sniper", [0.27, 2.0, 5.0, 1.4, 1.0, 3.0, 0.6, 0.9]);
    }
    match def.class {
        4 => ("pistol", [0.12, 1.0, 7.5, 0.15, 0.5, 2.0, 0.4, 0.8]),
        2 => ("smg", [0.16, 0.6, 1.8, 0.25, 0.25, 2.5, 0.42, 0.85]),
        3 => ("shotgun", [0.22, 3.0, 5.5, 1.3, 0.8, 3.0, 0.6, 0.9]),
        1 => ("lmg", [0.29, 0.7, 1.2, 0.5, 0.3, 2.5, 0.9, 0.92]),
        _ => ("rifle", [0.20, 0.8, 2.5, 0.35, 0.35, 3.0, 0.42, 0.88]),
    }
}

/// A gun's [`Heft`] (`livehands.kick_<class>`, `omega_<class>` scale it).
pub fn heft(def: &crate::weapons::WeaponDef) -> Heft {
    let (class, [base, median, flip, back, climb, climb_max, punch_zeta, climb_zeta]) = class_of(def);
    let mass = (0.45 * def.ads_trans_in / 0.25 + 0.35 * def.raise_time / 0.75 + base).clamp(0.5, 1.8);
    let energy = (def.ads_kick_pitch.1.max(0.01) / median).clamp(0.7, 1.4) * knob(&format!("kick_{class}"), 1.0);
    Heft {
        mass,
        energy,
        flip,
        back,
        climb,
        climb_max,
        punch_omega: knob("punch_omega", 50.0) * knob(&format!("omega_{class}"), 1.0),
        punch_zeta,
        climb_zeta,
    }
}

/// How much of the continuous shake (the idle drift, the tremor, the
/// fidgets' roll, the breathing heave) there is: the user found it too
/// much at full, and asked for 15% (`livehands.shake`).
fn shake() -> f32 {
    knob("shake", 0.15)
}

/// How much of CoD4's slow idle sway is kept at the hip with live hands.
pub fn cod_idle() -> f32 {
    knob("cod_idle", 0.35)
}

/// Where the gun's weight turns it about, in the view (CoD inches:
/// forward, left, up): near the right hand and the stock.
pub fn pivot() -> Vec3 {
    Vec3::new(knob("pivot_fwd", 10.0), knob("pivot_left", -6.0), knob("pivot_up", -7.0))
}

fn attach(mut commands: Commands, players: Query<Entity, (With<crate::splitscreen::LocalSlot>, Without<LiveHands>)>) {
    for e in &players {
        commands.entity(e).insert(LiveHands::default());
    }
}

/// The springs, timers and what prompts them.
#[allow(clippy::type_complexity)]
fn update(
    time: Res<Time>,
    spatial: SpatialQuery,
    mut players: Query<(
        Entity,
        &Transform,
        &Mover,
        &ViewAngles,
        &WeaponState,
        Option<&crate::loadout::Loadout>,
        Option<&crate::combat::Health>,
        &mut LiveHands,
        (Option<&crate::splitscreen::LocalSlot>, Has<crate::grenades::Offhand>, Has<crate::melee::Melee>),
        Option<&crate::weapons::WeaponInput>,
    )>,
    viewmodels: Query<(&crate::viewmodel::ViewModelSlot, &crate::viewmodel::WeaponMotion, &crate::models::AnimPlayer)>,
    mut damage: MessageReader<crate::combat::Damage>,
    attackers: Query<&Transform, Without<LiveHands>>,
) {
    let hits: Vec<(Entity, Option<Entity>, f32)> = damage.read().map(|d| (d.target, d.attacker, d.amount)).collect();
    let dt = time.delta_secs().min(0.05);
    let now = time.elapsed_secs();
    if dt <= 0.0 {
        return;
    }
    let on = super::live_hands();
    for (e, tf, mover, view, w, loadout, health, mut live, (slot, throwing, knifing), input) in &mut players {
        if !on {
            *live = LiveHands::default();
            continue;
        }
        let s = &mut live.s;
        if !s.started {
            s.started = true;
            // (`COD4RW_LIVEHANDS_SEED` for test runs: the same each time.)
            s.seed = match std::env::var("COD4RW_LIVEHANDS_SEED").ok().and_then(|v| v.parse::<u32>().ok()) {
                Some(seed) => seed.max(1),
                None => 0x9e37_79b9 ^ e.to_bits() as u32 ^ (now * 1000.0) as u32,
            };
            s.reach = 24.0;
            s.next_fidget = now + 3.0;
            s.last_shot = -100.0;
        }
        let ads = w.ads.max(w.laser_aim).clamp(0.0, 1.0);
        // Aimed in, a quarter of it: the sights stay put.
        let calm = 1.0 - 0.75 * ads;
        let h = heft(w.def);
        // Doing something else with the gun (or without it): no fidgets.
        let inspecting = slot.is_some_and(|sl| viewmodels.iter().any(|(v, m, _)| v.0 == sl.0 && m.inspecting()));
        let switching = loadout.is_some_and(|l| l.switching.is_some());
        let busy = throwing || knifing || inspecting || switching || mover.sprinting;
        // Nervous: hurt, or shot at lately.
        let hurt = health.map_or(0.0, |h| 1.0 - (h.current / crate::combat::max_health()).clamp(0.0, 1.0));
        let shaken = health.map_or(0.0, |h| (1.0 - (now - h.last_damage) / 6.0).clamp(0.0, 1.0));
        let nerves = 1.0 + knob("nerves", 1.0) * (hurt * 0.8 + shaken);
        let speed = Vec2::new(mover.velocity.x, mover.velocity.z).length() / crate::units::INCH;

        // --- Firing: each shot punches the gun back and flips its muzzle up
        // about the hands, on a fast spring that recovers before the next;
        // over a burst it climbs (slowly, a stiffer damping for a heavier
        // gun) and drifts sideways, the way its own kick leans; the wrists
        // give; the support hand tightens.
        let gun = w.def as *const crate::weapons::WeaponDef as usize;
        let shots = w.shots_fired_total;
        let fired = match s.last_shots {
            Some((g, l)) if g == gun => shots.saturating_sub(l).min(3),
            _ => 0,
        };
        s.last_shots = Some((gun, shots));
        let lean_side = (w.def.ads_kick_yaw.0 + w.def.ads_kick_yaw.1).signum();
        for _ in 0..fired {
            let vary = 1.0 + (s.rand() - 0.5) * knob("kick_vary", 0.4);
            let k = h.energy / h.mass.sqrt() * vary * calm * knob("kick", 1.0);
            // The punch: a speed that peaks near `flip`/`back` (a spring at
            // omega peaks near v/omega).
            let omega = h.punch_omega;
            s.rel_rot.v += Vec3::new(-h.flip * omega, 0.0, 0.0) * k;
            s.rel_pos.v += Vec3::new(-h.back * knob("punch_back", 1.0) * omega, 0.0, 0.05 * omega) * k;
            // Correlated drift: mostly the last shot's, a little new, leaning
            // the gun's way.
            let corr = knob("yaw_corr", 0.8);
            s.drift = corr * s.drift + (1.0 - corr) * ((s.rand() - 0.5) * 2.0 + 0.3 * lean_side);
            s.rel_rot.v += Vec3::new(0.0, h.flip * 0.35 * omega * s.drift, h.flip * 0.25 * omega * s.drift) * k * knob("twist", 1.0);
            s.wrist[RIGHT].v += Vec3::new(20.0 * k, 0.0, 6.0 * k * s.drift);
            s.wrist[LEFT].v += Vec3::new(14.0 * k, -5.0 * k * s.drift, 0.0);
            s.curl[LEFT].iter_mut().for_each(|c| c.v += 0.6 * knob("grip", 1.0));
            s.burst += 1;
            s.last_shot = now;
        }
        let firing = now - s.last_shot < 0.15;
        // A burst over: a small regrip (a long one: the gun recentred).
        if s.burst > 0 && now - s.last_shot > 0.25 {
            if s.burst >= 8 {
                s.fidget = Some((Fidget::Roll((s.rand() - 0.5) * 6.0), now + 0.5));
            } else if s.burst >= 3 {
                s.fidget = Some((Fidget::Regrip, now + 0.35));
            }
            s.burst = 0;
        }

        // --- What prompts a fidget: a reload done, a sprint over,
        // standing still a while; else a timer (sooner when nervous).
        let reloading = w.reloading();
        let sprinting = mover.sprinting;
        if speed > 20.0 || firing || reloading {
            s.still_since = now;
        }
        let prompted = (s.was_reloading && !reloading) || (s.was_sprinting && !sprinting);
        let still = now - s.still_since > 3.0;
        // How often: gamma-like intervals (three draws' sum), a mean of 8 s
        // standing still and 12 s moving, sooner when nervous.
        let mean = knob("fidget_mean", 8.0) * if still { 1.0 } else { 1.5 } / (knob("fidget_rate", 1.0) * nerves.sqrt()).max(0.1);
        if busy {
            s.fidget = None;
        }
        // Back to the usual hold after a reload or a raise.
        if (s.was_reloading && !reloading) || switching {
            s.rest = 0.0;
        }
        let prompted = prompted && s.rand() < knob("fidget_prompt_p", 0.6);
        if s.fidget.is_none() && !firing && !reloading && !busy && (prompted || now >= s.next_fidget) {
            // What this gun's hands can do: a pistol's support hand cups the
            // other (no sliding along); a gun with a foregrip holds that.
            let pistol = w.def.class == 4;
            let grip_gun = w.def.name.contains("grip");
            let mut kind = 0u8;
            for _ in 0..4 {
                let r = s.rand();
                kind = if r < 0.3 { 0 } else if r < 0.5 { 1 } else if r < 0.68 { 2 } else if r < 0.85 { 3 } else { 4 };
                if (kind == 0 && (pistol || grip_gun)) || kind == s.last_kind {
                    continue;
                }
                break;
            }
            if kind == 0 && (pistol || grip_gun) {
                kind = 3;
            }
            s.last_kind = kind;
            let fidget = match kind {
                // The support hand to another spot along the handguard (and it
                // stays there): one of a few, a shotgun's pump hand barely.
                0 => {
                    let spots = [-0.8f32, 0.0, 0.6, 1.2];
                    let i = (s.rand() * spots.len() as f32) as usize % spots.len();
                    let reach = if w.def.class == 3 { 0.4 } else { knob("slide", 1.2) };
                    s.rest = spots[i].clamp(-reach, reach);
                    Fidget::Slide(s.rest)
                }
                1 => Fidget::TriggerOff,
                2 => Fidget::Thumb(if s.rand() < 0.5 { RIGHT } else { LEFT }, (s.rand() - 0.5) * 0.6),
                3 => Fidget::Regrip,
                _ => Fidget::Roll((s.rand() - 0.5) * if w.def.class == 1 { 6.0 } else { 4.0 } * shake()),
            };
            // Release (0.08-0.12 s), move (0.15-0.3), settle (0.08).
            let len = 0.1 + 0.15 + 0.15 * s.rand() + 0.08;
            s.fidget = Some((fidget, now + len));
            s.fidget_from = now;
            let gamma = (s.rand() + s.rand() + s.rand()) / 1.5;
            s.next_fidget = now + mean * gamma;
        }
        if s.fidget.is_some_and(|(_, until)| now >= until) {
            // Settling: a small knock as the hand takes hold.
            s.wrist[LEFT].v += Vec3::new(6.0, 0.0, 0.0);
            s.fidget = None;
        }
        s.was_reloading = reloading;

        // --- Weight in the transitions (the whole gun).
        // Aiming in: it locks in with a little overshoot.
        // The sights' arrival: a lock-in by the gun's weight over how quickly
        // it comes up (a heavy gun thumps in; a pistol snaps); going out, it
        // drops a touch.
        if s.last_ads < 0.97 && ads >= 0.97 {
            let k = h.mass / (w.def.ads_trans_in / 0.25).max(0.4) * knob("lock_in", 1.0);
            s.rot.v += Vec3::new(10.0, 0.0, -4.0) * k;
        }
        if s.last_ads > 0.97 && ads <= 0.97 {
            s.rot.v += Vec3::new(12.0 * h.mass, 0.0, 0.0) * knob("ads_out", 1.0);
        }
        s.last_ads = ads;
        // Raised: the same, heavier.
        let raising = loadout.and_then(|l| l.switching).is_some_and(|sw| sw.raising);
        if s.was_raising && !raising {
            s.rot.v += Vec3::new(30.0, 0.0, 10.0) * knob("lock_in", 1.0);
            s.pos.v += Vec3::new(-6.0, 0.0, -3.0);
        }
        s.was_raising = raising;
        // A hard landing: the hands brace, the gun dips.
        if !s.was_on_ground && mover.on_ground {
            let fall = (-s.last_vy / u(200.0)).clamp(0.0, 3.0);
            s.pos.v += Vec3::new(0.0, 0.0, -14.0 * fall) * knob("land", 1.0);
            s.rot.v += Vec3::new(40.0 * fall, 0.0, 0.0) * knob("land", 1.0);
            for wr in &mut s.wrist {
                wr.v += Vec3::new(-30.0 * fall, 0.0, 0.0);
            }
            s.curl.iter_mut().flatten().for_each(|c| c.v += 2.0 * fall);
        }
        s.was_on_ground = mover.on_ground;
        s.last_vy = mover.velocity.y;
        // Out of a sprint: it settles.
        if s.was_sprinting && !sprinting {
            s.rot.v += Vec3::new(-25.0, 10.0, 0.0) * h.mass.sqrt() * knob("sprint_settle", 1.0);
        }
        s.was_sprinting = sprinting;
        // Out of breath after sprinting: a heave, calming.
        s.fatigue = if sprinting { (s.fatigue + dt / knob("fatigue_build", 5.0)).min(1.0) } else { (s.fatigue - dt / knob("fatigue_calm", 7.0)).max(0.0) };
        s.breath += dt * std::f32::consts::TAU * (0.25 + 0.3 * s.fatigue);
        let heave = s.breath.sin() * s.fatigue * knob("heave", 1.0) * shake() * (1.0 - 0.6 * ads) * if sprinting { 0.0 } else { 1.0 };
        // --- The reload's beats, from its animation's notes (the sounds'
        // names: `..._clipout_...`, `..._clipin_...`, `..._chamber_...`):
        // the magazine out, the gun lighter, it lifts; in, it's knocked up
        // and back down; the bolt or slide worked, a jolt back and a roll.
        if let Some((_, _, player)) = slot.and_then(|sl| viewmodels.iter().find(|(v, ..)| v.0 == sl.0)) {
            if let Some(anim) = &player.anim {
                let key = std::sync::Arc::as_ptr(anim) as usize;
                let from = match s.last_note {
                    Some((k, t)) if k == key && t <= player.time => t,
                    _ => -1.0,
                };
                let duration = anim.duration().max(1e-3);
                let empty = w.clip == 0;
                for n in &anim.notifies {
                    let at = n.time * duration;
                    if at <= from || at > player.time {
                        continue;
                    }
                    let name = n.name.to_ascii_lowercase();
                    let beat = knob("reload_beats", 1.0) * calm;
                    if name.contains("clipout") || name.contains("magout") {
                        s.rel_rot.v += Vec3::new(-10.0, 0.0, 0.0) * beat;
                        s.curl[LEFT].iter_mut().for_each(|c| c.v -= 2.0 * beat);
                    } else if name.contains("clipin") || name.contains("magin") {
                        s.rel_pos.v += Vec3::new(0.0, 0.0, 6.0 * h.mass) * beat;
                        s.rel_rot.v += Vec3::new(-25.0, 0.0, 0.0) * beat;
                    } else if ["chamber", "bolt", "slide", "pump", "rechamber"].iter().any(|k| name.contains(k)) {
                        let k = if empty { 1.5 } else { 1.0 } * beat;
                        s.rel_pos.v += Vec3::new(-10.0, 0.0, 0.0) * k;
                        s.rel_rot.v += Vec3::new(0.0, 0.0, 15.0) * k;
                    }
                }
                s.last_note = Some((key, player.time));
            }
        }

        // --- Shot: a flinch (`BG_CalculateWeaponPosition_DamageKick`),
        // away from where it came from, quick out and slower back.
        for &(target, attacker, amount) in &hits {
            if target != e {
                continue;
            }
            let kick = (amount * 0.2).clamp(5.0, 10.0) * knob("flinch", 1.0) * (0.5 + 0.5 * ads);
            let side = attacker
                .and_then(|a| attackers.get(a).ok())
                .map_or(0.0, |a| {
                    let to = (a.translation - tf.translation).with_y(0.0).normalize_or_zero();
                    let right = view.rotation() * Vec3::X;
                    to.dot(right)
                });
            s.rot.v += Vec3::new(-kick * 8.0, -side * kick * 6.0, side * kick * 8.0);
            s.wrist[RIGHT].v += Vec3::new(kick * 3.0, 0.0, 0.0);
        }

        // --- Into a sprint: the gun swung down and across, by its weight.
        if !s.was_sprinting && sprinting {
            s.rot.v += Vec3::new(20.0, -15.0, 25.0) * h.mass.sqrt() * knob("sprint_in", 1.0);
        }
        // In the air a moment: the gun lifted a touch, the hands easing.
        if mover.on_ground {
            s.left_ground = now;
        }
        let airborne = !mover.on_ground && now - s.left_ground > 0.12;

        // A wall within the gun's reach: drawn in to the chest, muzzle up
        // and in (high ready), not back into the eye. A ball the gun's
        // thickness cast from the gun's side of the view, as far as the
        // muzzle reaches; held a moment, eased in and out. Not while
        // sprinting (the sprint carries the gun).
        let eye = mover.eye(tf.translation);
        let rot = view.rotation();
        let from = eye + rot * Vec3::new(u(knob("wall_side", 3.0)), -u(3.0), 0.0);
        let reach = u(s.reach + knob("wall_margin", 4.0));
        let ball = avian3d::prelude::Collider::sphere(u(knob("wall_radius", 3.0)));
        let config = avian3d::prelude::ShapeCastConfig::from_max_distance(reach);
        let hit = Dir3::new(rot * Vec3::NEG_Z)
            .ok()
            .and_then(|d| spatial.cast_shape(&ball, from, Quat::IDENTITY, d, &config, &crate::collision::sight_filter()));
        // (Facing the barrel, not a wall it runs alongside: the side of a
        // building the gun passes on an open street.)
        let facing = |h: &avian3d::prelude::ShapeHitData| h.normal1.dot(-(rot * Vec3::NEG_Z)) > knob("wall_facing", 0.85);
        // (And straight ahead of the eye too, not only the gun's side: a
        // corner's edge the barrel grazes isn't a wall in front.)
        let ahead = Dir3::new(rot * Vec3::NEG_Z).ok().and_then(|d| spatial.cast_ray(eye, d, reach, true, &crate::collision::sight_filter())).is_some();
        let close = hit.filter(facing).filter(|_| ahead).map_or(1.0, |h| h.distance / reach);
        let tucked = s.tuck.x > 0.5;
        let blocked = !sprinting && ads < 0.1 && mover.lean.abs() < 0.05 && if tucked { close < 0.95 } else { close < 0.85 };
        // Tucks once the wall's been there 0.2 s (not a corner swept past);
        // stays a moment after.
        s.wall_since = if blocked { Some(s.wall_since.unwrap_or(now)) } else { None };
        let want = blocked && (tucked || s.wall_since.is_some_and(|t| now - t >= knob("wall_delay", 0.2)));
        if want {
            s.tucked_until = now + knob("wall_hold", 0.25);
        }
        let target = if want || now < s.tucked_until { 1.0 } else { 0.0 };
        s.tuck.step(target, if target > s.tuck.x { 14.0 } else { 9.0 }, 1.0, dt);
        let wall = s.tuck.x.clamp(0.0, 1.0);
        let fid = s.fidget.map(|f| f.0);
        let roll = match fid {
            Some(Fidget::Roll(r)) => r,
            _ => 0.0,
        };
        // (None of it aimed in: the sights, and a 3D scope's reticle, stay
        // on the eye's line.)
        let hip = (1.0 - ads).powi(2);
        let (wall, heave, roll) = (wall * hip, heave * hip, roll * hip);
        let air = if airborne { hip } else { 0.0 };
        let rot_target = Vec3::new(-heave * 1.2 - wall * knob("wall_up", 30.0) - 2.0 * air, wall * knob("wall_in", 12.0), roll + wall * knob("wall_roll", 15.0));
        let pos_target = Vec3::new(-wall * knob("wall_back", 2.0), 0.0, heave * 0.25 - wall * knob("wall_down", 1.5) + 0.4 * air);
        s.rot.step(rot_target, 9.0, 0.55, dt);
        s.pos.step(pos_target, 9.0, 0.6, dt);

        // --- The gun against the hands, hauled back by them (faster for a
        // lighter gun).
        s.rel_rot.step(Vec3::ZERO, h.punch_omega, h.punch_zeta, dt);
        s.rel_pos.step(Vec3::ZERO, h.punch_omega, h.punch_zeta, dt);
        // The climb builds with the burst and settles after it.
        // (None aimed in: the sights stay on the line the bullets go.)
        let climbing = if firing { (s.burst as f32 * h.climb * knob("climb_per_shot", 1.0)).min(h.climb_max) * h.energy / h.mass * (1.0 - ads).powi(2) } else { 0.0 };
        s.climb.step(Vec3::new(-climbing, 0.0, 0.0), knob("climb_omega", 9.0), h.climb_zeta, dt);
        s.rel_rot.x = s.rel_rot.x.clamp(Vec3::splat(-12.0), Vec3::splat(12.0));
        s.rel_pos.x = s.rel_pos.x.clamp(Vec3::splat(-2.5), Vec3::splat(2.5));

        // --- Active idle: the gun drifts as the arms hold it (an
        // Ornstein-Uhlenbeck walk: a pull back, a random push, the same at
        // any frame rate), more for a heavier gun, when nervous or out of
        // breath; and every 1.5-4 s, or once it's strayed, the hands take
        // most of it back in a quick move. The whole gun, about the hands.
        let (theta, sigma) = (knob("idle_theta", 0.6), knob("idle_sigma", 0.35) * shake() * nerves.sqrt() * (1.0 + s.fatigue) * h.mass.sqrt() * calm);
        let normal = |s: &mut State| {
            // (Two uniforms' sum, about normal.)
            (s.rand() + s.rand() + s.rand() - 1.5) * 2.0
        };
        for i in 0..3 {
            let n = normal(s);
            s.idle[i] += -theta * s.idle[i] * dt + sigma * dt.sqrt() * n * if i == 2 { 0.6 } else { 1.0 };
        }
        let strayed = s.idle.length() > knob("idle_stray", 0.9) * h.mass.sqrt();
        if !busy && (strayed || now >= s.next_fix) {
            let take = 0.6 + 0.2 * s.rand();
            s.idle_fix.x += s.idle * take;
            s.idle *= 1.0 - take;
            let (lo, hi) = (knob("idle_fix_min", 1.5), knob("idle_fix_max", 4.0));
            s.next_fix = now + (lo + (hi - lo) * s.rand()) * h.mass.sqrt();
        }
        // (The correction: what was taken back eases away fast.)
        s.idle_fix.step(Vec3::ZERO, knob("idle_fix_omega", 22.0) / h.mass.sqrt(), 0.8, dt);
        let idle = s.idle + s.idle_fix.x;

        // --- The wrists: absorbing, and a small restless readjusting (most
        // of it the gun and hands together, a little in the wrists).
        let tremor_amp = knob("tremor", 0.6) * shake() * nerves * calm;
        for i in 0..3 {
            let n = normal(s);
            s.tremor[i] += -4.0 * s.tremor[i] * dt + 1.2 * dt.sqrt() * n;
        }
        let tremor = s.tremor * tremor_amp;
        s.wrist[RIGHT].step(tremor * 0.2, 18.0, 0.45, dt);
        s.wrist[LEFT].step(Vec3::new(tremor.y, tremor.z, tremor.x) * 0.2, 16.0, 0.45, dt);
        s.rel_rot.x += tremor * 0.8 * dt * 10.0;

        // --- The fingers.
        let regrip = matches!(fid, Some(Fidget::Regrip));
        let grip = knob("grip", 1.0);
        for side in [RIGHT, LEFT] {
            for f in 0..5 {
                let mut target = 0.0;
                if side == LEFT {
                    // The support hand holds harder while firing.
                    target += if firing { 0.12 * grip } else { 0.0 };
                }
                if regrip {
                    target -= 0.3;
                }
                if f == THUMB {
                    if let Some(Fidget::Thumb(h, a)) = fid {
                        if h == side {
                            target += a;
                        }
                    }
                }
                if side == RIGHT && f == INDEX {
                    // The trigger: squeezed as it's pressed (before the shot),
                    // held in for automatic fire with a ripple a shot, eased
                    // off after; off the trigger, along the frame, while
                    // sprinting and in a trigger fidget.
                    let pressed = input.is_some_and(|i| i.fire) && !busy;
                    if pressed {
                        let auto = w.def.fire_type == 0;
                        target += if auto { knob("trigger_hold", 0.12) + 0.05 * (now * 40.0).sin() * f32::from(firing) } else { knob("trigger_pull", 0.18) };
                    }
                    if !s.trigger_held && pressed {
                        s.curl[side][f].v += knob("trigger_pull", 0.18) * 30.0;
                    }
                    if mover.sprinting || matches!(fid, Some(Fidget::TriggerOff)) {
                        target -= 0.4;
                    }
                }
                s.curl[side][f].step(target, 20.0, 0.5, dt);
            }
        }
        s.trigger_held = input.is_some_and(|i| i.fire) && !busy;
        s.slide.step(s.rest, 12.0, 0.7, dt);

        // On the way in (and out), the gun cants over and dips a little, as
        // it's swung up to the eye: none at the hip or fully aimed.
        let going_in = ads > s.ads_was;
        s.ads_was = ads;
        let swing = (std::f32::consts::PI * ads).sin();
        let cant = Vec3::new(swing * knob("ads_dip", 1.5), 0.0, swing * knob("ads_cant", 6.0) * h.mass.sqrt() * if going_in { 1.0 } else { -1.0 });
        let (rot, pos) = (s.rot.x + idle * (1.0 - 0.8 * ads) + cant, s.pos.x);
        live.gun_angles = rot * knob("whole", 1.0);
        live.gun_shift = pos * knob("whole", 1.0);
    }
}

/// A joint's place in its viewmodel's own space (this frame's animated
/// pose, before it's propagated).
fn in_root(poses: &Query<(&mut Transform, Option<&ChildOf>)>, joint: Entity, root: Entity) -> Option<Affine3A> {
    let mut at = Affine3A::IDENTITY;
    let mut e = joint;
    for _ in 0..64 {
        if e == root {
            return Some(at);
        }
        let (tf, parent) = poses.get(e).ok()?;
        at = tf.compute_affine() * at;
        e = parent?.parent();
    }
    None
}

/// Two-bone IK (as [`crate::killstreaks`]' hands): the turns of the upper
/// arm about the shoulder `s`, then of the forearm about the moved elbow,
/// taking the wrist `w` to `target`, bending the way it already does.
fn two_bone(s: Vec3, e: Vec3, w: Vec3, target: Vec3) -> Option<(Quat, Quat)> {
    let (upper, fore) = ((e - s).length(), (w - e).length());
    let to = target - s;
    let dir = to.try_normalize()?;
    let reach = to.length().clamp((upper - fore).abs() + 1e-4, upper + fore - 1e-4);
    let bend = ((e - s) - dir * (e - s).dot(dir)).try_normalize().or_else(|| dir.any_orthonormal_vector().try_normalize())?;
    let cos = ((upper * upper + reach * reach - fore * fore) / (2.0 * upper * reach)).clamp(-1.0, 1.0);
    let elbow = s + (dir * cos + bend * (1.0 - cos * cos).max(0.0).sqrt()) * upper;
    let q1 = Quat::from_rotation_arc((e - s).try_normalize()?, (elbow - s).try_normalize()?);
    let wrist = s + q1 * (w - s);
    let q2 = Quat::from_rotation_arc((wrist - elbow).try_normalize()?, (s + dir * reach - elbow).try_normalize()?);
    Some((q1, q2))
}

/// CoD angles (degrees: pitch down, yaw left, roll right side down) as a
/// turn in a frame whose x is forward, y left, z up (the gun's and hands'
/// own bones, in Bevy's axes as `crate::units` converts them).
fn cod_turn(a: Vec3) -> Quat {
    // CoD's (x forward, y left, z up) is Bevy's (x, z, -y) for a model's
    // own axes ([`crate::units::dir`]): yaw about z (Bevy y), pitch about
    // y (Bevy -z), roll about x.
    Quat::from_rotation_y(a.y.to_radians()) * Quat::from_rotation_z(-a.x.to_radians()) * Quat::from_rotation_x(a.z.to_radians())
}

/// Pose the hands on the gun: move the gun against them, keep each wrist on
/// its grip (IK), turn the wrists, curl the fingers.
#[allow(clippy::type_complexity)]
fn pose(
    mut players: Query<(&crate::splitscreen::LocalSlot, &mut LiveHands), (Without<crate::killstreaks::hands::HardpointHands>, Without<crate::killstreaks::sentry::Carrying>)>,
    roots: Query<(Entity, &Skeleton, &crate::viewmodel::ViewModelSlot, &crate::models::AnimPlayer), With<crate::viewmodel::ViewModelRoot>>,
    mut poses: Query<(&mut Transform, Option<&ChildOf>)>,
) {
    if !super::live_hands() {
        return;
    }
    for (root, skeleton, slot, player) in &roots {
        // (Only over a pose the animation set afresh this frame: without
        // one, the joints keep last frame's, and the offsets would pile up.)
        if player.anim.is_none() {
            continue;
        }
        let Some((_, mut live)) = players.iter_mut().find(|(s, _)| s.0 == slot.0) else { continue };
        let joint = |n: &str| skeleton.joint(n);
        // The gun's reach from the eye (its muzzle), for the wall tuck.
        if let Some(flash) = joint("tag_flash").and_then(|f| in_root(&poses, f, root)) {
            let r = Vec3::from(flash.translation).length() / crate::units::INCH;
            if r > 4.0 && r < 80.0 {
                live.s.reach += (r - live.s.reach) * 0.1;
            }
        }
        let s = &live.s;
        let Some(gun) = joint("tag_weapon") else { continue };
        let Some(gun_parent) = poses.get(gun).ok().and_then(|(_, p)| p.map(|p| p.parent())) else { continue };
        let (Some(gun_at), Some(parent_at)) = (in_root(&poses, gun, root), in_root(&poses, gun_parent, root)) else { continue };

        // Where each wrist holds the gun (the animation's grip, in the
        // gun's frame), the support hand slid along it.
        let mut grips = [None; 2];
        for side in [RIGHT, LEFT] {
            if let Some(w) = joint(&format!("j_wrist_{}", SIDES[side])).and_then(|w| in_root(&poses, w, root)) {
                let mut g = gun_at.inverse() * w;
                if side == LEFT {
                    g.translation += bevy::math::Vec3A::X * u(s.slide.x);
                }
                grips[side] = Some(g);
            }
        }

        // The gun moved against the hands, about its grip.
        let turn = cod_turn(s.rel_rot.x + s.climb.x);
        let shift = crate::units::pos(s.rel_pos.x.to_array());
        let moved = gun_at * Affine3A::from_rotation_translation(turn, shift);
        if let Ok((mut tf, _)) = poses.get_mut(gun) {
            *tf = Transform::from_matrix((parent_at.inverse() * moved).into());
        }

        // Each arm reaches its grip; the wrist turns to hold it (and gives).
        for side in [RIGHT, LEFT] {
            let Some(grip) = grips[side] else { continue };
            let name = |n: &str| joint(&format!("j_{n}_{}", SIDES[side]));
            let (Some(sj), Some(ej), Some(wj)) = (name("shoulder"), name("elbow"), name("wrist")) else { continue };
            let Some(sp) = poses.get(sj).ok().and_then(|(_, p)| p.map(|p| p.parent())) else { continue };
            let (Some(sg), Some(eg), Some(wg), Some(pg)) = (in_root(&poses, sj, root), in_root(&poses, ej, root), in_root(&poses, wj, root), in_root(&poses, sp, root)) else { continue };
            let want = moved * grip;
            let target: Vec3 = want.translation.into();
            let Some((q1, q2)) = two_bone(sg.translation.into(), eg.translation.into(), wg.translation.into(), target) else { continue };
            let (_, prot, _) = pg.to_scale_rotation_translation();
            let (_, srot, _) = sg.to_scale_rotation_translation();
            let (_, erot, _) = eg.to_scale_rotation_translation();
            if let Ok((mut tf, _)) = poses.get_mut(sj) {
                tf.rotation = (prot.inverse() * q1 * prot * tf.rotation).normalize();
            }
            if let Ok((mut tf, _)) = poses.get_mut(ej) {
                tf.rotation = (srot.inverse() * q1.inverse() * q2 * q1 * srot * tf.rotation).normalize();
            }
            // The wrist: the grip's own turn, and its give.
            let (_, want_rot, _) = want.to_scale_rotation_translation();
            let elbow_now = q2 * q1 * erot;
            let give = cod_turn(s.wrist[side].x);
            if let Ok((mut tf, _)) = poses.get_mut(wj) {
                tf.rotation = (elbow_now.inverse() * want_rot * give).normalize();
            }
        }

        // The fingers: each curls towards its palm by its spring.
        for side in [RIGHT, LEFT] {
            let sd = SIDES[side];
            let Some(wrist) = joint(&format!("j_wrist_{sd}")).and_then(|w| in_root(&poses, w, root)) else { continue };
            let w: Vec3 = wrist.translation.into();
            // Which way the palm faces: where the curled fingers' tips lean.
            let mut palm = Vec3::ZERO;
            for (f, _) in FINGERS.iter().take(4) {
                let (Some(a), Some(c)) = (joint(&format!("j_{f}_{sd}_1")).and_then(|j| in_root(&poses, j, root)), joint(&format!("j_{f}_{sd}_3")).and_then(|j| in_root(&poses, j, root))) else { continue };
                let (a, c): (Vec3, Vec3) = (a.translation.into(), c.translation.into());
                let base = (a - w).normalize_or_zero();
                palm += (c - a) - base * (c - a).dot(base);
            }
            // (A flat or open hand has no lean: the palm from the hand's own
            // bones, the index's root and the pinky's, kept facing the way
            // the fingers lean where they do.)
            let bones = joint(&format!("j_index_{sd}_1"))
                .and_then(|j| in_root(&poses, j, root))
                .zip(joint(&format!("j_pinky_{sd}_1")).and_then(|j| in_root(&poses, j, root)))
                .map(|(i, p)| {
                    let (i, p): (Vec3, Vec3) = (i.translation.into(), p.translation.into());
                    let n = (i - w).cross(p - w);
                    if side == LEFT { -n } else { n }
                });
            let palm = match (bones.and_then(|b| b.try_normalize()), palm.try_normalize()) {
                (Some(b), Some(lean)) => if b.dot(lean) < 0.0 { -b } else { b },
                (Some(b), None) => b,
                (None, Some(lean)) => lean,
                (None, None) => continue,
            };
            let index_dir = joint(&format!("j_index_{sd}_1")).and_then(|j| in_root(&poses, j, root)).map(|i| Vec3::from(i.translation) - w);
            for (f, (name, chain)) in FINGERS.iter().enumerate() {
                let curl = s.curl[side][f].x * knob("finger", 1.0);
                if curl.abs() < 1e-4 {
                    continue;
                }
                for (k, n) in chain.iter().enumerate() {
                    let Some(j) = joint(&format!("j_{name}_{sd}_{n}")) else { continue };
                    let next = chain.get(k + 1).and_then(|m| joint(&format!("j_{name}_{sd}_{m}")));
                    let Some(jp) = poses.get(j).ok().and_then(|(_, p)| p.map(|p| p.parent())) else { continue };
                    let (Some(jg), Some(pg)) = (in_root(&poses, j, root), in_root(&poses, jp, root)) else { continue };
                    let dir: Vec3 = match next.and_then(|m| in_root(&poses, m, root)) {
                        Some(ng) => Vec3::from(ng.translation) - Vec3::from(jg.translation),
                        None => jg.transform_vector3(Vec3::X),
                    };
                    // (The thumb bends across the palm, towards the index.)
                    let axis = if f == THUMB { index_dir.and_then(|i| dir.cross(i).try_normalize()) } else { dir.cross(palm).try_normalize() };
                    let Some(axis) = axis else { continue };
                    // Most at the middle joint; never bent back past straight
                    // by more than a few degrees.
                    let share = [0.3f32, 0.45, 0.25][k.min(2)];
                    let r = Quat::from_axis_angle(axis, (curl * share * 1.5).max(-5f32.to_radians()));
                    let (_, prot, _) = pg.to_scale_rotation_translation();
                    let (_, jrot, _) = jg.to_scale_rotation_translation();
                    if let Ok((mut tf, _)) = poses.get_mut(j) {
                        tf.rotation = (prot.inverse() * r * jrot).normalize();
                    }
                }
            }
        }
    }
}

#[derive(Resource)]
struct AbDir(std::path::PathBuf);

/// Test aid: `COD4RW_LIVEHANDS_AB=<dir>`, the same short sequence twice,
/// live hands off then on (`a_*.png`, `b_*.png`): standing still (0-10 s,
/// aiming in and out at 6-8 s),
/// a burst (10-11 s), a reload (11.5 s), a sprint (14-16 s), a jump and its
/// landing (16.5 s); a frame every 0.1 s from 4 s; then exit.
fn ab_test(
    mut commands: Commands,
    time: Res<Time>,
    dir: Res<AbDir>,
    mut players: Query<(&mut Transform, &mut ViewAngles, &mut crate::weapons::WeaponState, &mut Mover), With<crate::player::LocalPlayer>>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    mut cursor: Single<&mut bevy::window::CursorOptions, With<bevy::window::PrimaryWindow>>,
    mut start: Local<Option<(f32, Transform, ViewAngles)>>,
    mut next_shot: Local<f32>,
    mut restarted: Local<bool>,
    mut exit: MessageWriter<AppExit>,
    spatial: SpatialQuery,
) {
    let Ok((mut tf, mut view, mut weapon, mut mover)) = players.single_mut() else { return };
    let now = time.elapsed_secs();
    // `COD4RW_LIVEHANDS_STAIRS=1`: the player at the foot of the nearest
    // stairs (stepped ones), walking up (1-4 s), turning, walking back down
    // (4.5-7.5 s), each half 8 s, the camera's step smoothing off the first.
    let stairs = std::env::var_os("COD4RW_LIVEHANDS_STAIRS").is_some();
    if stairs && start.is_none() {
        let filter = crate::collision::movement_filter();
        let eye = mover.eye(tf.translation);
        let ground = |p: Vec3| {
            let from = Vec3::new(p.x, eye.y + u(200.0), p.z);
            spatial.cast_ray(from, Dir3::NEG_Y, u(800.0), true, &filter).map(|h| from - Vec3::Y * h.distance)
        };
        let mut best: Option<(f32, Vec3, f32)> = None;
        for gx in -60..=60 {
            for gz in -60..=60 {
                let p = tf.translation + Vec3::new(gx as f32, 0.0, gz as f32) * u(16.0);
                let d = p.with_y(0.0).distance(tf.translation.with_y(0.0));
                if best.is_some_and(|b| b.0 < d) {
                    continue;
                }
                for k in 0..8 {
                    let yaw = k as f32 * std::f32::consts::FRAC_PI_4;
                    let dir = Quat::from_rotation_y(yaw) * Vec3::NEG_Z;
                    let heights: Vec<f32> = (0..8).filter_map(|i| ground(p + dir * u(12.0) * i as f32).map(|g| g.y)).collect();
                    if heights.len() < 8 {
                        continue;
                    }
                    let steps = heights.windows(2).filter(|w| (u(5.0)..u(14.0)).contains(&(w[1] - w[0]))).count();
                    if steps >= 5 {
                        if let Some(foot) = ground(p - dir * u(24.0)) {
                            best = Some((d, foot, yaw));
                        }
                    }
                }
            }
        }
        match best {
            Some((_, foot, yaw)) => {
                tf.translation = foot + Vec3::Y * u(1.0);
                view.yaw = yaw;
                view.pitch = 0.0;
                info!("live hands test: stairs at CoD {:?}, yaw {:.0}", crate::units::to_cod(foot).map(f32::round), yaw.to_degrees());
            }
            None => warn!("live hands test: no stairs near the spawn"),
        }
    }
    // `COD4RW_LIVEHANDS_WALL=1`: the player stood facing a wall about 20
    // units off (the nearest found from the spawn), still, each half 7 s.
    let wall = std::env::var_os("COD4RW_LIVEHANDS_WALL").is_some();
    if wall && start.is_none() {
        let eye = mover.eye(tf.translation);
        let mut best: Option<(f32, f32)> = None;
        for k in 0..48 {
            let yaw = k as f32 * std::f32::consts::TAU / 48.0;
            let dir = Quat::from_rotation_y(yaw) * Vec3::NEG_Z;
            let Ok(d) = Dir3::new(dir) else { continue };
            if let Some(hit) = spatial.cast_ray(eye, d, u(400.0), true, &crate::collision::sight_filter()) {
                // A face square on (not a glancing one).
                if hit.distance > u(40.0) && hit.normal.dot(-dir) > 0.9 && best.is_none_or(|b| hit.distance < b.0) {
                    best = Some((hit.distance, yaw));
                }
            }
        }
        if let Some((dist, yaw)) = best {
            let dir = Quat::from_rotation_y(yaw) * Vec3::NEG_Z;
            tf.translation += dir * (dist - u(20.0));
            view.yaw = yaw;
            view.pitch = 0.0;
            info!("live hands test: facing a wall 20 units off, yaw {:.0}", yaw.to_degrees());
        }
    }
    let (t0, at, looking) = *start.get_or_insert((now, *tf, *view));
    let t = now - t0;
    let half_len: f32 = if wall { 7.0 } else if stairs { 8.0 } else { 19.5 };
    #[allow(non_snake_case)]
    let HALF = half_len;
    let (half, t) = if t < HALF { (0, t) } else { (1, t - HALF) };
    if half == 1 && t > HALF {
        exit.write(AppExit::Success);
        return;
    }
    super::set_live_hands_test(half == 1);
    crate::player::ZSMOOTH_OFF.store(stairs && half == 0, std::sync::atomic::Ordering::Relaxed);
    // The second time from the same place, looking the same way, the
    // magazine full.
    if half == 1 && !*restarted {
        *restarted = true;
        *tf = at;
        *view = looking;
        weapon.clip = weapon.def.clip_size;
        mover.velocity = Vec3::ZERO;
    }
    if cursor.grab_mode == bevy::window::CursorGrabMode::None {
        cursor.grab_mode = bevy::window::CursorGrabMode::Locked;
    }
    let quiet = wall || stairs;
    // The stairs: up, turn round, down.
    if stairs {
        if (4.0..4.5).contains(&t) {
            view.yaw += std::f32::consts::PI * time.delta_secs() / 0.5;
        }
    }
    let fire = !quiet && (10.0..11.0).contains(&t);
    let reload = !quiet && (11.5..11.55).contains(&t);
    let walk_stairs = stairs && ((1.0..4.0).contains(&t) || (4.5..7.5).contains(&t));
    let (fwd, sprint) = (walk_stairs || !quiet && (14.0..16.0).contains(&t), !quiet && (14.0..16.0).contains(&t));
    let jump = !quiet && (16.5..16.55).contains(&t);
    for (key, on) in [(KeyCode::KeyR, reload), (KeyCode::KeyW, fwd), (KeyCode::ShiftLeft, sprint), (KeyCode::Space, jump)] {
        if on { keys.press(key) } else { keys.release(key) }
    }
    if fire { mouse.press(MouseButton::Left) } else { mouse.release(MouseButton::Left) }
    // Aiming in and out (6-8 s).
    let aim = !quiet && (6.0..8.0).contains(&t);
    if aim { mouse.press(MouseButton::Right) } else { mouse.release(MouseButton::Right) }
    if t >= if wall { 2.0 } else if stairs { 0.5 } else { 4.0 } && now >= *next_shot {
        *next_shot = now + 0.1;
        let path = dir.0.join(format!("{}_{:05.1}.png", if half == 0 { "a" } else { "b" }, t));
        commands.spawn(bevy::render::view::screenshot::Screenshot::primary_window()).observe(bevy::render::view::screenshot::save_to_disk(path));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The arm reaches a target it can, bending the way it did.
    #[test]
    fn arm_reaches_its_grip() {
        let (s, e, w) = (Vec3::ZERO, Vec3::new(0.3, -0.1, 0.0), Vec3::new(0.55, 0.05, 0.0));
        let target = Vec3::new(0.5, 0.1, 0.1);
        let (q1, q2) = two_bone(s, e, w, target).unwrap();
        let elbow = s + q1 * (e - s);
        let wrist = elbow + q2 * (q1 * (w - e));
        assert!(wrist.distance(target) < 1e-3, "{wrist} vs {target}");
        // Bone lengths kept.
        assert!(((elbow - s).length() - (e - s).length()).abs() < 1e-4);
    }

    /// CoD's pitch down tips the muzzle (forward, +x) down; yaw left turns
    /// it left (CoD's +y, Bevy's -z).
    #[test]
    fn cod_angles_turn_the_right_way() {
        assert!((cod_turn(Vec3::new(10.0, 0.0, 0.0)) * Vec3::X).y < 0.0);
        assert!((cod_turn(Vec3::new(0.0, 10.0, 0.0)) * Vec3::X).z < 0.0);
    }
}

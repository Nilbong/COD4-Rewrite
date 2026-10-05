//! What the killcam replays: every pawn, projectile, shot, explosion and
//! kill of the last few seconds, sampled 20 times a second from the
//! components and messages gameplay already has.

use crate::combat::{Damage, HitLocation, Killed, Pawn};
use crate::explosives::{Exploded, Explosive, ExplosiveDefs};
use crate::grenades::{GrenadeDefs, LiveGrenade};
use crate::loadout::Loadout;
use crate::models::AnimPlayer;
use crate::movement::{Mover, ViewAngles};
use crate::thirdperson::Body;
use crate::weapons::{ShotFired, WeaponDef, WeaponState};
use bevy::prelude::*;
use iw3::xanim::XAnim;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

/// Samples a second, and how long the history goes back.
pub const RATE: f32 = 20.0;
pub const KEEP: f32 = 12.0;

/// A pawn at one moment.
#[derive(Clone)]
pub struct PawnSample {
    pub entity: Entity,
    pub feet: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub eye_height: f32,
    pub ads: f32,
    pub weapon: &'static WeaponDef,
    /// The class gun in hand (`ak47:reflex`) and its camo, if it has a class.
    pub gun: Option<(String, usize)>,
    pub shots: u32,
    pub reloading: bool,
    pub dead: bool,
    /// Its body: the animation playing and how far in, what's posed over
    /// it (a knife swing), and the body's turn.
    pub anim: Option<(Arc<XAnim>, f32)>,
    pub overlay: Option<(Arc<XAnim>, f32)>,
    pub body_turn: Quat,
    /// A knife swing: when it started, and whether a lunge.
    pub melee: Option<(f32, bool)>,
}

/// A grenade or projectile at one moment.
#[derive(Clone)]
pub struct ProjectileSample {
    pub entity: Entity,
    pub owner: Entity,
    pub at: Vec3,
    pub rotation: Quat,
    pub model: String,
}

#[derive(Clone)]
pub struct Frame {
    pub time: f32,
    pub pawns: Vec<PawnSample>,
    pub projectiles: Vec<ProjectileSample>,
}

/// A kill: who, by whom, with what, and when.
#[derive(Clone, Debug)]
pub struct Kill {
    pub time: f32,
    pub victim: Entity,
    pub attacker: Entity,
    pub weapon: &'static str,
    pub headshot: bool,
}

/// The recent past.
#[derive(Resource, Default)]
pub struct History {
    pub frames: VecDeque<Frame>,
    pub shots: VecDeque<(f32, ShotFired)>,
    pub blasts: VecDeque<(f32, Exploded)>,
    pub kills: VecDeque<Kill>,
    /// The last damage to each pawn (what killed it).
    last_hit: HashMap<Entity, (&'static str, HitLocation)>,
    next_sample: f32,
}

impl History {
    /// The frames either side of `time` and how far between them.
    pub fn at(&self, time: f32) -> Option<(&Frame, &Frame, f32)> {
        let i = self.frames.iter().position(|f| f.time > time)?;
        let b = &self.frames[i];
        let a = if i == 0 { b } else { &self.frames[i - 1] };
        let span = (b.time - a.time).max(1e-4);
        Some((a, b, ((time - a.time) / span).clamp(0.0, 1.0)))
    }

    /// The latest frame (for times past the end of the history).
    pub fn last(&self) -> Option<&Frame> {
        self.frames.back()
    }
}

/// Record the messages as they come, and the world every 1/20 s (before the
/// killcam poses bodies its own way, so its replays aren't recorded).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn record(
    time: Res<Time>,
    mut history: ResMut<History>,
    mut shots: MessageReader<ShotFired>,
    mut blasts: MessageReader<Exploded>,
    mut killed: MessageReader<Killed>,
    mut damage: MessageReader<Damage>,
    pawns: Query<
        (Entity, &Transform, &Mover, &ViewAngles, &WeaponState, Option<&Loadout>, Option<&Body>, Has<crate::combat::Dead>, Option<&crate::melee::Melee>),
        With<Pawn>,
    >,
    bodies: Query<(&AnimPlayer, &Transform)>,
    grenades: Query<(Entity, &LiveGrenade, &Transform)>,
    explosives: Query<(Entity, &Explosive, &Transform)>,
    grenade_defs: Res<GrenadeDefs>,
    explosive_defs: Res<ExplosiveDefs>,
) {
    let now = time.elapsed_secs();
    let h = &mut *history;
    for s in shots.read() {
        h.shots.push_back((now, *s));
    }
    for b in blasts.read() {
        h.blasts.push_back((now, b.clone()));
    }
    for d in damage.read() {
        h.last_hit.insert(d.target, (d.weapon, d.location));
    }
    for k in killed.read() {
        let (weapon, location) = h.last_hit.get(&k.victim).copied().unwrap_or(("", HitLocation::Torso));
        if let Some(attacker) = k.attacker.filter(|&a| a != k.victim) {
            h.kills.push_back(Kill { time: now, victim: k.victim, attacker, weapon, headshot: location == HitLocation::Head });
        }
    }
    let old = now - KEEP;
    while h.shots.front().is_some_and(|(t, _)| *t < old) {
        h.shots.pop_front();
    }
    while h.blasts.front().is_some_and(|(t, _)| *t < old) {
        h.blasts.pop_front();
    }
    while h.kills.front().is_some_and(|k| k.time < old - 30.0) {
        h.kills.pop_front();
    }
    if now < h.next_sample {
        return;
    }
    h.next_sample = now + 1.0 / RATE;
    let pawns = pawns
        .iter()
        .map(|(entity, tf, mover, view, w, loadout, body, dead, melee)| {
            let (anim, overlay, body_turn) = body.and_then(|b| bodies.get(b.0).ok()).map_or((None, None, Quat::IDENTITY), |(player, btf)| {
                (player.anim.clone().map(|a| (a, player.time)), player.overlay.clone(), btf.rotation)
            });
            PawnSample {
                entity,
                feet: tf.translation,
                yaw: view.yaw,
                pitch: view.pitch,
                eye_height: mover.eye_height,
                ads: w.ads,
                weapon: w.def,
                gun: loadout.and_then(|l| l.gun_for(w.def)).map(|g| (g.spec.clone(), g.camo)),
                shots: w.shots_fired_total,
                reloading: w.reloading(),
                dead,
                anim,
                overlay,
                body_turn,
                melee: melee.map(|m| (m.started, m.charge)),
            }
        })
        .collect();
    let mut projectiles: Vec<ProjectileSample> = grenades
        .iter()
        .map(|(entity, g, tf)| ProjectileSample {
            entity,
            owner: g.thrower,
            at: tf.translation,
            rotation: tf.rotation,
            model: grenade_defs.get(g.kind).map(|s| s.model.clone()).unwrap_or_default(),
        })
        .collect();
    projectiles.extend(explosives.iter().map(|(entity, x, tf)| ProjectileSample {
        entity,
        owner: x.owner,
        at: tf.translation,
        rotation: tf.rotation,
        model: explosive_defs.get(&x.weapon).map(|d| d.model.clone()).unwrap_or_default(),
    }));
    h.frames.push_back(Frame { time: now, pawns, projectiles });
    while h.frames.front().is_some_and(|f| f.time < old) {
        h.frames.pop_front();
    }
}

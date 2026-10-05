//! What a bot knows about its enemies.
//!
//! - Sight: an enemy is visible with a clear line from the bot's eye to its
//!   head or chest inside the field of view. Noticing takes time: longer in
//!   peripheral vision, at range, or for a crouched target; shorter for one
//!   that moves or shoots.
//! - Hearing: gunfire reveals the shooter roughly, like CoD4's radar red
//!   dots; nearby running is heard.
//! - Being shot reveals roughly where it came from.
//! - Teammates call out what they notice, after a short delay.
//! - Contacts fade from memory.

use crate::collision;
use crate::movement::Stance;
use crate::units::u;
use avian3d::prelude::*;
use bevy::prelude::*;
use rand::Rng;
use std::collections::HashMap;

/// How far bots can see.
pub const SIGHT_RANGE: f32 = u(6000.0);
/// Half-angle of the field of view.
const FOV_COS: f32 = 0.26;
/// Gunfire is heard (radar-style) this far away, footsteps much closer.
const GUNFIRE_RANGE: f32 = u(3000.0);
const FOOTSTEP_RANGE: f32 = u(500.0);
/// How long until teammates act on a callout.
const CALLOUT_DELAY: f32 = 0.8;
/// Forget contacts not updated for this long.
const MEMORY: f32 = 20.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Seen,
    Heard,
    Told,
    ShotBy,
}

#[derive(Clone, Copy, Debug)]
pub struct Contact {
    /// Last known feet position (exact when seen, rough otherwise).
    pub pos: Vec3,
    pub vel: Vec3,
    /// When `pos` was last updated.
    pub time: f32,
    pub source: Source,
    /// Visible right now.
    pub visible: bool,
    /// 0..1: how close the bot is to noticing this enemy while visible.
    pub awareness: f32,
    /// When the bot noticed this enemy (while it stays in view).
    pub noticed_at: Option<f32>,
    /// When this enemy last came into view.
    pub visible_since: f32,
    /// When this enemy was last in view.
    pub last_visible: f32,
    /// The enemy shot at us recently.
    pub shot_us_at: Option<f32>,
}

impl Contact {
    /// Where the enemy probably is now.
    pub fn predicted(&self, now: f32) -> Vec3 {
        self.pos + self.vel * (now - self.time).min(0.75)
    }

    pub fn noticed(&self) -> bool {
        self.visible && self.noticed_at.is_some()
    }

    /// Noticed and in view, or out of view only for a moment: still a fight.
    pub fn in_fight(&self, now: f32) -> bool {
        self.noticed_at.is_some() && (self.visible || now - self.last_visible < 0.6)
    }

    pub fn age(&self, now: f32) -> f32 {
        now - self.time
    }
}

#[derive(Default)]
pub struct Knowledge {
    pub contacts: HashMap<Entity, Contact>,
}

/// A snapshot of one pawn for perception.
#[derive(Clone, Copy)]
pub struct PawnView {
    pub entity: Entity,
    pub team: crate::combat::Team,
    /// [`crate::combat::Pawn::id`], for free-for-all.
    pub id: u32,
    pub feet: Vec3,
    pub eye_height: f32,
    pub vel: Vec3,
    pub stance: Stance,
    pub sprinting: bool,
}

impl PawnView {
    /// An enemy of `p` (a different team, or anyone else in free-for-all).
    pub fn hostile_to(&self, p: &crate::combat::Pawn) -> bool {
        if crate::combat::free_for_all() { self.id != p.id } else { self.team != p.team }
    }

    pub fn head(&self) -> Vec3 {
        self.feet + Vec3::Y * (self.eye_height + u(2.0))
    }
    pub fn chest(&self) -> Vec3 {
        self.feet + Vec3::Y * self.eye_height * 0.75
    }
}

/// Team-shared sightings: (team that saw, enemy, feet position, time).
#[derive(Resource, Default)]
pub struct Callouts(pub Vec<(crate::combat::Team, Entity, Vec3, f32)>);

/// Update a bot's view of `enemies` from its eye at `eye` looking `forward`.
#[allow(clippy::too_many_arguments)]
pub fn look(
    know: &mut Knowledge,
    spatial: &SpatialQuery,
    eye: Vec3,
    forward: Vec3,
    enemies: &[PawnView],
    recently_fired: &HashMap<Entity, f32>,
    skill: f32,
    now: f32,
    dt: f32,
) -> Vec<Entity> {
    let sight = collision::sight_filter();
    let mut newly_noticed = Vec::new();
    for e in enemies {
        let to = e.chest() - eye;
        let dist = to.length();
        let dir = to / dist.max(1e-3);
        let cos = dir.dot(forward);
        let mut visible = false;
        if dist < SIGHT_RANGE && cos > FOV_COS {
            visible = [e.head(), e.chest()].iter().any(|&p| {
                let d = p - eye;
                let len = d.length();
                Dir3::new(d).ok().is_none_or(|dir| spatial.cast_ray(eye, dir, len, true, &sight).is_none())
            });
        }
        let c = know.contacts.entry(e.entity).or_insert(Contact {
            pos: e.feet,
            vel: Vec3::ZERO,
            time: now,
            source: Source::Seen,
            visible: false,
            awareness: 0.0,
            noticed_at: None,
            shot_us_at: None,
            visible_since: now,
            last_visible: f32::MIN,
        });
        if visible {
            // Seconds to notice, from a quick glance in the centre of view
            // to a slow realisation at the edge, far away.
            let off_axis = (1.0 - cos.clamp(-1.0, 1.0)) / (1.0 - FOV_COS);
            let mut t = 0.12 * (1.0 + 5.0 * off_axis * off_axis) * (1.0 + dist / u(2500.0));
            t *= match e.stance {
                Stance::Stand => 1.0,
                Stance::Crouch => 1.4,
                Stance::Prone => 2.2,
            };
            if e.vel.length() > u(60.0) {
                t *= 0.65;
            }
            if recently_fired.get(&e.entity).is_some_and(|&ft| now - ft < 0.5) {
                t *= 0.35;
            }
            // Already expecting someone there: faster.
            if c.age(now) < 3.0 && c.pos.distance(e.feet) < u(300.0) {
                t *= 0.5;
            }
            t *= 1.4 - 0.6 * skill;
            c.awareness = (c.awareness + dt / t.max(0.02)).min(1.0);
            if c.awareness >= 1.0 && c.noticed_at.is_none() {
                c.noticed_at = Some(now);
                newly_noticed.push(e.entity);
            }
            // A glimpse lost for a moment (a post, a doorframe) is the same
            // sighting, not a new one.
            if !c.visible && now - c.last_visible > 0.3 {
                c.visible_since = now;
            }
            c.visible = true;
            c.last_visible = now;
            if c.noticed_at.is_some() {
                c.pos = e.feet;
                c.vel = e.vel;
                c.time = now;
                c.source = Source::Seen;
            }
        } else {
            c.visible = false;
            c.awareness = (c.awareness - dt * 0.5).max(0.0);
            // Someone just seen stays noticed through a brief gap.
            if now - c.last_visible > 1.0 {
                c.noticed_at = None;
            }
        }
    }
    newly_noticed
}

/// What gave an enemy away out of the blue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sound {
    Footsteps,
    /// Shots fired close by.
    Gunfire,
}

/// Gunfire this close is startling.
const STARTLE_RANGE: f32 = u(1200.0);

/// Hear gunfire and footsteps. Returns the enemies that have just been
/// heard out of the blue (not in view, nor heard a moment ago): close
/// footsteps, or shots close by.
pub fn listen(
    know: &mut Knowledge,
    me: Vec3,
    enemies: &[PawnView],
    recently_fired: &HashMap<Entity, f32>,
    now: f32,
    rng: &mut impl Rng,
) -> Vec<(Entity, Sound)> {
    let mut sudden = Vec::new();
    for e in enemies {
        let dist = e.feet.distance(me);
        let fired = recently_fired.get(&e.entity).is_some_and(|&t| now - t < 0.3);
        let range = if e.sprinting { FOOTSTEP_RANGE * 1.6 } else { FOOTSTEP_RANGE };
        let footsteps = dist < range && e.vel.length() > u(150.0) && e.stance == Stance::Stand;
        if !(fired && dist < GUNFIRE_RANGE || footsteps) {
            continue;
        }
        let quiet_before = |gap: f32| know.contacts.get(&e.entity).is_none_or(|c| !c.visible && c.age(now) > gap);
        if fired && dist < STARTLE_RANGE && quiet_before(1.0) {
            sudden.push((e.entity, Sound::Gunfire));
        } else if footsteps && !fired && quiet_before(2.0) {
            sudden.push((e.entity, Sound::Footsteps));
        }
        // Rough position: radar dots and sound are imprecise.
        let spread = if fired { u(120.0) } else { u(80.0) };
        let noise = Vec3::new(rng.random_range(-spread..spread), 0.0, rng.random_range(-spread..spread));
        let c = know.contacts.entry(e.entity).or_insert(Contact {
            pos: e.feet + noise,
            vel: Vec3::ZERO,
            time: now,
            source: Source::Heard,
            visible: false,
            awareness: 0.0,
            noticed_at: None,
            shot_us_at: None,
            visible_since: now,
            last_visible: f32::MIN,
        });
        if !c.visible && (c.source != Source::Seen || c.age(now) > 1.0) {
            c.pos = e.feet + noise;
            c.vel = Vec3::ZERO;
            c.time = now;
            c.source = Source::Heard;
        }
    }
    sudden
}

/// We were shot by `attacker` from around `from`.
pub fn shot_by(know: &mut Knowledge, attacker: Entity, from: Vec3, now: f32, rng: &mut impl Rng) {
    let noise = Vec3::new(rng.random_range(-u(60.0)..u(60.0)), 0.0, rng.random_range(-u(60.0)..u(60.0)));
    let c = know.contacts.entry(attacker).or_insert(Contact {
        pos: from + noise,
        vel: Vec3::ZERO,
        time: now,
        source: Source::ShotBy,
        visible: false,
        awareness: 0.0,
        noticed_at: None,
        shot_us_at: None,
        visible_since: now,
            last_visible: f32::MIN,
    });
    c.shot_us_at = Some(now);
    if !c.visible {
        c.pos = from + noise;
        c.time = now;
        c.source = Source::ShotBy;
        // Being hit makes noticing the shooter much quicker.
        c.awareness = c.awareness.max(0.6);
    }
}

/// Take in teammates' callouts.
pub fn hear_callouts(know: &mut Knowledge, team: crate::combat::Team, callouts: &Callouts, now: f32) {
    for &(t, enemy, pos, time) in &callouts.0 {
        if t != team || now - time < CALLOUT_DELAY {
            continue;
        }
        let c = know.contacts.entry(enemy).or_insert(Contact {
            pos,
            vel: Vec3::ZERO,
            time,
            source: Source::Told,
            visible: false,
            awareness: 0.0,
            noticed_at: None,
            shot_us_at: None,
            visible_since: now,
            last_visible: f32::MIN,
        });
        if !c.visible && time > c.time {
            c.pos = pos;
            c.time = time;
            c.source = Source::Told;
        }
    }
}

/// A UAV sweep shows an enemy at `pos`, as the compass does.
pub fn radar(know: &mut Knowledge, enemy: Entity, pos: Vec3, now: f32) {
    let c = know.contacts.entry(enemy).or_insert(Contact {
        pos,
        vel: Vec3::ZERO,
        time: now,
        source: Source::Told,
        visible: false,
        awareness: 0.0,
        noticed_at: None,
        shot_us_at: None,
        visible_since: now,
        last_visible: f32::MIN,
    });
    if !c.visible {
        c.pos = pos;
        c.vel = Vec3::ZERO;
        c.time = now;
        c.source = Source::Told;
    }
}

/// Drop stale and dead contacts.
pub fn forget(know: &mut Knowledge, alive: &[Entity], now: f32) {
    know.contacts.retain(|e, c| alive.contains(e) && (c.visible || c.age(now) < MEMORY));
}

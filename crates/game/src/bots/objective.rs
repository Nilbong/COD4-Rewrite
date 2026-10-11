//! Bots playing the game modes' objectives ([`crate::modes::Objectives`]).
//!
//! Domination: take the flags their team doesn't hold and keep theirs that
//! are under attack, nearest first, spread over them (not one that enough
//! teammates are already on); standing in a flag until it's taken, and
//! looking round it when it's contested by someone out of sight.
//!
//! Search and Destroy: attackers take the bomb to the round's target (the
//! same site for the whole team), the carrier plants it and the rest go
//! with them, the nearest picks it up when it's dropped, and once it's
//! planted they all guard it. Defenders split over the sites and, once it's
//! planted, all go for it, the nearest defusing while the rest cover.

use super::{nav::NavGraph, spaced, Bot, TacCtx};
use crate::combat::Dead;
use crate::modes::{Objectives, UseObjective};
use crate::units::u;
use bevy::prelude::*;
use rand::Rng;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Goal {
    /// A Domination flag (index), and whether it's ours to keep.
    Flag(usize, bool),
    /// Pick up the dropped bomb.
    Pickup(Vec3),
    /// Plant it at this bomb site (index).
    Plant(usize),
    /// Hold around here (a site, the planted bomb).
    Guard(Vec3),
    /// Defuse the planted bomb.
    Defuse(Vec3),
    /// Headquarters: stand in the HQ to take it.
    Hq,
    /// Open a care package there ([`crate::killstreaks::carepackage`]), in
    /// any mode.
    Crate(Vec3),
}

impl Goal {
    /// How much it matters, next to fighting and the rest.
    pub(super) fn weight(self, aggression: f32) -> f32 {
        match self {
            Goal::Flag(_, true) => 1.2,
            Goal::Flag(_, false) => 0.95 + 0.25 * aggression,
            Goal::Pickup(_) => 1.4,
            Goal::Plant(_) => 1.6,
            Goal::Guard(_) => 0.9,
            Goal::Defuse(_) => 1.8,
            Goal::Hq => 1.2,
            Goal::Crate(_) => 1.3,
        }
    }

    /// The same objective as `other` (the bot keeps its place).
    fn same(self, other: Goal) -> bool {
        match (self, other) {
            (Goal::Flag(a, _), Goal::Flag(b, _)) | (Goal::Plant(a), Goal::Plant(b)) => a == b,
            (Goal::Pickup(_), Goal::Pickup(_)) | (Goal::Defuse(_), Goal::Defuse(_)) | (Goal::Hq, Goal::Hq) | (Goal::Crate(_), Goal::Crate(_)) => true,
            (Goal::Guard(a), Goal::Guard(b)) => a.distance(b) < u(400.0),
            _ => false,
        }
    }
}

/// How long a bot leaves an objective it found no route to.
pub(super) const BLOCKED_FOR: f32 = 20.0;

/// Domination, from the demos of real matches (District, Broadcast, Wet
/// Work: `tools/dom_play.py`): players spent about a third of their time
/// within 600 units of a flag but only 2-9% on one, in visits of 2-3 s
/// (median), most of them alone on it. So one bot takes a flag (two keep
/// one under attack) and the rest fight round the flags in play: holding
/// spots near them are worth up to twice as much ([`flag_fit`]), and hunts
/// head for one this often.
pub(super) const HUNT_FLAGS: f32 = 0.6;
const FLAG_NEAR: f32 = u(700.0);
/// Up to this much extra distance per bot and flag in choosing one: a
/// fixed, personal preference (so not everyone goes to the nearest).
const FLAG_LEAN: f32 = u(2600.0);

/// How well `p` suits fighting round the flags: up to 2 by a flag the team
/// doesn't hold (or holds but is losing), down to 0.5 far from them all.
/// With every flag theirs, all of them count.
pub(super) fn flag_fit(o: &Objectives, team: crate::combat::Team, p: Vec3) -> f32 {
    if o.flags.is_empty() {
        return 1.0;
    }
    let live = |f: &&crate::modes::Flag| f.owner != Some(team) || f.contested || f.capture.is_some_and(|(t, _)| t != team);
    let any_live = o.flags.iter().any(|f| live(&f));
    let d = o
        .flags
        .iter()
        .filter(|f| !any_live || live(f))
        .map(|f| Vec2::new(f.pos.x - p.x, f.pos.z - p.z).length())
        .fold(f32::MAX, f32::min);
    0.5 + 1.5 * (-(d / FLAG_NEAR).powi(2)).exp()
}

/// What this bot should be doing for the mode's objective, if anything (not
/// one it couldn't find a way to lately).
pub(super) fn goal(bot: &Bot, me: Entity, feet: Vec3, tc: &TacCtx) -> Option<Goal> {
    let care_package = crate::killstreaks::carepackage::bot_goal(me).map(Goal::Crate);
    care_package.or_else(|| goal_of(bot, me, feet, tc)).filter(|g| !blocked(bot, *g))
}

fn blocked(bot: &Bot, g: Goal) -> bool {
    bot.blocked.iter().any(|b| b.0.same(g))
}

fn goal_of(bot: &Bot, me: Entity, feet: Vec3, tc: &TacCtx) -> Option<Goal> {
    let o = tc.objectives?;
    if !o.flags.is_empty() {
        return flag(bot, me, feet, tc, o);
    }
    if let Some(h) = &o.hq {
        return hq(bot, feet, tc, h);
    }
    if o.sites.iter().any(|s| s.team.is_some()) {
        return sabotage(me, feet, tc, o);
    }
    if o.sites.is_empty() || o.round_over.is_some() {
        return None;
    }
    let bomb = o.bomb.as_ref();
    let planted = bomb.filter(|b| b.planted.is_some()).map(|b| b.pos);
    let live: Vec<usize> = (0..o.sites.len()).filter(|&i| !o.sites[i].destroyed).collect();
    // Nearest of the team to `p`.
    let nearest = |p: Vec3| tc.mates.iter().all(|m| m.feet.distance(p) > feet.distance(p));
    if crate::modes::sd::attacking(tc.team) {
        if let Some(p) = planted {
            return Some(Goal::Guard(p));
        }
        let target = round_target(o, &live)?;
        Some(match bomb {
            Some(b) if b.carrier == Some(me) => Goal::Plant(target),
            Some(b) if b.carrier.is_none() && nearest(b.pos) => Goal::Pickup(b.pos),
            _ => Goal::Guard(o.sites[target].pos),
        })
    } else {
        if let Some(p) = planted {
            return Some(if nearest(p) { Goal::Defuse(p) } else { Goal::Guard(p) });
        }
        // Spread over them (entities come in runs, so mix the bits first).
        let mix = (me.to_bits().wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 33) as usize;
        let i = live.get(mix % live.len().max(1))?;
        Some(Goal::Guard(o.sites[*i].pos))
    }
}

/// Headquarters: take an HQ that isn't ours, a few at a time (more inside
/// take it faster, `koth.gsc`); keep one that is (holders don't respawn).
/// Between HQs there's nothing to go for.
fn hq(bot: &Bot, feet: Vec3, tc: &TacCtx, h: &crate::modes::Hq) -> Option<Goal> {
    if h.owner == Some(tc.team) {
        return Some(Goal::Guard(h.pos));
    }
    if blocked(bot, Goal::Hq) {
        return None;
    }
    let mine = matches!(bot.goal, Some(Goal::Hq)) && bot.mode == super::Mode::Objective;
    let going = tc
        .mates
        .iter()
        .filter(|m| h.contains(m.feet) || (m.mode == super::Mode::Objective && m.dest.is_some_and(|d| h.contains(d))))
        .count();
    let _ = feet;
    (mine || going < 3).then_some(Goal::Hq)
}

/// Sabotage (`sab.gsc`): one bomb for both sides, each side's own site to
/// keep. A loose bomb: the two nearest go for it, the rest close in. Ours:
/// the carrier plants it at their site, the rest go ahead to clear it.
/// Theirs: back to our site. Planted at ours: the nearest defuses, the rest
/// guard; at theirs: guard it.
fn sabotage(me: Entity, feet: Vec3, tc: &TacCtx, o: &Objectives) -> Option<Goal> {
    if o.round_over.is_some() {
        return None;
    }
    let ours = o.sites.iter().position(|s| s.team == Some(tc.team))?;
    let theirs = o.sites.iter().position(|s| s.team == Some(tc.team.other()))?;
    let bomb = o.bomb.as_ref()?;
    let closer = |p: Vec3| tc.mates.iter().filter(|m| m.feet.distance(p) < feet.distance(p)).count();
    if let Some(label) = bomb.planted {
        let at_ours = o.sites[ours].label == label;
        return Some(if at_ours && closer(bomb.pos) == 0 { Goal::Defuse(bomb.pos) } else { Goal::Guard(bomb.pos) });
    }
    Some(match bomb.carrier {
        Some(c) if c == me => Goal::Plant(theirs),
        Some(c) if tc.mates.iter().any(|m| m.entity == c) => Goal::Guard(o.sites[theirs].pos),
        Some(_) => Goal::Guard(o.sites[ours].pos),
        None if closer(bomb.pos) < 2 => Goal::Pickup(bomb.pos),
        None => Goal::Guard(bomb.pos),
    })
}

/// The site attackers go for this round: one still standing, the same for
/// the whole team (picked from the round's end time, fixed for the round).
fn round_target(o: &Objectives, live: &[usize]) -> Option<usize> {
    let seed = o.timer.map_or(0, |(t, _)| t.to_bits()) as usize;
    live.get((seed / 7) % live.len().max(1)).copied()
}

/// Domination: flags we don't hold, or ours being taken, nearest first;
/// not one enough teammates are already on or heading for (one to take it,
/// two to keep it: real players were mostly alone on a flag; one more for
/// every six on the team).
fn flag(bot: &Bot, me: Entity, feet: Vec3, tc: &TacCtx, o: &Objectives) -> Option<Goal> {
    let current = match bot.goal {
        Some(Goal::Flag(i, _)) if bot.mode == super::Mode::Objective => Some(i),
        _ => None,
    };
    o.flags
        .iter()
        .enumerate()
        .filter_map(|(i, f)| {
            let ours = f.owner == Some(tc.team);
            let attacked = f.capture.is_some_and(|(t, _)| t != tc.team) || (ours && f.contested);
            if (ours && !attacked) || blocked(bot, Goal::Flag(i, ours)) {
                return None;
            }
            let on_it = tc
                .mates
                .iter()
                .filter(|m| {
                    m.feet.distance(f.pos) < f.radius * 1.5
                        || (m.mode == super::Mode::Objective && m.dest.is_some_and(|d| d.distance(f.pos) < f.radius * 2.0))
                })
                .count();
            let mine = current == Some(i);
            // Big teams send more: alone, the one on its way to a flag in
            // the thick of it (Wet Work's B with ten a side) keeps dying
            // there and it never falls, where real players kept taking it.
            // Bot lab experiment `crew`: one more (more inside take a flag
            // faster, `dom.gsc`).
            let crew = if attacked { 2 } else { 1 } + (tc.mates.len() + 1) / 6 + usize::from(super::lab::on("crew"));
            if !mine && on_it >= crew {
                return None;
            }
            // Each player's own lean towards some flags (real players spread
            // over all three: Vacant's demo 30/39/32%, where nearest-first
            // bots crowded the middle one 70%).
            let lean = {
                let h = (me.to_bits() ^ (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
                ((h >> 40) as f32 / (1u64 << 24) as f32) * FLAG_LEAN
            };
            let cost = feet.distance(f.pos) + lean + on_it as f32 * u(600.0)
                - if attacked { u(800.0) } else { 0.0 }
                - if mine { u(500.0) } else { 0.0 };
            Some((Goal::Flag(i, ours), cost))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(g, _)| g)
}

/// Set off for (or carry on with) `goal`: a new place to go when it's a
/// different objective, or when the bot is there but needs to be elsewhere
/// in it (outside a flag on another floor, a contested flag to search).
#[allow(clippy::too_many_arguments)]
pub(super) fn pursue(bot: &mut Bot, goal: Goal, tc: &TacCtx, nav: Option<&NavGraph>, feet: Vec3, now: f32, rng: &mut impl Rng) {
    // A care package: straight there (no mode's objectives needed).
    if let Goal::Crate(p) = goal {
        let changed = bot.mode != super::Mode::Objective || !bot.goal.is_some_and(|g| g.same(goal));
        bot.goal = Some(goal);
        if changed || (bot.dest.is_none() && feet.distance(p) > u(40.0)) {
            bot.set_mode(super::Mode::Objective, now);
            bot.hold = None;
            bot.go_to(p);
        }
        return;
    }
    let o = tc.objectives.expect("objectives");
    let there = bot.mode == super::Mode::Objective && bot.dest.is_none();
    let redo = there
        && match goal {
            Goal::Flag(i, _) => !o.flags[i].contains(feet + Vec3::Y * u(1.0)) || (o.flags[i].contested && now > bot.hold_until),
            Goal::Plant(i) => !o.sites[i].contains(feet),
            Goal::Pickup(p) => !crate::modes::sd::in_pickup_reach(feet, p),
            Goal::Defuse(p) => feet.distance(p) > u(48.0),
            // (Respawned for a new round with the same goal, say.)
            Goal::Guard(p) => feet.distance(p) > u(700.0),
            Goal::Hq => !o.hq.as_ref().is_some_and(|h| h.contains(feet)),
            Goal::Crate(_) => false,
        };
    bot.blocked.retain(|b| b.1 > now);
    let changed = bot.mode != super::Mode::Objective || !bot.goal.is_some_and(|g| g.same(goal));
    bot.goal = Some(goal);
    if !changed && !redo {
        return;
    }
    bot.set_mode(super::Mode::Objective, now);
    bot.hold = None;
    let avoid = tc.mate_points();
    let around = |rng: &mut dyn rand::RngCore, centre: Vec3, near: f32, far: f32| {
        let a = rng.random_range(0.0..std::f32::consts::TAU);
        centre + Vec3::new(a.cos(), 0.0, a.sin()) * rng.random_range(near..far)
    };
    // Somewhere inside `inside`, near `centre` (the nav point near a random
    // spot there can be on another floor).
    let mut within = |centre: Vec3, reach: f32, inside: &dyn Fn(Vec3) -> bool| {
        (0..6).map(|_| spaced(nav, around(rng, centre, 0.0, reach), &avoid)).find(|p| inside(*p)).unwrap_or(centre)
    };
    let spot = match goal {
        Goal::Flag(i, _) => {
            let f = &o.flags[i];
            within(f.pos, f.radius * 0.55, &|p| f.contains(p + Vec3::Y * u(1.0)))
        }
        Goal::Plant(i) => {
            let s = &o.sites[i];
            within(s.pos, u(80.0), &|p| s.contains(p))
        }
        Goal::Pickup(p) | Goal::Defuse(p) | Goal::Crate(p) => p,
        // A walkable point inside the HQ's box, at whatever height (random
        // spots near the radio snapped to the ground floor below one
        // upstairs: Crash), spread from teammates; else the radio.
        Goal::Hq => match &o.hq {
            Some(h) => {
                let inside: Vec<Vec3> = nav
                    .map(|n| n.nodes.iter().map(|n| n.pos).filter(|p| h.contains(*p)).collect())
                    .unwrap_or_default();
                let free: Vec<Vec3> = inside.iter().copied().filter(|p| avoid.iter().all(|m| m.distance(*p) > u(60.0))).collect();
                let pick = if free.is_empty() { &inside } else { &free };
                if pick.is_empty() { h.pos } else { pick[rng.random_range(0..pick.len())] }
            }
            None => feet,
        },
        // One of the map's holding spots near it (open ground or a corner
        // picked at random gets players stuck), else somewhere around it.
        Goal::Guard(p) => {
            let spots: Vec<Vec3> = tc
                .tactics
                .map(|t| {
                    t.spots
                        .iter()
                        .enumerate()
                        .filter(|(k, s)| !tc.taken.contains(k) && (u(120.0)..u(800.0)).contains(&s.pos.distance(p)))
                        .map(|(_, s)| s.pos)
                        .filter(|s| avoid.iter().all(|m| m.distance(*s) > u(150.0)))
                        .collect()
                })
                .unwrap_or_default();
            match spots.len() {
                0 => within(p, u(450.0), &|q| q.distance(p) > u(120.0)),
                n => spots[rng.random_range(0..n)],
            }
        }
    };
    bot.go_to(spot);
}

/// Hold "use" to plant (the carrier in its site) or defuse (by the bomb),
/// while nobody's in sight.
pub(super) fn use_objectives(
    objectives: Option<Res<Objectives>>,
    mut bots: Query<(Entity, &Bot, &Transform, Option<&mut UseObjective>, Has<crate::perks::Downed>), Without<Dead>>,
) {
    let Some(o) = objectives else { return };
    for (me, bot, tf, using, downed) in &mut bots {
        let Some(mut using) = using else { continue };
        let feet = tf.translation;
        // Downed (Last Stand) can't touch the bomb.
        let calm = !bot.know.contacts.values().any(|c| c.noticed()) && !downed;
        let carrying = o.bomb.as_ref().is_some_and(|b| b.carrier == Some(me));
        let want = calm
            && bot.mode == super::Mode::Objective
            && match bot.goal {
                Some(Goal::Plant(i)) => carrying && o.sites.get(i).is_some_and(|s| s.contains(feet)),
                Some(Goal::Defuse(p)) => feet.distance(p) < u(60.0),
                _ => false,
            };
        if using.0 != want {
            using.0 = want;
        }
    }
}

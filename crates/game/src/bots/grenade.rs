//! Bots' grenades: now and then a frag at an enemy they know is just behind
//! cover (out of sight a moment ago, or heard), and a flashbang or stun
//! grenade before pushing in on one round a corner; thrown on an arc that
//! gets there with nothing in the way, frags cooked by better players so
//! they go off as they land. And running from a live frag that lands near
//! them, if they see it in time.

use super::aim::{AimGoal, AimState};
use super::nav::NavGraph;
use super::{keys_for, line_clear, straight_walk, Bot, Plan};
use crate::collision;
use crate::combat::{Dead, Pawn};
use crate::grenades::{GrenadeDefs, GrenadeInput, Grenades, Kind, LiveGrenade, Offhand, Phase};
use crate::movement::{Mover, Stance, ViewAngles};
use crate::units::u;
use avian3d::prelude::*;
use bevy::prelude::*;
use rand::Rng;

/// Gravity on grenades in flight (CoD units/s², as `grenades` flies them).
const GRAVITY: f32 = 800.0;
/// How close to the enemy a throw has to come down (CoD units).
const NEAR_ENOUGH: f32 = 96.0;
/// Out of sight this recently (seconds): they're still about there.
const FRESH: f32 = 4.0;
/// Throwing range, CoD units.
const RANGE: (f32, f32) = (350.0, 1500.0);

/// A throw under way.
pub(super) struct Nade {
    kind: Kind,
    /// The view to throw along.
    angles: Vec2,
    /// Seconds to hold it once the pin is out.
    cook: f32,
    started: f32,
    /// When the button went down.
    pressed: Option<f32>,
    released: bool,
}

/// Getting away from a live grenade.
pub(super) struct Flee {
    to: Vec3,
    /// From when (after noticing it), until when.
    from: f32,
    until: f32,
}

/// The view (yaw, pitch) that brings a grenade thrown from `eye` down within
/// reach of `target` with nothing in the way (the flattest throw that does),
/// and how long it flies. `speed` and `speed_up` are the weapon's
/// `iProjectileSpeed` and `iProjectileSpeedUp`.
pub(super) fn aim(spatial: &SpatialQuery, eye: Vec3, target: Vec3, speed: f32, speed_up: f32) -> Option<(Vec2, f32)> {
    let to = target - eye;
    let yaw = f32::atan2(-to.x, -to.z);
    let filter = collision::sight_filter();
    let mut best: Option<(Vec2, f32, f32)> = None;
    for k in 0..=35 {
        let pitch = (-10.0 + 2.0 * k as f32).to_radians();
        let fwd = Vec3::new(-yaw.sin() * pitch.cos(), pitch.sin(), -yaw.cos() * pitch.cos());
        let mut p = eye + fwd * u(16.0);
        let mut v = fwd * u(speed) + Vec3::Y * u(speed_up);
        let (dt, mut t) = (0.03, 0.0);
        let landed = loop {
            v.y -= u(GRAVITY) * dt;
            let step = v * dt;
            if let Some(hit) = Dir3::new(step).ok().and_then(|d| spatial.cast_ray(p, d, step.length(), true, &filter)) {
                break Some((p + step.normalize() * hit.distance, t));
            }
            p += step;
            t += dt;
            if p.y < target.y - u(200.0) || t > 3.0 {
                break None;
            }
        };
        let Some((at, t)) = landed else { continue };
        let miss = Vec2::new(at.x - target.x, at.z - target.z).length() + (at.y - target.y).abs();
        if miss < u(NEAR_ENOUGH * 0.6) {
            return Some((Vec2::new(yaw, pitch), t));
        }
        if miss < u(NEAR_ENOUGH) && best.is_none_or(|b| miss < b.2) {
            best = Some((Vec2::new(yaw, pitch), t, miss));
        }
    }
    best.map(|(a, t, _)| (a, t))
}

/// Somewhere to run from a grenade at `grenade`: the nearest nav point out
/// of its `radius` (or out of its sight) we can run straight to; else just
/// away from it.
fn refuge(spatial: &SpatialQuery, nav: Option<&NavGraph>, feet: Vec3, grenade: Vec3, radius: f32) -> Vec3 {
    let away = Vec3::new(feet.x - grenade.x, 0.0, feet.z - grenade.z).normalize_or_zero();
    let mut spots: Vec<Vec3> = nav
        .map(|nav| nav.within(feet, radius + u(300.0)).into_iter().map(|i| nav.nodes[i as usize].pos).collect())
        .unwrap_or_default();
    spots.retain(|p| {
        (p.y - feet.y).abs() < u(40.0)
            && (p.distance(grenade) > radius + u(40.0) || !line_clear(spatial, grenade + Vec3::Y * u(8.0), *p + Vec3::Y * u(30.0)))
    });
    spots.sort_by(|a, b| a.distance(feet).total_cmp(&b.distance(feet)));
    spots.into_iter().take(12).find(|p| straight_walk(spatial, feet, *p)).unwrap_or(feet + away * radius)
}

/// Decide on and carry out bots' throws, and notice grenades to run from.
/// The throw itself goes through [`GrenadeInput`], like anyone's.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub(super) fn plan_grenades(
    time: Res<Time>,
    spatial: SpatialQuery,
    defs: Res<GrenadeDefs>,
    nav: Option<Res<NavGraph>>,
    mut bots: Query<
        (Entity, &mut Bot, &Pawn, &Transform, &Mover, &ViewAngles, &Grenades, Option<&Offhand>, &mut GrenadeInput),
        Without<Dead>,
    >,
    pawns: Query<(Entity, &Pawn, &Transform, Has<Dead>)>,
    live: Query<(Entity, &LiveGrenade, &Transform)>,
) {
    let Some(stats) = defs.get(Kind::Frag) else { return };
    let now = time.elapsed_secs();
    let mut rng = rand::rng();
    let radius = u(stats.radius);
    let sim = std::env::var_os("COD4RW_SIM").is_some();
    for (me, mut bot, pawn, tf, mover, view, nades, offhand, mut input) in &mut bots {
        let bot = &mut *bot;
        let feet = tf.translation;
        let eye = mover.eye(feet);

        // A live frag near us that would hurt us (ours, or an enemy's).
        if bot.flee.as_ref().is_some_and(|f| now > f.until) {
            bot.flee = None;
        }
        if bot.flee.is_none() {
            for (ge, g, gtf) in &live {
                if g.kind != Kind::Frag || bot.flee_seen == Some(ge) {
                    continue;
                }
                let friendly = g.thrower != me && pawns.get(g.thrower).is_ok_and(|p| !crate::combat::hostile(p.1, pawn));
                let at = gtf.translation;
                // Once it's come down (not on its way out of our own hand).
                let landed = g.resting || g.velocity.length() < u(200.0);
                if friendly || !landed || at.distance(feet) > radius + u(40.0) || g.explode_at - now > 3.0 {
                    continue;
                }
                // Seen, or heard landing right by us.
                if at.distance(feet) > u(150.0) && !line_clear(&spatial, eye, at + Vec3::Y * u(4.0)) {
                    continue;
                }
                bot.flee_seen = Some(ge);
                // Not everyone notices in time (but they know where their
                // own went).
                if g.thrower != me && rng.random::<f32>() > 0.5 + 0.45 * bot.skill {
                    continue;
                }
                let to = refuge(&spatial, nav.as_deref(), feet, at, radius);
                bot.flee = Some(Flee { to, from: now + bot.reaction, until: g.explode_at + 0.2 });
                if sim {
                    info!("grenade: {} runs from one {:.0}u away", pawn.name, at.distance(feet) / u(1.0));
                }
                break;
            }
        }

        // A throw under way: line it up, pull the pin, cook, let go, and
        // keep aiming until it's gone.
        if let Some(n) = bot.nade.as_mut() {
            let err = AimState::error(view, n.angles).length();
            let fuse = defs.get(n.kind).map_or(0.0, |s| s.fuse);
            match (n.pressed, n.released) {
                (None, _) => {
                    if err < 0.04 {
                        n.pressed = Some(now);
                        hold(&mut input, n.kind, true);
                    } else if now - n.started > 1.5 {
                        bot.nade = None;
                    }
                }
                (Some(at), false) => {
                    hold(&mut input, n.kind, true);
                    let cooked = offhand
                        .filter(|o| o.phase == Phase::Hold)
                        .is_some_and(|o| o.explode_at.is_none_or(|x| x - now <= fuse - n.cook));
                    if cooked && err < 0.06 {
                        hold(&mut input, n.kind, false);
                        n.released = true;
                    } else if offhand.is_none() && now - at > 0.6 {
                        hold(&mut input, n.kind, false);
                        bot.nade = None;
                    }
                }
                (_, true) => {
                    hold(&mut input, n.kind, false);
                    if offhand.is_none_or(|o| o.phase == Phase::Raise) {
                        bot.nade = None;
                    }
                }
            }
            continue;
        }
        input.frag = false;
        input.special = false;
        if offhand.is_some() || now < bot.next_nade || bot.flee.is_some() {
            continue;
        }
        bot.next_nade = now + 0.5;
        let skill = bot.skill;
        let mut choice = None;
        // Smoke: between us and someone shooting at us from far off while we
        // get to cover, or over a bomb site we're planting or defusing at.
        if nades.special == Some(Kind::Smoke) && nades.specials > 0 {
            let shot_from_afar = bot.mode == super::Mode::Cover
                && bot.know.contacts.values().any(|c| c.shot_us_at.is_some_and(|t| now - t < 2.0) && c.pos.distance(feet) > u(800.0));
            let at_bomb = bot.mode == super::Mode::Objective
                && matches!(bot.goal, Some(super::objective::Goal::Plant(_)) | Some(super::objective::Goal::Defuse(_)))
                && bot.dest.is_none_or(|d| d.distance(feet) < u(400.0));
            let threat = bot
                .know
                .contacts
                .values()
                .filter(|c| c.age(now) < 6.0 && (u(600.0)..u(3000.0)).contains(&c.pos.distance(feet)))
                .map(|c| c.pos)
                .min_by(|a, b| a.distance(feet).total_cmp(&b.distance(feet)));
            if let (true, Some(t)) = (shot_from_afar || at_bomb, threat) {
                if rng.random::<f32>() < 0.5 + 0.4 * skill {
                    let towards = (t - feet).normalize_or_zero();
                    let reach = (t.distance(feet) * 0.35).clamp(u(300.0), u(700.0));
                    choice = Some((Kind::Smoke, feet + towards * reach));
                }
            }
        }
        // Otherwise someone in view gets shot, not a grenade.
        if choice.is_none() && bot.know.contacts.values().any(|c| c.noticed()) {
            continue;
        }
        // The nearest enemy we know is just out of sight within a range.
        let known = |near: f32, far: f32| {
            bot.know
                .contacts
                .values()
                .filter(|c| !c.visible && c.age(now) < FRESH)
                .map(|c| c.pos)
                .filter(|p| (near..far).contains(&p.distance(feet)))
                .min_by(|a, b| a.distance(feet).total_cmp(&b.distance(feet)))
        };
        let aggression = bot.personality.aggression;
        // Pushing in on someone just round a corner: a flashbang or stun
        // first. Otherwise, now and then, a frag (livelier and better
        // players reach for one more often; never so near we'd catch it).
        let pushing = matches!(bot.mode, super::Mode::Investigate | super::Mode::Flank);
        let special = nades.special.filter(|k| matches!(k, Kind::Flash | Kind::Stun) && nades.specials > 0);
        if let (true, Some(kind), Some(t)) = (choice.is_none() && pushing, special, known(u(250.0), u(900.0))) {
            if rng.random::<f32>() < (0.08 + 0.12 * aggression) * (0.5 + skill) {
                choice = Some((kind, t));
            }
        }
        if choice.is_none() && nades.frags > 0 {
            if let Some(t) = known(u(RANGE.0).max(radius + u(60.0)), u(RANGE.1)) {
                if rng.random::<f32>() < (0.02 + 0.05 * aggression) * (0.5 + skill) {
                    choice = Some((Kind::Frag, t));
                }
            }
        }
        let Some((kind, target)) = choice else { continue };
        let Some(kstats) = defs.get(kind) else { continue };
        // A teammate already there: no.
        if pawns.iter().any(|(e, p, ptf, dead)| e != me && !dead && !crate::combat::hostile(p, pawn) && ptf.translation.distance(target) < radius) {
            continue;
        }
        // Where they guess the enemy is, the better the player the closer.
        let a = rng.random_range(0.0..std::f32::consts::TAU);
        let off = super::gaussian(&mut rng).abs() * u(40.0 + 120.0 * (1.0 - skill));
        let target = target + Vec3::new(a.cos(), 0.0, a.sin()) * off;
        let Some((angles, flight)) = aim(&spatial, eye, target + Vec3::Y * u(8.0), kstats.speed, kstats.speed_up) else { continue };
        // Better players cook a frag so it goes off about as it lands.
        let spare = (kstats.fuse - flight - 0.5).max(0.0);
        let cook = if kstats.cook && rng.random::<f32>() < skill { rng.random_range(0.3..1.0) * spare } else { 0.0 };
        bot.nade = Some(Nade { kind, angles, cook, started: now, pressed: None, released: false });
        bot.next_nade = now + 8.0;
        if sim {
            info!(
                "grenade: {} throws a {} at someone {:.0}u away (pitch {:.0}, flight {:.1}s, cooked {:.1}s)",
                pawn.name,
                kind.display(),
                target.distance(feet) / u(1.0),
                angles.y.to_degrees(),
                flight,
                cook
            );
        }
    }
}

/// Hold (or let go of) the button for a grenade of `kind`.
fn hold(input: &mut GrenadeInput, kind: Kind, on: bool) {
    input.frag = on && kind == Kind::Frag;
    input.special = on && kind != Kind::Frag;
}

/// With `COD4RW_SIM`: kills by weapon (and frags' own), every 30 s.
pub(super) fn tally(
    time: Res<Time>,
    mut damage: MessageReader<crate::combat::Damage>,
    mut killed: MessageReader<crate::combat::Killed>,
    mut by: Local<std::collections::BTreeMap<&'static str, u32>>,
    mut counts: Local<(u32, u32, f32)>,
    mut hit: Local<std::collections::HashMap<Entity, (f32, &'static str)>>,
) {
    let now = time.elapsed_secs();
    for d in damage.read() {
        hit.insert(d.target, (now, d.weapon));
    }
    for k in killed.read() {
        counts.0 += 1;
        if let Some(&(t, weapon)) = hit.get(&k.victim).filter(|h| now - h.0 < 0.3) {
            let _ = t;
            *by.entry(weapon).or_default() += 1;
            counts.1 += (weapon == Kind::Frag.display() && k.attacker == Some(k.victim)) as u32;
        }
    }
    if now >= counts.2 {
        counts.2 = now + 30.0;
        let mut top: Vec<_> = by.iter().map(|(w, n)| (*n, *w)).collect();
        top.sort_by(|a, b| b.cmp(a));
        let list: Vec<String> = top.iter().map(|(n, w)| format!("{w} {n}")).collect();
        info!("kills: {} deaths ({} by their own frag): {}", counts.0, counts.1, list.join(", "));
    }
}

/// This frame's plan with any throw or escape in it: aim along a throw (no
/// shooting meanwhile); run for it from a grenade, facing the way out of
/// a fight.
pub(super) fn steer(bot: &mut Bot, plan: &mut Plan, feet: Vec3, eye: Vec3, view: &ViewAngles, now: f32) {
    if let Some(n) = &bot.nade {
        plan.look = Some(AimGoal { angles: n.angles, width: 0.02, combat: true });
        plan.turn = None;
        plan.pitch = None;
        plan.fire = false;
        plan.ads = false;
        bot.look_src = "grenade";
    }
    let Some(f) = bot.flee.as_ref().filter(|f| now >= f.from) else { return };
    let dir = Vec3::new(f.to.x - feet.x, 0.0, f.to.z - feet.z);
    if dir.length() < u(16.0) {
        plan.forward = 0.0;
        plan.right = 0.0;
        return;
    }
    let dir = dir.normalize();
    // Out of a fight: turn and sprint for it.
    let fighting = bot.know.contacts.values().any(|c| c.noticed());
    if !fighting && bot.nade.is_none() {
        let to = Vec3::new(f.to.x, eye.y, f.to.z);
        plan.look = Some(AimGoal { angles: super::angles_to(eye, to), width: 0.1, combat: false });
        plan.turn = None;
        plan.pitch = None;
        bot.look_src = "flee";
    }
    let (forward, right) = keys_for(dir, view.yaw);
    plan.forward = forward;
    plan.right = right;
    plan.sprint = forward > 0.5 && right == 0.0;
    plan.stance = Stance::Stand;
    plan.jump = false;
}

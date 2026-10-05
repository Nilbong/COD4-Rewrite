//! Bots' perk-1 equipment, the way players use it: a claymore set down at
//! the spot they hold, facing the way they watch; an RPG fired at an enemy
//! helicopter, or at someone they've just spotted at range; C4 thrown at
//! someone they know is round a corner and set off a moment later. Each is
//! switched to (the extra slot, through [`SwitchInput`] as the player's 5),
//! used, and put away. And an enemy helicopter in sight gets shot at with
//! whatever's in hand when nobody on the ground is close, and enemy
//! claymores they spot (through walls with Bomb Squad) are routed round.

use super::aim::{AimGoal, AimState};
use super::{angles_to, Bot, Mode, Plan};
use crate::combat::Dead;
use crate::collision;
use crate::combat::Pawn;
use crate::explosives::{Explosive, ExplosiveDefs};
use crate::killstreaks::helicopter::Helicopter;
use crate::loadout::{Loadout, SwitchInput};
use crate::movement::{Mover, ViewAngles};
use crate::units::u;
use crate::weapons::WeaponState;
use avian3d::prelude::*;
use bevy::prelude::*;
use rand::Rng;

/// Gives up on a use that hasn't happened in this long (seconds).
const TIMEOUT: f32 = 6.0;
/// No second claymore within this of the last (CoD units).
const CLAYMORE_SPACING: f32 = 500.0;

#[derive(Clone, Copy, Debug)]
enum Use {
    /// Set a claymore down facing this way.
    Claymore(Vec2),
    /// A rocket at them.
    Rocket(Entity),
    /// A rocket at the enemy helicopter.
    RocketHeli,
    /// Throw C4 along this view, then set it off.
    C4(Vec2),
}

/// Equipment being used.
pub(super) struct Gear {
    what: Use,
    started: f32,
    /// Its rounds when we started, to tell when it's gone.
    ammo: u32,
    /// In hand and ready.
    ready: bool,
    /// Gone (planted, fired, thrown) at.
    used: Option<f32>,
    /// Set C4 off this frame.
    detonate: bool,
}

/// Decide on and carry out equipment use: switching to it and back. The
/// aiming and the trigger are in [`steer`].
#[allow(clippy::type_complexity)]
pub(super) fn plan_gear(
    time: Res<Time>,
    spatial: SpatialQuery,
    defs: Res<ExplosiveDefs>,
    mut bots: Query<(Entity, &mut Bot, &Pawn, &Transform, &Mover, Option<&Loadout>, &WeaponState, Option<&mut SwitchInput>), Without<Dead>>,
    helis: Query<(&Helicopter, &Transform), Without<Pawn>>,
    explosives: Query<(Entity, &Explosive, &Transform), Without<Pawn>>,
    owners: Query<&Pawn>,
) {
    let now = time.elapsed_secs();
    let mut rng = rand::rng();
    let sim = std::env::var_os("COD4RW_SIM").is_some();
    let sight = collision::sight_filter();
    for (me, mut bot, pawn, tf, mover, loadout, weapon, switch) in &mut bots {
        let bot = &mut *bot;
        // An enemy helicopter in sight.
        let eye = mover.eye(tf.translation);
        bot.heli = helis
            .iter()
            .filter(|(h, _)| h.owner != me && (h.team != pawn.team || crate::combat::free_for_all()))
            .map(|(_, htf)| htf.translation - Vec3::Y * u(110.0))
            .filter(|p| p.distance(eye) < u(4000.0))
            .filter(|p| Dir3::new(*p - eye).ok().is_none_or(|d| spatial.cast_ray(eye, d, p.distance(eye) - u(120.0), true, &sight).is_none()))
            .min_by(|a, b| a.distance(eye).total_cmp(&b.distance(eye)));
        // Enemy claymores: spotted (better players more surely), or known
        // through walls with Bomb Squad; forgotten once gone.
        bot.claymores.retain(|(e, _, _)| explosives.contains(*e));
        bot.claymore_checked.retain(|e| explosives.contains(*e));
        let bomb_squad = loadout.is_some_and(|l| l.class.perks.iter().any(|p| p == "specialty_detectexplosive"));
        for (ce, ex, ctf) in &explosives {
            if ex.weapon != "claymore_mp" || bot.claymores.iter().any(|c| c.0 == ce) || bot.claymore_checked.contains(&ce) {
                continue;
            }
            let Ok(owner) = owners.get(ex.owner) else { continue };
            let at = ctf.translation;
            if !crate::combat::hostile(owner, pawn) || at.distance(eye) > u(if bomb_squad { 1500.0 } else { 700.0 }) {
                continue;
            }
            let seen = bomb_squad
                || Dir3::new(at - eye).ok().is_some_and(|d| spatial.cast_ray(eye, d, at.distance(eye) - u(6.0), true, &sight).is_none());
            if !seen {
                continue;
            }
            bot.claymore_checked.push(ce);
            if bomb_squad || rng.random::<f32>() < 0.4 + 0.5 * bot.skill {
                bot.claymores.push((ce, at, ctf.rotation * Vec3::X));
                bot.next_repath = now;
                if sim {
                    info!("equipment: {} spots a claymore {:.0}u away", pawn.name, at.distance(eye) / u(1.0));
                }
            }
        }
        let (Some(loadout), Some(mut switch)) = (loadout, switch) else { continue };
        let (Some(slot), Some((def, ammo))) = (loadout.extra_slot(), loadout.equipment(weapon)) else {
            bot.gear = None;
            continue;
        };
        let in_hand = loadout.current == slot && loadout.switching.is_none() && std::ptr::eq(weapon.def, def);
        if let Some(g) = bot.gear.as_mut() {
            g.ready = in_hand;
            g.detonate = false;
            if g.used.is_none() && ammo < g.ammo {
                g.used = Some(now);
            }
            let done = match (g.what, g.used) {
                // C4: set it off once it's had time to land.
                (Use::C4(_), Some(at)) if now - at > 1.4 => {
                    g.detonate = in_hand;
                    now - at > 1.6
                }
                (Use::C4(_), _) => false,
                (_, Some(at)) => now - at > 0.4,
                _ => false,
            };
            if done || now - g.started > TIMEOUT {
                bot.gear = None;
                switch.to = Some(0);
            } else if g.used.is_none() && !in_hand && loadout.current != slot && loadout.switching.is_none() {
                switch.to = Some(slot);
            }
            continue;
        }
        if loadout.current == slot && loadout.switching.is_none() {
            // Left in hand: back to the gun.
            switch.to = Some(0);
            continue;
        }
        if ammo == 0 || now < bot.next_gear {
            continue;
        }
        bot.next_gear = now + 0.5;
        let feet = tf.translation;
        let fighting = bot.know.contacts.values().any(|c| c.noticed());
        let what = match def.name.as_str() {
            // Settled where we hold, watching a way in.
            "claymore_mp" => {
                let settled = matches!(bot.mode, Mode::Post | Mode::Hold) && bot.dest.is_none() && !fighting;
                let watch = bot.hold.as_ref().and_then(|h| h.angles.get(h.angle_i)).copied();
                let fresh_spot = bot.claymore_at.is_none_or(|p| p.distance(feet) > u(CLAYMORE_SPACING));
                match (settled && fresh_spot, watch) {
                    (true, Some(w)) => {
                        let a = angles_to(eye, w);
                        Some(Use::Claymore(Vec2::new(a.x, (-25f32).to_radians())))
                    }
                    _ => None,
                }
            }
            // The enemy's helicopter; else someone just spotted, far enough
            // off to be worth a rocket.
            "rpg_mp" if bot.heli.is_some() => Some(Use::RocketHeli),
            "rpg_mp" => bot.engagement.as_ref().and_then(|eng| {
                let c = bot.know.contacts.get(&eng.target)?;
                let d = c.pos.distance(feet);
                let fresh = eng.first_shot.is_none() && c.noticed();
                (fresh && (u(600.0)..u(2500.0)).contains(&d) && rng.random::<f32>() < 0.15).then_some(Use::Rocket(eng.target))
            }),
            // Someone just round a corner, not too far.
            "c4_mp" if !fighting => {
                let target = bot
                    .know
                    .contacts
                    .values()
                    .filter(|c| !c.visible && c.age(now) < 4.0)
                    .map(|c| c.pos)
                    .filter(|p| (u(300.0)..u(900.0)).contains(&p.distance(feet)))
                    .min_by(|a, b| a.distance(feet).total_cmp(&b.distance(feet)));
                let throw = defs.get("c4_mp").map_or((500.0, 0.0), |d| (d.speed, d.speed_up));
                target
                    .filter(|_| rng.random::<f32>() < 0.08 + 0.1 * bot.personality.aggression)
                    .and_then(|t| super::grenade::aim(&spatial, eye, t + Vec3::Y * u(8.0), throw.0, throw.1))
                    .map(|(angles, _)| Use::C4(angles))
            }
            _ => None,
        };
        let Some(what) = what else { continue };
        if let Use::Claymore(_) = what {
            bot.claymore_at = Some(feet);
        }
        if sim {
            info!("equipment: {:?} with {}", what, def.name);
        }
        bot.gear = Some(Gear { what, started: now, ammo, ready: false, used: None, detonate: false });
        switch.to = Some(slot);
    }
}

/// This frame's plan with equipment in it: aim it and use it once it's in
/// hand; nothing else fires or aims down sights meanwhile (but C4 goes off
/// by aiming with it in hand).
pub(super) fn steer(bot: &mut Bot, plan: &mut Plan, view: &ViewAngles, eye: Vec3, now: f32) {
    let Some(g) = &bot.gear else { return };
    plan.fire = false;
    plan.ads = g.detonate;
    plan.sprint = false;
    let angles = match g.what {
        Use::Claymore(a) | Use::C4(a) => a,
        Use::Rocket(target) => match bot.know.contacts.get(&target) {
            Some(c) => angles_to(eye, c.predicted(now) + Vec3::Y * u(40.0)),
            None => return,
        },
        Use::RocketHeli => match bot.heli {
            Some(p) => angles_to(eye, p),
            None => return,
        },
    };
    if g.used.is_none() {
        plan.look = Some(AimGoal { angles, width: 0.03, combat: true });
        plan.turn = None;
        plan.pitch = None;
        bot.look_src = "equipment";
        plan.fire = g.ready && AimState::error(view, angles).length() < 0.05;
    }
}

/// An enemy helicopter in sight and nobody on the ground close by: shoot it
/// (bursts, sights up at range), as players do.
pub(super) fn shoot_heli(bot: &mut Bot, plan: &mut Plan, view: &ViewAngles, eye: Vec3, feet: Vec3, now: f32, rng: &mut impl Rng) {
    let Some(heli) = bot.heli.filter(|_| bot.gear.is_none() && bot.nade.is_none()) else { return };
    let close = bot.know.contacts.values().any(|c| c.noticed() && c.pos.distance(feet) < u(1500.0));
    if close {
        return;
    }
    let angles = angles_to(eye, heli);
    // About a fuselage wide.
    let width = 2.0 * (u(150.0) / heli.distance(eye).max(u(100.0))).atan();
    plan.look = Some(AimGoal { angles, width, combat: true });
    plan.turn = None;
    plan.pitch = None;
    bot.look_src = "heli";
    plan.ads = heli.distance(eye) > u(1200.0);
    if now >= bot.burst_until && now >= bot.next_burst {
        bot.burst_until = now + rng.random_range(0.3..0.8);
        bot.next_burst = bot.burst_until + rng.random_range(0.2..0.5);
    }
    plan.fire = now < bot.burst_until && AimState::error(view, angles).length() < width;
}

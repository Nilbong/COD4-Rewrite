//! Bots and kill streaks: a UAV, helicopter, care package or sentry gun
//! called in once they're out of a fight (the sentry planted in front of
//! them, the care package's crate opened once it's down:
//! [`crate::killstreaks::carepackage`] sends them to it), an airstrike on
//! where they know enemies are bunched up; and
//! while their team's UAV is up, knowing where every enemy was at each
//! sweep, as players see them on the compass.

use super::perception;
use super::Bot;
use crate::combat::{Dead, Pawn};
use crate::killstreaks::uav::Radar;
use crate::killstreaks::carepackage::UseCrate;
use crate::killstreaks::{Hardpoint, HardpointInput, Killstreak};
use crate::units::u;
use bevy::prelude::*;
use rand::Rng;
use std::collections::HashMap;

/// Enemies within this of each other count as bunched up (CoD units).
const BUNCHED: f32 = 450.0;
/// Never an airstrike this near ourselves (CoD units).
const DANGER_CLOSE: f32 = 800.0;
/// Waiting this long for two enemies together, then one will do.
const WAIT_FOR_TWO: f32 = 15.0;

#[allow(clippy::type_complexity)]
pub(super) fn hardpoints(
    mut commands: Commands,
    time: Res<Time>,
    radar: Option<Res<Radar>>,
    mut bots: Query<(Entity, &mut Bot, &Pawn, &Transform, &Killstreak, &mut HardpointInput, Option<&mut UseCrate>), Without<Dead>>,
    pawns: Query<(Entity, &Pawn, &Transform, Option<&crate::loadout::Loadout>), Without<Dead>>,
    mut swept: Local<HashMap<Entity, (u32, f32)>>,
    mut ready: Local<HashMap<Entity, f32>>,
) {
    let now = time.elapsed_secs();
    let mut rng = rand::rng();
    // UAV Jammer keeps a pawn off the sweeps.
    let everyone: Vec<(Entity, Pawn, Vec3)> = pawns
        .iter()
        .filter(|(.., loadout)| !crate::perks::has(*loadout, "specialty_gpsjammer"))
        .map(|(e, p, tf, _)| (e, p.clone(), tf.translation))
        .collect();
    let sim = std::env::var_os("COD4RW_SIM").is_some();
    for (me, mut bot, pawn, tf, streak, mut input, use_crate) in &mut bots {
        // At a crate it's after, and calm: hold Use on it.
        let calm = !bot.know.contacts.values().any(|c| c.noticed());
        let at_crate = calm && crate::killstreaks::carepackage::bot_goal(me).is_some_and(|p| (p - tf.translation).with_y(0.0).length() < u(70.0));
        match use_crate {
            Some(mut c) => {
                if c.0 != at_crate {
                    c.0 = at_crate;
                }
            }
            None => {
                commands.entity(me).insert(UseCrate(at_crate));
            }
        }
        // A new sweep of a UAV this bot sees (its team's; its own in
        // free-for-all).
        let sweep = radar.as_ref().and_then(|r| r.sweep_for(pawn.team, me, now));
        let fresh = sweep.is_some_and(|s| swept.get(&me) != Some(&s));
        match sweep {
            Some(s) => swept.insert(me, s),
            None => swept.remove(&me),
        };
        if fresh {
            for (e, other, pos) in &everyone {
                if crate::combat::hostile(other, pawn) {
                    perception::radar(&mut bot.know, *e, *pos, now);
                }
            }
        }
        let Some(item) = streak.held.best() else {
            *input = HardpointInput::default();
            ready.remove(&me);
            continue;
        };
        // A moment after earning it, and not mid-fight.
        let at = *ready.entry(me).or_insert_with(|| now + rng.random_range(1.0..4.0));
        let fighting = bot.know.contacts.values().any(|c| c.noticed());
        if fighting || now < at {
            input.use_now = false;
            continue;
        }
        // Asked for a while and still held (one already up, say): try
        // again later.
        if now - at > 1.0 && input.use_now {
            ready.insert(me, now + rng.random_range(10.0..20.0));
            input.use_now = false;
            continue;
        }
        input.target = None;
        input.item = Some(item);
        if item == Hardpoint::Airstrike {
            input.target = airstrike_target(&bot, tf.translation, now, now - at > WAIT_FOR_TWO);
        }
        let asked = input.use_now;
        input.use_now = item != Hardpoint::Airstrike || input.target.is_some();
        if input.use_now && !asked {
            ready.insert(me, now);
            if sim {
                info!("hardpoint: {} calls in {:?}", pawn.name, item);
            }
        }
    }
}

/// Where to drop an airstrike: the enemy we know of with the most others
/// near them (seen, heard or swept in the last few seconds), away from us;
/// at least two together unless `any` will do.
fn airstrike_target(bot: &Bot, feet: Vec3, now: f32, any: bool) -> Option<Vec3> {
    let known: Vec<Vec3> = bot.know.contacts.values().filter(|c| c.age(now) < 5.0).map(|c| c.pos).collect();
    known
        .iter()
        .filter(|p| p.distance(feet) > u(DANGER_CLOSE))
        .map(|p| (*p, known.iter().filter(|q| q.distance(*p) < u(BUNCHED)).count()))
        .filter(|(_, n)| *n >= 2 || any)
        .max_by_key(|(_, n)| *n)
        .map(|(p, _)| p)
}

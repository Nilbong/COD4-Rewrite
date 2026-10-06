//! Bots and dropped weapons ([`crate::pickups`]): outside fights, a bot
//! short of ammo walks over a dropped copy of a gun it carries (that tops
//! its reserve up by itself), and one whose gun has run dry swaps it for a
//! weapon lying within reach.

use super::{Bot, Mode};
use crate::combat::Dead;
use crate::loadout::Loadout;
use crate::movement::{Mover, ViewAngles};
use crate::pickups::{Pickup, PickupInput, may_swap_for, would_take_ammo};
use crate::units::u;
use crate::weapons::WeaponState;
use avian3d::prelude::*;
use bevy::prelude::*;

/// How far a bot goes out of its way for ammo.
const AMMO_REACH: f32 = u(1200.0);
/// How close a weapon must be to swap for it (`PickupInput`'s 128 from the
/// eye, with a little to spare).
const SWAP_REACH: f32 = u(110.0);

#[allow(clippy::type_complexity)]
pub(super) fn resupply(
    spatial: SpatialQuery,
    items: Query<(&Pickup, &GlobalTransform)>,
    mut bots: Query<(&mut Bot, &Transform, &Mover, &WeaponState, &Loadout, &mut PickupInput, &mut ViewAngles), Without<Dead>>,
) {
    if items.is_empty() {
        return;
    }
    for (mut bot, tf, mover, weapon, loadout, mut input, mut view) in &mut bots {
        if matches!(bot.mode, Mode::Engage | Mode::Cover) {
            continue;
        }
        let feet = tf.translation;
        let eye = mover.eye(feet);
        // Run dry: swap for whatever's within reach and in sight.
        if weapon.clip == 0 && weapon.reserve == 0 {
            let near = items
                .iter()
                .filter(|(p, _)| may_swap_for(p, loadout, weapon))
                .map(|(_, g)| g.translation())
                .filter(|p| p.distance(eye) < SWAP_REACH && clear(&spatial, eye, *p + Vec3::Y * u(2.0)))
                .min_by(|a, b| a.distance(eye).total_cmp(&b.distance(eye)));
            if let Some(at) = near {
                let d = (at - eye).normalize_or_zero();
                view.yaw = (-d.x).atan2(-d.z);
                view.pitch = d.y.asin();
                input.swap = true;
                continue;
            }
        }
        // Short of ammo: over to a dropped copy of a gun we carry.
        if weapon.reserve >= weapon.def.clip_size {
            continue;
        }
        let near = items
            .iter()
            .filter(|(p, _)| would_take_ammo(p, loadout, weapon))
            .map(|(_, g)| g.translation())
            .filter(|p| p.distance(feet) < AMMO_REACH)
            .min_by(|a, b| a.distance(feet).total_cmp(&b.distance(feet)));
        if let Some(at) = near {
            if bot.dest.is_none_or(|d| d.distance(at) > u(40.0)) {
                bot.go_to(at);
            }
        }
    }
}

fn clear(spatial: &SpatialQuery, from: Vec3, to: Vec3) -> bool {
    let d = to - from;
    Dir3::new(d).ok().is_none_or(|dir| spatial.cast_ray(from, dir, d.length(), true, &crate::collision::sight_filter()).is_none())
}

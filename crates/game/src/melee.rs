//! Melee: CoD4's knife, on V (`+melee`). A swing takes `iMeleeTime` and
//! hits `iMeleeDelay` in, for `iMeleeDamage` (135: a kill, Juggernaut or
//! not), whoever is within `player_meleeRange` (64 units) in front: a
//! `player_meleeWidth` × `player_meleeHeight` box swept along the view.
//! With an enemy within `aim_automelee_range` (128 units) near the
//! crosshair it's a lunge (`PM_MeleeChargeStart`): the knife's charge
//! animation and timing (`meleeChargeTime`, `meleeChargeDelay`), the pawn
//! carried at them ([`Mover::charge`]) and the view drawn onto them
//! (`aim_automelee_lerp`). A swing cancels a reload, and nothing fires or
//! aims meanwhile. Any pawn melees through its [`MeleeInput`], bots too.

use crate::collision;
use crate::combat::{Damage, Dead, HitLocation, Hitbox, Pawn};
use crate::content::Content;
use crate::movement::{Mover, ViewAngles};

use crate::units::u;
use crate::weapons::{HitConfirmed, WeaponInput, WeaponState};
use avian3d::prelude::*;
use bevy::prelude::*;

use iw3::zone::AssetType;
use std::collections::HashMap;

pub struct MeleePlugin;

impl Plugin for MeleePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                (give_input, keys).chain().in_set(crate::player::InputSet),
                melee.after(crate::movement::MovementSet).before(crate::weapons::WeaponSet),
            )
                .run_if(crate::state::in_game),
        );
    }
}

/// The kill feed's name for a knife kill ([`Damage::weapon`]), and its icon.
pub const WEAPON: &str = "knife";
pub const KILL_ICON: &str = "killiconmelee";

/// Every CoD4 weapon's melee (they share the knife): `iMeleeTime`,
/// `iMeleeDelay`, `meleeChargeTime`, `meleeChargeDelay` (seconds) and
/// `iMeleeDamage`.
const TIME: f32 = 0.8;
const DELAY: f32 = 0.129;
const CHARGE_TIME: f32 = 1.159;
const CHARGE_DELAY: f32 = 0.159;
const DAMAGE: f32 = 135.0;
/// `player_meleeRange`, `player_meleeWidth`, `player_meleeHeight`.
const RANGE: f32 = 64.0;
const WIDTH: f32 = 10.0;
const HEIGHT: f32 = 10.0;
/// `aim_automelee_range`, and how far off the crosshair a lunge target may
/// be (`aim_automelee_region_width`/`_height` of the screen, about 18
/// degrees either way at CoD4's field of view).
const LUNGE_RANGE: f32 = 128.0;
const LUNGE_CONE: f32 = 18.0;
/// A lunge stops this short of them (CoD units, about a player's width).
const LUNGE_STOP: f32 = 30.0;
/// `aim_automelee_lerp`: the share of the way to them the view turns each
/// (60 Hz) frame.
const LERP: f32 = 0.4;

/// Melee now: `pressed` while the button's down (a swing starts on a fresh
/// press).
#[derive(Component, Default, Clone, Copy, Debug)]
pub struct MeleeInput {
    pub pressed: bool,
    held: bool,
}

/// A swing under way.
#[derive(Component, Clone, Copy, Debug)]
pub struct Melee {
    pub started: f32,
    pub hit_at: f32,
    pub until: f32,
    /// A lunge (the charge animation).
    pub charge: bool,
    pub target: Option<Entity>,
    hit: bool,
}

fn give_input(mut commands: Commands, pawns: Query<Entity, (With<Pawn>, Without<MeleeInput>)>) {
    for e in &pawns {
        commands.entity(e).insert(MeleeInput::default());
    }
}

/// V, while the game has the mouse.
fn keys(mut players: Query<(&crate::splitscreen::PlayerInput, &mut MeleeInput)>) {
    for (player, mut input) in &mut players {
        input.pressed = player.live && player.keys.pressed(KeyCode::KeyV);
    }
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn melee(
    mut commands: Commands,
    time: Res<Time>,
    spatial: SpatialQuery,
    content: Option<Res<Content>>,
    mut pawns: Query<
        (
            Entity,
            &Pawn,
            &Transform,
            &mut Mover,
            &mut ViewAngles,
            &mut WeaponState,
            &mut WeaponInput,
            &mut MeleeInput,
            Option<&mut Melee>,
            Has<crate::grenades::Offhand>,
            Option<&crate::loadout::Loadout>,
        ),
        Without<Dead>,
    >,
    hitboxes: Query<&Hitbox>,
    mut damage: MessageWriter<Damage>,
    mut hits: MessageWriter<HitConfirmed>,
    mut sfx: ResMut<crate::audio::Sfx>,
    mut sounds: Local<HashMap<String, (Option<String>, Option<String>)>>,
) {
    let now = time.elapsed_secs();
    let dt = time.delta_secs();
    // Everyone's chest and team, to lunge at.
    let others: Vec<(Entity, Pawn, Vec3)> =
        pawns.iter().map(|(e, p, tf, m, ..)| (e, p.clone(), tf.translation + Vec3::Y * m.eye_height * 0.75)).collect();
    let sight = collision::sight_filter();
    for (me, pawn, tf, mut mover, mut view, mut weapon, mut wi, mut input, swing, throwing, loadout) in &mut pawns {
        let fresh = input.pressed && !input.held;
        input.held = input.pressed;
        let eye = mover.eye(tf.translation);
        // The weapon's swipe and hit sounds (`meleeSwipeSound`,
        // `meleeHitSound`).
        let (swipe, hit_sound) = sounds
            .entry(weapon.def.name.clone())
            .or_insert_with(|| {
                let sound = |f: &str| {
                    let (_, w) = content.as_ref()?.generic(AssetType::Weapon, &weapon.def.name)?;
                    w.node(f).and_then(|n| n.node("name")).and_then(|n| n.string("soundName")).map(str::to_owned)
                };
                (sound("meleeSwipeSound"), sound("meleeHitSound"))
            })
            .clone();
        match swing {
            None => {
                let switching = loadout.is_some_and(|l| l.switching.is_some());
                if !fresh || throwing || switching || weapon.ads >= 1.0 && weapon.def.ads_overlay.is_some() {
                    continue;
                }
                // Someone to lunge at: the nearest enemy in range near the
                // crosshair, in sight.
                let forward = view.forward();
                let target = others
                    .iter()
                    .filter(|(e, p, _)| *e != me && crate::combat::hostile(p, pawn))
                    .filter(|(_, _, chest)| {
                        let to = *chest - eye;
                        to.length() < u(LUNGE_RANGE) && to.normalize_or_zero().dot(forward) > LUNGE_CONE.to_radians().cos()
                    })
                    .filter(|(_, _, chest)| {
                        Dir3::new(*chest - eye).ok().is_none_or(|d| spatial.cast_ray(eye, d, chest.distance(eye), true, &sight).is_none())
                    })
                    .min_by(|a, b| a.2.distance(eye).total_cmp(&b.2.distance(eye)))
                    .map(|(e, _, chest)| (*e, *chest));
                let flat = target.map(|(_, c)| Vec3::new(c.x - eye.x, 0.0, c.z - eye.z));
                let lunge = flat.map(|f| f.length() - u(LUNGE_STOP)).filter(|d| *d > u(8.0));
                if let (Some(f), Some(d)) = (flat, lunge) {
                    // `player_meleeChargeFriction` (1200) stops it there.
                    mover.charge = Some(f.normalize_or_zero() * (2.0 * u(1200.0) * d).sqrt());
                }
                let charge = lunge.is_some();
                let (length, delay) = if charge { (CHARGE_TIME, CHARGE_DELAY) } else { (TIME, DELAY) };
                commands.entity(me).insert(Melee {
                    started: now,
                    hit_at: now + delay,
                    until: now + length,
                    charge,
                    target: target.map(|t| t.0),
                    hit: false,
                });
                weapon.reload_until = None;
                weapon.reload_add_at = None;
                weapon.next_fire = weapon.next_fire.max(now + length);
                mover.sprinting = false;
                *wi = WeaponInput::default();
                if let Some(s) = swipe {
                    sfx.play(s, Some(eye));
                }
            }
            Some(mut m) => {
                // Nothing fires, aims or reloads meanwhile.
                *wi = WeaponInput::default();
                weapon.next_fire = weapon.next_fire.max(m.until);
                // The view drawn onto them until the blow.
                if let Some((_, _, chest)) = m.target.and_then(|t| others.iter().find(|o| o.0 == t)) {
                    if now < m.hit_at {
                        let to = *chest - eye;
                        let goal = Vec2::new(f32::atan2(-to.x, -to.z), f32::atan2(to.y, Vec2::new(to.x, to.z).length()));
                        let k = 1.0 - (1.0 - LERP).powf(dt * 60.0);
                        let dyaw = (goal.x - view.yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
                        view.yaw += dyaw * k;
                        view.pitch += (goal.y - view.pitch) * k;
                    }
                }
                if !m.hit && now >= m.hit_at {
                    m.hit = true;
                    let shape = Collider::cuboid(u(WIDTH), u(HEIGHT), u(1.0));
                    let rot = view.rotation();
                    let config = ShapeCastConfig::from_max_distance(u(RANGE));
                    let struck = Dir3::new(view.forward())
                        .ok()
                        .and_then(|d| {
                            spatial.cast_shape_predicate(&shape, eye, rot, d, &config, &collision::bullet_filter(), &|e| {
                                hitboxes.get(e).map_or(true, |h| h.owner != me)
                            })
                        })
                        .and_then(|hit| hitboxes.get(hit.entity).ok())
                        .map(|h| h.owner)
                        .filter(|owner| others.iter().any(|o| o.0 == *owner && crate::combat::hostile(&o.1, pawn)));
                    if let Some(victim) = struck {
                        damage.write(Damage { target: victim, attacker: Some(me), amount: DAMAGE, location: HitLocation::Torso, weapon: WEAPON });
                        hits.write(HitConfirmed { shooter: me, headshot: false });
                        if let Some(s) = &hit_sound {
                            sfx.play(s.clone(), Some(eye));
                        }
                    }
                }
                if now >= m.until {
                    commands.entity(me).remove::<Melee>();
                }
            }
        }
    }
}

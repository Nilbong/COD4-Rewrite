//! Mantling (IW3's `bg_mantle`): holding jump while facing a mantle surface
//! (volumes the level designers put on climbable walls and crates, marked
//! `mantle_on` or `mantle_over`), a standing player climbs onto the ledge
//! behind it, and for `mantle_over` on over the far side if it drops away,
//! following the mantle animations' root motion without collision.

use super::{Ctx, Mover, Stance};
use crate::units::u;
use avian3d::prelude::*;
use bevy::prelude::*;
use iw3::xanim::XAnim;
use std::sync::Arc;

/// Climbs for ledges about this high (units), each with its up animation
/// and the over animation that follows it (`s_mantleTrans`). The low over
/// is `player_mantle_over_low`: the MP one has no root motion.
const TRANSITIONS: [(&str, &str, f32); 7] = [
    ("mp_mantle_up_57", "mp_mantle_over_high", 57.0),
    ("mp_mantle_up_51", "mp_mantle_over_high", 51.0),
    ("mp_mantle_up_45", "mp_mantle_over_mid", 45.0),
    ("mp_mantle_up_39", "mp_mantle_over_mid", 39.0),
    ("mp_mantle_up_33", "mp_mantle_over_mid", 33.0),
    ("mp_mantle_up_27", "player_mantle_over_low", 27.0),
    ("mp_mantle_up_21", "player_mantle_over_low", 21.0),
];

/// `mantle_check_range`, `mantle_check_radius`, `mantle_check_angle`.
const CHECK_RANGE: f32 = u(20.0);
const CHECK_RADIUS: f32 = u(0.1);
const CHECK_ANGLE_COS: f32 = 0.5;
/// Ledge heights tried, highest first.
const LEDGE_HEIGHTS: [f32; 3] = [u(60.0), u(40.0), u(20.0)];
/// The player's half width and standing height; crouched height.
const HALF: f32 = u(15.0);
const STAND: f32 = u(70.0);
const CROUCH: f32 = u(50.0);

/// The mantle animations, loaded with the match's content.
#[derive(Resource, Default)]
pub struct MantleAnims(Vec<(Arc<XAnim>, Arc<XAnim>, f32)>);

pub fn load_anims(mut commands: Commands, content: Option<ResMut<crate::content::Content>>, anims: Option<Res<MantleAnims>>) {
    let (Some(mut content), None) = (content, anims) else { return };
    let loaded: Option<Vec<_>> =
        TRANSITIONS.iter().map(|&(up, over, h)| Some((content.anim(up)?, content.anim(over)?, h))).collect();
    if loaded.is_none() {
        warn!("mantle animations not found: no mantling");
    }
    commands.insert_resource(MantleAnims(loaded.unwrap_or_default()));
}

/// A mantle under way.
#[derive(Clone, Copy, Debug)]
pub struct Mantle {
    /// Index into [`TRANSITIONS`].
    trans: usize,
    over: bool,
    /// Seconds in.
    timer: f32,
    /// Horizontal, into the ledge.
    dir: Vec3,
    /// The ledge is too low to stand on.
    pub crouch: bool,
}

fn cast(ctx: &Ctx, shape: &Collider, from: Vec3, dir: Dir3, dist: f32) -> Option<ShapeHitData> {
    ctx.spatial.cast_shape(shape, from, Quat::IDENTITY, dir, &ShapeCastConfig::from_max_distance(dist), ctx.filter)
}

fn solid(ctx: &Ctx, shape: &Collider, at: Vec3) -> bool {
    !ctx.spatial.shape_intersections(shape, at, Quat::IDENTITY, ctx.filter).is_empty()
}

/// A player-wide box `height` tall, and its centre for feet at `feet`.
fn player_box(height: f32) -> Collider {
    Collider::cuboid(2.0 * HALF, height, 2.0 * HALF)
}

/// `Mantle_Check`: start a mantle if there's a mantle surface ahead and a
/// ledge to climb onto. `over` lists the `mantle_over` surfaces.
pub(super) fn check(ctx: &Ctx, pos: &mut Vec3, m: &mut Mover, yaw: f32, anims: &MantleAnims, over: &[Entity]) -> bool {
    if anims.0.is_empty() {
        return false;
    }
    let Some((dir, ledge, mantle_over)) = find(ctx, *pos, m.stance, yaw, over) else { return false };
    start_mantle(ctx, pos, m, anims, dir, ledge, mantle_over);
    true
}

/// Whether a mantle is there to be had (IW3 shows its hint then).
pub(super) fn available(ctx: &Ctx, pos: Vec3, m: &Mover, yaw: f32, anims: &MantleAnims, over: &[Entity]) -> bool {
    !anims.0.is_empty() && find(ctx, pos, m.stance, yaw, over).is_some()
}

/// Where a standing player at `feet` facing `yaw` would end up mantling,
/// and the way in (for bots' routes).
pub(super) fn landing(ctx: &Ctx, feet: Vec3, yaw: f32, over: &[Entity]) -> Option<(Vec3, Vec3)> {
    let (dir, ledge, mantle_over) = find(ctx, feet, Stance::Stand, yaw, over)?;
    Some((end_of(ctx, dir, ledge, mantle_over).0, *dir))
}

/// The mantle surface ahead and the ledge behind it: the direction in, the
/// ledge (feet) and whether it's `mantle_over`.
fn find(ctx: &Ctx, pos: Vec3, stance: Stance, yaw: f32, over: &[Entity]) -> Option<(Dir3, Vec3, bool)> {
    if stance != Stance::Stand {
        return None;
    }
    let fwd = Vec3::new(-yaw.sin(), 0.0, -yaw.cos());
    let fwd_dir = Dir3::new(fwd).ok()?;
    // `Mantle_FindMantleSurface`: a thin box from inside the player to
    // `mantle_check_range` beyond its edge, against mantle volumes only.
    let inner = HALF - CHECK_RADIUS;
    let probe = Collider::cuboid(2.0 * CHECK_RADIUS, STAND, 2.0 * CHECK_RADIUS);
    let start = pos - fwd * inner + Vec3::Y * (STAND * 0.5);
    let mantles = SpatialQueryFilter::from_mask(crate::collision::Layer::Mantle);
    let config = ShapeCastConfig::from_max_distance(inner + CHECK_RANGE + inner);
    let hit = ctx.spatial.cast_shape(&probe, start, Quat::IDENTITY, fwd_dir, &config, &mantles)?;
    if hit.distance <= 0.0 {
        return None; // too thick: started inside it
    }
    let mut n = hit.normal1;
    if n.dot(fwd) > 0.0 {
        n = -n;
    }
    let dir = Dir3::new(-Vec3::new(n.x, 0.0, n.z)).ok()?;
    if fwd.dot(*dir) < CHECK_ANGLE_COS {
        return None;
    }
    let mantle_over = over.contains(&hit.entity);
    LEDGE_HEIGHTS.into_iter().find_map(|height| ledge(ctx, pos, dir, height)).map(|l| (dir, l, mantle_over))
}

/// `Mantle_CheckLedge`: from `height` up, a player-wide box can move 16
/// units on and drop onto walkable ground at least 18 units above the
/// feet, with room to crouch there. Returns the ledge (feet).
fn ledge(ctx: &Ctx, feet: Vec3, dir: Dir3, height: f32) -> Option<Vec3> {
    let shape = player_box(u(30.0));
    let from = feet + Vec3::Y * (height + u(15.0));
    if solid(ctx, &shape, from) || cast(ctx, &shape, from, dir, u(16.0)).is_some() {
        return None;
    }
    let above = from + *dir * u(16.0);
    let hit = cast(ctx, &shape, above, Dir3::NEG_Y, height - u(18.0))?;
    let mut n = hit.normal1;
    if n.y < 0.0 {
        n = -n;
    }
    if hit.distance <= 0.0 || n.y < super::MIN_WALK_NORMAL {
        return None;
    }
    let ledge = above - Vec3::Y * (u(15.0) + hit.distance);
    (!solid(ctx, &player_box(CROUCH), ledge + Vec3::Y * CROUCH * 0.5)).then_some(ledge)
}

/// `Mantle_CalcEndPos`: where the climb ends, over the far side when it's
/// clear and drops away (and whether it does go over).
fn end_of(ctx: &Ctx, dir: Dir3, ledge: Vec3, over: bool) -> (Vec3, bool) {
    if over {
        let shape = player_box(CROUCH);
        let from = ledge + Vec3::Y * CROUCH * 0.5;
        let beyond = from + *dir * u(31.0);
        let blocked = solid(ctx, &shape, from)
            || cast(ctx, &shape, from, dir, u(31.0)).is_some()
            || cast(ctx, &shape, beyond, Dir3::NEG_Y, u(18.0)).is_some();
        if !blocked {
            return (beyond - Vec3::Y * (CROUCH * 0.5 + u(18.0)), true);
        }
    }
    (ledge, false)
}

/// `Mantle_Start`: the climb begins.
fn start_mantle(ctx: &Ctx, pos: &mut Vec3, m: &mut Mover, anims: &MantleAnims, dir: Dir3, ledge: Vec3, over: bool) {
    let (end, over) = end_of(ctx, dir, ledge, over);
    let rise = (ledge.y - pos.y) / u(1.0);
    let trans = (0..anims.0.len()).min_by(|&a, &b| (anims.0[a].2 - rise).abs().total_cmp(&(anims.0[b].2 - rise).abs())).unwrap_or(0);
    let crouch = solid(ctx, &player_box(STAND), ledge + Vec3::Y * STAND * 0.5);
    let mantle = Mantle { trans, over, timer: 0.0, dir: *dir, crouch };
    let total = duration(anims, &mantle);
    *pos = end - world(&mantle, delta(anims, &mantle, total));
    m.mantle = Some(mantle);
    m.velocity = Vec3::ZERO;
    m.on_ground = false;
    m.sprinting = false;
    m.jump_held = true;
}

fn duration(anims: &MantleAnims, mt: &Mantle) -> f32 {
    let (up, over, _) = &anims.0[mt.trans];
    up.duration() + if mt.over { over.duration() } else { 0.0 }
}

/// `Mantle_GetAnimDelta`: root motion `t` seconds in (CoD units, the
/// animations' axes).
fn delta(anims: &MantleAnims, mt: &Mantle, t: f32) -> Vec3 {
    let (up, over, _) = &anims.0[mt.trans];
    let up_len = up.duration().max(1e-3);
    let d = if t > up_len && mt.over {
        let a = up.delta_at(1.0);
        let b = over.delta_at((t - up_len) / over.duration().max(1e-3));
        [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
    } else {
        up.delta_at(t / up_len)
    };
    Vec3::from(d)
}

/// Animation axes (x into the ledge, y left, z up) to the world, in metres.
fn world(mt: &Mantle, d: Vec3) -> Vec3 {
    let left = Vec3::Y.cross(mt.dir);
    (mt.dir * d.x + left * d.y + Vec3::Y * d.z) * u(1.0)
}

/// `Mantle_Move`: carry on along the climb.
pub(super) fn step(pos: &mut Vec3, m: &mut Mover, anims: &MantleAnims, dt: f32) {
    let Some(mut mt) = m.mantle else { return };
    if mt.trans >= anims.0.len() {
        m.mantle = None;
        return;
    }
    let total = duration(anims, &mt);
    let before = delta(anims, &mt, mt.timer);
    mt.timer = (mt.timer + dt).min(total);
    let moved = world(&mt, delta(anims, &mt, mt.timer) - before);
    *pos += moved;
    m.velocity = moved / dt.max(1e-4);
    m.on_ground = false;
    m.mantle = (mt.timer < total).then_some(mt);
    if m.mantle.is_none() {
        m.velocity = Vec3::ZERO;
    }
}

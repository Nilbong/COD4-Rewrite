//! 3rd Person TDM ([`crate::modes::GameMode::Tdm3`]): the player seen over
//! the shoulder, with cover. Design: `data/cover/cover-design-notes.md`.
//! Everything here is inert outside that game type (`COD4RW_COVER=1` turns
//! it on for tests), so first-person play is CoD4 as it was.
//!
//! This file so far: the shoulder camera (closer than F5's, swappable,
//! tightening while aiming, kept out of walls) and the third-person shot
//! (the camera's aim ray, corrected from the muzzle so that a wall between
//! the gun and the crosshair's target stops the bullet).

use crate::combat::{Dead, Hitbox};
use crate::movement::{MoveInput, Mover, Stance, ViewAngles};
use crate::splitscreen::{LocalSlot, MAX_PLAYERS, PlayerInput, SlotCamera};
use crate::units::u;
use crate::weapons::{FreeAim, WeaponInput, WeaponState};
use avian3d::prelude::*;
use bevy::prelude::*;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct CoverPlugin;

impl Plugin for CoverPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CoverView>()
            .init_resource::<bots::CoverMap>()
            .add_systems(Update, (bots::grow_map, bots::botshot, expire_leaving).run_if(crate::state::in_game))
            .add_systems(OnEnter(crate::state::GameState::InGame), load_anims)
            .add_systems(Update, (force_third_person, update_view).chain().run_if(crate::state::in_game))
            .add_systems(
                Update,
                (test::run, cover_control)
                    .chain()
                    .after(crate::player::InputSet)
                    .before(crate::movement::MovementSet)
                    .run_if(crate::state::in_game),
            )
            .add_systems(
                PostUpdate,
                (shoulder_fov, third_person_aim.after(shoulder_fov))
                    .after(crate::wardrobe::place_camera)
                    .before(TransformSystems::Propagate)
                    .run_if(crate::state::in_game),
            );
    }
}

/// Is the over-the-shoulder view and its cover live? (The game type, or
/// `COD4RW_COVER` for tests.)
pub fn active() -> bool {
    static FORCED: OnceLock<bool> = OnceLock::new();
    crate::modes::third_person() || *FORCED.get_or_init(|| std::env::var_os("COD4RW_COVER").is_some())
}

/// The camera's place relative to the eye, in CoD units: right, up, behind.
const HIP: Vec3 = Vec3::new(26.0, 8.0, 64.0);
const ADS: Vec3 = Vec3::new(18.0, 5.0, 34.0);
/// How far it keeps from walls.
const WALL_MARGIN: f32 = 5.0;
/// The field of view while aiming, as a fraction of the hip view's.
const ADS_FOV: f32 = 0.82;
/// How fast the camera changes shoulder, per second.
const SWAP_RATE: f32 = 10.0;

/// Where a gun held at the shoulder points from, in CoD units from the eye:
/// right, up, forward.
const MUZZLE: Vec3 = Vec3::new(11.0, -9.0, 28.0);
/// Closer than this the aim point is on the shooter: fire along the view.
const NEAR_AIM: f32 = 60.0;
/// A wall this far in front of the aim point (from the muzzle) blocks the shot.
const BLOCK_SLACK: f32 = 14.0;

/// The first player's shot is blocked, for the HUD's crosshair.
static BLOCKED: AtomicBool = AtomicBool::new(false);

pub fn blocked() -> bool {
    BLOCKED.load(Ordering::Relaxed)
}

/// Each local player's shoulder view state.
#[derive(Resource)]
pub struct CoverView {
    /// The left shoulder is chosen.
    pub left: [bool; MAX_PLAYERS],
    /// Smoothed side: +1 right shoulder, -1 left.
    pub side: [f32; MAX_PLAYERS],
    /// How far into aiming (0..1), from the weapon.
    pub ads: [f32; MAX_PLAYERS],
    /// The last shot is blocked at the muzzle, for the indicator.
    pub blocked: [bool; MAX_PLAYERS],
}

impl Default for CoverView {
    fn default() -> Self {
        CoverView { left: [false; MAX_PLAYERS], side: [1.0; MAX_PLAYERS], ads: [0.0; MAX_PLAYERS], blocked: [false; MAX_PLAYERS] }
    }
}

/// In this game type every local player is in third person, whatever F5 says.
fn force_third_person(mut view: ResMut<crate::wardrobe::ThirdPerson>) {
    if !active() {
        return;
    }
    if view.0.iter().any(|on| !*on) {
        view.0 = [true; MAX_PLAYERS];
    }
}

/// Shoulder swap (Z) and the aim fraction the camera follows.
fn update_view(
    time: Res<Time>,
    mut cover: ResMut<CoverView>,
    spatial: SpatialQuery,
    players: Query<(&LocalSlot, &PlayerInput, &WeaponState, Option<&InCover>, &Mover, &Transform, &ViewAngles), Without<Dead>>,
) {
    if !active() {
        return;
    }
    let dt = time.delta_secs();
    for (slot, input, weapon, in_cover, mover, tf, view) in &players {
        let s = slot.0.min(MAX_PLAYERS - 1);
        if input.live && input.keys.just_pressed(KeyCode::KeyZ) {
            cover.left[s] = !cover.left[s];
        }
        cover.ads[s] = weapon.ads;
        let mut target = if cover.left[s] { -1.0 } else { 1.0 };
        // Leaning out of cover: the camera takes the shoulder on that side.
        if in_cover.is_some() && mover.lean.abs() > 0.2 {
            target = mover.lean.signum();
        } else {
            // A wall on the chosen side crowds the camera: take the other
            // shoulder when it has the room.
            let eye = mover.eye(tf.translation);
            let right = view.rotation() * Vec3::X;
            let want = u(HIP.x) + u(WALL_MARGIN);
            let room = |dir: Vec3| probe(&spatial, eye, dir, want).map_or(1.0, |(d, _)| d / want);
            let (mine, other) = (room(right * target), room(-right * target));
            if mine < 0.6 && other > mine + 0.25 {
                target = -target;
            }
        }
        cover.side[s] += (target - cover.side[s]) * (1.0 - (-SWAP_RATE * dt).exp());
    }
}

/// The camera's position for a player: the eye, moved to the shoulder
/// (first, clear of any wall beside the head), then back and up, short of
/// any wall behind. `rot` is the view's rotation.
pub fn camera_position(eye: Vec3, rot: Quat, slot: usize, cover: &CoverView, spatial: &SpatialQuery) -> Vec3 {
    let s = slot.min(MAX_PLAYERS - 1);
    let ads = cover.ads[s].clamp(0.0, 1.0);
    let at = HIP.lerp(ADS, ads);
    let filter = crate::collision::sight_filter();
    let cast = |from: Vec3, to: Vec3| -> Vec3 {
        let d = to - from;
        let len = d.length();
        let Ok(dir) = Dir3::new(d) else { return from };
        let reach = spatial.cast_ray(from, dir, len + u(WALL_MARGIN), true, &filter).map_or(len, |h| (h.distance - u(WALL_MARGIN)).clamp(0.0, len));
        from + dir * reach
    };
    let sideways = rot * Vec3::X * cover.side[s];
    let shoulder = cast(eye, eye + sideways * u(at.x));
    let wanted = shoulder + rot * Vec3::new(0.0, u(at.y), u(at.z));
    let back = cast(shoulder, wanted);
    // A wall behind (the player's back to cover): the room it takes comes
    // out of the sideways, so the body stays in view beside the camera.
    let missing = back.distance(wanted);
    if missing > u(4.0) {
        return cast(back, back + sideways * (missing * 0.8).min(u(40.0)));
    }
    back
}

/// A small zoom while aiming, instead of the weapon's own.
fn shoulder_fov(cover: Res<CoverView>, mut cameras: Query<(&SlotCamera, &mut Projection)>) {
    if !active() {
        return;
    }
    for (camera, mut proj) in &mut cameras {
        let ads = cover.ads[camera.0.min(MAX_PLAYERS - 1)].clamp(0.0, 1.0);
        if let Projection::Perspective(p) = proj.as_mut() {
            p.fov = crate::player::hip_fov().to_radians() * (1.0 - (1.0 - ADS_FOV) * ads);
        }
    }
}

/// Shots leave the muzzle for the point the camera's centre sees
/// ([`FreeAim`]); with something solid between the gun and that point, or
/// the gun pressed into a wall, the shot is blocked.
#[allow(clippy::type_complexity)]
fn third_person_aim(
    mut commands: Commands,
    mut cover: ResMut<CoverView>,
    spatial: SpatialQuery,
    hitboxes: Query<&Hitbox>,
    cameras: Query<(&SlotCamera, &Transform), Without<LocalSlot>>,
    mut pawns: Query<(Entity, &LocalSlot, &Transform, &Mover, &ViewAngles, &WeaponState, &WeaponInput, Option<&InCover>, Option<&mut FreeAim>), Without<Dead>>,
    gunplay: Res<crate::bodycam::Gunplay>,
) {
    if !active() || (*gunplay).is_bodycam() {
        BLOCKED.store(false, Ordering::Relaxed);
        return;
    }
    let world = crate::collision::sight_filter();
    let bullets = crate::collision::bullet_filter();
    for (me, slot, tf, mover, view, weapon, weapon_input, in_cover, free_aim) in &mut pawns {
        let Some((_, cam)) = cameras.iter().find(|c| c.0.0 == slot.0) else { continue };
        let eye = mover.eye(tf.translation);
        let fwd = cam.rotation * Vec3::NEG_Z;
        // The aim ray starts level with the head, so what's behind the
        // player (and the player) is never what it hits.
        let start = cam.translation + fwd * (eye - cam.translation).dot(fwd).max(0.0);
        let reach = u(8192.0);
        let Ok(ray) = Dir3::new(fwd) else { continue };
        let skip = |e: Entity| hitboxes.get(e).map_or(true, |h| h.owner != me);
        let distance = spatial.cast_ray_predicate(start, ray, reach, true, &bullets, &skip).map_or(reach, |h| h.distance);
        let target = start + fwd * distance;

        let mut muzzle = eye + Quat::from_rotation_y(view.yaw) * Vec3::new(u(MUZZLE.x), u(MUZZLE.y), -u(MUZZLE.z));
        let mut blocked = false;
        // The gun pressed into a wall (or a corner between it and the head).
        if let Ok(to_gun) = Dir3::new(muzzle - eye)
            && spatial.cast_ray(eye, to_gun, eye.distance(muzzle), true, &world).is_some()
        {
            muzzle = eye;
            blocked = true;
        }
        let mut dir = target - muzzle;
        if dir.length() < u(NEAR_AIM) {
            dir = fwd;
        }
        // Something solid between the gun and the point the camera sees:
        // the classic shot through the wall you hide behind.
        if let Ok(d) = Dir3::new(dir) {
            let to_target = muzzle.distance(target);
            if spatial.cast_ray(muzzle, d, (to_target - u(BLOCK_SLACK)).max(0.0), true, &world).is_some() {
                blocked = true;
            }
        }
        // In cover and not aiming: blind fire, the gun held up over a low
        // wall or round an open corner, shooting the way the camera looks
        // with a wide spread; with nowhere to hold it out, nothing.
        let mut blind = false;
        if let Some(c) = in_cover.filter(|_| !weapon_input.ads) {
            let side = if c.open[1] { Some(1.0) } else if c.open[0] { Some(-1.0) } else { None };
            let out = if c.high { side.map(|s| eye + c.tangent * s * u(24.0)) } else { Some(eye + Vec3::Y * u(30.0) - c.normal * u(2.0)) };
            match out {
                Some(o) => {
                    muzzle = o;
                    dir = fwd;
                    blocked = false;
                    blind = true;
                }
                None => blocked = true,
            }
        }
        let rotation = Transform::IDENTITY.looking_to(dir.normalize_or(fwd), Vec3::Y).rotation;
        cover.blocked[slot.0.min(MAX_PLAYERS - 1)] = blocked;
        if slot.0 == 0 {
            BLOCKED.store(blocked, Ordering::Relaxed);
        }
        let spread = if blind { weapon.spread(mover) * 3.0 + 5.0 } else { weapon.spread(mover) };
        let aim = FreeAim { origin: muzzle, rotation, spread, view_kick: 1.0, blocked };
        match free_aim {
            Some(mut a) => *a = aim,
            None => {
                commands.entity(me).insert(aim);
            }
        }
    }
}


// ---------------------------------------------------------------------
// Cover.

/// How far from a wall a cover point is looked for, and where its player
/// stands (CoD units).
const REACH: f32 = 46.0;
const STAND_OFF: f32 = 17.0;
/// Probe heights above the feet: a crouched body, and a standing one.
const LOW_PROBE: [f32; 2] = [24.0, 40.0];
const HIGH_PROBE: f32 = 62.0;
/// Sliding along cover, units per second (crouched, standing).
const SLIDE_LOW: f32 = 85.0;
const SLIDE_HIGH: f32 = 120.0;
/// The wall ends this far ahead: an open end, where the body can lean out.
const END_LOOK: f32 = 26.0;
/// Moving away from the wall this long leaves cover.
const LEAVE_AFTER: f32 = 0.3;
/// Cover key (X).
pub const COVER_KEY: KeyCode = KeyCode::KeyX;

/// The first player has cover within reach / is in cover: what the pad's
/// B (circle) tap does ([`crate::gamepad`]: cover instead of crouching).
static AVAILABLE: AtomicBool = AtomicBool::new(false);
static IN_COVER: AtomicBool = AtomicBool::new(false);

/// B's tap is the cover button now (a place to take cover, or leaving it).
pub fn button_is_cover() -> bool {
    active() && (AVAILABLE.load(Ordering::Relaxed) || IN_COVER.load(Ordering::Relaxed))
}

/// Cover is within reach (for the hint).
pub fn available() -> bool {
    active() && AVAILABLE.load(Ordering::Relaxed) && !IN_COVER.load(Ordering::Relaxed)
}

/// A player in cover: what they are against.
#[derive(Component, Clone, Copy, Debug)]
pub struct InCover {
    /// Out of the wall, horizontal.
    pub normal: Vec3,
    /// Along the wall, to the right of someone looking out.
    pub tangent: Vec3,
    /// Tall enough to hide a standing player (else crouched behind it).
    pub high: bool,
    /// The wall ends within reach to the left / right.
    pub open: [bool; 2],
    /// Time spent pushing away from the wall.
    pub leaving: f32,
    /// Being drawn in to this spot against the wall.
    pub snap: Option<Vec3>,
    /// When it took cover (the clock's seconds), for the entry pose.
    pub since: f32,
}

/// Just out of cover (for a moment): the pose for leaving it plays.
#[derive(Component, Clone, Copy, Debug)]
pub struct LeavingCover {
    pub since: f32,
    pub high: bool,
}

/// How long the leaving state lasts.
const LEAVING_FOR: f32 = 0.8;

/// The pose for leaving low cover: standing up from behind it. (None while
/// moving, crouching still, or behind a high wall: the usual loops.)
pub fn exit_anim_name(l: &LeavingCover, moving: bool, standing: bool, age: f32) -> Option<&'static str> {
    (!moving && standing && !l.high && age < 0.6).then_some("covercrouch_hide_2_stand")
}

fn expire_leaving(mut commands: Commands, time: Res<Time>, leaving: Query<(Entity, &LeavingCover, Has<InCover>)>) {
    let now = time.elapsed_secs();
    for (e, l, in_cover) in &leaving {
        if in_cover || now - l.since > LEAVING_FOR {
            commands.entity(e).remove::<LeavingCover>();
        }
    }
}

/// A place to take cover.
#[derive(Clone, Copy, Debug)]
pub struct CoverPoint {
    /// Feet position, [`STAND_OFF`] from the wall.
    pub pos: Vec3,
    pub normal: Vec3,
    pub high: bool,
}

fn horizontal(v: Vec3) -> Vec3 {
    Vec3::new(v.x, 0.0, v.z).normalize_or_zero()
}

/// The nearest solid along a ray: distance and surface normal.
fn probe(spatial: &SpatialQuery, from: Vec3, dir: Vec3, max: f32) -> Option<(f32, Vec3)> {
    let dir = Dir3::new(dir).ok()?;
    spatial.cast_ray(from, dir, max, true, &crate::collision::sight_filter()).map(|h| (h.distance, h.normal))
}

/// Cover within reach of `feet`, preferring the wall `facing` points at: a
/// wall (steep surface) that stops a crouched body, and a standing one too
/// if it is `high`.
pub fn find_cover(spatial: &SpatialQuery, feet: Vec3, facing: Vec3) -> Option<CoverPoint> {
    let mut best: Option<(f32, CoverPoint)> = None;
    let facing = horizontal(facing);
    for i in 0..16 {
        let a = i as f32 * std::f32::consts::TAU / 16.0;
        let dir = Vec3::new(a.sin(), 0.0, a.cos());
        let mut normal = Vec3::ZERO;
        let mut dist = 0.0;
        let mut ok = true;
        for h in LOW_PROBE {
            match probe(spatial, feet + Vec3::Y * u(h), dir, u(REACH)) {
                Some((d, n)) if n.y.abs() < 0.35 && n.dot(-dir) > 0.7 => {
                    normal = horizontal(n);
                    dist = d;
                }
                _ => ok = false,
            }
        }
        if !ok {
            continue;
        }
        let high = probe(spatial, feet + Vec3::Y * u(HIGH_PROBE), dir, u(REACH + 12.0)).is_some_and(|(_, n)| n.y.abs() < 0.35);
        let score = dir.dot(facing) - dist / u(REACH) * 0.3;
        let wall = feet + dir * dist;
        let point = CoverPoint { pos: Vec3::new(wall.x, feet.y, wall.z) + normal * u(STAND_OFF), normal, high };
        if best.as_ref().is_none_or(|b| score > b.0) {
            best = Some((score, point));
        }
    }
    best.map(|b| b.1)
}

/// Is there still wall, the face `normal` is of, `along` from `pos` (feet)?
/// An open end reads as no.
fn wall_at(spatial: &SpatialQuery, pos: Vec3, normal: Vec3, along: Vec3) -> bool {
    let from = pos + along + Vec3::Y * u(32.0);
    probe(spatial, from, -normal, u(STAND_OFF + 14.0)).is_some_and(|(_, n)| n.dot(normal) > 0.9)
}

/// Cover points around `center`, a grid sweep at the player's level: for
/// finding somewhere to go (bots, tests).
pub fn scan(spatial: &SpatialQuery, center: Vec3, radius: f32) -> Vec<CoverPoint> {
    let mut out: Vec<CoverPoint> = Vec::new();
    let step = u(40.0);
    let n = (radius / step) as i32;
    for ix in -n..=n {
        for iz in -n..=n {
            let at = center + Vec3::new(ix as f32 * step, 0.0, iz as f32 * step);
            let Some((d, _)) = probe(spatial, at + Vec3::Y * u(50.0), Vec3::NEG_Y, u(110.0)) else { continue };
            let feet = at + Vec3::Y * (u(50.0) - d);
            if let Some(p) = find_cover(spatial, feet, Vec3::ZERO) {
                // One per spot along a wall.
                if out.iter().all(|o| o.normal.dot(p.normal) < 0.9 || o.pos.distance(p.pos) > u(48.0)) {
                    out.push(p);
                }
            }
        }
    }
    out
}

/// Sprinting to another cover: the player runs (the usual sprint, steered
/// toward it) and takes cover on arrival.
#[derive(Component, Clone, Copy, Debug)]
pub struct CoverDash {
    pub to: CoverPoint,
    /// Gives up at this time.
    pub until: f32,
}

/// How fast a player is drawn into the cover they take (CoD units/s), and
/// how far another cover can be to run to.
const SNAP_SPEED: f32 = 260.0;
const DASH_MIN: f32 = 80.0;
const DASH_MAX: f32 = 1100.0;

/// Another cover where the camera's centre points: a wall face, with cover
/// before it, that isn't the one the player is already against.
fn aim_cover(spatial: &SpatialQuery, cam: &Transform, feet: Vec3) -> Option<CoverPoint> {
    let fwd = cam.rotation * Vec3::NEG_Z;
    let (d, n) = probe(spatial, cam.translation, fwd, u(DASH_MAX + 200.0))?;
    if n.y.abs() > 0.35 {
        return None;
    }
    let hit = cam.translation + fwd * d;
    let n = horizontal(n);
    let at = hit + n * u(STAND_OFF) + Vec3::Y * u(50.0);
    let (down, _) = probe(spatial, at, Vec3::NEG_Y, u(120.0))?;
    let ground = at - Vec3::Y * down;
    let found = find_cover(spatial, ground, -n)?;
    let far = Vec3::new(found.pos.x - feet.x, 0.0, found.pos.z - feet.z).length();
    (far > u(DASH_MIN) && far < u(DASH_MAX)).then_some(found)
}

/// The cover key, and moving along cover while in it: the pawn's move
/// input is replaced by sliding along the wall, an open end plus aim leans
/// out, and a low wall is looked over by standing up to aim or fire.
/// Pointing at other cover when pressing it sprints there instead.
#[allow(clippy::type_complexity)]
fn cover_control(
    mut commands: Commands,
    time: Res<Time>,
    spatial: SpatialQuery,
    cameras: Query<(&SlotCamera, &Transform), Without<LocalSlot>>,
    mut players: Query<
        (Entity, &LocalSlot, &PlayerInput, &ViewAngles, &mut Transform, &mut Mover, &mut MoveInput, &WeaponInput, Option<&mut InCover>, Option<&CoverDash>),
        (With<LocalSlot>, Without<Dead>),
    >,
) {
    if !active() {
        AVAILABLE.store(false, Ordering::Relaxed);
        IN_COVER.store(false, Ordering::Relaxed);
        return;
    }
    let dt = time.delta_secs().min(0.1);
    let now = time.elapsed_secs();
    for (me, slot, input, view, mut tf, mut mover, mut mi, weapon_input, in_cover, dash) in &mut players {
        let want = input.live && input.keys.just_pressed(COVER_KEY);
        let cam = cameras.iter().find(|c| c.0.0 == slot.0).map(|c| *c.1);
        if slot.0 == 0 {
            IN_COVER.store(in_cover.is_some() || dash.is_some(), Ordering::Relaxed);
            let near = in_cover.is_none() && dash.is_none() && !mover.sprinting && find_cover(&spatial, tf.translation, view.forward()).is_some();
            AVAILABLE.store(near, Ordering::Relaxed);
        }
        let fwd = horizontal(view.forward());
        let right = Vec3::new(-fwd.z, 0.0, fwd.x);
        // Running to another cover.
        if let Some(d) = dash {
            let to = d.to;
            let delta = Vec3::new(to.pos.x - tf.translation.x, 0.0, to.pos.z - tf.translation.z);
            if want || mi.jump || now > d.until {
                commands.entity(me).remove::<CoverDash>();
            } else if delta.length() < u(16.0) {
                commands.entity(me).remove::<CoverDash>().insert(InCover { normal: to.normal, tangent: to.normal.cross(Vec3::Y), high: to.high, open: [false; 2], leaving: 0.0, since: now, snap: Some(to.pos) });
            } else {
                let dir = delta.normalize();
                mi.forward = dir.dot(fwd);
                mi.right = dir.dot(right);
                mi.sprint = true;
                mi.stance = Stance::Stand;
            }
            continue;
        }
        let Some(mut c) = in_cover else {
            if want && !mover.sprinting {
                if let Some(p) = find_cover(&spatial, tf.translation, view.forward()) {
                    // Drawn in to the wall's side of the room.
                    commands.entity(me).insert(InCover { normal: p.normal, tangent: p.normal.cross(Vec3::Y), high: p.high, open: [false; 2], leaving: 0.0, since: now, snap: Some(p.pos) });
                } else if let Some(p) = cam.and_then(|c| aim_cover(&spatial, &c, tf.translation)) {
                    commands.entity(me).insert(CoverDash { to: p, until: now + 4.0 });
                }
            }
            continue;
        };
        // The key again: to the cover pointed at, else out. A jump or a
        // sprint leave too.
        if want {
            commands.entity(me).remove::<InCover>().insert(LeavingCover { since: now, high: c.high });
            mover.lean = 0.0;
            if let Some(p) = cam.and_then(|c| aim_cover(&spatial, &c, tf.translation)) {
                commands.entity(me).insert(CoverDash { to: p, until: now + 4.0 });
            }
            continue;
        }
        if mi.jump || mi.sprint {
            commands.entity(me).remove::<InCover>().insert(LeavingCover { since: now, high: c.high });
            mover.lean = 0.0;
            continue;
        }
        let pos = tf.translation;
        let wish = right * mi.right + fwd * mi.forward;
        // Pushing away from the wall for a while is leaving it.
        c.leaving = if wish.dot(c.normal) > 0.8 && !weapon_input.ads { c.leaving + dt } else { 0.0 };
        if c.leaving > LEAVE_AFTER {
            commands.entity(me).remove::<InCover>().insert(LeavingCover { since: now, high: c.high });
            continue;
        }
        let mut next = pos;
        if let Some(t) = c.snap {
            // Still being drawn in: no sliding yet.
            let d = Vec3::new(t.x - pos.x, 0.0, t.z - pos.z);
            let step = u(SNAP_SPEED) * dt;
            if d.length() <= step {
                next.x = t.x;
                next.z = t.z;
                c.snap = None;
            } else {
                next += d.normalize() * step;
            }
        } else {
            // Slide along the wall as far as it goes.
            let mut along = if wish.length() > 0.2 { wish.dot(c.tangent).clamp(-1.0, 1.0) } else { 0.0 };
            if along.abs() < 0.25 {
                along = 0.0;
            }
            let speed = if c.high { SLIDE_HIGH } else { SLIDE_LOW };
            let step = c.tangent * along.signum() * u(speed) * along.abs() * dt;
            if along != 0.0 && wall_at(&spatial, pos, c.normal, step * 2.0 + c.tangent * along.signum() * u(6.0)) {
                // Not into a corner either, nor off the edge of the floor.
                let ahead = pos + Vec3::Y * u(32.0);
                let floor_ahead = probe(&spatial, pos + step * 3.0 + Vec3::Y * u(20.0), Vec3::NEG_Y, u(44.0)).is_some();
                if probe(&spatial, ahead, c.tangent * along.signum(), u(18.0)).is_none() && floor_ahead {
                    next += step;
                }
            }
            // Keep the distance from the wall.
            if let Some((d, n)) = probe(&spatial, next + Vec3::Y * u(32.0), -c.normal, u(STAND_OFF + 14.0))
                && n.dot(c.normal) > 0.9
            {
                next += c.normal * (u(STAND_OFF) - d).clamp(-u(2.0), u(2.0));
            }
            c.open = [!wall_at(&spatial, next, c.normal, -c.tangent * u(END_LOOK)), !wall_at(&spatial, next, c.normal, c.tangent * u(END_LOOK))];
            let lean_side = along;
            // The player's own move input is spent; what is left is the pose.
            mi.lean = 0.0;
            if weapon_input.ads {
                let side = if c.open[1] && (!c.open[0] || lean_side >= 0.0) {
                    Some(1.0)
                } else if c.open[0] {
                    Some(-1.0)
                } else {
                    None
                };
                if let Some(side) = side {
                    // Lean is along the view's right; the open side as seen from it.
                    mi.lean = (c.tangent * side).dot(right).signum();
                }
            }
        }
        tf.translation = next;
        mover.velocity = Vec3::ZERO;
        mi.forward = 0.0;
        mi.right = 0.0;
        mi.sprint = false;
        // Standing up to look over low cover, only to aim; firing without
        // aiming is blind, the gun held up over it.
        mi.stance = if c.high || weapon_input.ads { Stance::Stand } else { Stance::Crouch };
    }
}

/// Test aid (`COD4RW_COVER_TEST=<dir>`, with `COD4RW_COVER`): find cover
/// near the spawn, go in, slide to the end of the wall, lean out, fire
/// blind, then run to other cover, saving a shot at each. With
/// `COD4RW_COVER_TEST_KIND=high` it uses a high wall's corner.
mod test {
    use super::*;
    use bevy::render::view::screenshot::{Screenshot, save_to_disk};

    #[derive(Default)]
    pub struct Script {
        t: f32,
        spot: Option<CoverPoint>,
        others: Vec<CoverPoint>,
        done: u32,
        searched: bool,
        /// Which side along the tangent the wall ends.
        end: f32,
    }

    fn cod(p: Vec3) -> String {
        let c = crate::units::to_cod(p);
        format!("{:.0},{:.0},{:.0}", c[0], c[1], c[2] + 1.0)
    }

    #[allow(clippy::type_complexity)]
    pub fn run(
        mut commands: Commands,
        time: Res<Time>,
        spatial: SpatialQuery,
        mut script: Local<Script>,
        mut players: Query<(Entity, &mut Transform, &mut ViewAngles, &mut MoveInput, &mut WeaponInput, Option<&InCover>), (With<LocalSlot>, Without<Dead>)>,
        mut exit: MessageWriter<AppExit>,
    ) {
        let Ok(dir) = std::env::var("COD4RW_COVER_TEST") else { return };
        let high = std::env::var("COD4RW_COVER_TEST_KIND").is_ok_and(|k| k == "high");
        let Ok((me, mut tf, mut view, mut mi, mut wi, in_cover)) = players.single_mut() else { return };
        script.t += time.delta_secs();
        let t = script.t;
        // Wait out the spawn and the animations, then look for a wall with an end to it.
        if t < 4.0 || (!script.searched && !ready() && t < 60.0) {
            return;
        }
        if !script.searched {
            script.searched = true;
            let spots = scan(&spatial, tf.translation, u(900.0));
            info!("cover test: {} cover points near the spawn", spots.len());
            // Walls with an end within slide range.
            let ends = |p: &CoverPoint| -> Option<f32> {
                let tangent = p.normal.cross(Vec3::Y);
                let solid = |k: i32| wall_at(&spatial, p.pos, p.normal, tangent * u(24.0 * k as f32));
                let plus = (1..8).find(|&k| !solid(k));
                let minus = (1..8).find(|&k| !solid(-k));
                match (plus, minus) {
                    (Some(a), Some(b)) => Some(if a <= b { 1.0 } else { -1.0 }),
                    (Some(_), None) => Some(1.0),
                    (None, Some(_)) => Some(-1.0),
                    _ => None,
                }
            };
            for p in spots.iter().filter(|p| p.high == high) {
                if let Some(side) = ends(p) {
                    info!("cover test: {} cover with an end: COD4RW_SPAWN={},0 (end {side:+})", if high { "high" } else { "low" }, cod(p.pos));
                }
            }
            let level = tf.translation.y;
            let origin = tf.translation;
            let mut near: Vec<CoverPoint> = spots.iter().copied().filter(|p| p.high == high && (p.pos.y - level).abs() < 0.5).collect();
            near.sort_by(|a, b| a.pos.distance(origin).total_cmp(&b.pos.distance(origin)));
            script.spot = near.into_iter().find(|p| ends(p).is_some_and(|s| (1..4).all(|k| wall_at(&spatial, p.pos, p.normal, -p.normal.cross(Vec3::Y) * u(10.0 * k as f32 * s)))));
            if let Some(p) = script.spot {
                script.end = ends(&p).unwrap_or(1.0);
                // Somewhere else to run to: 6..14 m away, another wall.
                script.others = spots
                    .iter()
                    .copied()
                    .filter(|o| {
                        let d = o.pos.distance(p.pos);
                        d > 4.0 && d < 16.0 && (o.pos.y - p.pos.y).abs() < 0.5 && o.normal.dot(p.normal) < 0.5
                    })
                    .collect();
                info!("cover test: using {:?}, {} places to run to", p, script.others.len());
                tf.translation = p.pos;
                // Facing the wall, to look over or around it.
                view.yaw = p.normal.x.atan2(p.normal.z);
                view.pitch = -0.1;
                script.t = 4.0;
            } else {
                warn!("cover test: no cover found");
                exit.write(AppExit::Success);
            }
            return;
        }
        let Some(spot) = script.spot else { return };
        let shot = |commands: &mut Commands, n: &str| {
            let _ = std::fs::create_dir_all(&dir);
            commands.spawn(Screenshot::primary_window()).observe(save_to_disk(format!("{dir}/{n}.png")));
        };
        let at = t - 4.0;
        // Timeline: 0.8 before; 1 in; 2 shot; slide to the end 2..5.2, shot;
        // aim 5.5..8, shot at 7.2; blind fire 8..9.6, shot at 9.2; 10 run
        // to other cover (looking at it), shots at 10.8 and 14; 15 done.
        let step = |n: u32, when: f32, script: &mut Script| -> bool {
            if script.done == n && at > when {
                script.done += 1;
                true
            } else {
                false
            }
        };
        if step(0, 0.8, &mut script) {
            shot(&mut commands, "0_before");
        }
        if step(1, 1.0, &mut script) {
            // The key press, simulated.
            commands.entity(me).insert(InCover { normal: spot.normal, tangent: spot.normal.cross(Vec3::Y), high: spot.high, open: [false; 2], leaving: 0.0, since: time.elapsed_secs(), snap: Some(spot.pos) });
        }
        if step(2, 2.0, &mut script) {
            shot(&mut commands, "1_in_cover");
        }
        // Toward the end of the wall, as the view sees it.
        if (2.0..5.2).contains(&at) && in_cover.is_some() {
            let fwd = horizontal(view.forward());
            let screen_right = Vec3::new(-fwd.z, 0.0, fwd.x);
            let along = spot.normal.cross(Vec3::Y) * script.end;
            mi.right = screen_right.dot(along).signum();
        }
        if step(3, 5.2, &mut script) {
            shot(&mut commands, "2_slid");
        }
        if (5.5..8.0).contains(&at) {
            wi.ads = true;
        }
        if step(4, 7.2, &mut script) {
            shot(&mut commands, "3_peek");
        }
        if step(5, 8.0, &mut script) {
            wi.ads = false;
        }
        if (8.2..9.6).contains(&at) {
            wi.fire = true;
        }
        if step(6, 9.2, &mut script) {
            shot(&mut commands, "5_blind");
        }
        if step(7, 9.6, &mut script) {
            wi.fire = false;
        }
        if step(8, 10.0, &mut script) {
            // Look at another cover, and run to it by the key's own rule.
            let from = tf.translation + Vec3::Y * u(50.0);
            let pick = script.others.first().copied();
            commands.entity(me).remove::<InCover>();
            mi.lean = 0.0;
            match pick {
                Some(o) => {
                    let wall = o.pos - o.normal * u(STAND_OFF) + Vec3::Y * u(30.0);
                    let d = wall - from;
                    view.yaw = (-d.x).atan2(-d.z);
                    view.pitch = d.y.atan2(Vec3::new(d.x, 0.0, d.z).length());
                    let camera = Transform { translation: from, rotation: view.rotation(), ..default() };
                    let found = aim_cover(&spatial, &camera, tf.translation);
                    info!("cover test: running to {:?}; pointing at it finds {:?}", o.pos, found.map(|f| f.pos));
                    commands.entity(me).insert(CoverDash { to: found.unwrap_or(o), until: 4.0 + time.elapsed_secs() });
                }
                None => warn!("cover test: nowhere to run to"),
            }
        }
        if step(9, 10.8, &mut script) {
            shot(&mut commands, "6_dash");
        }
        if step(10, 14.0, &mut script) {
            shot(&mut commands, "7_arrived");
            info!("cover test: in cover on arrival: {}", in_cover.is_some());
        }
        if step(11, 15.0, &mut script) {
            exit.write(AppExit::Success);
        }
    }
}

// ---------------------------------------------------------------------
// Animations: CoD4's single-player cover set (zone `common`), read from the
// player's own install on a thread, as Black Ops' are.

mod library {
    use iw3::xanim::XAnim;
    use iw3::zone::{Asset, AssetType, ParseOptions, Zone};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, OnceLock};

    struct Library {
        zone: Zone,
        decoded: Mutex<HashMap<String, Option<Arc<XAnim>>>>,
    }

    static LIBRARY: OnceLock<Option<Library>> = OnceLock::new();
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    /// The prefixes of the cover animations used (the campaign's AI cover).
    const PREFIXES: [&str; 6] = ["covercrouch_", "coverstand_", "cornercr", "cornerstnd", "corner_", "exposed_"];

    pub fn is_cover_anim(name: &str) -> bool {
        PREFIXES.iter().any(|p| name.starts_with(p))
    }

    /// Start reading the single-player `common` zone, once.
    pub fn start() {
        if STARTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        std::thread::spawn(|| {
            let lib = (|| {
                let install = iw3::Install::locate().ok()?;
                let data = iw3::fastfile::load(&install.zone_path("common")).ok()?;
                let mut zone = Zone::parse(&data, ParseOptions::default()).ok()?;
                zone.assets.retain(|a| matches!(a, Asset::Generic(g) if g.ty == AssetType::XAnimParts && is_cover_anim(&g.name)));
                Some(Library { zone, decoded: Mutex::default() })
            })();
            match &lib {
                Some(l) => bevy::log::info!("cover animations: {} read from the campaign's common zone", l.zone.assets.len()),
                None => bevy::log::warn!("cover animations: the campaign's common zone could not be read"),
            }
            let _ = LIBRARY.set(lib);
        });
    }

    /// Has the library finished loading (with or without the animations)?
    pub fn ready() -> bool {
        LIBRARY.get().is_some()
    }

    /// A cover animation, once the library has loaded.
    pub fn anim(name: &str) -> Option<Arc<XAnim>> {
        let lib = LIBRARY.get()?.as_ref()?;
        let mut decoded = lib.decoded.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(a) = decoded.get(name) {
            return a.clone();
        }
        let anim = lib.zone.assets.iter().find_map(|a| match a {
            Asset::Generic(g) if g.name.eq_ignore_ascii_case(name) => XAnim::from_node(&g.name, &g.root, &lib.zone.script_strings).ok().map(Arc::new),
            _ => None,
        });
        decoded.insert(name.to_owned(), anim.clone());
        anim
    }
}

pub use library::{anim, is_cover_anim, ready};

/// Read the cover animations as a match starts, in the game type that has
/// them.
fn load_anims() {
    if active() {
        library::start();
    }
}

/// The animation for a pawn in cover (None: the usual crouch or stand
/// loop, which is what sliding along the wall uses).
pub fn anim_name(c: &InCover, lean: f32, aiming: bool, firing: bool, moving: bool, age: f32) -> Option<&'static str> {
    if moving {
        return None;
    }
    // Just taken: the low wall's stand-to-hide.
    if !c.high && age < 0.9 && !aiming && !firing {
        return Some("covercrouch_stand_2_hide");
    }
    Some(match (c.high, lean, aiming) {
        (true, l, true) if l != 0.0 => if l > 0.0 { "cornerstndr_lean_aim_5" } else { "cornerstndl_lean_aim_5" },
        (true, l, false) if l != 0.0 => if l > 0.0 { "cornerstndr_lean_idle" } else { "cornerstndl_lean_idle" },
        (true, _, false) if firing => "coverstand_blindfire_1",
        (true, _, _) => "coverstand_hide_idle",
        (false, _, true) => "covercrouch_aim5",
        (false, _, false) if firing => "covercrouch_blindfire_1",
        (false, _, false) => "covercrouch_hide_idle",
    })
}


// ---------------------------------------------------------------------
// Bots in cover (3rd Person TDM only): shot at, a bot runs to cover that
// puts a wall between it and its attacker, holds it crouched or standing
// behind, and peeks now and then to fire; then its own behaviour goes on.

pub mod bots {
    use super::*;
    use crate::bots::Bot;
    use crate::bots::nav::NavGraph;
    use crate::combat::{Damage, Pawn, hostile};
    use std::collections::HashMap;

    /// Cover found near the nav graph's nodes, grown a few nodes per frame.
    #[derive(Resource, Default)]
    pub struct CoverMap {
        pub points: Vec<CoverPoint>,
        next: usize,
    }

    /// Nodes looked at per frame.
    const PER_FRAME: usize = 120;
    /// How far a shot bot will run for cover (m), and how soon it can do so again.
    const RUN_MAX: f32 = 30.0;
    const COOLDOWN: f32 = 7.0;
    /// Moves toward the enemy from cover to cover, at most this many in a row.
    const MAX_HOPS: u8 = 3;

    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Phase {
        Run,
        Hide,
    }

    /// A bot using cover.
    #[derive(Component, Clone, Copy, Debug)]
    pub struct BotCover {
        to: CoverPoint,
        phase: Phase,
        until: f32,
        /// Peeking (standing, aiming and firing as its own behaviour
        /// wants) until this time; next peek after.
        peek_until: f32,
        next_peek: f32,
        /// Cover to cover so far, and where it set off from (to see it moves).
        hops: u8,
        last_pos: Vec3,
        last_moved: f32,
    }

    pub fn grow_map(mut map: ResMut<CoverMap>, nav: Option<Res<NavGraph>>, spatial: SpatialQuery) {
        if !super::active() {
            return;
        }
        let Some(nav) = nav else {
            map.points.clear();
            map.next = 0;
            return;
        };
        if nav.is_added() {
            map.points.clear();
            map.next = 0;
        }
        let end = (map.next + PER_FRAME).min(nav.nodes.len());
        for i in map.next..end {
            let node = &nav.nodes[i];
            if let Some(p) = find_cover(&spatial, node.pos, Vec3::ZERO)
                && map.points.iter().rev().take(60).all(|o| o.pos.distance(p.pos) > u(36.0))
            {
                map.points.push(p);
            }
        }
        map.next = end;
    }

    /// The nearest cover to `from` with a wall between it and `threat` (the
    /// threat is on the far side of the wall, against its normal), at least
    /// `gain` metres closer to the threat than `from` is when advancing.
    fn pick(spatial: &SpatialQuery, map: &CoverMap, from: Vec3, threat: Vec3, within: f32, advance: Option<f32>) -> Option<CoverPoint> {
        let now_far = Vec3::new(threat.x - from.x, 0.0, threat.z - from.z).length();
        let mut found: Vec<CoverPoint> = map
            .points
            .iter()
            .filter(|p| {
                let to_threat = Vec3::new(threat.x - p.pos.x, 0.0, threat.z - p.pos.z);
                let far = to_threat.length();
                let cover = to_threat.normalize_or_zero().dot(p.normal) < -0.3;
                let d = p.pos.distance(from);
                cover && d < within && d > u(60.0) * advance.map_or(0.0, |_| 1.0) && advance.is_none_or(|gain| far < now_far - gain)
            })
            .copied()
            .collect();
        found.sort_by(|a, b| a.pos.distance(from).total_cmp(&b.pos.distance(from)));
        // The nearest it can run to in a straight line (not through a wall).
        found.into_iter().take(10).find(|p| {
            let (a, b) = (from + Vec3::Y * u(30.0), p.pos + Vec3::Y * u(30.0));
            probe(spatial, a, b - a, a.distance(b)).is_none()
        })
    }

    #[allow(clippy::type_complexity)]
    pub fn cover_bots(
        mut commands: Commands,
        time: Res<Time>,
        map: Res<CoverMap>,
        spatial: SpatialQuery,
        mut damage: MessageReader<Damage>,
        places: Query<&GlobalTransform>,
        enemies: Query<(&Pawn, &Transform), Without<Dead>>,
        mut bots: Query<(Entity, &Pawn, &Transform, &mut MoveInput, &mut WeaponInput, Option<&mut BotCover>, Has<InCover>), (With<Bot>, Without<Dead>)>,
        dead: Query<Entity, (With<Bot>, With<Dead>, Or<(With<BotCover>, With<InCover>)>)>,
        mut cooldown: Local<HashMap<Entity, f32>>,
    ) {
        if !super::active() {
            damage.clear();
            return;
        }
        let now = time.elapsed_secs();
        // The dead leave their cover (a respawn is somewhere else).
        for e in &dead {
            commands.entity(e).remove::<(BotCover, InCover)>();
        }
        // Shot at: run for cover from the attacker.
        for d in damage.read() {
            let Ok((bot, _, tf, _, _, cover, _)) = bots.get(d.target) else { continue };
            if cover.is_some() || cooldown.get(&bot).is_some_and(|&t| now < t) {
                continue;
            }
            let Some(threat) = d.attacker.and_then(|a| places.get(a).ok()).map(|t| t.translation()) else { continue };
            if let Some(to) = pick(&spatial, &map, tf.translation, threat, RUN_MAX, None) {
                bevy::log::info!("bot cover: {bot} shot at, runs {:.1} m to cover", to.pos.distance(tf.translation));
                commands.entity(bot).insert(BotCover { to, phase: Phase::Run, until: now + 6.0, peek_until: 0.0, next_peek: 0.0, hops: 0, last_pos: tf.translation, last_moved: now });
            }
        }
        for (bot, pawn, tf, mut mi, mut wi, cover, in_cover) in &mut bots {
            let Some(mut c) = cover else { continue };
            let delta = Vec3::new(c.to.pos.x - tf.translation.x, 0.0, c.to.pos.z - tf.translation.z);
            let high = c.to.high;
            let giving_up = |commands: &mut Commands, cooldown: &mut HashMap<Entity, f32>, why: &str| {
                bevy::log::info!("bot cover: {bot} done ({why})");
                cooldown.insert(bot, now + COOLDOWN);
                commands.entity(bot).remove::<(BotCover, InCover)>().insert(LeavingCover { since: now, high });
            };
            match c.phase {
                Phase::Run => {
                    // Moving at all? (Stuck on something: give up.)
                    if tf.translation.distance(c.last_pos) > u(12.0) {
                        c.last_pos = tf.translation;
                        c.last_moved = now;
                    }
                    if now > c.until || now - c.last_moved > 1.2 {
                        bevy::log::info!("bot cover: {bot} did not arrive, {:.1} m short", delta.length());
                        giving_up(&mut commands, &mut cooldown, "no arrival");
                    } else if delta.length() < u(16.0) {
                        bevy::log::info!("bot cover: {bot} in cover");
                        c.phase = Phase::Hide;
                        c.until = now + 3.0 + (bot.to_bits() % 20) as f32 * 0.15;
                        c.next_peek = now + 1.2;
                        // The same cover poses as the player's.
                        commands.entity(bot).insert(InCover { normal: c.to.normal, tangent: c.to.normal.cross(Vec3::Y), high: c.to.high, open: [false; 2], leaving: 0.0, since: now, snap: None });
                    } else {
                        mi.sprint = true;
                        steer(&mut mi, tf, delta);
                    }
                }
                Phase::Hide => {
                    if now > c.until {
                        // On toward the nearest enemy, cover to cover, else done.
                        let nearest = enemies
                            .iter()
                            .filter(|(p, _)| hostile(pawn, p))
                            .map(|(_, t)| t.translation)
                            .min_by(|a, b| a.distance(tf.translation).total_cmp(&b.distance(tf.translation)));
                        let next = nearest.filter(|_| c.hops < MAX_HOPS).and_then(|enemy| pick(&spatial, &map, tf.translation, enemy, 18.0, Some(3.0)));
                        match next {
                            Some(to) => {
                                bevy::log::info!("bot cover: {bot} on to cover {:.1} m closer to the enemy (hop {})", to.pos.distance(tf.translation), c.hops + 1);
                                c.to = to;
                                c.phase = Phase::Run;
                                c.until = now + 6.0;
                                c.hops += 1;
                                c.last_pos = tf.translation;
                                c.last_moved = now;
                                commands.entity(bot).remove::<InCover>();
                            }
                            None => giving_up(&mut commands, &mut cooldown, "held"),
                        }
                        continue;
                    }
                    mi.forward = 0.0;
                    mi.right = 0.0;
                    mi.sprint = false;
                    if now < c.peek_until {
                        // Up and firing as its own aim wants, out of the cover pose.
                        mi.stance = Stance::Stand;
                        if in_cover {
                            commands.entity(bot).remove::<InCover>();
                        }
                    } else {
                        mi.stance = if c.to.high { Stance::Stand } else { Stance::Crouch };
                        wi.fire = false;
                        wi.ads = false;
                        if !in_cover {
                            commands.entity(bot).insert(InCover { normal: c.to.normal, tangent: c.to.normal.cross(Vec3::Y), high: c.to.high, open: [false; 2], leaving: 0.0, since: now - 5.0, snap: None });
                        }
                        if now > c.next_peek {
                            c.peek_until = now + 0.9;
                            c.next_peek = now + 1.8;
                        }
                    }
                }
            }
        }
    }

    /// Walk input toward a world-space offset, for a pawn whose view the
    /// bot's own aim sets: the offset in the body's frame.
    fn steer(mi: &mut MoveInput, tf: &Transform, delta: Vec3) {
        let local = tf.rotation.inverse() * delta.normalize_or_zero();
        mi.forward = -local.z;
        mi.right = local.x;
    }

    /// Test aid (`COD4RW_COVER_BOTSHOT=<dir>`, with `COD4RW_COVER`): when a
    /// bot is in cover, stand the player on its side of the wall four
    /// metres off, looking at it, and save a shot; three of them.
    #[allow(clippy::type_complexity)]
    pub fn botshot(
        mut commands: Commands,
        time: Res<Time>,
        bots: Query<(&Transform, &InCover), (With<Bot>, Without<LocalSlot>)>,
        mut player: Query<(&mut Transform, &mut ViewAngles), (With<LocalSlot>, Without<Bot>)>,
        mut state: Local<(u32, f32, f32)>,
        mut exit: MessageWriter<AppExit>,
    ) {
        use bevy::render::view::screenshot::{Screenshot, save_to_disk};
        let Ok(dir) = std::env::var("COD4RW_COVER_BOTSHOT") else { return };
        let now = time.elapsed_secs();
        if now > 150.0 || state.0 >= 3 {
            exit.write(AppExit::Success);
            return;
        }
        let Ok((mut tf, mut view)) = player.single_mut() else { return };
        // A shot pending.
        if state.1 > 0.0 {
            if now > state.1 {
                let _ = std::fs::create_dir_all(&dir);
                commands.spawn(Screenshot::primary_window()).observe(save_to_disk(format!("{dir}/bot_cover_{}.png", state.0)));
                state.0 += 1;
                state.1 = 0.0;
                state.2 = now + 6.0;
            }
            return;
        }
        if now < state.2.max(20.0) {
            return;
        }
        if let Some((bot_tf, c)) = bots.iter().next() {
            tf.translation = bot_tf.translation + c.normal * u(4.5);
            view.yaw = c.normal.x.atan2(c.normal.z);
            view.pitch = -0.05;
            state.1 = now + 0.5;
        }
    }
}

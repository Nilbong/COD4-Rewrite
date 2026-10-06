//! Player movement: a port of CoD4's `Pmove` (bg_pmove / bg_slidemove /
//! bg_jump), shared by the local player and bots.
//!
//! Behaviour and constants follow the shipped game: Quake 3 style friction
//! and acceleration with CoD4's `max(stopspeed, wishspeed)` acceleration base,
//! the walk/back/strafe/sprint/stance speed scales, sprint stamina, jump
//! landing slowdown, view height transition curves and a capsule hull.
//!
//! The simulation works in Bevy space (metres, Y up); tunables are written in
//! CoD units and converted with [`u`].

use crate::collision;
use crate::units::u;
use avian3d::prelude::*;
use bevy::prelude::*;

mod mantle;
pub use mantle::{Mantle, MantleAnims};

pub struct MovementPlugin;

impl Plugin for MovementPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<Landed>()
            .init_resource::<MoveTuning>()
            .add_systems(Update, (mantle::load_anims, move_pawns.in_set(MovementSet)).chain().run_if(crate::state::in_game));
    }
}

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct MovementSet;

// --- dvar defaults (g_gravity, g_speed, friction, stopspeed, player_*, jump_*)
pub const GRAVITY: f32 = u(800.0);
pub const RUN_SPEED: f32 = u(190.0);
pub const FRICTION: f32 = 5.5;
pub const STOP_SPEED: f32 = u(100.0);
pub const ACCELERATE: f32 = 9.0;
pub const AIR_ACCELERATE: f32 = 1.0;
pub const BACK_SPEED_SCALE: f32 = 0.7;
pub const STRAFE_SPEED_SCALE: f32 = 0.8;
pub const SPRINT_SPEED_SCALE: f32 = 1.5;
pub const SPRINT_STRAFE_SPEED_SCALE: f32 = 0.667;
/// Forward input (out of 127) needed to start or keep sprinting.
pub const SPRINT_FORWARD_MINIMUM: f32 = 105.0;
pub const SPRINT_TIME: f32 = 4.0;
pub const SPRINT_MIN_TIME: f32 = 1.0;
pub const JUMP_HEIGHT: f32 = u(39.0);
pub const STEP_HEIGHT: f32 = u(18.0);
const PRONE_STEP_HEIGHT: f32 = u(10.0);
/// Minimum time between jumps.
const JUMP_INTERVAL_MS: f32 = 500.0;
const JUMP_LAND_SLOWDOWN_MS: f32 = 1800.0;
pub const HULL_RADIUS: f32 = u(15.0);
const MIN_WALK_NORMAL: f32 = 0.7;
/// Clip planes are pushed slightly further than parallel (Q3 OVERCLIP).
const OVERCLIP: f32 = 1.001;
/// Traces stop this far from surfaces.
const SKIN: f32 = u(0.125);
/// A trace down that hits an edge with a rounded normal this far up hit
/// it under the hull, not at its side.
const EDGE_UNDER: f32 = 0.3;
/// Ground probe distance.
const GROUND_PROBE: f32 = u(0.25);
/// When walking, stay glued to the ground over drops this small.
const STICK_DOWN: f32 = u(9.0);
/// `player_meleeChargeFriction`: a melee lunge's deceleration (units/s²).
const MELEE_CHARGE_FRICTION: f32 = 1200.0;
/// `jump_stepSize`: how far a jump steps up onto something it hits.
const JUMP_STEP_SIZE: f32 = u(18.0);
/// A step is tried on ground this steep even without being blocked, and
/// may land on anything less steep than this.
const STEP_ON_SLOPE_NORMAL: f32 = 0.9;
const STEP_LAND_NORMAL: f32 = 0.3;
/// Share of the speed a full step's height change costs.
const STEP_SLOWDOWN: f32 = 0.8;
const MAX_SUBSTEP: f32 = 1.0 / 125.0;
/// How far leaning moves the eye sideways.
pub const LEAN_DISTANCE: f32 = u(16.0);
/// Seconds to lean fully out.
const LEAN_TIME: f32 = 0.25;

/// Movement feel shared by every pawn: CoD4's dvar defaults, or the heavier
/// Bodycam set (see [`crate::bodycam`]).
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct MoveTuning {
    /// Multiplies every walking and air speed (`g_speed`).
    pub speed_scale: f32,
    pub accelerate: f32,
    pub friction: f32,
    pub sprint_speed_scale: f32,
}

impl MoveTuning {
    pub const COD4: MoveTuning =
        MoveTuning { speed_scale: 1.0, accelerate: ACCELERATE, friction: FRICTION, sprint_speed_scale: SPRINT_SPEED_SCALE };
}

impl Default for MoveTuning {
    fn default() -> Self {
        MoveTuning::COD4
    }
}

// --- view bob (bg_bobAmplitude*, bg_bobMax, player_sprintCameraBob,
// player_moveThreshhold and PM_GetBobMove's factor table)
/// (horizontal, vertical) bob amplitude in inches per unit/s of speed.
const BOB_AMPLITUDE_STANDING: (f32, f32) = (0.007, 0.007);
const BOB_AMPLITUDE_DUCKED: (f32, f32) = (0.0075, 0.0075);
const BOB_AMPLITUDE_PRONE: (f32, f32) = (0.02, 0.005);
const BOB_AMPLITUDE_SPRINTING: (f32, f32) = (0.02, 0.014);
const BOB_MAX: f32 = 8.0;
/// Bob cycle advance (in 1/256ths of a cycle per ms at full speed) moving
/// (forward, backward).
const BOB_FACTOR_STAND: (f32, f32) = (0.335, 0.36);
const BOB_FACTOR_CROUCH: (f32, f32) = (0.34, 0.34);
const BOB_FACTOR_PRONE: (f32, f32) = (0.25, 0.25);
const SPRINT_CAMERA_BOB: f32 = 0.5;
const MOVE_THRESHOLD: f32 = u(10.0);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Stance {
    #[default]
    Stand,
    Crouch,
    Prone,
}

impl Stance {
    pub fn hull_height(self) -> f32 {
        match self {
            Stance::Stand => u(70.0),
            Stance::Crouch => u(50.0),
            Stance::Prone => u(30.0),
        }
    }

    pub fn eye_height(self) -> f32 {
        match self {
            Stance::Stand => u(60.0),
            Stance::Crouch => u(40.0),
            Stance::Prone => u(11.0),
        }
    }

    /// `PM_CmdScaleForStance`.
    pub fn speed_scale(self) -> f32 {
        match self {
            Stance::Stand => 1.0,
            Stance::Crouch => 0.65,
            Stance::Prone => 0.15,
        }
    }
}

/// View height keyframes (percent of the transition, height in CoD units),
/// straight from the game's `viewLerp_*` tables.
const VIEW_STAND_CROUCH: &[(f32, f32)] =
    &[(0.0, 60.0), (1.0, 59.5), (4.0, 58.5), (30.0, 56.0), (80.0, 44.0), (90.0, 41.5), (95.0, 40.5), (100.0, 40.0)];
const VIEW_CROUCH_STAND: &[(f32, f32)] =
    &[(0.0, 40.0), (5.0, 40.5), (10.0, 41.5), (20.0, 44.0), (70.0, 56.0), (96.0, 58.5), (99.0, 59.5), (100.0, 60.0)];
const VIEW_CROUCH_PRONE: &[(f32, f32)] = &[
    (0.0, 40.0),
    (11.0, 38.0),
    (22.0, 33.0),
    (34.0, 25.0),
    (45.0, 16.0),
    (50.0, 15.0),
    (55.0, 16.0),
    (70.0, 18.0),
    (90.0, 17.0),
    (100.0, 11.0),
];
const VIEW_PRONE_CROUCH: &[(f32, f32)] =
    &[(0.0, 11.0), (5.0, 10.0), (30.0, 21.0), (50.0, 25.0), (67.0, 31.0), (83.0, 34.0), (100.0, 40.0)];

fn sample_curve(curve: &[(f32, f32)], percent: f32) -> f32 {
    let p = percent.clamp(0.0, 100.0);
    for w in curve.windows(2) {
        let (a, b) = (w[0], w[1]);
        if p <= b.0 {
            let t = if b.0 > a.0 { (p - a.0) / (b.0 - a.0) } else { 1.0 };
            return a.1 + (b.1 - a.1) * t;
        }
    }
    curve.last().map_or(60.0, |c| c.1)
}

/// What a pawn wants to do this frame (from the keyboard or from bot AI).
#[derive(Component, Clone, Copy, Debug)]
pub struct MoveInput {
    /// -1..1, forward positive.
    pub forward: f32,
    /// -1..1, right positive.
    pub right: f32,
    pub jump: bool,
    pub sprint: bool,
    pub stance: Stance,
    /// Weapon move speed scale (already including ADS).
    pub speed_scale: f32,
    /// -1..1, lean left/right.
    pub lean: f32,
}

impl Default for MoveInput {
    fn default() -> Self {
        MoveInput {
            forward: 0.0,
            right: 0.0,
            jump: false,
            sprint: false,
            stance: Stance::Stand,
            speed_scale: 1.0,
            lean: 0.0,
        }
    }
}

/// Look direction in radians (Bevy convention: yaw about +Y, pitch about +X).
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct ViewAngles {
    pub yaw: f32,
    pub pitch: f32,
}

impl ViewAngles {
    pub fn rotation(&self) -> Quat {
        Quat::from_euler(EulerRot::YXZ, self.yaw, self.pitch, 0.0)
    }

    pub fn forward(&self) -> Vec3 {
        self.rotation() * Vec3::NEG_Z
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct SprintState {
    /// Times in ms on the mover's clock; `last_start == 0` means never sprinted.
    last_start: f32,
    last_end: f32,
    start_max_length: f32,
    button_up_required: bool,
}

#[derive(Component, Clone, Debug)]
pub struct Mover {
    pub velocity: Vec3,
    /// On walkable ground (`pml.walking`).
    pub on_ground: bool,
    pub ground_normal: Vec3,
    /// Touching ground that is too steep to walk on.
    ground_plane: bool,
    pub stance: Stance,
    stance_from: Stance,
    stance_elapsed_ms: f32,
    /// Eye height above the feet, following the game's transition curves.
    pub eye_height: f32,
    pub sprinting: bool,
    /// Sprint stamina left in seconds.
    pub sprint_left: f32,
    /// Scales the longest sprint (Extreme Conditioning).
    pub sprint_time_scale: f32,
    sprint: SprintState,
    /// `PMF_JUMPING`: set on jump, cleared when the landing slowdown ends.
    jumping: bool,
    /// Landing slowdown time remaining (`pm_time`), 0 while still airborne.
    land_ms: f32,
    last_jump_ms: f32,
    jump_origin_y: f32,
    jump_held: bool,
    /// Highest point of the current fall, for fall damage.
    fall_start_y: f32,
    clock_ms: f32,
    /// Position in the footstep/bob cycle, 0..1 (`playerState_s::bobCycle`).
    pub bob_cycle: f32,
    /// -1..1 lean, already limited by nearby walls.
    pub lean: f32,
    /// Climbing onto or over something.
    pub mantle: Option<Mantle>,
    /// A mantle is there to be had (the player's own; IW3 shows a hint).
    pub mantle_hint: bool,
    /// A melee lunge under way: its horizontal velocity ([`crate::melee`]).
    pub charge: Option<Vec3>,
}

impl Default for Mover {
    fn default() -> Self {
        Mover {
            velocity: Vec3::ZERO,
            on_ground: false,
            ground_normal: Vec3::Y,
            ground_plane: false,
            stance: Stance::Stand,
            stance_from: Stance::Stand,
            stance_elapsed_ms: f32::MAX,
            eye_height: Stance::Stand.eye_height(),
            sprinting: false,
            sprint_left: SPRINT_TIME,
            sprint_time_scale: 1.0,
            sprint: SprintState::default(),
            jumping: false,
            land_ms: 0.0,
            last_jump_ms: -10_000.0,
            jump_origin_y: 0.0,
            jump_held: false,
            fall_start_y: f32::MIN,
            clock_ms: 1.0,
            bob_cycle: 0.0,
            lean: 0.0,
            mantle: None,
            mantle_hint: false,
            charge: None,
        }
    }
}

impl Mover {
    pub fn eye(&self, feet: Vec3) -> Vec3 {
        feet + Vec3::Y * self.eye_height
    }

    pub fn horizontal_speed(&self) -> f32 {
        Vec2::new(self.velocity.x, self.velocity.z).length()
    }

    /// Eye offset from leaning, for a view yaw.
    pub fn lean_offset(&self, yaw: f32) -> Vec3 {
        let (_, right) = yaw_vectors(yaw);
        right * self.lean * LEAN_DISTANCE - Vec3::Y * self.lean.abs() * u(2.0)
    }

    fn stance_lerp_ms(&self) -> f32 {
        if self.stance == Stance::Prone || self.stance_from == Stance::Prone { 400.0 } else { 200.0 }
    }

    /// 0..1 progress of the current stance transition.
    fn stance_progress(&self) -> f32 {
        (self.stance_elapsed_ms / self.stance_lerp_ms()).clamp(0.0, 1.0)
    }

    /// Stance speed scale, blended during transitions like `PM_CmdScaleForStance`.
    fn stance_speed_scale(&self) -> f32 {
        let t = self.stance_progress();
        self.stance_from.speed_scale() * (1.0 - t) + self.stance.speed_scale() * t
    }

    fn update_view_height(&mut self) {
        let p = self.stance_progress() * 100.0;
        let h = match (self.stance_from, self.stance) {
            (a, b) if a == b => b.eye_height() / crate::units::INCH,
            (Stance::Stand, Stance::Crouch) => sample_curve(VIEW_STAND_CROUCH, p),
            (Stance::Crouch, Stance::Stand) => sample_curve(VIEW_CROUCH_STAND, p),
            (Stance::Crouch, Stance::Prone) => sample_curve(VIEW_CROUCH_PRONE, p),
            (Stance::Prone, Stance::Crouch) => sample_curve(VIEW_PRONE_CROUCH, p),
            (Stance::Stand, Stance::Prone) => {
                if p < 50.0 {
                    sample_curve(VIEW_STAND_CROUCH, p * 2.0)
                } else {
                    sample_curve(VIEW_CROUCH_PRONE, (p - 50.0) * 2.0)
                }
            }
            (Stance::Prone, Stance::Stand) => {
                if p < 50.0 {
                    sample_curve(VIEW_PRONE_CROUCH, p * 2.0)
                } else {
                    sample_curve(VIEW_CROUCH_STAND, (p - 50.0) * 2.0)
                }
            }
            _ => self.stance.eye_height() / crate::units::INCH,
        };
        self.eye_height = u(h);
    }

    /// `PM_GetSprintLeft`, in ms.
    fn sprint_left_ms(&self) -> f32 {
        let max = SPRINT_TIME * self.sprint_time_scale * 1000.0;
        let s = &self.sprint;
        let left = if s.last_start == 0.0 {
            max
        } else if s.last_start > s.last_end {
            s.start_max_length - (self.clock_ms - s.last_start)
        } else {
            let at_end = s.start_max_length - (s.last_end - s.last_start);
            at_end + (self.clock_ms - s.last_end)
        };
        left.clamp(0.0, max)
    }

    /// `Jump_GetLandFactor`: friction/jump penalty right after landing.
    fn land_factor(&self) -> f32 {
        if self.land_ms < 1700.0 { self.land_ms * 1.5 / 1700.0 + 1.0 } else { 2.5 }
    }
}

/// Fired when a pawn lands; `fall_height` is in CoD units.
#[derive(Message)]
pub struct Landed {
    pub entity: Entity,
    pub fall_height: f32,
}

/// Pawns that are dead or frozen don't move.
#[derive(Component)]
pub struct Frozen;

/// CoD4 traces the player as a capsule spanning the stance's bounds.
fn hull(stance: Stance) -> Collider {
    let length = (stance.hull_height() - 2.0 * HULL_RADIUS).max(0.0);
    if length > 0.0 { Collider::capsule(HULL_RADIUS, length) } else { Collider::sphere(HULL_RADIUS) }
}

/// The hull is centred halfway up from the feet.
fn center(feet: Vec3, stance: Stance) -> Vec3 {
    feet + Vec3::Y * stance.hull_height() * 0.5
}

struct Trace {
    fraction: f32,
    end: Vec3,
    normal: Vec3,
    hit: bool,
}

struct Ctx<'a, 'w, 's> {
    /// Mantle animations, and the `mantle_over` surfaces.
    mantle: Option<&'a MantleAnims>,
    mantle_over: &'a [Entity],
    spatial: &'a SpatialQuery<'w, 's>,
    filter: &'a SpatialQueryFilter,
    tuning: MoveTuning,
    /// The brushes' faces ([`collision::BrushFaces`]).
    faces: Option<&'a collision::BrushFaces>,
}

impl Ctx<'_, '_, '_> {
    /// Sweep the stance hull from `start` (feet) to `end`. Like a Quake
    /// trace, the hull stops [`SKIN`] short of whatever it hits.
    fn trace(&self, start: Vec3, end: Vec3, stance: Stance) -> Trace {
        let delta = end - start;
        let len = delta.length();
        let Ok(dir) = Dir3::new(delta) else {
            return Trace { fraction: 1.0, end, normal: Vec3::Y, hit: false };
        };
        let shape = hull(stance);
        let origin = center(start, stance);
        let cast = |ignore_start: bool| {
            let config = ShapeCastConfig {
                max_distance: len + SKIN,
                target_distance: 0.0,
                ignore_origin_penetration: ignore_start,
                ..ShapeCastConfig::DEFAULT
            };
            self.spatial.cast_shape(&shape, origin, Quat::IDENTITY, dir, &config, self.filter)
        };
        let mut hit = cast(false);
        // Already touching something we're moving along or away from (e.g.
        // the floor while walking): look past it.
        if let Some(h) = &hit {
            if h.distance <= 1e-5 && h.normal1.dot(*dir) >= -0.05 && -h.normal2.dot(*dir) >= -0.05 {
                hit = cast(true);
            }
        }
        match hit {
            Some(h) if h.distance < len + SKIN => {
                let mut n = h.normal1.normalize_or(-*dir);
                if n.dot(*dir) > 0.0 {
                    n = -n;
                }
                // CoD4's brush traces give the face hit, even at an edge;
                // the capsule's contact there comes out rounded (the round
                // bottom wedged between two stair edges reads as a steep
                // slope, and you're stuck). Going down onto an edge under
                // the hull (the ground, landing a step): the brush's face
                // through the contact, when it's ground to stand on, as
                // CoD4's. Not an edge at the hull's side (brushing past a
                // ledge), nor sideways: there the rounded normal slides
                // over it, and a face picked there pins you.
                if dir.y < -0.7
                    && n.y > EDGE_UNDER
                    && let Some(f) = self.faces.and_then(|f| f.face(h.entity, h.point1, n, *dir)).filter(|f| f.y >= MIN_WALK_NORMAL)
                {
                    n = f;
                }
                let d = (h.distance - SKIN).clamp(0.0, len);
                Trace { fraction: d / len, end: start + *dir * d, normal: n, hit: true }
            }
            _ => Trace { fraction: 1.0, end, normal: Vec3::Y, hit: false },
        }
    }

    fn solid_at(&self, feet: Vec3, stance: Stance) -> bool {
        !self.spatial.shape_intersections(&hull(stance), center(feet, stance), Quat::IDENTITY, self.filter).is_empty()
    }
}

/// Where a standing player at `feet` (facing `yaw`) would end up mantling,
/// and the way in, if there's a mantle in front of them. `over` lists the
/// `mantle_over` surfaces.
pub fn mantle_landing(spatial: &SpatialQuery, feet: Vec3, yaw: f32, over: &[Entity]) -> Option<(Vec3, Vec3)> {
    let filter = collision::movement_filter();
    let ctx = Ctx { mantle: None, mantle_over: over, spatial, filter: &filter, tuning: MoveTuning::COD4, faces: None };
    mantle::landing(&ctx, feet, yaw, over)
}

fn move_pawns(
    time: Res<Time>,
    tuning: Res<MoveTuning>,
    spatial: SpatialQuery,
    mut pawns: Query<(Entity, &mut Transform, &mut Mover, &MoveInput, &ViewAngles, Has<crate::player::LocalPlayer>), Without<Frozen>>,
    mut landed: MessageWriter<Landed>,
    mantle_anims: Option<Res<MantleAnims>>,
    mantle_surfaces: Query<(Entity, &collision::MantleSurface)>,
    faces: Option<Res<collision::BrushFaces>>,
) {
    let dt = time.delta_secs().min(0.1);
    let steps = (dt / MAX_SUBSTEP).ceil().max(1.0);
    let step_dt = dt / steps;
    let filter = collision::movement_filter();
    let over: Vec<Entity> = mantle_surfaces.iter().filter(|(_, s)| s.over).map(|(e, _)| e).collect();
    let ctx = Ctx { mantle: mantle_anims.as_deref(), mantle_over: &over, spatial: &spatial, filter: &filter, tuning: *tuning, faces: faces.as_deref() };
    for (entity, mut tf, mut mover, input, view, local) in &mut pawns {
        tf.rotation = Quat::from_rotation_y(view.yaw);
        let mut pos = tf.translation;
        for _ in 0..steps as u32 {
            if let Some(h) = pmove(&ctx, &mut pos, &mut mover, input, view.yaw, step_dt) {
                landed.write(Landed { entity, fall_height: h });
            }
        }
        update_lean(&ctx, pos, &mut mover, input, view.yaw, dt);
        // The player's mantle hint (only theirs: it's a few traces).
        mover.mantle_hint = local
            && mover.mantle.is_none()
            && ctx.mantle.is_some_and(|a| mantle::available(&ctx, pos, &mover, view.yaw, a, ctx.mantle_over));
        tf.translation = pos;
    }
}

/// One `PmoveSingle`. Returns the fall height when landing.
fn pmove(ctx: &Ctx, pos: &mut Vec3, m: &mut Mover, input: &MoveInput, yaw: f32, dt: f32) -> Option<f32> {
    let msec = dt * 1000.0;
    m.clock_ms += msec;

    // usercmd-style inputs in -127..127.
    let fmove = (input.forward.clamp(-1.0, 1.0) * 127.0).round();
    let mut smove = (input.right.clamp(-1.0, 1.0) * 127.0).round();

    // Drop the landing timer; the jump state ends with it.
    if m.jumping && m.land_ms > 0.0 {
        m.land_ms = (m.land_ms - msec).max(0.0);
        if m.land_ms == 0.0 {
            m.jumping = false;
        }
    }

    // Mantling: following the climb, nothing else.
    if m.mantle.is_some() {
        match ctx.mantle {
            Some(anims) => mantle::step(pos, m, anims, dt),
            None => m.mantle = None,
        }
        if m.mantle.is_some_and(|mt| mt.crouch) {
            m.stance = Stance::Crouch;
        }
        m.jump_held = input.jump;
        m.fall_start_y = pos.y;
        return None;
    }

    update_sprint(m, input, fmove);
    check_duck(ctx, *pos, m, input, msec);
    let was_on_ground = m.on_ground;
    ground_trace(ctx, *pos, m);

    // Holding jump at a mantle surface climbs it instead of jumping.
    if input.jump {
        if let Some(anims) = ctx.mantle {
            if mantle::check(ctx, pos, m, yaw, anims, ctx.mantle_over) {
                return None;
            }
        }
    }

    // A melee lunge (`PM_MeleeChargeUpdate`): straight at them at the speed
    // `player_meleeChargeFriction` brings to a stop there, slowed by it
    // alone.
    if let Some(v) = m.charge {
        let speed = (v.length() - u(MELEE_CHARGE_FRICTION) * dt).max(0.0);
        m.charge = (speed > 0.0).then(|| v.normalize_or_zero() * speed);
        m.sprinting = false;
        m.velocity = Vec3::new(v.x, m.velocity.y.min(0.0), v.z);
        step_slide_move(ctx, pos, m, !m.on_ground, dt);
        m.jump_held = input.jump;
        ground_trace(ctx, *pos, m);
        if m.on_ground {
            m.fall_start_y = pos.y;
        }
        return None;
    }

    if m.on_ground {
        if m.sprinting {
            smove *= SPRINT_STRAFE_SPEED_SCALE;
        }
        if jump_check(m, input, *pos) {
            air_move(ctx, pos, m, fmove, smove, yaw, dt);
        } else {
            walk_move(ctx, pos, m, input, fmove, smove, yaw, dt);
        }
    } else {
        air_move(ctx, pos, m, fmove, smove, yaw, dt);
    }
    m.jump_held = input.jump;

    ground_trace(ctx, *pos, m);
    update_bob_cycle(m, input, &ctx.tuning, fmove, input.right.clamp(-1.0, 1.0) * 127.0, msec);

    // Track the fall for damage and landing.
    if !m.on_ground {
        m.fall_start_y = m.fall_start_y.max(pos.y);
        return None;
    }
    let mut result = None;
    if !was_on_ground {
        if m.jumping && m.land_ms == 0.0 {
            jump_apply_slowdown(m, pos.y);
        }
        let fall = (m.fall_start_y - pos.y).max(0.0) / crate::units::INCH;
        result = Some(fall);
    }
    m.fall_start_y = pos.y;
    result
}

/// `PM_Footsteps`' bob cycle: advances with input while on the ground, at a
/// rate set by stance and by how close the player is to full speed.
fn update_bob_cycle(m: &mut Mover, input: &MoveInput, t: &MoveTuning, fmove: f32, smove: f32, msec: f32) {
    let speed = m.horizontal_speed();
    if !m.on_ground || (fmove == 0.0 && smove == 0.0) || speed < MOVE_THRESHOLD {
        return;
    }
    let mut full_speed = RUN_SPEED * t.speed_scale;
    if fmove != 0.0 && smove != 0.0 {
        full_speed *= ((STRAFE_SPEED_SCALE - 1.0) * 0.75 + 2.0) * 0.5;
    } else if smove != 0.0 {
        full_speed *= (STRAFE_SPEED_SCALE - 1.0) * 0.75 + 1.0;
    }
    if fmove < 0.0 {
        full_speed *= if smove != 0.0 { (1.0 + BACK_SPEED_SCALE) * 0.5 } else { BACK_SPEED_SCALE };
    } else if m.sprinting {
        full_speed *= t.sprint_speed_scale;
    }
    full_speed *= input.speed_scale * m.stance_speed_scale();
    let (forward, back) = match m.stance {
        Stance::Stand => BOB_FACTOR_STAND,
        Stance::Crouch => BOB_FACTOR_CROUCH,
        Stance::Prone => BOB_FACTOR_PRONE,
    };
    let factor = if fmove < 0.0 {
        back
    } else if m.sprinting && m.stance == Stance::Stand {
        SPRINT_CAMERA_BOB
    } else {
        forward
    };
    if full_speed > 0.0 {
        m.bob_cycle = (m.bob_cycle + factor * (speed / full_speed) * msec / 256.0).fract();
    }
}

impl Mover {
    /// `bg_bobAmplitude*`: (horizontal, vertical) bob per unit/s of speed.
    fn bob_amplitude(&self) -> (f32, f32) {
        match self.stance {
            Stance::Prone => BOB_AMPLITUDE_PRONE,
            Stance::Crouch => BOB_AMPLITUDE_DUCKED,
            Stance::Stand if self.sprinting => BOB_AMPLITUDE_SPRINTING,
            Stance::Stand => BOB_AMPLITUDE_STANDING,
        }
    }

    /// `BG_GetBobCycle`: the bob cycle as an angle in radians.
    pub fn bob_angle(&self) -> f32 {
        std::f32::consts::TAU * (self.bob_cycle * 256.0 / 255.0 + 1.0)
    }

    /// Horizontal speed in CoD units/s (`pm->xyspeed`).
    pub fn xy_speed_units(&self) -> f32 {
        self.horizontal_speed() / crate::units::INCH
    }

    /// `BG_GetHorizontalBobFactor` for an amplitude per unit of `speed`.
    pub fn horizontal_bob(&self, cycle: f32, speed: f32, max: f32) -> f32 {
        (speed * self.bob_amplitude().0).min(max) * cycle.sin()
    }

    /// `BG_GetVerticalBobFactor`.
    pub fn vertical_bob(&self, cycle: f32, speed: f32, max: f32) -> f32 {
        let a = (speed * self.bob_amplitude().1).min(max);
        a * 0.75 * ((2.0 * cycle).sin() + 0.2 * (4.0 * cycle + std::f32::consts::FRAC_PI_2).sin())
    }

    /// The eye's (sideways, vertical) bob offset in metres, as
    /// `BG_GetPlayerViewOrigin` applies it.
    pub fn view_bob(&self) -> (f32, f32) {
        let (cycle, speed) = (self.bob_angle(), self.xy_speed_units());
        (u(self.horizontal_bob(cycle, speed, BOB_MAX)), u(self.vertical_bob(cycle, speed, BOB_MAX)))
    }
}

/// `PM_UpdateSprint`.
fn update_sprint(m: &mut Mover, input: &MoveInput, fmove: f32) {
    let left = m.sprint_left_ms();
    let wants = input.sprint && fmove >= SPRINT_FORWARD_MINIMUM && m.stance == Stance::Stand && input.speed_scale > 0.0;
    if !input.sprint {
        m.sprint.button_up_required = false;
    }
    if m.sprinting {
        if !wants || left <= 0.0 {
            m.sprinting = false;
            m.sprint.last_end = m.clock_ms;
            if left <= 0.0 {
                m.sprint.button_up_required = true;
            }
        }
    } else if wants && m.on_ground && !m.sprint.button_up_required && left >= SPRINT_MIN_TIME * 1000.0 {
        m.sprinting = true;
        m.sprint.last_start = m.clock_ms;
        m.sprint.start_max_length = left;
    }
    m.sprint_left = m.sprint_left_ms() / 1000.0;
}

/// Lean toward `input.lean`, stopping short of walls. Prone and sprinting
/// pawns stand straight.
fn update_lean(ctx: &Ctx, pos: Vec3, m: &mut Mover, input: &MoveInput, yaw: f32, dt: f32) {
    let target = if m.stance == Stance::Prone || m.sprinting { 0.0 } else { input.lean.clamp(-1.0, 1.0) };
    let step = dt / LEAN_TIME;
    m.lean += (target - m.lean).clamp(-step, step);
    if m.lean == 0.0 {
        return;
    }
    let (_, right) = yaw_vectors(yaw);
    let Ok(dir) = Dir3::new(right * m.lean.signum()) else { return };
    let config = ShapeCastConfig { max_distance: LEAN_DISTANCE, ..ShapeCastConfig::DEFAULT };
    let head = Collider::sphere(u(6.0));
    if let Some(hit) = ctx.spatial.cast_shape(&head, m.eye(pos), Quat::IDENTITY, dir, &config, ctx.filter) {
        let room = (hit.distance / LEAN_DISTANCE).clamp(0.0, 1.0);
        m.lean = m.lean.clamp(-room, room);
    }
}

/// `PM_CheckDuck` + `PM_ViewHeightAdjust`.
fn check_duck(ctx: &Ctx, pos: Vec3, m: &mut Mover, input: &MoveInput, msec: f32) {
    m.stance_elapsed_ms = (m.stance_elapsed_ms + msec).min(10_000.0);
    // Sprinting stands you up.
    let want = if m.sprinting { Stance::Stand } else { input.stance };
    if want != m.stance {
        let taller = want.hull_height() > m.stance.hull_height();
        if !taller || !ctx.solid_at(pos + Vec3::Y * SKIN, want) {
            m.stance_from = m.stance;
            m.stance = want;
            m.stance_elapsed_ms = 0.0;
        }
    }
    m.update_view_height();
}

/// `PM_GroundTrace`.
fn ground_trace(ctx: &Ctx, pos: Vec3, m: &mut Mover) {
    if m.velocity.y > u(180.0) && !m.on_ground {
        m.on_ground = false;
        m.ground_plane = false;
        return;
    }
    let tr = ctx.trace(pos, pos - Vec3::Y * GROUND_PROBE, m.stance);
    if !tr.hit {
        m.on_ground = false;
        m.ground_plane = false;
        m.ground_normal = Vec3::Y;
        return;
    }
    m.ground_plane = true;
    m.ground_normal = tr.normal;
    // Moving up and away from the surface: still airborne.
    if m.velocity.y > 0.0 && m.velocity.dot(tr.normal) > u(10.0) {
        m.on_ground = false;
        return;
    }
    m.on_ground = tr.normal.y >= MIN_WALK_NORMAL;
}

/// `Jump_Check` / `Jump_Start`.
fn jump_check(m: &mut Mover, input: &MoveInput, pos: Vec3) -> bool {
    if !input.jump || m.jump_held {
        return false;
    }
    if m.clock_ms - m.last_jump_ms < JUMP_INTERVAL_MS || m.stance != Stance::Stand {
        return false;
    }
    let mut v2 = 2.0 * GRAVITY * JUMP_HEIGHT;
    if m.jumping && m.land_ms <= JUMP_LAND_SLOWDOWN_MS {
        v2 /= m.land_factor();
    }
    m.on_ground = false;
    m.ground_plane = false;
    m.last_jump_ms = m.clock_ms;
    m.jump_origin_y = pos.y;
    m.velocity.y = v2.sqrt();
    m.jumping = true;
    m.land_ms = 0.0;
    true
}

/// `Jump_ApplySlowdown`: scale velocity on landing and start the friction
/// penalty window.
fn jump_apply_slowdown(m: &mut Mover, y: f32) {
    let scale = if m.jump_origin_y + u(18.0) <= y {
        m.land_ms = 1200.0;
        0.5
    } else {
        m.land_ms = JUMP_LAND_SLOWDOWN_MS;
        0.65
    };
    m.velocity *= scale;
}

/// `PM_Friction`.
fn friction(m: &mut Mover, t: &MoveTuning, dt: f32) {
    let v = if m.on_ground { Vec3::new(m.velocity.x, 0.0, m.velocity.z) } else { m.velocity };
    let speed = v.length();
    if speed < u(1.0) {
        m.velocity.x = 0.0;
        m.velocity.z = 0.0;
        if m.on_ground {
            m.velocity.y = 0.0;
        }
        return;
    }
    if !m.on_ground {
        return;
    }
    let mut control = speed.max(STOP_SPEED);
    if m.jumping {
        control *= m.land_factor();
    }
    let drop = control * t.friction * dt;
    m.velocity *= (speed - drop).max(0.0) / speed;
}

/// `PM_Accelerate`: CoD4 uses `max(stopspeed, wishspeed)` as the base.
fn accelerate(vel: &mut Vec3, wish_dir: Vec3, wish_speed: f32, accel: f32, dt: f32) {
    let add = wish_speed - vel.dot(wish_dir);
    if add <= 0.0 {
        return;
    }
    let amount = (wish_speed.max(STOP_SPEED) * accel * dt).min(add);
    *vel += wish_dir * amount;
}

fn clip_velocity(v: Vec3, normal: Vec3, overbounce: f32) -> Vec3 {
    let backoff = v.dot(normal);
    let backoff = if backoff < 0.0 { backoff * overbounce } else { backoff / overbounce };
    v - normal * backoff
}

fn yaw_vectors(yaw: f32) -> (Vec3, Vec3) {
    let rot = Quat::from_rotation_y(yaw);
    (rot * Vec3::NEG_Z, rot * Vec3::X)
}

/// `PM_CmdScale_Walk`: the walking speed for this input, before acceleration.
fn walk_scale(m: &Mover, input: &MoveInput, t: &MoveTuning, fmove: f32, smove: f32) -> f32 {
    let f = if fmove < 0.0 { fmove.abs() * BACK_SPEED_SCALE } else { fmove.abs() };
    let s = (smove * STRAFE_SPEED_SCALE).abs();
    let max = f.max(s);
    if max == 0.0 {
        return 0.0;
    }
    let total = (fmove * fmove + smove * smove).sqrt();
    let mut scale = RUN_SPEED * t.speed_scale * max / (127.0 * total);
    if m.sprinting {
        scale *= t.sprint_speed_scale;
    }
    scale * m.stance_speed_scale() * input.speed_scale
}

#[allow(clippy::too_many_arguments)]
fn walk_move(ctx: &Ctx, pos: &mut Vec3, m: &mut Mover, input: &MoveInput, fmove: f32, smove: f32, yaw: f32, dt: f32) {
    friction(m, &ctx.tuning, dt);
    let scale = walk_scale(m, input, &ctx.tuning, fmove, smove);

    // Movement directions follow the ground plane.
    let (forward, right) = yaw_vectors(yaw);
    let forward = clip_velocity(forward, m.ground_normal, OVERCLIP).normalize_or_zero();
    let right = clip_velocity(right, m.ground_normal, OVERCLIP).normalize_or_zero();
    let wish_vel = forward * fmove + right * smove;
    let wish_dir = wish_vel.normalize_or_zero();
    let wish_speed = wish_vel.length() * scale;
    accelerate(&mut m.velocity, wish_dir, wish_speed, ctx.tuning.accelerate, dt);

    // Slide along the ground without losing speed.
    let speed = m.velocity.length();
    m.velocity = clip_velocity(m.velocity, m.ground_normal, OVERCLIP).normalize_or_zero() * speed;
    if m.velocity.x == 0.0 && m.velocity.z == 0.0 {
        return;
    }
    step_slide_move(ctx, pos, m, false, dt);
}

fn air_move(ctx: &Ctx, pos: &mut Vec3, m: &mut Mover, fmove: f32, smove: f32, yaw: f32, dt: f32) {
    friction(m, &ctx.tuning, dt);
    let max = fmove.abs().max(smove.abs());
    let total = (fmove * fmove + smove * smove).sqrt();
    let scale = if max > 0.0 { RUN_SPEED * ctx.tuning.speed_scale * max / (127.0 * total) } else { 0.0 };
    let (forward, right) = yaw_vectors(yaw);
    let wish_vel = forward * fmove + right * smove;
    let wish_dir = Vec3::new(wish_vel.x, 0.0, wish_vel.z).normalize_or_zero();
    accelerate(&mut m.velocity, wish_dir, wish_vel.length() * scale, AIR_ACCELERATE, dt);
    // Sliding down a steep slope.
    if m.ground_plane {
        let backoff = m.velocity.dot(m.ground_normal);
        let backoff = backoff - backoff.abs() * 0.001;
        m.velocity -= m.ground_normal * backoff;
    }
    step_slide_move(ctx, pos, m, true, dt);
}

/// `Jump_ClampVelocity`: never rise above the jump apex.
fn clamp_jump_velocity(m: &mut Mover, y: f32) {
    if !m.jumping || m.velocity.y <= 0.0 {
        return;
    }
    let room = m.jump_origin_y + JUMP_HEIGHT - y;
    if room >= u(0.1) {
        m.velocity.y = m.velocity.y.min((2.0 * GRAVITY * room).sqrt());
    } else {
        m.velocity.y = 0.0;
    }
}

/// `PM_SlideMove`: returns true if anything was hit.
fn slide_move(ctx: &Ctx, pos: &mut Vec3, m: &mut Mover, gravity: bool, dt: f32) -> bool {
    let mut end_velocity = m.velocity;
    if gravity {
        end_velocity.y -= GRAVITY * dt;
        m.velocity.y = (m.velocity.y + end_velocity.y) * 0.5;
        if m.ground_plane {
            m.velocity = clip_velocity(m.velocity, m.ground_normal, OVERCLIP);
        }
    }
    clamp_jump_velocity(m, pos.y);

    let mut time_left = dt;
    let mut planes: Vec<Vec3> = Vec::with_capacity(5);
    if m.ground_plane {
        planes.push(m.ground_normal);
    }
    planes.push(m.velocity.normalize_or_zero());

    let mut bumps = 0;
    'bump: while bumps < 4 {
        let end = *pos + m.velocity * time_left;
        let tr = ctx.trace(*pos, end, m.stance);
        if tr.fraction > 0.0 {
            *pos = tr.end;
        }
        if !tr.hit {
            break;
        }
        bumps += 1;
        time_left -= time_left * tr.fraction;
        if planes.len() >= 5 {
            m.velocity = Vec3::ZERO;
            return true;
        }
        // Same plane as before: nudge out along it.
        for p in &planes {
            if tr.normal.dot(*p) > 0.99 {
                m.velocity += tr.normal * u(1.0);
                continue 'bump;
            }
        }
        planes.push(tr.normal);

        // Find a velocity parallel to every plane we touch.
        let n = planes.len();
        for i in 0..n {
            if m.velocity.dot(planes[i]) >= 0.1 {
                continue;
            }
            let mut clip = clip_velocity(m.velocity, planes[i], OVERCLIP);
            let mut end_clip = clip_velocity(end_velocity, planes[i], OVERCLIP);
            for j in 0..n {
                if j == i || clip.dot(planes[j]) >= 0.1 {
                    continue;
                }
                clip = clip_velocity(clip, planes[j], OVERCLIP);
                end_clip = clip_velocity(end_clip, planes[j], OVERCLIP);
                if clip.dot(planes[i]) >= 0.0 {
                    continue;
                }
                // Slide along the crease.
                let dir = planes[i].cross(planes[j]).normalize_or_zero();
                clip = dir * dir.dot(m.velocity);
                end_clip = dir * dir.dot(end_velocity);
                for k in 0..n {
                    if k != i && k != j && clip.dot(planes[k]) < 0.1 {
                        m.velocity = Vec3::ZERO;
                        return true;
                    }
                }
            }
            m.velocity = clip;
            end_velocity = end_clip;
            break;
        }
    }
    if gravity {
        m.velocity = end_velocity;
        clamp_jump_velocity(m, pos.y);
    }
    bumps > 0
}

/// `PM_StepSlideMove`: slide, and if blocked try again from a step higher.
///
/// As CoD4 does it: on the ground, a blocked move (or one on a steep floor)
/// is retried from up to a step higher, then put back down (up to 9 units
/// further, which keeps players on stairs and slopes going down). In the
/// air only a jump steps, by `jump_stepSize` up to the top of the jump. A
/// step that lands on anything but a near-wall counts (the top of a door
/// lip, met by the rounded hull, can look like a slope), and taking one
/// costs speed: the more the height changed, the more.
fn step_slide_move(ctx: &Ctx, pos: &mut Vec3, m: &mut Mover, gravity: bool, dt: f32) {
    let start_pos = *pos;
    let start_vel = m.velocity;
    let had_ground = m.on_ground;
    let steep_ground = had_ground && m.ground_normal.y < STEP_ON_SLOPE_NORMAL;

    let bumped = slide_move(ctx, pos, m, gravity, dt);

    let mut step = if m.stance == Stance::Prone { PRONE_STEP_HEIGHT } else { STEP_HEIGHT };
    let mut jumping = false;
    if !had_ground {
        // Jump_GetStepHeight: in the air, only a jump that hit something
        // steps, no higher than the top of the jump.
        let apex = m.jump_origin_y + JUMP_HEIGHT;
        if !(bumped && m.jumping) || start_pos.y >= apex {
            return;
        }
        step = JUMP_STEP_SIZE.min(apex - start_pos.y);
        if step < u(1.0) {
            return;
        }
        jumping = true;
    }

    let (down_pos, down_vel) = (*pos, m.velocity);
    let mut amount = 0.0;
    if bumped || steep_ground {
        let up = ctx.trace(start_pos, start_pos + Vec3::Y * (step + u(1.0)), m.stance);
        amount = (step + u(1.0)) * up.fraction - u(1.0);
        if amount >= u(1.0) {
            *pos = start_pos + Vec3::Y * amount;
            m.velocity = start_vel;
            slide_move(ctx, pos, m, gravity, dt);
        } else {
            amount = 0.0;
        }
    }
    if had_ground || amount != 0.0 {
        let mut down = *pos - Vec3::Y * amount;
        if had_ground {
            down.y -= STICK_DOWN;
        }
        let tr = ctx.trace(*pos, down, m.stance);
        if !tr.hit {
            pos.y -= amount;
        } else if tr.normal.y < STEP_LAND_NORMAL {
            // Onto something like a wall: no step.
            *pos = down_pos;
            m.velocity = down_vel;
            return;
        } else {
            *pos = tr.end;
            m.velocity = project_velocity(m.velocity, tr.normal);
        }
    }

    // The step must get further along than plain sliding did (and a jump
    // can't step above its top).
    let along = |p: Vec3| (p.x - start_pos.x) * start_vel.x + (p.z - start_pos.z) * start_vel.z;
    if along(*pos) <= along(down_pos) + 1e-4 || jumping && pos.y >= m.jump_origin_y + JUMP_HEIGHT {
        *pos = down_pos;
        m.velocity = down_vel;
        if had_ground {
            let tr = ctx.trace(*pos, *pos - Vec3::Y * STICK_DOWN, m.stance);
            if tr.hit {
                *pos = tr.end;
                m.velocity = clip_velocity(m.velocity, tr.normal, OVERCLIP);
            }
        }
    }
    if jumping && pos.y > down_pos.y {
        clamp_jump_velocity(m, pos.y);
    }
    // Stepping up or down costs speed (not when the step was undone).
    if had_ground && m.stance != Stance::Prone && (pos.y - down_pos.y).abs() > u(0.5) {
        let scale = 1.0 - STEP_SLOWDOWN + (1.0 - (pos.y - start_pos.y).abs() / step).max(0.0) * STEP_SLOWDOWN;
        m.velocity *= scale;
    }
}

/// `PM_ProjectVelocity`: along a slope, keeping the speed (or the part
/// that fits).
fn project_velocity(v: Vec3, normal: Vec3) -> Vec3 {
    let flat_sq = v.x * v.x + v.z * v.z;
    if normal.y.abs() < 1e-4 || flat_sq == 0.0 {
        return v;
    }
    let new_y = -(normal.x * v.x + normal.z * v.z) / normal.y;
    let adjusted = Vec3::new(v.x, new_y, v.z);
    let scale = ((v.y * v.y + flat_sq) / (new_y * new_y + flat_sq)).sqrt();
    if scale < 1.0 || new_y < 0.0 || v.y > 0.0 { adjusted * scale } else { adjusted }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_curves_hit_endpoints() {
        assert_eq!(sample_curve(VIEW_STAND_CROUCH, 0.0), 60.0);
        assert_eq!(sample_curve(VIEW_STAND_CROUCH, 100.0), 40.0);
        assert_eq!(sample_curve(VIEW_CROUCH_PRONE, 100.0), 11.0);
        assert!((sample_curve(VIEW_STAND_CROUCH, 55.0) - 50.0).abs() < 0.01);
    }

    #[test]
    fn diagonal_is_not_faster() {
        let m = Mover::default();
        let input = MoveInput::default();
        let t = MoveTuning::COD4;
        let straight = walk_scale(&m, &input, &t, 127.0, 0.0) * 127.0;
        let diag = walk_scale(&m, &input, &t, 127.0, 127.0) * (2.0f32 * 127.0 * 127.0).sqrt();
        assert!((straight - RUN_SPEED).abs() < 1e-4);
        // Strafing is scaled by 0.8 but the forward component dominates.
        assert!((diag - RUN_SPEED).abs() < 1e-4, "{diag} vs {}", RUN_SPEED);
    }

    #[test]
    fn backpedal_is_slower() {
        let m = Mover::default();
        let input = MoveInput::default();
        let back = walk_scale(&m, &input, &MoveTuning::COD4, -127.0, 0.0) * 127.0;
        assert!((back - RUN_SPEED * BACK_SPEED_SCALE).abs() < 1e-4);
    }

    #[test]
    fn sprint_meter_drains_and_recharges() {
        let mut m = Mover { on_ground: true, ..Default::default() };
        let input = MoveInput { forward: 1.0, sprint: true, ..Default::default() };
        update_sprint(&mut m, &input, 127.0);
        assert!(m.sprinting);
        m.clock_ms += 4000.0;
        update_sprint(&mut m, &input, 127.0);
        assert!(!m.sprinting, "stamina ran out");
        assert!(m.sprint.button_up_required);
        m.clock_ms += 2000.0;
        assert!((m.sprint_left_ms() - 2000.0).abs() < 1.0);
    }

    #[test]
    fn view_bob_matches_cod4_rates() {
        // Full-speed run: 0.335/256 of a cycle per ms, +-1.33" sideways.
        let input = MoveInput { forward: 1.0, ..Default::default() };
        let mut m = Mover { on_ground: true, velocity: Vec3::new(0.0, 0.0, -RUN_SPEED), ..Default::default() };
        for _ in 0..1000 {
            update_bob_cycle(&mut m, &input, &MoveTuning::COD4, 127.0, 0.0, 1.0);
        }
        assert!((m.bob_cycle - (335.0 / 256.0f32).fract()).abs() < 1e-3, "cycle {}", m.bob_cycle);
        let peak = (0..256)
            .map(|i| Mover { bob_cycle: i as f32 / 256.0, ..m }.view_bob().0.abs())
            .fold(0.0f32, f32::max);
        assert!((peak / crate::units::INCH - 1.33).abs() < 0.01, "peak {}", peak / crate::units::INCH);

        // Sprinting cycles at player_sprintCameraBob (0.5).
        let mut m = Mover { on_ground: true, sprinting: true, velocity: Vec3::new(0.0, 0.0, -RUN_SPEED * 1.5), ..m };
        m.bob_cycle = 0.0;
        update_bob_cycle(&mut m, &MoveInput { sprint: true, ..input }, &MoveTuning::COD4, 127.0, 0.0, 100.0);
        assert!((m.bob_cycle - 50.0 / 256.0).abs() < 1e-4);

        // No input, no bob progress.
        let before = m.bob_cycle;
        update_bob_cycle(&mut m, &input, &MoveTuning::COD4, 0.0, 0.0, 100.0);
        assert_eq!(m.bob_cycle, before);
    }
}

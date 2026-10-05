//! Human-like aiming.
//!
//! Bots turn their view the way a hand moves a mouse, not by steering
//! straight at the target:
//! - aim moves in discrete *submovements*, each a bell-shaped (minimum-jerk)
//!   flick lasting `a + b * log2(D / W + 1)` seconds (Fitts' law: further and
//!   smaller targets take longer);
//! - each flick lands with a random gain (humans tend to undershoot slightly)
//!   and a sideways error, so big flicks are followed by smaller corrections
//!   after a short pause;
//! - a moving target is followed with smooth pursuit driven by a lagged
//!   estimate of its angular velocity, so strafing really does throw aim off;
//! - recoil is pulled back down only after it is felt, with a delay;
//! - the hand trembles a little all the time.

use crate::movement::ViewAngles;
use bevy::prelude::*;
use rand::Rng;
use std::collections::VecDeque;
use std::f32::consts::PI;

/// How one bot's hand and eyes behave. See [`AimProfile::for_skill`].
#[derive(Clone, Debug)]
pub struct AimProfile {
    /// Fitts' law intercept and slope (seconds, seconds per bit).
    pub fitts_a: f32,
    pub fitts_b: f32,
    /// Mean amplitude of a submovement relative to the error it corrects.
    pub gain_bias: f32,
    /// Spread of that amplitude.
    pub gain_sd: f32,
    /// Sideways error, as a fraction of the amplitude.
    pub dir_sd: f32,
    /// Pause after a submovement before the next can start (seconds).
    pub correction_delay: f32,
    /// Fraction of a target's angular velocity matched while tracking.
    pub pursuit_gain: f32,
    /// Time constant of the target-motion estimate (seconds).
    pub motion_lag: f32,
    /// Hand tremor (radians per sqrt second).
    pub tremor: f32,
    /// Fraction of felt recoil pulled back down, and the delay before it.
    pub recoil_comp: f32,
    pub recoil_delay: f32,
}

impl AimProfile {
    /// Parameters for `skill` in 0..1, varied a little by `rng` so no two
    /// bots aim exactly alike.
    pub fn for_skill(skill: f32, rng: &mut impl Rng) -> Self {
        let s = skill.clamp(0.0, 1.0);
        let vary = |rng: &mut dyn rand::RngCore, v: f32| v * rng.random_range(0.85..1.15);
        let lerp = |a: f32, b: f32| a + (b - a) * s;
        AimProfile {
            fitts_a: vary(rng, lerp(0.09, 0.05)),
            fitts_b: vary(rng, lerp(0.11, 0.06)),
            gain_bias: lerp(0.9, 0.97),
            gain_sd: vary(rng, lerp(0.14, 0.05)),
            dir_sd: vary(rng, lerp(0.09, 0.03)),
            correction_delay: vary(rng, lerp(0.16, 0.08)),
            pursuit_gain: lerp(0.65, 0.92),
            motion_lag: vary(rng, lerp(0.25, 0.1)),
            tremor: lerp(0.004, 0.0015),
            recoil_comp: lerp(0.35, 0.85),
            recoil_delay: vary(rng, lerp(0.2, 0.1)),
        }
    }
}

/// Largest view change (radians) followed smoothly rather than with a flick
/// when not aiming at someone.
const STEER_TURN: f32 = 1.0;
/// How quickly that smooth following closes the gap (per second).
const STEER_RATE: f32 = 7.0;
/// Within this (radians) the view isn't adjusted at all.
const STEER_DEADBAND: f32 = 0.05;

/// Where the bot is trying to point.
#[derive(Clone, Copy, Debug)]
pub struct AimGoal {
    /// Desired (yaw, pitch) in radians, as `ViewAngles`.
    pub angles: Vec2,
    /// Angular width of what is being aimed at (radians); sets precision.
    pub width: f32,
    /// Engaging a target (fast, precise) rather than just looking around.
    pub combat: bool,
}

struct Submove {
    start: f32,
    duration: f32,
    delta: Vec2,
    applied: f32,
}

#[derive(Default)]
pub struct AimState {
    submove: Option<Submove>,
    next_move_at: f32,
    /// Smoothed estimate of the goal's angular velocity.
    goal_vel: Vec2,
    last_goal: Option<Vec2>,
    /// The view as this model left it last frame, to tell recoil apart from
    /// our own movement.
    last_out: Option<Vec2>,
    /// Felt recoil waiting to be compensated: (time felt, displacement).
    felt_recoil: VecDeque<(f32, Vec2)>,
    tremor: Vec2,
    /// Submovements made towards the current goal (for debugging/tests).
    pub submoves: u32,
}

fn wrap(a: f32) -> f32 {
    (a + PI).rem_euclid(2.0 * PI) - PI
}

/// A standard normal sample (Box-Muller).
pub fn gaussian(rng: &mut impl Rng) -> f32 {
    let u1: f32 = rng.random::<f32>().max(1e-7);
    let u2: f32 = rng.random();
    (-2.0 * u1.ln()).sqrt() * (2.0 * PI * u2).cos()
}

/// Minimum-jerk position profile, 0..1 over normalised time.
fn min_jerk(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * t * (10.0 - 15.0 * t + 6.0 * t * t)
}

impl AimState {
    /// Angular error from the view to `goal` (yaw, pitch).
    pub fn error(view: &ViewAngles, goal: Vec2) -> Vec2 {
        Vec2::new(wrap(goal.x - view.yaw), goal.y - view.pitch)
    }

    /// Advance the hand by `dt` towards `goal` (or just hold still).
    pub fn update(&mut self, p: &AimProfile, view: &mut ViewAngles, goal: Option<AimGoal>, now: f32, dt: f32, rng: &mut impl Rng) {
        let current = Vec2::new(view.yaw, view.pitch);
        // Anything that moved the view since last frame was recoil (or a
        // respawn): feel it, and plan to pull it back.
        if let Some(last) = self.last_out {
            let kicked = Vec2::new(wrap(current.x - last.x), current.y - last.y);
            if kicked.length() > 1e-5 && kicked.length() < 0.2 {
                self.felt_recoil.push_back((now, kicked));
            }
        }
        let mut out = current;

        while let Some(&(t, kick)) = self.felt_recoil.front() {
            if now - t < p.recoil_delay {
                break;
            }
            self.felt_recoil.pop_front();
            out -= kick * p.recoil_comp;
        }

        // Carry on with the current flick.
        if let Some(m) = &mut self.submove {
            let tau = (now - m.start) / m.duration;
            let s = min_jerk(tau);
            out += m.delta * (s - m.applied);
            m.applied = s;
            if tau >= 1.0 {
                self.submove = None;
                self.next_move_at = now + p.correction_delay * rng.random_range(0.7..1.3);
            }
        }

        if let Some(goal) = goal {
            // Lagged estimate of how fast the goal moves across the view.
            if let Some(last) = self.last_goal {
                let raw = Vec2::new(wrap(goal.angles.x - last.x), goal.angles.y - last.y) / dt.max(1e-4);
                let k = (dt / p.motion_lag.max(1e-3)).min(1.0);
                self.goal_vel += (raw.clamp_length_max(6.0) - self.goal_vel) * k;
            }
            self.last_goal = Some(goal.angles);

            let err = Vec2::new(wrap(goal.angles.x - out.x), goal.angles.y - out.y);
            let tolerance = if goal.combat { (goal.width * 0.2).max(0.002) } else { 0.03 };

            // Smooth pursuit while roughly on target.
            if goal.combat && err.length() < goal.width * 3.0 + 0.03 {
                out += self.goal_vel * p.pursuit_gain * dt;
            }

            // Looking around rather than aiming: within a turn of about 60
            // degrees the view follows smoothly, as a player steers with the
            // mouse while walking. Bigger turns are a flick.
            let steering = !goal.combat && err.length() < STEER_TURN && self.submove.is_none();
            // Close enough is left alone: nobody nudges the mouse for a few
            // degrees while walking.
            if steering && err.length() > STEER_DEADBAND {
                out += self.goal_vel * dt * 0.8 + err * (1.0 - (-STEER_RATE * dt).exp());
            }

            if !steering && self.submove.is_none() && now >= self.next_move_at && err.length() > tolerance {
                let dist = err.length();
                let width = goal.width.max(0.002);
                let (a, b) = if goal.combat { (p.fitts_a, p.fitts_b) } else { (p.fitts_a * 2.0, p.fitts_b * 2.5) };
                let duration = (a + b * (dist / width + 1.0).log2()).clamp(0.05, 0.9);
                let (normal, side) = (gaussian(rng), gaussian(rng));
                let gain = p.gain_bias + p.gain_sd * normal;
                let dir = err / dist;
                let perp = Vec2::new(-dir.y, dir.x);
                // Lead a moving target by part of how far it travels meanwhile.
                let lead = if goal.combat { self.goal_vel * duration * 0.5 } else { Vec2::ZERO };
                let delta = dir * dist * gain + perp * dist * p.dir_sd * side + lead;
                self.submove = Some(Submove { start: now, duration, delta, applied: 0.0 });
                self.submoves += 1;
            }
        } else {
            self.last_goal = None;
            self.goal_vel = Vec2::ZERO;
        }

        // Tremor: a slowly wandering offset while aiming at someone (a hand
        // off the mouse doesn't shake the view).
        let aiming = goal.is_some_and(|g| g.combat);
        let n = Vec2::new(gaussian(rng), gaussian(rng));
        let step = if aiming { n * p.tremor * dt.sqrt() } else { Vec2::ZERO } - self.tremor * (dt * 4.0).min(1.0);
        self.tremor += step;
        out += step;

        view.yaw = out.x;
        view.pitch = out.y.clamp(-1.45, 1.45);
        self.last_out = Some(Vec2::new(view.yaw, view.pitch));
    }

    /// The view was turned by something else on purpose (motion matching):
    /// don't take it for recoil.
    pub fn sync(&mut self, view: &ViewAngles) {
        self.last_out = Some(Vec2::new(view.yaw, view.pitch));
    }

    /// Forget the last frame's view (after a respawn or teleport).
    pub fn reset(&mut self) {
        *self = AimState::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    fn run(skill: f32, target_deg: f32, width_deg: f32, seed: u64) -> (f32, u32, f32) {
        let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
        let p = AimProfile::for_skill(skill, &mut rng);
        let mut s = AimState::default();
        let mut view = ViewAngles { yaw: 0.0, pitch: 0.0 };
        let goal = AimGoal { angles: Vec2::new(target_deg.to_radians(), 0.0), width: width_deg.to_radians(), combat: true };
        let dt = 1.0 / 120.0;
        let mut acquired = None;
        let mut peak_overshoot: f32 = 0.0;
        for f in 0..240 {
            let now = f as f32 * dt;
            s.update(&p, &mut view, Some(goal), now, dt, &mut rng);
            peak_overshoot = peak_overshoot.max(view.yaw.to_degrees() - target_deg);
            let err = AimState::error(&view, goal.angles).length().to_degrees();
            if acquired.is_none() && err < width_deg * 0.5 {
                acquired = Some(now);
            }
        }
        (acquired.unwrap_or(f32::INFINITY), s.submoves, peak_overshoot)
    }

    #[test]
    fn flicks_take_human_time_and_need_corrections() {
        // A 40 degree flick onto a 2 degree target.
        let (mut times, mut moves) = (Vec::new(), Vec::new());
        for seed in 0..40 {
            let (t, n, _) = run(0.6, 40.0, 2.0, seed);
            times.push(t);
            moves.push(n);
        }
        let mean = times.iter().sum::<f32>() / times.len() as f32;
        assert!((0.25..0.9).contains(&mean), "mean acquisition {mean}s");
        assert!(times.iter().all(|t| t.is_finite()), "always gets there: {times:?}");
        assert!(moves.iter().any(|&n| n >= 2), "sometimes needs corrective flicks: {moves:?}");
    }

    #[test]
    fn skill_speeds_up_acquisition() {
        let avg = |skill| (0..40).map(|s| run(skill, 30.0, 1.5, s).0).sum::<f32>() / 40.0;
        let (low, high) = (avg(0.1), avg(0.95));
        assert!(high < low, "skilled {high}s vs unskilled {low}s");
    }

    #[test]
    fn tracking_lags_a_strafing_target() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(7);
        let p = AimProfile::for_skill(0.7, &mut rng);
        let mut s = AimState::default();
        let mut view = ViewAngles { yaw: 0.0, pitch: 0.0 };
        let dt = 1.0 / 120.0;
        // Target sweeps sideways at 20 deg/s, then reverses at t = 1.5 s.
        let mut errs_after_reverse = Vec::new();
        for f in 0..300 {
            let t = f as f32 * dt;
            let yaw = if t < 1.5 { t * 20.0 } else { 30.0 - (t - 1.5) * 20.0 };
            let goal = AimGoal { angles: Vec2::new(yaw.to_radians(), 0.0), width: 2f32.to_radians(), combat: true };
            s.update(&p, &mut view, Some(goal), t, dt, &mut rng);
            if (1.55..1.75).contains(&t) {
                errs_after_reverse.push(AimState::error(&view, goal.angles).length().to_degrees());
            }
        }
        let worst = errs_after_reverse.iter().cloned().fold(0.0, f32::max);
        assert!(worst > 0.5, "a direction change should open up an aim error, got {worst} deg");
    }
}

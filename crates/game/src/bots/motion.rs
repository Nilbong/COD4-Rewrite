//! Motion matching: bots walk and look around the way real players did.
//!
//! At startup every CoD4 demo in the install's `main/demos` folder is cut
//! (see [`super::demos`]) into short clips of players moving outside fights: how they turned the
//! view, which way they moved relative to it, crouched or sprinted. While a
//! bot walks a route, a few times a second it finds the clip whose situation
//! best matches its own (where it needs to be in a second, how it's moving
//! and turning now) and plays that clip's view turning and movement. The
//! route still decides where to go; real players decide how.

use bevy::prelude::*;
use std::sync::Arc;

/// Samples per second in the library (demo snapshots come at 20 Hz).
pub const RATE: f32 = 20.0;
/// How far ahead a clip's goal looks, in samples (1 s).
const GOAL_STEPS: usize = 20;
/// Clip length in samples (0.6 s).
pub const CLIP_STEPS: usize = 12;

/// One step of a clip.
#[derive(Clone, Copy, Debug, Default)]
pub struct Step {
    /// View turn since the previous step (radians, Bevy's sense: + is left).
    pub turn: f32,
    /// View pitch (radians, + is up).
    pub pitch: f32,
    /// Velocity relative to the view: forward and right (CoD units/s).
    pub forward: f32,
    pub right: f32,
    pub crouch: bool,
    pub prone: bool,
}

/// What a clip is matched on (see [`features`]).
pub type Features = [f32; 6];

pub struct Library {
    features: Vec<Features>,
    clips: Vec<[Step; CLIP_STEPS]>,
    /// Which track each sample comes from (for carrying on along it).
    track: Vec<u32>,
    pub players: usize,
    pub seconds: f32,
}

/// Weights per feature: goal forward/right, velocity forward/right, turn
/// rate, pitch.
const WEIGHTS: Features = [1.0, 1.0, 0.6, 0.6, 0.8, 0.3];

/// The situation a clip (or a bot) is in: where it needs to be in a second
/// and how it's moving now, all relative to its view.
pub fn features(goal_fwd: f32, goal_right: f32, vel_fwd: f32, vel_right: f32, turn_rate: f32, pitch: f32) -> Features {
    [goal_fwd / 200.0, goal_right / 200.0, vel_fwd / 200.0, vel_right / 200.0, turn_rate / 3.0, pitch / 0.5]
}

/// Carrying on with the clip being played costs this much less than
/// switching, so bots don't hop between players' motions.
const CONTINUE_BIAS: f32 = 0.35;

impl Library {
    /// The best sample for `f`, preferring to carry on from sample `from`
    /// `steps` samples later when that still fits.
    pub fn best(&self, f: &Features, from: Option<(usize, usize)>) -> Option<usize> {
        let dist = |g: &Features| (0..6).map(|k| WEIGHTS[k] * (f[k] - g[k]).powi(2)).sum::<f32>();
        let (i, d) = self
            .features
            .iter()
            .enumerate()
            .map(|(i, g)| (i, dist(g)))
            .min_by(|a, b| a.1.total_cmp(&b.1))?;
        let carry = from
            .map(|(at, steps)| at + steps)
            .filter(|&j| j < self.len() && self.track[j] == self.track[from.unwrap().0])
            .map(|j| (j, dist(&self.features[j]) * CONTINUE_BIAS));
        Some(match carry {
            Some((j, dj)) if dj <= d => j,
            _ => i,
        })
    }

    pub fn clip(&self, i: usize) -> &[Step; CLIP_STEPS] {
        &self.clips[i]
    }

    pub fn len(&self) -> usize {
        self.features.len()
    }
}

/// eFlags bits on player entities.
const EF_CROUCHING: u32 = 0x4;
const EF_PRONE: u32 = 0x8;
const EF_FIRING: u32 = 0x40;
const EF_ADS: u32 = 0x40000;

/// One player sample from a demo, CoD space.
#[derive(Clone, Copy)]
pub(super) struct Sample {
    /// Seconds from the start of the demo.
    pub time: f32,
    pub pos: Vec3,
    /// Degrees, CoD's sense (yaw + left, pitch + down).
    pub yaw: f32,
    pub pitch: f32,
    pub flags: u32,
}

/// Server bots are named `[BOT]...` by CoD4X or `bot0`, `bot1`... by the
/// stock game: their play isn't worth learning from.
fn is_bot(name: &str) -> bool {
    name.starts_with("[BOT]")
        || name.strip_prefix("bot").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// Every real player's track in a demo by name: runs of 20 Hz samples,
/// each run starting at a respawn or where the player came back into view.
pub(super) fn tracks(demo: &iw3::demo::Demo) -> Vec<(String, Vec<Sample>)> {
    let mut by_player: std::collections::BTreeMap<(u32, &str), Vec<Sample>> = Default::default();
    // Server times are milliseconds since the server started, days' worth
    // on a long-running server: too big for an f32 to keep 50 ms steps.
    let t0 = demo.snapshots.first().map_or(0, |s| s.server_time);
    for s in &demo.snapshots {
        for e in s.entities.iter().filter(|e| e.e_type() == 1) {
            // Client numbers are reused when players leave.
            let name = demo.name_at(e.number, s.server_time).unwrap_or("");
            if is_bot(name) {
                continue;
            }
            let a = e.angles();
            by_player.entry((e.number, name)).or_default().push(Sample {
                time: (s.server_time - t0) as f32 / 1000.0,
                pos: Vec3::from(e.origin()),
                yaw: a[1],
                pitch: a[0],
                flags: e.int("lerp.eFlags"),
            });
        }
    }
    // Split where the player wasn't sent for a moment or teleported (respawn).
    let mut out = Vec::new();
    for ((_, name), track) in by_player {
        let mut run: Vec<Sample> = Vec::new();
        for s in track {
            if let Some(last) = run.last() {
                if s.time - last.time > 0.075 || s.pos.distance(last.pos) > 60.0 {
                    out.push((name.to_string(), std::mem::take(&mut run)));
                }
            }
            run.push(s);
        }
        out.push((name.to_string(), run));
    }
    out.retain(|r| r.1.len() > GOAL_STEPS + CLIP_STEPS + 2);
    out
}

fn wrap_deg(a: f32) -> f32 {
    (a + 180.0).rem_euclid(360.0) - 180.0
}

/// Velocity of `track[i]` (from the next sample) relative to its view.
fn relative_velocity(track: &[Sample], i: usize) -> (f32, f32) {
    let (a, b) = (track[i], track[(i + 1).min(track.len() - 1)]);
    let v = (b.pos - a.pos) * RATE;
    let yaw = a.yaw.to_radians();
    let (fwd, right) = (Vec2::new(yaw.cos(), yaw.sin()), Vec2::new(yaw.sin(), -yaw.cos()));
    (v.truncate().dot(fwd), v.truncate().dot(right))
}

pub(super) fn build(demos: &[iw3::demo::Demo]) -> Library {
    let mut lib = Library { features: Vec::new(), clips: Vec::new(), track: Vec::new(), players: 0, seconds: 0.0 };
    let mut track_id = 0u32;
    for demo in demos {
        for (_, track) in tracks(demo) {
            lib.players += 1;
            track_id += 1;
            lib.seconds += track.len() as f32 / RATE;
            for i in 1..track.len() - GOAL_STEPS.max(CLIP_STEPS) - 1 {
                // Outside fights only: not firing or aiming down sights
                // around this moment.
                let window = &track[i.saturating_sub(10)..i + CLIP_STEPS];
                if window.iter().any(|s| s.flags & (EF_FIRING | EF_ADS) != 0) {
                    continue;
                }
                let now = track[i];
                let yaw = now.yaw.to_radians();
                let (fwd, right) = (Vec2::new(yaw.cos(), yaw.sin()), Vec2::new(yaw.sin(), -yaw.cos()));
                let goal = (track[i + GOAL_STEPS].pos - now.pos).truncate();
                let (vf, vr) = relative_velocity(&track, i);
                let turn_rate = wrap_deg(now.yaw - track[i - 1].yaw).to_radians() * RATE;
                let pitch = -now.pitch.to_radians();
                lib.features.push(features(goal.dot(fwd), goal.dot(right), vf, vr, turn_rate, pitch));
                lib.track.push(track_id);
                lib.clips.push(std::array::from_fn(|k| {
                    let (s, prev) = (track[i + 1 + k], track[i + k]);
                    let (f, r) = relative_velocity(&track, i + 1 + k);
                    Step {
                        turn: wrap_deg(s.yaw - prev.yaw).to_radians(),
                        pitch: -s.pitch.to_radians(),
                        forward: f,
                        right: r,
                        crouch: s.flags & EF_CROUCHING != 0,
                        prone: s.flags & EF_PRONE != 0,
                    }
                }));
            }
        }
    }
    lib
}

/// The library, once built (see [`super::demos`]).
#[derive(Resource, Clone)]
pub struct MotionLibrary(pub Arc<Library>);

/// A clip a bot is playing.
#[derive(Clone)]
pub struct Playing {
    /// Its library sample.
    pub sample: usize,
    pub clip: [Step; CLIP_STEPS],
    pub started: f32,
}

impl Playing {
    /// The step for time `now`, if the clip hasn't ended.
    pub fn step(&self, now: f32) -> Option<Step> {
        let k = ((now - self.started) * RATE) as usize;
        self.clip.get(k).copied()
    }
}

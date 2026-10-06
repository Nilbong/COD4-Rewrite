//! Learned models of how real players play, trained on recordings of them
//! (`tools/learn`): small networks run on the CPU each frame.
//!
//! The aim model ([`AimNet`]): from what a player sees of a target (where
//! it is against the crosshair, how it moves, how long since it was
//! spotted, how far) to how their view turns next, as a mean and a spread
//! (so no two flicks are alike). `tools/learn/train_aim.py` writes it as
//! JSON; `COD4RW_NETAIM=<file.json>` loads it, and bots then aim with it
//! while engaging (looking around stays the hand model's, [`super::aim`]).
//! Off by default: it needs far more recorded play than there is yet to beat
//! the hand model.

use super::aim::AimGoal;
use crate::movement::ViewAngles;
use bevy::prelude::*;
use rand::Rng;
use std::sync::{Arc, OnceLock};

/// A dense layer: `out = w · in + b`.
#[derive(Clone, Debug)]
struct Layer {
    w: Vec<Vec<f32>>,
    b: Vec<f32>,
}

/// The aim network and its input and output scaling.
#[derive(Clone, Debug)]
pub struct AimNet {
    x_mean: Vec<f32>,
    x_std: Vec<f32>,
    y_mean: [f32; 2],
    y_std: [f32; 2],
    layers: Vec<Layer>,
    /// The step it was trained at (seconds) and how long a noise draw holds.
    dt: f32,
    noise_hold: f32,
}

/// Inputs, as `tools/learn/aim_data.py` FEATURES lists them.
const N_IN: usize = 10;

impl AimNet {
    pub fn from_json(text: &str) -> Option<AimNet> {
        let v: serde_json::Value = serde_json::from_str(text).ok()?;
        let floats = |v: &serde_json::Value| -> Option<Vec<f32>> { v.as_array()?.iter().map(|x| x.as_f64().map(|f| f as f32)).collect() };
        let layers = v["layers"]
            .as_array()?
            .iter()
            .map(|l| Some(Layer { w: l["w"].as_array()?.iter().map(floats).collect::<Option<_>>()?, b: floats(&l["b"])? }))
            .collect::<Option<Vec<_>>>()?;
        let ym = floats(&v["y_mean"])?;
        let ys = floats(&v["y_std"])?;
        let net = AimNet {
            x_mean: floats(&v["x_mean"])?,
            x_std: floats(&v["x_std"])?,
            y_mean: [*ym.first()?, *ym.get(1)?],
            y_std: [*ys.first()?, *ys.get(1)?],
            layers,
            dt: v["dt"].as_f64()? as f32,
            noise_hold: v["noise_hold"].as_f64().unwrap_or(0.05) as f32,
        };
        (net.x_mean.len() == N_IN && net.layers.last()?.b.len() == 4).then_some(net)
    }

    /// Mean and standard deviation of the view's angular velocity (deg/s,
    /// yaw then pitch) for these inputs.
    fn predict(&self, x: [f32; N_IN]) -> ([f32; 2], [f32; 2]) {
        let mut h: Vec<f32> = x.iter().zip(&self.x_mean).zip(&self.x_std).map(|((v, m), s)| (v - m) / s).collect();
        for (i, l) in self.layers.iter().enumerate() {
            let mut out: Vec<f32> = l.w.iter().zip(&l.b).map(|(row, b)| row.iter().zip(&h).map(|(w, x)| w * x).sum::<f32>() + b).collect();
            if i + 1 < self.layers.len() {
                out.iter_mut().for_each(|v| *v = v.tanh());
            }
            h = out;
        }
        let sd = |i: usize| h[2 + i].clamp(-4.0, 3.0).exp() * self.y_std[i];
        ([h[0] * self.y_std[0] + self.y_mean[0], h[1] * self.y_std[1] + self.y_mean[1]], [sd(0), sd(1)])
    }
}

/// The aim model, if `COD4RW_NETAIM` names one.
pub fn aim_net() -> Option<&'static Arc<AimNet>> {
    static NET: OnceLock<Option<Arc<AimNet>>> = OnceLock::new();
    NET.get_or_init(|| {
        let path = std::env::var("COD4RW_NETAIM").ok()?;
        match std::fs::read_to_string(&path).ok().and_then(|t| AimNet::from_json(&t)) {
            Some(n) => {
                info!("bots: learned aim from {path}");
                Some(Arc::new(n))
            }
            None => {
                warn!("bots: can't load the aim model {path}");
                None
            }
        }
    })
    .as_ref()
}

/// One bot's state for the aim model.
#[derive(Clone, Debug, Default)]
pub struct NetAimState {
    /// The goal last frame (degrees), to tell its angular velocity.
    last_goal: Option<Vec2>,
    goal_vel: Vec2,
    /// The view's own angular velocity last frame (deg/s).
    view_vel: Vec2,
    /// Since this engagement began (seconds).
    since: f32,
    noise: [f32; 2],
    hold: f32,
}

/// A human hand's tremor never quite stops; the network was trained on a
/// target's chest of about this width (CoD units), which turns the goal's
/// angular width into a distance.
const CHEST_WIDTH: f32 = 16.0;

impl NetAimState {
    /// Turn `view` towards an engaged `goal` the way the recorded player did.
    /// Returns false (doing nothing) when not engaging, for the hand model.
    pub fn update(&mut self, net: &AimNet, view: &mut ViewAngles, goal: Option<AimGoal>, moving: bool, dt: f32, rng: &mut impl Rng) -> bool {
        let Some(goal) = goal.filter(|g| g.combat) else {
            *self = NetAimState::default();
            return false;
        };
        let g = Vec2::new(goal.angles.x.to_degrees(), goal.angles.y.to_degrees());
        if let Some(last) = self.last_goal {
            let d = Vec2::new(wrap_deg(g.x - last.x), g.y - last.y) / dt.max(1e-4);
            // Smoothed over about the training step.
            let k = (dt / net.dt).min(1.0);
            self.goal_vel += (d - self.goal_vel) * k;
        }
        self.last_goal = Some(g);
        let v = Vec2::new(view.yaw.to_degrees(), view.pitch.to_degrees());
        let dist = CHEST_WIDTH / goal.width.max(1e-3);
        let x = [
            wrap_deg(g.x - v.x),
            g.y - v.y,
            self.goal_vel.x,
            self.goal_vel.y,
            self.view_vel.x,
            self.view_vel.y,
            self.since.min(1.5),
            dist.max(50.0).ln(),
            0.0,
            moving as u8 as f32,
        ];
        let (mean, sd) = net.predict(x);
        if self.hold <= 0.0 {
            self.noise = normal_pair(rng);
            self.hold = net.noise_hold;
        }
        self.hold -= dt;
        let vel = Vec2::new(mean[0] + sd[0] * self.noise[0], mean[1] + sd[1] * self.noise[1]);
        view.yaw += (vel.x * dt).to_radians();
        view.pitch = (view.pitch + (vel.y * dt).to_radians()).clamp(-1.5, 1.5);
        self.view_vel = vel;
        self.since += dt;
        true
    }
}

/// Two standard normal draws (Box-Muller).
fn normal_pair(rng: &mut impl Rng) -> [f32; 2] {
    let u1: f32 = rng.random_range(1e-6..1.0);
    let u2: f32 = rng.random_range(0.0..1.0);
    let r = (-2.0 * u1.ln()).sqrt();
    let a = std::f32::consts::TAU * u2;
    [r * a.cos(), r * a.sin()]
}

fn wrap_deg(a: f32) -> f32 {
    (a + 180.0).rem_euclid(360.0) - 180.0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-layer net that turns at the error's rate (10 deg/s per degree).
    fn proportional() -> AimNet {
        let mut w = vec![vec![0.0; N_IN]; 4];
        w[0][0] = 10.0;
        w[1][1] = 10.0;
        w[2][0] = 0.0;
        AimNet {
            x_mean: vec![0.0; N_IN],
            x_std: vec![1.0; N_IN],
            y_mean: [0.0; 2],
            y_std: [1.0; 2],
            layers: vec![Layer { w, b: vec![0.0, 0.0, -4.0, -4.0] }],
            dt: 1.0 / 60.0,
            noise_hold: 0.05,
        }
    }

    #[test]
    fn it_closes_on_an_engaged_target_and_leaves_looking_around_alone() {
        let net = proportional();
        let mut s = NetAimState::default();
        let mut view = ViewAngles::default();
        let mut rng = rand::rng();
        let goal = AimGoal { angles: Vec2::new(20f32.to_radians(), 0.0), width: 0.02, combat: true };
        for _ in 0..120 {
            assert!(s.update(&net, &mut view, Some(goal), false, 1.0 / 60.0, &mut rng));
        }
        assert!((view.yaw.to_degrees() - 20.0).abs() < 1.0, "{}", view.yaw.to_degrees());
        let calm = AimGoal { combat: false, ..goal };
        assert!(!s.update(&net, &mut view, Some(calm), false, 1.0 / 60.0, &mut rng));
    }

    #[test]
    fn json_round_trips() {
        let text = r#"{"features":[],"dt":0.0166,"noise_hold":0.05,"x_mean":[0,0,0,0,0,0,0,0,0,0],"x_std":[1,1,1,1,1,1,1,1,1,1],
            "y_mean":[0,0],"y_std":[1,1],"layers":[{"w":[[1,0,0,0,0,0,0,0,0,0],[0,1,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0],[0,0,0,0,0,0,0,0,0,0]],"b":[0,0,0,0]}]}"#;
        let net = AimNet::from_json(text).unwrap();
        let (m, _) = net.predict([3.0, -2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(m, [3.0, -2.0]);
    }
}

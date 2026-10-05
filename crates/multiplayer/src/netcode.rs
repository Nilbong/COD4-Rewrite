//! Fixed-rate authoritative input consumption, local prediction replay,
//! and delayed remote interpolation. Actual CoD4 movement remains in game.

use crate::protocol::{InputCommand, PawnState, Snapshot, TICK_RATE, button};
use crate::security::newer;
use anyhow::{Result, ensure};
use std::collections::VecDeque;

pub const STEP: f32 = 1.0 / TICK_RATE as f32;
pub const HISTORY: usize = 128;

/// Host consumes at most one player command per authoritative simulation
/// tick. A client cannot speed up its pawn by sending more commands.
#[derive(Default)]
pub struct InputQueue {
    pending: Vec<InputCommand>,
    consumed: u32,
    last: InputCommand,
    idle_ticks: u32,
}

impl InputQueue {
    pub fn insert(&mut self, command: InputCommand) -> Result<bool> {
        command.validate()?;
        if !newer(command.sequence, self.consumed) || command.sequence.wrapping_sub(self.consumed) > HISTORY as u32 {
            return Ok(false);
        }
        if self.pending.iter().any(|c| c.sequence == command.sequence) {
            return Ok(false);
        }
        ensure!(self.pending.len() < HISTORY, "input queue full");
        self.pending.push(command);
        self.pending.sort_by_key(|c| c.sequence.wrapping_sub(self.consumed));
        Ok(true)
    }
    pub fn next_tick(&mut self) -> InputCommand {
        if !self.pending.is_empty() {
            let command = self.pending.remove(0);
            self.consumed = command.sequence;
            self.last = command;
            self.idle_ticks = 0;
            return command;
        }
        self.idle_ticks = self.idle_ticks.saturating_add(1);
        let mut held = self.last;
        // Discrete actions happen once, not again on every missed packet.
        held.buttons &= button::FIRE | button::AIM | button::SPRINT | button::JUMP;
        if self.idle_ticks >= TICK_RATE / 10 {
            held.forward = 0;
            held.right = 0;
            held.buttons = 0;
        }
        held
    }
    pub fn acknowledged(&self) -> u32 {
        self.consumed
    }
}

/// Store a small command history to resend the last three commands in each
/// datagram. Prediction overflow is reported rather than silently losing
/// the state needed to reconcile.
pub struct Prediction<S> {
    pub state: S,
    history: VecDeque<InputCommand>,
    next: u32,
    acknowledged: u32,
}

impl<S: Clone> Prediction<S> {
    pub fn new(state: S) -> Self {
        Self { state, history: VecDeque::new(), next: 1, acknowledged: 0 }
    }
    pub fn advance(
        &mut self,
        mut command: InputCommand,
        simulate: impl FnOnce(&mut S, InputCommand, f32),
    ) -> Result<InputCommand> {
        ensure!(self.history.len() < HISTORY, "prediction backlog full; resynchronization required");
        command.sequence = self.next;
        command.validate()?;
        self.next = self.next.wrapping_add(1);
        simulate(&mut self.state, command, STEP);
        self.history.push_back(command);
        Ok(command)
    }
    pub fn redundant_inputs(&self) -> Vec<InputCommand> {
        self.history
            .iter()
            .rev()
            .take(crate::protocol::INPUT_REDUNDANCY)
            .copied()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }
    pub fn reconcile(
        &mut self,
        authoritative: S,
        ack: u32,
        mut simulate: impl FnMut(&mut S, InputCommand, f32),
    ) -> Result<()> {
        ensure!(!newer(ack, self.next.wrapping_sub(1)), "host acknowledged an unsent input");
        ensure!(ack == self.acknowledged || newer(ack, self.acknowledged), "stale acknowledgement");
        self.acknowledged = ack;
        self.history.retain(|c| newer(c.sequence, ack));
        self.state = authoritative;
        for command in &self.history {
            simulate(&mut self.state, *command, STEP);
        }
        Ok(())
    }
    /// Used after an explicit resynchronization. Keep sequence numbers;
    /// replacing the whole Prediction would restart them on a live connection.
    pub fn reset(&mut self, authoritative: S, ack: u32) -> Result<()> {
        ensure!(!newer(ack, self.next.wrapping_sub(1)), "host acknowledged an unsent input");
        ensure!(ack == self.acknowledged || newer(ack, self.acknowledged), "stale acknowledgement");
        self.state = authoritative;
        self.history.clear();
        self.acknowledged = ack;
        Ok(())
    }
}

/// Samples use host tick time, normally 6 ticks (100 ms) behind the newest
/// snapshot. The game adapter must synchronize its render clock with the host.
#[derive(Default)]
pub struct Interpolation {
    snapshots: VecDeque<Snapshot>,
}
impl Interpolation {
    pub fn push(&mut self, snapshot: Snapshot) -> Result<bool> {
        ensure!(snapshot.pawns.len() <= crate::protocol::MAX_PLAYERS, "too many pawns");
        for pawn in &snapshot.pawns {
            pawn.validate()?;
        }
        if self.snapshots.back().is_some_and(|s| !newer(snapshot.tick, s.tick)) {
            return Ok(false);
        }
        self.snapshots.push_back(snapshot);
        while self.snapshots.len() > 32 {
            self.snapshots.pop_front();
        }
        Ok(true)
    }
    pub fn latest_tick(&self) -> Option<u32> {
        self.snapshots.back().map(|s| s.tick)
    }
    pub fn sample(&self, pawn: u16, tick: u32, fraction: f32) -> Option<PawnState> {
        let first = self.snapshots.front()?;
        if newer(first.tick, tick) {
            return first.pawns.iter().find(|p| p.id == pawn).copied();
        }
        for (a, b) in self.snapshots.iter().zip(self.snapshots.iter().skip(1)) {
            if (tick == a.tick || newer(tick, a.tick)) && newer(b.tick, tick) {
                let before = a.pawns.iter().find(|p| p.id == pawn);
                let after = b.pawns.iter().find(|p| p.id == pawn);
                let (Some(a_p), Some(b_p)) = (before, after) else { return before.copied() };
                let span = b.tick.wrapping_sub(a.tick) as f32;
                let alpha = ((tick.wrapping_sub(a.tick) as f32 + fraction.clamp(0.0, 1.0)) / span).clamp(0.0, 1.0);
                let distance_squared: f32 = a_p.position.iter().zip(b_p.position).map(|(a, b)| (b - a).powi(2)).sum();
                if distance_squared > 64.0 || a_p.life != b_p.life {
                    return Some(*a_p);
                }
                let mut p = *a_p;
                for i in 0..3 {
                    p.position[i] += (b_p.position[i] - p.position[i]) * alpha;
                    p.velocity[i] += (b_p.velocity[i] - p.velocity[i]) * alpha;
                }
                let delta =
                    (b_p.yaw - a_p.yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
                p.yaw = (a_p.yaw + delta * alpha + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
                    - std::f32::consts::PI;
                p.pitch += (b_p.pitch - p.pitch) * alpha;
                return Some(p);
            }
        }
        // Freeze at the newest state rather than extrapolating through walls.
        self.snapshots.back()?.pawns.iter().find(|p| p.id == pawn).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn step(position: &mut f32, c: InputCommand, dt: f32) {
        *position += c.forward as f32 / 127.0 * 5.0 * dt;
    }
    #[test]
    fn prediction_replays_only_unacknowledged_inputs() {
        let mut p = Prediction::new(0.0);
        for _ in 0..6 {
            p.advance(InputCommand { forward: 127, ..Default::default() }, step).unwrap();
        }
        p.reconcile(1.0, 3, step).unwrap();
        assert!((p.state - 1.25).abs() < 0.00001);
        assert_eq!(p.redundant_inputs().iter().map(|c| c.sequence).collect::<Vec<_>>(), [4, 5, 6]);
        assert!(p.reconcile(0.0, 7, step).is_err());
        assert!(p.reconcile(0.0, 2, step).is_err());
    }

    #[test]
    fn prediction_converges_under_delay_loss_and_reordered_input_batches() {
        let mut prediction = Prediction::new(0.0);
        let mut queue = InputQueue::default();
        let mut authoritative = 0.0;
        let mut transit: Vec<(u32, Vec<InputCommand>)> = Vec::new();
        let mut snapshots = Vec::new();
        for tick in 1..=300u32 {
            // A deterministic lossy network, variable 2..7-tick latency.
            prediction
                .advance(InputCommand { forward: if tick < 240 { 127 } else { 0 }, ..Default::default() }, step)
                .unwrap();
            if tick % 5 != 0 {
                transit.push((tick + 2 + (tick * 7 % 6), prediction.redundant_inputs()));
            }
            let arrived: Vec<_> = transit.extract_if(.., |(at, _)| *at <= tick).collect();
            for (_, commands) in arrived {
                for c in commands {
                    queue.insert(c).unwrap();
                }
            }
            step(&mut authoritative, queue.next_tick(), STEP);
            if tick % 3 == 0 && tick % 15 != 0 {
                snapshots.push((tick + 4, queue.acknowledged(), authoritative));
            }
            let arrived: Vec<_> = snapshots.extract_if(.., |(at, _, _)| *at <= tick).collect();
            for (_, ack, x) in arrived {
                prediction.reconcile(x, ack, step).unwrap();
            }
            assert!(prediction.history.len() < 24);
        }
        // Final authoritative correction replays only neutral inputs.
        prediction.reconcile(authoritative, queue.acknowledged(), step).unwrap();
        assert!((prediction.state - authoritative).abs() < 0.00001);
        assert!(authoritative > 15.0 && authoritative < 21.0);
    }

    #[test]
    fn prediction_overflow_requires_explicit_resynchronization() {
        let mut prediction = Prediction::new(0.0);
        for _ in 0..HISTORY {
            prediction.advance(InputCommand::default(), step).unwrap();
        }
        assert!(prediction.advance(InputCommand::default(), step).is_err());
        prediction.reset(5.0, HISTORY as u32).unwrap();
        assert_eq!(prediction.advance(InputCommand::default(), step).unwrap().sequence, HISTORY as u32 + 1);
    }
    #[test]
    fn command_flood_cannot_advance_extra_ticks_and_loss_releases_actions() {
        let mut q = InputQueue::default();
        for n in (1..=30).rev() {
            q.insert(InputCommand {
                sequence: n,
                buttons: button::RELOAD | button::FIRE,
                forward: 127,
                ..Default::default()
            })
            .unwrap();
        }
        assert!(!q.insert(InputCommand { sequence: 1, ..Default::default() }).unwrap());
        assert_eq!(q.next_tick().sequence, 1);
        assert_eq!(q.acknowledged(), 1);
        for _ in 0..29 {
            q.next_tick();
        }
        assert_eq!(q.next_tick().buttons, button::FIRE);
        for _ in 0..5 {
            q.next_tick();
        }
        assert_eq!(q.next_tick().forward, 0);
        assert!(!q.insert(InputCommand { sequence: 10000, ..Default::default() }).unwrap());
    }
    #[test]
    fn interpolation_handles_angles_respawns_and_stale_packets() {
        let mut i = Interpolation::default();
        let snap = |tick, x, yaw, life| Snapshot {
            tick,
            acknowledged_input: 0,
            pawns: vec![PawnState { id: 1, position: [x, 0.0, 0.0], yaw, life, ..Default::default() }],
        };
        i.push(snap(10, 0.0, 3.1, 0)).unwrap();
        i.push(snap(16, 6.0, -3.1, 0)).unwrap();
        let mid = i.sample(1, 13, 0.0).unwrap();
        assert!((mid.position[0] - 3.0).abs() < 0.001);
        assert!(mid.yaw.abs() > 3.0);
        assert!(!i.push(snap(12, 0.0, 0.0, 0)).unwrap());
        i.push(snap(22, 100.0, 0.0, 1)).unwrap();
        assert_eq!(i.sample(1, 19, 0.0).unwrap().position[0], 6.0);
        assert_eq!(i.sample(1, 50, 0.0).unwrap().position[0], 100.0);
    }
}

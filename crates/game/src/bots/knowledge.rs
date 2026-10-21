//! What a bot learns about the other team during a match: where it was
//! killed from, where it saw someone sitting still. A spot gathers weight
//! each time and fades away when nothing more happens there. How fast a bot
//! learns and how long it remembers comes with experience
//! ([`super::persona::Persona::knowledge`]): a veteran who dies twice to
//! the same camper checks that corner, nades it or goes round; a new player
//! keeps walking into it.

use crate::units::u;
use bevy::prelude::*;

/// Notes this close together are one spot.
const SAME_SPOT: f32 = u(250.0);
/// At most this many spots remembered (the weakest go).
const MAX_SPOTS: usize = 12;
/// A spot worth acting on (about two deaths there for a veteran).
pub const STRONG: f32 = 1.0;

#[derive(Clone, Copy, Debug)]
pub struct Spot {
    pub pos: Vec3,
    pub weight: f32,
    pub at: f32,
}

#[derive(Clone, Debug, Default)]
pub struct Learned {
    pub spots: Vec<Spot>,
    decayed_at: f32,
}

/// How much one lesson counts, for a bot with this `knowledge` (0..1).
pub fn rate(knowledge: f32) -> f32 {
    0.25 + 0.75 * knowledge
}

/// Seconds for a spot's weight to fall to a third.
fn memory(knowledge: f32) -> f32 {
    40.0 + 160.0 * knowledge
}

impl Learned {
    /// Something happened at `pos`: killed from there (`amount` ~0.6), an
    /// enemy seen holding there (a little per second).
    pub fn note(&mut self, pos: Vec3, amount: f32, now: f32) {
        if let Some(s) = self.spots.iter_mut().find(|s| s.pos.distance(pos) < SAME_SPOT) {
            // Drift towards the latest, by how much this adds.
            let k = amount / (s.weight + amount);
            s.pos = s.pos.lerp(pos, k);
            s.weight += amount;
            s.at = now;
        } else {
            self.spots.push(Spot { pos, weight: amount, at: now });
        }
        if self.spots.len() > MAX_SPOTS {
            self.spots.sort_by(|a, b| b.weight.total_cmp(&a.weight));
            self.spots.truncate(MAX_SPOTS);
        }
    }

    pub fn decay(&mut self, now: f32, knowledge: f32) {
        let dt = (now - self.decayed_at).clamp(0.0, 5.0);
        if dt < 0.5 {
            return;
        }
        self.decayed_at = now;
        let k = (-dt / memory(knowledge)).exp();
        for s in &mut self.spots {
            s.weight *= k;
        }
        self.spots.retain(|s| s.weight > 0.05);
    }

    /// The strongest spot between `near` and `far` from `feet`.
    pub fn strongest(&self, feet: Vec3, near: f32, far: f32) -> Option<Spot> {
        self.spots
            .iter()
            .filter(|s| (near..far).contains(&s.pos.distance(feet)))
            .max_by(|a, b| a.weight.total_cmp(&b.weight))
            .copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn veterans_remember_longer() {
        let mut vet = Learned::default();
        let mut new = Learned::default();
        for l in [&mut vet, &mut new] {
            l.note(Vec3::ZERO, 1.0, 0.0);
            l.note(Vec3::X * u(100.0), 1.0, 0.0);
        }
        assert_eq!(vet.spots.len(), 1, "close notes merge");
        for t in 1..=120 {
            vet.decay(t as f32, 1.0);
            new.decay(t as f32, 0.0);
        }
        let w = |l: &Learned| l.spots.first().map_or(0.0, |s| s.weight);
        assert!(w(&vet) > 2.0 * w(&new), "vet {} new {}", w(&vet), w(&new));
    }
}

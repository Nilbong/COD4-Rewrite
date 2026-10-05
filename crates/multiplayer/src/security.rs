//! Resource limits are defense in depth; upstream DDoS protection is still
//! needed for the relay's public IP and internet link.

use std::time::{Duration, Instant};

pub struct RateLimit {
    tokens: f64,
    burst: f64,
    per_second: f64,
    updated: Instant,
}

impl RateLimit {
    pub fn new(per_second: f64, burst: f64, now: Instant) -> Self {
        assert!(per_second > 0.0 && burst > 0.0);
        Self { tokens: burst, burst, per_second, updated: now }
    }
    pub fn allow(&mut self, cost: usize, now: Instant) -> bool {
        self.tokens =
            (self.tokens + now.saturating_duration_since(self.updated).as_secs_f64() * self.per_second).min(self.burst);
        self.updated = now;
        if cost as f64 > self.tokens {
            return false;
        }
        self.tokens -= cost as f64;
        true
    }
}

/// Wrapping sequence comparison, valid when values are less than 2^31 apart.
pub fn newer(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) > 0
}

/// Permit late packets within a 64-command window, reject duplicates and
/// absurd forward jumps. The initial command must be within the first 128.
#[derive(Default)]
pub struct ReplayWindow {
    newest: u32,
    bits: u64,
}
impl ReplayWindow {
    pub fn accept(&mut self, sequence: u32) -> bool {
        if newer(sequence, self.newest) {
            let delta = sequence.wrapping_sub(self.newest);
            if delta > 128 {
                return false;
            }
            self.bits = if delta >= 64 { 1 } else { (self.bits << delta) | 1 };
            self.newest = sequence;
            true
        } else {
            let age = self.newest.wrapping_sub(sequence);
            if age >= 64 || sequence == 0 && self.newest < 64 {
                return false;
            }
            let bit = 1u64 << age;
            if self.bits & bit != 0 {
                return false;
            }
            self.bits |= bit;
            true
        }
    }
}

pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
pub const MAX_CONNECTIONS: usize = 128;
pub const MAX_ROOMS: usize = 32;
pub const MAX_PER_IP: usize = 8;
pub const CONTROL_QUEUE: usize = 32;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rate_limits_bursts_and_recovers() {
        let now = Instant::now();
        let mut r = RateLimit::new(10.0, 20.0, now);
        assert!(r.allow(20, now));
        assert!(!r.allow(1, now));
        assert!(r.allow(10, now + Duration::from_secs(1)));
        assert!(!r.allow(11, now + Duration::from_secs(1)));
    }
    #[test]
    fn replay_window_handles_reorder_duplicates_and_attacks() {
        let mut r = ReplayWindow::default();
        assert!(!r.accept(0));
        assert!(!r.accept(10000));
        assert!(r.accept(3));
        assert!(r.accept(1));
        assert!(r.accept(2));
        assert!(!r.accept(2));
        assert!(r.accept(80));
        assert!(!r.accept(3));
        assert!(!r.accept(500));
        assert!(r.accept(81));
    }
    #[test]
    fn wrapping_sequences_are_ordered() {
        assert!(newer(0, u32::MAX));
        assert!(!newer(u32::MAX, 0));
        assert!(!newer(4, 4));
        let mut r = ReplayWindow { newest: u32::MAX - 1, bits: 1 };
        assert!(r.accept(u32::MAX));
        assert!(r.accept(0));
        assert!(r.accept(1));
        assert!(!r.accept(0));
    }
}

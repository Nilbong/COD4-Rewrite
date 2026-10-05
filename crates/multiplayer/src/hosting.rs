//! Host election policy for a relay lobby. Measurement/matchmaking is a
//! later service; never trust a player's claimed bandwidth or latency.

use crate::protocol::{MAX_PACKET, MAX_PLAYERS, PeerId, SNAPSHOT_RATE};

#[derive(Clone, Copy, Debug)]
pub struct Candidate {
    pub player: PeerId,
    /// Measurements to the lobby's relay, not an IP address for a peer.
    pub rtt_ms: f32,
    pub jitter_ms: f32,
    pub loss_fraction: f32,
    /// Conservative sustained throughput measured by a service, in kbps.
    pub upload_kbps: f32,
    /// Observed 95th percentile authoritative simulation cost, milliseconds.
    pub simulation_ms: f32,
}

/// Prefer stable low latency among candidates who have enough CPU/upload.
/// Hosting needs more than the largest advertised download speed. The
/// session currently creates a room with a manually chosen host; this
/// policy is reusable by the future lobby, not automatic migration.
pub fn choose_host(candidates: &[Candidate], players: usize) -> Option<PeerId> {
    if !(2..=MAX_PLAYERS).contains(&players) {
        return None;
    }
    let required_upload = (players - 1) as f32 * MAX_PACKET as f32 * SNAPSHOT_RATE as f32 * 8.0 * 1.5 / 1000.0;
    candidates
        .iter()
        .filter(|c| {
            [c.rtt_ms, c.jitter_ms, c.loss_fraction, c.upload_kbps, c.simulation_ms]
                .into_iter()
                .all(|v| v.is_finite() && v >= 0.0)
                && c.rtt_ms <= 250.0
                && c.loss_fraction <= 0.02
                && c.upload_kbps >= required_upload
                && c.simulation_ms < 1000.0 / crate::protocol::TICK_RATE as f32
        })
        .min_by(|a, b| {
            let score = |c: &Candidate| c.rtt_ms + 2.0 * c.jitter_ms + 1000.0 * c.loss_fraction;
            score(a).total_cmp(&score(b)).then(a.player.cmp(&b.player))
        })
        .map(|c| c.player)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn host_selection_prefers_stability_and_sufficient_capacity() {
        let base = Candidate {
            player: 1,
            rtt_ms: 20.0,
            jitter_ms: 2.0,
            loss_fraction: 0.0,
            upload_kbps: 10_000.0,
            simulation_ms: 8.0,
        };
        let noisy = Candidate { player: 2, rtt_ms: 10.0, jitter_ms: 20.0, ..base };
        let slow_upload = Candidate { player: 3, rtt_ms: 1.0, upload_kbps: 100.0, ..base };
        let slow_cpu = Candidate { player: 4, rtt_ms: 1.0, simulation_ms: 25.0, ..base };
        assert_eq!(choose_host(&[noisy, slow_upload, base, slow_cpu], 18), Some(1));
        assert_eq!(choose_host(&[slow_upload, slow_cpu], 18), None);
        assert_eq!(choose_host(&[Candidate { rtt_ms: f32::NAN, ..base }], 2), None);
    }
}

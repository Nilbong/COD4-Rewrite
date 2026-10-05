//! The UAV (`radar_mp`): for `level.radarViewTime` (30 s) the team's
//! compasses (in free-for-all, its caller's) show every enemy, refreshed
//! each radar sweep (`compassRadarUpdateTime` 4 s) and fading between
//! sweeps (`compassRadarPingFadeTime` 4 s). The sweeps are drawn by the HUD
//! ([`crate::ui`]).

use crate::combat::Team;
use bevy::prelude::*;

/// How long a UAV stays up.
pub const RADAR_TIME: f32 = 30.0;
/// Seconds between sweeps, and how long a sweep's pings take to fade.
pub const SWEEP_TIME: f32 = 4.0;
pub const PING_FADE: f32 = 4.0;

/// Whose compasses a UAV shows on: a team's, or in free-for-all its
/// caller's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Team(Team),
    Player(Entity),
}

impl Side {
    /// The side of `player`, on `team`, in this match.
    pub fn of(team: Team, player: Entity) -> Side {
        if crate::combat::free_for_all() { Side::Player(player) } else { Side::Team(team) }
    }
}

/// The UAVs up: whose, when called and until when.
#[derive(Resource, Default, Debug)]
pub struct Radar {
    up: Vec<(Side, f32, f32)>,
}

impl Radar {
    /// A new UAV for `side`: one already up starts over
    /// (`radar_timer_kill_<team>`).
    pub fn call(&mut self, side: Side, now: f32) {
        self.up.retain(|u| u.0 != side);
        self.up.push((side, now, now + RADAR_TIME));
    }

    /// The sweep `team`'s UAV last made by `now`: (its number, its time),
    /// or `None` if none is up. In free-for-all, [`Radar::sweep_for`].
    #[cfg(test)]
    pub fn sweep(&self, team: Team, now: f32) -> Option<(u32, f32)> {
        self.sweep_side(Side::Team(team), now)
    }

    /// The sweep showing on `player`'s compass (on `team`), team play or
    /// free-for-all.
    pub fn sweep_for(&self, team: Team, player: Entity, now: f32) -> Option<(u32, f32)> {
        self.sweep_side(Side::of(team, player), now)
    }

    fn sweep_side(&self, side: Side, now: f32) -> Option<(u32, f32)> {
        let &(_, start, until) = self.up.iter().find(|u| u.0 == side)?;
        if now < start || now >= until {
            return None;
        }
        let k = ((now - start) / SWEEP_TIME) as u32;
        Some((k, start + k as f32 * SWEEP_TIME))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sweeps_every_four_seconds_for_thirty() {
        let mut r = Radar::default();
        r.call(Side::Team(Team::Axis), 10.0);
        assert_eq!(r.sweep(Team::Allies, 11.0), None);
        assert_eq!(r.sweep(Team::Axis, 10.0), Some((0, 10.0)));
        assert_eq!(r.sweep(Team::Axis, 19.5), Some((2, 18.0)));
        assert_eq!(r.sweep(Team::Axis, 40.0), None);
        let me = Entity::from_raw_u32(7).unwrap();
        r.call(Side::Player(me), 50.0);
        assert_eq!(r.sweep_side(Side::Player(me), 51.0), Some((0, 50.0)));
        assert_eq!(r.sweep(Team::Allies, 51.0), None);
    }
}

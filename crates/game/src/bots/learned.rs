//! What real players did on each map, read from CoD4 demos: where they
//! walked, where they stopped to hold, and which way they watched from
//! there. The map analysis ([`super::tactical`]) uses it in place of its
//! own guesses where there is any.

use super::motion::{Sample, tracks};
use crate::units;
use bevy::prelude::*;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Notes per map, by map name (`mp_killhouse`), lower case.
#[derive(Resource, Clone, Default)]
pub struct DemoNotes(pub Arc<HashMap<String, MapNotes>>);

impl DemoNotes {
    pub fn get(&self, map: &str) -> Option<&MapNotes> {
        self.0.get(&map.to_ascii_lowercase())
    }
}

#[derive(Default)]
pub struct MapNotes {
    /// Where players on the move were, four times a second (Bevy space,
    /// feet).
    pub visits: Vec<Vec3>,
    /// Places players stood and held, most held first.
    pub holds: Vec<Held>,
    pub players: usize,
    /// Player time watched, all players together.
    pub seconds: f32,
}

/// A place real players held.
pub struct Held {
    /// Feet (Bevy space).
    pub pos: Vec3,
    /// Time held, all players together.
    pub seconds: f32,
    pub players: usize,
    /// Where they looked from there (unit vectors, Bevy space, pitch
    /// included) with each one's share of the time, most watched first.
    pub looks: Vec<(Vec3, f32)>,
}

/// Standing within this of where a stop began, for this long, is a hold.
const STILL_RADIUS: f32 = 24.0;
const MIN_HOLD: f32 = 1.5;
/// Stops just after a respawn (or coming into view) are ignored.
const SETTLE: f32 = 2.0;
/// Holds closer than this (horizontally, on the same floor) are one place.
const SAME_PLACE: f32 = 120.0;
const SAME_FLOOR: f32 = 48.0;
/// Places held less than this in total are noise.
const MIN_PLACE: f32 = 3.0;
/// Below this speed (units/s) a player isn't on the move.
const MOVING: f32 = 60.0;
/// Look directions are found in bins of this many degrees.
const LOOK_BIN: f32 = 15.0;
/// At most this many look directions per place, each this share at least.
const MAX_LOOKS: usize = 3;
const MIN_LOOK_SHARE: f32 = 0.1;

/// A hold: where, how long, by whom, and the view (yaw, pitch degrees,
/// CoD's sense) each sample of it.
struct Stop {
    pos: Vec3,
    seconds: f32,
    player: String,
    views: Vec<(f32, f32)>,
}

pub fn build(demos: &[iw3::demo::Demo]) -> DemoNotes {
    let mut maps: HashMap<String, (MapNotes, Vec<Stop>, HashSet<String>)> = HashMap::new();
    for demo in demos {
        let Some(map) = demo.server_info("mapname") else { continue };
        let (notes, stops, players) = maps.entry(map.to_ascii_lowercase()).or_default();
        for (player, run) in &tracks(demo) {
            players.insert(player.clone());
            notes.seconds += run.len() as f32 / super::motion::RATE;
            for w in run.windows(2).step_by(5) {
                if w[1].pos.distance(w[0].pos) / (w[1].time - w[0].time).max(1e-3) > MOVING {
                    notes.visits.push(units::pos(w[0].pos.to_array()));
                }
            }
            find_stops(run, player, stops);
        }
    }
    let notes = maps
        .into_iter()
        .map(|(map, (mut notes, stops, players))| {
            notes.players = players.len();
            notes.holds = places(stops);
            (map, notes)
        })
        .collect();
    DemoNotes(Arc::new(notes))
}

/// The holds in one run of samples.
fn find_stops(run: &[Sample], player: &str, out: &mut Vec<Stop>) {
    let mut j = 0;
    while j < run.len() {
        let mut k = j;
        while k + 1 < run.len() && run[k + 1].pos.distance(run[j].pos) < STILL_RADIUS {
            k += 1;
        }
        let seconds = run[k].time - run[j].time;
        if seconds >= MIN_HOLD && run[j].time - run[0].time > SETTLE {
            out.push(Stop {
                // The middle sample: the first may still be landing a jump.
                pos: run[(j + k) / 2].pos,
                seconds,
                player: player.to_string(),
                views: run[j..=k].iter().map(|s| (s.yaw, s.pitch)).collect(),
            });
        }
        j = k + 1;
    }
}

/// Holds gathered into places, most held first.
fn places(mut stops: Vec<Stop>) -> Vec<Held> {
    stops.sort_by(|a, b| b.seconds.total_cmp(&a.seconds));
    let mut groups: Vec<(Vec3, f32, Vec<String>, Vec<(f32, f32)>)> = Vec::new();
    for s in stops {
        let near = |p: Vec3| p.truncate().distance(s.pos.truncate()) < SAME_PLACE && (p.z - s.pos.z).abs() < SAME_FLOOR;
        match groups.iter_mut().find(|g| near(g.0)) {
            Some(g) => {
                g.1 += s.seconds;
                if !g.2.contains(&s.player) {
                    g.2.push(s.player);
                }
                g.3.extend(s.views);
            }
            None => groups.push((s.pos, s.seconds, vec![s.player], s.views)),
        }
    }
    let mut held: Vec<Held> = groups
        .into_iter()
        .filter(|g| g.1 >= MIN_PLACE)
        .map(|(pos, seconds, players, views)| Held {
            pos: units::pos(pos.to_array()),
            seconds,
            players: players.len(),
            looks: looks(&views),
        })
        .collect();
    held.sort_by(|a, b| b.seconds.total_cmp(&a.seconds));
    held
}

/// The main directions in a set of views: peaks of the yaw histogram, each
/// taking in its neighbouring bins.
fn looks(views: &[(f32, f32)]) -> Vec<(Vec3, f32)> {
    const BINS: usize = (360.0 / LOOK_BIN) as usize;
    let bin = |yaw: f32| (yaw.rem_euclid(360.0) / LOOK_BIN) as usize % BINS;
    let mut counts = [0usize; BINS];
    for &(yaw, _) in views {
        counts[bin(yaw)] += 1;
    }
    let mut out = Vec::new();
    let total = views.len().max(1) as f32;
    while out.len() < MAX_LOOKS {
        let (peak, _) = counts.iter().enumerate().max_by_key(|c| c.1).unwrap();
        let window = [(peak + BINS - 1) % BINS, peak, (peak + 1) % BINS];
        let mine: Vec<(f32, f32)> = views.iter().copied().filter(|v| window.contains(&bin(v.0))).collect();
        let share = window.iter().map(|&b| counts[b]).sum::<usize>() as f32 / total;
        if share < MIN_LOOK_SHARE || mine.is_empty() {
            break;
        }
        for b in window {
            counts[b] = 0;
        }
        // Mean direction (yaw wraps; pitch doesn't).
        let (mut x, mut y, mut pitch) = (0.0, 0.0, 0.0);
        for &(yw, p) in &mine {
            x += yw.to_radians().cos();
            y += yw.to_radians().sin();
            pitch += p;
        }
        let (yaw, pitch) = (y.atan2(x), (pitch / mine.len() as f32).to_radians());
        // CoD: pitch + is down.
        let cod = [yaw.cos() * pitch.cos(), yaw.sin() * pitch.cos(), -pitch.sin()];
        out.push((units::dir(cod).normalize(), share));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_find_the_two_directions_watched() {
        // 70% due +X (CoD yaw 0, a little either side), 30% along +Y.
        let mut views = Vec::new();
        for i in 0..70 {
            views.push(((i % 7) as f32 - 3.0, 0.0));
        }
        for _ in 0..30 {
            views.push((90.0, 10.0));
        }
        let l = looks(&views);
        assert_eq!(l.len(), 2);
        assert!((l[0].1 - 0.7).abs() < 0.01 && (l[1].1 - 0.3).abs() < 0.01);
        // CoD +X is Bevy +X; CoD +Y is Bevy -Z, looking a little down.
        assert!(l[0].0.dot(Vec3::X) > 0.99);
        assert!(l[1].0.dot(-Vec3::Z) > 0.97 && l[1].0.y < -0.1);
    }

    #[test]
    fn holds_at_the_same_place_are_one() {
        // CoD space: z is up.
        let stop = |x: f32, z: f32, seconds: f32, player: &str| Stop {
            pos: Vec3::new(x, 0.0, z),
            seconds,
            player: player.into(),
            views: vec![(0.0, 0.0)],
        };
        let held = places(vec![
            stop(0.0, 0.0, 2.0, "a"),
            stop(50.0, 0.0, 3.0, "b"),
            stop(50.0, 0.0, 9.0, "c"),
            // Upstairs: somewhere else.
            stop(50.0, 120.0, 5.0, "a"),
            // Too little time there.
            stop(900.0, 0.0, 2.0, "a"),
        ]);
        assert_eq!(held.len(), 2);
        assert_eq!(held[0].players, 3);
        assert!((held[0].seconds - 14.0).abs() < 1e-3);
        assert!((held[1].pos.y - crate::units::u(120.0)).abs() < 1e-4);
    }
}

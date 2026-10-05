//! What a player learns about a map: where each side spawns, the lanes
//! people move along between those areas, and the spots worth holding.
//!
//! Worked out once per map from the spawn points, the navigation graph and
//! line-of-sight checks:
//! - spawn areas: spawn points clustered by distance;
//! - traffic: routes between many pairs of spawn points (each pair's second
//!   route avoiding the first) are planned over the nav graph, and every nav
//!   point counts the routes that cross it;
//! - lane points: a spread-out sample of the busiest ground, where enemies
//!   will actually walk;
//! - holding spots: points with cover close by that see a lot of lane at
//!   fighting range without standing in the lane, each with the direction
//!   it watches.
//!
//! Enemy spawns are predicted with the game's own rule (they spawn far from
//! their nearest enemy), from where the bot's team is.
//!
//! Where there are demos of real matches on the map ([`super::learned`]),
//! traffic leans on where real players walked, and the places they held
//! become holding spots, each watching what they watched from there; the
//! worked-out spots stay as a fallback.

use super::learned::MapNotes;
use super::nav::NavGraph;
use crate::collision;
use crate::units::u;
use crate::world::{MapInfo, SpawnKind};
use avian3d::prelude::*;
use bevy::prelude::*;
use rand::Rng;
use rand::SeedableRng;
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct SpawnArea {
    pub center: Vec3,
    /// Indices into `MapInfo::spawns`.
    pub spawns: Vec<usize>,
}

#[derive(Clone, Copy, Debug)]
pub struct LanePoint {
    pub node: u32,
    pub pos: Vec3,
    pub traffic: f32,
}

#[derive(Clone, Debug)]
pub struct HoldSpot {
    pub pos: Vec3,
    pub score: f32,
    /// Lane points visible from here.
    pub watches: Vec<u16>,
    /// Main direction to watch (horizontal, normalised).
    pub facing: Vec3,
    /// 0 for a spot worked out from the map; for a place real players
    /// held, how much (the most held is 1).
    pub learned: f32,
    /// What real players watched from here, most watched first.
    pub looks: Vec<Look>,
}

/// A direction real players watched from a holding spot.
#[derive(Clone, Debug)]
pub struct Look {
    /// A point on the floor: watched from eye height, it's the way they
    /// looked.
    pub point: Vec3,
    /// Share of the time they looked this way.
    pub share: f32,
    /// Lane points that way.
    pub lanes: Vec<u16>,
}

/// A left/right coordinate across the map (see [`TacticalMap::lateral`]).
#[derive(Clone, Copy, Debug)]
pub struct Lateral {
    origin: Vec3,
    across: Vec3,
    mid: f32,
    half: f32,
}

impl Lateral {
    /// Where `p` is across the map: about -1 on the left lanes, 1 on the right.
    pub fn of(&self, p: Vec3) -> f32 {
        ((p - self.origin).dot(self.across) - self.mid) / self.half
    }
}

#[derive(Resource)]
pub struct TacticalMap {
    /// Per nav node, 0..1.
    pub traffic: Vec<f32>,
    pub lanes: Vec<LanePoint>,
    pub spots: Vec<HoldSpot>,
    pub areas: Vec<SpawnArea>,
    /// Whether the spots come from real matches.
    pub learned: bool,
}

const AREA_LINK: f32 = u(400.0);
const ROUTES_PER_PAIR: usize = 2;
const MAX_PAIRS: usize = 400;
const MIN_PAIR_DISTANCE: f32 = u(800.0);
const LANE_SPACING: f32 = u(160.0);
const MAX_LANE_POINTS: usize = 220;
const SPOT_SPACING: f32 = u(220.0);
const MAX_SPOTS: usize = 80;
const WATCH_RANGE: f32 = u(2600.0);
/// Height above the floor bots watch a lane point at: chest high.
pub const CHECK_HEIGHT: f32 = u(46.0);
/// How much of the traffic comes from real players' walking, where known.
const REAL_TRAFFIC: f32 = 0.7;
/// A lane point within this angle of a look is watched by it.
const LOOK_CONE: f32 = 0.77; // cos 40°
/// Places real players held this little (of the most held) aren't spots.
const MIN_LEARNED: f32 = 0.05;
/// How much less worked-out spots are picked on a map with real holds.
const GUESSED_WEIGHT: f32 = 0.3;

impl TacticalMap {
    pub fn build(nav: &NavGraph, map: &MapInfo, spatial: &SpatialQuery, notes: Option<&MapNotes>) -> TacticalMap {
        let areas = spawn_areas(map);
        let mut traffic = traffic(nav, map);
        if let Some(notes) = notes.filter(|n| !n.visits.is_empty()) {
            let real = real_traffic(nav, &notes.visits);
            for (t, r) in traffic.iter_mut().zip(real) {
                *t = (1.0 - REAL_TRAFFIC) * *t + REAL_TRAFFIC * r;
            }
        }
        let lanes = lane_points(nav, &traffic);
        let mut spots = notes.map_or_else(Vec::new, |n| learned_spots(nav, &lanes, spatial, n));
        let learned = !spots.is_empty();
        for s in hold_spots(nav, &traffic, &lanes, spatial) {
            if spots.iter().all(|t| t.pos.distance(s.pos) > SPOT_SPACING) {
                spots.push(s);
            }
        }
        TacticalMap { traffic, lanes, spots, areas, learned }
    }

    /// Spawn points enemies of a team standing at `my_team` will likely use
    /// next, with weights summing to 1 (the game spawns players far from
    /// their nearest enemy).
    pub fn predicted_enemy_spawns(&self, map: &MapInfo, my_team: &[Vec3]) -> Vec<(Vec3, f32)> {
        if my_team.is_empty() {
            return Vec::new();
        }
        let scored: Vec<(Vec3, f32)> = map
            .spawns
            .iter()
            .filter(|s| matches!(s.kind, SpawnKind::Tdm | SpawnKind::Dm))
            .map(|s| (s.pos, my_team.iter().map(|p| p.distance(s.pos)).fold(f32::MAX, f32::min)))
            .collect();
        let best = scored.iter().map(|s| s.1).fold(0.0, f32::max);
        // Spawns within ~25% of the best distance are about as likely.
        let mut weights: Vec<(Vec3, f32)> =
            scored.iter().map(|&(p, d)| (p, (-(best - d) / (best * 0.12 + u(100.0))).exp())).collect();
        let total: f32 = weights.iter().map(|w| w.1).sum();
        for w in &mut weights {
            w.1 /= total.max(1e-6);
        }
        weights.retain(|w| w.1 > 0.01);
        weights
    }

    /// Lane points within `radius` of `p`.
    pub fn lanes_near(&self, p: Vec3, radius: f32) -> impl Iterator<Item = (usize, &LanePoint)> {
        self.lanes.iter().enumerate().filter(move |(_, l)| l.pos.distance(p) < radius)
    }
}

fn spawn_areas(map: &MapInfo) -> Vec<SpawnArea> {
    let n = map.spawns.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut [usize], mut i: usize) -> usize {
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    for i in 0..n {
        for j in i + 1..n {
            if map.spawns[i].pos.distance(map.spawns[j].pos) < AREA_LINK {
                let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                parent[a] = b;
            }
        }
    }
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        let r = find(&mut parent, i);
        groups.entry(r).or_default().push(i);
    }
    groups
        .into_values()
        .map(|spawns| {
            let center = spawns.iter().map(|&i| map.spawns[i].pos).sum::<Vec3>() / spawns.len() as f32;
            SpawnArea { center, spawns }
        })
        .collect()
}

fn traffic(nav: &NavGraph, map: &MapInfo) -> Vec<f32> {
    let mut traffic = vec![0.0f32; nav.nodes.len()];
    // People walk between spawns: plan routes between pairs of spawn points
    // that are far enough apart to cross the map (a seeded sample on big maps).
    let ends: Vec<(Vec3, u32)> = map.spawns.iter().filter_map(|s| nav.nearest(s.pos).map(|n| (s.pos, n))).collect();
    let mut pairs: Vec<(u32, u32)> = Vec::new();
    for i in 0..ends.len() {
        for j in i + 1..ends.len() {
            if ends[i].0.distance(ends[j].0) > MIN_PAIR_DISTANCE {
                pairs.push((ends[i].1, ends[j].1));
            }
        }
    }
    let mut rng = rand::rngs::StdRng::seed_from_u64(2);
    if pairs.len() > MAX_PAIRS {
        for k in 0..MAX_PAIRS {
            let j = rng.random_range(k..pairs.len());
            pairs.swap(k, j);
        }
        pairs.truncate(MAX_PAIRS);
    }
    for (a, b) in pairs {
        {
            let mut used: HashMap<u32, f32> = HashMap::new();
            for _ in 0..ROUTES_PER_PAIR {
                let Some(path) = nav.path(a, b, 120_000, |n| used.get(&n).copied().unwrap_or(0.0)) else {
                    break;
                };
                for &n in &path {
                    traffic[n as usize] += 1.0;
                    // Push later routes elsewhere so lanes other than the
                    // single shortest one count too.
                    *used.entry(n).or_default() += u(40.0);
                    for l in &nav.nodes[n as usize].links {
                        traffic[l.to as usize] += 0.35;
                        *used.entry(l.to).or_default() += u(20.0);
                    }
                }
            }
        }
    }
    let max = traffic.iter().cloned().fold(0.0, f32::max).max(1e-6);
    for t in &mut traffic {
        *t /= max;
    }
    traffic
}

/// Where real players walked, per nav node: 1 for the busiest ground.
fn real_traffic(nav: &NavGraph, visits: &[Vec3]) -> Vec<f32> {
    let mut t = vec![0.0f32; nav.nodes.len()];
    let radius = u(48.0);
    for &p in visits {
        for i in nav.within(p, radius) {
            let d = nav.nodes[i as usize].pos.distance(p);
            t[i as usize] += (1.0 - d / radius).max(0.0);
        }
    }
    // Scaled by the busiest ground but one in twenty, so a few spots
    // everyone crosses don't flatten the rest.
    let mut busy: Vec<f32> = t.iter().copied().filter(|&v| v > 0.0).collect();
    busy.sort_by(f32::total_cmp);
    let top = busy.get(busy.len() * 95 / 100).copied().unwrap_or(1.0).max(1e-6);
    for v in &mut t {
        *v = (*v / top).min(1.0);
    }
    t
}

fn lane_points(nav: &NavGraph, traffic: &[f32]) -> Vec<LanePoint> {
    let mut order: Vec<u32> = (0..nav.nodes.len() as u32).filter(|&i| traffic[i as usize] > 0.08).collect();
    order.sort_by(|&a, &b| traffic[b as usize].total_cmp(&traffic[a as usize]));
    let mut lanes: Vec<LanePoint> = Vec::new();
    for i in order {
        let pos = nav.nodes[i as usize].pos;
        if lanes.iter().all(|l| l.pos.distance(pos) > LANE_SPACING) {
            lanes.push(LanePoint { node: i, pos, traffic: traffic[i as usize] });
            if lanes.len() >= MAX_LANE_POINTS {
                break;
            }
        }
    }
    lanes
}

fn clear(spatial: &SpatialQuery, from: Vec3, to: Vec3) -> bool {
    let d = to - from;
    let len = d.length();
    Dir3::new(d).ok().is_none_or(|dir| spatial.cast_ray(from, dir, len, true, &collision::sight_filter()).is_none())
}

fn hold_spots(nav: &NavGraph, traffic: &[f32], lanes: &[LanePoint], spatial: &SpatialQuery) -> Vec<HoldSpot> {
    // Candidates: walkable points with some cover, thinned out.
    let mut rng = rand::rngs::StdRng::seed_from_u64(1);
    let mut candidates: Vec<u32> = (0..nav.nodes.len() as u32)
        .filter(|&i| {
            let n = &nav.nodes[i as usize];
            !n.crouch_only && n.exposure <= 0.75
        })
        .collect();
    let keep = 900usize;
    if candidates.len() > keep {
        for k in 0..keep {
            let j = rng.random_range(k..candidates.len());
            candidates.swap(k, j);
        }
        candidates.truncate(keep);
    }

    let mut scored: Vec<HoldSpot> = Vec::new();
    for &c in &candidates {
        let n = &nav.nodes[c as usize];
        let eye = n.pos + Vec3::Y * u(60.0);
        let mut score = 0.0;
        let mut watches = Vec::new();
        let mut facing = Vec3::ZERO;
        for (k, l) in lanes.iter().enumerate() {
            let d = l.pos.distance(n.pos);
            if !(u(250.0)..WATCH_RANGE).contains(&d) {
                continue;
            }
            if !clear(spatial, eye, l.pos + Vec3::Y * u(48.0)) {
                continue;
            }
            // Mid range is the sweet spot for an assault rifle.
            let range_weight = if d < u(400.0) { 0.6 } else if d < u(1600.0) { 1.0 } else { 0.7 };
            let w = l.traffic * range_weight;
            score += w;
            watches.push(k as u16);
            facing += (l.pos - n.pos).normalize_or_zero() * w;
        }
        if watches.is_empty() {
            continue;
        }
        // Don't camp in the middle of a lane; prefer cover.
        score *= 1.0 - 0.6 * traffic[c as usize];
        score *= 1.25 - 0.5 * n.exposure;
        let facing = Vec3::new(facing.x, 0.0, facing.z).normalize_or_zero();
        scored.push(HoldSpot { pos: n.pos, score, watches, facing, learned: 0.0, looks: Vec::new() });
    }
    scored.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut spots: Vec<HoldSpot> = Vec::new();
    for s in scored {
        if spots.iter().all(|t| t.pos.distance(s.pos) > SPOT_SPACING) {
            spots.push(s);
            if spots.len() >= MAX_SPOTS {
                break;
            }
        }
    }
    spots
}

/// Holding spots where real players held, watching what they watched.
fn learned_spots(nav: &NavGraph, lanes: &[LanePoint], spatial: &SpatialQuery, notes: &MapNotes) -> Vec<HoldSpot> {
    let most = notes.holds.first().map_or(1.0, |h| h.seconds);
    let mut spots: Vec<HoldSpot> = Vec::new();
    for h in &notes.holds {
        if h.seconds < MIN_LEARNED * most {
            break;
        }
        // Somewhere bots can walk to: not on top of a crate, say.
        let Some(n) = nav.nearest(h.pos) else { continue };
        let pos = nav.nodes[n as usize].pos;
        if pos.distance(h.pos) > u(40.0) || spots.iter().any(|s| s.pos.distance(pos) < u(60.0)) {
            continue;
        }
        let eye = pos + Vec3::Y * u(60.0);
        let mut looks: Vec<Look> = Vec::new();
        for &(dir, share) in &h.looks {
            // As far as the view goes that way (a wall, or the lanes beyond).
            let reach = Dir3::new(dir)
                .ok()
                .and_then(|d| spatial.cast_ray(eye, d, WATCH_RANGE, true, &collision::sight_filter()))
                .map_or(WATCH_RANGE, |hit| hit.distance);
            let point = eye + dir * reach.max(u(64.0)) - Vec3::Y * u(60.0);
            let flat = Vec3::new(dir.x, 0.0, dir.z).normalize_or_zero();
            let lanes: Vec<u16> = lanes
                .iter()
                .enumerate()
                .filter(|(_, l)| {
                    let to = l.pos - pos;
                    let d = to.length();
                    (u(150.0)..WATCH_RANGE).contains(&d)
                        && Vec3::new(to.x, 0.0, to.z).normalize_or_zero().dot(flat) > LOOK_CONE
                        && clear(spatial, eye, l.pos + Vec3::Y * u(48.0))
                })
                .map(|(k, _)| k as u16)
                .collect();
            looks.push(Look { point, share, lanes });
        }
        let Some(first) = looks.first() else { continue };
        let facing = Vec3::new(first.point.x - pos.x, 0.0, first.point.z - pos.z).normalize_or_zero();
        let mut watches: Vec<u16> = looks.iter().flat_map(|l| l.lanes.iter().copied()).collect();
        watches.sort_unstable();
        watches.dedup();
        // A place several players chose is a known spot; one player's
        // favourite, less so.
        let learned = (h.seconds / most).sqrt() * if h.players > 1 { 1.0 } else { 0.6 };
        spots.push(HoldSpot { pos, score: h.seconds, watches, facing, learned, looks });
    }
    spots
}

/// What a holding spot is like, worked out from the map alone (no demos):
/// the inputs to [`spot_score`], and to fitting it to where real players
/// held (`COD4RW_HOLDFIT`).
#[derive(Clone, Copy, Debug, Default)]
pub struct SpotFeatures {
    /// Traffic of the lane points seen, by range: close (250-400 units),
    /// mid (400-1600), long (1600-2600); and how many lane points.
    pub close: f32,
    pub mid: f32,
    pub long: f32,
    pub seen: f32,
    /// Traffic through the spot itself (0..1).
    pub traffic: f32,
    pub exposure: f32,
    /// Units to the nearest lane point.
    pub lane_dist: f32,
    /// Units above the lane points it sees (on average).
    pub height: f32,
    /// Units to the nearest spawn area.
    pub spawn_dist: f32,
    /// Ways out (nav links).
    pub links: f32,
}

fn features(nav: &NavGraph, node: u32, traffic: &[f32], lanes: &[LanePoint], areas: &[SpawnArea], spatial: &SpatialQuery) -> SpotFeatures {
    let n = &nav.nodes[node as usize];
    let eye = n.pos + Vec3::Y * u(60.0);
    let mut f = SpotFeatures {
        traffic: traffic[node as usize],
        exposure: n.exposure,
        links: n.links.len() as f32,
        lane_dist: lanes.iter().map(|l| l.pos.distance(n.pos)).fold(f32::MAX, f32::min) / crate::units::INCH,
        spawn_dist: areas.iter().map(|a| a.center.distance(n.pos)).fold(f32::MAX, f32::min) / crate::units::INCH,
        ..default()
    };
    let mut heights = 0.0;
    for l in lanes {
        let d = l.pos.distance(n.pos);
        if !(u(250.0)..WATCH_RANGE).contains(&d) || !clear(spatial, eye, l.pos + Vec3::Y * u(48.0)) {
            continue;
        }
        if d < u(400.0) {
            f.close += l.traffic;
        } else if d < u(1600.0) {
            f.mid += l.traffic;
        } else {
            f.long += l.traffic;
        }
        f.seen += 1.0;
        heights += n.pos.y - l.pos.y;
    }
    if f.seen > 0.0 {
        f.height = heights / f.seen / crate::units::INCH;
    }
    f
}

/// Debug aid: with `COD4RW_HOLDFIT=<file.csv>`, every candidate holding
/// spot on the map with its [`SpotFeatures`] (from the map alone) and how
/// long real players held within 150 units of it (from the demos), for
/// fitting [`spot_score`] (`tools/fit_holds.py`).
pub fn fit_dump(nav: &NavGraph, map: &MapInfo, spatial: &SpatialQuery, notes: Option<&MapNotes>, map_name: &str, path: &str) {
    use std::io::Write;
    let areas = spawn_areas(map);
    let traffic = traffic(nav, map);
    let lanes = lane_points(nav, &traffic);
    let mut rng = rand::rngs::StdRng::seed_from_u64(3);
    let mut candidates: Vec<u32> = (0..nav.nodes.len() as u32).filter(|&i| !nav.nodes[i as usize].crouch_only).collect();
    let keep = 2500usize;
    if candidates.len() > keep {
        for k in 0..keep {
            let j = rng.random_range(k..candidates.len());
            candidates.swap(k, j);
        }
        candidates.truncate(keep);
    }
    let new = !std::path::Path::new(path).exists();
    let Ok(mut out) = std::fs::OpenOptions::new().create(true).append(true).open(path) else { return };
    if new {
        writeln!(out, "map,x,y,z,close,mid,long,seen,traffic,exposure,lane_dist,height,spawn_dist,links,held").ok();
    }
    for c in candidates {
        let f = features(nav, c, &traffic, &lanes, &areas, spatial);
        let p = nav.nodes[c as usize].pos;
        let held: f32 = notes.map_or(0.0, |n| {
            n.holds
                .iter()
                .filter(|h| Vec2::new(h.pos.x - p.x, h.pos.z - p.z).length() < u(150.0) && (h.pos.y - p.y).abs() < u(60.0))
                .map(|h| h.seconds)
                .sum()
        });
        let q = crate::units::to_cod(p);
        writeln!(
            out,
            "{map_name},{:.0},{:.0},{:.0},{:.3},{:.3},{:.3},{},{:.3},{:.3},{:.0},{:.0},{:.0},{},{:.1}",
            q[0], q[1], q[2], f.close, f.mid, f.long, f.seen, f.traffic, f.exposure, f.lane_dist, f.height, f.spawn_dist, f.links, held
        )
        .ok();
    }
    info!("hold fit: {map_name} written to {path}");
}

/// Where the fight is coming from: the predicted enemy spawn (weighted
/// centre) and the centre of our own team.
#[derive(Clone, Copy, Debug)]
pub struct Front {
    pub enemy: Vec3,
    pub ours: Vec3,
}

/// What each team has learned this match: where enemies keep showing up.
/// Heat per nav node, fading with a half-life of about a minute.
#[derive(Resource, Default)]
pub struct TeamIntel {
    heat: [Vec<f32>; 2],
    pub decayed_at: f32,
}

fn team_index(team: crate::combat::Team) -> usize {
    match team {
        crate::combat::Team::Allies => 0,
        crate::combat::Team::Axis => 1,
    }
}

impl TeamIntel {
    /// Note enemy activity around `pos` for `team`.
    pub fn deposit(&mut self, nav: &NavGraph, team: crate::combat::Team, pos: Vec3, amount: f32) {
        let heat = &mut self.heat[team_index(team)];
        heat.resize(nav.nodes.len(), 0.0);
        let radius = u(300.0);
        for i in nav.within(pos, radius) {
            let d = nav.nodes[i as usize].pos.distance(pos);
            heat[i as usize] += amount * (1.0 - d / radius);
        }
    }

    /// Fade everything by `dt` seconds.
    pub fn decay(&mut self, dt: f32) {
        let k = (-dt / 85.0).exp(); // half-life ~ 1 minute
        for h in &mut self.heat {
            for v in h.iter_mut() {
                *v *= k;
            }
        }
    }

    /// 0..1: how much enemy activity `team` has seen around nav node `node`.
    pub fn heat(&self, team: crate::combat::Team, node: u32) -> f32 {
        let h = self.heat[team_index(team)].get(node as usize).copied().unwrap_or(0.0);
        h / (h + 1.0)
    }

    pub fn clear(&mut self) {
        self.heat = [Vec::new(), Vec::new()];
    }
}

impl TacticalMap {
    /// Where the enemies of a team standing at `my_team` are coming from.
    pub fn front(&self, map: &MapInfo, my_team: &[Vec3]) -> Option<Front> {
        let predicted = self.predicted_enemy_spawns(map, my_team);
        if predicted.is_empty() {
            return None;
        }
        let enemy = predicted.iter().map(|&(p, w)| p * w).sum::<Vec3>();
        let ours = my_team.iter().copied().sum::<Vec3>() / my_team.len() as f32;
        Some(Front { enemy, ours })
    }

    /// Left/right across the map for a team spawning around `from` and
    /// fighting towards `to`, scaled so the busy lanes span -1..1.
    pub fn lateral(&self, from: Vec3, to: Vec3) -> Option<Lateral> {
        let axis = Vec3::new(to.x - from.x, 0.0, to.z - from.z);
        if axis.length() < u(800.0) || self.lanes.len() < 8 {
            return None;
        }
        let across = Vec3::new(-axis.z, 0.0, axis.x).normalize();
        let mut xs: Vec<f32> = self.lanes.iter().map(|l| (l.pos - from).dot(across)).collect();
        xs.sort_by(f32::total_cmp);
        let (lo, hi) = (xs[xs.len() / 10], xs[xs.len() * 9 / 10]);
        Some(Lateral { origin: from, across, mid: (lo + hi) / 2.0, half: ((hi - lo) / 2.0).max(u(300.0)) })
    }

    /// How likely enemies come through lane point `k`: its traffic, how close
    /// it is to the corridor from their spawn to us, and where enemies have
    /// been seen this match.
    pub fn danger(&self, k: usize, front: Option<Front>, intel: &TeamIntel, team: crate::combat::Team) -> f32 {
        let l = &self.lanes[k];
        let corridor = front.map_or(1.0, |f| {
            let len = f.enemy.distance(f.ours);
            let detour = l.pos.distance(f.enemy) + l.pos.distance(f.ours) - len;
            0.35 + (-detour / (0.35 * len + u(300.0))).exp()
        });
        l.traffic * corridor + 1.5 * intel.heat(team, l.node)
    }

    /// The best holding spot for a bot at `feet`: watching the most danger,
    /// near enough, not claimed by a teammate, and (for `own_side`) not
    /// closer to the enemy spawn than to our team.
    #[allow(clippy::too_many_arguments)]
    pub fn choose_spot(
        &self,
        feet: Vec3,
        front: Option<Front>,
        intel: &TeamIntel,
        team: crate::combat::Team,
        taken: &[usize],
        own_side: bool,
        fit: impl Fn(Vec3) -> f32,
        rng: &mut impl Rng,
    ) -> Option<usize> {
        let mut scored: Vec<(usize, f32)> = self
            .spots
            .iter()
            .enumerate()
            .filter(|(i, s)| !taken.contains(i) && s.pos.distance(feet) < u(3000.0))
            .map(|(i, s)| {
                let watch: f32 = s.watches.iter().map(|&k| self.danger(k as usize, front, intel, team)).sum();
                let mut score = watch / (1.0 + s.pos.distance(feet) / u(1500.0)) * fit(s.pos);
                // Where real players held, as much as they did.
                if self.learned {
                    score *= if s.looks.is_empty() { GUESSED_WEIGHT } else { 0.5 + s.learned };
                }
                // Not right next to a spot a teammate is holding.
                if taken.iter().any(|&t| self.spots[t].pos.distance(s.pos) < u(600.0)) {
                    score *= 0.3;
                }
                if let (true, Some(f)) = (own_side, front) {
                    if s.pos.distance(f.enemy) < s.pos.distance(f.ours) {
                        score *= 0.5;
                    }
                }
                (i, score)
            })
            .collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        scored.truncate(3);
        if scored.is_empty() {
            return None;
        }
        Some(scored[rng.random_range(0..scored.len())].0)
    }

    /// Where to watch from spot `i`, most important first: the ways real
    /// players looked from there (the busier with enemies now, the more),
    /// else its lane points, most dangerous first.
    pub fn spot_angles(&self, i: usize, front: Option<Front>, intel: &TeamIntel, team: crate::combat::Team) -> Vec<Vec3> {
        let spot = &self.spots[i];
        if !spot.looks.is_empty() {
            let mut w: Vec<(Vec3, f32)> = spot
                .looks
                .iter()
                .map(|l| {
                    let danger: f32 = l.lanes.iter().map(|&k| self.danger(k as usize, front, intel, team)).sum();
                    (l.point, l.share * (0.5 + danger.min(3.0)))
                })
                .collect();
            w.sort_by(|a, b| b.1.total_cmp(&a.1));
            return w.into_iter().map(|(p, _)| p).collect();
        }
        let mut w: Vec<(Vec3, f32)> = spot
            .watches
            .iter()
            .map(|&k| (self.lanes[k as usize].pos, self.danger(k as usize, front, intel, team)))
            .collect();
        w.sort_by(|a, b| b.1.total_cmp(&a.1));
        w.into_iter().take(3).map(|(p, _)| p).collect()
    }

    /// A visible lane point worth checking while moving: dangerous and
    /// roughly ahead.
    #[allow(clippy::too_many_arguments)]
    pub fn check_point(
        &self,
        spatial: &SpatialQuery,
        eye: Vec3,
        feet: Vec3,
        heading: Vec3,
        front: Option<Front>,
        intel: &TeamIntel,
        team: crate::combat::Team,
        rng: &mut impl Rng,
    ) -> Option<Vec3> {
        let mut options: Vec<(Vec3, f32)> = Vec::new();
        for (k, l) in self.lanes_near(feet, u(1800.0)) {
            let to = l.pos - feet;
            if to.length() < u(150.0) {
                continue;
            }
            let ahead = to.normalize_or_zero().dot(heading);
            if ahead < -0.3 {
                continue;
            }
            let head = l.pos + Vec3::Y * u(60.0);
            if !clear(spatial, eye, head) {
                continue;
            }
            // Looked at chest high, as real players hold their crosshair
            // (their view sits ~4 degrees low on average).
            options.push((l.pos + Vec3::Y * CHECK_HEIGHT, self.danger(k, front, intel, team) * (0.4 + ahead.max(0.0))));
        }
        let total: f32 = options.iter().map(|o| o.1 * o.1).sum();
        if total <= 0.0 {
            return None;
        }
        let mut pick = rng.random::<f32>() * total;
        for (p, w) in &options {
            pick -= w * w;
            if pick <= 0.0 {
                return Some(*p);
            }
        }
        options.last().map(|o| o.0)
    }
}

/// Debug: write the analysis as CSV (`kind,x,y,z,value,dx,dz`) in CoD units.
pub fn dump(t: &TacticalMap, nav: &NavGraph, path: &str) {
    let mut out = String::from("kind,x,y,z,value,dx,dz\n");
    let c = |p: Vec3| p / u(1.0);
    for (i, n) in nav.nodes.iter().enumerate() {
        let p = c(n.pos);
        out.push_str(&format!("node,{:.0},{:.0},{:.0},{:.3},0,0\n", p.x, p.y, p.z, t.traffic[i]));
    }
    for l in &t.lanes {
        let p = c(l.pos);
        out.push_str(&format!("lane,{:.0},{:.0},{:.0},{:.3},0,0\n", p.x, p.y, p.z, l.traffic));
    }
    for s in &t.spots {
        let p = c(s.pos);
        let kind = if s.looks.is_empty() { "spot" } else { "learned" };
        out.push_str(&format!("{kind},{:.0},{:.0},{:.0},{:.3},{:.3},{:.3}\n", p.x, p.y, p.z, s.score, s.facing.x, s.facing.z));
    }
    for a in &t.areas {
        let p = c(a.center);
        out.push_str(&format!("area,{:.0},{:.0},{:.0},{},0,0\n", p.x, p.y, p.z, a.spawns.len()));
    }
    std::fs::write(path, out).ok();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::SpawnPoint;

    #[test]
    fn enemies_are_predicted_to_spawn_far_from_us() {
        let spawns = (0..11)
            .map(|i| SpawnPoint { pos: Vec3::new(i as f32 * u(300.0), 0.0, 0.0), yaw: 0.0, kind: SpawnKind::Tdm })
            .collect();
        let map = MapInfo { spawns };
        let t = TacticalMap { traffic: Vec::new(), lanes: Vec::new(), spots: Vec::new(), areas: Vec::new(), learned: false };
        // Our team stands at the x = 0 end.
        let predicted = t.predicted_enemy_spawns(&map, &[Vec3::ZERO, Vec3::new(u(200.0), 0.0, 0.0)]);
        let total: f32 = predicted.iter().map(|p| p.1).sum();
        assert!((total - 1.0).abs() < 0.05);
        let mean_x = predicted.iter().map(|p| p.0.x * p.1).sum::<f32>() / total;
        assert!(mean_x > u(2400.0), "expected far end, got {}", mean_x / u(1.0));
    }
}

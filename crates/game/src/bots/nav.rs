//! Navigation graph for bots.
//!
//! CoD4 MP maps ship no path nodes, so the graph is built from the map's
//! collision once the match starts. Walkable points on a [`CELL`] grid are
//! flood-filled from the spawn points, so only floor a player can reach is
//! included, across stairs, ledges and upper floors. Points link where a
//! player hull can walk, step, jump up or drop down between them, or mantle
//! (climb what the level designers marked climbable), and bots plan routes
//! over the links with A*.

use crate::collision;
use crate::movement::{HULL_RADIUS, STEP_HEIGHT};
use crate::units::u;
use avian3d::prelude::*;
use bevy::prelude::*;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, VecDeque};

/// Grid spacing of nav points.
pub const CELL: f32 = u(32.0);
/// Highest ledge a link may climb; above a step it needs a jump.
const MAX_CLIMB: f32 = u(36.0);
/// Deepest drop a link may fall (fall damage starts at 128 units).
const MAX_DROP: f32 = u(110.0);
/// Steepest walkable floor (`MIN_WALK_NORMAL`).
const MIN_FLOOR_NORMAL: f32 = 0.7;
/// Two points in one grid column closer than this are the same floor.
const SAME_FLOOR: f32 = u(24.0);
const STAND_HEIGHT: f32 = u(70.0);
const CROUCH_HEIGHT: f32 = u(50.0);
const MAX_NODES: usize = 200_000;
/// Extra cost of a mantle: it's slower than walking, but players take one
/// when it saves going round.
const MANTLE_COST: f32 = u(150.0);

pub struct NavNode {
    /// Point on the floor.
    pub pos: Vec3,
    pub links: Vec<NavLink>,
    /// Only a crouching player fits here.
    pub crouch_only: bool,
    /// 0..1: how open the spot is (share of directions with no wall nearby
    /// at chest height). Routes prefer covered ground.
    pub exposure: f32,
    /// Room around the spot: 0 only just fits a player, 1 a little more, 2
    /// plenty. Routes keep off walls and boxes, where players snag.
    pub clearance: u8,
}

#[derive(Clone, Copy, Debug)]
pub struct NavLink {
    pub to: u32,
    pub cost: f32,
    /// Climbing more than a step: jump to cross.
    pub jump: bool,
    /// Mantling: face the way and hold jump at the start.
    pub mantle: bool,
}

#[derive(Resource, Default)]
pub struct NavGraph {
    pub nodes: Vec<NavNode>,
    columns: HashMap<(i32, i32), Vec<u32>>,
}

fn column(p: Vec3) -> (i32, i32) {
    ((p.x / CELL).round() as i32, (p.z / CELL).round() as i32)
}

struct Probe<'a, 'w, 's> {
    spatial: &'a SpatialQuery<'w, 's>,
    filter: SpatialQueryFilter,
}

impl Probe<'_, '_, '_> {
    /// Floor under (x, z) reachable from height `from_y`: within a climb
    /// above or a drop below.
    fn floor(&self, x: f32, z: f32, from_y: f32) -> Option<f32> {
        // Start as high as a climb allows, lower if that's inside a ceiling.
        for start in [from_y + MAX_CLIMB + u(2.0), from_y + STEP_HEIGHT + u(2.0), from_y + u(6.0)] {
            let origin = Vec3::new(x, start, z);
            let max = start - (from_y - MAX_DROP);
            let hit = self.spatial.cast_ray(origin, Dir3::NEG_Y, max, true, &self.filter)?;
            if hit.distance <= u(0.05) {
                continue;
            }
            if hit.normal.y < MIN_FLOOR_NORMAL {
                return None;
            }
            return Some(start - hit.distance);
        }
        None
    }

    /// Whether a player hull of `height` fits standing on `floor`.
    fn fits(&self, floor: Vec3, height: f32) -> bool {
        self.fits_wide(floor, height, HULL_RADIUS - u(1.0))
    }

    /// Whether a hull of `radius` and `height` fits standing on `floor`:
    /// room above a step's height (anything lower the player steps onto;
    /// on stairs, the hull reaches over the next step).
    fn fits_wide(&self, floor: Vec3, height: f32, radius: f32) -> bool {
        let room = height - STEP_HEIGHT;
        let length = (room - 2.0 * radius - u(2.0)).max(0.0);
        let shape = Collider::capsule(radius, length);
        let center = floor + Vec3::Y * (STEP_HEIGHT + u(1.5) + room * 0.5);
        self.spatial.shape_intersections(&shape, center, Quat::IDENTITY, &self.filter).is_empty()
    }

    /// Whether a crouching hull can move horizontally from `a` to `b` at a
    /// step above the higher floor.
    fn passage(&self, a: Vec3, b: Vec3) -> bool {
        let base = a.y.max(b.y) + STEP_HEIGHT + u(1.0);
        let height = CROUCH_HEIGHT - STEP_HEIGHT;
        let radius = HULL_RADIUS - u(1.0);
        let shape = Collider::capsule(radius, (height - 2.0 * radius).max(0.0));
        let center_y = base + height * 0.5;
        let from = Vec3::new(a.x, center_y, a.z);
        let delta = Vec3::new(b.x - a.x, 0.0, b.z - a.z);
        let Ok(dir) = Dir3::new(delta) else { return true };
        let config = ShapeCastConfig::from_max_distance(delta.length());
        self.spatial.cast_shape(&shape, from, Quat::IDENTITY, dir, &config, &self.filter).is_none()
    }
}

impl NavGraph {
    /// Flood-fill the walkable floor reachable from `seeds`, and over the
    /// mantles (`mantles` their boxes, `over` the `mantle_over` ones).
    pub fn build(spatial: &SpatialQuery, seeds: &[Vec3], mantles: &[(Vec3, Vec3)], over: &[Entity]) -> NavGraph {
        let probe = Probe { spatial, filter: collision::movement_filter() };
        let mut graph = NavGraph::default();
        let mut queue = VecDeque::new();

        for &seed in seeds {
            let (cx, cz) = column(seed);
            let (x, z) = (cx as f32 * CELL, cz as f32 * CELL);
            let Some(y) = probe.floor(x, z, seed.y) else { continue };
            let pos = Vec3::new(x, y, z);
            if graph.find_in_column((cx, cz), y).is_some() {
                continue;
            }
            if let Some(i) = graph.add(&probe, pos) {
                queue.push_back(i);
            }
        }

        graph.fill(&probe, &mut queue);
        // Mantles, and the floor they lead to (which may have more).
        for _ in 0..3 {
            if graph.add_mantles(&probe, &mut queue, mantles, over) == 0 {
                break;
            }
            graph.fill(&probe, &mut queue);
        }

        // Exposure: rays in 8 directions at chest height.
        let sight = collision::sight_filter();
        for n in &mut graph.nodes {
            let chest = n.pos + Vec3::Y * u(48.0);
            let open = (0..8)
                .filter(|k| {
                    let a = *k as f32 * std::f32::consts::FRAC_PI_4;
                    let dir = Dir3::new(Vec3::new(a.cos(), 0.0, a.sin())).expect("unit");
                    spatial.cast_ray(chest, dir, u(160.0), true, &sight).is_none()
                })
                .count();
            n.exposure = open as f32 / 8.0;
        }
        graph
    }

    /// Link mantles: from points up against a mantle volume to where the
    /// climb ends, adding that point (to `queue`) if it's new. Returns the
    /// links added.
    fn add_mantles(&mut self, probe: &Probe, queue: &mut VecDeque<u32>, mantles: &[(Vec3, Vec3)], over: &[Entity]) -> usize {
        let mut added = 0;
        for &(lo, hi) in mantles {
            let centre = Vec3::new((lo.x + hi.x) * 0.5, lo.y, (lo.z + hi.z) * 0.5);
            let reach = Vec2::new(hi.x - lo.x, hi.z - lo.z).length() * 0.5 + u(48.0);
            for i in self.within(centre, reach) {
                let a = self.nodes[i as usize].pos;
                // Something to climb: from the floor at its foot.
                if a.y > hi.y - STEP_HEIGHT || a.y < lo.y - u(40.0) {
                    continue;
                }
                let near = Vec3::new(a.x.clamp(lo.x, hi.x), a.y, a.z.clamp(lo.z, hi.z));
                let to = near - a;
                let d = to.length();
                if !(u(1.0)..=u(48.0)).contains(&d) {
                    continue;
                }
                let dir = to / d;
                // Up against it, as a player would be.
                let feet = a + dir * (d - HULL_RADIUS - u(1.0)).max(0.0);
                let yaw = (-dir.x).atan2(-dir.z);
                let Some((end, _)) = crate::movement::mantle_landing(probe.spatial, feet, yaw, over) else { continue };
                // The grid point there, on the same floor.
                let col = column(end);
                let (x, z) = (col.0 as f32 * CELL, col.1 as f32 * CELL);
                let Some(y) = probe.floor(x, z, end.y) else { continue };
                if (y - end.y).abs() > STEP_HEIGHT {
                    continue;
                }
                let j = match self.find_in_column(col, y) {
                    Some(j) => j,
                    None => {
                        if self.nodes.len() >= MAX_NODES {
                            continue;
                        }
                        let Some(j) = self.add(probe, Vec3::new(x, y, z)) else { continue };
                        queue.push_back(j);
                        j
                    }
                };
                if j == i || self.link(i, j).is_some() {
                    continue;
                }
                let cost = a.distance(end) + MANTLE_COST;
                self.nodes[i as usize].links.push(NavLink { to: j, cost, jump: false, mantle: true });
                added += 1;
            }
        }
        added
    }

    /// Flood-fill on from the points in `queue`.
    fn fill(&mut self, probe: &Probe, queue: &mut VecDeque<u32>) {
        let graph = self;
        const DIRS: [(i32, i32); 8] = [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)];
        while let Some(i) = queue.pop_front() {
            let a = graph.nodes[i as usize].pos;
            let (cx, cz) = column(a);
            for (dx, dz) in DIRS {
                let col = (cx + dx, cz + dz);
                let (x, z) = (col.0 as f32 * CELL, col.1 as f32 * CELL);
                let Some(y) = probe.floor(x, z, a.y) else { continue };
                let b = Vec3::new(x, y, z);
                if !probe.passage(a, b) {
                    continue;
                }
                let j = match graph.find_in_column(col, y) {
                    Some(j) => j,
                    None => {
                        if graph.nodes.len() >= MAX_NODES {
                            continue;
                        }
                        let Some(j) = graph.add(probe, b) else { continue };
                        queue.push_back(j);
                        j
                    }
                };
                let climb = y - a.y;
                let jump = climb > STEP_HEIGHT;
                let mut cost = a.distance(b);
                if graph.nodes[j as usize].crouch_only {
                    cost *= 2.0;
                }
                cost += match graph.nodes[j as usize].clearance {
                    0 => u(14.0),
                    1 => u(5.0),
                    _ => 0.0,
                };
                // Real players walk round rather than jump up (1.5 jumps a
                // minute in the demos): only when it saves a long way.
                if jump {
                    cost += u(400.0);
                }
                graph.nodes[i as usize].links.push(NavLink { to: j, cost, jump, mantle: false });
            }
        }
    }

    fn add(&mut self, probe: &Probe, pos: Vec3) -> Option<u32> {
        let crouch_only = if probe.fits(pos, STAND_HEIGHT) {
            false
        } else if probe.fits(pos, CROUCH_HEIGHT) {
            true
        } else {
            return None;
        };
        let height = if crouch_only { CROUCH_HEIGHT } else { STAND_HEIGHT };
        let clearance = if !probe.fits_wide(pos, height, u(22.0)) {
            0
        } else if !probe.fits_wide(pos, height, u(30.0)) {
            1
        } else {
            2
        };
        let i = self.nodes.len() as u32;
        self.nodes.push(NavNode { pos, links: Vec::new(), crouch_only, exposure: 0.0, clearance });
        self.columns.entry(column(pos)).or_default().push(i);
        Some(i)
    }

    fn find_in_column(&self, col: (i32, i32), y: f32) -> Option<u32> {
        self.columns.get(&col)?.iter().copied().find(|&i| (self.nodes[i as usize].pos.y - y).abs() < SAME_FLOOR)
    }

    /// The node nearest a position (feet), preferring the same floor.
    pub fn nearest(&self, p: Vec3) -> Option<u32> {
        let (cx, cz) = column(p);
        let mut best: Option<(u32, f32)> = None;
        for r in 0..=3i32 {
            for dx in -r..=r {
                for dz in -r..=r {
                    if dx.abs() != r && dz.abs() != r {
                        continue;
                    }
                    let Some(ids) = self.columns.get(&(cx + dx, cz + dz)) else { continue };
                    for &i in ids {
                        let n = self.nodes[i as usize].pos;
                        let dy = n.y - p.y;
                        if !(-u(72.0)..=u(40.0)).contains(&dy) {
                            continue;
                        }
                        let d = Vec2::new(n.x - p.x, n.z - p.z).length() + dy.abs() * 2.0;
                        if best.is_none_or(|(_, bd)| d < bd) {
                            best = Some((i, d));
                        }
                    }
                }
            }
            if best.is_some() {
                break;
            }
        }
        best.map(|(i, _)| i)
    }

    /// Debug: how the grid point under `p` links to each neighbouring cell,
    /// or why it doesn't (no floor, no passage, no room).
    pub fn explain(&self, spatial: &SpatialQuery, p: Vec3) -> Vec<String> {
        let probe = Probe { spatial, filter: collision::movement_filter() };
        let (cx, cz) = column(p);
        let Some(y) = probe.floor(cx as f32 * CELL, cz as f32 * CELL, p.y) else { return vec!["no floor here".into()] };
        let a = Vec3::new(cx as f32 * CELL, y, cz as f32 * CELL);
        let i = self.find_in_column((cx, cz), y);
        let cod = |v: Vec3| {
            let c = crate::units::to_cod(v);
            format!("({:.0} {:.0} {:.0})", c[0], c[1], c[2])
        };
        let mut out = vec![format!("point {} node {:?} stands {} crouches {}", cod(a), i, probe.fits(a, STAND_HEIGHT), probe.fits(a, CROUCH_HEIGHT))];
        const DIRS: [(i32, i32); 8] = [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)];
        for (dx, dz) in DIRS {
            let col = (cx + dx, cz + dz);
            let (x, z) = (col.0 as f32 * CELL, col.1 as f32 * CELL);
            let line = match probe.floor(x, z, a.y) {
                None => format!("  -> ({dx},{dz}): no floor in reach"),
                Some(fy) => {
                    let b = Vec3::new(x, fy, z);
                    let j = self.find_in_column(col, fy);
                    let linked = i.zip(j).is_some_and(|(i, j)| self.link(i, j).is_some());
                    format!(
                        "  -> ({dx},{dz}) {}: climb {:.0}, passage {}, stands {} crouches {}, node {:?}, linked {linked}",
                        cod(b),
                        (fy - a.y) / u(1.0),
                        probe.passage(a, b),
                        probe.fits(b, STAND_HEIGHT),
                        probe.fits(b, CROUCH_HEIGHT),
                        j
                    )
                }
            };
            out.push(line);
        }
        out
    }

    /// Every point reachable from `from` along the links (some are one
    /// way: drops).
    pub fn reachable(&self, from: u32) -> Vec<bool> {
        let mut seen = vec![false; self.nodes.len()];
        let mut stack = vec![from];
        seen[from as usize] = true;
        while let Some(i) = stack.pop() {
            for l in &self.nodes[i as usize].links {
                if !std::mem::replace(&mut seen[l.to as usize], true) {
                    stack.push(l.to);
                }
            }
        }
        seen
    }

    /// A* from `from` to `to`. `extra` adds a cost for entering a node (for
    /// example, being visible to an enemy). Gives up after `budget` node
    /// expansions.
    pub fn path(&self, from: u32, to: u32, budget: usize, extra: impl Fn(u32) -> f32) -> Option<Vec<u32>> {
        if from == to {
            return Some(vec![to]);
        }
        let n = self.nodes.len();
        let goal = self.nodes[to as usize].pos;
        let h = |i: u32| self.nodes[i as usize].pos.distance(goal);
        // Flat arrays (a path can touch thousands of points), and each
        // point's extra cost worked out once.
        let mut g = vec![f32::INFINITY; n];
        let mut came = vec![u32::MAX; n];
        let mut closed = vec![false; n];
        let mut extras = vec![f32::NAN; n];
        let mut open = BinaryHeap::new();
        g[from as usize] = 0.0;
        open.push(Reverse((h(from).to_bits(), from)));
        let mut expanded = 0;
        while let Some(Reverse((_, i))) = open.pop() {
            if i == to {
                let mut path = vec![to];
                let mut cur = to;
                while came[cur as usize] != u32::MAX {
                    cur = came[cur as usize];
                    path.push(cur);
                }
                path.reverse();
                return Some(path);
            }
            // A stale entry for a point already done.
            if std::mem::replace(&mut closed[i as usize], true) {
                continue;
            }
            expanded += 1;
            if expanded > budget {
                return None;
            }
            let gi = g[i as usize];
            for link in &self.nodes[i as usize].links {
                let j = link.to as usize;
                if closed[j] {
                    continue;
                }
                if extras[j].is_nan() {
                    extras[j] = extra(link.to);
                }
                let cost = gi + link.cost + extras[j];
                if cost < g[j] {
                    g[j] = cost;
                    came[j] = i;
                    // Non-negative f32 bit patterns order like the floats.
                    open.push(Reverse(((cost + h(link.to)).to_bits(), link.to)));
                }
            }
        }
        None
    }

    /// The link from `a` to `b`, if any.
    pub fn link(&self, a: u32, b: u32) -> Option<NavLink> {
        self.nodes.get(a as usize)?.links.iter().copied().find(|l| l.to == b)
    }

    /// Nodes within `radius` of `p` (any floor within a storey).
    pub fn within(&self, p: Vec3, radius: f32) -> Vec<u32> {
        let r = (radius / CELL).ceil() as i32;
        let (cx, cz) = column(p);
        let mut out = Vec::new();
        for dx in -r..=r {
            for dz in -r..=r {
                let Some(ids) = self.columns.get(&(cx + dx, cz + dz)) else { continue };
                for &i in ids {
                    let n = self.nodes[i as usize].pos;
                    if (n.y - p.y).abs() < u(120.0) && n.distance(p) <= radius {
                        out.push(i);
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(w: u32, h: u32) -> NavGraph {
        let mut g = NavGraph::default();
        for z in 0..h {
            for x in 0..w {
                let pos = Vec3::new(x as f32 * CELL, 0.0, z as f32 * CELL);
                g.columns.entry(column(pos)).or_default().push(g.nodes.len() as u32);
                g.nodes.push(NavNode { pos, links: Vec::new(), crouch_only: false, exposure: 0.0, clearance: 2 });
            }
        }
        for z in 0..h as i32 {
            for x in 0..w as i32 {
                for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                    let (nx, nz) = (x + dx, z + dz);
                    if nx < 0 || nz < 0 || nx >= w as i32 || nz >= h as i32 {
                        continue;
                    }
                    let (a, b) = ((z * w as i32 + x) as u32, (nz * w as i32 + nx) as u32);
                    g.nodes[a as usize].links.push(NavLink { to: b, cost: CELL, jump: false, mantle: false });
                }
            }
        }
        g
    }

    #[test]
    fn astar_finds_shortest_and_respects_extra_cost() {
        let g = grid(10, 10);
        let p = g.path(0, 99, 10_000, |_| 0.0).unwrap();
        assert_eq!(p.len(), 19, "manhattan path on a 4-connected grid");
        // Make the middle column expensive: the path should route around x=5
        // except where it must cross.
        let avoid = |i: u32| if i % 10 == 5 && i / 10 < 9 { 1000.0 } else { 0.0 };
        let p = g.path(0, 9, 10_000, avoid).unwrap();
        assert!(p.iter().filter(|&&i| i % 10 == 5).all(|&i| i / 10 == 9));
        assert_eq!(g.nearest(Vec3::new(CELL * 3.2, 0.0, CELL * 4.9)), Some(53));
    }
}

//! What the map's triangle collision (terrain, curved patches: ground,
//! corrugated roofs, pipes, rocks) is made of. [`crate::collision`] makes it
//! one trimesh without surface types, so bullet impacts, their sounds and
//! footsteps on it were all one kind. The clip map's AABB tree knows: each
//! leaf box holds a run of triangles of one material. A point's surface is
//! that of the smallest leaf box around it, found through a coarse grid.

use crate::collision::SURFACE_NAMES;
use crate::content::Content;
use crate::state::{GameState, Setup};
use crate::units;
use bevy::prelude::*;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

pub struct TerrainPlugin;

impl Plugin for TerrainPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(GameState::InGame), build.in_set(Setup::Spawn));
    }
}

/// Grid cells (CoD units).
const CELL: f32 = 256.0;
/// A point this far outside a leaf box (CoD units) still counts as in it.
const SLACK: f32 = 2.0;

static TERRAIN: RwLock<Option<Arc<Surfaces>>> = RwLock::new(None);

struct Surfaces {
    /// Leaf boxes (CoD space): min, max, surface type.
    leaves: Vec<([f32; 3], [f32; 3], u8)>,
    grid: HashMap<[i32; 3], Vec<u32>>,
}

fn cell(p: [f32; 3]) -> [i32; 3] {
    p.map(|v| (v / CELL).floor() as i32)
}

impl Surfaces {
    fn new(clip: &iw3::zone::ClipMap) -> Surfaces {
        let mut leaves = Vec::new();
        let mut grid: HashMap<[i32; 3], Vec<u32>> = HashMap::new();
        for t in clip.aabb_trees.iter().filter(|t| t.child_count == 0) {
            let surface = clip.materials.get(t.material_index as usize).map_or(0, |m| ((m.surface_flags >> 20) & 31) as u8);
            let lo: [f32; 3] = std::array::from_fn(|i| t.origin[i] - t.half_size[i]);
            let hi: [f32; 3] = std::array::from_fn(|i| t.origin[i] + t.half_size[i]);
            let (a, b) = (cell(lo), cell(hi));
            // Huge leaves would fill the grid; they're rare and coarse anyway.
            if (0..3).map(|i| (b[i] - a[i] + 1) as i64).product::<i64>() > 512 {
                continue;
            }
            let id = leaves.len() as u32;
            leaves.push((lo, hi, surface));
            for x in a[0]..=b[0] {
                for y in a[1]..=b[1] {
                    for z in a[2]..=b[2] {
                        grid.entry([x, y, z]).or_default().push(id);
                    }
                }
            }
        }
        Surfaces { leaves, grid }
    }

    /// The surface type at `p` (CoD space).
    fn at(&self, p: [f32; 3]) -> Option<u8> {
        let ids = self.grid.get(&cell(p))?;
        ids.iter()
            .map(|&i| &self.leaves[i as usize])
            .filter(|(lo, hi, _)| (0..3).all(|i| p[i] >= lo[i] - SLACK && p[i] <= hi[i] + SLACK))
            .min_by(|a, b| volume(a).total_cmp(&volume(b)))
            .map(|l| l.2)
    }
}

fn volume((lo, hi, _): &([f32; 3], [f32; 3], u8)) -> f32 {
    (0..3).map(|i| (hi[i] - lo[i]).max(1.0)).product()
}

fn build(content: Res<Content>) {
    let surfaces = content.map().clip_map().map(|c| Arc::new(Surfaces::new(c)));
    if let Some(s) = &surfaces {
        info!("terrain: {} surface boxes", s.leaves.len());
    }
    if let Ok(mut t) = TERRAIN.write() {
        *t = surfaces;
    }
}

/// The surface name (`"grass"`, `"metal"`) of the triangle collision at
/// world point `p` (Bevy space), if it's there.
pub fn surface_at(p: Vec3) -> Option<&'static str> {
    let t = TERRAIN.read().ok()?.clone()?;
    let s = t.at(units::to_cod(p))?;
    SURFACE_NAMES.get(s as usize).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smallest_box_wins() {
        let s = Surfaces {
            leaves: vec![([0.0; 3], [200.0; 3], 6), ([10.0; 3], [20.0; 3], 13)],
            grid: HashMap::from([([0, 0, 0], vec![0, 1])]),
        };
        assert_eq!(s.at([15.0, 15.0, 15.0]), Some(13));
        assert_eq!(s.at([100.0, 100.0, 100.0]), Some(6));
        assert_eq!(s.at([21.0, 15.0, 15.0]), Some(13));
        assert_eq!(s.at([300.0, 15.0, 15.0]), None);
    }
}

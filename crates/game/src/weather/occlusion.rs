//! How high rain gets, seen from above, around the camera: for each cell of
//! a grid (`CELLS`² of `CELL` metres), the height of the first thing a drop
//! falling there would hit: the ground, a deck, a container's top, a roof.
//! Rain draws only above it (none indoors or under cover) and splashes on
//! it; the camera is out in the rain when its own cell's is below it.
//!
//! The grid stays put in the world and wraps round as the camera moves
//! (cell `(x, z)` lives at `(x mod CELLS, z mod CELLS)`), so only the cells
//! coming into range need finding: a few hundred downward rays a frame,
//! nearest first. The heights go to the GPU as a small float texture.

use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

/// Grid size (cells a side) and cell size (metres): 64 m across.
pub const CELLS: usize = 128;
pub const CELL: f32 = 0.5;
/// Rays cast a frame.
const RAYS_PER_FRAME: usize = 384;
/// Rays start this far above the camera.
const ABOVE: f32 = 60.0;
/// A cell not yet found (the shader treats it as open).
pub const UNKNOWN: f32 = -1.0e6;

pub(super) fn build(app: &mut App) {
    app.add_systems(Update, update.run_if(crate::state::in_game).run_if(crate::atmos::climate::on));
}

/// The grid and its texture.
#[derive(Resource)]
pub struct RainMap {
    /// Per slot: the world cell it holds, and the height there.
    cells: Vec<(IVec2, f32)>,
    /// Slot offsets from the camera's cell, nearest first.
    order: Vec<IVec2>,
    pub image: Handle<Image>,
    dirty: bool,
}

impl RainMap {
    fn new(images: &mut Assets<Image>) -> Self {
        let half = CELLS as i32 / 2;
        let mut order: Vec<IVec2> = (-half..half).flat_map(|z| (-half..half).map(move |x| IVec2::new(x, z))).collect();
        order.sort_by_key(|o| o.length_squared());
        let mut image = Image::new(
            Extent3d { width: CELLS as u32, height: CELLS as u32, depth_or_array_layers: 1 },
            TextureDimension::D2,
            vec![0u8; CELLS * CELLS * 4],
            TextureFormat::R32Float,
            RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
        );
        image.sampler = ImageSampler::nearest();
        let unknown = UNKNOWN.to_le_bytes();
        if let Some(data) = image.data.as_mut() {
            for px in data.chunks_mut(4) {
                px.copy_from_slice(&unknown);
            }
        }
        RainMap { cells: vec![(IVec2::new(i32::MIN, i32::MIN), UNKNOWN); CELLS * CELLS], order, image: images.add(image), dirty: false }
    }

    /// The world cell a point is in.
    pub fn cell_of(p: Vec3) -> IVec2 {
        IVec2::new((p.x / CELL).floor() as i32, (p.z / CELL).floor() as i32)
    }

    fn slot(c: IVec2) -> usize {
        let n = CELLS as i32;
        (c.y.rem_euclid(n) * n + c.x.rem_euclid(n)) as usize
    }

    /// The height rain reaches at `p` (world), if known.
    pub fn height(&self, p: Vec3) -> Option<f32> {
        let c = Self::cell_of(p);
        let (at, h) = self.cells[Self::slot(c)];
        (at == c && h > UNKNOWN).then_some(h)
    }

    /// Is `p` out in the rain (nothing above it)?
    pub fn exposed(&self, p: Vec3) -> Option<bool> {
        self.height(p).map(|h| h < p.y + 0.3)
    }

    /// The share of cells within `radius` metres of `p` that rain reaches
    /// below `p` (how open it is round about).
    pub fn openness(&self, p: Vec3, radius: f32) -> f32 {
        let r = (radius / CELL) as i32;
        let c = Self::cell_of(p);
        let (mut open, mut all) = (0, 0);
        for z in -r..=r {
            for x in -r..=r {
                if x * x + z * z > r * r {
                    continue;
                }
                let at = c + IVec2::new(x, z);
                let (held, h) = self.cells[Self::slot(at)];
                if held == at && h > UNKNOWN {
                    all += 1;
                    open += (h < p.y + 0.3) as u32;
                }
            }
        }
        if all == 0 { 1.0 } else { open as f32 / all as f32 }
    }
}

fn update(
    mut commands: Commands,
    map: Option<ResMut<RainMap>>,
    mut images: ResMut<Assets<Image>>,
    camera: Query<&GlobalTransform, With<crate::player::MainCamera>>,
    spatial: avian3d::prelude::SpatialQuery,
) {
    let Some(mut map) = map else {
        commands.insert_resource(RainMap::new(&mut images));
        return;
    };
    let Some(eye) = camera.iter().next().map(|c| c.translation()) else { return };
    let centre = RainMap::cell_of(eye);
    // Rain is stopped by anything solid, the clip that keeps players out
    // included (it caps containers and the ship's superstructure).
    let filter = crate::collision::movement_filter();
    let mut cast = 0;
    let order = std::mem::take(&mut map.order);
    for o in &order {
        if cast >= RAYS_PER_FRAME {
            break;
        }
        let c = centre + *o;
        let slot = RainMap::slot(c);
        if map.cells[slot].0 == c {
            continue;
        }
        cast += 1;
        let top = Vec3::new((c.x as f32 + 0.5) * CELL, eye.y + ABOVE, (c.y as f32 + 0.5) * CELL);
        let h = spatial
            .cast_ray(top, Dir3::NEG_Y, ABOVE * 2.0 + 100.0, true, &filter)
            .map_or(eye.y - ABOVE - 100.0, |hit| top.y - hit.distance);
        map.cells[slot] = (c, h);
        map.dirty = true;
    }
    map.order = order;
    if map.dirty {
        map.dirty = false;
        let handle = map.image.clone();
        if let Some(mut image) = images.get_mut(&handle)
            && let Some(data) = image.data.as_mut()
        {
            for (i, (_, h)) in map.cells.iter().enumerate() {
                data[i * 4..i * 4 + 4].copy_from_slice(&h.to_le_bytes());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_wrap_and_cells_floor() {
        assert_eq!(RainMap::cell_of(Vec3::new(-0.1, 0.0, 0.6)), IVec2::new(-1, 1));
        assert_eq!(RainMap::slot(IVec2::new(0, 0)), RainMap::slot(IVec2::new(CELLS as i32, -(CELLS as i32))));
        assert_ne!(RainMap::slot(IVec2::new(1, 0)), RainMap::slot(IVec2::new(0, 1)));
    }
}

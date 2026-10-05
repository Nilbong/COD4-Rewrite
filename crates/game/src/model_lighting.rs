//! Lighting for models (players, guns, props) from the map's light grid.
//!
//! CoD4 lights models from a grid of baked light samples (see
//! [`iw3::zone::LightGrid`]): the light arriving from 56 directions, every 32
//! units across the playable space (64 vertically). Here that becomes a Bevy
//! irradiance volume of ambient cubes, so models darken in shadowed rooms and
//! pick up bounce light from what's around them, matching the lightmapped
//! world. The grid leaves out the sun's direct light; the real-time sun adds
//! it. The viewmodel can't see the world's shadow maps, so its own sun is
//! dimmed by how much of the sun is visible from the player's eye.
//!
//! Like Black Ops' maps (`r_lightGridIntensity`, `r_lightGridContrast` in
//! their art scripts), the grid's light is brightened and its sides pushed
//! apart, so models read with more shape; `COD4RW_NOVISION` leaves that out.

use crate::lightmaps::LIGHTMAP_EXPOSURE;
use crate::units;
use avian3d::prelude::*;
use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::RenderLayers;
use bevy::image::ImageSampler;
use bevy::light::IrradianceVolume;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use iw3::zone::{GfxWorld, LightGrid};
use std::collections::VecDeque;

pub struct ModelLightingPlugin;

impl Plugin for ModelLightingPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, viewmodel_sun.run_if(crate::state::in_game));
    }
}

/// The light grid's points are cropped to where nearly all of them are (a
/// few strays far outside the playable area would blow up the volume) plus
/// this many cells of margin...
const CROP_MARGIN: u32 = 2;

/// ...and cropped harder, dropping a few more outlying points, while the
/// volume would have more voxels than this (24 bytes each).
const MAX_VOXELS: usize = 1_500_000;

/// Black Ops' light grid tweaks: its maps use 1.1-1.5 and 0.2-0.4; CoD4's
/// set none, so these are Black Ops' usual values. Its contrast is off: on
/// CoD4's grids (lit from one side much more than the other) pushing the
/// sides apart left trees', guns' and players' shaded sides black.
const LIGHT_GRID_INTENSITY: f32 = 1.3;
const LIGHT_GRID_CONTRAST: f32 = 0.0;

/// `r_lightGridContrast`: each side of an ambient cube pushed away from the
/// cube's mean (never below black).
fn grid_contrast(cube: [[f32; 3]; 6], contrast: f32) -> [[f32; 3]; 6] {
    let mean: [f32; 3] = std::array::from_fn(|c| cube.iter().map(|f| f[c]).sum::<f32>() / 6.0);
    cube.map(|f| std::array::from_fn(|c| (mean[c] + (f[c] - mean[c]) * (1.0 + contrast)).max(0.0)))
}

/// Build an irradiance volume from `world`'s light grid. Returns false if the
/// map has no usable grid.
pub fn spawn_irradiance_volume(commands: &mut Commands, world: &GfxWorld, images: &mut Assets<Image>) -> bool {
    let grid = &world.light_grid;
    let points = grid.points();
    if points.is_empty() {
        return false;
    }

    // Grid bounds to cover (CoD axes, grid coordinates).
    let sorted: [Vec<u32>; 3] = std::array::from_fn(|axis| {
        let mut v: Vec<u32> = points.iter().map(|(p, _)| p[axis]).collect();
        v.sort_unstable();
        v
    });
    let bounds = |cut: f32| -> ([u32; 3], [u32; 3]) {
        let at = |axis: usize, f: f32| sorted[axis][((sorted[axis].len() - 1) as f32 * f) as usize];
        (
            std::array::from_fn(|i| at(i, cut).saturating_sub(CROP_MARGIN).max(grid.mins[i] as u32)),
            std::array::from_fn(|i| (at(i, 1.0 - cut) + CROP_MARGIN).min(grid.maxs[i] as u32)),
        )
    };
    let size = |(lo, hi): ([u32; 3], [u32; 3])| (0..3).map(|i| (hi[i] - lo[i] + 1) as usize).product::<usize>();
    let (lo, hi) = [0.005, 0.01, 0.02, 0.03, 0.05, 0.08]
        .into_iter()
        .map(bounds)
        .find(|&b| size(b) <= MAX_VOXELS)
        .unwrap_or_else(|| bounds(0.08));
    let n = [0, 1, 2].map(|i| (hi[i] - lo[i] + 1) as usize);
    let index = |p: [usize; 3]| p[0] + n[0] * (p[1] + n[1] * p[2]);

    // Ambient cube per point: the light arriving at a surface facing each of
    // CoD's ±x, ±y, ±z, linearised the way the world's lightmaps are.
    let dirs = LightGrid::directions();
    let face_dirs: Vec<Vec<usize>> = (0..6)
        .map(|f| {
            let (axis, sign) = (f / 2, if f % 2 == 0 { 1.0 } else { -1.0 });
            (0..dirs.len())
                .filter(|&k| (dirs[k][axis] - sign).abs() < 1e-3 && (0..3).all(|a| a == axis || dirs[k][a].abs() < 0.5))
                .collect()
        })
        .collect();
    let mut cells: Vec<Option<[[f32; 3]; 6]>> = vec![None; n[0] * n[1] * n[2]];
    let mut queue = VecDeque::new();
    for (p, entry) in &points {
        if (0..3).any(|i| p[i] < lo[i] || p[i] > hi[i]) {
            continue;
        }
        let Some(colors) = grid.colors.get(entry.colors_index as usize) else { continue };
        let cube: [[f32; 3]; 6] = std::array::from_fn(|f| {
            let ks = &face_dirs[f];
            std::array::from_fn(|c| {
                ks.iter().map(|&k| (colors.0[k][c] as f32 / 255.0).powf(2.2)).sum::<f32>() / ks.len() as f32
            })
        });
        let cube = if crate::vision::disabled() { cube } else { grid_contrast(cube, LIGHT_GRID_CONTRAST) };
        let q = [0, 1, 2].map(|i| (p[i] - lo[i]) as usize);
        cells[index(q)] = Some(cube);
        queue.push_back(q);
    }
    if queue.is_empty() {
        return false;
    }
    // Cells without a sample (inside walls, under floors, in the air) take
    // their nearest sample's light, so blending towards them doesn't darken.
    while let Some(q) = queue.pop_front() {
        let cube = cells[index(q)];
        for (axis, step) in [(0, -1), (0, 1), (1, -1), (1, 1), (2, -1), (2, 1)] {
            let mut r = q;
            let Some(c) = r[axis].checked_add_signed(step).filter(|&c| c < n[axis]) else { continue };
            r[axis] = c;
            if cells[index(r)].is_none() {
                cells[index(r)] = cube;
                queue.push_back(r);
            }
        }
    }

    // Bevy's layout: a (Rx, 2Ry, 3Rz) texture over Bevy axes, positive sides
    // in the first Ry rows and negative ones in the next, X, Y and Z sides in
    // successive thirds of the depth. Bevy x = CoD x, y = CoD z, z = -CoD y.
    let r = [n[0], n[2], n[1]];
    let mut data = vec![0u8; r[0] * 2 * r[1] * 3 * r[2] * 4];
    // Bevy sides +X, -X, +Y, -Y, +Z, -Z as CoD faces (index = axis * 2 + negative).
    const SIDES: [usize; 6] = [0, 1, 4, 5, 3, 2];
    for bz in 0..r[2] {
        for by in 0..r[1] {
            for bx in 0..r[0] {
                let Some(cube) = cells[index([bx, n[1] - 1 - bz, by])] else { continue };
                for (side, &face) in SIDES.iter().enumerate() {
                    let t = by + if side % 2 == 1 { r[1] } else { 0 };
                    let p = bz + (side / 2) * r[2];
                    let texel = bx + r[0] * (t + 2 * r[1] * p);
                    data[texel * 4..texel * 4 + 4].copy_from_slice(&rgb9e5(cube[face]).to_le_bytes());
                }
            }
        }
    }
    let mut image = Image::new(
        Extent3d { width: r[0] as u32, height: 2 * r[1] as u32, depth_or_array_layers: 3 * r[2] as u32 },
        TextureDimension::D3,
        data,
        TextureFormat::Rgb9e5Ufloat,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::linear();

    // The volume's box: voxel centres on the grid points.
    let spacing = LightGrid::SPACING;
    let corner = |p: [u32; 3], side: f32| {
        let w = LightGrid::world_pos(p);
        units::pos(std::array::from_fn(|i| w[i] + side * spacing[i] / 2.0))
    };
    let (a, b) = (corner(lo, -1.0), corner(hi, 1.0));
    info!(
        "light grid: {} points, irradiance volume {}x{}x{} ({:.1} MB)",
        points.len(),
        r[0],
        r[1],
        r[2],
        (r[0] * r[1] * r[2] * 24) as f32 / 1e6
    );
    commands.spawn((
        Name::new("light grid"),
        IrradianceVolume {
            voxels: images.add(image),
            intensity: LIGHTMAP_EXPOSURE * if crate::vision::disabled() { 1.0 } else { LIGHT_GRID_INTENSITY },
            affects_lightmapped_meshes: false,
        },
        Transform::from_translation((a + b) / 2.0).with_scale((b - a).abs()),
        // The viewmodel cameras only see their own layers.
        RenderLayers::from_layers(&std::iter::once(0).chain(crate::splitscreen::viewmodel_layers()).collect::<Vec<_>>()),
    ));
    true
}

/// Pack linear RGB into `Rgb9e5Ufloat`.
fn rgb9e5(c: [f32; 3]) -> u32 {
    const MANTISSA: i32 = 9;
    const BIAS: i32 = 15;
    let max = ((1 << MANTISSA) - 1) as f32 / (1 << MANTISSA) as f32 * 2f32.powi(31 - BIAS);
    let c = c.map(|v| if v.is_finite() { v.clamp(0.0, max) } else { 0.0 });
    let largest = c[0].max(c[1]).max(c[2]);
    if largest <= 0.0 {
        return 0;
    }
    let mut exp = (-BIAS - 1).max(largest.log2().floor() as i32) + 1 + BIAS;
    let mut denom = 2f32.powi(exp - BIAS - MANTISSA);
    if (largest / denom + 0.5).floor() as i32 == 1 << MANTISSA {
        denom *= 2.0;
        exp += 1;
    }
    let [r, g, b] = c.map(|v| (v / denom + 0.5).floor() as u32);
    r | g << 9 | b << 18 | (exp as u32) << 27
}

/// The sun as a local player's viewmodel sees it: the map's sun, dimmed by
/// how much of it their eye can see.
#[derive(Component)]
pub struct ViewModelSun {
    visible: f32,
    slot: usize,
}

impl ViewModelSun {
    pub fn new(slot: usize) -> Self {
        ViewModelSun { visible: 1.0, slot }
    }
}

/// Rays towards the sun from around the eye; offsets in metres (right, up).
const SUN_RAYS: [(f32, f32); 5] = [(0.0, 0.0), (0.25, 0.0), (-0.25, 0.0), (0.0, 0.25), (0.0, -0.3)];

fn viewmodel_sun(
    time: Res<Time>,
    spatial: SpatialQuery,
    cameras: Query<(&GlobalTransform, &crate::splitscreen::SlotCamera)>,
    sun: Query<(&DirectionalLight, &Transform), Without<ViewModelSun>>,
    mut vm_sun: Query<(&mut DirectionalLight, &mut Transform, &mut ViewModelSun)>,
) {
    let Some((sun_light, sun_tf)) = sun.iter().find(|(l, _)| l.shadow_maps_enabled) else { return };
    let to_sun = -sun_tf.forward();
    let filter = crate::collision::sight_filter();
    for (mut light, mut tf, mut state) in &mut vm_sun {
        let Some((camera, _)) = cameras.iter().find(|(_, s)| s.0 == state.slot) else { continue };
        let eye = camera.translation();
        let (right, up) = (camera.right(), camera.up());
        let lit = SUN_RAYS
            .iter()
            .filter(|(r, u)| {
                let from = eye + right * *r + up * *u;
                spatial.cast_ray(from, to_sun, units::u(4000.0), true, &filter).is_none()
            })
            .count() as f32
            / SUN_RAYS.len() as f32;
        // Eyes adjust over a moment rather than snapping.
        let k = 1.0 - (-time.delta_secs() / 0.15).exp();
        state.visible += (lit - state.visible) * k;
        light.color = sun_light.color;
        light.illuminance = sun_light.illuminance * state.visible;
        *tf = *sun_tf;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contrast_pushes_sides_apart() {
        let cube = [[1.0, 1.0, 1.0], [0.5, 0.5, 0.5], [0.75; 3], [0.75; 3], [0.75; 3], [0.75; 3]];
        let out = grid_contrast(cube, 0.25);
        assert!((out[0][0] - (0.75 + 0.25 * 1.25)).abs() < 1e-6);
        assert!((out[1][0] - (0.75 - 0.25 * 1.25)).abs() < 1e-6);
        assert_eq!(out[2], [0.75; 3]);
    }
}

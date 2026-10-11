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
        app.add_systems(Update, (soak, viewmodel_sun, viewmodel_reflections).chain().run_if(crate::state::in_game));
    }
}

/// The light grid's points are cropped to where nearly all of them are (a
/// few strays far outside the playable area would blow up the volume) plus
/// this many cells of margin...
const CROP_MARGIN: u32 = 2;

/// ...and cropped harder, dropping a few more outlying points, while the
/// volume would have more voxels than this (24 bytes each).
const MAX_VOXELS: usize = 1_500_000;

/// Grid layers added over the grid's top (CoD4's grid is 64 units high a
/// layer: about 20 m).
const LAYERS_ABOVE: u32 = 12;

/// Black Ops' light grid tweaks: its maps use 1.1-1.5 and 0.2-0.4; CoD4's
/// set none, so these are Black Ops' usual values. Its contrast is off: on
/// CoD4's grids (lit from one side much more than the other) pushing the
/// sides apart left trees', guns' and players' shaded sides black.
const LIGHT_GRID_INTENSITY: f32 = 1.3;
const LIGHT_GRID_CONTRAST: f32 = 0.0;

/// The light grid's tweaks: an MW2 map's own (its script sets them for its
/// grid), else the ones above.
fn light_grid_intensity() -> f32 {
    crate::mw2::light_grid_tweaks().map_or(LIGHT_GRID_INTENSITY, |t| t.0)
}

fn light_grid_contrast() -> f32 {
    crate::mw2::light_grid_tweaks().map_or(LIGHT_GRID_CONTRAST, |t| t.1)
}

/// `r_lightGridContrast`: each side of an ambient cube pushed away from the
/// cube's mean (never below black).
fn grid_contrast(cube: [[f32; 3]; 6], contrast: f32) -> [[f32; 3]; 6] {
    let mean: [f32; 3] = std::array::from_fn(|c| cube.iter().map(|f| f[c]).sum::<f32>() / 6.0);
    cube.map(|f| std::array::from_fn(|c| (mean[c] + (f[c] - mean[c]) * (1.0 + contrast)).max(0.0)))
}

/// Build an irradiance volume from `world`'s light grid. Returns false if the
/// map has no usable grid.
///
/// `rebaked`: the re-baked grid's cubes (`crate::lightmaps::Rebaked`), used
/// instead of CoD4's at the points it has.
pub fn spawn_irradiance_volume(commands: &mut Commands, world: &GfxWorld, images: &mut Assets<Image>, rebaked: Option<&std::collections::HashMap<[u32; 3], [[f32; 3]; 6]>>) -> bool {
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
    // The grid stops a little over the playable space; trees, poles and
    // roofs above it took no light (Bloc's trees were black). The layers
    // above take the top samples' light (filled below).
    let hi = [hi[0], hi[1], hi[2] + LAYERS_ABOVE];
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
    // Which grid point (index in `points`) each cell takes its light from.
    let mut src: Vec<Option<u32>> = vec![None; cells.len()];
    let mut queue = VecDeque::new();
    for (pi, (p, entry)) in points.iter().enumerate() {
        if (0..3).any(|i| p[i] < lo[i] || p[i] > hi[i]) {
            continue;
        }
        let Some(colors) = grid.colors.get(entry.colors_index as usize) else { continue };
        let cube: [[f32; 3]; 6] = if let Some(c) = rebaked.and_then(|r| r.get(p)) { *c } else { std::array::from_fn(|f| {
            let ks = &face_dirs[f];
            std::array::from_fn(|c| {
                ks.iter().map(|&k| (colors.0[k][c] as f32 / 255.0).powf(2.2)).sum::<f32>() / ks.len() as f32
            })
        }) };
        let cube = if crate::vision::disabled() { cube } else { grid_contrast(cube, light_grid_contrast()) };
        let q = [0, 1, 2].map(|i| (p[i] - lo[i]) as usize);
        cells[index(q)] = Some(cube);
        src[index(q)] = Some(pi as u32);
        queue.push_back(q);
    }
    if queue.is_empty() {
        return false;
    }
    // Cells without a sample (inside walls, under floors, in the air) take
    // their nearest sample's light, so blending towards them doesn't darken.
    while let Some(q) = queue.pop_front() {
        let (cube, from) = (cells[index(q)], src[index(q)]);
        for (axis, step) in [(0, -1), (0, 1), (1, -1), (1, 1), (2, -1), (2, 1)] {
            let mut r = q;
            let Some(c) = r[axis].checked_add_signed(step).filter(|&c| c < n[axis]) else { continue };
            r[axis] = c;
            if cells[index(r)].is_none() {
                cells[index(r)] = cube;
                src[index(r)] = from;
                queue.push_back(r);
            }
        }
    }

    // Each cell's mean light, for looking up: ray-traced lighting lights
    // props from one sample each, as CoD4 did (`crate::rtgi`), and the
    // viewmodel's reflections follow how lit the eye is.
    {
        let mean: Vec<[f32; 3]> = cells
            .iter()
            .map(|c| c.map_or([0.0; 3], |c| std::array::from_fn(|k| c.iter().map(|f| f[k]).sum::<f32>() / 6.0)))
            .collect();
        let scale = LIGHTMAP_EXPOSURE * if crate::vision::disabled() { 1.0 } else { light_grid_intensity() };
        // The open air's light: the brightest tenth of the sampled cells.
        let mut lit: Vec<f32> =
            points.iter().filter_map(|(p, _)| (0..3).all(|i| p[i] >= lo[i] && p[i] <= hi[i]).then(|| luma(mean[index([0, 1, 2].map(|i| (p[i] - lo[i]) as usize))]))).collect();
        lit.sort_by(f32::total_cmp);
        let open = lit.get(lit.len() * 9 / 10).copied().unwrap_or(1.0).max(1e-4) * scale;
        commands.insert_resource(LightGridLookup { lo, n, mean, scale, open });
    }

    let r = [n[0], n[2], n[1]];
    let data = volume_data(n, |cell| cells[cell]);
    let mut image = Image::new(
        Extent3d { width: r[0] as u32, height: 2 * r[1] as u32, depth_or_array_layers: 3 * r[2] as u32 },
        TextureDimension::D3,
        data,
        TextureFormat::Rgb9e5Ufloat,
        // With the showcase's time of day the volume is lit anew as the
        // clock runs (`crate::tod_light`): kept in the main world for that
        // (a render-world-only image leaves it once uploaded, and the
        // rewrites were lost: the gun and arms kept the first hour's light).
        if crate::atmos::climate::enabled() { RenderAssetUsages::default() } else { RenderAssetUsages::RENDER_WORLD },
    );
    image.sampler = ImageSampler::linear();
    let voxels = images.add(image);
    commands.insert_resource(GridVolume { image: voxels.clone(), n, src });

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
            voxels,
            intensity: LIGHTMAP_EXPOSURE * if crate::vision::disabled() { 1.0 } else { light_grid_intensity() },
            affects_lightmapped_meshes: false,
        },
        GridIntensity(LIGHTMAP_EXPOSURE * if crate::vision::disabled() { 1.0 } else { light_grid_intensity() }),
        Transform::from_translation((a + b) / 2.0).with_scale((b - a).abs()),
        // The viewmodel cameras only see their own layers.
        // (And the players' own first-person bodies, in splitscreen.)
        RenderLayers::from_layers(
            &std::iter::once(0)
                .chain(crate::splitscreen::viewmodel_layers())
                .chain((0..crate::splitscreen::MAX_PLAYERS).map(crate::first_person::body::layer))
                .collect::<Vec<_>>(),
        ),
    ));
    true
}

/// The light grid's mean light per cell (the irradiance volume's cells),
/// for lighting a whole model from one point, as CoD4 lit static models
/// (from the light grid at their bounds' centre).
#[derive(Resource)]
pub struct LightGridLookup {
    lo: [u32; 3],
    n: [usize; 3],
    mean: Vec<[f32; 3]>,
    /// To Bevy light units (as the irradiance volume's intensity).
    scale: f32,
    /// How bright the open air is (luminance, Bevy light units).
    pub open: f32,
}

fn luma(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

impl LightGridLookup {
    /// The light around Bevy-space point `at` (zero outside the grid).
    pub fn at(&self, at: Vec3) -> Vec3 {
        let cod = units::to_cod(at);
        let spacing = LightGrid::SPACING;
        let q: [i64; 3] = std::array::from_fn(|i| ((cod[i] + 131_072.0) / spacing[i]).round() as i64 - self.lo[i] as i64);
        if (0..3).any(|i| q[i] < 0 || q[i] >= self.n[i] as i64) {
            return Vec3::ZERO;
        }
        let index = q[0] as usize + self.n[0] * (q[1] as usize + self.n[1] * q[2] as usize);
        Vec3::from(self.mean[index]) * self.scale
    }
}

/// The irradiance volume's texels for grid cells `n` (CoD axes), each
/// cell's ambient cube from `cell_cube`. Bevy's layout: a (Rx, 2Ry, 3Rz)
/// texture over Bevy axes, positive sides in the first Ry rows and negative
/// ones in the next, X, Y and Z sides in successive thirds of the depth.
/// Bevy x = CoD x, y = CoD z, z = -CoD y.
fn volume_data(n: [usize; 3], cell_cube: impl Fn(usize) -> Option<[[f32; 3]; 6]>) -> Vec<u8> {
    let index = |p: [usize; 3]| p[0] + n[0] * (p[1] + n[1] * p[2]);
    let r = [n[0], n[2], n[1]];
    let mut data = vec![0u8; r[0] * 2 * r[1] * 3 * r[2] * 4];
    // Bevy sides +X, -X, +Y, -Y, +Z, -Z as CoD faces (index = axis * 2 + negative).
    const SIDES: [usize; 6] = [0, 1, 4, 5, 3, 2];
    for bz in 0..r[2] {
        for by in 0..r[1] {
            for bx in 0..r[0] {
                let Some(cube) = cell_cube(index([bx, n[1] - 1 - bz, by])) else { continue };
                for (side, &face) in SIDES.iter().enumerate() {
                    let t = by + if side % 2 == 1 { r[1] } else { 0 };
                    let p = bz + (side / 2) * r[2];
                    let texel = bx + r[0] * (t + 2 * r[1] * p);
                    data[texel * 4..texel * 4 + 4].copy_from_slice(&rgb9e5(cube[face]).to_le_bytes());
                }
            }
        }
    }
    data
}

/// The light grid's irradiance volume, for lighting it anew (the
/// showcase's time of day, `crate::tod_light`).
#[derive(Resource, Clone)]
pub struct GridVolume {
    pub image: Handle<Image>,
    /// Cells (CoD axes).
    n: [usize; 3],
    /// Per cell, the grid point (index in `LightGrid::points`) it takes its
    /// light from.
    src: Vec<Option<u32>>,
}

impl GridVolume {
    /// The volume's texels with each grid point's light from `point_cube`
    /// (linear lightmap units, as the bake stores them).
    pub fn data(&self, point_cube: impl Fn(usize) -> Option<[[f32; 3]; 6]>) -> Vec<u8> {
        volume_data(self.n, |cell| {
            let cube = point_cube(self.src[cell]? as usize)?;
            Some(if crate::vision::disabled() { cube } else { grid_contrast(cube, light_grid_contrast()) })
        })
    }
}

/// The light grid volume's own intensity, before the rain darkens it.
#[derive(Component)]
struct GridIntensity(f32);

/// How much of their light models keep in the rain (the showcase's
/// weather): the world's wet surfaces darken (`shaders/world.wgsl`'s
/// soak), so models standing in the same rain do too, through the light
/// grid that lights them (the gun, arms, props and soldiers), and the
/// viewmodel's sun and reflections. Dry, 1. Without it the gun stayed as
/// bright as dry over a soaked deck, and auto-exposure lifted it to a glow.
static SOAK: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x3f80_0000);

pub fn soak_share() -> f32 {
    f32::from_bits(SOAK.load(std::sync::atomic::Ordering::Relaxed))
}

/// About the world's average soak at full wetness (its wet surfaces lose
/// 25-55% of their colour, puddles more).
const SOAK_DARKEN: f32 = 0.4;

fn soak(weather: Option<Res<crate::atmos::climate::Weather>>, mut volumes: Query<(&mut IrradianceVolume, &GridIntensity)>) {
    let wet = weather.filter(|w| w.enabled).map_or(0.0, |w| w.wetness.clamp(0.0, 1.0));
    let keep = 1.0 - SOAK_DARKEN * wet;
    // Debug aid: `COD4RW_VM_NOGRID` turns the light grid off.
    let grid_on = if std::env::var_os("COD4RW_VM_NOGRID").is_some() { 0.0 } else { 1.0 };
    SOAK.store(keep.to_bits(), std::sync::atomic::Ordering::Relaxed);
    for (mut v, base) in &mut volumes {
        let want = base.0 * keep * grid_on;
        if (v.intensity - want).abs() > base.0 * 0.005 {
            v.intensity = want;
        }
    }
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

/// The sun as a local player's viewmodel sees it. With the world's
/// lighting ([`crate::first_person::world_lighting`]) it's the map's sun
/// shadowed by the world: its shadow map takes the world's shadow casters
/// (the merged proxy, [`crate::world::SHADOW_PROXY_LAYER`], with the
/// player's own body on it), in one small cascade fitted to the viewmodel
/// camera's few metres, so the gun goes dark in a doorway's shade or under
/// a roof, with its own shadows on the arms. Otherwise (CoD4's way) it's
/// unshadowed, dimmed by how much of the sun the eye can see.
#[derive(Component)]
pub struct ViewModelSun {
    visible: f32,
    slot: usize,
    /// Shadowed by the world, as last set up.
    shadowed: Option<bool>,
}

impl ViewModelSun {
    pub fn new(slot: usize) -> Self {
        ViewModelSun { visible: 1.0, slot, shadowed: None }
    }
}

/// Rays towards the sun from around the eye; offsets in metres (right, up).
const SUN_RAYS: [(f32, f32); 5] = [(0.0, 0.0), (0.25, 0.0), (-0.25, 0.0), (0.0, 0.25), (0.0, -0.3)];

/// How far the viewmodel sun's shadow cascade reaches (metres): past the
/// gun's muzzle, and no more, for sharp shadows.
const VIEWMODEL_SHADOW_DISTANCE: f32 = 3.0;

#[allow(clippy::type_complexity)]
/// The viewmodel's reflections (its guns' IW3 specular, from the reflection
/// probe nearest the eye) scaled by how lit the eye is against the open
/// air: probes are captured in the open, so in a doorway's shade or
/// indoors a gun went on gleaming with the sky. Only CoD4's gun materials
/// (`CamoMaterial`) reflect probes; the arms take the light grid.
fn viewmodel_reflections(
    time: Res<Time>,
    grid: Option<Res<LightGridLookup>>,
    cameras: Query<(&GlobalTransform, &crate::splitscreen::SlotCamera)>,
    surfaces: Query<(&MeshMaterial3d<crate::gunmodel::CamoMaterial>, &RenderLayers)>,
    mut materials: ResMut<Assets<crate::gunmodel::CamoMaterial>>,
    mut level: Local<f32>,
) {
    let Some(grid) = grid else { return };
    let light_share_dim = crate::atmos::climate::light_share().min(1.0).max(1e-3);
    let target = match cameras.iter().find(|c| c.1.0 == 0) {
        Some((eye, _)) if crate::first_person::world_lighting() => {
            let here = grid.at(eye.translation());
            (luma(here.to_array()) / grid.open).clamp(0.2, 1.0)
        }
        _ => 1.0,
    // The probes hold CoD4's light: the showcase's night or storm dims them.
    } * light_share_dim;
    // Eyes adjust over a moment rather than snapping.
    let k = 1.0 - (-time.delta_secs() / 0.25).exp();
    let before = *level;
    *level = if *level == 0.0 { target } else { *level + (target - *level) * k };
    // The probes were captured in CoD4's light: with the showcase's time
    // of day, scaled by the sky light now against that (at night the gun
    // gleamed with the probe's light over a dark deck).
    let exposure = LIGHTMAP_EXPOSURE * *level * crate::atmos::climate::light_share() * soak_share();
    // Debug aid: `COD4RW_VM_NOSHINE` leaves the gun's reflections out.
    let exposure = if std::env::var_os("COD4RW_VM_NOSHINE").is_some() { 0.0 } else { exposure };
    let viewmodel = RenderLayers::from_layers(&crate::splitscreen::viewmodel_layers());
    for (m, layers) in &surfaces {
        if !layers.intersects(&viewmodel) {
            continue;
        }
        // Polished metal (gold, platinum: a fresnel minimum of 3 and up;
        // ordinary gun metal's is at most 2) isn't dimmed by the night
        // (`camo.wgsl` floors its light), but still by the shade round the
        // eye, as the arms are: undimmed, a platinum gun glowed white in
        // the shade.
        let polished = materials.get(&m.0).is_some_and(|mat| {
            let (z, polished) = (mat.extension.scale.z, mat.extension.env.x > 2.9);
            polished || (z > 2.5 && z < 5.5)
        });
        let exposure = if polished { LIGHTMAP_EXPOSURE * *level / light_share_dim * soak_share() } else { exposure };
        let exposure = if std::env::var_os("COD4RW_VM_NOSHINE").is_some() { 0.0 } else { exposure };
        // Only when it's moved (or for a new gun's materials).
        if materials.get(&m.0).is_some_and(|mat| (mat.extension.shine.w - exposure).abs() > 0.01 * LIGHTMAP_EXPOSURE || (before - *level).abs() > 0.01) {
            if let Some(mut mat) = materials.get_mut(&m.0) {
                mat.extension.shine.w = exposure;
            }
        }
    }
}

fn viewmodel_sun(
    mut commands: Commands,
    time: Res<Time>,
    spatial: SpatialQuery,
    cameras: Query<(&GlobalTransform, &crate::splitscreen::SlotCamera)>,
    sun: Query<(&DirectionalLight, &Transform), Without<ViewModelSun>>,
    mut vm_sun: Query<(Entity, &mut DirectionalLight, &mut Transform, &mut ViewModelSun)>,
) {
    // The map's sun: the one with shadows (or, with shadows off, the
    // brightest).
    let Some((sun_light, sun_tf)) = sun
        .iter()
        .find(|(l, _)| l.shadow_maps_enabled)
        .or_else(|| sun.iter().max_by(|a, b| a.0.illuminance.total_cmp(&b.0.illuminance)))
    else {
        return;
    };
    let world = crate::first_person::world_lighting();
    let shadows = world && sun_light.shadow_maps_enabled;
    let to_sun = -sun_tf.forward();
    let filter = crate::collision::sight_filter();
    for (e, mut light, mut tf, mut state) in &mut vm_sun {
        light.color = sun_light.color;
        *tf = *sun_tf;
        // Debug aid: `COD4RW_VM_NOSUN` leaves the viewmodel's sun (or moon) out.
        if std::env::var_os("COD4RW_VM_NOSUN").is_some() {
            light.illuminance = 0.0;
            continue;
        }
        if state.shadowed != Some(shadows) {
            state.shadowed = Some(shadows);
            light.shadow_maps_enabled = shadows;
            let layer = crate::splitscreen::viewmodel_layer(state.slot);
            let layers = if shadows { RenderLayers::from_layers(&[layer, crate::world::SHADOW_PROXY_LAYER]) } else { RenderLayers::layer(layer) };
            let cascades = bevy::light::CascadeShadowConfigBuilder {
                num_cascades: 1,
                minimum_distance: 0.01,
                maximum_distance: VIEWMODEL_SHADOW_DISTANCE,
                first_cascade_far_bound: VIEWMODEL_SHADOW_DISTANCE,
                overlap_proportion: 0.0,
            };
            commands.entity(e).insert((layers, cascades.build()));
        }
        if shadows {
            light.illuminance = sun_light.illuminance * soak_share();
            continue;
        }
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
        light.illuminance = sun_light.illuminance * state.visible * soak_share();
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


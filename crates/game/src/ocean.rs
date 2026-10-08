//! The showcase ocean (`COD4RW_SHOWCASE=1`): in place of the map's own sea
//! (`wc_water`, IW3's flat `water_l_sun` sheet), a moving one.
//! - Gerstner waves, their height set by the wind, calmer in the ship's lee;
//! - the sky reflected by Fresnel (whichever skybox the camera has, the
//!   classic one or the dynamic sky's), the sun's (or moon's) glint, shadowed
//!   by the ship;
//! - light through the crests (subsurface), foam where the waves fold and
//!   along the hull, a band streaming off it with the current;
//! - rain rings on the surface.
//!
//! See `shaders/ocean.wgsl`. The wind and rain come from `COD4RW_OCEAN_WIND`
//! / `COD4RW_OCEAN_RAIN` (0..1) until the atmosphere's weather is in.

use crate::content::{Content, MAP_ZONE};
use crate::player::MainCamera;
use crate::units;
use crate::world::MapWater;
use bevy::asset::RenderAssetUsages;
use bevy::light::{NotShadowCaster, Skybox};
use bevy::mesh::{Indices, MeshVertexBufferLayoutRef, PrimitiveTopology};
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, Extent3d, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError, TextureDimension, TextureFormat};
use bevy::shader::ShaderRef;

const SHADER: &str = "embedded://cod4rw/ocean.wgsl";

/// The showcase runs on this map (`crate::atmos::climate::showcase`).
pub fn showcase() -> bool {
    crate::atmos::climate::showcase()
}

pub struct OceanPlugin;

impl Plugin for OceanPlugin {
    fn build(&self, app: &mut App) {
        // (`COD4RW_NOOCEAN`: the showcase without it, for timing.)
        if !crate::atmos::climate::enabled() || std::env::var_os("COD4RW_NOOCEAN").is_some() {
            return;
        }
        app.world().resource::<bevy::asset::io::embedded::EmbeddedAssetRegistry>().insert_asset(
            std::path::PathBuf::from(file!()).with_file_name("shaders/ocean.wgsl"),
            std::path::Path::new("cod4rw/ocean.wgsl"),
            include_bytes!("shaders/ocean.wgsl").as_slice(),
        );
        app.add_plugins(MaterialPlugin::<OceanMaterial>::default())
            .add_systems(Update, (spawn_ocean, drive_ocean).chain().run_if(crate::state::in_game).run_if(crate::atmos::climate::on));
    }
}

/// The ocean's numbers (see `ocean.wgsl`'s `Ocean`).
#[derive(Clone, Copy, Debug, Default, ShaderType)]
struct OceanUniform {
    /// x, y: the wind's direction (Bevy x, z); z: seconds; w: wind 0..1.
    wind: Vec4,
    /// x: rain 0..1; y: the surface's height; z: the skybox's brightness;
    /// w: 1 when the hull map is there.
    misc: Vec4,
    /// The hull map's corner (x, z) and size (x, z), metres.
    hull_rect: Vec4,
    /// The skybox's turn (a quaternion, applied inverted as the skybox does).
    sky_rotation: Vec4,
    /// Towards the moon (xyz) and how much night it is (w).
    moon: Vec4,
    /// The moon's light (rgb, lux).
    moon_light: Vec4,
    /// rgb: what the sky draws at the horizon line, in the frame's units
    /// (after exposure: `TimeOfDay::horizon`); w: 1 when there is one.
    horizon: Vec4,
    /// x: debug view (`COD4RW_OCEAN_DEBUG`: 1 the sky reflected, 2 the
    /// water's body, 3 the glints, 4 foam, 5 the sky along the view, which
    /// should carry on the sky drawn behind the sea); y: cloud cover 0..1.
    debug: Vec4,
}

#[derive(Asset, AsBindGroup, TypePath, Debug, Clone)]
pub struct OceanMaterial {
    #[uniform(0)]
    ocean: OceanUniform,
    #[texture(1, dimension = "cube")]
    #[sampler(2)]
    sky: Handle<Image>,
    /// How far each spot is from the hull (metres / 32, 0..1).
    #[texture(3)]
    #[sampler(4)]
    hull: Handle<Image>,
}

impl Material for OceanMaterial {
    fn vertex_shader() -> ShaderRef {
        SHADER.into()
    }

    fn fragment_shader() -> ShaderRef {
        SHADER.into()
    }

    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        // Seen from either side (from under a wave's lip).
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

#[derive(Component)]
struct Ocean;

/// Grid cells across the ocean's mesh each way.
const GRID: u32 = 220;
/// The hull map's size, texels each way.
const HULL_MAP: usize = 256;
/// Distance it stores at most (metres).
const HULL_RANGE: f32 = 32.0;

/// Once the map's sea is in: hide it and lay the ocean in its place.
#[allow(clippy::too_many_arguments)]
fn spawn_ocean(
    mut commands: Commands,
    mut waters: Query<(&MapWater, &mut Visibility), Without<Ocean>>,
    oceans: Query<(), With<Ocean>>,
    content: Option<Res<Content>>,
    camera: Query<&Skybox, With<MainCamera>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<OceanMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    if waters.is_empty() || !oceans.is_empty() {
        return;
    }
    let Some(content) = content else { return };
    let Ok(skybox) = camera.single() else { return };
    let Some(sky) = skybox.image.clone() else { return };
    let (mut lo, mut hi) = (Vec3::MAX, Vec3::MIN);
    for (w, mut vis) in &mut waters {
        lo = lo.min(w.min);
        hi = hi.max(w.max);
        *vis = Visibility::Hidden;
    }
    let height = lo.y.max(hi.y.min(lo.y + 0.01));
    let rect = Rect::from_corners(lo.xz(), hi.xz());
    let t0 = std::time::Instant::now();
    let hull = hull_map(&content, height, rect);
    info!(
        "ocean: {:.0} x {:.0} m at {:.1} m, hull map {} in {:.0} ms",
        rect.width(),
        rect.height(),
        height,
        if hull.is_some() { "made" } else { "empty" },
        t0.elapsed().as_secs_f32() * 1000.0
    );
    let has_hull = hull.is_some();
    let hull = images.add(hull.unwrap_or_else(|| hull_image(vec![255; HULL_MAP * HULL_MAP])));
    let material = materials.add(OceanMaterial {
        ocean: OceanUniform {
            wind: Vec4::new(0.8, 0.6, 0.0, wind()),
            misc: Vec4::new(rain(), height, skybox.brightness, has_hull as u32 as f32),
            hull_rect: Vec4::new(rect.min.x, rect.min.y, rect.width(), rect.height()),
            sky_rotation: Vec4::from(skybox.rotation),
            moon: Vec4::ZERO,
            moon_light: Vec4::ZERO,
            horizon: Vec4::ZERO,
            debug: Vec4::new(std::env::var("COD4RW_OCEAN_DEBUG").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0), 0.0, 0.0, 0.0),
        },
        sky,
        hull,
    });
    commands.spawn((Name::new("ocean"), Ocean, Mesh3d(meshes.add(grid(rect, height))), MeshMaterial3d(material), NotShadowCaster, Transform::default()));
}

/// The wind, 0 (calm) to 1 (a storm).
fn wind() -> f32 {
    std::env::var("COD4RW_OCEAN_WIND").ok().and_then(|v| v.parse().ok()).unwrap_or(SEA_STATE).clamp(0.0, 1.0)
}

fn rain() -> f32 {
    std::env::var("COD4RW_OCEAN_RAIN").ok().and_then(|v| v.parse().ok()).unwrap_or(0.5f32).clamp(0.0, 1.0)
}

/// The sea is at least this rough (0..1) whatever the local weather: Wet
/// Work is out in a storm, and a swell doesn't drop with the rain.
const SEA_STATE: f32 = 0.75;

/// A wind this strong (m/s) is a full storm's waves: the climate's wind in
/// hard rain (`3.2 m/s × (1 + 3 × rain)`), so Wet Work's rain is a storm.
const STORM_WIND: f32 = 12.0;

/// Each frame: the time, the weather (`crate::atmos::climate`), the moon,
/// and the sky the camera shows now.
fn drive_ocean(
    time: Res<Time>,
    oceans: Query<&MeshMaterial3d<OceanMaterial>, With<Ocean>>,
    camera: Query<&Skybox, With<MainCamera>>,
    weather: Option<Res<crate::atmos::climate::Weather>>,
    day: Option<Res<crate::atmos::climate::TimeOfDay>>,
    mut materials: ResMut<Assets<OceanMaterial>>,
) {
    let (Ok(handle), Ok(skybox)) = (oceans.single(), camera.single()) else { return };
    let Some(mut m) = materials.get_mut(&handle.0) else { return };
    if let Some(w) = weather.filter(|w| w.enabled) {
        let flat = Vec2::new(w.wind.x, w.wind.z);
        if flat.length() > 0.1 {
            let d = flat.normalize();
            m.ocean.wind.x = d.x;
            m.ocean.wind.y = d.y;
        }
        m.ocean.wind.w = (w.wind.length() / STORM_WIND).clamp(0.0, 1.0).max(SEA_STATE);
        m.ocean.misc.x = w.rain.clamp(0.0, 1.0);
        m.ocean.debug.y = w.cloud_cover.clamp(0.0, 1.0);
    }
    if let Some(d) = day.filter(|d| d.enabled) {
        m.ocean.moon = d.moon_dir.normalize_or(Vec3::Y).extend(d.night.clamp(0.0, 1.0));
        m.ocean.moon_light = (d.moon_color * d.moon_illuminance).extend(0.0);
        m.ocean.horizon = d.horizon.extend(1.0);
    }
    // (Wrapped, for the waves' precision.)
    m.ocean.wind.z = (time.elapsed_secs_wrapped() as f64 % 3600.0) as f32;
    m.ocean.misc.z = skybox.brightness;
    m.ocean.sky_rotation = Vec4::from(skybox.rotation);
    if let Some(sky) = skybox.image.as_ref().filter(|s| **s != m.sky) {
        m.sky = sky.clone();
    }
}

/// How far the sea reaches past the map's water each way (metres): to the
/// horizon, where it fades into the sky.
const FAR_SEA: f32 = 6000.0;

/// A flat grid at `height` (the vertex shader moves it): even over `rect`,
/// the map's own water, then cells growing ever larger out to
/// [`FAR_SEA`] beyond it, so the sea runs to the horizon.
fn grid(rect: Rect, height: f32) -> Mesh {
    let n = GRID + 1;
    // Cell edges along one axis: the inner part even, the outer quarter of
    // the cells on each side stretched out to the far sea.
    let edges = |lo: f32, hi: f32| -> Vec<f32> {
        let outer = GRID / 5;
        let inner = GRID - 2 * outer;
        (0..n)
            .map(|i| {
                if i < outer {
                    let t = 1.0 - i as f32 / outer as f32;
                    lo - FAR_SEA * t * t * t
                } else if i > outer + inner {
                    let t = (i - outer - inner) as f32 / outer as f32;
                    hi + FAR_SEA * t * t * t
                } else {
                    lo + (hi - lo) * (i - outer) as f32 / inner as f32
                }
            })
            .collect()
    };
    let (xs, zs) = (edges(rect.min.x, rect.max.x), edges(rect.min.y, rect.max.y));
    let mut positions = Vec::with_capacity((n * n) as usize);
    for z in &zs {
        for x in &xs {
            positions.push([*x, height, *z]);
        }
    }
    let mut indices = Vec::with_capacity((GRID * GRID * 6) as usize);
    for j in 0..GRID {
        for i in 0..GRID {
            let a = j * n + i;
            indices.extend_from_slice(&[a, a + n, a + 1, a + 1, a + n, a + n + 1]);
        }
    }
    let normals = vec![[0.0, 1.0, 0.0]; positions.len()];
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

fn hull_image(data: Vec<u8>) -> Image {
    let mut image = Image::new(
        Extent3d { width: HULL_MAP as u32, height: HULL_MAP as u32, depth_or_array_layers: 1 },
        TextureDimension::D2,
        data,
        TextureFormat::R8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = bevy::image::ImageSampler::linear();
    image
}

/// How far each spot of the sea is from the hull: the map's world
/// triangles that reach through the water line (from the map's data, not
/// the meshes), spread out by a distance transform.
fn hull_map(content: &Content, height: f32, rect: Rect) -> Option<Image> {
    let world = content.map().gfx_world()?;
    let mut solid = vec![false; HULL_MAP * HULL_MAP];
    let cell = rect.size() / HULL_MAP as f32;
    let band = (height - 1.0, height + 1.5);
    let mut any = false;
    for s in &world.surfaces {
        let Some(mat) = s.material else { continue };
        let tech = content.technique_set(MAP_ZONE, mat);
        if tech.starts_with("wc_water") || tech.contains("sky") || tech.contains("tools") {
            continue;
        }
        let first = s.first_vertex.max(0) as usize;
        let start = s.base_index.max(0) as usize;
        let end = (start + s.tri_count as usize * 3).min(world.indices.len());
        for tri in world.indices[start..end].chunks_exact(3) {
            let p: Vec<Vec3> = tri.iter().filter_map(|&i| world.vertices.get(first + i as usize)).map(|v| units::pos(v.xyz)).collect();
            if p.len() < 3 {
                continue;
            }
            let (ylo, yhi) = (p.iter().map(|v| v.y).fold(f32::MAX, f32::min), p.iter().map(|v| v.y).fold(f32::MIN, f32::max));
            if yhi < band.0 || ylo > band.1 {
                continue;
            }
            let (xlo, xhi) = (p.iter().map(|v| v.x).fold(f32::MAX, f32::min), p.iter().map(|v| v.x).fold(f32::MIN, f32::max));
            let (zlo, zhi) = (p.iter().map(|v| v.z).fold(f32::MAX, f32::min), p.iter().map(|v| v.z).fold(f32::MIN, f32::max));
            let to_cell = |x: f32, z: f32| (((x - rect.min.x) / cell.x).floor() as i32, ((z - rect.min.y) / cell.y).floor() as i32);
            let (a, b) = (to_cell(xlo, zlo), to_cell(xhi, zhi));
            for j in a.1.max(0)..=b.1.min(HULL_MAP as i32 - 1) {
                for i in a.0.max(0)..=b.0.min(HULL_MAP as i32 - 1) {
                    solid[j as usize * HULL_MAP + i as usize] = true;
                    any = true;
                }
            }
        }
    }
    if !any {
        return None;
    }
    // Two-pass chamfer distance (cells), then metres.
    let n = HULL_MAP;
    let mut d: Vec<f32> = solid.iter().map(|&s| if s { 0.0 } else { 1e9 }).collect();
    let diag = std::f32::consts::SQRT_2;
    for j in 0..n {
        for i in 0..n {
            let mut v = d[j * n + i];
            if i > 0 {
                v = v.min(d[j * n + i - 1] + 1.0);
            }
            if j > 0 {
                v = v.min(d[(j - 1) * n + i] + 1.0);
                if i > 0 {
                    v = v.min(d[(j - 1) * n + i - 1] + diag);
                }
                if i + 1 < n {
                    v = v.min(d[(j - 1) * n + i + 1] + diag);
                }
            }
            d[j * n + i] = v;
        }
    }
    for j in (0..n).rev() {
        for i in (0..n).rev() {
            let mut v = d[j * n + i];
            if i + 1 < n {
                v = v.min(d[j * n + i + 1] + 1.0);
            }
            if j + 1 < n {
                v = v.min(d[(j + 1) * n + i] + 1.0);
                if i + 1 < n {
                    v = v.min(d[(j + 1) * n + i + 1] + diag);
                }
                if i > 0 {
                    v = v.min(d[(j + 1) * n + i - 1] + diag);
                }
            }
            d[j * n + i] = v;
        }
    }
    let metres = cell.x.max(cell.y);
    let data = d.iter().map(|&c| ((c * metres / HULL_RANGE).clamp(0.0, 1.0) * 255.0) as u8).collect();
    Some(hull_image(data))
}

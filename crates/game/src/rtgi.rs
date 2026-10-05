//! Ray-traced lighting (Options > Game > Lighting: Ray Traced Low/High), with
//! Bevy's Solari: the sun and the sky traced through the map, with bounce
//! light (ReSTIR direct and indirect light, a world-space radiance cache),
//! in place of the baked lightmaps, light grid and shadow maps.
//!
//! The map is lit as usual (Baked) unless the setting asks for ray tracing,
//! the GPU can trace rays ([`supported`]) and one player is playing (each
//! splitscreen view would trace the whole frame again). Then, as a match
//! loads:
//! - opaque and alpha-tested world surfaces and props draw into Solari's
//!   G-buffer: their colour textures as plain deferred standard materials
//!   (without lightmaps, and without CoD4's normal maps, which aren't in
//!   Bevy's format);
//! - each also gets a copy of its mesh in the form Solari traces (position,
//!   normal, UV, tangent, 32-bit indices). Alpha-tested ones (foliage,
//!   fences) aren't traced, as Solari traces them solid: they're lit but
//!   cast no shadow. Skinned characters keep their usual lighting;
//! - the sky lights the map through an emissive dome only the rays see
//!   (Solari has no sky light of its own), tinted like the sky;
//! - the sun's shadow maps go (Solari traces its shadows) and screen-space
//!   AO with them.
//!
//! Low traces the world's brushes and big props; High every prop as well,
//! and a finer sky dome.

use crate::splitscreen::SlotCamera;
use bevy::camera::CameraMainTextureUsages;
use bevy::camera::visibility::RenderLayers;
use bevy::asset::UntypedAssetId;
use bevy::material::OpaqueRendererMethod;
use bevy::mesh::skinning::SkinnedMesh;
use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
use bevy::pbr::{DefaultOpaqueRendererMethod, Lightmap, ScreenSpaceAmbientOcclusion};
use bevy::prelude::*;
use bevy::render::render_resource::TextureUsages;
use bevy::render::renderer::RenderDevice;
use bevy::solari::prelude::{RaytracingMesh3d, SolariLighting, SolariPlugins};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

/// The lighting the setting asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lighting {
    Baked,
    RayTracedLow,
    RayTracedHigh,
}

static SUPPORTED: AtomicBool = AtomicBool::new(false);

/// The GPU can trace rays (wgpu's ray query and binding arrays).
pub fn supported() -> bool {
    SUPPORTED.load(Ordering::Relaxed)
}

/// The lighting this match uses: the setting's, if it can be traced.
pub fn lighting() -> Lighting {
    let wanted = crate::ui::lighting();
    if wanted == Lighting::Baked || !supported() || crate::splitscreen::count() > 1 {
        Lighting::Baked
    } else {
        wanted
    }
}

pub struct RtgiPlugin;

impl Plugin for RtgiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(SolariPlugins)
            // Solari makes every material deferred by default; only the ones
            // converted here are (the rest keep their forward shaders, the
            // world's lightmaps among them).
            .insert_resource(DefaultOpaqueRendererMethod::forward())
            .init_resource::<Converted>()
            .add_systems(Startup, check_support)
            .add_systems(OnEnter(crate::state::GameState::InGame), reset.in_set(crate::state::Setup::Content))
            .add_systems(Update, (set_up_view, spawn_sky_dome).run_if(crate::state::in_game))
            .add_systems(PostUpdate, convert.run_if(crate::state::in_game));
    }
}

/// Temporary debug switches (`COD4RW_RTDEBUG=noconvert,nocamera`).
fn debug(what: &str) -> bool {
    std::env::var("COD4RW_RTDEBUG").is_ok_and(|v| v.split(',').any(|w| w == what))
}

fn check_support(device: Option<Res<RenderDevice>>) {
    let ok = device.is_some_and(|d| d.features().contains(SolariPlugins::required_wgpu_features()));
    SUPPORTED.store(ok, Ordering::Relaxed);
    info!("ray-traced lighting {}", if ok { "available" } else { "unavailable: the GPU lacks ray queries" });
}

/// A mesh entity looked at for ray tracing.
#[derive(Component)]
struct Checked;

/// Materials and meshes already converted, this match.
#[derive(Resource, Default)]
struct Converted {
    /// Each material's deferred copy, and whether it's alpha-tested.
    materials: HashMap<UntypedAssetId, Option<(Handle<StandardMaterial>, bool)>>,
    meshes: HashMap<AssetId<Mesh>, Option<Handle<Mesh>>>,
    dome: bool,
    drawn: usize,
    traced: usize,
    logged: (usize, usize),
}

fn reset(mut converted: ResMut<Converted>) {
    *converted = Converted::default();
}

/// A deferred copy of a material for Solari's G-buffer: `None` for
/// translucent ones (they stay as they are), and whether it's alpha-tested.
fn deferred(base: &StandardMaterial, materials: &mut Assets<StandardMaterial>) -> Option<(Handle<StandardMaterial>, bool)> {
    let masked = match base.alpha_mode {
        AlphaMode::Opaque => false,
        AlphaMode::Mask(_) => true,
        _ => return None,
    };
    let m = StandardMaterial {
        opaque_render_method: OpaqueRendererMethod::Deferred,
        unlit: false,
        normal_map_texture: None,
        ..base.clone()
    };
    Some((materials.add(m), masked))
}

/// The mesh in the form Solari traces, and its size (metres across). The
/// game's meshes live only in the render world once drawn, so this works
/// in the frame they're made ([`convert`] runs in `PostUpdate`).
fn traceable(mesh: &Mesh) -> Option<(Mesh, f32)> {
    if mesh.primitive_topology() != PrimitiveTopology::TriangleList {
        return None;
    }
    let positions = mesh.try_attribute_option(Mesh::ATTRIBUTE_POSITION).ok()??.clone();
    let n = positions.len();
    let size = match &positions {
        VertexAttributeValues::Float32x3(p) => {
            let (lo, hi) = p.iter().fold((Vec3::MAX, Vec3::MIN), |(lo, hi), v| (lo.min(Vec3::from(*v)), hi.max(Vec3::from(*v))));
            (hi - lo).length()
        }
        _ => 0.0,
    };
    let normals = mesh
        .try_attribute_option(Mesh::ATTRIBUTE_NORMAL)
        .ok()
        .flatten()
        .cloned()
        .unwrap_or(VertexAttributeValues::Float32x3(vec![[0.0, 1.0, 0.0]; n]));
    let uvs = mesh
        .try_attribute_option(Mesh::ATTRIBUTE_UV_0)
        .ok()
        .flatten()
        .cloned()
        .unwrap_or(VertexAttributeValues::Float32x2(vec![[0.0, 0.0]; n]));
    let indices: Vec<u32> = match mesh.try_indices_option().ok()? {
        Some(i) => i.iter().map(|i| i as u32).collect(),
        None => (0..n as u32).collect(),
    };
    let mut out = Mesh::new(PrimitiveTopology::TriangleList, bevy::asset::RenderAssetUsages::RENDER_WORLD);
    out.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    out.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    out.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    out.insert_indices(Indices::U32(indices));
    if out.generate_tangents().is_err() {
        out.insert_attribute(Mesh::ATTRIBUTE_TANGENT, vec![[1.0f32, 0.0, 0.0, 1.0]; n]);
    }
    out.enable_raytracing = true;
    Some((out, size))
}

/// A prop too small to matter to Low's traced light (metres across).
const LOW_MIN_SIZE: f32 = 2.0;

#[allow(clippy::type_complexity)]
fn convert(
    mut commands: Commands,
    mut converted: ResMut<Converted>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    world_materials: Res<Assets<crate::world::WorldMaterial>>,
    world: Query<
        (Entity, &Mesh3d, &MeshMaterial3d<crate::world::WorldMaterial>, Option<&RenderLayers>),
        (Without<Checked>, Without<SkinnedMesh>),
    >,
    standard: Query<
        (Entity, &Mesh3d, &MeshMaterial3d<StandardMaterial>, Option<&RenderLayers>),
        (Without<Checked>, Without<SkinnedMesh>, Without<RaytracingMesh3d>),
    >,
) {
    let mode = lighting();
    if mode == Lighting::Baked || debug("noconvert") {
        return;
    }
    let mut targets: Vec<(Entity, Handle<Mesh>, UntypedAssetId, Option<StandardMaterial>, bool)> = Vec::new();
    for (e, mesh, mat, layers) in &world {
        commands.entity(e).insert(Checked);
        if layers.is_some_and(|l| !l.intersects(&RenderLayers::layer(0))) {
            continue;
        }
        let base = world_materials.get(&mat.0).map(|m| m.base.clone());
        targets.push((e, mesh.0.clone(), mat.0.id().untyped(), base, true));
    }
    for (e, mesh, mat, layers) in &standard {
        commands.entity(e).insert(Checked);
        if layers.is_some_and(|l| !l.intersects(&RenderLayers::layer(0))) {
            continue;
        }
        targets.push((e, mesh.0.clone(), mat.0.id().untyped(), materials.get(&mat.0).cloned(), false));
    }
    for (e, mesh, mat_id, base, is_world) in targets {
        let Some(base) = base else { continue };
        let material = converted.materials.entry(mat_id).or_insert_with(|| deferred(&base, &mut materials)).clone();
        let Some((material, masked)) = material else { continue };
        let mut entity = commands.entity(e);
        if is_world {
            entity.remove::<(MeshMaterial3d<crate::world::WorldMaterial>, Lightmap)>();
        }
        entity.insert(MeshMaterial3d(material));
        if masked {
            continue;
        }
        let traced = converted
            .meshes
            .entry(mesh.id())
            .or_insert_with(|| {
                let Some(m) = meshes.get(&mesh) else {
                    warn!("rt: mesh missing");
                    return None;
                };
                let Some((t, size)) = traceable(m) else {
                    warn!("rt: not traceable: extracted {}", m.try_attribute_option(Mesh::ATTRIBUTE_POSITION).is_err());
                    return None;
                };
                if mode == Lighting::RayTracedLow && !is_world && size < LOW_MIN_SIZE {
                    return None;
                }
                Some(meshes.add(t))
            })
            .clone();
        if let Some(t) = traced {
            commands.entity(e).insert(RaytracingMesh3d(t));
            converted.traced += 1;
        }
        converted.drawn += 1;
    }
    if converted.drawn > 0 && converted.logged != (converted.drawn, converted.traced) {
        converted.logged = (converted.drawn, converted.traced);
        info!("ray-traced lighting: converting {} surfaces, {} traced", converted.drawn, converted.traced);
    }
}

/// The player's camera traces; the sun stops casting shadow maps.
#[allow(clippy::type_complexity)]
fn set_up_view(
    mut commands: Commands,
    cameras: Query<Entity, (With<SlotCamera>, Without<SolariLighting>)>,
    mut suns: Query<&mut DirectionalLight, Without<crate::model_lighting::ViewModelSun>>,
) {
    if lighting() == Lighting::Baked {
        return;
    }
    for e in cameras.iter().filter(|_| !debug("nocamera")) {
        commands
            .entity(e)
            .insert((SolariLighting::default(), CameraMainTextureUsages::default().with(TextureUsages::STORAGE_BINDING)))
            .remove::<ScreenSpaceAmbientOcclusion>();
    }
    for mut sun in &mut suns {
        if sun.shadow_maps_enabled {
            sun.shadow_maps_enabled = false;
        }
    }
}

/// The sky's light: an emissive dome over the map that only the rays see.
/// Its radiance is the skybox's (`player::SKY_BRIGHTNESS` for white) times
/// the sky's colour.
fn spawn_sky_dome(
    mut commands: Commands,
    mut converted: ResMut<Converted>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    info: Option<Res<crate::world::MapInfo>>,
) {
    let mode = lighting();
    if mode == Lighting::Baked || converted.dome || info.is_none() {
        return;
    }
    converted.dome = true;
    let (rings, segments) = if mode == Lighting::RayTracedHigh { (16, 48) } else { (8, 24) };
    let radius = 600.0;
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    for r in 0..=rings {
        // From the zenith to a little below the horizon.
        let theta = r as f32 / rings as f32 * (std::f32::consts::FRAC_PI_2 + 0.15);
        for s in 0..=segments {
            let phi = s as f32 / segments as f32 * std::f32::consts::TAU;
            let d = Vec3::new(theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin());
            positions.push((d * radius).to_array());
            normals.push((-d).to_array());
        }
    }
    let mut indices = Vec::new();
    let row = segments + 1;
    for r in 0..rings {
        for s in 0..segments {
            let (a, b) = (r * row + s, (r + 1) * row + s);
            // Facing in.
            indices.extend_from_slice(&[a, a + 1, b, a + 1, b + 1, b]);
        }
    }
    let n = positions.len();
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, bevy::asset::RenderAssetUsages::RENDER_WORLD);
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0f32, 0.0]; n]);
    mesh.insert_attribute(Mesh::ATTRIBUTE_TANGENT, vec![[1.0f32, 0.0, 0.0, 1.0]; n]);
    mesh.insert_indices(Indices::U32(indices));
    mesh.enable_raytracing = true;
    let sky = LinearRgba::from(SKY_COLOUR) * SKY_RADIANCE;
    let material = materials.add(StandardMaterial {
        base_color: Color::BLACK,
        emissive: sky,
        unlit: true,
        opaque_render_method: OpaqueRendererMethod::Deferred,
        ..default()
    });
    commands.spawn((Name::new("sky light dome"), RaytracingMesh3d(meshes.add(mesh)), MeshMaterial3d(material), Transform::default()));
}

/// The sky light's colour and radiance (cd/m^2): a hazy daylight sky,
/// matched by eye to the baked lighting's shade.
const SKY_COLOUR: Color = Color::srgb(0.62, 0.7, 0.82);
const SKY_RADIANCE: f32 = 1500.0;

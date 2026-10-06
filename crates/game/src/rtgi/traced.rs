//! The Solari side of ray-traced lighting ([`super`]), built with
//! `--features raytracing`.

use super::{Lighting, SUPPORTED, lighting};
use crate::splitscreen::SlotCamera;
use bevy::camera::CameraMainTextureUsages;
use bevy::camera::visibility::RenderLayers;
use bevy::asset::UntypedAssetId;
use bevy::material::OpaqueRendererMethod;
use bevy::mesh::skinning::SkinnedMesh;
use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
use bevy::pbr::{DefaultOpaqueRendererMethod, ScreenSpaceAmbientOcclusion};
use bevy::prelude::*;
use bevy::render::render_resource::TextureUsages;
use bevy::render::renderer::RenderDevice;
use bevy::solari::prelude::{RaytracingMesh3d, SolariLighting, SolariPlugins};
use std::collections::HashMap;
use std::sync::atomic::Ordering;

pub(super) fn build(app: &mut App) {
    app.add_plugins(SolariPlugins)
        // Solari makes every material deferred by default; only the ones
        // converted here are (the rest keep their forward shaders, the
        // world's lightmaps among them).
        .insert_resource(DefaultOpaqueRendererMethod::forward())
        .init_resource::<Converted>()
        .add_systems(OnEnter(crate::state::GameState::InGame), reset.in_set(crate::state::Setup::Content))
        .add_systems(Update, (set_up_view, spawn_sky_dome).run_if(crate::state::in_game))
        .init_resource::<TraceCopies>()
        .add_systems(
            PostUpdate,
            (copy_meshes.after(bevy::asset::AssetEventSystems), convert.run_if(crate::state::in_game))
                .chain()
                .after(bevy::transform::TransformSystems::Propagate),
        );
}

/// Whether the GPU can trace rays, known once the renderer is up (before
/// the menus are built: their Lighting row depends on it).
pub(super) fn check_support(app: &App) {
    let device = app.world().get_resource::<RenderDevice>();
    let ok = device.is_some_and(|d| d.features().contains(SolariPlugins::required_wgpu_features()));
    SUPPORTED.store(ok, Ordering::Relaxed);
    info!("ray-traced lighting {}", if ok { "available" } else { "unavailable: the GPU lacks ray queries" });
}

/// Each mesh's copy in the form Solari traces, and its size, made as the
/// mesh is (the game's meshes live only in the render world once drawn).
#[derive(Resource, Default)]
struct TraceCopies(HashMap<AssetId<Mesh>, (Handle<Mesh>, f32, u32, Vec3)>, std::collections::HashSet<AssetId<Mesh>>);

fn copy_meshes(mut events: MessageReader<AssetEvent<Mesh>>, mut meshes: ResMut<Assets<Mesh>>, mut copies: ResMut<TraceCopies>) {
    let tracing = lighting() != Lighting::Baked;
    let added: Vec<AssetId<Mesh>> = events
        .read()
        .filter_map(|e| match e {
            AssetEvent::Added { id } => Some(*id),
            AssetEvent::Removed { id } | AssetEvent::Unused { id } => {
                copies.0.remove(id);
                None
            }
            _ => None,
        })
        .collect();
    if !tracing {
        return;
    }
    for id in added {
        if copies.0.contains_key(&id) || copies.1.contains(&id) {
            continue;
        }
        let Some((copy, size, triangles, centre)) = meshes.get(id).and_then(traceable) else { continue };
        let handle = meshes.add(copy);
        copies.1.insert(handle.id());
        copies.0.insert(id, (handle, size, triangles, centre));
    }
}

/// How much of the baked light the traced light sits on (1: all of it):
/// world surfaces draw their lightmap into Solari's G-buffer
/// (`shaders/world_deferred.wgsl`), which Solari adds to what it traces, so
/// rooms never drop below their baked look.
const LIGHTMAP_FLOOR: f32 = 1.0;

/// A viewmodel camera set up to share the traced image.
#[derive(Component)]
struct Traced;

/// A mesh entity looked at for ray tracing.
#[derive(Component)]
struct Checked;

/// Materials and meshes already converted, this match.
#[derive(Resource, Default)]
struct Converted {
    /// World materials' deferred copies.
    world_materials: HashMap<AssetId<crate::world::WorldMaterial>, Handle<crate::world::WorldMaterial>>,
    /// Props' drawn materials with their light floor, by material and floor.
    floored: HashMap<(UntypedAssetId, [i16; 3]), Handle<StandardMaterial>>,
    /// Each material's deferred copy, and whether it's alpha-tested.
    materials: HashMap<UntypedAssetId, Option<(Handle<StandardMaterial>, bool, bool)>>,
    meshes: HashMap<(AssetId<Mesh>, bool), Option<Handle<Mesh>>>,
    dome: bool,
    drawn: usize,
    traced: usize,
    logged: (usize, usize),
}

fn reset(mut converted: ResMut<Converted>) {
    *converted = Converted::default();
}

/// A deferred copy of a material for Solari's G-buffer: `None` for
/// translucent ones (they stay as they are), whether it's alpha-tested,
/// and whether it glows. CoD4's opaque unlit surfaces are lit-up fixtures
/// (lamps' bulbs, light boxes, screens), whose light the lightmaps baked
/// in: here they shine ([`FIXTURE_RADIANCE`]), lighting rooms the sun and
/// sky don't reach.
fn deferred(base: &StandardMaterial, materials: &mut Assets<StandardMaterial>) -> Option<(Handle<StandardMaterial>, bool, bool)> {
    let masked = match base.alpha_mode {
        AlphaMode::Opaque => false,
        AlphaMode::Mask(_) => true,
        _ => return None,
    };
    let glows = base.unlit && !masked;
    let m = StandardMaterial {
        emissive: if glows { LinearRgba::from(base.base_color) * FIXTURE_RADIANCE } else { LinearRgba::BLACK },
        emissive_texture: if glows { base.base_color_texture.clone() } else { None },
        opaque_render_method: OpaqueRendererMethod::Deferred,
        unlit: false,
        normal_map_texture: None,
        // CoD4's surfaces are matte: no mirror-like reflections of the sky.
        perceptual_roughness: 1.0,
        reflectance: 0.0,
        metallic: 0.0,
        ..base.clone()
    };
    Some((materials.add(m), masked, glows))
}

/// A prop's drawn material with its light floor (`floor`, the light grid's
/// light around it): emissive, the floor times its colour texture. Floors
/// are rounded to a quarter stop so props share materials. `None` for
/// glowing, translucent and alpha-tested ones (foliage with a floor came out
/// glowing white, for reasons not yet found).
fn floored(
    converted: &mut Converted,
    materials: &mut Assets<StandardMaterial>,
    mat_id: UntypedAssetId,
    base: &StandardMaterial,
    floor: Vec3,
) -> Option<Handle<StandardMaterial>> {
    if base.unlit || base.alpha_mode != AlphaMode::Opaque {
        return None;
    }
    let level = |v: f32| if v <= 0.0 { i16::MIN } else { (v.log2() * 4.0).round() as i16 };
    let key = (mat_id, [level(floor.x), level(floor.y), level(floor.z)]);
    let handle = converted.floored.entry(key).or_insert_with(|| {
        let floor = Vec3::from_array(key.1.map(|l| if l == i16::MIN { 0.0 } else { (l as f32 / 4.0).exp2() }));
        let colour = LinearRgba::from(base.base_color);
        materials.add(StandardMaterial {
            emissive: LinearRgba::rgb(floor.x * colour.red, floor.y * colour.green, floor.z * colour.blue) * LIGHT_GRID_FLOOR,
            emissive_texture: base.base_color_texture.clone(),
            opaque_render_method: OpaqueRendererMethod::Deferred,
            normal_map_texture: None,
            perceptual_roughness: 1.0,
            reflectance: 0.0,
            metallic: 0.0,
            ..base.clone()
        })
    });
    Some(handle.clone())
}

/// How much of the light grid's light props' traced light sits on.
const LIGHT_GRID_FLOOR: f32 = 1.0;

/// A fixture's radiance (cd/m^2) for a white texel.
const FIXTURE_RADIANCE: f32 = 4000.0;

/// Solari's most triangles in a glowing mesh.
const MAX_GLOWING_TRIANGLES: u32 = 65_535;

/// The mesh in the form Solari traces, and its size (metres across). The
/// game's meshes live only in the render world once drawn, so this works
/// in the frame they're made ([`convert`] runs in `PostUpdate`).
fn traceable(mesh: &Mesh) -> Option<(Mesh, f32, u32, Vec3)> {
    if mesh.primitive_topology() != PrimitiveTopology::TriangleList {
        return None;
    }
    let positions = mesh.try_attribute_option(Mesh::ATTRIBUTE_POSITION).ok()??.clone();
    let n = positions.len();
    let (size, centre) = match &positions {
        VertexAttributeValues::Float32x3(p) => {
            let (lo, hi) = p.iter().fold((Vec3::MAX, Vec3::MIN), |(lo, hi), v| (lo.min(Vec3::from(*v)), hi.max(Vec3::from(*v))));
            ((hi - lo).length(), (lo + hi) / 2.0)
        }
        _ => (0.0, Vec3::ZERO),
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
    let triangles = (out.try_indices_option().ok().flatten().map_or(0, |i| i.len()) / 3) as u32;
    Some((out, size, triangles, centre))
}

/// A prop too small to matter to Low's traced light (metres across).
const LOW_MIN_SIZE: f32 = 2.0;

#[allow(clippy::type_complexity)]
fn convert(
    mut commands: Commands,
    mut converted: ResMut<Converted>,
    copies: Res<TraceCopies>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut world_materials: ResMut<Assets<crate::world::WorldMaterial>>,
    world: Query<
        (Entity, &Mesh3d, &MeshMaterial3d<crate::world::WorldMaterial>, Option<&RenderLayers>, Option<&ChildOf>),
        (Without<Checked>, Without<SkinnedMesh>),
    >,
    grid: Option<Res<crate::model_lighting::LightGridLookup>>,
    standard: Query<
        (Entity, &Mesh3d, &MeshMaterial3d<StandardMaterial>, Option<&RenderLayers>, &GlobalTransform),
        (Without<Checked>, Without<SkinnedMesh>, Without<RaytracingMesh3d>),
    >,
) {
    let mode = lighting();
    if mode == Lighting::Baked {
        return;
    }
    let mut targets: Vec<(Entity, Handle<Mesh>, UntypedAssetId, Option<StandardMaterial>, bool)> = Vec::new();
    let mut floors: HashMap<Entity, Vec3> = HashMap::new();
    for (e, mesh, mat, layers, parent) in &world {
        commands.entity(e).insert(Checked);
        if layers.is_some_and(|l| !l.intersects(&RenderLayers::layer(0))) {
            continue;
        }
        let Some(world_mat) = world_materials.get(&mat.0).cloned() else { continue };
        if !matches!(world_mat.base.alpha_mode, AlphaMode::Opaque | AlphaMode::Mask(_)) {
            continue;
        }
        // The surface draws deferred with its own material (normal maps and
        // the baked floor, `shaders/world_deferred.wgsl`); a separate entity
        // carries what Solari traces, which must be a standard material.
        let drawn = converted
            .world_materials
            .entry(mat.0.id())
            .or_insert_with(|| {
                let mut m = world_mat.clone();
                m.base.opaque_render_method = OpaqueRendererMethod::Deferred;
                m.extension.params.traced.x = LIGHTMAP_FLOOR;
                world_materials.add(m)
            })
            .clone();
        commands.entity(e).insert(MeshMaterial3d(drawn));
        let mut traced = commands.spawn((Transform::default(), Checked));
        if let Some(parent) = parent {
            traced.insert(ChildOf(parent.parent()));
        }
        targets.push((traced.id(), mesh.0.clone(), mat.0.id().untyped(), Some(world_mat.base), true));
    }
    for (e, mesh, mat, layers, at) in &standard {
        commands.entity(e).insert(Checked);
        if layers.is_some_and(|l| !l.intersects(&RenderLayers::layer(0))) {
            continue;
        }
        // Lit from the light grid at its middle, as CoD4 lit static models.
        let centre = copies.0.get(&mesh.id()).map_or(Vec3::ZERO, |c| c.3);
        if let Some(grid) = &grid {
            floors.insert(e, grid.at(at.transform_point(centre)));
        }
        targets.push((e, mesh.0.clone(), mat.0.id().untyped(), materials.get(&mat.0).cloned(), false));
    }
    for (e, mesh, mat_id, base, is_world) in targets {
        let Some(base) = base else { continue };
        let material = converted.materials.entry(mat_id).or_insert_with(|| deferred(&base, &mut materials)).clone();
        let Some((material, masked, glows)) = material else { continue };
        // Props: drawn with their light floor (as emissive: the floor times
        // their colour), traced by a child without it (Solari would make an
        // emissive material a light).
        let e = if is_world {
            commands.entity(e).insert(MeshMaterial3d(material.clone()));
            e
        } else {
            let floor = floors.get(&e).copied().unwrap_or(Vec3::ZERO);
            let drawn = floored(&mut converted, &mut materials, mat_id, &base, floor);
            commands.entity(e).insert(MeshMaterial3d(drawn.unwrap_or_else(|| material.clone())));
            if masked {
                continue;
            }
            commands.spawn((Transform::default(), ChildOf(e), Checked, MeshMaterial3d(material.clone()))).id()
        };
        if masked {
            commands.entity(e).despawn();
            continue;
        }
        let traced = converted
            .meshes
            .entry((mesh.id(), glows))
            .or_insert_with(|| {
                let (t, size, triangles, _) = copies.0.get(&mesh.id())?.clone();
                if mode == Lighting::RayTracedLow && !is_world && size < LOW_MIN_SIZE && !glows {
                    return None;
                }
                if glows && triangles > MAX_GLOWING_TRIANGLES {
                    return None;
                }
                Some(t)
            })
            .clone();
        if let Some(t) = traced {
            commands.entity(e).insert(RaytracingMesh3d(t));
            converted.traced += 1;
        } else {
            commands.entity(e).despawn();
        }
        converted.drawn += 1;
    }
    if converted.drawn >= converted.logged.0 + 100 || converted.logged.0 == 0 && converted.drawn > 0 {
        converted.logged = (converted.drawn, converted.traced);
        info!("ray-traced lighting: converting {} surfaces, {} traced", converted.drawn, converted.traced);
    }
}

/// The player's camera traces; the sun stops casting shadow maps.
#[allow(clippy::type_complexity)]
fn set_up_view(
    mut commands: Commands,
    cameras: Query<Entity, (With<SlotCamera>, Without<SolariLighting>)>,
    viewmodel_cameras: Query<Entity, (With<crate::splitscreen::SlotViewModelCamera>, Without<Traced>)>,
    mut suns: Query<&mut DirectionalLight, Without<crate::model_lighting::ViewModelSun>>,
) {
    if lighting() == Lighting::Baked {
        return;
    }
    for e in &cameras {
        commands
            .entity(e)
            .insert((SolariLighting::default(), CameraMainTextureUsages::default().with(TextureUsages::STORAGE_BINDING)))
            .remove::<ScreenSpaceAmbientOcclusion>();
    }
    // The viewmodel camera draws into the same image, which it shares only
    // if its usages match.
    for e in &viewmodel_cameras {
        commands.entity(e).insert((Traced, CameraMainTextureUsages::default().with(TextureUsages::STORAGE_BINDING)));
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
const SKY_RADIANCE: f32 = 3000.0;

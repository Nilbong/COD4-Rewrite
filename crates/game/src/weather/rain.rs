//! The rain itself: streaks and splashes, each one draw of many quads whose
//! places the vertex shader (`rain.wgsl`) works out from the clock, the
//! camera and the rain map ([`super::occlusion`]), so the CPU only sets a
//! few numbers a frame.
//!
//! Streaks fill a box round the camera that the drops fall through and
//! wrap in (so they stay put in the world as it moves), slanted by the
//! wind and stretched along their fall as a camera's shutter would blur
//! them; past the box, fainter and longer, a far layer reads as sheets.
//! Splashes come and go where the rain lands. All are lit by the light
//! where the camera is (the light grid), the sun's colour and lightning.

use bevy::mesh::{Indices, MeshVertexBufferLayoutRef, PrimitiveTopology};
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, RenderPipelineDescriptor, SpecializedMeshPipelineError};
use bevy::shader::ShaderRef;
use bevy::asset::RenderAssetUsages;

/// Quads drawn at full rain: near streaks, far streaks, splashes, drips.
const NEAR: u32 = 14000;
const FAR: u32 = 5000;
const SPLASHES: u32 = 2500;
const DRIPS: u32 = 1500;
/// Distant rain curtains: big soft sheets in a ring 30-150 m out.
const CURTAINS: u32 = 48;
/// Where `rain.wgsl` is registered in the `embedded://` asset source.
const SHADER: &str = "cod4rw/rain.wgsl";

pub(super) fn build(app: &mut App) {
    app.world().resource::<bevy::asset::io::embedded::EmbeddedAssetRegistry>().insert_asset(
        std::path::PathBuf::from(file!()).with_file_name("rain.wgsl"),
        std::path::Path::new(SHADER),
        include_bytes!("rain.wgsl").as_slice(),
    );
    app.add_plugins(MaterialPlugin::<RainMaterial>::default())
        .add_systems(Update, (spawn, drive).chain().run_if(crate::state::in_game).run_if(crate::atmos::climate::on));
}

/// Rain quads: streaks (`kind.x` 0 near, 1 far), splashes (2) or drips (3).
#[derive(Asset, AsBindGroup, TypePath, Debug, Clone)]
pub struct RainMaterial {
    /// xyz: the box's centre (the camera), w: the clock (seconds).
    #[uniform(0)]
    pub centre: Vec4,
    /// xyz: the box's half size (metres), w: share of quads drawn (rain).
    #[uniform(1)]
    pub extent: Vec4,
    /// xyz: the drops' velocity (m/s), w: streak length per m/s.
    #[uniform(2)]
    pub velocity: Vec4,
    /// rgb: light (HDR), w: lightning's share of it.
    #[uniform(3)]
    pub light: Vec4,
    /// x: kind, y: the rain map's cell size, z: its cells a side, w: unused.
    #[uniform(4)]
    pub kind: Vec4,
    /// The rain map's heights.
    #[texture(5, sample_type = "float", filterable = false)]
    pub heights: Handle<Image>,
    /// xyz: toward the key light (the sun, or the moon by night); w: unused.
    #[uniform(6)]
    pub key_dir: Vec4,
    /// rgb: the key light the drops scatter forward (HDR, as `light`).
    #[uniform(7)]
    pub key_light: Vec4,
}

impl Material for RainMaterial {
    fn vertex_shader() -> ShaderRef {
        "embedded://cod4rw/rain.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "embedded://cod4rw/rain.wgsl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Add
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
        layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.vertex.buffers = vec![layout.0.get_layout(&[Mesh::ATTRIBUTE_POSITION.at_shader_location(0)])?];
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

/// `count` quads: each vertex is (corner x, corner y, quad number).
fn quads(count: u32) -> Mesh {
    let mut positions = Vec::with_capacity(count as usize * 4);
    let mut indices = Vec::with_capacity(count as usize * 6);
    for i in 0..count {
        for (x, y) in [(-1.0, 0.0), (1.0, 0.0), (1.0, 1.0), (-1.0, 1.0)] {
            positions.push([x, y, i as f32]);
        }
        let b = i * 4;
        indices.extend([b, b + 1, b + 2, b, b + 2, b + 3]);
    }
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

/// The rain's draws (one per kind).
#[derive(Component)]
struct RainDraw(u32);

fn spawn(
    mut commands: Commands,
    existing: Query<(), With<RainDraw>>,
    map: Option<Res<super::occlusion::RainMap>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<RainMaterial>>,
) {
    let Some(map) = map else { return };
    if !existing.is_empty() {
        return;
    }
    for (kind, count) in [(0, NEAR), (1, FAR), (2, SPLASHES), (3, DRIPS), (4, CURTAINS)] {
        let material = materials.add(RainMaterial {
            centre: Vec4::ZERO,
            extent: Vec4::ZERO,
            velocity: Vec4::ZERO,
            light: Vec4::ZERO,
            kind: Vec4::new(kind as f32, super::occlusion::CELL, super::occlusion::CELLS as f32, 0.0),
            heights: map.image.clone(),
            key_dir: Vec4::Y,
            key_light: Vec4::ZERO,
        });
        commands.spawn((
            Name::new("rain"),
            RainDraw(kind),
            Mesh3d(crate::mesh_bounds::add(&mut meshes, quads(count))),
            MeshMaterial3d(material),
            Transform::default(),
            bevy::camera::visibility::NoFrustumCulling,
            bevy::light::NotShadowCaster,
        ));
    }
}

/// How fast drops fall (m/s).
const FALL: f32 = 9.0;

fn drive(
    time: Res<Time>,
    storm: Res<super::Storm>,
    camera: Query<&GlobalTransform, With<crate::player::MainCamera>>,
    grid: Option<Res<crate::model_lighting::LightGridLookup>>,
    tod: Option<Res<crate::atmos::climate::TimeOfDay>>,
    sun: Query<(&DirectionalLight, &GlobalTransform), Without<crate::model_lighting::ViewModelSun>>,
    draws: Query<(&RainDraw, &MeshMaterial3d<RainMaterial>)>,
    mut materials: ResMut<Assets<RainMaterial>>,
) {
    let Some(eye) = camera.iter().next().map(|c| c.translation()) else { return };
    let t = time.elapsed_secs_wrapped();
    // The light drops catch: the air's light where the camera is, a little
    // of the sun's colour, and lightning's flash, much brighter.
    let ambient = grid.as_ref().map_or(Vec3::splat(0.4), |g| g.at(eye)).max(Vec3::splat(0.02));
    let key = sun.iter().max_by(|a, b| a.0.illuminance.total_cmp(&b.0.illuminance));
    let sun = key.map_or(Vec3::ONE, |s| s.0.color.to_linear().to_vec3());
    let flash = storm.flash;
    // With the showcase's clock running, the baked light follows the live
    // sky (dark at night) and takes its tint.
    let sky = tod.as_ref().filter(|t| t.enabled).and_then(|t| {
        let map = t.map.as_ref()?;
        let scale = (t.sky_illuminance / map.ambient.max(1e-3)).clamp(0.0, 4.0);
        // On a map CoD4 lit by night (moonlight key, as Wet Work), the
        // night's air is at least as light as CoD4's (`atmos::daylight`).
        let night_map = map.color.z > map.color.x * 1.15;
        let scale = if night_map { scale.max(t.night) } else { scale };
        Some(ambient * scale * (0.7 + 0.3 * t.sky_color))
    });
    let ambient = sky.unwrap_or(ambient * (0.8 + 0.2 * sun));
    let light = ambient * 1.4 + Vec3::new(0.8, 0.85, 1.0) * flash * 12.0;
    // The key light (the moon by night) the drops scatter toward the eye,
    // as bright as the map's own light makes it, relative to the air's.
    let full = tod.as_ref().and_then(|t| t.map.as_ref()).map_or(10_000.0, |m| m.illuminance.max(1.0));
    let (key_dir, key_light) = key.map_or((Vec3::Y, Vec3::ZERO), |(l, tf)| (-tf.forward().as_vec3(), l.color.to_linear().to_vec3() * (l.illuminance / full).min(1.5)));
    let velocity = Vec3::new(storm.wind.x, -FALL, storm.wind.y);
    for (draw, handle) in &draws {
        let Some(mut m) = materials.get_mut(&handle.0) else { continue };
        let (extent, share) = match draw.0 {
            0 => (Vec3::new(14.0, 9.0, 14.0), storm.rain),
            1 => (Vec3::new(38.0, 16.0, 38.0), storm.rain * storm.rain),
            2 => (Vec3::new(11.0, 0.0, 11.0), storm.rain),
            3 => (Vec3::new(9.0, 0.0, 9.0), storm.rain.sqrt()),
            // (Off: over open sea they read as a haze in the water.)
            _ => (Vec3::new(30.0, 150.0, 0.0), 0.0),
        };
        m.centre = eye.extend(t);
        m.extent = extent.extend(share);
        m.velocity = velocity.extend(0.03);
        m.light = light.extend(flash);
        m.key_dir = key_dir.extend(0.0);
        // The curtains' strength: full by night, a third by day (the day's
        // light is far brighter, and a haze by day is greyer, not lit).
        if draw.0 == 4 {
            m.kind.w = 0.3 + 0.7 * tod.as_ref().filter(|t| t.enabled).map_or(1.0, |t| t.night);
            // Debug: `COD4RW_NOCURTAINS` leaves them out (for A/B).
            if std::env::var_os("COD4RW_NOCURTAINS").is_some() {
                m.kind.w = 0.0;
            }
        }
        m.key_light = key_light.extend(0.0);
    }
}

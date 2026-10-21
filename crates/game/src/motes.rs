//! Airborne particles, drawn as `crate::fire`'s embers are (one draw of many
//! quads, each placed by the vertex shader, `motes.wgsl`, from the clock; no
//! CPU work per particle):
//!
//! - Dust motes: specks drifting in a box round the eye, lit only where
//!   the sun reaches them (its shadow map) and brightest looking toward it
//!   (forward scatter), so they show in shafts of sunlight and vanish in
//!   shade. Knob `motes.dust` (1; per map `motes.dust.<map>`).
//! - Leaves: small tumbling leaf shapes blowing past on the wind, on maps
//!   with trees (static models named `tree`), tinted a dry olive-brown.
//!   Knob `motes.leaves` (1).
//!
//! `COD4RW_MOTES=0` leaves them out; `COD4RW_MOTES_AB=1` with
//! `COD4RW_SHOT_VIEWS` leaves them out on the even shots (same-view pairs).

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, MeshVertexBufferLayoutRef, PrimitiveTopology};
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, RenderPipelineDescriptor, SpecializedMeshPipelineError};
use bevy::shader::ShaderRef;

pub struct MotesPlugin;

impl Plugin for MotesPlugin {
    fn build(&self, app: &mut App) {
        if std::env::var("COD4RW_MOTES").is_ok_and(|v| v == "0") {
            return;
        }
        app.world().resource::<bevy::asset::io::embedded::EmbeddedAssetRegistry>().insert_asset(
            std::path::PathBuf::from(file!()).with_file_name("motes.wgsl"),
            std::path::Path::new("cod4rw/motes.wgsl"),
            include_bytes!("motes.wgsl").as_slice(),
        );
        app.add_plugins(MaterialPlugin::<MoteMaterial>::default()).add_systems(Update, drive.run_if(crate::state::in_game));
    }
}

/// Specks and leaves drawn at most.
const DUST: u32 = 5000;
const LEAVES: u32 = 400;

#[derive(Asset, AsBindGroup, TypePath, Debug, Clone)]
pub struct MoteMaterial {
    /// x: clock, y: share drawn, z: kind (0 dust, 1 leaves), w: the sun's
    /// light on them (0..).
    #[uniform(0)]
    pub params: Vec4,
    /// xy: wind (m/s), z: rain 0..1, w: unused.
    #[uniform(1)]
    pub air: Vec4,
    /// xyz: toward the sun; w: unused.
    #[uniform(2)]
    pub sun: Vec4,
    /// rgb: the sun's colour; w: unused.
    #[uniform(3)]
    pub sun_color: Vec4,
}

impl Material for MoteMaterial {
    fn vertex_shader() -> ShaderRef {
        "embedded://cod4rw/motes.wgsl".into()
    }
    fn fragment_shader() -> ShaderRef {
        "embedded://cod4rw/motes.wgsl".into()
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

fn quads(count: u32) -> Mesh {
    let mut positions = Vec::with_capacity(count as usize * 4);
    let mut indices = Vec::with_capacity(count as usize * 6);
    for i in 0..count {
        for (x, y) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
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

#[derive(Component)]
struct MoteDraw(u32);

fn ab_on(time: &Time) -> bool {
    if std::env::var("COD4RW_MOTES_AB").map_or(true, |v| v != "1") {
        return true;
    }
    let late: f32 = std::env::var("COD4RW_SHOT_LATE").ok().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let k = (((time.elapsed_secs() - late - 5.0) / 1.5).floor().max(0.0)) as usize;
    k % 2 == 1
}

#[allow(clippy::too_many_arguments)]
fn drive(
    mut commands: Commands,
    time: Res<Time>,
    storm: Option<Res<crate::weather::Storm>>,
    content: Option<Res<crate::content::Content>>,
    sun: Query<(&DirectionalLight, &GlobalTransform, &Name), Without<crate::model_lighting::ViewModelSun>>,
    mut draws: Query<(&MoteDraw, &MeshMaterial3d<MoteMaterial>, &mut Visibility)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<MoteMaterial>>,
    mut trees: Local<Option<(u32, bool)>>,
) {
    if draws.is_empty() {
        for (kind, count) in [(0, DUST), (1, LEAVES)] {
            let material = materials.add(MoteMaterial { params: Vec4::ZERO, air: Vec4::ZERO, sun: Vec4::Y, sun_color: Vec4::ONE });
            commands.spawn((
                Name::new("motes"),
                MoteDraw(kind),
                Mesh3d(crate::mesh_bounds::add(&mut meshes, quads(count))),
                MeshMaterial3d(material),
                Transform::default(),
                bevy::camera::visibility::NoFrustumCulling,
                bevy::light::NotShadowCaster,
            ));
        }
        return;
    }
    // This map's trees (once a match).
    let id = crate::atmos::climate::match_id();
    if trees.is_none_or(|(m, _)| m != id) {
        let has = content.as_ref().and_then(|c| {
            let zone = c.map();
            let w = zone.gfx_world()?;
            Some(w.static_models.iter().any(|sm| sm.model.and_then(|m| zone.xmodel(m)).is_some_and(|x| x.name.to_ascii_lowercase().contains("tree"))))
        });
        *trees = Some((id, has.unwrap_or(false)));
    }
    let has_trees = trees.is_some_and(|(_, t)| t);
    let map = crate::atmos::climate::map_name();
    let knob = |n: &str, d: f32| {
        let per = crate::tune::get(&format!("{n}.{map}"), f32::NAN);
        if per.is_nan() { crate::tune::get(n, d) } else { per }
    };
    let key = sun.iter().filter(|s| s.2.as_str() == "sun").max_by(|a, b| a.0.illuminance.total_cmp(&b.0.illuminance));
    let (to_sun, colour, lux) = key.map_or((Vec3::Y, Vec3::ONE, 0.0), |(l, g, _)| (g.back().as_vec3(), l.color.to_linear().to_vec3(), l.illuminance));
    let (wind, rain) = storm.as_ref().map_or((Vec2::new(1.0, 0.4), 0.0), |s| (s.wind, s.rain));
    let on = ab_on(&time);
    let t = time.elapsed_secs_wrapped();
    for (draw, handle, mut vis) in &mut draws {
        let share = match draw.0 {
            0 => knob("motes.dust", 1.0) * (1.0 - rain),
            _ => if has_trees { knob("motes.leaves", 1.0) * (1.0 - 0.7 * rain) } else { 0.0 },
        }
        .clamp(0.0, 1.0)
            * on as u32 as f32;
        let want = if share > 0.0 { Visibility::Visible } else { Visibility::Hidden };
        if *vis != want {
            *vis = want;
        }
        if share <= 0.0 {
            continue;
        }
        let Some(mut m) = materials.get_mut(&handle.0) else { continue };
        m.params = Vec4::new(t, share, draw.0 as f32, (lux / 10_000.0).clamp(0.0, 4.0));
        m.air = Vec4::new(wind.x, wind.y, rain, 0.0);
        m.sun = to_sun.extend(0.0);
        m.sun_color = colour.extend(1.0);
    }
}

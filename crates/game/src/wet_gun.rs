//! Rain on the gun in hand (and the arms holding it): out in the rain the
//! viewmodel gets wet, a darker, glossier film with beaded drops on what
//! faces up and now and then one running down; under cover it slowly
//! dries.
//!
//! Each viewmodel surface gets a thin transparent layer: the same mesh,
//! skinned to the same joints, drawn over it with [`WetLayer`]
//! (`wet_gun.wgsl`), so whatever the surface's own material (a camo, a
//! custom camo, the reflex's glass) it's wet the same way. Not a 3D scope's
//! eyepiece (`ScopeLens`): that's the scope's view.
//!
//! Only with the sky's weather on ([`crate::weather::Storm`]);
//! `COD4RW_GUNWET=<0..1>` holds how wet, for test shots.

use crate::gunmodel::{GunSurface, ScopeCrosshair, ScopeLens};
use crate::viewmodel::ViewModelRoot;
use bevy::camera::visibility::RenderLayers;
use bevy::light::NotShadowCaster;
use bevy::mesh::skinning::SkinnedMesh;
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;

const SHADER: &str = "cod4rw/wet_gun.wgsl";
/// Seconds of full rain to soak, and of cover to dry.
const SOAK: f32 = 10.0;
const DRY: f32 = 75.0;

pub struct WetGunPlugin;

impl Plugin for WetGunPlugin {
    fn build(&self, app: &mut App) {
        app.world().resource::<bevy::asset::io::embedded::EmbeddedAssetRegistry>().insert_asset(
            std::path::PathBuf::from(file!()).with_file_name("wet_gun.wgsl"),
            std::path::Path::new(SHADER),
            include_bytes!("wet_gun.wgsl").as_slice(),
        );
        app.add_plugins(MaterialPlugin::<WetLayer>::default());
        if !crate::atmos::climate::enabled() {
            return;
        }
        app.init_resource::<Wetness>().add_systems(
            Update,
            (soak, layer, drive).chain().after(crate::weapons::WeaponSet).run_if(crate::state::in_game),
        );
    }
}

/// The wet layer over a viewmodel surface.
#[derive(Asset, TypePath, AsBindGroup, Clone)]
pub struct WetLayer {
    /// x: how wet (0..1), y: how hard it rains (0..1), z: time (s), w: 0
    /// the gun, 1 the arms (cloth and leather: darker, less beading).
    #[uniform(0)]
    params: Vec4,
    /// Live-tuned strengths (`crate::tune`, `wetgun.*`): x the beads, y
    /// the film's darkening, z its shine, w the running drops.
    #[uniform(1)]
    knobs: Vec4,
}

impl Material for WetLayer {
    fn vertex_shader() -> ShaderRef {
        "embedded://cod4rw/wet_gun.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "embedded://cod4rw/wet_gun.wgsl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Premultiplied
    }

    fn enable_shadows() -> bool {
        false
    }

    fn enable_prepass() -> bool {
        false
    }
}

/// How wet the local player's gun is, and the layers' two materials.
#[derive(Resource, Default)]
struct Wetness {
    wet: f32,
    rain: f32,
    gun: Option<Handle<WetLayer>>,
    arms: Option<Handle<WetLayer>>,
}

/// The layer itself (a child of the surface it covers).
#[derive(Component)]
struct Layer;

/// A surface that has its layer.
#[derive(Component)]
struct Layered;

/// Wetter out in the rain, drier under cover.
fn soak(
    time: Res<Time>,
    storm: Option<Res<crate::weather::Storm>>,
    map: Option<Res<crate::weather::occlusion::RainMap>>,
    camera: Query<&GlobalTransform, With<crate::player::MainCamera>>,
    mut wetness: ResMut<Wetness>,
) {
    let rain = storm.map_or(0.0, |s| s.rain);
    let at = camera.iter().next().map(|c| c.translation());
    let open = at.zip(map.as_deref()).and_then(|(p, m)| m.exposed(p)).unwrap_or(false);
    let dt = time.delta_secs();
    let w = &mut wetness;
    w.rain = if open { rain } else { 0.0 };
    if let Some(v) = std::env::var("COD4RW_GUNWET").ok().and_then(|v| v.parse::<f32>().ok()) {
        w.wet = v.clamp(0.0, 1.0);
        w.rain = w.rain.max(rain);
        return;
    }
    if open && rain > 0.02 {
        w.wet = (w.wet + dt * rain / SOAK).min(rain.sqrt());
    } else {
        w.wet = (w.wet - dt / DRY).max(0.0);
    }
}

/// A layer over each viewmodel surface (but a 3D scope's eyepiece).
#[allow(clippy::type_complexity)]
fn layer(
    mut commands: Commands,
    mut wetness: ResMut<Wetness>,
    mut materials: ResMut<Assets<WetLayer>>,
    roots: Query<(), With<ViewModelRoot>>,
    surfaces: Query<
        (Entity, &ChildOf, &Mesh3d, &SkinnedMesh, Option<&RenderLayers>, Has<GunSurface>),
        (Without<Layered>, Without<Layer>, Without<ScopeLens>, Without<ScopeCrosshair>),
    >,
) {
    let mut handle = |arms: bool| {
        let slot = if arms { &mut wetness.arms } else { &mut wetness.gun };
        slot.get_or_insert_with(|| materials.add(WetLayer { params: Vec4::new(0.0, 0.0, 0.0, arms as u32 as f32), knobs: Vec4::ONE })).clone()
    };
    for (e, parent, mesh, skin, layers, gun) in &surfaces {
        if !roots.contains(parent.parent()) {
            continue;
        }
        let mut layer = commands.spawn((
            Layer,
            Mesh3d(mesh.0.clone()),
            MeshMaterial3d(handle(!gun)),
            skin.clone(),
            bevy::camera::visibility::NoFrustumCulling,
            NotShadowCaster,
            Transform::IDENTITY,
            Visibility::Hidden,
            ChildOf(e),
        ));
        if let Some(l) = layers {
            layer.insert(l.clone());
        }
        commands.entity(e).insert(Layered);
    }
}

/// The layers shown while there's anything wet, their materials kept up.
fn drive(
    time: Res<Time>,
    wetness: Res<Wetness>,
    mut materials: ResMut<Assets<WetLayer>>,
    mut layers: Query<&mut Visibility, With<Layer>>,
) {
    let show = wetness.wet > 0.002;
    let want = if show { Visibility::Inherited } else { Visibility::Hidden };
    for mut v in &mut layers {
        if *v != want {
            *v = want;
        }
    }
    if !show {
        return;
    }
    let t = time.elapsed_secs() % 1000.0;
    let tune = crate::tune::get;
    let knobs = Vec4::new(tune("wetgun.beads", 1.0), tune("wetgun.darken", 1.0), tune("wetgun.shine", 1.0), tune("wetgun.runs", 1.0));
    for (h, arms) in [(&wetness.gun, 0.0), (&wetness.arms, 1.0)] {
        if let Some(mut m) = h.as_ref().and_then(|h| materials.get_mut(h)) {
            m.params = Vec4::new(wetness.wet, wetness.rain, t, arms);
            m.knobs = knobs;
        }
    }
}

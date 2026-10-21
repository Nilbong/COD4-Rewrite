//! The maps' own water (`wc_water`) drawn by bevy_atmos's modern water:
//! depth colour and absorption, Fresnel, screen-space and sky reflections,
//! the sun's glitter, the bubble-lattice foam on shores and round anything
//! crossing it, rain rings. The CoD side stays here: which surfaces are
//! water ([`crate::world::MapWater`]), their colour from the IW3 material
//! (`waterColor`), the sky (the camera's skybox, rotated as it is), the
//! map's knobs from the Map Look Editor, and the weather.
//!
//! Each water entity holds every pool of one material (several, at their
//! own heights), so the modern water is drawn on that mesh itself
//! (`OwnMesh`), its waves in the normals. The showcase's ocean (Wet Work)
//! keeps its own water (`crate::ocean`).
//!
//! Knobs (per map, the Map Look Editor's Water tab): `water.modern.<map>`
//! (1; 0 the CoD water as before), `water.brightness`, `water.clarity`,
//! `water.waves`, `water.reflection`.

use bevy::prelude::*;
use bevy_atmos::modern_water::{AtmosWater, FadeEdges, OwnMesh, WaterDepth, WaterTint};

pub struct AtmosWaterPlugin;

impl Plugin for AtmosWaterPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Muddy>()
            .add_plugins((bevy_atmos::modern_water::ModernWaterPlugin, bevy_atmos::ssr::SsrPlugin))
            .add_systems(Update, (mark, bridge).chain().before(bevy_atmos::LookSet));
    }
}

/// How muddy the map's water is (IW3 `waterColor`'s alpha past 1, the
/// per-map look's muddy water): murkier modern water.
#[derive(Resource, Default)]
struct Muddy(f32);

/// Maps where the modern water is off unless the editor turns it on.
const NOT_YET: &[&str] = &["mp_bog"];

/// The showcase's ocean takes this map's water instead.
fn ocean() -> bool {
    crate::atmos::climate::profile().is_some_and(|p| p.ocean)
}

/// The camera, the sun and the map's water, for bevy_atmos.
#[allow(clippy::type_complexity)]
fn mark(
    mut commands: Commands,
    cameras: Query<Entity, (With<crate::player::MainCamera>, Without<bevy_atmos::AtmosCamera>)>,
    suns: Query<(Entity, &DirectionalLight, Has<bevy_atmos::AtmosSun>), Without<crate::model_lighting::ViewModelSun>>,
    waters: Query<(Entity, &MeshMaterial3d<crate::world::WorldMaterial>), (With<crate::world::MapWater>, Without<AtmosWater>)>,
    materials: Res<Assets<crate::world::WorldMaterial>>,
    mut muddy: ResMut<Muddy>,
) {
    for e in &cameras {
        commands.entity(e).insert(bevy_atmos::AtmosCamera);
    }
    // The sun: the brightest light that isn't the gun's own.
    if let Some((best, _, has)) = suns.iter().max_by(|a, b| a.1.illuminance.total_cmp(&b.1.illuminance)) {
        if !has {
            commands.entity(best).insert(bevy_atmos::AtmosSun);
        }
        for (e, _, has) in &suns {
            if has && e != best {
                commands.entity(e).remove::<bevy_atmos::AtmosSun>();
            }
        }
    }
    if ocean() {
        return;
    }
    for (e, handle) in &waters {
        let mut entity = commands.entity(e);
        // (CoD's water is a sheet a few centimetres over the ground: coloured
        // as if 0.6 m deep, no foam ringing every puddle's rim.)
        // (CoD fades its water's edges by the vertex colour's alpha.)
        entity.insert((AtmosWater, OwnMesh, WaterDepth(0.6), FadeEdges));
        // Its colour: IW3's `waterColor` (gamma), as the body's colour.
        if let Some(m) = materials.get(&handle.0) {
            let c = m.extension.params.water_color;
            muddy.0 = muddy.0.max((c.w - 1.0).clamp(0.0, 1.0));
            if c.w >= 1.0 {
                let lin = Color::srgb(c.x.clamp(0.0, 1.0), c.y.clamp(0.0, 1.0), c.z.clamp(0.0, 1.0)).to_linear();
                entity.insert(WaterTint(Color::linear_rgb(lin.red * 0.35, lin.green * 0.35, lin.blue * 0.35)));
            }
        }
    }
}

/// The map's knobs and weather into bevy_atmos's.
#[allow(clippy::type_complexity)]
fn bridge(
    mut commands: Commands,
    weather: Option<Res<crate::atmos::climate::Weather>>,
    mut atmos: ResMut<bevy_atmos::weather::Weather>,
    muddy: Res<Muddy>,
    grid: Option<Res<crate::model_lighting::LightGridLookup>>,
    camera: Query<&GlobalTransform, With<crate::player::MainCamera>>,
) {
    // The world is lit by its lightmaps, not Bevy's lights: the water's
    // body takes the light grid's light where the camera is (as the rain
    // does).
    if let (Some(grid), Some(eye)) = (grid.as_ref(), camera.iter().next()) {
        commands.insert_resource(bevy_atmos::modern_water::WaterAmbient(grid.at(eye.translation()).max(Vec3::splat(0.02))));
    }
    let map = crate::atmos::climate::map_name();
    let knob = |n: &str, d: f32| crate::tune::get(&format!("water.{n}.{map}"), d);
    let set = bevy_atmos::knobs::set;
    // (Off by default where it doesn't yet beat CoD's own water. Bog: at a
    // low angle CoD's puddles reflect the bright sky; the modern ones read
    // as a dark brown stain, the sky hardly in them (the SSR or the sky's
    // brightness too dark for the game's sky, or the body colour drowning
    // the reflection: to look into).)
    let default = if NOT_YET.contains(&map.as_str()) { 0.0 } else { 1.0 };
    let modern = knob("modern", default) >= 0.5 && !ocean();
    // Screen-space reflections from the game's own copy of the frame
    // (`crate::ssr`, kept while the showcase runs); none where it isn't
    // (a black history would darken every reflection).
    bevy_atmos::ssr::set_host_history(crate::ssr::history());
    set("ssr.enabled", Some(if crate::atmos::climate::on() { 1.0 } else { 0.0 }));
    set("water.replace", Some(if modern { 1.0 } else { 0.0 }));
    set("water.brightness", Some(knob("brightness", 1.0).clamp(0.0, 3.0)));
    set("water.reflection", Some(knob("reflection", 1.0).clamp(0.0, 3.0)));
    // (CoD's water reflected strongly at every angle: its sheets read as
    // water by their sheen. Reflecting from steeper angles keeps that.)
    set("water.fresnel", Some(1.8));
    // (Muddy water hides its bed within centimetres.)
    set("water.absorption", Some(((1.0 + 2.0 * muddy.0) / knob("clarity", 1.0).clamp(0.2, 3.0)).clamp(0.2, 8.0)));
    // (CoD's pools are small and mostly still: low waves.)
    set("water.wave_height", Some(0.1 * knob("waves", 1.0).clamp(0.0, 1.5)));
    if let Some(w) = weather {
        atmos.rain = w.rain;
        atmos.wind = Vec2::new(w.wind.x, w.wind.z);
        atmos.wetness = w.wetness;
    }
}

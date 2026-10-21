//! Fire and embers: the map's and the mission's fires (looping effects
//! whose names say fire, flame, burn or embers) light the scene, and send
//! up glowing embers.
//!
//! - Light: each fire near the eye gets a warm point light, flickering,
//!   its reach by the fire's size (`small`/`med`/`large` in its name),
//!   lighting the world (its lightmapped surfaces take Bevy's lights on top
//!   of their lightmaps), characters and the gun. The nearest few cast
//!   shadows. Knobs `fire.lights` (4: how many at once), `fire.shadows` (0)
//!   `fire.light` (1: brightness).
//! - Embers: thousands of sparks drawn by one draw, each placed by the
//!   vertex shader (`fire/embers.wgsl`) from the clock: rising from the
//!   fires on curling air, carried by the wind, cooling from white-yellow
//!   to red as they go, stretched along their motion, bright enough to
//!   bloom; and some drifting round the eye near a fire. Rain thins them.
//!   No CPU work per spark (as `crate::weather`'s rain). Knob `fire.embers`
//!   (1: the share of sparks; 0 none).
//!
//! - Night grade (`look.night`, 0/1, the Map Look Editor's knobs per map
//!   as `look.night.<map>`): deeper shadows, brighter highlights, more
//!   contrast at the ends, a touch of lens colour fringing at the edges
//!   (`look.fringe`, 0..1, 0.4). Bevy 0.19 has no film grain pass: none.
//!
//! `COD4RW_NOFIRE=1` leaves all of it out (before/after shots).

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::RenderLayers;
use bevy::mesh::{Indices, MeshVertexBufferLayoutRef, PrimitiveTopology};
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, RenderPipelineDescriptor, SpecializedMeshPipelineError};
use bevy::shader::ShaderRef;
use std::sync::Mutex;

pub struct FirePlugin;

impl Plugin for FirePlugin {
    fn build(&self, app: &mut App) {
        if std::env::var("COD4RW_NOFIRE").is_ok_and(|v| v == "1") {
            return;
        }
        app.world().resource::<bevy::asset::io::embedded::EmbeddedAssetRegistry>().insert_asset(
            std::path::PathBuf::from(file!()).with_file_name("fire_embers.wgsl"),
            std::path::Path::new(SHADER),
            include_bytes!("fire_embers.wgsl").as_slice(),
        );
        app.add_plugins(MaterialPlugin::<EmberMaterial>::default()).add_systems(Update, (lights, embers, night_grade).run_if(crate::state::in_game));
    }
}

const SHADER: &str = "cod4rw/fire_embers.wgsl";
/// Sparks drawn at most: from fires, and round the eye.
const SPARKS: u32 = 6000;
const NEAR_SPARKS: u32 = 600;
/// Fires sending up sparks at once (the nearest).
const EMITTERS: usize = 16;

/// Where a fire burns.
#[derive(Clone, Copy, Debug)]
pub enum FireAt {
    Fixed(Vec3),
    Entity(Entity),
}

/// A fire: where, how big (1 small .. 3 large), and the match it's in.
#[derive(Clone, Copy, Debug)]
struct Fire {
    at: FireAt,
    size: f32,
    seed: f32,
    match_id: u32,
}

static FIRES: Mutex<Vec<Fire>> = Mutex::new(Vec::new());

/// Whether a looping effect named `name` is a fire.
pub fn is_fire(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["fire", "flame", "burn", "ember"].iter().any(|w| n.contains(w)) && !n.contains("extinguish") && !n.contains("fireworks") && !n.contains("muzzle") && !n.contains("paper")
}

/// A looping effect started (`crate::fx::Effects::play_ambient`,
/// `play_ambient_bolted`): if it's a fire, remembered for its light and
/// embers.
pub fn note(name: &str, at: FireAt) {
    if !is_fire(name) {
        return;
    }
    let n = name.to_ascii_lowercase();
    let size = if ["large", "_lg", "big", "building", "huge", "xl"].iter().any(|w| n.contains(w)) {
        3.0
    } else if ["small", "_sm", "tiny", "candle", "torch"].iter().any(|w| n.contains(w)) {
        1.0
    } else {
        2.0
    };
    let mut fires = FIRES.lock().unwrap_or_else(|e| e.into_inner());
    let id = crate::atmos::climate::match_id();
    fires.retain(|f| f.match_id == id);
    let seed = (fires.len() as f32 * 0.618_034).fract() * 100.0;
    fires.push(Fire { at, size, seed, match_id: id });
}

/// `COD4RW_FIRE_AB=1` with `COD4RW_SHOT_VIEWS` (each view twice): the
/// fire's light, embers and night grade off for the even shots, on for the
/// odd (as `crate::autoshot` times them), for same-view pairs.
fn ab_on(time: &Time) -> bool {
    if std::env::var("COD4RW_FIRE_AB").map_or(true, |v| v != "1") {
        return true;
    }
    let late: f32 = std::env::var("COD4RW_SHOT_LATE").ok().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let k = (((time.elapsed_secs() - late - 5.0) / 1.5).floor().max(0.0)) as usize;
    k % 2 == 1
}

/// This match's fires, placed: where (Bevy), size, flicker seed.
fn fires_now(places: &Query<&GlobalTransform>) -> Vec<(Vec3, f32, f32)> {
    let id = crate::atmos::climate::match_id();
    let fires = FIRES.lock().unwrap_or_else(|e| e.into_inner());
    fires
        .iter()
        .filter(|f| f.match_id == id)
        .filter_map(|f| {
            let at = match f.at {
                FireAt::Fixed(p) => p,
                FireAt::Entity(e) => places.get(e).ok()?.translation(),
            };
            Some((at, f.size, f.seed))
        })
        .collect()
}

/// A fire's light (`lights`).
#[derive(Component)]
struct FireLight;

/// How much a fire's light flickers now (about 1).
fn flicker(t: f32, seed: f32) -> f32 {
    let a = (t * 7.3 + seed).sin() * 0.12 + (t * 13.7 + seed * 2.1).sin() * 0.08 + (t * 23.1 + seed * 0.7).sin() * 0.05;
    // (Now and then a gust: the flames flare.)
    let gust = ((t * 0.9 + seed).sin() * 0.5 + 0.5).powi(6) * 0.25;
    0.85 + a + gust
}

#[allow(clippy::type_complexity)]
fn lights(
    mut commands: Commands,
    time: Res<Time>,
    camera: Query<&GlobalTransform, With<crate::player::MainCamera>>,
    places: Query<&GlobalTransform>,
    mut pool: Query<(Entity, &mut PointLight, &mut Transform, &mut Visibility), With<FireLight>>,
) {
    let Some(eye) = camera.iter().next().map(|g| g.translation()) else { return };
    let budget = if ab_on(&time) { crate::tune::get("fire.lights", 4.0).clamp(0.0, 32.0) as usize } else { 0 };
    // (Off by default: a shadowed light that moves re-renders its six
    // shadow faces every frame.)
    let shadows = crate::tune::get("fire.shadows", 0.0).clamp(0.0, 4.0) as usize;
    let gain = crate::tune::get("fire.light", 1.0).clamp(0.0, 10.0);
    let mut near: Vec<(f32, Vec3, f32, f32)> = fires_now(&places)
        .into_iter()
        .map(|(p, size, seed)| (p.distance(eye) - size * 6.0, p, size, seed))
        .filter(|f| f.0 < 60.0)
        .collect();
    near.sort_by(|a, b| a.0.total_cmp(&b.0));
    near.truncate(budget);
    let t = time.elapsed_secs_wrapped();
    let mut free: Vec<_> = pool.iter_mut().collect();
    let smoke = crate::tune::get("fire.smoke", 1.0).clamp(0.0, 4.0);
    let mut for_smoke = Vec::new();
    for (i, (_, at, size, seed)) in near.iter().enumerate() {
        let range = 4.0 + size * 4.0;
        // (Lifted off the ground into the flames, and wandering a little as
        // they lick about.)
        let wander = Vec3::new((t * 3.1 + seed).sin(), 0.0, (t * 2.7 + seed * 1.3).cos()) * 0.12 * size;
        let shadow_here = i < shadows;
        // (A shadowed one holds still: moving, its shadow re-rendered each
        // frame.)
        let pos = *at + Vec3::Y * (0.4 + 0.3 * size) + if shadow_here { Vec3::ZERO } else { wander };
        // (Against the lightmaps' light (`crate::lightmaps::LIGHTMAP_EXPOSURE`):
        // twice a muzzle flash's per square metre of reach.)
        let intensity = 8000.0 * range * range * flicker(t, *seed) * gain;
        let colour = Color::linear_rgb(1.0, 0.34 + 0.05 * (t * 11.0 + seed).sin(), 0.07);
        let shadow = i < shadows;
        if smoke > 0.0 {
            let c = colour.to_linear();
            for_smoke.push((pos, range, Vec3::new(c.red, c.green, c.blue) * intensity * smoke));
        }
        match free.pop() {
            Some((_, mut light, mut tf, mut vis)) => {
                light.intensity = intensity;
                light.range = range;
                light.color = colour;
                if light.shadow_maps_enabled != shadow {
                    light.shadow_maps_enabled = shadow;
                }
                tf.translation = pos;
                *vis = Visibility::Visible;
            }
            None => {
                commands.spawn((
                    FireLight,
                    Name::new("fire light"),
                    PointLight { intensity, range, color: colour, shadow_maps_enabled: shadow, radius: 0.3 * size, ..default() },
                    Transform::from_translation(pos),
                    Visibility::Visible,
                    // The world and player 1's gun.
                    RenderLayers::from_layers(&[0, crate::splitscreen::viewmodel_layer(0)]),
                ));
            }
        }
    }
    *SMOKE_LIGHTS.lock().unwrap_or_else(|e| e.into_inner()) = for_smoke;
    for (_, _, _, mut vis) in free {
        if *vis != Visibility::Hidden {
            *vis = Visibility::Hidden;
        }
    }
}

/// The embers' draw.
#[derive(Asset, AsBindGroup, TypePath, Debug, Clone)]
pub struct EmberMaterial {
    /// x: clock (s), y: share of sparks drawn, z: fires in `emitters`, w:
    /// how near a fire the eye is (0..1, for the sparks round it).
    #[uniform(0)]
    pub params: Vec4,
    /// xy: wind (m/s, Bevy x, z), z: rain 0..1, w: 1 for the sparks round
    /// the eye, 0 those from fires.
    #[uniform(1)]
    pub air: Vec4,
    /// The fires: xyz where, w size (1..3).
    #[uniform(2)]
    pub emitters: [Vec4; EMITTERS],
}

impl Material for EmberMaterial {
    fn vertex_shader() -> ShaderRef {
        "embedded://cod4rw/fire_embers.wgsl".into()
    }
    fn fragment_shader() -> ShaderRef {
        "embedded://cod4rw/fire_embers.wgsl".into()
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

/// The embers' two draws: from the fires, round the eye.
#[derive(Component)]
struct EmberDraw(bool);

#[allow(clippy::too_many_arguments)]
fn embers(
    mut commands: Commands,
    time: Res<Time>,
    storm: Option<Res<crate::weather::Storm>>,
    camera: Query<&GlobalTransform, With<crate::player::MainCamera>>,
    places: Query<&GlobalTransform>,
    mut draws: Query<(&EmberDraw, &MeshMaterial3d<EmberMaterial>, &mut Visibility)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<EmberMaterial>>,
) {
    let Some(eye) = camera.iter().next().map(|g| g.translation()) else { return };
    let mut fires: Vec<(f32, Vec3, f32)> = fires_now(&places).into_iter().map(|(p, s, _)| (p.distance(eye), p, s)).filter(|f| f.0 < 120.0).collect();
    let share = if ab_on(&time) { crate::tune::get("fire.embers", 1.0).clamp(0.0, 1.0) } else { 0.0 };
    if draws.is_empty() {
        if fires.is_empty() {
            return;
        }
        for near in [false, true] {
            let material = materials.add(EmberMaterial { params: Vec4::ZERO, air: Vec4::ZERO, emitters: [Vec4::ZERO; EMITTERS] });
            commands.spawn((
                Name::new("embers"),
                EmberDraw(near),
                Mesh3d(crate::mesh_bounds::add(&mut meshes, quads(if near { NEAR_SPARKS } else { SPARKS }))),
                MeshMaterial3d(material),
                Transform::default(),
                bevy::camera::visibility::NoFrustumCulling,
                bevy::light::NotShadowCaster,
            ));
        }
        return;
    }
    fires.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut emitters = [Vec4::ZERO; EMITTERS];
    for (k, f) in fires.iter().take(EMITTERS).enumerate() {
        emitters[k] = f.1.extend(f.2);
    }
    let n = fires.len().min(EMITTERS) as f32;
    // (The eye's nearness to fire: sparks drift round it within ~15 m of
    // a big one.)
    let heat = fires.iter().map(|f| (1.0 - (f.0 - f.2 * 3.0) / 15.0).clamp(0.0, 1.0)).fold(0.0, f32::max);
    let (wind, rain) = storm.as_ref().map_or((Vec2::ZERO, 0.0), |s| (s.wind, s.rain));
    let t = time.elapsed_secs_wrapped();
    for (draw, handle, mut vis) in &mut draws {
        let on = share > 0.0 && n > 0.0 && (!draw.0 || heat > 0.0);
        let want = if on { Visibility::Visible } else { Visibility::Hidden };
        if *vis != want {
            *vis = want;
        }
        if !on {
            continue;
        }
        let Some(mut m) = materials.get_mut(&handle.0) else { continue };
        m.params = Vec4::new(t, share * (1.0 - 0.6 * rain), n, heat);
        m.air = Vec4::new(wind.x, wind.y, rain, draw.0 as u32 as f32);
        m.emitters = emitters;
    }
}

/// The night grade on the world camera (`look.night`).
fn night_grade(
    mut commands: Commands,
    time: Res<Time>,
    map: Option<Res<crate::world::MapName>>,
    mut cams: Query<(Entity, &mut bevy::render::view::ColorGrading, Has<bevy::post_process::effect_stack::ChromaticAberration>), With<crate::player::MainCamera>>,
    mut world: ResMut<Assets<crate::world::WorldMaterial>>,
    mut exposures: Query<&mut bevy::post_process::auto_exposure::AutoExposure, With<crate::player::MainCamera>>,
    mut skies: Query<&mut bevy::core_pipeline::Skybox>,
    mut sky_base: Local<Option<f32>>,
    mut was: Local<Option<(u32, bool)>>,
    mut ranges: Local<Option<std::ops::RangeInclusive<f32>>>,
) {
    let per_map = map.as_ref().map_or(-1.0, |m| crate::tune::get(&format!("look.night.{}", m.0), -1.0));
    let on = ab_on(&time) && if per_map >= 0.0 { per_map >= 0.5 } else { crate::tune::get("look.night", 0.0) >= 0.5 };
    let fringe = crate::tune::get("look.fringe", 0.4).clamp(0.0, 1.0);
    // (Each match afresh: a new map's materials, camera and sky.)
    let id = crate::atmos::climate::match_id();
    if was.is_some_and(|(m, _)| m != id) {
        *sky_base = None;
        *ranges = None;
    }
    if *was == Some((id, on)) {
        return;
    }
    *was = Some((id, on));
    // The baked light turned down to a dim fill (`look.night_ambient`,
    // 0.2), the map's practical lights (each surface's primary light,
    // `crate::world::world_lights`) and the fires carrying the scene, as a
    // city at night is lit.
    // (The eye kept from adjusting up to the dark: it brightened the dimmed
    // scene straight back to day.)
    for mut e in &mut exposures {
        let base = ranges.get_or_insert_with(|| e.range.clone()).clone();
        // (Held: a night's eye doesn't open up to day; `look.night_ev`
        // stops, 1.)
        let ev = crate::tune::get("look.night_ev", 1.0).clamp(-3.0, 3.0);
        let want = if on { ev..=ev } else { base };
        if e.range != want {
            e.range = want;
        }
    }
    let dim = if on { crate::tune::get("look.night_ambient", 0.035).clamp(0.0, 1.0) } else { 1.0 };
    NIGHT_DIM.store(dim.max(1e-3).to_bits(), std::sync::atomic::Ordering::Relaxed);
    // The night sky near black, its clouds faint (`look.night_sky`, 0.12).
    let sky_k = if on { crate::tune::get("look.night_sky", 0.12).clamp(0.0, 1.0) } else { 1.0 };
    for mut sky in &mut skies {
        let base = *sky_base.get_or_insert(sky.brightness);
        sky.brightness = base * sky_k;
    }
    let ids: Vec<_> = world.ids().collect();
    for id in ids {
        let Some(m) = world.get(id) else { continue };
        let want_w = crate::lightmaps::LIGHTMAP_EXPOSURE * dim;
        let has_light = m.extension.params.plight_pos.w > 0.0;
        if m.extension.params.flags.z < 0.5 && !has_light {
            continue;
        }
        if let Some(mut m) = world.get_mut(id) {
            if m.extension.params.flags.z > 0.5 {
                m.extension.params.flags.w = want_w;
            }
            if has_light {
                m.extension.params.plight_color.w = on as u32 as f32;
            }
        }
    }
    for (e, mut g, has_ca) in &mut cams {
        // (Only the shadows' and highlights' sections: the midtones and the
        // global ones are `settings_apply`'s.)
        // (The surroundings sink toward black; what's lit stays bright.)
        let (sg, ss, hg, hs, sgain) = if on { (1.45, 0.8, 1.12, 1.1, 0.7) } else { (1.0, 1.0, 1.0, 1.0, 1.0) };
        if g.shadows.gamma != sg || g.highlights.gain != hg {
            g.shadows.gain = sgain;
            g.shadows.gamma = sg;
            g.shadows.saturation = ss;
            g.highlights.gain = hg;
            g.highlights.saturation = hs;
        }
        let want_ca = on && fringe > 0.0;
        if want_ca && !has_ca {
            commands.entity(e).try_insert(bevy::post_process::effect_stack::ChromaticAberration { intensity: 0.01 * fringe, ..default() });
        } else if !want_ca && has_ca {
            commands.entity(e).try_remove::<bevy::post_process::effect_stack::ChromaticAberration>();
        }
    }
}

/// The night grade's dim fill (`look.night_ambient`), 1 without it: the
/// models' and props' light (`crate::atmos::climate::light_share`) sinks
/// with the baked light.
static NIGHT_DIM: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x3f80_0000);

pub fn night_dim() -> f32 {
    f32::from_bits(NIGHT_DIM.load(std::sync::atomic::Ordering::Relaxed))
}

/// The fires' lights this frame (where, reach, colour times lumens), for
/// the smoke's lighting (`crate::fx`).
static SMOKE_LIGHTS: Mutex<Vec<(Vec3, f32, Vec3)>> = Mutex::new(Vec::new());

pub fn smoke_lights() -> Vec<(Vec3, f32, Vec3)> {
    SMOKE_LIGHTS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Whether the night grade is on for this map (`look.night.<map>`, else
/// `look.night`).
pub fn night_on() -> bool {
    let map = crate::atmos::climate::map_name();
    let per = crate::tune::get(&format!("look.night.{map}"), -1.0);
    if per >= 0.0 { per >= 0.5 } else { crate::tune::get("look.night", 0.0) >= 0.5 }
}

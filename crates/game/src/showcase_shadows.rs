//! The showcase maps' shadow upgrades (only where the showcase is on,
//! `crate::atmos::climate::showcase`, and not on low or off shadows; the
//! soft and contact sun shadows also on every map at high shadows or better):
//!
//! - Soft sun shadows: Bevy's PCSS (`soft_shadow_size` on the map's sun),
//!   penumbras widening with distance from the caster. Knob
//!   `shadows.soft` (the sun's apparent size, metres of light at its
//!   distance; 0 off).
//! - Contact shadows: Bevy's screen-space shadow rays (`ContactShadows` on
//!   the world and viewmodel cameras) toward the sun, for the detail the
//!   shadow map misses: feet on the ground, props on floors, the gun on the
//!   hands. The lightmapped world takes the sun's in `world.wgsl`
//!   (`cod4rw::shadows::sun_contact`). Knob `shadows.contact` (ray length,
//!   metres; 0 off).
//! - Lamp shadows: the map's lamps are baked, their light in the lightmaps
//!   already shadowed by walls. What they lack is shadows of what moves
//!   (soldiers). The few lamps that matter most here (bright, near the
//!   eye) get a shadow-casting Bevy light on a layer of their own,
//!   [`LAMP_LAYER`], which only soldiers' bodies join: the map and its
//!   props (whose shadows are baked) don't cast there. Their light is next to nothing ([`LAMP_MARK`]:
//!   no light added twice); `world.wgsl` (`cod4rw::shadows::lamp_shade`)
//!   darkens the baked light where their shadow maps say something blocks
//!   the lamp, by the lamp's reach there. Lamps fade in and out over
//!   [`FADE_SECS`] as they change. Knobs `shadows.lamps` (how many at
//!   once, 0 off) and `shadows.lamp_strength` (how much of the baked light
//!   is the lamp's, 0..1).

use crate::content::Content;
use crate::units::{self, u};
use bevy::camera::visibility::RenderLayers;
use bevy::light::{PointLightShadowMap, ShadowFilteringMethod};
use bevy::pbr::ContactShadows;
use bevy::prelude::*;

pub struct ShowcaseShadowsPlugin;

impl Plugin for ShowcaseShadowsPlugin {
    fn build(&self, app: &mut App) {
        bevy::shader::load_shader_library!(app, "shaders/shadows.wgsl");
        app.add_systems(Update, (sun, contact, lamps, lamp_casters).run_if(crate::state::in_game));
    }
}

/// The shadow-casting lamps' power (lumens): their light is this times their
/// fade and strength, next to nothing, which `shadows.wgsl` reads back as
/// their weight. Keep in step with `LAMP_MARK` there.
const LAMP_MARK: f32 = 0.01;
/// Seconds for a lamp's shadow to fade in or out.
const FADE_SECS: f32 = 0.5;
/// Lamps further than this past their own radius aren't considered.
const SLACK: f32 = 2.0;
/// Point lights' shadow maps (each face): only moving things cast there.
const LAMP_MAP: usize = 512;
/// The lamp lights' render layer: soldiers' bodies (skinned meshes on the
/// world's layer) join it to cast there; the world camera sees it so the
/// lights reach the world's shader. (Previews use 8-27, splitscreen 28-38.)
const LAMP_LAYER: usize = 40;

/// Whether the upgrades run now: on a showcase map, with shadows at medium
/// or better.
fn active(settings: &crate::settings::Settings) -> bool {
    crate::atmos::climate::showcase() && !crate::atmos::climate::generic() && !matches!(settings.text("r_shadows"), "off" | "low") && !crate::splitscreen::active()
}

/// Whether the soft and contact sun shadows run now: as [`active`], or on
/// any map with shadows at high or better (knob `shadows.all_maps`, 1 on):
/// a hard-edged sun shadow with nothing where things meet the ground read
/// as flat (Crossfire's dumpsters, 2026-10-18 reports).
fn sun_upgrades(settings: &crate::settings::Settings) -> bool {
    active(settings) || (crate::tune::get("shadows.all_maps", 1.0) > 0.5 && matches!(settings.text("r_shadows"), "high" | "ultra") && !crate::splitscreen::active())
}

/// Soft (PCSS) sun shadows.
fn sun(settings: Res<crate::settings::Settings>, mut suns: Query<(&Name, &mut DirectionalLight), Without<crate::model_lighting::ViewModelSun>>) {
    // (0.6: at 1.5 a roof 10 m up blurred its shadow on the floor away,
    // Killhouse's hall, 2026-10-20 reports.)
    let size = crate::tune::get("shadows.soft", 0.6);
    let want = (sun_upgrades(&settings) && size > 0.0).then_some(size);
    for (name, mut light) in &mut suns {
        if name.as_str() == "sun" && light.soft_shadow_size != want {
            light.soft_shadow_size = want;
        }
    }
}

/// Contact shadows on the world and viewmodel cameras, from the sun (and
/// the viewmodel's sun).
#[allow(clippy::type_complexity)]
fn contact(
    mut commands: Commands,
    settings: Res<crate::settings::Settings>,
    // (The world camera only: on the viewmodel's its required depth prepass
    // was a whole extra view for the render thread, for little to see.)
    cameras: Query<(Entity, Option<&ContactShadows>), With<crate::player::MainCamera>>,
    mut suns: Query<(&Name, &mut DirectionalLight, Has<crate::model_lighting::ViewModelSun>)>,
) {
    let length = crate::tune::get("shadows.contact", 0.35);
    let on = sun_upgrades(&settings) && length > 0.0;
    for (e, has) in &cameras {
        match (on, has) {
            (true, Some(c)) if (c.length - length).abs() < 1e-4 => {}
            (true, _) => {
                commands.entity(e).try_insert((ContactShadows { linear_steps: 8, thickness: 0.15, length }, ShadowFilteringMethod::Gaussian));
            }
            (false, Some(_)) => {
                commands.entity(e).try_remove::<ContactShadows>();
            }
            (false, None) => {}
        }
    }
    for (name, mut light, vm) in &mut suns {
        if (vm || name.as_str() == "sun") && light.contact_shadows_enabled != on {
            light.contact_shadows_enabled = on;
        }
    }
}

#[derive(Clone, Copy)]
struct Lamp {
    pos: Vec3,
    dir: Vec3,
    /// Linear colour, brightest channel 1, and how bright (CoD4's).
    colour: Vec3,
    bright: f32,
    range: f32,
    /// Spot cone half angles (outer, inner), radians; none for an omni.
    cone: Option<(f32, f32)>,
}

/// A shadow-casting lamp light and the lamp it stands for.
#[derive(Component)]
struct LampShadow {
    lamp: Option<usize>,
    fade: f32,
    spot: bool,
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn lamps(
    mut commands: Commands,
    time: Res<Time>,
    settings: Res<crate::settings::Settings>,
    content: Option<Res<Content>>,
    camera: Query<&GlobalTransform, With<crate::player::MainCamera>>,
    pawns: Query<&GlobalTransform, (With<crate::thirdperson::Body>, Without<crate::player::LocalPlayer>)>,
    mut map_size: ResMut<PointLightShadowMap>,
    mut lamps: Local<Option<(u32, Vec<Lamp>)>>,
    mut points: Query<(Entity, &mut LampShadow, &mut Transform, &mut PointLight, &mut Visibility), Without<SpotLight>>,
    mut spots: Query<(Entity, &mut LampShadow, &mut Transform, &mut SpotLight, &mut Visibility), Without<PointLight>>,
) {
    let budget = if active(&settings) { crate::tune::get("shadows.lamps", 4.0).clamp(0.0, 8.0) as usize } else { 0 };
    let strength = crate::tune::get("shadows.lamp_strength", 0.7).clamp(0.0, 1.0);
    // The map's lamps, once a match.
    if lamps.as_ref().is_none_or(|(m, _)| *m != crate::atmos::climate::match_id()) {
        let Some(content) = content.as_ref() else { return };
        let list = map_lamps(content);
        // (Remembered for `lamp_casters`: nothing to cast for on a map
        // without lamps, Bog.)
        LAMPS_IN.store(if list.is_empty() { 0 } else { crate::atmos::climate::match_id() }, std::sync::atomic::Ordering::Relaxed);
        *lamps = Some((crate::atmos::climate::match_id(), list));
    }
    let Some((_, all)) = lamps.as_ref() else { return };
    if budget == 0 && points.is_empty() && spots.is_empty() {
        return;
    }
    if map_size.size != LAMP_MAP {
        map_size.size = LAMP_MAP;
    }
    let Some(eye) = camera.iter().next().map(|c| c.translation()) else { return };
    // The lamps that matter most: bright, and near (within their reach);
    // only those with a soldier in reach (nothing else casts there, and
    // each shadow map is views for the render thread: an omni six).
    let bodies: Vec<Vec3> = pawns.iter().map(|g| g.translation()).collect();
    let mut scored: Vec<(f32, usize)> = all
        .iter()
        .enumerate()
        .filter_map(|(i, l)| {
            let d = l.pos.distance(eye);
            let cast = bodies.iter().any(|b| b.distance(l.pos) < l.range + 1.0);
            (cast && d < l.range + SLACK).then(|| (l.bright * l.range * l.range / (d * d + 1.0), i))
        })
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    let wanted: Vec<usize> = scored.iter().take(budget).map(|s| s.1).collect();
    let step = time.delta_secs() / FADE_SECS;
    // Fade: lamps still wanted toward 1, others toward 0 (then freed).
    let mut held: Vec<usize> = Vec::new();
    let mut free_points = Vec::new();
    let mut free_spots = Vec::new();
    for (e, mut s, mut tf, mut light, mut vis) in &mut points {
        drive(&mut s, &wanted, step, all, strength, &mut tf, &mut vis, |c, i| {
            light.color = c;
            light.intensity = i;
        });
        match s.lamp {
            Some(l) => held.push(l),
            None => free_points.push(e),
        }
    }
    for (e, mut s, mut tf, mut light, mut vis) in &mut spots {
        drive(&mut s, &wanted, step, all, strength, &mut tf, &mut vis, |c, i| {
            light.color = c;
            light.intensity = i;
        });
        match s.lamp {
            Some(l) => held.push(l),
            None => free_spots.push(e),
        }
    }
    // Newly wanted lamps take a free light of their kind (or a new one).
    for &l in wanted.iter().filter(|l| !held.contains(l)) {
        let lamp = &all[l];
        info!("lamp shadows: lamp at CoD {:.0?} ({}, range {:.1} m)", units::to_cod(lamp.pos), if lamp.cone.is_some() { "spot" } else { "omni" }, lamp.range);
        let spot = lamp.cone.is_some();
        let free = if spot { free_spots.pop() } else { free_points.pop() };
        let tf = Transform::from_translation(lamp.pos).looking_to(lamp.dir, if lamp.dir.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y });
        let state = LampShadow { lamp: Some(l), fade: 0.0, spot };
        match free {
            Some(e) => {
                commands.entity(e).try_insert((state, tf, Visibility::Visible));
            }
            None => {
                let colour = Color::linear_rgb(lamp.colour.x, lamp.colour.y, lamp.colour.z);
                let mut e = commands.spawn((Name::new("lamp shadow"), state, tf, Visibility::Visible, RenderLayers::layer(LAMP_LAYER)));
                match lamp.cone {
                    Some((outer, inner)) => e.insert(SpotLight {
                        color: colour,
                        intensity: 0.0,
                        range: lamp.range,
                        outer_angle: outer,
                        inner_angle: inner,
                        shadow_maps_enabled: true,
                        ..default()
                    }),
                    None => e.insert(PointLight { color: colour, intensity: 0.0, range: lamp.range, shadow_maps_enabled: true, ..default() }),
                };
            }
        }
    }
}

/// One lamp light's fade toward wanted (1) or not (0); freed at 0.
#[allow(clippy::too_many_arguments)]
fn drive(
    s: &mut LampShadow,
    wanted: &[usize],
    step: f32,
    all: &[Lamp],
    strength: f32,
    tf: &mut Transform,
    vis: &mut Visibility,
    mut set: impl FnMut(Color, f32),
) {
    let Some(l) = s.lamp else {
        if *vis != Visibility::Hidden {
            *vis = Visibility::Hidden;
        }
        return;
    };
    let target = if wanted.contains(&l) { 1.0 } else { 0.0 };
    s.fade = if target > s.fade { (s.fade + step).min(1.0) } else { (s.fade - step).max(0.0) };
    if s.fade <= 0.0 && target == 0.0 {
        s.lamp = None;
        *vis = Visibility::Hidden;
        return;
    }
    let lamp = &all[l];
    tf.translation = lamp.pos;
    // (`shadows.wgsl` reads the weight back from the light's brightness.
    // Debug: `COD4RW_LAMPVIS` makes them real lights, to see where they are.)
    // `COD4RW_LAMPDEBUG`: weight 2, which `shadows.wgsl` draws as reach
    // (a quarter darker) and shadow (black).)
    let power = if std::env::var_os("COD4RW_LAMPVIS").is_some() {
        400.0 * lamp.range * lamp.range
    } else if std::env::var_os("COD4RW_LAMPDEBUG").is_some() {
        LAMP_MARK * 2.0
    } else {
        LAMP_MARK * s.fade * strength
    };
    set(Color::linear_rgb(lamp.colour.x, lamp.colour.y, lamp.colour.z), power);
}

/// The map's primary lights (CoD4's omni and spot lights).
fn map_lamps(content: &Content) -> Vec<Lamp> {
    const SPOT: u8 = 2;
    const OMNI: u8 = 3;
    let Some(world) = content.map().com_world() else { return Vec::new() };
    world
        .primary_lights
        .iter()
        .filter(|l| matches!(l.kind, SPOT | OMNI) && l.radius > 0.0)
        .map(|l| {
            let c = Vec3::from(l.color);
            let bright = c.max_element();
            let colour = c / bright.max(1e-4);
            // (CoD stores a light's direction toward it.)
            let dir = -units::dir(l.dir).normalize_or(Vec3::Y);
            let cone = (l.kind == SPOT).then(|| {
                let outer = l.cos_half_fov_outer.clamp(-1.0, 1.0).acos();
                let inner = l.cos_half_fov_inner.clamp(-1.0, 1.0).acos().min(outer);
                (outer, inner)
            });
            Lamp { pos: units::pos(l.origin), dir, colour, bright, range: u(l.radius), cone }
        })
        .collect()
}

/// The match whose map has lamps ([`map_lamps`]), 0 for none.
static LAMPS_IN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// With lamp shadows on: soldiers' bodies (skinned meshes on the world's
/// layer only) cast in the lamps' layer, and the world camera sees it.
#[allow(clippy::type_complexity)]
fn lamp_casters(
    mut commands: Commands,
    settings: Res<crate::settings::Settings>,
    bodies: Query<Entity, (With<Mesh3d>, With<bevy::mesh::skinning::SkinnedMesh>, Without<RenderLayers>)>,
    cameras: Query<Entity, (With<crate::player::MainCamera>, Without<RenderLayers>)>,
) {
    // (Only on a map with lamps; each body once, as it arrives: the query
    // takes only those without layers.)
    let has_lamps = LAMPS_IN.load(std::sync::atomic::Ordering::Relaxed) == crate::atmos::climate::match_id();
    if !has_lamps || !active(&settings) || crate::tune::get("shadows.lamps", 4.0) < 1.0 {
        return;
    }
    let both = RenderLayers::from_layers(&[0, LAMP_LAYER]);
    // (A body the wardrobe swaps out this frame is gone by then: try.)
    for e in bodies.iter().chain(cameras.iter()) {
        commands.entity(e).try_insert(both.clone());
    }
}

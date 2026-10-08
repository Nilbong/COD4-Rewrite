//! The map's lamps on the viewmodel: a red wall lamp tints the gun and
//! arms as the player passes it.
//!
//! The world has its lamps baked into the lightmaps and the light grid; the
//! gun takes the grid's (averaged) light, which washes a lamp's colour out
//! up close. Here the map's primary lights (`ComWorld`: omni and spot
//! lights, CoD4's `GFX_LIGHT_TYPE_*`) nearest the eye are real Bevy lights
//! on player 1's viewmodel layer only, so they light nothing else: up to
//! [`MAX_LIGHTS`] within their radius and in sight of the eye.
//! `COD4RW_VMLIGHTS=0` leaves them out. In the rain they dim with the rest
//! of the gun's light (`model_lighting::soak_share`).

use crate::content::Content;
use crate::units::{self, u};
use avian3d::prelude::*;
use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;

pub struct VmLightsPlugin;

impl Plugin for VmLightsPlugin {
    fn build(&self, app: &mut App) {
        if std::env::var("COD4RW_VMLIGHTS").is_ok_and(|v| v == "0") {
            return;
        }
        app.add_systems(Update, place.run_if(crate::state::in_game));
    }
}

/// Lamps lighting the gun at once.
const MAX_LIGHTS: usize = 4;
/// CoD4's light types.
const SPOT: u8 = 2;
const OMNI: u8 = 3;
/// Bevy lumens per square metre of radius: half `crate::fx`'s lights' (a
/// lamp's colour on the gun, not a flash's).
const LUMENS_PER_M2: f32 = 400.0;

#[derive(Clone, Copy)]
struct Lamp {
    pos: Vec3,
    dir: Vec3,
    colour: Vec3,
    range: f32,
    /// Spot cone half angles (outer, inner), radians; none for an omni.
    cone: Option<(f32, f32)>,
}

#[derive(Component)]
struct VmLamp;

#[allow(clippy::type_complexity)]
fn place(
    mut commands: Commands,
    content: Option<Res<Content>>,
    camera: Query<&GlobalTransform, With<crate::player::MainCamera>>,
    spatial: SpatialQuery,
    mut lamps: Local<Option<Vec<Lamp>>>,
    mut placed: Query<(Entity, &mut Transform, &mut PointLight, &mut Visibility), (With<VmLamp>, Without<SpotLight>)>,
    mut spots: Query<(Entity, &mut Transform, &mut SpotLight, &mut Visibility), (With<VmLamp>, Without<PointLight>)>,
    mut spawned: Local<bool>,
) {
    let Some(content) = content else { return };
    let lamps = lamps.get_or_insert_with(|| {
        let list: Vec<Lamp> = content
            .map()
            .com_world()
            .map(|w| {
                w.primary_lights
                    .iter()
                    .filter(|l| matches!(l.kind, SPOT | OMNI) && l.radius > 0.0)
                    .map(|l| Lamp {
                        pos: units::pos(l.origin),
                        // CoD stores a light's direction toward it: it
                        // shines the other way.
                        dir: -units::dir(l.dir).normalize_or(Vec3::Y),
                        colour: Vec3::from(l.color),
                        range: u(l.radius),
                        cone: (l.kind == SPOT).then(|| (l.cos_half_fov_outer.clamp(-1.0, 1.0).acos(), l.cos_half_fov_inner.clamp(-1.0, 1.0).acos())),
                    })
                    .collect()
            })
            .unwrap_or_default();
        info!("viewmodel lamps: {} of the map's primary lights", list.len());
        for l in &list {
            debug!("viewmodel lamp at {:?} (CoD {:?}) colour {:?} range {:.1} m spot {:?} dir (CoD) {:?}", l.pos, units::to_cod(l.pos), l.colour, l.range, l.cone, units::to_cod(l.dir));
        }
        list
    });
    let layer = RenderLayers::layer(crate::splitscreen::viewmodel_layer(0));
    if !*spawned {
        *spawned = true;
        for _ in 0..MAX_LIGHTS {
            commands.spawn((VmLamp, PointLight { shadow_maps_enabled: false, intensity: 0.0, ..default() }, Transform::default(), Visibility::Hidden, layer.clone()));
            commands.spawn((VmLamp, SpotLight { shadow_maps_enabled: false, intensity: 0.0, ..default() }, Transform::default(), Visibility::Hidden, layer.clone()));
        }
        return;
    }
    let Ok(eye) = camera.single().map(|g| g.translation()) else { return };
    let filter = crate::collision::sight_filter();
    // In reach and in sight of the eye, nearest first.
    let mut near: Vec<(f32, Lamp)> = lamps
        .iter()
        .filter_map(|l| {
            let d = l.pos.distance(eye);
            (d < l.range).then_some((d, *l))
        })
        .collect();
    near.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut lit: Vec<Lamp> = Vec::new();
    for (d, l) in near {
        if lit.len() >= MAX_LIGHTS {
            break;
        }
        let Ok(dir) = Dir3::new(l.pos - eye) else { continue };
        // (The lamp's own fitting is often what a ray from it hits first.)
        if spatial.cast_ray(eye, dir, (d - u(8.0)).max(0.0), true, &filter).is_some() {
            trace!("viewmodel lamp {:?} {d:.1} m away: out of sight", units::to_cod(l.pos));
            continue;
        }
        trace!("viewmodel lamp {:?} {d:.1} m away: lit", units::to_cod(l.pos));
        lit.push(l);
    }
    let soak = crate::model_lighting::soak_share();
    let (mut omnis, mut cones): (Vec<Lamp>, Vec<Lamp>) = lit.into_iter().partition(|l| l.cone.is_none());
    for (_, mut tf, mut light, mut vis) in &mut placed {
        let Some(l) = omnis.pop() else {
            vis.set_if_neq(Visibility::Hidden);
            continue;
        };
        let brightness = l.colour.max_element().max(1e-3);
        let c = l.colour / brightness;
        light.color = Color::linear_rgb(c.x, c.y, c.z);
        light.range = l.range;
        light.intensity = LUMENS_PER_M2 * l.range * l.range * brightness * soak;
        *tf = Transform::from_translation(l.pos);
        vis.set_if_neq(Visibility::Visible);
    }
    for (_, mut tf, mut light, mut vis) in &mut spots {
        let Some(l) = cones.pop() else {
            vis.set_if_neq(Visibility::Hidden);
            continue;
        };
        let (outer, inner) = l.cone.unwrap_or((0.8, 0.6));
        let brightness = l.colour.max_element().max(1e-3);
        let c = l.colour / brightness;
        light.color = Color::linear_rgb(c.x, c.y, c.z);
        light.range = l.range;
        light.intensity = LUMENS_PER_M2 * l.range * l.range * brightness * soak;
        light.outer_angle = outer;
        light.inner_angle = inner.min(outer);
        *tf = Transform::from_translation(l.pos).looking_to(l.dir, if l.dir.y.abs() > 0.99 { Vec3::X } else { Vec3::Y });
        vis.set_if_neq(Visibility::Visible);
    }
}

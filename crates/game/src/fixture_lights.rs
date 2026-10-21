//! Switched-on lamps light what's around them live: each of the map's lamp
//! models that's on (`com_cafe_light_part1_on`, `me_lightfluohang_on`...)
//! gets a short-range Bevy point light under its shade.
//!
//! The lamps' light is in the lightmaps already (CoD4's, or the re-bake's
//! fitted lamps, `crate::bake::lamps`), but on a daytime map it was faint
//! enough that a lit cafe read as unlit (Crossfire, 2026-10-18 reports:
//! "doesn't look like lamp lighting is doing anything"). A live light adds
//! the pool on the floor and tables, highlights on shiny things and light on
//! soldiers walking under it. Shaded lamps shine down (spot lights: their
//! shade kept the ceiling dark), fluorescent tubes all round. No shadows (dozens of lamps): the range is
//! kept short so little reaches through walls. Knobs `lamps.light` (lumens,
//! 0 off) and `lamps.range` (metres).

use crate::content::Content;
use crate::units;
use bevy::prelude::*;

pub struct FixtureLightsPlugin;

impl Plugin for FixtureLightsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (place, tune).run_if(crate::state::in_game));
    }
}

/// A lamp's light (lumens) and reach (metres): a pool a few metres
/// across that holds its own beside daylight through the windows.
const LUMENS: f32 = 150_000.0;
const RANGE: f32 = 5.0;

#[derive(Component)]
struct FixtureLight;

/// Whether a model is a lamp that's switched on.
fn lit_lamp(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.contains("_on")
        && ["light", "lamp", "bulb", "fluo", "lantern"].iter().any(|k| name.contains(k))
        && !name.contains("lightning")
        && !name.contains("lighthouse")
}

/// A lamp's light colour (linear): fluorescent tubes cool, bulbs warm.
fn colour_of(name: &str) -> Vec3 {
    if name.contains("fluo") { Vec3::new(0.85, 0.95, 1.0) } else { Vec3::new(1.0, 0.78, 0.52) }
}

fn place(mut commands: Commands, content: Option<Res<Content>>, lights: Query<Entity, With<FixtureLight>>) {
    let Some(content) = content else { return };
    if !content.is_added() && !content.is_changed() {
        return;
    }
    if !lights.is_empty() {
        return;
    }
    let lumens = crate::tune::get("lamps.light", LUMENS);
    if lumens <= 0.0 {
        return;
    }
    let range = crate::tune::get("lamps.range", RANGE);
    let Some(world) = content.map().gfx_world() else { return };
    let mut n = 0;
    for sm in &world.static_models {
        let Some(id) = sm.model else { continue };
        let (zi, id) = content.resolve_xmodel(crate::content::MAP_ZONE, id);
        let Some(xm) = content.zone(zi).xmodel(id) else { continue };
        if !lit_lamp(&xm.name) || crate::map_editor::layout::hidden(&xm.name, sm.origin) {
            continue;
        }
        // Under the shade: the bottom of the model's bounds, a little below
        // (model space, CoD axes), as the bake finds its lamps.
        let local = [(xm.mins[0] + xm.maxs[0]) * 0.5, (xm.mins[1] + xm.maxs[1]) * 0.5, xm.mins[2] - 6.0];
        let rot = units::axis_rotation(sm.axis);
        let pos = units::pos(sm.origin) + rot * (units::pos(local) * sm.scale);
        let lower = xm.name.to_ascii_lowercase();
        let colour = colour_of(&lower);
        let color = Color::linear_rgb(colour.x, colour.y, colour.z);
        let mut e = commands.spawn((Name::new("lamp light"), FixtureLight, DespawnOnExit(crate::state::GameState::InGame)));
        if lower.contains("fluo") {
            e.insert((PointLight { color, intensity: lumens, range, radius: 0.05, shadow_maps_enabled: false, ..default() }, Transform::from_translation(pos)));
        } else {
            // Down the model's own axis.
            let down = rot * units::dir([0.0, 0.0, -1.0]);
            let up = if down.dot(Vec3::Z).abs() > 0.9 { Vec3::X } else { Vec3::Z };
            e.insert((
                SpotLight { color, intensity: lumens, range, radius: 0.05, shadow_maps_enabled: false, outer_angle: 1.25, inner_angle: 0.6, ..default() },
                Transform::from_translation(pos).looking_to(down, up),
            ));
        }
        n += 1;
    }
    if n > 0 {
        info!("lamp lights: {n} switched-on lamps lit live ({lumens:.0} lm, {range} m)");
    }
}

/// Live knob changes.
fn tune(mut points: Query<&mut PointLight, With<FixtureLight>>, mut spots: Query<&mut SpotLight, With<FixtureLight>>) {
    let lumens = crate::tune::get("lamps.light", LUMENS);
    let range = crate::tune::get("lamps.range", RANGE);
    for mut l in &mut points {
        if l.intensity != lumens || l.range != range {
            (l.intensity, l.range) = (lumens, range);
        }
    }
    for mut l in &mut spots {
        if l.intensity != lumens || l.range != range {
            (l.intensity, l.range) = (lumens, range);
        }
    }
}

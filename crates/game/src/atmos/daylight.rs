//! The showcase's live light from [`TimeOfDay`] and [`Weather`]: the map's
//! sun light follows the sun (or, at night, carries the moon), the ambient
//! light the sky, the distance fog darkens with the light, and lightning
//! flashes the world with a short directional pulse. Only with the
//! showcase on (`TimeOfDay::enabled`); otherwise CoD4's lighting stands.
//! (The lightmaps are baked: their keyframes per time of day are the
//! lighting bake's.)

use super::climate::{TimeOfDay, Weather};
use bevy::prelude::*;

pub struct DaylightPlugin;

impl Plugin for DaylightPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, drive.after(super::climate::ClimateSet).run_if(crate::state::in_game));
    }
}

/// The lightning's light (no shadows: a flash from the whole sky's side).
#[derive(Component)]
struct LightningLight;

#[allow(clippy::type_complexity)]
fn drive(
    mut commands: Commands,
    mut tod: ResMut<TimeOfDay>,
    weather: Res<Weather>,
    mut ambient: ResMut<GlobalAmbientLight>,
    mut suns: Query<(&Name, &mut DirectionalLight, &mut Transform), (Without<LightningLight>, Without<crate::model_lighting::ViewModelSun>)>,
    mut flash: Query<(&mut DirectionalLight, &mut Transform), With<LightningLight>>,
    mut fogs: Query<&mut DistanceFog>,
    mut exposures: Query<&mut bevy::post_process::auto_exposure::AutoExposure>,
) {
    let (true, Some(map)) = (tod.enabled, tod.map) else { return };
    let tod = &mut *tod;
    // Storm cloud dims the light (by night less: the moon's is weak already).
    let storm = smooth(weather.rain / 0.8) * (1.0 - 0.5 * tod.night);
    // The sun, or by night the moon, through the clouds.
    let (dir, color, lux) = if tod.sun_illuminance >= tod.moon_illuminance {
        (tod.sun_dir, tod.sun_color * map.color, tod.sun_illuminance)
    } else {
        (tod.moon_dir, tod.moon_color, tod.moon_illuminance)
    };
    let lux = lux * (1.0 - 0.85 * storm);
    // A map CoD4 lit by night (its key light is moonlight: blue, as Wet
    // Work's, Winter Crash's, Chinatown's) keeps a share of CoD4's own
    // moonlight by night ([`NIGHT_FLOOR`]): readable, but clearly night (CoD4's
    // whole read as dusk, the user's verdict).
    let night_map = map.color.z > map.color.x * 1.15;
    let floor = map.illuminance * NIGHT_FLOOR * tod.night * (1.0 - 0.4 * storm);
    let (color, lux) = if night_map && tod.night > 0.0 && floor > lux {
        (color.lerp(map.color, tod.night), floor)
    } else {
        (color, lux)
    };
    for (name, mut light, mut tf) in &mut suns {
        if name.as_str() != "sun" {
            continue;
        }
        light.color = Color::linear_rgb(color.x, color.y, color.z);
        light.illuminance = lux;
        let up = if dir.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
        // (Kept just above the horizon: a light from below lights nothing.)
        let d = Vec3::new(dir.x, dir.y.max(0.02), dir.z).normalize();
        *tf = Transform::default().looking_to(-d, up);
    }
    // Lightning: the world lit for a moment from the bolt's side.
    let pulse = weather.lightning.map_or(0.0, |l| l.flash);
    let bolt = weather.lightning.map_or(Vec3::Y, |l| l.dir);
    match flash.single_mut() {
        Ok((mut light, mut tf)) => {
            light.illuminance = pulse * map.illuminance * 0.6;
            *tf = Transform::default().looking_to(-bolt, if bolt.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y });
        }
        Err(_) => {
            commands.spawn((
                Name::new("lightning"),
                LightningLight,
                DirectionalLight { color: Color::linear_rgb(0.8, 0.85, 1.0), illuminance: 0.0, shadow_maps_enabled: false, ..default() },
                Transform::default(),
            ));
        }
    }
    // The sky's light, dimmer under storm cloud, lifted by a flash.
    // (Less blue by night: a blue-white ambient lit models like a lamp.)
    let c = tod.sky_color.lerp(Vec3::splat(0.7), (storm * 0.7).max(0.5 * tod.night));
    ambient.color = Color::linear_rgb(c.x, c.y, c.z);
    let sky_light = tod.sky_illuminance * (1.0 - 0.4 * storm);
    // By night models take their light from the baked grid (night sky,
    // lamps): the global ambient, which the lightmapped world doesn't take,
    // fades out, or the gun outshone the deck.
    let sky_light = sky_light * (1.0 - 0.85 * tod.night);
    let _ = night_map;
    ambient.brightness = sky_light + pulse * map.ambient * 0.3;
    super::climate::LIGHT_SHARE.store((ambient.brightness / map.ambient.max(1.0)).clamp(0.05, 1.5).to_bits(), std::sync::atomic::Ordering::Relaxed);
    // The fog is lit air: the sky's colour near the horizon, as bright as
    // the sky's light, greyer in a storm (the map's own fog colour is for
    // its own hour; Wet Work's is night).
    let level = (tod.sky_illuminance / map.ambient.max(1.0)).clamp(0.02, 1.2);
    let dusk = tod.sun_color.lerp(Vec3::ONE, 0.5) * (1.0 - tod.night);
    let horizon = tod.sky_color.lerp(dusk, 0.35);
    let grey = Vec3::splat(horizon.dot(Vec3::new(0.2126, 0.7152, 0.0722)));
    // (A storm's day horizon is the dark deck's, not a glowing band.)
    let day = horizon.lerp(grey, storm * 0.6) * FOG_LEVEL * level * (1.0 - 0.75 * storm);
    // By night the horizon glows a little (sea, distant lights, the cloud
    // deck lit from far off), brighter than overhead: never black.
    let f = day.lerp(NIGHT_HORIZON * (1.0 + 0.5 * weather.rain), tod.night);
    tod.horizon = f;
    for mut d in &mut fogs {
        d.color = Color::linear_rgba(f.x, f.y, f.z, d.color.alpha());
    }
    let range = night_exposure_range(tod.night);
    for mut e in &mut exposures {
        if e.range != range {
            e.range = range.clone();
        }
    }
}

/// By night a moonlit map keeps this share of CoD4's own moonlight and sky
/// light (moody but readable: soldiers stay visible against the deck).
const NIGHT_FLOOR: f32 = 0.2;

/// How many stops the auto-exposure may brighten a dark frame by night (by
/// day its usual 3): it lifted a moonlit storm back to dusk.
fn night_exposure_range(night: f32) -> std::ops::RangeInclusive<f32> {
    -3.0..=3.0 - 2.0 * night
}

/// The day's fog colour's brightness (the frame's units): about the
/// physical sky's horizon by day (`sky.wgsl`).
const FOG_LEVEL: f32 = 0.4;

/// The night's horizon (the frame's units, linear): a dim blue-grey glow.
const NIGHT_HORIZON: Vec3 = Vec3::new(0.016, 0.019, 0.024);

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

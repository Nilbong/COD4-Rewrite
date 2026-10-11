//! A storm for the showcase (`COD4RW_SHOWCASE=1`, Wet Work): rain of our
//! own in place of the map's rain effects.
//!
//! - [`rain`]: streaks falling around the camera, slanted by the wind,
//!   blurred by their speed, lit by the light around and by lightning;
//!   splashes where they land, on the ground, decks, containers and
//!   roofs alike. None fall indoors or under cover: [`occlusion`] keeps a
//!   map, from above, of how high rain gets.
//! - [`screen`]: drops on the screen when looking up out in the rain.
//! - [`sound`]: rain on what's around (the open deck or the roof
//!   overhead) and thunder after each flash, as late as the strike is far.
//!
//! The weather itself (how hard it rains, the wind, lightning) is the
//! sky's ([`crate::atmos::climate::Weather`]; [`Storm`] reads it).

use bevy::prelude::*;

pub mod occlusion;
pub mod rain;
pub mod screen;
pub mod sound;

pub struct WeatherPlugin;

impl Plugin for WeatherPlugin {
    fn build(&self, app: &mut App) {
        if !crate::atmos::climate::enabled() {
            return;
        }
        app.init_resource::<Storm>()
            .add_systems(Update, update_storm.after(crate::atmos::climate::ClimateSet).run_if(crate::state::in_game).run_if(crate::atmos::climate::on));
        occlusion::build(app);
        rain::build(app);
        screen::build(app);
        sound::build(app);
    }
}

/// The showcase storm is on (`COD4RW_SHOWCASE=1`).
pub fn showcase() -> bool {
    crate::atmos::climate::showcase()
}

/// The weather as the rain sees it this frame.
#[derive(Resource, Debug, Clone)]
pub struct Storm {
    /// How hard it rains, 0..1.
    pub rain: f32,
    /// How hard it snows, 0..1.
    pub snow: f32,
    /// The wind over the ground, m/s (Bevy x, z), gusts included.
    pub wind: Vec2,
    /// Lightning's light now, 0..1, and where the last strike was.
    pub flash: f32,
    pub strike: Option<Vec3>,
    /// Bumped at each new strike (for the thunder).
    pub strikes: u32,
    /// The stand-in's clock: the next strike.
    next_strike: f32,
}

impl Default for Storm {
    fn default() -> Self {
        Storm { rain: 0.85, snow: 0.0, wind: Vec2::new(3.0, 1.0), flash: 0.0, strike: None, strikes: 0, next_strike: 6.0 }
    }
}

/// The storm from the sky's weather ([`crate::atmos::climate::Weather`]):
/// its rain, wind and lightning, each strike's place (for the thunder's
/// delay). Without it, a
/// stand-in: steady rain (`COD4RW_RAIN`), gusting wind and a strike every 8
/// to 20 seconds.
fn update_storm(
    time: Res<Time>,
    mut storm: ResMut<Storm>,
    weather: Option<Res<crate::atmos::climate::Weather>>,
    camera: Query<&GlobalTransform, With<crate::player::MainCamera>>,
    mut last_id: Local<u32>,
) {
    let t = time.elapsed_secs();
    let at = camera.iter().next().map_or(Vec3::ZERO, |c| c.translation());
    let rand = |k: u32| {
        let h = ((t * 1000.0) as u32).wrapping_mul(2654435761).wrapping_add(k.wrapping_mul(40503));
        (h % 10000) as f32 / 10000.0
    };
    if let Some(w) = weather.filter(|w| w.enabled) {
        // (How hard it comes down now: severity and gusts, `climate`.)
        storm.rain = w.downpour.max(w.rain);
        storm.snow = w.snow;
        storm.wind = Vec2::new(w.wind.x, w.wind.z);
        storm.flash = w.lightning.as_ref().map_or(0.0, |l| l.flash);
        // Each new strike (atmos counts them): where it hit, for the thunder.
        if let Some(l) = w.lightning.as_ref().filter(|l| l.id != *last_id) {
            *last_id = l.id;
            storm.strike = Some(l.strike);
            storm.strikes += 1;
        }
        return;
    }
    if let Some(r) = std::env::var("COD4RW_RAIN").ok().and_then(|v| v.parse::<f32>().ok()) {
        storm.rain = r.clamp(0.0, 1.0);
    }
    // Gusts: slow swells and quicker flurries over a steady 4 m/s.
    let gust = 1.0 + 0.45 * (t * 0.31).sin() * (t * 0.07).sin().abs() + 0.2 * (t * 1.3).sin().max(0.0);
    let heading = 0.6 + 0.25 * (t * 0.05).sin();
    storm.wind = Vec2::new(heading.cos(), heading.sin()) * 4.0 * gust;
    if t >= storm.next_strike && storm.rain > 0.3 {
        let a = rand(1) * std::f32::consts::TAU;
        let d = 300.0 + 1200.0 * rand(2);
        storm.strike = Some(at + Vec3::new(a.cos() * d, 400.0, a.sin() * d));
        storm.strikes += 1;
        storm.next_strike = t + 8.0 + 12.0 * rand(3);
        storm.flash = 1.0;
    }
    storm.flash *= (-time.delta_secs() / 0.12).exp();
}

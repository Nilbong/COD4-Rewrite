//! Light in the air and a sky of our own, each a quality setting:
//!
//! - Volumetric Lighting (`r_volumetric` Off/Low/High): [`godrays`] (the
//!   sun streaming past window frames, doorways and foliage, screen-space)
//!   at Low and High, and at High also [`volumetric`]: raymarched sun
//!   shafts in the map's own fog colour and density, drawn by Bevy's
//!   volumetric fog pass with our shader.
//! - [`sky`]: a dynamic sky (`r_sky` Classic/Dynamic): the map's skybox
//!   colours, our own raymarched clouds drifting over them and a sun disc
//!   where the map's sun is, with the horizon in the map's fog colour so
//!   distant scenery fades into it.
//!
//! `COD4RW_VOLUMETRIC=off|low|high` and `COD4RW_SKY=classic|dynamic`
//! override the settings (for test runs).

pub mod climate;
mod daylight;
mod godrays;
pub mod sky;
pub mod sun_path;
mod test;
pub mod volumetric;

use bevy::prelude::*;

pub struct AtmosPlugin;

impl Plugin for AtmosPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((climate::ClimatePlugin, daylight::DaylightPlugin, volumetric::VolumetricPlugin, godrays::GodRaysPlugin, sky::SkyPlugin, test::AtmosTestPlugin));
    }
}

/// A setting's value, unless the test (`test`) or a test run's environment
/// variable sets it.
fn setting(settings: &crate::settings::Settings, dvar: &str, env: &str) -> String {
    if let Some(v) = test::forced(dvar) {
        return v.to_owned();
    }
    std::env::var(env).ok().map_or_else(|| settings.text(dvar).to_ascii_lowercase(), |v| v.to_ascii_lowercase())
}

/// The map's fog colour, linear, and its density per metre (CoD4's
/// `ln 2 / halfway`), if it has fog.
fn map_fog(fog: Option<&crate::fog::MapFog>) -> Option<(Vec3, f32)> {
    let f = fog?.0?;
    let c = Color::srgb(f.color[0], f.color[1], f.color[2]).to_linear();
    Some((Vec3::new(c.red, c.green, c.blue), std::f32::consts::LN_2 / crate::units::u(f.halfway)))
}

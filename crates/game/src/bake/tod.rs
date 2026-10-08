//! Time-of-day keyframes: a map baked at a few hours of the day, which the
//! game blends between as `atmos::climate::TimeOfDay`'s clock runs
//! (`COD4RW_SHOWCASE`). Every keyframe is lit by the same sun and moon as
//! the live light ([`super::sun_path`], `atmos/sun_path.rs`, turned so CoD4's
//! sun is on it at the map's hour) under a gradient sky of climate.rs's
//! colour, with the lamps found in the map's own lightmap switched on as
//! it gets dark.

use super::integrate::Spot;
use super::sky::Sky;
use super::sun_path::MapSun;
use glam33::{Vec3, Vec3A};

pub struct Key {
    pub name: &'static str,
    pub hour: f32,
}

/// The keyframes, in hour order (agreed with the atmos session).
pub const KEYS: [Key; 7] = [
    Key { name: "h06", hour: 6.0 },
    Key { name: "h08", hour: 8.0 },
    Key { name: "h12", hour: 12.0 },
    Key { name: "h16", hour: 16.0 },
    Key { name: "h18", hour: 18.0 },
    // Blue hour: the sun just down, the sky still lit (its light falls
    // steeply between 18 and 21).
    Key { name: "h19_5", hour: 19.5 },
    Key { name: "h21", hour: 21.0 },
];

/// The cache variant a keyframe is kept under.
pub fn variant(key: &Key) -> String {
    format!("tod-{}", key.name)
}

/// How a keyframe's bake differs from the map's own lighting.
pub struct Override {
    pub variant: String,
    /// The light the game adds live (sun or moon): its direction and its
    /// irradiance in lightmap units. Only its bounce is baked.
    pub light: (Vec3A, Vec3),
    pub sky: Sky,
    /// The lamps (found in the map's own bake) and how far they're on.
    pub lamps: Vec<Spot>,
    pub lamps_on: f32,
    /// The grid's units (the map's own bake's).
    pub grid_scale: f32,
}

/// The sky's light on open ground at full day (lightmap units): about a
/// third of a full sun's, as on a clear day.
const DAY_SKY: f32 = 0.42;

impl Override {
    pub fn new(key: &Key, map: &MapSun, lamps: &[Spot], grid_scale: f32, lux_to_units: f32) -> Override {
        let (sun, moon) = (map.sun(key.hour), map.moon(key.hour));
        let l = if sun.lux >= moon.lux { sun } else { moon };
        let light = (Vec3A::from(l.to_light).normalize(), Vec3::from(l.colour) * l.lux * lux_to_units);
        let (tint, brightness) = map.sky(key.hour);
        let tint = Vec3::from(tint);
        let shape = |k: f32| Sky::gradient(tint * Vec3::new(0.55, 0.65, 0.9) * k, tint * Vec3::new(1.0, 0.97, 0.92) * k, Vec3::new(0.22, 0.2, 0.18) * k);
        let k = DAY_SKY * brightness / super::sky::luma(shape(1.0).ground).max(1e-6);
        // Lamps on as the sun gets low (off from ~15 degrees up).
        let lamps_on = 1.0 - ((map.sun_height(key.hour)) / 0.26).clamp(0.0, 1.0);
        Override { variant: variant(key), light, sky: shape(k), lamps: lamps.to_vec(), lamps_on, grid_scale }
    }
}

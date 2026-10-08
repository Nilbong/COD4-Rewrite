//! The sun's and moon's path over a day: the one source for the live sun
//! (`super::climate`) and the time-of-day lighting bake (`crate::bake`,
//! which also builds this file standalone, so no Bevy and no `crate::`
//! here). Bevy axes (y up) unless a function says CoD (z up); directions
//! point to the light.

/// A light as the baker wants it.
pub struct Light {
    /// CoD axes.
    pub to_light: [f32; 3],
    /// Linear, brightest channel 1.
    pub colour: [f32; 3],
    /// The map's sun light units (lux as `crate::world` sets it).
    pub lux: f32,
}

/// CoD4's sun for the map, placed on the path as climate.rs's `MapSun`.
#[derive(Clone, Copy, Debug)]
pub struct MapSun {
    /// Bevy axes.
    pub dir: [f32; 3],
    pub colour: [f32; 3],
    pub illuminance: f32,
    pub hour: f32,
    pub azimuth_offset: f32,
}

const LATITUDE: f32 = 35.0 * std::f32::consts::PI / 180.0;

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-12);
    v.map(|c| c / l)
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// climate.rs `path_dir` (Bevy axes).
fn path_dir(hours: f32) -> [f32; 3] {
    let h = (hours - 12.0) / 24.0 * std::f32::consts::TAU;
    let (sl, cl) = LATITUDE.sin_cos();
    normalize([-h.sin(), cl * h.cos(), sl * h.cos()])
}

/// `Quat::from_rotation_y(a) * v`.
fn turn(a: f32, v: [f32; 3]) -> [f32; 3] {
    let (s, c) = a.sin_cos();
    [v[0] * c + v[2] * s, v[1], -v[0] * s + v[2] * c]
}

/// climate.rs `sun_dir_at` (Bevy axes).
pub fn sun_dir_at(hours: f32, azimuth_offset: f32) -> [f32; 3] {
    turn(azimuth_offset, path_dir(hours))
}

/// climate.rs `moon_dir_at` (Bevy axes).
pub fn moon_dir_at(hours: f32, azimuth_offset: f32) -> [f32; 3] {
    let m = turn(azimuth_offset + 0.3, path_dir(hours + 12.0));
    normalize([m[0], m[1] + 0.1, m[2]])
}

/// climate.rs `fit_path`: the map's hour and the path's turn.
pub fn fit_path(dir: [f32; 3]) -> (f32, f32) {
    let target = dir[1].clamp(-1.0, 1.0);
    let (mut lo, mut hi) = (6.0f32, 12.0f32);
    for _ in 0..30 {
        let mid = 0.5 * (lo + hi);
        if path_dir(mid)[1] < target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let hour = 0.5 * (lo + hi);
    let p = path_dir(hour);
    (hour, f32::atan2(dir[0], dir[2]) - f32::atan2(p[0], p[2]))
}

/// Bevy axes to CoD's (Bevy x, y, z = CoD x, z, -y).
fn to_cod(v: [f32; 3]) -> [f32; 3] {
    [v[0], -v[2], v[1]]
}

impl MapSun {
    /// From CoD4's sun direction (CoD axes), colour and the live sun's
    /// illuminance (as `crate::world` sets it).
    pub fn new(to_sun_cod: [f32; 3], colour: [f32; 3], illuminance: f32) -> MapSun {
        let dir = normalize([to_sun_cod[0], to_sun_cod[2], -to_sun_cod[1]]);
        let m = colour.iter().cloned().fold(1e-3, f32::max);
        let (hour, azimuth_offset) = fit_path(dir);
        MapSun { dir, colour: colour.map(|c| c / m), illuminance, hour, azimuth_offset }
    }

    /// How far into the night (climate.rs `night`).
    pub fn night(&self, hours: f32) -> f32 {
        1.0 - smooth((sun_dir_at(hours, self.azimuth_offset)[1] + 0.1) / 0.15)
    }

    /// The sun's elevation sine at `hours`.
    pub fn sun_height(&self, hours: f32) -> f32 {
        sun_dir_at(hours, self.azimuth_offset)[1]
    }

    /// climate.rs `apply_clock`'s sun.
    pub fn sun(&self, hours: f32) -> Light {
        let sun = sun_dir_at(hours, self.azimuth_offset);
        let m = 1.0 / sun[1].max(0.035);
        let red = [(-0.03 * m).exp(), (-0.07 * m).exp(), (-0.16 * m).exp()];
        let tint: [f32; 3] = std::array::from_fn(|i| red[i] * self.colour[i]);
        let tmax = tint.iter().cloned().fold(1e-3, f32::max);
        let up = smooth((sun[1] + 0.02) / 0.19);
        let at_map = self.dir[1].max(0.05).sqrt();
        let lux = self.illuminance * up * (sun[1].max(0.0).sqrt() / at_map).min(1.3);
        Light { to_light: to_cod(sun), colour: tint.map(|c| c / tmax), lux }
    }

    /// climate.rs `apply_clock`'s moon.
    pub fn moon(&self, hours: f32) -> Light {
        let moon = moon_dir_at(hours, self.azimuth_offset);
        // A tenth of the sun: bright enough that soldiers never vanish by night.
        let lux = self.illuminance * 0.10 * self.night(hours) * smooth(moon[1] / 0.1);
        Light { to_light: to_cod(moon), colour: [0.62, 0.72, 1.0], lux }
    }

    /// climate.rs `apply_clock`'s sky: its colour and how bright against
    /// the day's (its ambient's share).
    pub fn sky(&self, hours: f32) -> ([f32; 3], f32) {
        let sun_y = sun_dir_at(hours, self.azimuth_offset)[1];
        let night = self.night(hours);
        let up = smooth((sun_y + 0.02) / 0.19);
        let lerp = |a: [f32; 3], b: [f32; 3], t: f32| -> [f32; 3] { std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t) };
        let dusk = (1.0 - (sun_y.abs() / 0.25).min(1.0)) * (1.0 - night);
        let colour = lerp(lerp([0.55, 0.7, 1.0], [1.0, 0.6, 0.45], dusk * 0.6), [0.25, 0.35, 0.7], night);
        // At least a quarter of the day's sky light by night (gameplay).
        (colour, (0.25 + 0.75 * (1.0 - night)) * (0.4 + 0.6 * up))
    }
}

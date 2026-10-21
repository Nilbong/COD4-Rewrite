//! The map editor's layout (`crate::map_editor::layout`), read here on its
//! own so the baker builds without the game: the models placed on the map
//! (they block and bounce light), the map's own models left out (`@hide:`),
//! and placed lights (`@light`). Same files, same format, same order:
//! the user's copy, the project's, the one built in. (Online, a host's
//! layout isn't the saved one: the bake is of the saved layout.)

use glam33::{Quat, Vec3, Vec3A};

include!(concat!(env!("OUT_DIR"), "/maplayouts.rs"));

/// A placed model or light, CoD axes and units.
pub struct Placed {
    pub model: String,
    pub origin: Vec3A,
    /// The model's axes in the world (rows of its rotation), as a static
    /// model's.
    pub axis: [Vec3A; 3],
    pub scale: f32,
}

/// A placed light (`@light`): drawn live by the game (a Bevy point light),
/// so the bake takes only its bounce.
#[derive(Clone)]
pub struct Light {
    pub origin: Vec3A,
    /// Linear colour times luminous intensity (candela).
    pub intensity: Vec3,
    /// Range in CoD units (Bevy's smooth cut-off).
    pub range: f32,
}

#[derive(Default)]
pub struct Layout {
    pub models: Vec<Placed>,
    /// The map's own models left out: name and origin.
    pub hidden: Vec<(String, Vec3A)>,
    pub lights: Vec<Light>,
    /// A fingerprint of the lines that change the lighting (0 for none),
    /// kept with the bake so an edited layout isn't lit by a stale one.
    pub hash: u64,
}

/// `crate::map_editor::layout::LIGHT_DEFAULTS`.
const LIGHT_DEFAULTS: [f32; 6] = [1.0, 0.85, 0.65, 1.0, 8.0, 0.0];
const INCH: f32 = 0.0254;

fn user_dir() -> std::path::PathBuf {
    if let Some(d) = std::env::var_os("COD4RW_MAPLAYOUT_DIR") {
        return d.into();
    }
    std::env::var_os("LOCALAPPDATA").map_or_else(|| std::path::PathBuf::from("maplayouts"), |d| std::path::PathBuf::from(d).join("cod4rw").join("maplayouts"))
}

fn saved_text(map: &str) -> Option<String> {
    let name = format!("{map}.txt");
    [user_dir().join(&name), std::path::PathBuf::from("crates/game/assets/maplayouts").join(&name)]
        .iter()
        .find_map(|p| std::fs::read_to_string(p).ok())
        .or_else(|| MAPLAYOUTS.iter().find(|(n, _)| *n == name).map(|(_, t)| (*t).to_owned()))
}

/// Bevy's axes from CoD's (`crate::units::dir`) and back.
fn to_bevy(v: Vec3) -> Vec3 {
    Vec3::new(v.x, v.z, -v.y)
}
fn to_cod(v: Vec3) -> Vec3 {
    Vec3::new(v.x, -v.z, v.y)
}

fn srgb(c: f32) -> f32 {
    let c = c.max(0.0);
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// FNV-1a (stable between builds, unlike std's hasher).
fn fnv(h: &mut u64, bytes: &[u8]) {
    for &b in bytes {
        *h ^= b as u64;
        *h = h.wrapping_mul(0x100_0000_01b3);
    }
}

impl Layout {
    pub fn load(map: &str) -> Layout {
        saved_text(map).map(|t| Layout::parse(&t)).unwrap_or_default()
    }

    pub fn parse(text: &str) -> Layout {
        let mut out = Layout::default();
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        let mut any = false;
        for l in text.lines() {
            let Some(l) = l.split('#').next().map(str::trim) else { continue };
            let mut it = l.split_whitespace();
            let Some(model) = it.next() else { continue };
            let n: Vec<f32> = it.filter_map(|v| v.parse().ok()).collect();
            if n.len() < 8 || n.iter().any(|v| !v.is_finite()) {
                continue;
            }
            // (Effects and spawns don't light anything.)
            let lights = model == "@light" || model.starts_with("@hide:") || !model.starts_with('@');
            if !lights {
                continue;
            }
            any = true;
            fnv(&mut h, l.split_whitespace().collect::<Vec<_>>().join(" ").as_bytes());
            fnv(&mut h, b"\n");
            let origin = Vec3A::new(n[0], n[1], n[2]);
            let q = Quat::from_xyzw(n[3], n[4], n[5], n[6]);
            let q = if q.length_squared() > 1e-6 { q.normalize() } else { Quat::IDENTITY };
            let scale = n[7].clamp(0.05, 20.0);
            let params = n[8..].to_vec();
            if let Some(m) = model.strip_prefix("@hide:") {
                out.hidden.push((m.to_owned(), origin));
            } else if model == "@light" {
                let p = |i: usize| params.get(i).copied().unwrap_or(LIGHT_DEFAULTS[i]);
                // As the game draws it: `PointLight { intensity: 400k lm x
                // brightness, range: metres }`; candela = lumens / 4 pi.
                let candela = 400_000.0 * p(3).max(0.0) / (4.0 * std::f32::consts::PI);
                out.lights.push(Light {
                    origin,
                    intensity: Vec3::new(srgb(p(0)), srgb(p(1)), srgb(p(2))) * candela,
                    range: p(4).clamp(0.5, 60.0) / INCH,
                });
            } else {
                // The turn is in Bevy's axes: the same turn in CoD's.
                let axis = [Vec3::X, Vec3::Y, Vec3::Z].map(|e| Vec3A::from(to_cod(q * to_bevy(e))));
                out.models.push(Placed { model: model.to_owned(), origin, axis, scale });
            }
        }
        out.hash = if any { h } else { 0 };
        out
    }

    /// Whether the map's own `model` at `origin` is left out
    /// (`crate::map_editor::layout::hidden`).
    pub fn hides(&self, model: &str, origin: [f32; 3]) -> bool {
        let model = model.trim_start_matches(',');
        let at = Vec3A::from(origin);
        self.hidden.iter().any(|(m, p)| m.trim_start_matches(',') == model && p.distance(at) < 2.0)
    }
}

impl Light {
    /// Irradiance (lux) on a surface facing the light, `d` units away.
    pub fn lux(&self, d: f32) -> Vec3 {
        if d >= self.range {
            return Vec3::ZERO;
        }
        let m = (d * INCH).max(0.01);
        // Bevy's smooth range window.
        let r = d / self.range;
        let window = (1.0 - r * r * r * r).clamp(0.0, 1.0).powi(2);
        self.intensity * window / (m * m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_matches_the_game() {
        // A quarter turn about Bevy's up (Y) is a quarter turn about CoD's
        // up (Z): the model's X goes to CoD's -Y... as `units` maps it.
        let s = (std::f32::consts::FRAC_PI_4).sin();
        let l = Layout::parse(&format!("box 10 20 30 0 {s} 0 {s} 1\n@fx:x 0 0 0 0 0 0 1 1\n"));
        assert_eq!(l.models.len(), 1);
        let a = l.models[0].axis;
        // Bevy: rotating +X 90 deg about +Y gives -Z; Bevy -Z is CoD +Y.
        assert!((a[0] - Vec3A::Y).length() < 1e-4, "{a:?}");
        assert!((a[2] - Vec3A::Z).length() < 1e-4, "{a:?}");
        let fx_only = Layout::parse("@fx:x 0 0 0 0 0 0 1 1\n@spawn:tdm 0 0 0 0 0 0 1 1\n");
        assert_eq!(fx_only.hash, 0);
        assert_ne!(l.hash, 0);
    }
}

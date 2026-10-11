//! Each map's own fog, as its art script sets it
//! (`maps/createart/<map>_art.gsc`: `setExpFog(start, halfway, r, g, b,
//! time)`), in place of one haze for every map. CoD4's fog thickens
//! exponentially past `start`, half way there `halfway` further on
//! (`Scr_SetExponentialFog`: density ln 2 / halfway). Its colour is the
//! map's: warm sand on Backlot, brown on Bog, grey on Crash. A map whose
//! script turns fog off (Killhouse) or sets none (Wet Work) has none.

use crate::content::Content;
use crate::state::{GameState, Setup};
use crate::units::u;
use bevy::prelude::*;

pub struct FogPlugin;

impl Plugin for FogPlugin {
    fn build(&self, app: &mut App) {
        // Bevy's fog shader with CoD4's fog (a start, then exponential) as its
        // "linear" mode: Bevy's exponential fog has no start, which hazed
        // rooms and the near ground (Downpour's barns).
        let registry = app.world().resource::<bevy::asset::io::embedded::EmbeddedAssetRegistry>();
        registry.insert_asset(
            std::path::PathBuf::from(file!()).with_file_name("shaders/fog.wgsl"),
            std::path::Path::new("bevy_pbr/render/fog.wgsl"),
            include_bytes!("shaders/fog.wgsl").as_slice(),
        );
        app.add_systems(OnEnter(GameState::InGame), read_fog.in_set(Setup::Spawn))
            .add_systems(Update, apply_fog.run_if(crate::state::in_game));
    }
}

/// The map's fog: where it starts and how far on it's half way (CoD units),
/// and its colour (gamma space). `None`: no fog.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq)]
pub struct MapFog(pub Option<ExpFog>);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExpFog {
    pub start: f32,
    pub halfway: f32,
    pub color: [f32; 3],
    /// How thick it gets at most (MW2's seventh argument; CoD4's: 1).
    pub max_opacity: f32,
}

impl ExpFog {
    /// Bevy's exponential fog has no start: the density that matches
    /// CoD4's at three quarters thick (`start + 2 halfway`), where most of
    /// the fog's look is.
    fn density(&self) -> f32 {
        std::f32::consts::LN_2 * 2.0 / u(self.start + 2.0 * self.halfway)
    }
}

/// The fog an art script sets: its last `setExpFog` that isn't commented
/// out, unless it sets `scr_fog_disable` to 1.
pub fn parse_art(script: &str) -> Option<ExpFog> {
    let mut fog = None;
    for line in script.lines() {
        let line = line.split("//").next().unwrap_or("").trim();
        if line.contains("scr_fog_disable") && line.contains("\"1\"") {
            return None;
        }
        let Some(args) = line.strip_prefix("setExpFog").and_then(|r| r.trim().strip_prefix('(')).and_then(|r| r.split(')').next()) else {
            continue;
        };
        let v: Vec<f32> = args.split(',').filter_map(number).collect();
        if v.len() >= 5 && v[1] > 0.0 {
            // MW2's: start, halfway, r, g, b, max opacity, time; CoD4's has no
            // max opacity.
            let max_opacity = if v.len() >= 7 { v[5].clamp(0.0, 1.0) } else { 1.0 };
            fog = Some(ExpFog { start: v[0].max(0.0), halfway: v[1], color: [v[2], v[3], v[4]], max_opacity });
        }
    }
    fog
}

/// A script number: `0.5`, `.4` or `233/255`.
fn number(s: &str) -> Option<f32> {
    let s = s.trim();
    match s.split_once('/') {
        Some((a, b)) => Some(a.trim().parse::<f32>().ok()? / b.trim().parse::<f32>().ok()?),
        None => s.parse().ok(),
    }
}

fn read_fog(mut commands: Commands, content: Res<Content>, map: Res<crate::world::MapName>) {
    let name = format!("maps/createart/{}_art.gsc", map.0);
    let script = content.map().assets.iter().find_map(|a| match a {
        iw3::zone::Asset::RawFile(r) if r.name.eq_ignore_ascii_case(&name) => Some(String::from_utf8_lossy(&r.data).into_owned()),
        _ => None,
    });
    let fog = script.as_deref().and_then(parse_art);
    info!("fog: {}", fog.map_or_else(|| "none".to_owned(), |f| format!("from {} units, half way at {} more, colour {:?}", f.start, f.halfway, f.color)));
    commands.insert_resource(MapFog(fog));
}

/// Every view's fog is the map's (cameras come and go: splitscreen, the
/// killcam).
fn apply_fog(fog: Option<Res<MapFog>>, map: Option<Res<crate::world::MapName>>, mut cameras: Query<&mut DistanceFog>, mut knobs: Local<(f32, f32)>) {
    let Some(fog) = fog else { return };
    // (Live knobs, `crate::tune`: the showcase profile's fog reach and tone.)
    let profile = crate::atmos::climate::profile();
    // Per map (`fog.<map>.reach`, `fog.<map>.tone`): the showcase profile's
    // values by default, CoD4's (1) elsewhere.
    let map = map.map_or(String::new(), |m| m.0.clone());
    let now = (
        crate::tune::get(&format!("fog.{map}.reach"), profile.map_or(crate::atmos::climate::look().map_or(1.0, |l| l.fog_reach), |p| p.fog_reach)),
        crate::tune::get(&format!("fog.{map}.tone"), profile.map_or(crate::atmos::climate::look().map_or(1.0, |l| l.fog_tone), |p| p.fog_tone)),
    );
    let all = fog.is_changed() || *knobs != now;
    *knobs = now;
    for mut d in &mut cameras {
        if !all && !d.is_added() {
            continue;
        }
        match fog.0 {
            Some(f) => {
                let tone = now.1;
                let c = Color::srgb(f.color[0], f.color[1], f.color[2]).to_linear();
                d.color = Color::linear_rgba(c.red * tone, c.green * tone, c.blue * tone, f.max_opacity);
                // (`shaders/fog.wgsl`: start, halfway; `COD4RW_FOGNOSTART` the
                // old start-less fog, for A/B.)
                d.falloff = if std::env::var_os("COD4RW_FOGNOSTART").is_some() {
                    FogFalloff::Exponential { density: f.density() }
                } else {
                    let reach = now.0;
                    FogFalloff::Linear { start: u(f.start), end: u(f.halfway) * reach }
                };
            }
            // No fog: fully transparent.
            None => d.color = Color::srgba(0.0, 0.0, 0.0, 0.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn art_scripts_set_the_fog() {
        let crash = "\tsetdvar( \"scr_fog_disable\", \"0\" );\n\tsetExpFog( 500, 3500, 0.501961, 0.501961, 0.45098, 0 );\n";
        assert_eq!(parse_art(crash), Some(ExpFog { start: 500.0, halfway: 3500.0, color: [0.501961, 0.501961, 0.45098], max_opacity: 1.0 }));
        let strike = "setExpFog(0, 7000, 233/255, 207/255, 157/255, 3.0);";
        let f = parse_art(strike).unwrap();
        assert!((f.color[0] - 233.0 / 255.0).abs() < 1e-6 && f.halfway == 7000.0);
        // Commented out (Wet Work), or turned off (Killhouse): none.
        assert_eq!(parse_art("//setExpFog(300, 1400, 0.5, 0.5, 0.5, 0);"), None);
        assert_eq!(parse_art("setdvar( \"scr_fog_disable\", \"1\" );\nsetExpFog(300, 1400, 0.5, 0.5, 0.5, 0);"), None);
    }

    #[test]
    fn three_quarters_thick_at_start_plus_two_halfways() {
        let f = ExpFog { start: 0.0, halfway: 1000.0, color: [0.5; 3], max_opacity: 1.0 };
        let thick = 1.0 - (-f.density() * u(2000.0)).exp();
        assert!((thick - 0.75).abs() < 1e-4, "{thick}");
    }
}

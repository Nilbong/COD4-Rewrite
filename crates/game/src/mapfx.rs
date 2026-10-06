//! The map's ambient effects, as CoD4's multiplayer client plays them
//! (`CG_StartClientSideEffects`): `maps/mp/<map>_fx.gsc` names each effect
//! (`level._effect["smoke"] = loadfx("smoke/thin_black_smoke_M")`), and the
//! createfx script `maps/createfx/<map>_fx.gsc` places them
//! (`createOneshotEffect`: origin, angles, effect, delay). Each is started
//! once a match, its looping elements going on for good; negative delays
//! start them in the past, so smoke is already up. Their sound emitters are
//! [`crate::audio`]'s. `COD4RW_NOMAPFX=1` leaves them out.

use crate::content::Content;
use crate::fx::{Effects, Frame};
use crate::state::{GameState, in_game};
use bevy::prelude::*;
use std::collections::HashMap;

pub struct MapFxPlugin;

impl Plugin for MapFxPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Started>()
            .add_systems(OnEnter(GameState::InGame), |mut s: ResMut<Started>| s.0 = false)
            .add_systems(Update, start.run_if(in_game));
    }
}

/// This match's effects are going.
#[derive(Resource, Default)]
struct Started(bool);

/// One placed effect: CoD space origin and angles (degrees), the effect's
/// file, its delay (s).
#[derive(Debug, Clone, PartialEq)]
struct Placed {
    origin: [f32; 3],
    angles: [f32; 3],
    effect: String,
    delay: f32,
}

fn start(content: Res<Content>, map: Res<crate::world::MapName>, effects: Option<ResMut<Effects>>, mut started: ResMut<Started>) {
    let Some(mut effects) = effects.filter(|_| !started.0) else { return };
    started.0 = true;
    if std::env::var("COD4RW_NOMAPFX").is_ok_and(|v| !v.is_empty() && v != "0") || !crate::settings_apply::hud(crate::settings_apply::Hud::MapEffects) {
        return;
    }
    let script = |name: &str| {
        content.zones.iter().flat_map(|z| &z.assets).find_map(|a| match a {
            iw3::zone::Asset::RawFile(r) if r.name.eq_ignore_ascii_case(name) => Some(String::from_utf8_lossy(&r.data).into_owned()),
            _ => None,
        })
    };
    let names = script(&format!("maps/mp/{}_fx.gsc", map.0)).map(|s| effect_names(&s)).unwrap_or_default();
    let placed = script(&format!("maps/createfx/{}_fx.gsc", map.0)).map(|s| placements(&s, &names)).unwrap_or_default();
    for p in &placed {
        effects.play_ambient(&p.effect, frame(p.origin, p.angles), p.delay as f64 * 1000.0);
    }
    if !placed.is_empty() {
        info!("map fx: {} effects placed", placed.len());
    }
}

/// An effect's frame from CoD's origin and angles (`AnglesToAxis`:
/// forward, left, up).
fn frame(origin: [f32; 3], [pitch, yaw, roll]: [f32; 3]) -> Frame {
    let (sp, cp) = pitch.to_radians().sin_cos();
    let (sy, cy) = yaw.to_radians().sin_cos();
    let (sr, cr) = roll.to_radians().sin_cos();
    let forward = [cp * cy, cp * sy, -sp];
    let right = [-sr * sp * cy + cr * sy, -sr * sp * sy - cr * cy, -sr * cp];
    let up = [cr * sp * cy + sr * sy, cr * sp * sy - sr * cy, cr * cp];
    let left = right.map(|v| -v);
    let axis = Mat3::from_cols(crate::units::dir(forward), crate::units::dir(left), crate::units::dir(up));
    Frame { origin: crate::units::pos(origin), axis }
}

/// `level._effect["name"] = loadfx("file")` lines: name to file.
fn effect_names(script: &str) -> HashMap<String, String> {
    script
        .lines()
        .filter_map(|l| {
            let rest = l.trim().strip_prefix("level._effect")?;
            let name = rest.split('"').nth(1)?;
            let file = rest.split("loadfx").nth(1)?.split('"').nth(1)?;
            Some((name.to_owned(), file.to_owned()))
        })
        .collect()
}

/// The createfx script's `createOneshotEffect`s, with their effects' files.
fn placements(script: &str, names: &HashMap<String, String>) -> Vec<Placed> {
    let vec3 = |block: &str, key: &str| -> Option<[f32; 3]> {
        let inner = block.split(&format!("\"{key}\" ] = (")).nth(1)?.split(')').next()?;
        let v: Vec<f32> = inner.split(',').filter_map(|c| c.trim().parse().ok()).collect();
        (v.len() == 3).then(|| [v[0], v[1], v[2]])
    };
    script
        .split("ent = ")
        .filter(|b| b.contains("createOneshotEffect("))
        .filter_map(|block| {
            let id = block.split("\"fxid\" ] = \"").nth(1)?.split('"').next()?;
            let effect = names.get(id)?.clone();
            let delay = block
                .split("\"delay\" ] = ")
                .nth(1)
                .and_then(|d| d.split(';').next())
                .and_then(|d| d.trim().parse().ok())
                .unwrap_or(0.0);
            Some(Placed { origin: vec3(block, "origin")?, angles: vec3(block, "angles").unwrap_or([0.0; 3]), effect, delay })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FX: &str = r#"
	level._effect[ "wood" ]				 = loadfx( "explosions/grenadeExp_wood" );
	level._effect["thin_black_smoke_M"]					= loadfx( "smoke/thin_black_smoke_M" );
"#;
    const CREATEFX: &str = r#"
     	ent = maps\mp\_utility::createOneshotEffect( "thin_black_smoke_M" );
     	ent.v[ "origin" ] = ( 5538.67, -4614.16, 265.815 );
     	ent.v[ "angles" ] = ( 270, 0, 0 );
     	ent.v[ "fxid" ] = "thin_black_smoke_M";
     	ent.v[ "delay" ] = -15;

     	ent = maps\mp\_createfx::createLoopSound();
     	ent.v[ "origin" ] = ( 1, 2, 3 );
     	ent.v[ "soundalias" ] = "emt_tree_palm_rustle";
"#;

    #[test]
    fn reads_the_scripts() {
        let names = effect_names(FX);
        assert_eq!(names.get("wood").map(String::as_str), Some("explosions/grenadeExp_wood"));
        let placed = placements(CREATEFX, &names);
        assert_eq!(
            placed,
            vec![Placed { origin: [5538.67, -4614.16, 265.815], angles: [270.0, 0.0, 0.0], effect: "smoke/thin_black_smoke_M".into(), delay: -15.0 }]
        );
    }

    #[test]
    fn angles_point_the_effect() {
        // Pitch 270 (-90): forward is up.
        let f = frame([0.0; 3], [270.0, 0.0, 0.0]);
        assert!((f.axis.col(0) - Vec3::Y).length() < 1e-5);
        // Yaw 90: forward is CoD +Y (Bevy -Z), up stays up.
        let f = frame([0.0; 3], [0.0, 90.0, 0.0]);
        assert!((f.axis.col(0) - Vec3::NEG_Z).length() < 1e-5 && (f.axis.col(2) - Vec3::Y).length() < 1e-5);
    }
}

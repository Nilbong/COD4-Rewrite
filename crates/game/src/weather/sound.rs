//! The storm's sound: rain all round, on the open deck or on the roof
//! overhead, and thunder after each flash.
//!
//! The rain is one soft bed round the listener, without position: the
//! map's own loops (`emt_rain_metal` out in it, `emt_rain_roof` under
//! cover), their volumes set here by the rain and how open it is round
//! about (and the settings' effects volume, and `audio::audible()`, so
//! test runs stay silent). Thunder: `elm_thunder_strike`
//! close, `elm_thunder_distant` far, as late as sound takes to come from
//! the strike.

use super::occlusion::RainMap;
use super::Storm;
use bevy::prelude::*;

pub(super) fn build(app: &mut App) {
    app.add_systems(Update, (bed, thunder).run_if(crate::state::in_game).run_if(crate::atmos::climate::on));
}

/// Speed of sound, m/s.
const SOUND_SPEED: f32 = 343.0;
/// Strikes nearer than this (m) crack; further, they rumble.
const NEAR_STRIKE: f32 = 700.0;

/// A rain loop: out in the rain (else under cover), and its level now
/// (eased, 0..1).
#[derive(Component)]
struct Bed {
    open: bool,
    level: f32,
}

/// The bed's loudness at full rain, under the alias's own.
const BED_VOLUME: f32 = 0.35;

#[allow(clippy::too_many_arguments)]
fn bed(
    mut commands: Commands,
    time: Res<Time>,
    storm: Res<Storm>,
    map: Option<Res<RainMap>>,
    mut sfx: ResMut<crate::audio::Sfx>,
    camera: Query<&GlobalTransform, With<crate::player::MainCamera>>,
    mut beds: Query<&mut Bed>,
    mut sinks: Query<(&ChildOf, &mut AudioSink), With<crate::audio::FlatSound>>,
    mut started: Local<bool>,
) {
    let Some(eye) = camera.iter().next().map(|c| c.translation()) else { return };
    if !*started {
        *started = true;
        for (alias, open) in [("emt_rain_metal", true), ("emt_rain_roof", false)] {
            // (A transform: a sound plays only on an entity that has one.)
            let e = commands.spawn((Name::new(alias), Bed { open, level: 0.0 }, Transform::default())).id();
            sfx.play_flat_on(alias, e);
        }
        return;
    }
    // How open it is round about: rain on the deck heard through a doorway.
    let open = map.as_ref().map_or(1.0, |m| m.openness(eye, 3.0));
    let k = 1.0 - (-time.delta_secs() / 0.6).exp();
    for mut b in &mut beds {
        let target = storm.rain * if b.open { open } else { 1.0 - open };
        b.level += (target - b.level) * k;
    }
    let effects = crate::settings_apply::volume(crate::settings_apply::Sound::Effects);
    for (parent, mut sink) in &mut sinks {
        if let Ok(b) = beds.get(parent.parent()) {
            sink.set_volume(bevy::audio::Volume::Linear(b.level * BED_VOLUME * effects * crate::audio::audible()));
        }
    }
}

fn thunder(time: Res<Time>, storm: Res<Storm>, mut sfx: ResMut<crate::audio::Sfx>, camera: Query<&GlobalTransform, With<crate::player::MainCamera>>, mut heard: Local<u32>) {
    if storm.strikes == *heard {
        return;
    }
    *heard = storm.strikes;
    let (Some(strike), Some(eye)) = (storm.strike, camera.iter().next().map(|c| c.translation())) else { return };
    let d = strike.distance(eye);
    let alias = if d < NEAR_STRIKE { "elm_thunder_strike" } else { "elm_thunder_distant" };
    // Fainter the further, never gone (thunder carries).
    let gain = (NEAR_STRIKE / d.max(1.0)).clamp(0.35, 1.0);
    sfx.play_later_at_gain(alias, None, gain, time.elapsed_secs(), d / SOUND_SPEED);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thunder_comes_as_late_as_sound_takes() {
        assert!((1029.0 / SOUND_SPEED - 3.0).abs() < 0.01);
    }
}

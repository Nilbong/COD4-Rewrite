//! Debug aid: with `COD4RW_WALK=<seconds>` (and `COD4RW_SPAWN` to place
//! the player), Player 1 holds forward (`COD4RW_WALK_KEYS=KeyW,Space`
//! for others; `Space` taps jump each second, `KeyC` toggles crouch once, `Key@t` taps it at t) for that long, logging where they are every
//! tenth of a second, then the game exits. For movement bugs (stuck on
//! stairs, can't jump through a gap) without a human at the keyboard.

use crate::movement::{Mover, ViewAngles};
use crate::player::LocalPlayer;
use crate::splitscreen::PlayerInput;
use bevy::prelude::*;

pub struct WalkTestPlugin;

impl Plugin for WalkTestPlugin {
    fn build(&self, app: &mut App) {
        let Some(secs) = std::env::var("COD4RW_WALK").ok().and_then(|s| s.parse::<f32>().ok()) else { return };
        // `Key@seconds`: tapped once then.
        let keys: Vec<(KeyCode, Option<f32>)> = std::env::var("COD4RW_WALK_KEYS")
            .unwrap_or_else(|_| "KeyW".into())
            .split(',')
            .filter_map(|k| {
                let (k, at) = k.trim().split_once('@').map_or((k.trim(), None), |(k, t)| (k, t.parse::<f32>().ok()));
                let code = match k {
                "KeyW" => Some(KeyCode::KeyW),
                "KeyA" => Some(KeyCode::KeyA),
                "KeyD" => Some(KeyCode::KeyD),
                "ShiftLeft" => Some(KeyCode::ShiftLeft),
                "Space" => Some(KeyCode::Space),
                "KeyC" => Some(KeyCode::KeyC),
                _ => None,
                };
                code.map(|c| (c, at))
            })
            .collect();
        app.insert_resource(WalkTest { secs, keys, start: None, logged: 0.0 })
            .add_systems(PreUpdate, walk.after(crate::splitscreen::InputGathered).run_if(crate::state::in_game));
    }
}

#[derive(Resource)]
struct WalkTest {
    secs: f32,
    keys: Vec<(KeyCode, Option<f32>)>,
    start: Option<f32>,
    logged: f32,
}

fn walk(
    time: Res<Time>,
    mut test: ResMut<WalkTest>,
    mut player: Query<(&mut PlayerInput, &Transform, &Mover, &ViewAngles), With<LocalPlayer>>,
    mut exit: MessageWriter<AppExit>,
) {
    let Ok((mut input, tf, mover, view)) = player.single_mut() else { return };
    let now = time.elapsed_secs();
    // A moment to land first.
    let start = *test.start.get_or_insert(now + 1.0);
    let t = now - start;
    if t < 0.0 {
        return;
    }
    input.live = true;
    let dt = time.delta_secs();
    for &(k, at) in &test.keys {
        // Jump is tapped once a second, crouch (a toggle) once at the
        // start, `Key@t` once at t; the rest are held.
        let tap = match (k, at) {
            (_, Some(at)) => Some(t >= at && t < at + dt),
            (KeyCode::Space, _) => Some(t.fract() < 0.05),
            (KeyCode::KeyC, _) => Some(t < 0.05),
            _ => None,
        };
        if tap.unwrap_or(true) {
            input.keys.press(k);
        }
    }
    if t - test.logged >= 0.02 || t == 0.0 {
        test.logged = t;
        let p = crate::units::to_cod(tf.translation);
        let v = mover.velocity / crate::units::u(1.0);
        info!(
            "walk {t:.1}: at ({:.1}, {:.1}, {:.1}) speed {:.0} up {:.0} ground {} normal {:.2} yaw {:.0}{}{}",
            p[0],
            p[1],
            p[2],
            (v.x * v.x + v.z * v.z).sqrt(),
            v.y,
            mover.on_ground,
            mover.ground_normal,
            view.yaw.to_degrees() + 90.0,
            if mover.ladder.is_some() { " ladder" } else { "" },
            if mover.mantle.is_some() { " mantle" } else { "" }
        );
    }
    if t > test.secs {
        exit.write(AppExit::Success);
    }
}

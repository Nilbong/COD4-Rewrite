//! Debug aid: with `COD4RW_PADTEST=<dir>`, a virtual controller (Xbox, or
//! PlayStation with `COD4RW_PAD=ps`) plays the comma-separated steps in
//! `COD4RW_PADSTEPS`, one every 0.7 s from 3 s in, saving screenshots to
//! `<dir>`, then exits. Steps: a button to tap (`a b x y lb rb lt rt l3 r3
//! view menu up down left right`), `+name`/`-name` to hold and let go,
//! `ls:x;y`/`rs:x;y` to set a stick (y up), `wait:secs`, `shot:name`.

use bevy::input::gamepad::{
    GamepadConnection, GamepadConnectionEvent, RawGamepadAxisChangedEvent, RawGamepadButtonChangedEvent, RawGamepadEvent,
};
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

pub(super) fn register(app: &mut App) {
    if let Ok(dir) = std::env::var("COD4RW_PADTEST") {
        let steps = std::env::var("COD4RW_PADSTEPS").unwrap_or_default();
        // Debug runs stay quiet.
        app.insert_resource(GlobalVolume::new(bevy::audio::Volume::SILENT));
        app.insert_resource(PadTest {
            dir: dir.into(),
            steps: steps.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect(),
            next: 0,
            at: 3.0,
            pad: None,
            releases: Vec::new(),
            burst: None,
        })
        .add_systems(First, drive);
    }
}

/// The test's pad: gilrs doesn't know it, so it mustn't be sent rumble.
#[derive(Component)]
pub(crate) struct VirtualPad;

const STEP: f32 = 0.7;
const TAP: f32 = 0.12;

#[derive(Resource)]
struct PadTest {
    dir: std::path::PathBuf,
    steps: Vec<String>,
    next: usize,
    at: f32,
    pad: Option<Entity>,
    /// Tapped buttons to let go of, and when.
    releases: Vec<(GamepadButton, f32)>,
    /// `burst:name:n`: a screenshot every frame, n of them.
    burst: Option<(String, usize, usize)>,
}

fn button(name: &str) -> Option<GamepadButton> {
    use GamepadButton as B;
    Some(match name {
        "a" => B::South,
        "b" => B::East,
        "x" => B::West,
        "y" => B::North,
        "lb" => B::LeftTrigger,
        "rb" => B::RightTrigger,
        "lt" => B::LeftTrigger2,
        "rt" => B::RightTrigger2,
        "l3" => B::LeftThumb,
        "r3" => B::RightThumb,
        "view" => B::Select,
        "menu" => B::Start,
        "up" => B::DPadUp,
        "down" => B::DPadDown,
        "left" => B::DPadLeft,
        "right" => B::DPadRight,
        _ => return None,
    })
}

fn drive(
    mut commands: Commands,
    real: Res<Time<Real>>,
    mut test: ResMut<PadTest>,
    mut connect: MessageWriter<GamepadConnectionEvent>,
    mut raw: MessageWriter<RawGamepadEvent>,
    mut exit: MessageWriter<AppExit>,
    mut window: Single<&mut Window, With<bevy::window::PrimaryWindow>>,
) {
    let now = real.elapsed_secs();
    let pad = match test.pad {
        Some(p) => p,
        None => {
            let ps = std::env::var("COD4RW_PAD").is_ok_and(|s| s.eq_ignore_ascii_case("ps"));
            let e = commands.spawn(VirtualPad).id();
            let (name, vendor) = if ps { ("DualSense Wireless Controller", 0x054C) } else { ("Xbox Controller", 0x045E) };
            connect.write(GamepadConnectionEvent::new(
                e,
                GamepadConnection::Connected { name: name.into(), vendor_id: Some(vendor), product_id: None },
            ));
            test.pad = Some(e);
            // The real mouse cursor out of the way: resting over a menu it
            // would hover (focus) an item, and the steps would start there.
            window.set_cursor_position(Some(Vec2::ZERO));
            return;
        }
    };
    let set = |raw: &mut MessageWriter<RawGamepadEvent>, b: GamepadButton, v: f32| {
        raw.write(RawGamepadEvent::Button(RawGamepadButtonChangedEvent::new(pad, b, v)));
    };
    if let Some((name, done, total)) = test.burst.clone() {
        std::fs::create_dir_all(&test.dir).ok();
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(test.dir.join(format!("{name}_{done:03}.png"))));
        test.burst = (done + 1 < total).then(|| (name, done + 1, total));
    }
    test.releases.retain(|&(b, t)| {
        let due = now >= t;
        if due {
            set(&mut raw, b, 0.0);
        }
        !due
    });
    if now < test.at {
        return;
    }
    let Some(step) = test.steps.get(test.next).cloned() else {
        info!("pad test: done");
        exit.write(AppExit::Success);
        return;
    };
    test.next += 1;
    test.at = now + STEP;
    info!("pad test: {step}");
    if let Some((name, n)) = step.strip_prefix("burst:").and_then(|r| r.split_once(':')) {
        test.burst = Some((name.to_owned(), 0, n.parse().unwrap_or(30)));
    } else if let Some(name) = step.strip_prefix("shot:") {
        std::fs::create_dir_all(&test.dir).ok();
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(test.dir.join(format!("{name}.png"))));
    } else if let Some(secs) = step.strip_prefix("wait:").and_then(|s| s.parse::<f32>().ok()) {
        test.at = now + secs;
    } else if let Some((stick, xy)) = step.split_once(':') {
        let (x, y) = xy.split_once(';').and_then(|(x, y)| Some((x.parse::<f32>().ok()?, y.parse::<f32>().ok()?))).unwrap_or_default();
        let (ax, ay) = if stick == "rs" {
            (GamepadAxis::RightStickX, GamepadAxis::RightStickY)
        } else {
            (GamepadAxis::LeftStickX, GamepadAxis::LeftStickY)
        };
        raw.write(RawGamepadEvent::Axis(RawGamepadAxisChangedEvent::new(pad, ax, x)));
        raw.write(RawGamepadEvent::Axis(RawGamepadAxisChangedEvent::new(pad, ay, y)));
    } else if let Some(b) = step.strip_prefix('+').and_then(button) {
        set(&mut raw, b, 1.0);
    } else if let Some(b) = step.strip_prefix('-').and_then(button) {
        set(&mut raw, b, 0.0);
    } else if let Some(b) = button(&step) {
        set(&mut raw, b, 1.0);
        test.releases.push((b, now + TAP));
    } else {
        warn!("pad test: unknown step {step}");
    }
}

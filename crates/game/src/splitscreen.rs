//! Splitscreen co-op: up to four players on one screen, on the same side
//! against the bots, each with their own device (the keyboard and mouse, or
//! a controller), view, gun and HUD. Set up in Private Match (Splitscreen,
//! then each player's device), or for a run with `--splitscreen kbm,pad`.
//!
//! Player 1 is still the [`LocalPlayer`]: theirs are the saved stats, XP and
//! challenges, and the sound is heard from their view. Every local player
//! has a [`LocalSlot`], its device's input ([`PlayerInput`]) and a camera
//! pair ([`SlotCamera`], [`SlotViewModelCamera`]) drawing its part of the
//! window ([`region`]). Killcams, Bodycam gunplay, lens scopes and the sun's
//! flare are left out with more than one player.

use crate::gamepad::{ActiveDevice, PadFrame, PadKind};
use crate::player::LocalPlayer;
use bevy::camera::Viewport;
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use std::sync::atomic::{AtomicUsize, Ordering};

pub const MAX_PLAYERS: usize = 4;

pub struct SplitscreenPlugin;

/// Each local player's [`PlayerInput`] gathered (`PreUpdate`).
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct InputGathered;

impl Plugin for SplitscreenPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LocalPlayers>()
            .add_systems(OnEnter(crate::state::GameState::InGame), begin.in_set(crate::state::Setup::Content))
            .add_systems(OnExit(crate::state::GameState::InGame), || set_count(1))
            .add_systems(
            PreUpdate,
            (assign_pads, gather_input).chain().in_set(InputGathered).after(crate::gamepad::PadSet).run_if(crate::state::in_game),
        )
        .add_systems(Update, lighter_shadows.run_if(crate::state::in_game.and_then(|| active())));
        if let Ok(dir) = std::env::var("COD4RW_SPLITTEST") {
            app.insert_resource(GlobalVolume::new(bevy::audio::Volume::SILENT))
                .insert_resource(SplitTest { dir: dir.into(), pads: Vec::new(), shots: 0, logged: 0.0 })
                .add_systems(First, split_test);
        }
    }
}

/// What a local player plays with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Device {
    KeyboardMouse,
    Pad(Entity),
    /// The next controller nobody has: one connected later (or again).
    NextPad,
}

/// The match's local players' devices, Player 1's first. One (or none):
/// an ordinary match, played with anything.
#[derive(Resource, Clone, Debug, Default)]
pub struct LocalPlayers {
    pub devices: Vec<Device>,
    /// Each one's team (the Private Match lobby's); none listed: all on
    /// the match's player team.
    pub teams: Vec<crate::combat::Team>,
}

impl LocalPlayers {
    /// `kbm,pad,pad`: the keyboard and mouse or the next free controller,
    /// for each player in turn.
    pub fn parse(spec: &str) -> LocalPlayers {
        let devices = spec
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .take(MAX_PLAYERS)
            .map(|s| if s.eq_ignore_ascii_case("kbm") { Device::KeyboardMouse } else { Device::NextPad })
            .collect();
        LocalPlayers { devices, teams: Vec::new() }
    }

    pub fn count(&self) -> usize {
        self.devices.len().clamp(1, MAX_PLAYERS)
    }
}

static COUNT: AtomicUsize = AtomicUsize::new(1);

/// Local players in the match (1 without splitscreen).
pub fn count() -> usize {
    COUNT.load(Ordering::Relaxed)
}

/// More than one local player.
pub fn active() -> bool {
    count() > 1
}

/// Set as a match starts.
pub fn set_count(n: usize) {
    COUNT.store(n.clamp(1, MAX_PLAYERS), Ordering::Relaxed);
}

/// A local (human) player, by their place: 0 is Player 1.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalSlot(pub usize);

/// A local player's world camera.
#[derive(Component, Clone, Copy, Debug)]
pub struct SlotCamera(pub usize);

/// A local player's viewmodel camera (a child of its world camera).
#[derive(Component, Clone, Copy, Debug)]
pub struct SlotViewModelCamera(pub usize);

/// The camera the splitscreen UI (menus, HUDs) is drawn with, over the
/// whole window.
#[derive(Component)]
pub struct OverlayCamera;

/// Render layer of a player's gun: Player 1's is [`VIEWMODEL_LAYER`]; the
/// others' are past the menus' preview layers (8..28).
///
/// [`VIEWMODEL_LAYER`]: crate::player::VIEWMODEL_LAYER
pub fn viewmodel_layer(slot: usize) -> usize {
    if slot == 0 { crate::player::VIEWMODEL_LAYER } else { 27 + slot }
}

/// Every player's gun layer.
pub fn viewmodel_layers() -> Vec<usize> {
    (0..MAX_PLAYERS).map(viewmodel_layer).collect()
}

/// Render layer of a player's own body in splitscreen: the other players'
/// cameras see it, its own only in third person.
pub fn body_layer(slot: usize) -> usize {
    31 + slot
}

/// What a player's world camera draws in splitscreen: the world, the
/// other players' bodies, and in third person its own.
pub fn world_layers(slot: usize, count: usize, third_person: bool) -> bevy::camera::visibility::RenderLayers {
    let layers: Vec<usize> =
        std::iter::once(0).chain((0..count).filter(|&s| s != slot || third_person).map(body_layer)).chain((!third_person).then(|| crate::first_person::body::layer(slot))).collect();
    bevy::camera::visibility::RenderLayers::from_layers(&layers)
}

/// A player's part of the window, as fractions (top left, size): two
/// players one over the other, three with Player 1 along the top, four in
/// quarters.
pub fn region(slot: usize, count: usize) -> Rect {
    let (x, y, w, h) = match (count, slot) {
        (2, 0) => (0.0, 0.0, 1.0, 0.5),
        (2, _) => (0.0, 0.5, 1.0, 0.5),
        (3, 0) => (0.0, 0.0, 1.0, 0.5),
        (3, 1) => (0.0, 0.5, 0.5, 0.5),
        (3, _) => (0.5, 0.5, 0.5, 0.5),
        (4, s) => (0.5 * (s % 2) as f32, 0.5 * (s / 2) as f32, 0.5, 0.5),
        _ => (0.0, 0.0, 1.0, 1.0),
    };
    Rect::new(x, y, x + w, y + h)
}

/// A player's part of a window `size` (physical pixels) as a viewport; none
/// without splitscreen.
pub fn viewport(slot: usize, count: usize, size: UVec2) -> Option<Viewport> {
    if count < 2 {
        return None;
    }
    let r = region(slot, count);
    let at = (r.min * size.as_vec2()).round().as_uvec2();
    let end = (r.max * size.as_vec2()).round().as_uvec2().min(size);
    Some(Viewport { physical_position: at, physical_size: (end - at).max(UVec2::ONE), ..default() })
}

/// A player's part of the window in logical pixels (top left, size).
pub fn logical_rect(slot: usize, count: usize, window: &Window) -> (Vec2, Vec2) {
    let r = region(slot, count);
    let size = Vec2::new(window.width(), window.height());
    (r.min * size, r.size() * size)
}

/// One player's input this frame, from their device: what the gameplay
/// systems read instead of the keyboard, mouse and pad resources. Without
/// splitscreen Player 1's is all of them, as before (a pad presses the
/// keyboard's bindings, `crate::gamepad`).
#[derive(Component, Default, Clone, Debug)]
pub struct PlayerInput {
    pub keys: ButtonInput<KeyCode>,
    pub mouse: ButtonInput<MouseButton>,
    /// Mouse movement (counts).
    pub look: Vec2,
    /// Mouse wheel (and the pad's Y, next weapon).
    pub scroll: f32,
    pub pad: PadFrame,
    /// The player's controller, while they play with one.
    pub pad_kind: Option<PadKind>,
    pub pad_entity: Option<Entity>,
    /// Their input reaches the game: no menu is up, and the mouse is the
    /// game's for the keyboard and mouse.
    pub live: bool,
}

impl PlayerInput {
    /// The [Use] key as the player's device names it (the key it's bound
    /// to on the keyboard).
    pub fn use_key(&self) -> String {
        match self.pad_kind {
            Some(PadKind::PlayStation) => "Square".into(),
            Some(PadKind::Xbox) => "X".into(),
            None => USE_KEY.lock().map_or_else(|_| "F".into(), |k| k.clone()),
        }
    }
}

/// The keyboard's Use key's name, for the hints (`crate::settings_apply`).
pub static USE_KEY: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// The match's local players, known before anything spawns for them
/// (none but Player 1 while spectating).
fn begin(players: Res<LocalPlayers>, spectate: Option<Res<crate::bots::SpectateArg>>) {
    let spectating = spectate.is_some_and(|s| s.0.is_some());
    set_count(if spectating { 1 } else { players.count() });
}

/// Each player's view draws the sun's shadows anew, which is most of what
/// another view costs. In splitscreen they're lighter: two cascades rather
/// than four, and the map's props cast none (their shadows are in its
/// lightmaps already). With four players that's about 35 ms a frame down to
/// 27 on the test machine.
fn lighter_shadows(
    mut commands: Commands,
    time: Res<Time>,
    // (Not the guns' own suns: one small cascade each, `model_lighting`.)
    suns: Query<(Entity, &DirectionalLight, &bevy::light::CascadeShadowConfig), Without<crate::model_lighting::ViewModelSun>>,
    names: Query<(Entity, &Name)>,
    children: Query<&Children>,
    casting: Query<(), (With<Mesh3d>, Without<bevy::light::NotShadowCaster>)>,
    mut next: Local<f32>,
) {
    for (e, light, cascades) in &suns {
        if light.shadow_maps_enabled && cascades.bounds.len() != 2 {
            let config = bevy::light::CascadeShadowConfigBuilder { num_cascades: 2, maximum_distance: 120.0, first_cascade_far_bound: 12.0, ..default() };
            commands.entity(e).insert(config.build());
        }
    }
    // The props (checked now and then: they may still be arriving).
    let now = time.elapsed_secs();
    if now < *next {
        return;
    }
    *next = now + 1.0;
    let Some((root, _)) = names.iter().find(|(_, n)| n.as_str() == "static models") else { return };
    let mut count = 0;
    for model in children.iter_descendants(root).filter(|&m| casting.contains(m)) {
        commands.entity(model).insert(bevy::light::NotShadowCaster);
        count += 1;
    }
    if count > 0 {
        debug!("splitscreen: {count} props cast no shadows");
    }
}

/// Give waiting players ([`Device::NextPad`]) a free controller, and take
/// back one that went away.
fn assign_pads(mut players: ResMut<LocalPlayers>, devices: Res<crate::gamepad::Devices>) {
    if players.devices.len() < 2 {
        return;
    }
    let present = |e: Entity| devices.0.iter().any(|(d, _)| *d == Device::Pad(e));
    for i in 0..players.devices.len() {
        if let Device::Pad(e) = players.devices[i] {
            if !present(e) {
                info!("splitscreen: player {}'s controller went away", i + 1);
                players.devices[i] = Device::NextPad;
            }
        }
        if players.devices[i] == Device::NextPad {
            let free = devices.0.iter().map(|(d, _)| *d).find(|d| matches!(d, Device::Pad(_)) && !players.devices.contains(d));
            if let Some(d) = free {
                let name = devices.0.iter().find(|(x, _)| *x == d).map_or("", |(_, n)| n.as_str());
                info!("splitscreen: player {} plays with {name}", i + 1);
                players.devices[i] = d;
            }
        }
    }
}

/// Fill each local player's [`PlayerInput`] from their device.
#[allow(clippy::too_many_arguments)]
fn gather_input(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    pad: Res<PadFrame>,
    active_device: Res<ActiveDevice>,
    slot_pads: Res<crate::gamepad::SlotPads>,
    players: Res<LocalPlayers>,
    cursor: Single<&CursorOptions, With<PrimaryWindow>>,
    fe: Option<Res<crate::ui::Frontend>>,
    mut pawns: Query<(&LocalSlot, &mut PlayerInput, Has<LocalPlayer>)>,
    settings: Res<crate::settings::Settings>,
    injected: Res<crate::gamepad::PadInjected>,
    mut translated: Local<std::collections::HashMap<usize, crate::bindings::Translated>>,
    mut bindings: Local<Option<crate::bindings::Bindings>>,
) {
    let grabbed = cursor.grab_mode != CursorGrabMode::None;
    // The keyboard and mouse as the game reads them: rebound keys pressing
    // their actions' default keys ([`crate::bindings`]).
    if settings.is_changed() || bindings.is_none() {
        *bindings = Some(settings.bindings());
    }
    let b = bindings.as_ref().expect("set above");
    let forward = b.of(crate::bindings::Action::Forward).iter().any(|i| match i {
        crate::bindings::Input::Key(k) => keys.pressed(*k),
        crate::bindings::Input::Mouse(m) => mouse.pressed(*m),
    });
    let mut translate = |slot: usize| {
        let t = translated.entry(slot).or_default();
        crate::bindings::translate(&keys, &mouse, (&injected.keys, &injected.mouse), b, forward, t);
        (t.keys.clone(), t.mouse.clone())
    };
    let global_menu = crate::ui::menu_open(fe.as_deref());
    for (slot, mut input, _) in &mut pawns {
        // A player's own menu (splitscreen) stops their play alone.
        let menu = global_menu || fe.as_deref().is_some_and(|fe| fe.slot_menu_open(slot.0));
        if !active() {
            let (keys, mouse) = translate(slot.0);
            *input = PlayerInput {
                keys,
                mouse,
                look: motion.delta,
                scroll: scroll.delta.y,
                pad: pad.clone(),
                pad_kind: active_device.pad,
                pad_entity: active_device.entity.filter(|_| active_device.pad.is_some()),
                live: grabbed,
            };
            // Headquarters has no fighting.
            crate::hq::strip(&mut input);
            continue;
        }
        *input = match players.devices.get(slot.0).copied().unwrap_or(Device::NextPad) {
            Device::KeyboardMouse => {
                let (keys, mouse) = translate(slot.0);
                PlayerInput {
                keys,
                mouse,
                look: motion.delta,
                scroll: scroll.delta.y,
                live: grabbed && !menu,
                ..default()
            }
            }
            Device::Pad(e) => match slot_pads.0.get(&e) {
                Some(p) => PlayerInput {
                    keys: p.keys.clone(),
                    mouse: p.mouse.clone(),
                    scroll: p.scroll,
                    pad: p.frame.clone(),
                    pad_kind: Some(p.kind),
                    pad_entity: Some(e),
                    live: !menu,
                    ..default()
                },
                None => PlayerInput::default(),
            },
            Device::NextPad => PlayerInput::default(),
        };
    }
}

/// Debug aid: with `COD4RW_SPLITTEST=<dir>` (and `COD4RW_SPLITSCREEN` for
/// the players, e.g. `kbm,pad,pad,pad`), virtual controllers play Players 2
/// to 4: Player 2 walks forward turning right, Player 3 fires then aims,
/// Player 4 crouches, stands and throws a frag; Player 1 is left alone.
/// Every player's state is logged each half second and the window
/// screenshot at each step, then the game exits.
#[derive(Resource)]
struct SplitTest {
    dir: std::path::PathBuf,
    pads: Vec<Entity>,
    shots: usize,
    logged: f32,
}

#[allow(clippy::too_many_arguments)]
fn split_test(
    mut commands: Commands,
    real: Res<Time<Real>>,
    mut test: ResMut<SplitTest>,
    mut players: ResMut<LocalPlayers>,
    mut connect: MessageWriter<bevy::input::gamepad::GamepadConnectionEvent>,
    mut raw: MessageWriter<bevy::input::gamepad::RawGamepadEvent>,
    pawns: Query<(&LocalSlot, &crate::combat::Pawn, &Transform, &crate::movement::ViewAngles, &crate::movement::Mover, &crate::weapons::WeaponState, Option<&crate::grenades::Grenades>, Has<crate::combat::Dead>)>,
    mut exit: MessageWriter<AppExit>,
) {
    use bevy::input::gamepad::{GamepadConnection, GamepadConnectionEvent, RawGamepadAxisChangedEvent, RawGamepadButtonChangedEvent, RawGamepadEvent};
    use bevy::render::view::screenshot::{Screenshot, save_to_disk};
    let t = real.elapsed_secs();
    let wanted = players.devices.len().saturating_sub(1);
    // Virtual controllers for the players past the first, as the match
    // starts (not whatever real pads are plugged in).
    if test.pads.len() < wanted {
        let names = [("Xbox Controller", 0x045E, 0x0B12), ("DualSense Edge Wireless Controller", 0x054C, 0x0DF2), ("Xbox Controller", 0x045E, 0x0B12)];
        for i in test.pads.len()..wanted {
            let (name, vendor, product) = names[i % names.len()];
            let e = commands.spawn(crate::gamepad::test::VirtualPad).id();
            connect.write(GamepadConnectionEvent::new(
                e,
                GamepadConnection::Connected { name: name.into(), vendor_id: Some(vendor), product_id: Some(product) },
            ));
            test.pads.push(e);
        }
        return;
    }
    for (i, &e) in test.pads.iter().enumerate() {
        if players.devices.get(i + 1) != Some(&Device::Pad(e)) {
            if let Some(d) = players.devices.get_mut(i + 1) {
                *d = Device::Pad(e);
            }
        }
    }
    let button = |raw: &mut MessageWriter<RawGamepadEvent>, pad: usize, b: GamepadButton, on: bool| {
        if let Some(&e) = test.pads.get(pad) {
            raw.write(RawGamepadEvent::Button(RawGamepadButtonChangedEvent::new(e, b, on as u8 as f32)));
        }
    };
    let axis = |raw: &mut MessageWriter<RawGamepadEvent>, pad: usize, a: GamepadAxis, v: f32| {
        if let Some(&e) = test.pads.get(pad) {
            raw.write(RawGamepadEvent::Axis(RawGamepadAxisChangedEvent::new(e, a, v)));
        }
    };
    let during = |from: f32, to: f32| (from..to).contains(&t);
    // Player 2: walk forward turning right, 10 to 12 s.
    let walking = during(10.0, 12.0);
    axis(&mut raw, 0, GamepadAxis::LeftStickY, if walking { 1.0 } else { 0.0 });
    axis(&mut raw, 0, GamepadAxis::RightStickX, if walking { 0.7 } else { 0.0 });
    // Player 3: fire 10 to 11 s, aim 12 to 13.5 s.
    button(&mut raw, 1, GamepadButton::RightTrigger2, during(10.0, 11.0));
    button(&mut raw, 1, GamepadButton::LeftTrigger2, during(12.0, 13.5));
    // Player 4: crouch (B tapped) at 10 s, stand (A) at 11.5 s, cook a frag
    // (RB held) 12.5 to 13.2 s.
    button(&mut raw, 2, GamepadButton::East, during(10.0, 10.1));
    button(&mut raw, 2, GamepadButton::South, during(11.5, 11.6));
    button(&mut raw, 2, GamepadButton::RightTrigger, during(12.5, 13.2));
    // Pictures and a log.
    const SHOTS: [(f32, &str); 5] = [(9.5, "start"), (10.8, "moving"), (12.4, "after"), (13.2, "aiming"), (14.5, "end")];
    if let Some(&(at, name)) = SHOTS.get(test.shots) {
        if t >= at {
            std::fs::create_dir_all(&test.dir).ok();
            commands.spawn(Screenshot::primary_window()).observe(save_to_disk(test.dir.join(format!("split_{}_{name}.png", test.shots))));
            test.shots += 1;
        }
    }
    if t >= 8.0 && t - test.logged >= 0.5 {
        test.logged = t;
        let mut rows: Vec<_> = pawns.iter().collect();
        rows.sort_by_key(|r| r.0.0);
        for (slot, pawn, tf, view, mover, w, g, dead) in rows {
            let p = crate::units::to_cod(tf.translation);
            info!(
                "split test {t:.1}: player {} {} at ({:.0}, {:.0}, {:.0}) yaw {:.0} pitch {:.0} {:?} ads {:.2} clip {} frags {}{}",
                slot.0 + 1,
                pawn.name,
                p[0],
                p[1],
                p[2],
                view.yaw.to_degrees(),
                view.pitch.to_degrees(),
                mover.stance,
                w.ads,
                w.clip,
                g.map_or(0, |g| g.frags),
                if dead { " dead" } else { "" }
            );
        }
    }
    if t > 15.0 {
        exit.write(AppExit::Success);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regions_tile_the_window() {
        for count in 1..=MAX_PLAYERS {
            let area: f32 = (0..count).map(|s| region(s, count).size().x * region(s, count).size().y).sum();
            assert!((area - 1.0).abs() < 1e-6, "{count} players cover {area}");
        }
        assert_eq!(region(1, 2), Rect::new(0.0, 0.5, 1.0, 1.0));
        assert_eq!(region(3, 4), Rect::new(0.5, 0.5, 1.0, 1.0));
        let v = viewport(1, 4, UVec2::new(1920, 1080)).unwrap();
        assert_eq!((v.physical_position, v.physical_size), (UVec2::new(960, 0), UVec2::new(960, 540)));
        assert!(viewport(0, 1, UVec2::new(1920, 1080)).is_none());
    }

    #[test]
    fn device_specs() {
        let p = LocalPlayers::parse("kbm, pad,pad,pad,pad");
        assert_eq!(p.devices, vec![Device::KeyboardMouse, Device::NextPad, Device::NextPad, Device::NextPad]);
        assert_eq!(LocalPlayers::default().count(), 1);
    }

    #[test]
    fn layers_are_distinct() {
        let mut all: Vec<usize> = (0..MAX_PLAYERS).flat_map(|s| [viewmodel_layer(s), body_layer(s)]).collect();
        all.extend([0, crate::world::SHADOW_PROXY_LAYER]);
        let n = all.len();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), n);
        assert!(viewmodel_layers().iter().skip(1).all(|&l| l >= 28));
    }
}

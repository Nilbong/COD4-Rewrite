//! Controllers: an Xbox or PlayStation pad plays alongside keyboard and
//! mouse. Its buttons feed the game's existing key and mouse bindings (CoD4's
//! console layout: A jump, B crouch/hold prone, X reload, Y switch weapon,
//! LT aim, RT fire, RB frag, LB special grenade, L3 sprint, D-pad right kill
//! streak, View scores, Menu pause; aiming, L3/R3 lean in Bodycam), the
//! sticks move
//! and look, and in the menus the D-pad or left stick moves focus, A selects
//! and B goes back.
//!
//! Whichever device was used last is the active one: its button glyphs
//! ([`glyphs`]) replace the keyboard hints, and aim assist ([`assist`])
//! only helps the sticks.

pub mod assist;
pub mod glyphs;
pub mod playstation;
pub mod prompts;
mod sony;
pub(crate) mod test;

use crate::state::GameState;
use bevy::input::InputSystems;
use bevy::input::gamepad::{GamepadConnection, GamepadConnectionEvent, GamepadRumbleIntensity, GamepadRumbleRequest};
use bevy::input::keyboard::KeyboardInput;
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseButtonInput};
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use std::time::Duration;

pub struct GamepadPlugin;

impl Plugin for GamepadPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreUpdate, take_settings.before(PadSet));
        app.insert_resource(PadSettings::from_env())
            .init_resource::<ActiveDevice>()
            .init_resource::<PadFrame>()
            .init_resource::<Bindings>()
            .add_systems(Startup, glyphs::build)
            .init_resource::<SlotPads>()
            .init_resource::<PadInjected>()
            .init_resource::<Devices>()
            .add_systems(
                PreUpdate,
                (detect_device, list_devices, read_pad, feed_bindings, read_slot_pads).chain().in_set(PadSet).after(InputSystems),
            )
            .add_systems(Update, grab_on_pad.run_if(crate::state::in_game.and_then(crate::ui::no_ingame_menu)))
            .add_systems(Update, rumble.run_if(crate::state::in_game))
            .add_plugins((assist::AssistPlugin, prompts::PromptsPlugin));
        sony::register(app);
        test::register(app);
    }
}

/// Reading the pads, before the players' input is gathered
/// ([`crate::splitscreen`]).
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct PadSet;

/// Which glyphs to show: Xbox (also any XInput or unknown pad) or
/// PlayStation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum PadKind {
    #[default]
    Xbox,
    PlayStation,
}

impl PadKind {
    /// Sony's USB vendor id, or a name gilrs gives Sony pads.
    fn of(vendor: Option<u16>, name: &str) -> PadKind {
        let name = name.to_ascii_lowercase();
        let sony = ["playstation", "dualshock", "dualsense", "ps3", "ps4", "ps5", "sony"].iter().any(|n| name.contains(n))
            // A DualShock 4 calls itself just "Wireless Controller".
            || name == "wireless controller";
        if vendor == Some(0x054C) || sony { PadKind::PlayStation } else { PadKind::Xbox }
    }
}

/// The device used last: `pad` is set while a controller is in use.
#[derive(Resource, Default, Clone, Copy, Debug)]
pub struct ActiveDevice {
    pub pad: Option<PadKind>,
    /// The pad in use (for rumble).
    pub entity: Option<Entity>,
}

/// Look speeds, inversion and aim assist. `COD4RW_PAD_SENS` scales the
/// look speed, `COD4RW_PAD_INVERT=1` inverts pitch, `COD4RW_PAD_AIM_ASSIST=0`
/// turns aim assist off and `COD4RW_PAD=ps|xbox` forces the glyphs.
#[derive(Resource, Clone, Copy, Debug)]
pub struct PadSettings {
    pub sensitivity: f32,
    /// Aiming down the sights' share of that.
    pub ads_sensitivity: f32,
    pub invert_pitch: bool,
    pub aim_assist: bool,
    pub force_kind: Option<PadKind>,
    pub rumble: bool,
}

impl PadSettings {
    fn from_env() -> PadSettings {
        let var = |k: &str| std::env::var(k).ok();
        PadSettings {
            sensitivity: var("COD4RW_PAD_SENS").and_then(|s| s.parse().ok()).unwrap_or(1.0f32).clamp(0.1, 5.0),
            ads_sensitivity: 1.0,
            invert_pitch: var("COD4RW_PAD_INVERT").is_some_and(|s| s == "1"),
            aim_assist: var("COD4RW_PAD_AIM_ASSIST").is_none_or(|s| s != "0"),
            force_kind: var("COD4RW_PAD").and_then(|s| match s.to_ascii_lowercase().as_str() {
                "ps" | "playstation" => Some(PadKind::PlayStation),
                "xbox" => Some(PadKind::Xbox),
                _ => None,
            }),
            rumble: var("COD4RW_PAD_RUMBLE").is_none_or(|s| s != "0"),
        }
    }
}

/// This frame's pad state, for the systems that read sticks directly.
#[derive(Resource, Default, Clone, Debug)]
pub struct PadFrame {
    /// Left stick after its deadzone: x right, y forward, -1..1.
    pub movement: Vec2,
    /// Right stick after its deadzone and response curve: x right, y up.
    pub look: Vec2,
    /// How long the right stick has been pushed all the way, in seconds.
    pub look_pinned: f32,
    /// L3 sprint, held until the stick leaves forward or the gun is used.
    pub sprint: bool,
    pub ads: bool,
    pub ads_pressed: bool,
    pub fire: bool,
    /// Menu focus moves this way (D-pad or left stick, with key repeat).
    pub menu_dir: Option<IVec2>,
    /// Right stick in the menus: turns the gun preview.
    pub menu_rotate: Vec2,
    /// A or B went down in the menus (they end typing into a text field).
    pub menu_done: bool,
    /// X held in the game: "use" (planting and defusing, [`crate::modes::sd`]).
    pub interact: bool,
    /// L3 held in the game: holding the breath, scoped ([`crate::perks`]).
    pub breath: bool,
    /// A button other than Menu went down, or a stick moved: the game takes
    /// the mouse back (Menu lets it go).
    pub wake: bool,
}

/// Radial deadzone, then rescaled to 0..1.
const STICK_DEADZONE: f32 = 0.16;
/// The settings' deadzones (left, right), as f32 bits.
static DEADZONES: [std::sync::atomic::AtomicU32; 2] =
    [std::sync::atomic::AtomicU32::new(0x3E23_D70A), std::sync::atomic::AtomicU32::new(0x3E23_D70A)];
/// The settings' Tactical button layout: melee on B, crouch on R3.
static TACTICAL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn tactical() -> bool {
    TACTICAL.load(std::sync::atomic::Ordering::Relaxed)
}

/// A stick after the settings' deadzone (0 left, 1 right).
fn stick(v: Vec2, which: usize) -> Vec2 {
    let dz = f32::from_bits(DEADZONES[which].load(std::sync::atomic::Ordering::Relaxed)).clamp(0.0, 0.9);
    let len = v.length();
    if len <= dz {
        return Vec2::ZERO;
    }
    v / len * ((len - dz) / (1.0 - dz)).min(1.0)
}

/// The settings' controller options ([`crate::settings_apply`]), when they
/// change.
fn take_settings(mut settings: ResMut<PadSettings>) {
    let Some(v) = crate::settings_apply::PAD.lock().ok().and_then(|mut p| p.take()) else { return };
    // The environment's debug settings still win.
    if std::env::var_os("COD4RW_PAD_SENS").is_none() {
        settings.sensitivity = v.sensitivity.clamp(0.1, 5.0);
    }
    settings.ads_sensitivity = v.ads.clamp(0.1, 4.0);
    settings.invert_pitch = v.invert || std::env::var("COD4RW_PAD_INVERT").is_ok_and(|s| s == "1");
    settings.aim_assist = v.aim_assist;
    settings.rumble = v.rumble;
    if std::env::var_os("COD4RW_PAD").is_none() {
        settings.force_kind = match v.glyphs.as_str() {
            "xbox" => Some(PadKind::Xbox),
            "ps" => Some(PadKind::PlayStation),
            _ => None,
        };
    }
    DEADZONES[0].store(v.deadzones.0.to_bits(), std::sync::atomic::Ordering::Relaxed);
    DEADZONES[1].store(v.deadzones.1.to_bits(), std::sync::atomic::Ordering::Relaxed);
    TACTICAL.store(v.tactical, std::sync::atomic::Ordering::Relaxed);
}
/// Triggers count as pressed past this.
const TRIGGER_PRESS: f32 = 0.3;
const TRIGGER_RELEASE: f32 = 0.2;
/// B held this long goes prone (shorter is a crouch).
const PRONE_HOLD: f32 = 0.3;
/// Menu focus repeat while the D-pad or stick is held.
const REPEAT_DELAY: f32 = 0.4;
const REPEAT_EVERY: f32 = 0.12;

fn deadzone(v: Vec2) -> Vec2 {
    let len = v.length();
    if len <= STICK_DEADZONE {
        return Vec2::ZERO;
    }
    v / len * ((len - STICK_DEADZONE) / (1.0 - STICK_DEADZONE)).min(1.0)
}

/// Stick response: fine aim near the centre, full speed at the edge.
fn look_curve(v: Vec2) -> Vec2 {
    let len = v.length();
    if len == 0.0 { v } else { v / len * len.powf(1.8) }
}

/// The last device touched becomes the active one. Raw keyboard and mouse
/// messages count; the presses [`feed_bindings`] makes don't. A stick counts
/// as it's pushed, not while it stays pushed (some keyboards also show up as
/// a pad, with an axis stuck off centre).
#[allow(clippy::too_many_arguments)]
fn detect_device(
    mut active: ResMut<ActiveDevice>,
    settings: Res<PadSettings>,
    pads: Query<(Entity, &Gamepad, Option<&Name>)>,
    virtual_pads: Query<(), With<test::VirtualPad>>,
    sony: Option<Res<sony::SonyPads>>,
    mut connections: MessageReader<GamepadConnectionEvent>,
    mut keys: MessageReader<KeyboardInput>,
    mut clicks: MessageReader<MouseButtonInput>,
    motion: Res<AccumulatedMouseMotion>,
    mut pushed: Local<std::collections::HashMap<Entity, bool>>,
) {
    for c in connections.read() {
        match &c.connection {
            GamepadConnection::Connected { name, vendor_id, product_id } => {
                info!("gamepad: {name} connected (vendor {vendor_id:04x?}, product {product_id:04x?}): {:?} buttons", PadKind::of(*vendor_id, name));
                if *vendor_id == Some(playstation::SONY) {
                    info!("gamepad: Sony pads over HID: {:?}", playstation::list());
                }
            }
            GamepadConnection::Disconnected => {
                info!("gamepad: disconnected");
                if active.entity == Some(c.gamepad) {
                    *active = ActiveDevice::default();
                }
            }
        }
    }
    let keyboard = keys.read().count() > 0 || clicks.read().count() > 0 || motion.delta.length_squared() > 4.0;
    let mut touched = None;
    for (entity, g, name) in &pads {
        // (The test's virtual pad is no twin, whatever it claims to be.)
        if !virtual_pads.contains(entity) && sony::SonyPads::twin(sony.as_deref(), entity, g.vendor_id()) {
            continue;
        }
        let stick = deadzone(g.left_stick()).length() > 0.3 || deadzone(g.right_stick()).length() > 0.3;
        let was = pushed.insert(entity, stick).unwrap_or(true);
        if g.get_just_pressed().next().is_some() || (stick && !was) {
            touched = Some((entity, g, name));
        }
    }
    let pad = touched;
    if let Some((entity, g, name)) = pad {
        let kind = settings.force_kind.unwrap_or_else(|| PadKind::of(g.vendor_id(), name.map_or("", |n| n.as_str())));
        if active.pad != Some(kind) || active.entity != Some(entity) {
            debug!("gamepad: active ({kind:?})");
        }
        *active = ActiveDevice { pad: Some(kind), entity: Some(entity) };
    } else if keyboard && active.pad.is_some() {
        debug!("gamepad: keyboard and mouse active");
        active.pad = None;
    }
}

/// Where the pad's input goes this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Context {
    /// Playing: no menu up.
    Game,
    /// An in-game menu is up.
    GameMenus,
    /// The main menus.
    Frontend,
    /// Loading, or no frontend at all.
    None,
}

impl Context {
    fn menus(self) -> bool {
        matches!(self, Context::GameMenus | Context::Frontend)
    }
}

fn context(state: Option<&State<GameState>>, fe: Option<&crate::ui::Frontend>) -> Context {
    match state.map(|s| *s.get()) {
        Some(GameState::Frontend) => Context::Frontend,
        Some(GameState::InGame) if crate::ui::menu_open(fe) => Context::GameMenus,
        Some(GameState::InGame) => Context::Game,
        _ => Context::None,
    }
}

/// Pad state the input systems read: sticks, sprint latch, triggers and
/// menu directions. In splitscreen play each player's pad is read on its
/// own ([`read_slot_pads`]); this one is the menus'.
#[allow(clippy::too_many_arguments)]
fn read_pad(
    time: Res<Time>,
    active: Res<ActiveDevice>,
    pads: Query<&Gamepad>,
    state: Option<Res<State<GameState>>>,
    fe: Option<Res<crate::ui::Frontend>>,
    mut frame: ResMut<PadFrame>,
    mut held_dir: Local<Option<(IVec2, f32)>>,
) {
    let ctx = context(state.as_deref(), fe.as_deref());
    let g = active.entity.and_then(|e| pads.get(e).ok()).filter(|_| !(crate::splitscreen::active() && ctx == Context::Game));
    let Some(g) = g else {
        *frame = PadFrame::default();
        *held_dir = None;
        return;
    };
    *frame = pad_frame(g, ctx, &frame, &mut held_dir, time.elapsed_secs(), time.delta_secs());
}

/// One pad's frame (`prev` its last).
fn pad_frame(g: &Gamepad, ctx: Context, prev: &PadFrame, held_dir: &mut Option<(IVec2, f32)>, now: f32, dt: f32) -> PadFrame {
    let trigger = |b: GamepadButton, was: bool| {
        let v = g.get(b).unwrap_or(0.0).max(if g.pressed(b) { 1.0 } else { 0.0 });
        if was { v > TRIGGER_RELEASE } else { v > TRIGGER_PRESS }
    };
    let movement = stick(g.left_stick(), 0);
    let look = look_curve(stick(g.right_stick(), 1));
    let in_game = ctx == Context::Game;
    let fire = in_game && trigger(GamepadButton::RightTrigger2, prev.fire);
    let ads = in_game && trigger(GamepadButton::LeftTrigger2, prev.ads);
    let ads_pressed = ads && !prev.ads;

    // L3 starts a sprint; it lasts while the stick stays forward.
    let mut sprint = prev.sprint && in_game;
    if in_game && g.just_pressed(GamepadButton::LeftThumb) {
        sprint = !sprint && movement.y > 0.3;
    }
    if movement.y < 0.3 || fire || ads {
        sprint = false;
    }

    let pinned = look.length() > 0.95;
    let look_pinned = if pinned && in_game { prev.look_pinned + dt } else { 0.0 };

    // Menu focus: D-pad first, else the left stick past half way.
    let mut dir = IVec2::ZERO;
    let dpad = g.dpad();
    let stick = g.left_stick();
    if dpad != Vec2::ZERO {
        dir = IVec2::new(dpad.x.signum() as i32 * (dpad.x.abs() > 0.5) as i32, -(dpad.y.signum() as i32) * (dpad.y.abs() > 0.5) as i32);
    } else if stick.length() > 0.5 {
        // Screen space: y grows downwards.
        dir = if stick.x.abs() > stick.y.abs() { IVec2::new(stick.x.signum() as i32, 0) } else { IVec2::new(0, -stick.y.signum() as i32) };
    }
    if dir.x != 0 && dir.y != 0 {
        dir.x = 0;
    }
    let menu_dir = if !ctx.menus() || dir == IVec2::ZERO {
        *held_dir = None;
        None
    } else {
        match *held_dir {
            Some((d, next)) if d == dir => {
                if now >= next {
                    *held_dir = Some((dir, now + REPEAT_EVERY));
                    Some(dir)
                } else {
                    None
                }
            }
            _ => {
                *held_dir = Some((dir, now + REPEAT_DELAY));
                Some(dir)
            }
        }
    };
    let menu_rotate = if ctx.menus() { deadzone(g.right_stick()) } else { Vec2::ZERO };
    PadFrame {
        movement: if in_game { movement } else { Vec2::ZERO },
        look: if in_game { look } else { Vec2::ZERO },
        look_pinned,
        sprint,
        ads,
        ads_pressed,
        fire,
        menu_dir,
        menu_rotate,
        menu_done: ctx.menus() && (g.just_pressed(GamepadButton::South) || g.just_pressed(GamepadButton::East)),
        interact: in_game && g.pressed(GamepadButton::West),
        breath: in_game && g.pressed(GamepadButton::LeftThumb),
        wake: g.get_just_pressed().any(|&b| b != GamepadButton::Start) || movement != Vec2::ZERO || look != Vec2::ZERO,
    }
}

/// A key or mouse button standing in for a pad button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Bound {
    Key(KeyCode),
    Mouse(MouseButton),
}

/// The bindings held down for pad buttons, and one-frame taps to let go.
#[derive(Resource, Default, Debug)]
struct Bindings {
    held: Vec<(GamepadButton, Bound)>,
    taps: Vec<Bound>,
    /// When B went down in the game (crouch on release, prone once held).
    b_down: Option<f32>,
}

/// The game's binding for a pad button pressed in `ctx`. `lean`: aiming in
/// Bodycam gunplay (the stick clicks lean); `view`: View/Share held (R3
/// then switches the view).
fn binding(ctx: Context, b: GamepadButton, lean: bool, view: bool) -> Option<Bound> {
    use Bound::*;
    use GamepadButton as P;
    Some(match (ctx, b) {
        (Context::GameMenus | Context::Frontend, P::South) => Key(KeyCode::Enter),
        (Context::GameMenus | Context::Frontend, P::East) => Key(KeyCode::Escape),
        (Context::Frontend, P::Start) => Key(KeyCode::Enter),
        (Context::Game, P::South) => Key(KeyCode::Space),
        (Context::Game, P::West) => Key(KeyCode::KeyR),
        (Context::Game, P::Select) => Key(KeyCode::Tab),
        // Grenades (`crate::grenades`): RB frag, LB special.
        (Context::Game, P::RightTrigger) => Key(KeyCode::KeyG),
        (Context::Game, P::LeftTrigger) => Key(KeyCode::Digit4),
        // The kill streak picker (`crate::killstreaks`): right opens it and
        // calls in the one picked, up and down choose, left closes it.
        (Context::Game, P::DPadRight) => Key(crate::killstreaks::PAD_PICK),
        (Context::Game, P::DPadLeft) if crate::killstreaks::picker_open() => Key(crate::killstreaks::PAD_CLOSE),
        (Context::Game, P::DPadUp) if crate::killstreaks::picker_open() => Key(crate::killstreaks::PAD_UP),
        (Context::Game, P::DPadDown) if crate::killstreaks::picker_open() => Key(crate::killstreaks::PAD_DOWN),
        // Left: equipment or the grenade launcher (PC 3).
        (Context::Game, P::DPadLeft) => Key(KeyCode::Digit3),
        // Lean (Bodycam gunplay) while aiming, as Rainbow Six does: L3 left,
        // R3 right. Otherwise L3 sprints (`read_pad`) and R3 knifes
        // (`crate::melee`, CoD4's console layout), or with View held
        // switches first and third person.
        (Context::Game, P::LeftThumb) if lean => Key(KeyCode::KeyQ),
        (Context::Game, P::RightThumb) if lean => Key(KeyCode::KeyE),
        (Context::Game, P::RightThumb) if view => Key(KeyCode::F5),
        (Context::Game, P::RightThumb) if tactical() => return None,
        (Context::Game, P::RightThumb) => Key(KeyCode::KeyV),
        (Context::Game, P::East) if tactical() => Key(KeyCode::KeyV),
        // Night vision (CoD4's action slot 1); D-pad down inspects the gun,
        // or with View held switches the gunplay.
        (Context::Game, P::DPadUp) => Key(KeyCode::KeyN),
        (Context::Game, P::DPadDown) if view => Key(KeyCode::KeyB),
        (Context::Game, P::DPadDown) => Key(KeyCode::KeyI),
        // Menu opens the in-game menu and closes it again.
        (Context::Game | Context::GameMenus, P::Start) => Key(KeyCode::Escape),
        _ => return None,
    })
}

/// Press and release the game's bindings for the pad's buttons. In
/// splitscreen play the players' pads press their own ([`read_slot_pads`]);
/// this one is for the menus then.
#[allow(clippy::too_many_arguments)]
fn feed_bindings(
    time: Res<Time>,
    active: Res<ActiveDevice>,
    frame: Res<PadFrame>,
    pads: Query<&Gamepad>,
    state: Option<Res<State<GameState>>>,
    fe: Option<Res<crate::ui::Frontend>>,
    mut bindings: ResMut<Bindings>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    mut scroll: ResMut<AccumulatedMouseScroll>,
    gunplay: Res<crate::bodycam::Gunplay>,
    mut injected: ResMut<PadInjected>,
) {
    let ctx = context(state.as_deref(), fe.as_deref());
    // Typing into a text field: A and B only end it (`ui::pad`).
    let typing = fe.as_deref().is_some_and(crate::ui::Frontend::typing);
    let g = active.entity.and_then(|e| pads.get(e).ok()).filter(|_| !(crate::splitscreen::active() && ctx == Context::Game));
    let lean = gunplay.is_bodycam();
    feed(g, ctx, &frame, &mut bindings, (&mut keys, &mut mouse, &mut scroll.delta.y), lean, typing, time.elapsed_secs());
    // What it pressed: the game's own keys, which rebinding leaves alone.
    injected.keys.clear();
    injected.mouse.clear();
    for bound in bindings.held.iter().map(|h| h.1).chain(bindings.taps.iter().copied()) {
        match bound {
            Bound::Key(k) => injected.keys.push(k),
            Bound::Mouse(m) => injected.mouse.push(m),
        }
    }
}

/// The keys and buttons the pad has down this frame (in the keyboard's and
/// mouse's resources): the game's own keys, passed through rebinding as
/// they are ([`crate::bindings`]).
#[derive(Resource, Default)]
pub struct PadInjected {
    pub keys: Vec<KeyCode>,
    pub mouse: Vec<MouseButton>,
}

/// Feed one pad's buttons (none: let go of everything) into `keys`,
/// `mouse` and the wheel.
#[allow(clippy::too_many_arguments)]
fn feed(
    g: Option<&Gamepad>,
    ctx: Context,
    frame: &PadFrame,
    b: &mut Bindings,
    (keys, mouse, wheel): (&mut ButtonInput<KeyCode>, &mut ButtonInput<MouseButton>, &mut f32),
    bodycam: bool,
    typing: bool,
    now: f32,
) {
    let release = |bound: Bound, keys: &mut ButtonInput<KeyCode>, mouse: &mut ButtonInput<MouseButton>| match bound {
        Bound::Key(k) => keys.release(k),
        Bound::Mouse(m) => mouse.release(m),
    };
    for t in b.taps.drain(..) {
        release(t, keys, mouse);
    }
    let Some(g) = g else {
        for (_, bound) in b.held.drain(..) {
            release(bound, keys, mouse);
        }
        b.b_down = None;
        return;
    };
    let press = |bound: Bound, keys: &mut ButtonInput<KeyCode>, mouse: &mut ButtonInput<MouseButton>| match bound {
        Bound::Key(k) => keys.press(k),
        Bound::Mouse(m) => mouse.press(m),
    };
    // Let go of bindings whose button is up.
    b.held.retain(|&(button, bound)| {
        let down = match button {
            GamepadButton::RightTrigger2 => frame.fire,
            GamepadButton::LeftTrigger2 => frame.ads,
            _ => g.pressed(button),
        };
        if !down {
            release(bound, keys, mouse);
        }
        down
    });
    for &button in g.get_just_pressed() {
        if typing && matches!(button, GamepadButton::South | GamepadButton::East) {
            continue;
        }
        let lean = frame.ads && bodycam;
        if let Some(bound) = binding(ctx, button, lean, g.pressed(GamepadButton::Select)) {
            press(bound, keys, mouse);
            b.held.push((button, bound));
        }
    }
    if ctx != Context::Game {
        b.b_down = None;
        return;
    }
    // Triggers: analog, so they're pressed past a threshold.
    for (button, on, bound) in [
        (GamepadButton::RightTrigger2, frame.fire, Bound::Mouse(MouseButton::Left)),
        (GamepadButton::LeftTrigger2, frame.ads, Bound::Mouse(MouseButton::Right)),
    ] {
        if on && !b.held.iter().any(|h| h.0 == button) {
            press(bound, keys, mouse);
            b.held.push((button, bound));
        }
    }
    // Y: the next weapon, like the mouse wheel.
    if g.just_pressed(GamepadButton::North) {
        *wheel += 1.0;
    }
    // B: crouch on a tap, prone once held (R3 in the Tactical layout).
    let crouch = if tactical() { GamepadButton::RightThumb } else { GamepadButton::East };
    if g.just_pressed(crouch) {
        b.b_down = Some(now);
    }
    if let Some(down) = b.b_down {
        let tap = if !g.pressed(crouch) {
            // 3rd Person TDM: a tap takes (or leaves) cover when it can
            // ([`crate::cover`]), else crouches.
            Some(if crate::cover::button_is_cover() { crate::cover::COVER_KEY } else { KeyCode::KeyC })
        } else if now - down >= PRONE_HOLD {
            Some(KeyCode::ControlLeft)
        } else {
            None
        };
        if let Some(k) = tap {
            keys.press(k);
            b.taps.push(Bound::Key(k));
            b.b_down = None;
        }
    }
}

/// Each splitscreen player's controller, read on its own: its frame and
/// the bindings it presses (their [`crate::splitscreen::PlayerInput`]).
#[derive(Resource, Default)]
pub struct SlotPads(pub std::collections::HashMap<Entity, SlotPad>);

#[derive(Default)]
pub struct SlotPad {
    pub frame: PadFrame,
    pub keys: ButtonInput<KeyCode>,
    pub mouse: ButtonInput<MouseButton>,
    pub scroll: f32,
    pub kind: PadKind,
    bindings: Bindings,
    held_dir: Option<(IVec2, f32)>,
}

/// Read each splitscreen player's controller while they play (in the menus
/// they let go of everything; the menus read the pads as before).
#[allow(clippy::too_many_arguments)]
fn read_slot_pads(
    time: Res<Time>,
    settings: Res<PadSettings>,
    pads: Query<(&Gamepad, Option<&Name>)>,
    state: Option<Res<State<GameState>>>,
    fe: Option<Res<crate::ui::Frontend>>,
    players: Option<Res<crate::splitscreen::LocalPlayers>>,
    mut slots: ResMut<SlotPads>,
) {
    if !crate::splitscreen::active() {
        slots.0.clear();
        return;
    }
    let ctx = context(state.as_deref(), fe.as_deref());
    let (now, dt) = (time.elapsed_secs(), time.delta_secs());
    let assigned: Vec<(usize, Entity)> = players
        .iter()
        .flat_map(|p| p.devices.iter().enumerate())
        .filter_map(|(slot, d)| match d {
            crate::splitscreen::Device::Pad(e) => Some((slot, *e)),
            _ => None,
        })
        .collect();
    slots.0.retain(|e, _| assigned.iter().any(|a| a.1 == *e));
    for (slot, e) in assigned {
        let s = slots.0.entry(e).or_default();
        s.keys.clear();
        s.mouse.clear();
        s.scroll = 0.0;
        // Their own menu up: the pad works it (A Enter, B Escape, focus).
        let ctx = if ctx == Context::Game && fe.as_deref().is_some_and(|fe| fe.slot_menu_open(slot)) { Context::GameMenus } else { ctx };
        let pad = pads.get(e).ok().filter(|_| matches!(ctx, Context::Game | Context::GameMenus));
        if let Some((g, name)) = pad {
            s.kind = settings.force_kind.unwrap_or_else(|| PadKind::of(g.vendor_id(), name.map_or("", |n| n.as_str())));
            s.frame = pad_frame(g, ctx, &s.frame, &mut s.held_dir, now, dt);
        } else {
            s.frame = PadFrame::default();
            s.held_dir = None;
        }
        let SlotPad { frame, keys, mouse, scroll, bindings, .. } = s;
        feed(pad.map(|p| p.0), ctx, frame, bindings, (keys, mouse, scroll), false, false, now);
    }
}

/// The devices a player can pick, with their names for the lobby: the
/// keyboard and mouse, then each controller (once: a Sony pad read over
/// HID, not its Windows twin too).
#[derive(Resource, Default, Clone, Debug)]
pub struct Devices(pub Vec<(crate::splitscreen::Device, String)>);

fn list_devices(
    pads: Query<(Entity, &Gamepad, Option<&Name>, Has<test::VirtualPad>)>,
    sony: Option<Res<sony::SonyPads>>,
    mut devices: ResMut<Devices>,
) {
    let mut list = vec![(crate::splitscreen::Device::KeyboardMouse, "Keyboard + Mouse".to_string())];
    let mut found: Vec<(Entity, String, bool)> = pads
        .iter()
        .filter(|(e, g, _, test)| *test || !sony::SonyPads::twin(sony.as_deref(), *e, g.vendor_id()))
        // Keyboard and mouse chip makers (Holtek, Sino Wealth, Corsair):
        // their devices sometimes also show up as a generic "HID-compliant
        // game controller", which isn't one.
        .filter(|(_, g, _, test)| *test || !matches!(g.vendor_id(), Some(0x04D9 | 0x258A | 0x1B1C)))
        .map(|(e, g, n, _)| {
            // Known makers' pads first: some keyboards and other gear show
            // up as a generic "HID-compliant game controller".
            let known = matches!(g.vendor_id(), Some(playstation::SONY | 0x045E | 0x057E));
            (e, pad_name(g.vendor_id(), g.product_id(), n.map_or("", |n| n.as_str())), known)
        })
        .collect();
    found.sort_by_key(|(e, _, known)| (!known, e.index()));
    for i in 0..found.len() {
        let same = found.iter().filter(|(_, n, _)| *n == found[i].1).count();
        let nth = found[..i].iter().filter(|(_, n, _)| *n == found[i].1).count();
        let name = if same > 1 { format!("{} {}", found[i].1, nth + 1) } else { found[i].1.clone() };
        list.push((crate::splitscreen::Device::Pad(found[i].0), name));
    }
    if devices.0 != list {
        devices.0 = list;
    }
}


/// A controller's name, from its USB ids or else what Windows calls it.
pub fn pad_name(vendor: Option<u16>, product: Option<u16>, name: &str) -> String {
    let known = match (vendor, product) {
        (Some(playstation::SONY), Some(0x0DF2)) => Some("DualSense Edge"),
        (Some(playstation::SONY), Some(0x0CE6)) => Some("DualSense"),
        (Some(playstation::SONY), Some(0x05C4 | 0x09CC | 0x0BA0)) => Some("DualShock 4"),
        (Some(playstation::SONY), _) => Some("PlayStation Controller"),
        (Some(0x045E), _) => Some("Xbox Controller"),
        (Some(0x057E), _) => Some("Switch Pro Controller"),
        _ => None,
    };
    if let Some(n) = known {
        return n.into();
    }
    let lower = name.to_ascii_lowercase();
    if lower.contains("dualsense edge") {
        "DualSense Edge".into()
    } else if PadKind::of(vendor, name) == PadKind::PlayStation {
        "PlayStation Controller".into()
    } else if lower.contains("xbox") || lower.contains("xinput") {
        "Xbox Controller".into()
    } else if name.trim().is_empty() {
        "Controller".into()
    } else {
        name.trim().into()
    }
}

/// A pad button in the game takes the mouse, like a click does.
fn grab_on_pad(
    active: Res<ActiveDevice>,
    frame: Res<PadFrame>,
    mut cursor: Single<&mut CursorOptions, With<PrimaryWindow>>,
) {
    if active.pad.is_some() && frame.wake && cursor.grab_mode == CursorGrabMode::None {
        cursor.grab_mode = CursorGrabMode::Locked;
        cursor.visible = false;
    }
}

/// A light buzz for a player's shots, a heavier one when hit, on their own
/// controller. Sony pads get it over HID ([`sony`]), the rest through gilrs.
#[allow(clippy::too_many_arguments)]
fn rumble(
    settings: Res<PadSettings>,
    players: Query<(Entity, &crate::splitscreen::PlayerInput)>,
    virtual_pads: Query<(), With<test::VirtualPad>>,
    hid_pads: Query<(), With<sony::HidPad>>,
    hid: Option<Res<sony::SonyLink>>,
    mut shots: MessageReader<crate::weapons::ShotFired>,
    mut damage: MessageReader<crate::combat::Damage>,
    mut out: MessageWriter<GamepadRumbleRequest>,
) {
    let shots: Vec<Entity> = shots.read().map(|s| s.shooter).collect();
    let damage: Vec<(Entity, f32)> = damage.read().map(|d| (d.target, d.amount)).collect();
    if !settings.rumble {
        return;
    }
    for (me, input) in &players {
        let Some(gamepad) = input.pad_entity.filter(|&e| !virtual_pads.contains(e)) else { continue };
        let mut buzz = |duration: Duration, strong_motor: f32, weak_motor: f32| match hid.as_ref().filter(|_| hid_pads.contains(gamepad)) {
            Some(h) => h.0.rumble(playstation::Rumble { strong: strong_motor, weak: weak_motor, duration }),
            None => {
                out.write(GamepadRumbleRequest::Add { duration, intensity: GamepadRumbleIntensity { strong_motor, weak_motor }, gamepad });
            }
        };
        if shots.contains(&me) {
            buzz(Duration::from_millis(70), 0.15, 0.45);
        }
        let hurt: f32 = damage.iter().filter(|d| d.0 == me).map(|d| d.1).sum();
        if hurt > 0.0 {
            let strength = (0.3 + hurt / 100.0).min(1.0);
            buzz(Duration::from_millis(180), strength, strength * 0.5);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sony_pads_get_playstation_glyphs() {
        assert_eq!(PadKind::of(Some(0x054C), "Anything"), PadKind::PlayStation);
        assert_eq!(PadKind::of(None, "DualSense Wireless Controller"), PadKind::PlayStation);
        assert_eq!(PadKind::of(None, "Wireless Controller"), PadKind::PlayStation);
        assert_eq!(PadKind::of(Some(0x045E), "Xbox Controller"), PadKind::Xbox);
        assert_eq!(PadKind::of(None, "Generic X-Box pad"), PadKind::Xbox);
    }

    #[test]
    fn controllers_are_named() {
        assert_eq!(pad_name(Some(0x054C), Some(0x0DF2), "Wireless Controller"), "DualSense Edge");
        assert_eq!(pad_name(Some(0x054C), Some(0x0CE6), ""), "DualSense");
        assert_eq!(pad_name(Some(0x045E), Some(0x0B12), "Controller (Xbox One For Windows)"), "Xbox Controller");
        assert_eq!(pad_name(None, None, "Xbox 360 Controller (XInput)"), "Xbox Controller");
        assert_eq!(pad_name(None, None, "  "), "Controller");
    }

    #[test]
    fn stick_deadzone_and_curve() {
        assert_eq!(deadzone(Vec2::new(0.1, 0.1)), Vec2::ZERO);
        assert!((deadzone(Vec2::X).length() - 1.0).abs() < 1e-5);
        let half = deadzone(Vec2::new(0.58, 0.0)).x;
        assert!((half - 0.5).abs() < 1e-3);
        // The curve keeps direction and full deflection.
        assert!((look_curve(Vec2::Y) - Vec2::Y).length() < 1e-5);
        assert!(look_curve(Vec2::new(0.5, 0.0)).x < 0.5);
    }

    #[test]
    fn bindings_follow_the_console_layout() {
        let b = |ctx, button| binding(ctx, button, false, false);
        assert_eq!(b(Context::Game, GamepadButton::South), Some(Bound::Key(KeyCode::Space)));
        assert_eq!(b(Context::Frontend, GamepadButton::South), Some(Bound::Key(KeyCode::Enter)));
        assert_eq!(b(Context::GameMenus, GamepadButton::East), Some(Bound::Key(KeyCode::Escape)));
        assert_eq!(b(Context::Game, GamepadButton::Start), Some(Bound::Key(KeyCode::Escape)));
        assert_eq!(b(Context::Game, GamepadButton::RightTrigger), Some(Bound::Key(KeyCode::KeyG)));
        assert_eq!(b(Context::Game, GamepadButton::DPadRight), Some(Bound::Key(crate::killstreaks::PAD_PICK)));
        assert_eq!(b(Context::Game, GamepadButton::DPadUp), Some(Bound::Key(KeyCode::KeyN)));
        assert_eq!(b(Context::Game, GamepadButton::DPadDown), Some(Bound::Key(KeyCode::KeyI)));
        assert_eq!(binding(Context::Game, GamepadButton::DPadDown, false, true), Some(Bound::Key(KeyCode::KeyB)));
        assert_eq!(b(Context::None, GamepadButton::South), None);
        // L3 sprints and R3 knifes, unless leaning (aiming in Bodycam);
        // with View held R3 switches the view.
        assert_eq!(b(Context::Game, GamepadButton::LeftThumb), None);
        assert_eq!(b(Context::Game, GamepadButton::RightThumb), Some(Bound::Key(KeyCode::KeyV)));
        assert_eq!(binding(Context::Game, GamepadButton::LeftThumb, true, false), Some(Bound::Key(KeyCode::KeyQ)));
        assert_eq!(binding(Context::Game, GamepadButton::RightThumb, true, false), Some(Bound::Key(KeyCode::KeyE)));
        assert_eq!(binding(Context::Game, GamepadButton::RightThumb, false, true), Some(Bound::Key(KeyCode::F5)));
    }
}

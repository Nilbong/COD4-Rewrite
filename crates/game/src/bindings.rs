//! Key bindings: the actions a player binds keys and mouse buttons to, and
//! the translation from what they press to the keys the game reads.
//!
//! The gameplay code reads each action's default key (W to move forward, G
//! for a frag, the left mouse button to fire...), as CoD4's defaults have
//! them. Rebinding doesn't touch it: each frame [`translate`] turns the keys
//! and buttons the player pressed into the default keys of the actions they
//! are bound to (so with Frag on H, pressing H presses G for the game, and G
//! itself does nothing). Keys bound to nothing pass through (Escape, Enter,
//! typing). A controller presses default keys directly; those pass through
//! as they are ([`crate::gamepad`]).
//!
//! Each action has two slots, stored as the dvar `bind_<action>`: the names
//! of what's bound, space separated (`KeyW ArrowUp`). The hold or toggle
//! modes of aiming, crouching, sprinting and leaning live here too.

use bevy::input::ButtonInput;
use bevy::prelude::*;
use std::collections::HashMap;

/// Something a key can be: a keyboard key or a mouse button.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Input {
    Key(KeyCode),
    Mouse(MouseButton),
}

impl Input {
    /// Its name in the bind dvars: Bevy's key names, `Mouse1`..`Mouse5`.
    pub fn name(self) -> String {
        match self {
            Input::Key(k) => format!("{k:?}"),
            Input::Mouse(MouseButton::Left) => "Mouse1".into(),
            Input::Mouse(MouseButton::Right) => "Mouse2".into(),
            Input::Mouse(MouseButton::Middle) => "Mouse3".into(),
            Input::Mouse(MouseButton::Back) => "Mouse4".into(),
            Input::Mouse(MouseButton::Forward) => "Mouse5".into(),
            Input::Mouse(MouseButton::Other(n)) => format!("Mouse{}", n as u32 + 6),
        }
    }

    pub fn parse(name: &str) -> Option<Input> {
        let mouse = match name {
            "Mouse1" => Some(MouseButton::Left),
            "Mouse2" => Some(MouseButton::Right),
            "Mouse3" => Some(MouseButton::Middle),
            "Mouse4" => Some(MouseButton::Back),
            "Mouse5" => Some(MouseButton::Forward),
            _ => None,
        };
        if let Some(m) = mouse {
            return Some(Input::Mouse(m));
        }
        KEYS.iter().find(|k| format!("{k:?}") == name).map(|&k| Input::Key(k))
    }

    /// How the menus show it: CoD4's style, upper case (`W`, `SPACE`,
    /// `MOUSE1`, `LSHIFT`).
    pub fn display(self) -> String {
        match self {
            Input::Mouse(_) => self.name().to_ascii_uppercase(),
            Input::Key(k) => {
                let n = format!("{k:?}");
                let n = n.strip_prefix("Key").or_else(|| n.strip_prefix("Digit")).unwrap_or(&n).to_owned();
                match n.as_str() {
                    "ShiftLeft" => "LSHIFT".into(),
                    "ShiftRight" => "RSHIFT".into(),
                    "ControlLeft" => "LCTRL".into(),
                    "ControlRight" => "RCTRL".into(),
                    "AltLeft" => "LALT".into(),
                    "AltRight" => "RALT".into(),
                    "ArrowUp" => "UPARROW".into(),
                    "ArrowDown" => "DOWNARROW".into(),
                    "ArrowLeft" => "LEFTARROW".into(),
                    "ArrowRight" => "RIGHTARROW".into(),
                    "Backquote" => "~".into(),
                    _ => n.to_ascii_uppercase(),
                }
            }
        }
    }
}

/// The keys that can be bound (and named in the dvars).
pub const KEYS: [KeyCode; 82] = {
    use KeyCode::*;
    [
        KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM, KeyN, KeyO, KeyP, KeyQ, KeyR, KeyS, KeyT, KeyU, KeyV, KeyW,
        KeyX, KeyY, KeyZ, Digit0, Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9, Space, Tab, Enter, Backspace, CapsLock,
        ShiftLeft, ShiftRight, ControlLeft, ControlRight, AltLeft, AltRight, ArrowUp, ArrowDown, ArrowLeft, ArrowRight, Insert, Delete, Home,
        End, PageUp, PageDown, F1, F2, F3, F4, F5, F6, F7, F8, F9, F11, F12, Minus, Equal, BracketLeft, BracketRight, Backslash, Semicolon, Quote,
        Comma, Period, Slash, Backquote, Numpad0, Numpad1, Numpad2,
    ]
};

/// What a player binds keys to. The default key of each is what the game
/// reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Forward,
    Back,
    Left,
    Right,
    Jump,
    Crouch,
    Prone,
    Sprint,
    LeanLeft,
    LeanRight,
    Fire,
    Aim,
    Reload,
    Melee,
    Weapon1,
    Weapon2,
    Frag,
    Special,
    Equipment,
    Killstreak,
    Inspect,
    Use,
    NightVision,
    Scores,
    ThirdPerson,
}

impl Action {
    pub const ALL: [Action; 25] = [
        Action::Forward,
        Action::Back,
        Action::Left,
        Action::Right,
        Action::Jump,
        Action::Crouch,
        Action::Prone,
        Action::Sprint,
        Action::LeanLeft,
        Action::LeanRight,
        Action::Fire,
        Action::Aim,
        Action::Reload,
        Action::Melee,
        Action::Weapon1,
        Action::Weapon2,
        Action::Frag,
        Action::Special,
        Action::Equipment,
        Action::Killstreak,
        Action::Inspect,
        Action::Use,
        Action::NightVision,
        Action::Scores,
        Action::ThirdPerson,
    ];

    /// The key the game reads for it (CoD4's default binding).
    pub fn default_input(self) -> Input {
        use KeyCode::*;
        Input::Key(match self {
            Action::Forward => KeyW,
            Action::Back => KeyS,
            Action::Left => KeyA,
            Action::Right => KeyD,
            Action::Jump => Space,
            Action::Crouch => KeyC,
            Action::Prone => ControlLeft,
            Action::Sprint => ShiftLeft,
            Action::LeanLeft => KeyQ,
            Action::LeanRight => KeyE,
            Action::Fire => return Input::Mouse(MouseButton::Left),
            Action::Aim => return Input::Mouse(MouseButton::Right),
            Action::Reload => KeyR,
            Action::Melee => KeyV,
            Action::Weapon1 => Digit1,
            Action::Weapon2 => Digit2,
            Action::Frag => KeyG,
            Action::Special => Digit4,
            Action::Equipment => Digit5,
            Action::Killstreak => Digit6,
            Action::Inspect => KeyI,
            Action::Use => KeyF,
            Action::NightVision => KeyN,
            Action::Scores => Tab,
            Action::ThirdPerson => F5,
        })
    }

    /// Its bind dvar, `bind_forward`.
    pub fn dvar(self) -> &'static str {
        match self {
            Action::Forward => "bind_forward",
            Action::Back => "bind_back",
            Action::Left => "bind_moveleft",
            Action::Right => "bind_moveright",
            Action::Jump => "bind_jump",
            Action::Crouch => "bind_crouch",
            Action::Prone => "bind_prone",
            Action::Sprint => "bind_sprint",
            Action::LeanLeft => "bind_leanleft",
            Action::LeanRight => "bind_leanright",
            Action::Fire => "bind_attack",
            Action::Aim => "bind_ads",
            Action::Reload => "bind_reload",
            Action::Melee => "bind_melee",
            Action::Weapon1 => "bind_weapon1",
            Action::Weapon2 => "bind_weapon2",
            Action::Frag => "bind_frag",
            Action::Special => "bind_smoke",
            Action::Equipment => "bind_actionslot3",
            Action::Killstreak => "bind_actionslot4",
            Action::Inspect => "bind_inspect",
            Action::Use => "bind_activate",
            Action::NightVision => "bind_nightvision",
            Action::Scores => "bind_scores",
            Action::ThirdPerson => "bind_thirdperson",
        }
    }

    /// Its default dvar value.
    pub fn default_value(self) -> String {
        self.default_input().name()
    }
}

/// The two slots of a bind dvar's value.
pub fn parse_value(value: &str) -> Vec<Input> {
    value.split_whitespace().filter_map(Input::parse).take(2).collect()
}

/// How a bind row shows: `W`, `W or UPARROW`, or nothing.
pub fn display_value(value: &str, unbound: &str, or: &str) -> String {
    let inputs = parse_value(value);
    if inputs.is_empty() {
        return unbound.to_owned();
    }
    inputs.iter().map(|i| i.display()).collect::<Vec<_>>().join(&format!(" {or} "))
}

/// Binding `input` to a row: it takes the first free slot (or replaces the
/// second), and comes off every other action (`Key_SetBinding`'s
/// one-command-per-key).
pub fn bind(values: &mut HashMap<&'static str, String>, action: Action, input: Input) {
    for other in Action::ALL {
        let v = values.entry(other.dvar()).or_insert_with(|| other.default_value());
        let kept: Vec<String> = parse_value(v).into_iter().filter(|i| *i != input).map(|i| i.name()).collect();
        *v = kept.join(" ");
    }
    let v = values.entry(action.dvar()).or_default();
    let mut slots = parse_value(v);
    if slots.len() >= 2 {
        slots.pop();
    }
    slots.push(input);
    *v = slots.iter().map(|i| i.name()).collect::<Vec<_>>().join(" ");
}

/// Hold or toggle, for the actions that have both.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Hold,
    Toggle,
}

/// The bindings in force, from the settings.
#[derive(Resource, Clone, Debug)]
pub struct Bindings {
    /// For each action, what's bound to it.
    pub actions: Vec<(Action, Vec<Input>)>,
    /// Aim down the sights and lean: hold (CoD4's) or toggle.
    pub aim: Mode,
    pub lean: Mode,
    /// Crouch and prone: CoD4's keys toggle; held, they're down while held.
    pub crouch: Mode,
    /// Sprint: held, or a press latches it until the player stops.
    pub sprint: Mode,
}

impl Default for Bindings {
    fn default() -> Self {
        Bindings {
            actions: Action::ALL.into_iter().map(|a| (a, vec![a.default_input()])).collect(),
            aim: Mode::Hold,
            lean: Mode::Hold,
            crouch: Mode::Toggle,
            sprint: Mode::Hold,
        }
    }
}

impl Bindings {
    /// From the settings' dvars (`value` gives each, or its default).
    pub fn from_values(value: impl Fn(&str) -> Option<String>) -> Bindings {
        let mode = |dvar: &str, default: Mode| match value(dvar).as_deref() {
            Some("toggle") => Mode::Toggle,
            Some("hold") => Mode::Hold,
            _ => default,
        };
        Bindings {
            actions: Action::ALL.into_iter().map(|a| (a, parse_value(&value(a.dvar()).unwrap_or_else(|| a.default_value())))).collect(),
            aim: mode("cg_ads_mode", Mode::Hold),
            lean: mode("cg_lean_mode", Mode::Hold),
            crouch: mode("cg_crouch_mode", Mode::Toggle),
            sprint: mode("cg_sprint_mode", Mode::Hold),
        }
    }

    /// What `action` is bound to.
    pub fn of(&self, action: Action) -> &[Input] {
        self.actions.iter().find(|(a, _)| *a == action).map_or(&[], |(_, i)| i)
    }

    /// The name of the first key bound to `action` (`F`), for hints.
    pub fn key_name(&self, action: Action) -> String {
        self.of(action).first().map_or_else(|| "UNBOUND".into(), |i| i.display())
    }
}

/// A player's translation state: what the game sees, and the toggles.
#[derive(Default, Clone, Debug)]
pub struct Translated {
    pub keys: ButtonInput<KeyCode>,
    pub mouse: ButtonInput<MouseButton>,
    aim_on: bool,
    lean_on: [bool; 2],
    sprint_on: bool,
}

/// Turn what a player pressed into what the game reads: each bound action's
/// default key down while (or, toggled, from one press to the next) any of
/// its inputs is; unbound keys as they are; `injected` (a controller's
/// presses, already the game's keys) as they are.
pub fn translate(
    keys: &ButtonInput<KeyCode>,
    mouse: &ButtonInput<MouseButton>,
    injected: (&[KeyCode], &[MouseButton]),
    b: &Bindings,
    moving_forward: bool,
    t: &mut Translated,
) {
    let pressed = |i: Input| match i {
        Input::Key(k) => keys.pressed(k) && !injected.0.contains(&k),
        Input::Mouse(m) => mouse.pressed(m) && !injected.1.contains(&m),
    };
    let just = |i: Input| match i {
        Input::Key(k) => keys.just_pressed(k) && !injected.0.contains(&k),
        Input::Mouse(m) => mouse.just_pressed(m) && !injected.1.contains(&m),
    };
    let mut want_keys: HashMap<KeyCode, bool> = HashMap::new();
    let mut want_mouse: HashMap<MouseButton, bool> = HashMap::new();
    let mut set = |input: Input, on: bool| match input {
        Input::Key(k) => *want_keys.entry(k).or_default() |= on,
        Input::Mouse(m) => *want_mouse.entry(m).or_default() |= on,
    };
    // Unbound keys are themselves (default keys whose action moved
    // elsewhere are not).
    let bound: Vec<Input> = b.actions.iter().flat_map(|(_, i)| i.iter().copied()).collect();
    let defaults: Vec<Input> = Action::ALL.iter().map(|a| a.default_input()).collect();
    for k in keys.get_pressed() {
        let i = Input::Key(*k);
        if !bound.contains(&i) && !defaults.contains(&i) {
            set(i, true);
        }
    }
    for m in mouse.get_pressed() {
        let i = Input::Mouse(*m);
        if !bound.contains(&i) && !defaults.contains(&i) {
            set(i, true);
        }
    }
    for (action, inputs) in &b.actions {
        let held = inputs.iter().any(|&i| pressed(i));
        let tapped = inputs.iter().any(|&i| just(i));
        let on = match (action, b.aim, b.lean, b.sprint) {
            (Action::Aim, Mode::Toggle, _, _) => {
                if tapped {
                    t.aim_on = !t.aim_on;
                }
                t.aim_on
            }
            (Action::LeanLeft | Action::LeanRight, _, Mode::Toggle, _) => {
                let side = (*action == Action::LeanRight) as usize;
                if tapped {
                    t.lean_on[side] = !t.lean_on[side];
                    if t.lean_on[side] {
                        t.lean_on[1 - side] = false;
                    }
                }
                t.lean_on[side]
            }
            (Action::Sprint, _, _, Mode::Toggle) => {
                if tapped {
                    t.sprint_on = !t.sprint_on;
                }
                // It ends when the player stops going forward.
                if !moving_forward {
                    t.sprint_on = false;
                }
                t.sprint_on || held
            }
            _ => held,
        };
        set(action.default_input(), on);
    }
    // A controller's presses are the game's keys already.
    for &k in injected.0 {
        if keys.pressed(k) {
            set(Input::Key(k), true);
        }
    }
    for &m in injected.1 {
        if mouse.pressed(m) {
            set(Input::Mouse(m), true);
        }
    }
    // Held crouch and prone: the game's keys toggle, so a press and a
    // release each toggle (back up when let go).
    if b.crouch == Mode::Hold {
        for action in [Action::Crouch, Action::Prone] {
            let inputs = b.of(action);
            let released = inputs.iter().any(|&i| match i {
                Input::Key(k) => keys.just_released(k),
                Input::Mouse(m) => mouse.just_released(m),
            });
            if released {
                // A fresh press of the default key this frame.
                if let Input::Key(k) = action.default_input() {
                    want_keys.insert(k, true);
                    t.keys.release(k);
                }
            }
        }
    }
    t.keys.clear();
    t.mouse.clear();
    let now_keys: Vec<KeyCode> = t.keys.get_pressed().copied().collect();
    for k in now_keys {
        if !want_keys.get(&k).copied().unwrap_or(false) {
            t.keys.release(k);
        }
    }
    for (k, on) in want_keys {
        if on {
            t.keys.press(k);
        }
    }
    let now_mouse: Vec<MouseButton> = t.mouse.get_pressed().copied().collect();
    for m in now_mouse {
        if !want_mouse.get(&m).copied().unwrap_or(false) {
            t.mouse.release(m);
        }
    }
    for (m, on) in want_mouse {
        if on {
            t.mouse.press(m);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebinding_presses_the_default_key() {
        let mut values = HashMap::new();
        bind(&mut values, Action::Frag, Input::Key(KeyCode::KeyH));
        let b = Bindings::from_values(|d| values.get(d).cloned());
        let mut keys = ButtonInput::<KeyCode>::default();
        let mouse = ButtonInput::<MouseButton>::default();
        keys.press(KeyCode::KeyH);
        let mut t = Translated::default();
        translate(&keys, &mouse, (&[], &[]), &b, false, &mut t);
        assert!(t.keys.pressed(KeyCode::KeyG) && t.keys.just_pressed(KeyCode::KeyG));
        // G stays the frag's second key.
        let mut keys = ButtonInput::<KeyCode>::default();
        keys.press(KeyCode::KeyG);
        translate(&keys, &mouse, (&[], &[]), &b, false, &mut t);
        assert!(t.keys.pressed(KeyCode::KeyG));
        // Moved to Use, G presses F for the game, and the frag's G no more.
        bind(&mut values, Action::Use, Input::Key(KeyCode::KeyG));
        let b = Bindings::from_values(|d| values.get(d).cloned());
        let mut t = Translated::default();
        translate(&keys, &mouse, (&[], &[]), &b, false, &mut t);
        assert!(t.keys.pressed(KeyCode::KeyF) && !t.keys.pressed(KeyCode::KeyG));
    }

    #[test]
    fn a_key_binds_to_one_action() {
        let mut values = HashMap::new();
        bind(&mut values, Action::Reload, Input::Key(KeyCode::KeyF));
        assert_eq!(values[Action::Use.dvar()], "");
        assert_eq!(values[Action::Reload.dvar()], "KeyR KeyF");
        bind(&mut values, Action::Reload, Input::Key(KeyCode::KeyT));
        assert_eq!(values[Action::Reload.dvar()], "KeyR KeyT");
    }

    #[test]
    fn unbound_keys_pass_through() {
        let b = Bindings::default();
        let mut keys = ButtonInput::<KeyCode>::default();
        keys.press(KeyCode::Escape);
        keys.press(KeyCode::KeyW);
        let mut t = Translated::default();
        translate(&keys, &ButtonInput::default(), (&[], &[]), &b, true, &mut t);
        assert!(t.keys.pressed(KeyCode::Escape) && t.keys.pressed(KeyCode::KeyW));
    }
}

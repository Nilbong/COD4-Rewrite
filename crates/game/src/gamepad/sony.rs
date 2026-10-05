//! PlayStation pads read over HID ([`playstation`]) as Bevy gamepads: an
//! entity per pad, fed the way gilrs feeds its own, so everything else
//! treats them like any pad. gilrs still sees the same pads through Windows;
//! while one is read here those twins are ignored ([`SonyPads::twin`]).
//! `COD4RW_PAD_HID=0` leaves them all to gilrs.

use super::playstation::{self, Event, Input, Link, button};
use bevy::input::gamepad::{
    GamepadConnection, GamepadConnectionEvent, RawGamepadAxisChangedEvent, RawGamepadButtonChangedEvent, RawGamepadEvent,
};
use bevy::prelude::*;
use std::collections::HashMap;

pub(super) fn register(app: &mut App) {
    if std::env::var("COD4RW_PAD_HID").is_ok_and(|v| v == "0") {
        info!("gamepad: PlayStation pads left to Windows (COD4RW_PAD_HID=0)");
        return;
    }
    if let Some(link) = Link::start() {
        app.insert_resource(SonyLink(link)).init_resource::<SonyPads>().add_systems(First, feed);
    }
}

/// The HID thread.
#[derive(Resource)]
pub(super) struct SonyLink(pub Link);

/// A pad read over HID.
#[derive(Component)]
pub(super) struct HidPad;

#[derive(Resource, Default)]
pub(super) struct SonyPads {
    pads: HashMap<u32, Pad>,
}

impl SonyPads {
    /// A Sony pad gilrs sees while HID reads one: it's the same pad, seen
    /// worse.
    pub(super) fn twin(pads: Option<&SonyPads>, entity: Entity, vendor: Option<u16>) -> bool {
        pads.is_some_and(|p| vendor == Some(playstation::SONY) && !p.pads.is_empty() && !p.pads.values().any(|p| p.entity == entity))
    }
}

struct Pad {
    entity: Entity,
    input: Input,
    /// What Bevy has been told, once it knows the pad.
    told: Option<Input>,
}

fn feed(
    mut commands: Commands,
    link: Res<SonyLink>,
    mut pads: ResMut<SonyPads>,
    mut connect: MessageWriter<GamepadConnectionEvent>,
    mut raw: MessageWriter<RawGamepadEvent>,
    mut new: Local<Vec<u32>>,
) {
    new.clear();
    for event in link.0.events() {
        match event {
            Event::Connected { id, model, product, bluetooth, name } => {
                info!("gamepad: {name} ({model:?} {product:04x}, {}) read over HID", if bluetooth { "Bluetooth" } else { "USB" });
                let entity = commands.spawn(HidPad).id();
                connect.write(GamepadConnectionEvent::new(
                    entity,
                    GamepadConnection::Connected { name, vendor_id: Some(playstation::SONY), product_id: Some(product) },
                ));
                pads.pads.insert(id, Pad { entity, input: Input::default(), told: None });
                new.push(id);
            }
            Event::Input { id, input } => {
                if let Some(p) = pads.pads.get_mut(&id) {
                    p.input = input;
                    // Each change goes to Bevy in order (a tap within a
                    // frame still presses and lets go), once it knows the
                    // pad.
                    if let Some(told) = p.told.filter(|t| *t != input) {
                        tell(&mut raw, p.entity, Some(told), input);
                        p.told = Some(input);
                    }
                }
            }
            Event::Disconnected { id } => {
                if let Some(p) = pads.pads.remove(&id) {
                    // Bevy keeps disconnected pads' entities, as for gilrs's.
                    connect.write(GamepadConnectionEvent::new(p.entity, GamepadConnection::Disconnected));
                }
            }
        }
    }
    // Pads Bevy has met (the frame after they connect) get what changed.
    for (id, p) in &mut pads.pads {
        if new.contains(id) || p.told == Some(p.input) {
            continue;
        }
        tell(&mut raw, p.entity, p.told, p.input);
        p.told = Some(p.input);
    }
}

/// Bevy's buttons from Sony's: the touchpad's click is Select too, as
/// PlayStation shooters use it.
const BUTTONS: [(GamepadButton, u32); 15] = [
    (GamepadButton::South, button::CROSS),
    (GamepadButton::East, button::CIRCLE),
    (GamepadButton::West, button::SQUARE),
    (GamepadButton::North, button::TRIANGLE),
    (GamepadButton::LeftTrigger, button::L1),
    (GamepadButton::RightTrigger, button::R1),
    (GamepadButton::Select, button::CREATE | button::TOUCHPAD),
    (GamepadButton::Start, button::OPTIONS),
    (GamepadButton::LeftThumb, button::L3),
    (GamepadButton::RightThumb, button::R3),
    (GamepadButton::Mode, button::PS),
    (GamepadButton::DPadUp, button::UP),
    (GamepadButton::DPadDown, button::DOWN),
    (GamepadButton::DPadLeft, button::LEFT),
    (GamepadButton::DPadRight, button::RIGHT),
];

fn tell(raw: &mut MessageWriter<RawGamepadEvent>, pad: Entity, old: Option<Input>, new: Input) {
    let was = old.unwrap_or_default();
    let all = old.is_none();
    let axes = [
        (GamepadAxis::LeftStickX, new.left.0, was.left.0),
        (GamepadAxis::LeftStickY, new.left.1, was.left.1),
        (GamepadAxis::RightStickX, new.right.0, was.right.0),
        (GamepadAxis::RightStickY, new.right.1, was.right.1),
    ];
    for (axis, v, before) in axes {
        if all || v != before {
            raw.write(RawGamepadEvent::Axis(RawGamepadAxisChangedEvent::new(pad, axis, v)));
        }
    }
    let held = |i: &Input, bits: u32| if i.buttons & bits != 0 { 1.0 } else { 0.0 };
    let buttons = BUTTONS
        .iter()
        .map(|&(b, bits)| (b, held(&new, bits), held(&was, bits)))
        .chain([(GamepadButton::LeftTrigger2, new.l2, was.l2), (GamepadButton::RightTrigger2, new.r2, was.r2)]);
    for (b, v, before) in buttons {
        if all || v != before {
            raw.write(RawGamepadEvent::Button(RawGamepadButtonChangedEvent::new(pad, b, v)));
        }
    }
}

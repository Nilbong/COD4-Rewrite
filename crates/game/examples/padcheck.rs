//! What the game's controller layer (gilrs) makes of the controllers plugged
//! in: names, ids, which mapping it uses, rumble; then a rumble test and
//! every button and stick event for 30 seconds (press everything, move the
//! sticks and triggers all the way).
//!
//!     cargo run --release -p game --example padcheck > padcheck.txt

use gilrs::ff::{BaseEffect, BaseEffectType, EffectBuilder, Replay, Ticks};
use gilrs::{EventType, Gilrs};
use std::time::{Duration, Instant};

#[path = "../src/gamepad/playstation.rs"]
#[allow(dead_code)]
mod playstation;

/// Prints the rumble thread's log lines.
struct Print;

impl log::Log for Print {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }
    fn log(&self, r: &log::Record) {
        println!("  [{}] {}", r.level(), r.args());
    }
    fn flush(&self) {}
}

fn main() {
    let _ = log::set_logger(&Print).map(|()| log::set_max_level(log::LevelFilter::Info));
    let mut gilrs = match Gilrs::new() {
        Ok(g) => g,
        Err(gilrs::Error::NotImplemented(g)) => {
            println!("gilrs: no backend on this platform");
            g
        }
        Err(e) => {
            println!("gilrs failed: {e}");
            return;
        }
    };
    // Windows reports pads a moment after start.
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(1500) {
        while gilrs.next_event().is_some() {}
        std::thread::sleep(Duration::from_millis(50));
    }

    println!("gilrs sees {} pad(s):", gilrs.gamepads().count());
    let mut ids = Vec::new();
    for (id, pad) in gilrs.gamepads() {
        ids.push(id);
        let uuid: String = pad.uuid().iter().map(|b| format!("{b:02x}")).collect();
        println!("  #{id}: \"{}\" (os name \"{}\")", pad.name(), pad.os_name());
        println!("      vendor {:04x?} product {:04x?} uuid {uuid}", pad.vendor_id(), pad.product_id());
        println!(
            "      mapping {:?}, rumble {}, connected {}, power {:?}",
            pad.mapping_source(),
            pad.is_ff_supported(),
            pad.is_connected(),
            pad.power_info()
        );
        // What gilrs makes of it at rest: mapped controls, then raw codes.
        use gilrs::{Axis, Button};
        let axes = [Axis::LeftStickX, Axis::LeftStickY, Axis::RightStickX, Axis::RightStickY, Axis::LeftZ, Axis::RightZ];
        let mapped: Vec<String> = axes.iter().map(|&a| format!("{a:?} {:+.2}", pad.value(a))).collect();
        println!("      at rest: {}", mapped.join(", "));
        let triggers = [Button::LeftTrigger2, Button::RightTrigger2];
        let held: Vec<String> =
            triggers.iter().map(|&b| format!("{b:?} {:.2}", pad.button_data(b).map_or(0.0, |d| d.value()))).collect();
        println!("      triggers: {}; buttons held: {:?}", held.join(", "), pad.state().buttons().filter(|b| b.1.is_pressed()).map(|b| b.0.to_string()).collect::<Vec<_>>());
        let raw: Vec<String> = pad.state().axes().map(|(c, d)| format!("{c} {:+.2}", d.value())).collect();
        println!("      raw axes: {}", raw.join(", "));
    }
    let sony = playstation::list();
    println!("HID sees {} Sony pad(s):", sony.len());
    for (model, product, bt, name) in &sony {
        println!("  {model:?} \"{name}\" product {product:04x} over {}", if *bt { "Bluetooth" } else { "USB" });
    }

    println!("rumble test (1 s each pad that can):");
    for &id in &ids {
        if !gilrs.gamepad(id).is_ff_supported() {
            continue;
        }
        let effect = EffectBuilder::new()
            .add_effect(BaseEffect {
                kind: BaseEffectType::Strong { magnitude: 40_000 },
                scheduling: Replay { play_for: Ticks::from_ms(1000), ..Default::default() },
                ..Default::default()
            })
            .gamepads(&[id])
            .finish(&mut gilrs);
        match effect.and_then(|e| e.play().map(|()| e)) {
            Ok(e) => {
                println!("  #{id}: through gilrs");
                std::thread::sleep(Duration::from_millis(1100));
                drop(e);
            }
            Err(e) => println!("  #{id}: gilrs rumble failed: {e}"),
        }
    }
    // Sony pads: rumble, and their input as the game reads it, over HID.
    let link = playstation::Link::start();
    if let Some(link) = &link {
        std::thread::sleep(Duration::from_millis(300));
        println!("  Sony pads: through HID");
        link.rumble(playstation::Rumble { strong: 0.6, weak: 0.6, duration: Duration::from_millis(1000) });
        std::thread::sleep(Duration::from_millis(1300));
    }

    println!("events for 30 s: press every button, move both sticks and both triggers all the way...");
    let start = Instant::now();
    let mut last_axis = std::collections::HashMap::new();
    let mut sony_last = std::collections::HashMap::new();
    while start.elapsed() < Duration::from_secs(30) {
        for ev in link.iter().flat_map(|l| l.events()) {
            let t = start.elapsed().as_secs_f32();
            match ev {
                playstation::Event::Input { id, input } => {
                    // Sticks in steps of 0.25, as below.
                    let q = |v: f32| (v * 4.0).round() as i32;
                    let key = [q(input.left.0), q(input.left.1), q(input.right.0), q(input.right.1), q(input.l2), q(input.r2), input.buttons as i32];
                    if sony_last.insert(id, key) != Some(key) {
                        println!(
                            "{t:6.2} HID {id}: left {:+.2} {:+.2} right {:+.2} {:+.2} L2 {:.2} R2 {:.2} buttons {}",
                            input.left.0, input.left.1, input.right.0, input.right.1, input.l2, input.r2, sony_buttons(input.buttons)
                        );
                    }
                }
                other => println!("{t:6.2} HID {other:?}"),
            }
        }
        while let Some(ev) = gilrs.next_event() {
            let t = start.elapsed().as_secs_f32();
            match ev.event {
                // Axes report constantly: only print steps of 0.25 and the ends.
                EventType::AxisChanged(axis, v, code) => {
                    let step = (v * 4.0).round() as i32;
                    if last_axis.insert((ev.id, code), step) != Some(step) {
                        println!("{t:6.2} #{}: axis {axis:?} = {v:+.2} (code {code})", ev.id);
                    }
                }
                EventType::ButtonChanged(button, v, code) => {
                    let step = (v * 4.0).round() as i32;
                    if last_axis.insert((ev.id, code), step) != Some(step) {
                        println!("{t:6.2} #{}: button {button:?} = {v:.2} (code {code})", ev.id);
                    }
                }
                EventType::ButtonPressed(button, code) => println!("{t:6.2} #{}: pressed {button:?} (code {code})", ev.id),
                EventType::ButtonReleased(button, code) => println!("{t:6.2} #{}: released {button:?} (code {code})", ev.id),
                EventType::Connected => {
                    let pad = gilrs.gamepad(ev.id);
                    println!("{t:6.2} #{}: connected \"{}\" vendor {:04x?} product {:04x?}", ev.id, pad.name(), pad.vendor_id(), pad.product_id());
                }
                EventType::Disconnected => println!("{t:6.2} #{}: disconnected", ev.id),
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    println!("done");
}

fn sony_buttons(held: u32) -> String {
    use playstation::button::*;
    let names = [
        (CROSS, "cross"),
        (CIRCLE, "circle"),
        (SQUARE, "square"),
        (TRIANGLE, "triangle"),
        (L1, "L1"),
        (R1, "R1"),
        (L2, "L2"),
        (R2, "R2"),
        (CREATE, "create"),
        (OPTIONS, "options"),
        (L3, "L3"),
        (R3, "R3"),
        (PS, "PS"),
        (TOUCHPAD, "touchpad"),
        (MUTE, "mute"),
        (UP, "up"),
        (DOWN, "down"),
        (LEFT, "left"),
        (RIGHT, "right"),
    ];
    let held: Vec<_> = names.iter().filter(|n| held & n.0 != 0).map(|n| n.1).collect();
    if held.is_empty() { "-".into() } else { held.join(" ") }
}

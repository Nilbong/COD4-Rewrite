//! PlayStation pads (DualSense, DualShock 4) over HID, as SDL and Linux's
//! `hid-playstation` drive them. On Windows gilrs reaches these pads through
//! Windows.Gaming.Input as "raw game controllers": their buttons and axes
//! come through an SDL mapping written for DirectInput, Bluetooth pads stop
//! reporting there once anything (Steam, SDL, rumble) switches them to their
//! full reports, and there are no motors to drive. So a thread opens them
//! itself: it reads their input reports (USB, and Bluetooth in either mode)
//! and sends rumble as output reports. Looking for newly plugged-in pads
//! (slow on Windows: hundreds of milliseconds to list HID devices) is a
//! second thread's job, so reading never stops for it.
//!
//! Self-contained (std, `hidapi` and `log`): `examples/padcheck.rs` includes
//! this file too.

use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;
#[cfg(windows)]
use std::time::Instant;

pub const SONY: u16 = 0x054C;

/// Sony's pads by USB product id: DualSense (and its Edge), DualShock 4
/// (both versions, and the wireless adapter).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    DualSense,
    DualShock4,
}

impl Model {
    pub fn of(product: u16) -> Option<Model> {
        match product {
            0x0CE6 | 0x0DF2 => Some(Model::DualSense),
            0x05C4 | 0x09CC | 0x0BA0 => Some(Model::DualShock4),
            _ => None,
        }
    }
}

/// Buttons in [`Input::buttons`].
pub mod button {
    pub const SQUARE: u32 = 1 << 0;
    pub const CROSS: u32 = 1 << 1;
    pub const CIRCLE: u32 = 1 << 2;
    pub const TRIANGLE: u32 = 1 << 3;
    pub const L1: u32 = 1 << 4;
    pub const R1: u32 = 1 << 5;
    pub const L2: u32 = 1 << 6;
    pub const R2: u32 = 1 << 7;
    /// Create (DualSense), Share (DualShock 4).
    pub const CREATE: u32 = 1 << 8;
    pub const OPTIONS: u32 = 1 << 9;
    pub const L3: u32 = 1 << 10;
    pub const R3: u32 = 1 << 11;
    pub const PS: u32 = 1 << 12;
    pub const TOUCHPAD: u32 = 1 << 13;
    pub const MUTE: u32 = 1 << 14;
    pub const UP: u32 = 1 << 15;
    pub const DOWN: u32 = 1 << 16;
    pub const LEFT: u32 = 1 << 17;
    pub const RIGHT: u32 = 1 << 18;
}

/// A pad's controls: sticks -1..1 (x right, y up), triggers 0..1, and
/// [`button`]s held.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Input {
    pub left: (f32, f32),
    pub right: (f32, f32),
    pub l2: f32,
    pub r2: f32,
    pub buttons: u32,
}

/// Reads an input report (report id first). Both pads put the sticks, the
/// buttons (hat in the low nibble, then square cross circle triangle; L1 R1
/// L2 R2 share options L3 R3; PS touchpad mute) and the triggers in the same
/// order, at offsets that depend on the report.
pub fn parse(model: Model, r: &[u8]) -> Option<Input> {
    let (sticks, buttons, triggers) = match (model, *r.first()?, r.len()) {
        // DualSense over USB, and Bluetooth's full reports (a tag first).
        // Windows pads Bluetooth's basic reports to 78 bytes.
        (Model::DualSense, 0x01, 64) => (1, 8, 5),
        (Model::DualSense, 0x31, 78..) => (2, 9, 6),
        // DualShock 4 over Bluetooth's full reports (two bytes first).
        (Model::DualShock4, 0x11, 78..) => (3, 7, 10),
        // DualShock 4 over USB, and either over Bluetooth's basic reports.
        (_, 0x01, 10..) => (1, 5, 8),
        _ => return None,
    };
    let s = r.get(sticks..sticks + 4)?;
    let b = r.get(buttons..buttons + 3)?;
    let t = r.get(triggers..triggers + 2)?;
    let axis = |v: u8| ((v as f32 - 127.5) / 127.5).clamp(-1.0, 1.0);
    use button::*;
    let bits = [
        (b[0], 4, SQUARE),
        (b[0], 5, CROSS),
        (b[0], 6, CIRCLE),
        (b[0], 7, TRIANGLE),
        (b[1], 0, L1),
        (b[1], 1, R1),
        (b[1], 2, L2),
        (b[1], 3, R2),
        (b[1], 4, CREATE),
        (b[1], 5, OPTIONS),
        (b[1], 6, L3),
        (b[1], 7, R3),
        (b[2], 0, PS),
        (b[2], 1, TOUCHPAD),
    ];
    let mut held = bits.iter().filter(|(byte, bit, _)| byte & (1 << bit) != 0).fold(0, |h, b| h | b.2);
    if model == Model::DualSense && b[2] & 4 != 0 {
        held |= MUTE;
    }
    // The hat: 0 up, clockwise in eighths, 8 (or more) let go.
    held |= match b[0] & 0x0F {
        0 => UP,
        1 => UP | RIGHT,
        2 => RIGHT,
        3 => DOWN | RIGHT,
        4 => DOWN,
        5 => DOWN | LEFT,
        6 => LEFT,
        7 => UP | LEFT,
        _ => 0,
    };
    Some(Input {
        left: (axis(s[0]), -axis(s[1])),
        right: (axis(s[2]), -axis(s[3])),
        l2: t[0] as f32 / 255.0,
        r2: t[1] as f32 / 255.0,
        buttons: held,
    })
}

/// The report that sets the motors: `strong` is the left (low frequency)
/// motor, `weak` the right. `seq` counts DualSense Bluetooth reports.
pub fn rumble_report(model: Model, bluetooth: bool, strong: u8, weak: u8, seq: u8) -> Vec<u8> {
    match (model, bluetooth) {
        // `dualsense_output_report_usb`: id 0x02, then the common part:
        // valid_flag0 (compatible vibration | haptics select), valid_flag1
        // (nothing else), motor_right, motor_left.
        (Model::DualSense, false) => {
            let mut r = vec![0u8; 48];
            r[0] = 0x02;
            r[1] = 0x03;
            r[3] = weak;
            r[4] = strong;
            r
        }
        // `dualsense_output_report_bt`: id 0x31, sequence tag, tag 0x10,
        // the common part, then a CRC-32 of 0xA2 and the rest.
        (Model::DualSense, true) => {
            let mut r = vec![0u8; 78];
            r[0] = 0x31;
            r[1] = (seq & 0x0F) << 4;
            r[2] = 0x10;
            r[3] = 0x03;
            r[5] = weak;
            r[6] = strong;
            seal(&mut r);
            r
        }
        // DualShock 4 over USB: id 0x05, flags (0x01: rumble only, so the
        // light bar stays as it is), then right and left motors.
        (Model::DualShock4, false) => {
            let mut r = vec![0u8; 32];
            r[0] = 0x05;
            r[1] = 0x01;
            r[4] = weak;
            r[5] = strong;
            r
        }
        // Over Bluetooth: id 0x11, HID + CRC (0xC0) at a 4 ms interval,
        // flags (rumble), the motors, and the CRC.
        (Model::DualShock4, true) => {
            let mut r = vec![0u8; 78];
            r[0] = 0x11;
            r[1] = 0xC4;
            r[3] = 0x01;
            r[6] = weak;
            r[7] = strong;
            seal(&mut r);
            r
        }
    }
}

/// Bluetooth output reports end with a CRC-32 of 0xA2 and the report.
fn seal(r: &mut [u8]) {
    let n = r.len() - 4;
    let crc = crc32(std::iter::once(0xA2).chain(r[..n].iter().copied()));
    r[n..].copy_from_slice(&crc.to_le_bytes());
}

/// CRC-32 (IEEE, as zlib's).
pub fn crc32(bytes: impl IntoIterator<Item = u8>) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// A rumble: motor strengths 0..1 for a while.
#[derive(Clone, Copy, Debug)]
pub struct Rumble {
    pub strong: f32,
    pub weak: f32,
    pub duration: Duration,
}

/// What the HID thread reports. `id` tells pads apart.
#[derive(Clone, Debug)]
pub enum Event {
    Connected { id: u32, model: Model, product: u16, bluetooth: bool, name: String },
    Input { id: u32, input: Input },
    Disconnected { id: u32 },
}

/// The HID thread: Sony pads' input comes out of [`Link::events`], rumble
/// goes to every one of them.
pub struct Link {
    tx: Sender<Rumble>,
    rx: Mutex<Receiver<Event>>,
}

impl Link {
    /// `None` where there's no HID access (not Windows).
    pub fn start() -> Option<Link> {
        #[cfg(windows)]
        {
            let (tx, rumbles) = channel();
            let (events, rx) = channel();
            std::thread::Builder::new().name("playstation pads".into()).spawn(move || run(rumbles, events)).ok()?;
            Some(Link { tx, rx: Mutex::new(rx) })
        }
        #[cfg(not(windows))]
        {
            let _: fn() -> (Sender<Rumble>, Receiver<Rumble>) = channel;
            None
        }
    }

    pub fn rumble(&self, r: Rumble) {
        let _ = self.tx.send(r);
    }

    /// What happened since last time.
    pub fn events(&self) -> Vec<Event> {
        self.rx.lock().map(|rx| rx.try_iter().collect()).unwrap_or_default()
    }
}

/// Sony pads the HID layer sees: (model, product id, Bluetooth, name).
#[cfg(windows)]
pub fn list() -> Vec<(Model, u16, bool, String)> {
    let Ok(api) = hidapi::HidApi::new() else { return Vec::new() };
    sony_devices(&api).into_iter().map(|d| (d.0, d.1, d.2, d.4)).collect()
}

#[cfg(not(windows))]
pub fn list() -> Vec<(Model, u16, bool, String)> {
    Vec::new()
}

#[cfg(windows)]
fn sony_devices(api: &hidapi::HidApi) -> Vec<(Model, u16, bool, std::ffi::CString, String)> {
    let mut seen = std::collections::HashSet::new();
    api.device_list()
        .filter(|d| d.vendor_id() == SONY)
        .filter_map(|d| {
            let model = Model::of(d.product_id())?;
            // One per pad (a pad can list several interfaces).
            seen.insert(d.path().to_owned()).then(|| {
                let bt = matches!(d.bus_type(), hidapi::BusType::Bluetooth);
                (model, d.product_id(), bt, d.path().to_owned(), d.product_string().unwrap_or("").to_owned())
            })
        })
        .collect()
}

#[cfg(windows)]
fn run(rumbles: Receiver<Rumble>, events: Sender<Event>) {
    struct Pad {
        id: u32,
        dev: hidapi::HidDevice,
        path: std::ffi::CString,
        model: Model,
        bluetooth: bool,
        seq: u8,
        last: Option<Input>,
        /// The motors as last sent.
        sent: (u8, u8),
    }
    let Ok(api) = hidapi::HidApi::new() else {
        log::warn!("playstation pads: no HID access");
        return;
    };
    // Listing devices takes long enough on Windows to stall input (pads
    // seemed to hold buttons down for a moment every two seconds), so it
    // happens on a thread of its own, which sends what it finds.
    let (found_tx, found) = channel::<Vec<(Model, u16, bool, std::ffi::CString, String)>>();
    let _ = std::thread::Builder::new().name("playstation pad scan".into()).spawn(move || {
        let Ok(mut api) = hidapi::HidApi::new() else { return };
        loop {
            let _ = api.refresh_devices();
            if found_tx.send(sony_devices(&api)).is_err() {
                return;
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });
    let mut pads: Vec<Pad> = Vec::new();
    let mut next_id = 0;
    let mut playing: Vec<(f32, f32, Instant)> = Vec::new();
    let mut buf = [0u8; 128];
    loop {
        loop {
            match rumbles.try_recv() {
                Ok(r) => playing.push((r.strong.clamp(0.0, 1.0), r.weak.clamp(0.0, 1.0), Instant::now() + r.duration)),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(_) => return,
            }
        }
        let now = Instant::now();
        // Pads the scan found that aren't open yet.
        for devices in found.try_iter() {
            for (model, product, bluetooth, path, name) in devices {
                if pads.iter().any(|p| p.path == path) {
                    continue;
                }
                match api.open_path(&path) {
                    Ok(dev) => {
                        let name = if name.is_empty() { format!("{model:?}") } else { name };
                        log::info!("playstation pads: {name} ({product:04x}) over {}", if bluetooth { "Bluetooth" } else { "USB" });
                        next_id += 1;
                        let _ = events.send(Event::Connected { id: next_id, model, product, bluetooth, name });
                        pads.push(Pad { id: next_id, dev, path, model, bluetooth, seq: 0, last: None, sent: (0, 0) });
                    }
                    Err(e) => log::warn!("playstation pads: can't open {name}: {e}"),
                }
            }
        }
        if pads.is_empty() {
            std::thread::sleep(Duration::from_millis(16));
            continue;
        }

        // Rumble: what's playing adds up, as Bevy's does.
        playing.retain(|a| a.2 > now);
        let (strong, weak) = playing.iter().fold((0.0f32, 0.0f32), |(s, w), a| (s + a.0, w + a.1));
        let want = ((strong.min(1.0) * 255.0) as u8, (weak.min(1.0) * 255.0) as u8);
        // Input: wait a moment for reports, then take what's queued, each
        // change in turn (a quick tap can start and end between frames).
        let wait = (4 / pads.len()).max(1) as i32;
        pads.retain_mut(|p| {
            let mut timeout = wait;
            loop {
                match p.dev.read_timeout(&mut buf, timeout) {
                    Ok(0) => break,
                    Ok(n) => {
                        if let Some(input) = parse(p.model, &buf[..n]).filter(|i| p.last != Some(*i)) {
                            p.last = Some(input);
                            let _ = events.send(Event::Input { id: p.id, input });
                        }
                        timeout = 0;
                    }
                    Err(e) => {
                        log::info!("playstation pads: pad {} gone ({e})", p.id);
                        let _ = events.send(Event::Disconnected { id: p.id });
                        return false;
                    }
                }
            }
            if p.sent != want {
                p.seq = p.seq.wrapping_add(1);
                if let Err(e) = p.dev.write(&rumble_report(p.model, p.bluetooth, want.0, want.1, p.seq)) {
                    log::warn!("playstation pads: rumble failed ({e})");
                }
                // Once either way: a pad that can't rumble isn't asked again
                // until the motors change.
                p.sent = want;
            }
            true
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_matches_zlib() {
        assert_eq!(crc32(*b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn reports_are_sized_and_sealed() {
        let usb = rumble_report(Model::DualSense, false, 200, 100, 0);
        assert_eq!((usb.len(), usb[0], usb[3], usb[4]), (48, 0x02, 100, 200));
        let bt = rumble_report(Model::DualSense, true, 200, 100, 3);
        assert_eq!((bt.len(), bt[0], bt[1], bt[2], bt[5], bt[6]), (78, 0x31, 0x30, 0x10, 100, 200));
        let crc = u32::from_le_bytes(bt[74..78].try_into().unwrap());
        assert_eq!(crc, crc32(std::iter::once(0xA2).chain(bt[..74].iter().copied())));
        let ds4 = rumble_report(Model::DualShock4, true, 1, 2, 0);
        assert_eq!((ds4.len(), ds4[0], ds4[1], ds4[6], ds4[7]), (78, 0x11, 0xC4, 2, 1));
        assert_eq!(Model::of(0x0CE6), Some(Model::DualSense));
        assert_eq!(Model::of(0x09CC), Some(Model::DualShock4));
        assert_eq!(Model::of(0x1234), None);
    }

    #[test]
    fn reads_reports() {
        use button::*;
        // DualSense over USB: left stick right, right stick up, right
        // trigger half, cross and the d-pad's down right, L1, PS.
        let mut usb = [0u8; 64];
        usb[..12].copy_from_slice(&[0x01, 255, 128, 128, 0, 0, 128, 7, 0x23, 0x01, 0x01, 0]);
        let i = parse(Model::DualSense, &usb).unwrap();
        assert_eq!(i.buttons, CROSS | DOWN | RIGHT | L1 | PS);
        assert!(i.left.0 > 0.99 && i.left.1.abs() < 0.01 && i.right.1 > 0.99 && (i.r2 - 0.5).abs() < 0.01);
        // The same over Bluetooth, full and basic.
        let mut bt = [0u8; 78];
        bt[0] = 0x31;
        bt[2..13].copy_from_slice(&usb[1..12]);
        assert_eq!(parse(Model::DualSense, &bt), Some(i));
        let mut basic = [0u8; 78];
        basic[..10].copy_from_slice(&[0x01, 255, 128, 128, 0, 0x23, 0x01, 0x01, 0, 128]);
        assert_eq!(parse(Model::DualSense, &basic[..10]), Some(i));
        assert_eq!(parse(Model::DualSense, &basic), Some(i));
        // DualShock 4 over USB and Bluetooth: triangle, left trigger in.
        let mut ds4 = [0u8; 64];
        ds4[..10].copy_from_slice(&[0x01, 128, 128, 128, 128, 0x88, 0, 0, 255, 0]);
        let d = parse(Model::DualShock4, &ds4).unwrap();
        assert_eq!((d.buttons, d.l2), (TRIANGLE, 1.0));
        let mut ds4bt = [0u8; 78];
        ds4bt[0] = 0x11;
        ds4bt[3..12].copy_from_slice(&ds4[1..10]);
        assert_eq!(parse(Model::DualShock4, &ds4bt), Some(d));
        assert_eq!(parse(Model::DualSense, &[0x05, 1, 2]), None);
    }
}

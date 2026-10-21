//! `common_mp` (the zone every multiplayer map loads alongside its own:
//! guns, characters, effects) parsed ahead of time. It took ~5 s of every
//! map load; now it's parsed on a background thread at startup (while the
//! menus show) and again when a match ends (its copy is dropped then), for
//! the next one. (Parsing the next one while a match ran held two copies,
//! ~1.5 GB more at the peak.)
//! A map change while it's still parsing waits for it (`take` joins).
//! The held copy (a few hundred MB) waits for the next match, or goes with
//! the process. Off on Android (12 GB phones) unless `COD4RW_PREFETCH=1`;
//! `COD4RW_NOPREFETCH=1` turns it off anywhere (parsed in the load as
//! before).

use bevy::prelude::*;
use std::sync::Mutex;
use std::thread::JoinHandle;

type Parse = JoinHandle<Option<iw3::zone::Zone>>;

static NEXT: Mutex<Option<Parse>> = Mutex::new(None);

pub struct CommonZonePlugin;

impl Plugin for CommonZonePlugin {
    fn build(&self, app: &mut App) {
        prefetch();
        app.add_systems(OnExit(crate::state::GameState::InGame), |_: Commands| prefetch());
    }
}

fn wanted() -> bool {
    if std::env::var("COD4RW_NOPREFETCH").is_ok_and(|v| v == "1") {
        return false;
    }
    !cfg!(target_os = "android") || std::env::var("COD4RW_PREFETCH").is_ok_and(|v| v == "1")
}

/// Start parsing the next copy (unless one is already on its way).
pub fn prefetch() {
    if !wanted() {
        return;
    }
    let Ok(mut next) = NEXT.lock() else { return };
    if next.is_some() {
        return;
    }
    *next = std::thread::Builder::new()
        .name("common_mp prefetch".into())
        .spawn(|| {
            let install = iw3::Install::locate().ok()?;
            let data = iw3::fastfile::load(&install.zone_path("common_mp")).ok()?;
            iw3::zone::Zone::parse(&data, iw3::zone::ParseOptions::default()).ok()
        })
        .ok();
}

/// The parsed zone if one was started (waiting for it to finish), else None.
pub fn take() -> Option<iw3::zone::Zone> {
    let handle = NEXT.lock().ok()?.take()?;
    let t = std::time::Instant::now();
    let zone = handle.join().ok().flatten();
    if zone.is_some() {
        info!("common_mp: parsed ahead ({:?} waited)", t.elapsed());
    }
    zone
}

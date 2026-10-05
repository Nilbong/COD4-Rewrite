//! Real matches to learn from: every CoD4 demo in the install's
//! `main/demos` folder, read once in the background at startup and turned
//! into the motion-matching library ([`super::motion`]) and notes on each
//! map ([`super::learned`]).

use super::learned::{self, DemoNotes};
use super::motion::{self, MotionLibrary};
use bevy::prelude::*;
use std::sync::{Arc, Mutex};

/// While the demos are being read. Map analysis waits for it to go.
#[derive(Resource)]
pub struct Loading(Arc<Mutex<Option<Loaded>>>);

struct Loaded {
    motion: motion::Library,
    notes: DemoNotes,
}

pub fn setup(app: &mut App) {
    let slot = Arc::new(Mutex::new(None));
    let out = slot.clone();
    std::thread::spawn(move || {
        let t0 = std::time::Instant::now();
        let mut demos = Vec::new();
        if let Ok(install) = iw3::Install::locate() {
            let dir = install.root.join("main").join("demos");
            for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let path = e.path();
                if path.extension().is_some_and(|x| x.eq_ignore_ascii_case("dm_1")) {
                    match std::fs::read(&path).map_err(anyhow::Error::from).and_then(|d| iw3::demo::read(&d)) {
                        Ok(d) => demos.push(d),
                        Err(err) => warn!("demos: skipping {}: {err:#}", path.display()),
                    }
                }
            }
        }
        let lib = motion::build(&demos);
        info!(
            "motion matching: {} clips from {} demos ({} player tracks, {:.0} min)",
            lib.len(),
            demos.len(),
            lib.players,
            lib.seconds / 60.0
        );
        let notes = learned::build(&demos);
        for (map, n) in notes.0.iter() {
            info!(
                "demos: {map}: {} players, {:.0} min; {} spots held ({:.0} s)",
                n.players,
                n.seconds / 60.0,
                n.holds.len(),
                n.holds.iter().map(|h| h.seconds).sum::<f32>()
            );
        }
        info!("demos read in {:?}", t0.elapsed());
        *out.lock().unwrap() = Some(Loaded { motion: lib, notes });
    });
    app.insert_resource(Loading(slot)).add_systems(Update, publish.run_if(resource_exists::<Loading>));
}

fn publish(mut commands: Commands, loading: Res<Loading>) {
    let loaded = loading.0.lock().unwrap().take();
    match loaded {
        Some(l) => {
            if l.motion.len() > 0 {
                commands.insert_resource(MotionLibrary(Arc::new(l.motion)));
            }
            commands.insert_resource(l.notes);
        }
        // Still reading (the thread holds the other reference).
        None if Arc::strong_count(&loading.0) > 1 => return,
        // The thread died without a result.
        None => commands.insert_resource(DemoNotes::default()),
    }
    commands.remove_resource::<Loading>();
}

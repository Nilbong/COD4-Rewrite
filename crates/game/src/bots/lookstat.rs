//! Debug measure: where players look while they walk, against the map's
//! lanes. With `COD4RW_LOOKSTAT=<dir>[,<dir>...]` (and `COD4RW_SIM`), once
//! the map is analysed, every track CSV in each directory (`iw3 --example
//! demodump` for real players, `COD4RW_RECORD` for bots: both have `t`,
//! `yaw`, `pitch`, `fire`, `x`, `y`, `z`) is replayed against this map's
//! lane points, and per directory it logs, of the time spent walking
//! outside fights:
//! - how often the view points at a lane point in sight (within 10 degrees);
//! - how often at one well off the way they're walking (a check to the side);
//! - the median angle between the view and the way they walk.
//!
//! So real players and bots are measured the same way, on the same map.

use super::tactical::TacticalMap;
use crate::collision;
use crate::units::{pos, u};
use avian3d::prelude::*;
use bevy::prelude::*;

pub(super) fn setup(app: &mut App) {
    if std::env::var_os("COD4RW_LOOKSTAT").is_some() {
        app.add_systems(Update, measure.run_if(crate::state::in_game));
    }
}

/// One sample of a track.
struct Sample {
    t: f32,
    yaw: f32,
    pitch: f32,
    fire: bool,
    feet: Vec3,
}

fn read(path: &std::path::Path) -> Vec<Sample> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    let mut lines = text.lines();
    let Some(header) = lines.next() else { return Vec::new() };
    let col = |name: &str| header.split(',').position(|h| h == name);
    let (Some(t), Some(yaw), Some(pitch), Some(fire), Some(x), Some(y), Some(z)) =
        (col("t"), col("yaw"), col("pitch"), col("fire"), col("x"), col("y"), col("z"))
    else {
        return Vec::new();
    };
    let dead = col("dead");
    lines
        .filter_map(|l| {
            let f: Vec<&str> = l.split(',').collect();
            let n = |i: usize| f.get(i)?.trim().parse::<f32>().ok();
            if dead.and_then(n).is_some_and(|d| d > 0.0) {
                return None;
            }
            Some(Sample {
                t: n(t)?,
                yaw: n(yaw)?.to_radians(),
                pitch: n(pitch)?.to_radians(),
                fire: n(fire)? > 0.0,
                feet: pos([n(x)?, n(y)?, n(z)?]),
            })
        })
        .collect()
}

fn clear(spatial: &SpatialQuery, from: Vec3, to: Vec3) -> bool {
    let d = to - from;
    Dir3::new(d).ok().is_none_or(|dir| spatial.cast_ray(from, dir, d.length(), true, &collision::sight_filter()).is_none())
}

fn measure(tactics: Option<Res<TacticalMap>>, spatial: SpatialQuery, mut done: Local<bool>) {
    let Some(t) = tactics.filter(|_| !*done) else { return };
    *done = true;
    let dirs = std::env::var("COD4RW_LOOKSTAT").unwrap_or_default();
    for dir in dirs.split(',').filter(|d| !d.is_empty()) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            warn!("lookstat: can't read {dir}");
            continue;
        };
        let (mut walking, mut at_lane, mut side_check) = (0u32, 0u32, 0u32);
        let mut offsets: Vec<f32> = Vec::new();
        for e in entries.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "csv")) {
            let samples = read(&e.path());
            let mut last_t = f32::NEG_INFINITY;
            for w in samples.windows(2) {
                let (a, b) = (&w[0], &w[1]);
                // Four looks a second is plenty.
                if a.t - last_t < 0.25 || a.fire {
                    continue;
                }
                let dt = b.t - a.t;
                if !(0.01..0.5).contains(&dt) {
                    continue;
                }
                let step = (b.feet - a.feet).with_y(0.0);
                if step.length() / dt / u(1.0) < 80.0 {
                    continue;
                }
                last_t = a.t;
                walking += 1;
                let view = Quat::from_euler(EulerRot::YXZ, a.yaw, a.pitch, 0.0) * Vec3::NEG_Z;
                let going = step.normalize();
                let flat = view.with_y(0.0).normalize_or_zero();
                offsets.push(flat.dot(going).clamp(-1.0, 1.0).acos().to_degrees());
                let eye = a.feet + Vec3::Y * u(60.0);
                let mut hit = None;
                for (_, l) in t.lanes_near(a.feet, u(1800.0)) {
                    let target = l.pos + Vec3::Y * u(48.0);
                    let to = target - eye;
                    if to.length() < u(300.0) || view.dot(to.normalize()) < 10f32.to_radians().cos() {
                        continue;
                    }
                    if clear(&spatial, eye, target) {
                        hit = Some(to.with_y(0.0).normalize_or_zero());
                        break;
                    }
                }
                if let Some(to) = hit {
                    at_lane += 1;
                    if to.dot(going) < 35f32.to_radians().cos() {
                        side_check += 1;
                    }
                }
            }
        }
        offsets.sort_by(f32::total_cmp);
        let median = offsets.get(offsets.len() / 2).copied().unwrap_or(0.0);
        let pct = |n: u32| 100.0 * n as f32 / walking.max(1) as f32;
        info!(
            "lookstat {dir}: {walking} walking looks; at a lane in sight {:.1}%, checking one to the side {:.1}%, view off the way walked p50 {median:.0} deg",
            pct(at_lane),
            pct(side_check)
        );
    }
}

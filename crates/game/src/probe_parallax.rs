//! Parallax-corrected reflection probes (showcase only). CoD4's probes are
//! cube maps taken at a point, with no bounds, so a surface looked them up
//! as if everything they saw were infinitely far: a room's reflection
//! slid over the floor as the eye moved. Here each probe gets a box for
//! its surroundings, found once the map's collision is up by casting rays
//! out from it along each axis, and `world.wgsl` looks the probe up where
//! the reflected ray meets that box (`WorldParams::probe_box_*`).
//! Off unless `COD4RW_PROBEBOX=1`; `COD4RW_REFLDEBUG=1` shows only the
//! reflections; `COD4RW_PROBEBOX_AB=1`
//! turns them on and off with the shots of `COD4RW_SHOT_VIEWS` (on for the
//! 1st, 3rd, ... shot), so the same view can be shot both ways in one run.

use bevy::prelude::*;

use crate::world::WorldMaterial;

pub struct ProbeParallaxPlugin;

impl Plugin for ProbeParallaxPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, fit_boxes.run_if(crate::state::in_game));
        if std::env::var_os("COD4RW_PROBEBOX_AB").is_some() {
            app.add_systems(Update, ab_toggle.after(fit_boxes).run_if(crate::state::in_game));
        }
    }
}

/// Farthest a box reaches from its probe along an axis (metres): open
/// sides (sky, long streets) stop here.
const REACH: f32 = 40.0;
/// Rays per side: the axis and eight around it, so a post or rail next to
/// the probe doesn't close that side.
const SPREAD: f32 = 0.3;

/// Off by default: CoD4's materials are too rough for the probes'
/// reflections to show (2026-10-14, the reflection-only view on Downpour
/// and Wet Work). `COD4RW_PROBEBOX=1` (or `COD4RW_REFLDEBUG=1`) turns it on.
fn enabled() -> bool {
    crate::atmos::climate::showcase()
        && (std::env::var("COD4RW_PROBEBOX").is_ok_and(|v| v == "1") || std::env::var("COD4RW_REFLDEBUG").is_ok_and(|v| v == "1"))
}

/// Once per map: every world material's probe box.
fn fit_boxes(
    spatial: avian3d::prelude::SpatialQuery,
    mut materials: ResMut<Assets<WorldMaterial>>,
    map: Option<Res<crate::world::MapName>>,
    mut done_for: Local<Option<String>>,
) {
    let Some(map) = map else { return };
    if done_for.as_deref() == Some(map.0.as_str()) || !enabled() {
        return;
    }
    let filter = avian3d::prelude::SpatialQueryFilter::from_mask([crate::collision::Layer::World]);
    let centers: Vec<Vec3> = {
        let mut list: Vec<Vec3> = Vec::new();
        for (_, m) in materials.iter() {
            let c = m.extension.params.probe_center;
            if c.w > 0.5 && !list.iter().any(|p| p.distance_squared(c.truncate()) < 1e-4) {
                list.push(c.truncate());
            }
        }
        list
    };
    if centers.is_empty() {
        return;
    }
    // The collision isn't up until a few frames in: wait for the ground
    // under some probe.
    if !centers.iter().any(|c| spatial.cast_ray(*c, Dir3::NEG_Y, REACH, true, &filter).is_some()) {
        return;
    }
    let started = std::time::Instant::now();
    let boxes: Vec<(Vec3, Option<(Vec3, Vec3)>)> = centers.iter().map(|c| (*c, fit(&spatial, &filter, *c))).collect();
    let mut set = 0;
    for (_, m) in materials.iter_mut() {
        let params = &mut m.extension.params;
        if params.probe_center.w < 0.5 {
            continue;
        }
        let c = params.probe_center.truncate();
        if let Some((_, Some((lo, hi)))) = boxes.iter().find(|(p, _)| p.distance_squared(c) < 1e-4) {
            params.probe_box_min = lo.extend(1.0);
            params.probe_box_max = hi.extend(0.0);
            set += 1;
        }
    }
    // `COD4RW_REFLDEBUG=1`: world materials show only their reflections
    // (`world.wgsl`).
    if std::env::var("COD4RW_REFLDEBUG").is_ok_and(|v| v == "1") {
        for (_, m) in materials.iter_mut() {
            m.extension.params.probe_box_max.w = 1.0;
        }
    }
    let fitted = boxes.iter().filter(|b| b.1.is_some()).count();
    info!("probe boxes: {fitted} of {} probes fitted, {set} materials, {:.1} ms", boxes.len(), started.elapsed().as_secs_f32() * 1000.0);
    *done_for = Some(map.0.clone());
}

/// The box around a probe: along each axis, the distance most of its rays
/// travel before meeting the world (or `REACH`). None when it sits inside
/// something solid.
fn fit(spatial: &avian3d::prelude::SpatialQuery, filter: &avian3d::prelude::SpatialQueryFilter, at: Vec3) -> Option<(Vec3, Vec3)> {
    let mut reach = [0.0f32; 6];
    for (i, axis) in [Vec3::X, Vec3::NEG_X, Vec3::Y, Vec3::NEG_Y, Vec3::Z, Vec3::NEG_Z].into_iter().enumerate() {
        let (u, v) = axis.any_orthonormal_pair();
        let mut hits: Vec<f32> = Vec::with_capacity(9);
        for a in [-1.0f32, 0.0, 1.0] {
            for b in [-1.0f32, 0.0, 1.0] {
                let dir = (axis + (u * a + v * b) * SPREAD).normalize();
                let d = spatial.cast_ray(at, Dir3::new(dir).unwrap_or(Dir3::Y), REACH, true, filter).map_or(REACH, |h| h.distance);
                // Along the axis itself.
                hits.push(d * dir.dot(axis));
            }
        }
        hits.sort_by(f32::total_cmp);
        // The 75th percentile: most of the side, not the nearest post.
        reach[i] = hits[6];
    }
    if reach.iter().filter(|r| **r < 0.05).count() >= 2 {
        return None;
    }
    let lo = at - Vec3::new(reach[1], reach[3], reach[5]);
    let hi = at + Vec3::new(reach[0], reach[2], reach[4]);
    Some((lo, hi))
}

/// `COD4RW_PROBEBOX_AB`: the boxes on for even shots of the autoshot's
/// views (its timing: from 5 s plus `COD4RW_SHOT_LATE`, 1.5 s each), off
/// for odd ones.
fn ab_toggle(time: Res<Time>, mut materials: ResMut<Assets<WorldMaterial>>, mut was: Local<Option<bool>>) {
    let late: f32 = std::env::var("COD4RW_SHOT_LATE").ok().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let k = ((time.elapsed_secs() - late - 5.0) / 1.5).floor().max(0.0) as i32;
    let on = k % 2 == 0;
    if *was == Some(on) {
        return;
    }
    let mut any = false;
    for (_, m) in materials.iter_mut() {
        let params = &mut m.extension.params;
        // Fitted boxes have a max corner above the min one.
        if params.probe_center.w > 0.5 && params.probe_box_max.x > params.probe_box_min.x {
            params.probe_box_min.w = on as u32 as f32;
            any = true;
        }
    }
    if any {
        info!("probe boxes {} for shot {k}", if on { "on" } else { "off" });
        *was = Some(on);
    }
}

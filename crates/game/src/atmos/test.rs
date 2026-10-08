//! Debug aid: with `COD4RW_ATMOSTEST=<dir>` (and `COD4RW_SPAWN`), screenshot
//! the same views under each sky and volumetric setting
//! (`<sky>_<volumetric>_<view>.png`), log each one's frame time and the
//! GPU time of the volumetric pass and the sky (with `COD4RW_PERF`), then
//! exit. `COD4RW_ATMOSVIEWS=yaw:pitch;...` sets the views (degrees, yaw
//! from the spawn's; pitch above 0 looks up; `sun` for either aims at the
//! sun, `sun+60` 60 degrees beside it), by default ahead and up.

use crate::movement::ViewAngles;
use crate::player::LocalPlayer;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use std::sync::Mutex;

pub struct AtmosTestPlugin;

impl Plugin for AtmosTestPlugin {
    fn build(&self, app: &mut App) {
        if let Ok(dir) = std::env::var("COD4RW_ATMOSTEST") {
            app.insert_resource(TestDir(dir.into())).add_systems(Update, run.run_if(crate::state::in_game));
        }
        if std::env::var_os("COD4RW_ATMOSSCOUT").is_some() {
            app.add_systems(Update, scout.run_if(crate::state::in_game));
        }
    }
}

static FORCED: Mutex<Option<(&'static str, &'static str)>> = Mutex::new(None);

/// The setting the test is trying, if it's running.
pub fn forced(dvar: &str) -> Option<&'static str> {
    let f = (*FORCED.lock().ok()?)?;
    match dvar {
        "r_sky" => Some(f.0),
        "r_volumetric" => Some(f.1),
        _ => None,
    }
}

const VARIANTS: [(&str, &str); 4] = [("classic", "off"), ("dynamic", "off"), ("dynamic", "low"), ("dynamic", "high")];

/// The hours to try in place of the variants (the showcase):
/// `COD4RW_ATMOSHOURS=6,12,18,0`.
fn hours() -> Vec<f32> {
    std::env::var("COD4RW_ATMOSHOURS").map_or(Vec::new(), |v| v.split(',').filter_map(|h| h.trim().parse().ok()).collect())
}

/// The variants to try: `COD4RW_ATMOSVARIANTS=classic:off,dynamic:high`,
/// or all of [`VARIANTS`].
fn variants() -> Vec<(&'static str, &'static str)> {
    if !hours().is_empty() {
        // The showcase, at each hour (Volumetric Low: god rays).
        return hours().iter().map(|_| ("dynamic", "low")).collect();
    }
    let Ok(list) = std::env::var("COD4RW_ATMOSVARIANTS") else { return VARIANTS.to_vec() };
    let all = [("classic", "off"), ("classic", "low"), ("classic", "high"), ("dynamic", "off"), ("dynamic", "low"), ("dynamic", "high")];
    list.split(',').filter_map(|v| all.iter().copied().find(|(s, l)| v.trim() == format!("{s}:{l}"))).collect()
}
/// Seconds per view: settle (exposure, the sky's first frames), then shoot.
const PER_VIEW: f32 = 3.0;

#[derive(Resource)]
struct TestDir(std::path::PathBuf);

#[allow(clippy::too_many_arguments)]
fn run(
    mut commands: Commands,
    time: Res<Time>,
    dir: Res<TestDir>,
    mut player: Query<&mut ViewAngles, With<LocalPlayer>>,
    suns: Query<&GlobalTransform, (With<DirectionalLight>, Without<crate::model_lighting::ViewModelSun>)>,
    diagnostics: Res<bevy::diagnostic::DiagnosticsStore>,
    mut start: Local<Option<(f32, f32)>>,
    mut done: Local<usize>,
    mut exit: MessageWriter<AppExit>,
    mut tod: ResMut<super::climate::TimeOfDay>,
) {
    let Ok(mut angles) = player.single_mut() else { return };
    // Toward the sun: absolute yaw and pitch (radians).
    let to_sun = suns.iter().next().map_or(Vec3::Y, |t| -t.forward().as_vec3());
    let sun = (f32::atan2(-to_sun.x, -to_sun.z), to_sun.y.asin());
    let views: Vec<(Result<f32, f32>, f32)> = std::env::var("COD4RW_ATMOSVIEWS")
        .unwrap_or_else(|_| "0:0;0:30".into())
        .split(';')
        .filter_map(|v| {
            let (y, p) = v.split_once(':')?;
            // `sun` or `sun+40`: the sun's yaw, or that many degrees beside it.
            let yaw = match y.trim().strip_prefix("sun") {
                Some(rest) => Err(rest.trim_start_matches('+').parse::<f32>().unwrap_or(0.0).to_radians()),
                None => Ok(y.trim().parse::<f32>().ok()?.to_radians()),
            };
            let pitch = if p.trim() == "sun" { sun.1 } else { p.trim().parse::<f32>().ok()?.to_radians() };
            Some((yaw, pitch))
        })
        .collect();
    let now = time.elapsed_secs();
    // Six seconds in (the map settled), from the spawn's yaw.
    let (t0, yaw0) = *start.get_or_insert((now + 6.0, angles.yaw));
    let t = now - t0;
    if t < 0.0 {
        return;
    }
    let variants = variants();
    let shots = variants.len() * views.len();
    let i = (t / PER_VIEW) as usize;
    if i >= shots {
        // (A second for the last screenshot to be written.)
        if t > shots as f32 * PER_VIEW + 1.0 {
            *FORCED.lock().unwrap() = None;
            exit.write(AppExit::Success);
        }
        return;
    }
    let (variant, view) = (variants[i / views.len()], views[i % views.len()]);
    *FORCED.lock().unwrap() = Some(variant);
    let hours = hours();
    if let Some(&h) = hours.get(i / views.len()) {
        if (tod.hours - h).abs() > 0.01 {
            super::climate::set_hour(&mut tod, h);
        }
    }
    angles.yaw = match view.0 {
        Ok(y) => yaw0 + y,
        Err(beside) => sun.0 + beside,
    };
    angles.pitch = view.1;
    if t - i as f32 * PER_VIEW >= PER_VIEW - 0.5 && *done == i {
        *done = i + 1;
        let name = match hours.get(i / views.len()) {
            Some(h) => format!("h{h:04.1}_{}", i % views.len()),
            None => format!("{}_{}_{}", variant.0, variant.1, i % views.len()),
        };
        std::fs::create_dir_all(&dir.0).ok();
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(dir.0.join(format!("{name}.png"))));
        let get = |path: &str| {
            diagnostics.iter().find(|d| d.path().as_str() == path).and_then(|d| d.average())
        };
        let ms = diagnostics.get(&bevy::diagnostic::FrameTimeDiagnosticsPlugin::FRAME_TIME).and_then(|d| d.average());
        info!(
            "atmostest {name}: frame {:.2} ms, gpu volumetric {:.3} ms, god rays {:.3} ms, sky {:.3} ms",
            ms.unwrap_or(0.0),
            get("render/volumetric_fog/elapsed_gpu").unwrap_or(0.0),
            get("render/god_rays/elapsed_gpu").unwrap_or(0.0),
            get("render/dynamic_sky/elapsed_gpu").unwrap_or(0.0),
        );
    }
}

/// Debug aid: with `COD4RW_ATMOSSCOUT=1`, 3 s into a match, log indoor
/// spots within 60 m in shade that look across a sunlit shaft (through a
/// window, doorway or roof hole), as `COD4RW_SPAWN` values facing it
/// (view `0:15`).
fn scout(
    time: Res<Time>,
    spatial: avian3d::prelude::SpatialQuery,
    player: Query<&Transform, With<LocalPlayer>>,
    suns: Query<&GlobalTransform, (With<DirectionalLight>, Without<crate::model_lighting::ViewModelSun>)>,
    mut done: Local<bool>,
) {
    use crate::units::u;
    if *done || time.elapsed_secs() < 3.0 {
        return;
    }
    let (Ok(tf), Some(sun)) = (player.single(), suns.iter().next()) else { return };
    *done = true;
    let here = crate::units::to_cod(tf.translation);
    info!("atmos scout: from COD4RW_SPAWN={:.0},{:.0},{:.0},0", here[0], here[1], here[2]);
    let filter = avian3d::prelude::SpatialQueryFilter::from_mask([crate::collision::Layer::World]);
    let to_sun = Dir3::new(-sun.forward().as_vec3()).unwrap_or(Dir3::Y);
    let lit = |eye: Vec3| spatial.cast_ray(eye, to_sun, 80.0, true, &filter).is_none();
    // `COD4RW_ATMOSSCOUT=sun`: outdoor spots where something stands partly
    // in front of the sun (a building's edge, a pole, a tree), for god rays.
    if std::env::var("COD4RW_ATMOSSCOUT").is_ok_and(|v| v == "sun") {
        let (side, up) = (to_sun.cross(Vec3::Y).normalize_or_zero(), to_sun.cross(to_sun.cross(Vec3::Y)).normalize_or_zero());
        let yaw = f32::atan2(-to_sun.x, -to_sun.z).to_degrees() + 90.0;
        let mut found = 0;
        for ix in -40..=40 {
            for iz in -40..=40 {
                let probe = tf.translation + Vec3::new(ix as f32 * 1.5, 1.6, iz as f32 * 1.5);
                let Some(floor) = spatial.cast_ray(probe, Dir3::NEG_Y, 2.9, true, &filter) else { continue };
                if floor.distance <= 0.0 || floor.normal.y < 0.9 {
                    continue;
                }
                let eye = probe - Vec3::Y * floor.distance + Vec3::Y * u(60.0);
                // 25 rays within 4 degrees of the sun.
                let (mut blocked, mut nearest) = (0, f32::MAX);
                for a in -2..=2 {
                    for b in -2..=2 {
                        let d = (to_sun.as_vec3() + (side * a as f32 + up * b as f32) * 0.035).normalize();
                        if let Some(h) = spatial.cast_ray(eye, Dir3::new(d).unwrap_or(Dir3::Y), 60.0, true, &filter) {
                            blocked += 1;
                            nearest = nearest.min(h.distance);
                        }
                    }
                }
                if (8..=17).contains(&blocked) && (3.0..40.0).contains(&nearest) {
                    let c = crate::units::to_cod(eye - Vec3::Y * u(60.0));
                    info!("atmos scout: COD4RW_SPAWN={:.0},{:.0},{:.0},{:.0} (view sun:sun, {blocked}/25 blocked at {nearest:.0} m)", c[0], c[1], c[2] + 1.0, yaw);
                    found += 1;
                    if found >= 20 {
                        return;
                    }
                }
            }
        }
        info!("atmos scout: {found} spots");
        return;
    }
    let mut found = 0;
    for level in [1.6, 4.6, 7.6] {
        for ix in -40..=40 {
            for iz in -40..=40 {
                let probe = tf.translation + Vec3::new(ix as f32 * 1.5, level, iz as f32 * 1.5);
                let Some(floor) = spatial.cast_ray(probe, Dir3::NEG_Y, 2.9, true, &filter) else { continue };
                if floor.distance <= 0.0 || floor.normal.y < 0.9 {
                    continue;
                }
                let ground = probe - Vec3::Y * floor.distance;
                // Indoors: a ceiling within 5 m.
                if spatial.cast_ray(ground + Vec3::Y, Dir3::Y, 5.0, true, &filter).is_none() {
                    continue;
                }
                let eye = ground + Vec3::Y * u(60.0);
                // In shade, looking across a shaft: of 16 points 1-12 m
                // along a view within 40 degrees of the sun's azimuth
                // (pitched up 15), some but not most in sunlight.
                if lit(eye) {
                    continue;
                }
                let sun_yaw = f32::atan2(-to_sun.x, -to_sun.z);
                for k in -4..=4 {
                    let yaw = sun_yaw + (k as f32 * 10.0).to_radians();
                    let view = Quat::from_euler(EulerRot::YXZ, yaw, 15f32.to_radians(), 0.0) * Vec3::NEG_Z;
                    let Ok(dir) = Dir3::new(view) else { continue };
                    let reach = spatial.cast_ray(eye, dir, 12.0, true, &filter).map_or(12.0, |h| h.distance);
                    if reach < 6.0 {
                        continue;
                    }
                    let lit_points = (0..16).filter(|&n| lit(eye + view * (1.0 + n as f32 * (reach - 1.0) / 16.0))).count();
                    if (3..=8).contains(&lit_points) {
                        let c = crate::units::to_cod(ground);
                        info!(
                            "atmos scout: COD4RW_SPAWN={:.0},{:.0},{:.0},{:.0} (view 0:15, {lit_points}/16 lit)",
                            c[0], c[1], c[2] + 1.0, yaw.to_degrees() + 90.0
                        );
                        found += 1;
                        if found >= 30 {
                            return;
                        }
                        break;
                    }
                }
            }
        }
    }
    info!("atmos scout: {found} spots");
}

//! Temporal anti-aliasing (`r_aa taa`) on the world camera, with
//! contrast-adaptive sharpening on the finished frame at native size.
//!
//! The world camera gets Bevy's `TemporalAntiAliasing` (which brings the
//! jitter, the mip bias and the motion-vector prepass with it) and the sun's
//! shadow gets the temporal filter (its samples are spread across frames for
//! the TAA to average). The viewmodel camera, which draws the gun over the
//! world's image and finishes the frame, keeps SMAA High for the gun: it has
//! no jitter, and the settings code gives it SMAA High for `taa`. With the
//! upscaler on, the TAA runs on the world's small image in place of SMAA and
//! FSR's sharpening follows; at native size CAS (knob `taa.cas`, 0 off)
//! sharpens the frame back after the TAA's softening.
//!
//! A cut (the camera moving more than 3 m in a frame: spawn, killcam,
//! spectating, a teleport) throws the history away. A Render Scale under
//! 100% with TAA goes through the upscaler's image (`render_scale::upscaler`). Not in splitscreen
//! (only player 1's camera would have it).
//! Knobs: `taa.cas` (0.4), `taa.mip_bias` (-1, plus log2 of the upscaler's
//! scale), `taa.cut` (3 m).

use bevy::anti_alias::contrast_adaptive_sharpening::ContrastAdaptiveSharpening;
use bevy::anti_alias::taa::TemporalAntiAliasing;
use bevy::render::camera::{MipBias, TemporalJitter};
use bevy::core_pipeline::prepass::MotionVectorPrepass;
use bevy::light::ShadowFilteringMethod;
use bevy::post_process::motion_blur::MotionBlur;
use bevy::prelude::*;

pub struct TaaPlugin;

impl Plugin for TaaPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostUpdate, (world_taa, sharpen).after(TransformSystems::Propagate).run_if(crate::state::in_game));
    }
}

/// Whether the settings ask for TAA (and it can run).
pub fn wanted(settings: &crate::settings::Settings) -> bool {
    let env = std::env::var("COD4RW_AA").ok();
    env.as_deref().unwrap_or(settings.text("r_aa")) == "taa" && !crate::splitscreen::active()
}

/// On the world camera when its motion-vector prepass came only with the TAA.
#[derive(Component)]
struct OurMotionVectors;

#[allow(clippy::type_complexity)]
fn world_taa(
    mut commands: Commands,
    settings: Res<crate::settings::Settings>,
    mut cameras: Query<
        (
            Entity,
            &GlobalTransform,
            Option<&mut TemporalAntiAliasing>,
            Option<&MipBias>,
            Option<&ShadowFilteringMethod>,
            (Has<MotionBlur>, Has<MotionVectorPrepass>, Has<OurMotionVectors>),
        ),
        With<crate::player::MainCamera>,
    >,
    mut last: Local<Option<Vec3>>,
) {
    let on = wanted(&settings);
    for (e, at, taa, bias, filter, (blur, motion, ours)) in &mut cameras {
        let pos = at.translation();
        let cut = last.is_some_and(|p| p.distance(pos) > crate::tune::get("taa.cut", 3.0));
        *last = Some(pos);
        match (on, taa) {
            (true, Some(mut taa)) => {
                if cut {
                    taa.reset = true;
                }
                let want = crate::tune::get("taa.mip_bias", -1.0) + crate::render_scale::upscaler(&settings).map_or(0.0, |(s, _)| s.log2());
                if bias.is_none_or(|b| (b.0 - want).abs() > 1e-3) {
                    commands.entity(e).insert(MipBias(want));
                }
                if filter != Some(&ShadowFilteringMethod::Temporal) {
                    commands.entity(e).insert(ShadowFilteringMethod::Temporal);
                }
            }
            (true, None) => {
                let mut c = commands.entity(e);
                c.insert((TemporalAntiAliasing::default(), ShadowFilteringMethod::Temporal));
                // (The motion vectors come with it; noted when they're ours
                // alone, to go again with it.)
                if !motion {
                    c.insert(OurMotionVectors);
                }
            }
            (false, Some(_)) => {
                let mut c = commands.entity(e);
                c.remove::<(TemporalAntiAliasing, TemporalJitter, MipBias)>().insert(ShadowFilteringMethod::Gaussian);
                // (Only if nothing else wants them since: bodycam's motion
                // blur, ray-traced lighting.)
                if ours && !blur {
                    c.remove::<(MotionVectorPrepass, OurMotionVectors)>();
                }
            }
            (false, None) => {}
        }
    }
}

/// CAS on the frame at native size while TAA is on (the upscaler's own
/// sharpening covers it otherwise).
fn sharpen(
    mut commands: Commands,
    settings: Res<crate::settings::Settings>,
    cameras: Query<(Entity, Option<&ContrastAdaptiveSharpening>), With<crate::player::ViewModelCamera>>,
) {
    let strength = crate::tune::get("taa.cas", 0.4);
    let on = wanted(&settings) && crate::render_scale::upscaler(&settings).is_none() && strength > 0.0;
    for (e, cas) in &cameras {
        match (on, cas) {
            (true, Some(c)) if (c.sharpening_strength - strength).abs() < 1e-4 => {}
            (true, _) => {
                commands.entity(e).insert(ContrastAdaptiveSharpening { enabled: true, sharpening_strength: strength, denoise: true });
            }
            (false, Some(_)) => {
                commands.entity(e).remove::<ContrastAdaptiveSharpening>();
            }
            (false, None) => {}
        }
    }
}

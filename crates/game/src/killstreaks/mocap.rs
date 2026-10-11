//! Motion-captured call-in hands: the phone (UAV, helicopter) taken out,
//! pressed and put away, from the user's own video (tools/mocap: the hand
//! tracked, retargeted onto CoD4's viewmodel arms, the grip fitted to the
//! phone), played in place of the C4 detonator's raise, fire and drop.
//!
//! The clips (`assets/mocap/*.json`, format `cod4rw-mocap-clip` v1) hold
//! per-joint local rotations and translations at a fixed rate, in an
//! xanim's own terms (CoD axes, inches, translations off the joint's
//! `anim_base`), so each loads as an [`XAnim`] and plays through the same
//! [`crate::models::AnimPlayer`] as CoD4's own. `hands.phone.mocap` (live
//! tuned, default 1) switches back to the C4's anims for comparison.

use super::hands::{HandsPhase, HardpointHands};
use bevy::prelude::*;
use iw3::xanim::{BoneTrack, RotTrack, TransTrack, XAnim};
use std::sync::{Arc, OnceLock};

pub(super) fn build(app: &mut App) {
    app.add_systems(Update, shots.run_if(|| std::env::var_os("COD4RW_MOCAPSHOT").is_some()));
}

/// A clip as an animation (`None` if it doesn't parse).
fn load(json: &str) -> Option<Arc<XAnim>> {
    let clip: serde_json::Value = serde_json::from_str(json).map_err(|e| warn!("mocap clip: {e}")).ok()?;
    if clip["format"] != "cod4rw-mocap-clip" {
        return None;
    }
    let frames = clip["frames"].as_u64()? as usize;
    let floats = |v: &serde_json::Value| -> Option<Vec<f32>> { v.as_array()?.iter().map(|x| x.as_f64().map(|f| f as f32)).collect() };
    // One key is constant; else a key a frame.
    let keys = |n: usize| if n == 1 { vec![0] } else { (0..n as u16).collect() };
    let mut bones = Vec::new();
    for b in clip["bones"].as_array()? {
        let rot: Vec<[f32; 4]> = b["rot"].as_array()?.iter().map(|q| floats(q).and_then(|v| v.try_into().ok())).collect::<Option<_>>()?;
        let trans: Vec<[f32; 3]> = b["trans"].as_array()?.iter().map(|t| floats(t).and_then(|v| v.try_into().ok())).collect::<Option<_>>()?;
        bones.push(BoneTrack {
            name: b["name"].as_str()?.to_owned(),
            rot: RotTrack::Keys { frames: keys(rot.len()), values: rot },
            trans: TransTrack::Keys { frames: keys(trans.len()), values: trans },
        });
    }
    if frames < 2 {
        return None;
    }
    Some(Arc::new(XAnim {
        name: format!("mocap_{}", clip["name"].as_str().unwrap_or("clip")),
        num_frames: (frames - 1) as u16,
        framerate: clip["fps"].as_f64()? as f32,
        looping: false,
        delta: false,
        bones,
        notifies: Vec::new(),
        delta_trans: TransTrack::None,
    }))
}

struct PhoneClips {
    raise: Option<Arc<XAnim>>,
    press: Option<Arc<XAnim>>,
    put_away: Option<Arc<XAnim>>,
}

fn phone_clips() -> &'static PhoneClips {
    static CLIPS: OnceLock<PhoneClips> = OnceLock::new();
    CLIPS.get_or_init(|| PhoneClips {
        raise: load(include_str!("../../assets/mocap/phone_raise.json")),
        press: load(include_str!("../../assets/mocap/phone_press.json")),
        put_away: load(include_str!("../../assets/mocap/phone_putaway.json")),
    })
}

/// The captured phone anim for this part of a call-in, if it has one and
/// it's switched on (`hands.phone.mocap`). Holding (the airstrike's
/// laptop's, never the phone's) keeps CoD4's.
pub fn phone_anim(phase: HandsPhase) -> Option<Arc<XAnim>> {
    if crate::tune::get("hands.phone.mocap", 1.0) < 0.5 {
        return None;
    }
    let c = phone_clips();
    match phase {
        HandsPhase::Raise => c.raise.clone(),
        HandsPhase::Press => c.press.clone(),
        HandsPhase::PutAway => c.put_away.clone(),
        _ => None,
    }
}

/// Test aid: `COD4RW_MOCAPSHOT=<dir>` saves a screenshot about every
/// 1/15 s of each phone call-in's raise, press and putting away, named by
/// the call-in, the part and the milliseconds into it
/// (`c1_raise_0350.png`), to set beside the video's frames.
fn shots(mut commands: Commands, time: Res<Time>, hands: Query<&HardpointHands>, mut next: Local<f32>, mut call: Local<(u32, u32)>) {
    let now = time.elapsed_secs();
    let Some(h) = hands.iter().find(|h| !super::hands::is_laptop(h.def) && matches!(h.phase, HandsPhase::Raise | HandsPhase::Press | HandsPhase::PutAway)) else { return };
    if h.phase == HandsPhase::Raise && call.1 != h.started.to_bits() {
        *call = (call.0 + 1, h.started.to_bits());
    }
    if now < *next {
        return;
    }
    *next = now + 1.0 / 15.0;
    let part = match h.phase {
        HandsPhase::Raise => "raise",
        HandsPhase::Press => "press",
        _ => "putaway",
    };
    let dir = std::path::PathBuf::from(std::env::var("COD4RW_MOCAPSHOT").unwrap_or_default());
    std::fs::create_dir_all(&dir).ok();
    let ms = ((now - h.started) * 1000.0).round().max(0.0) as u32;
    let path = dir.join(format!("c{}_{part}_{ms:04}.png", call.0.max(1)));
    commands.spawn(bevy::render::view::screenshot::Screenshot::primary_window()).observe(bevy::render::view::screenshot::save_to_disk(path));
}

#[cfg(test)]
mod tests {
    #[test]
    fn clips_load() {
        let c = super::phone_clips();
        for a in [&c.raise, &c.press, &c.put_away] {
            let a = a.as_ref().expect("clip loads");
            assert!(a.duration() > 0.3 && a.bones.iter().any(|b| b.name == "j_wrist_ri"));
        }
    }
}

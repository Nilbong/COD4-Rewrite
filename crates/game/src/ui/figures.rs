//! Characters in 3D for the menus' previews ([`super::preview`]): CoD4's
//! and Black Ops' soldiers ([`crate::characters`]), their models loaded from
//! their own zone (with any zones whose materials they use) in the
//! background, normal-mapped, standing with a gun in hand.
//!
//! A preview spec `char:<id>` asks for one, optionally with
//! `|gun:<weapon:attachment+attachment>` and `|camo:<n>` for the gun in
//! hand (otherwise a plain AK-47).

use crate::characters::{self, Character, Game};
use crate::content::Content;
use crate::gunmodel::{CamoCache, GunAssets, GunTarget};
use crate::models::{AnimPlayer, Skeleton, SpawnModel, spawn_model};
use crate::wardrobe::{Dressing, Source};
use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;
use iw3::iwd::Vfs;
use std::sync::Arc;
use std::thread::JoinHandle;

/// Zones kept loaded; more are dropped, least recently used first.
const KEEP_ZONES: usize = 3;
/// The idle pose: the third-person models' own standing animation.
const POSE: &str = "pb_stand_alert";
/// The figure's height to frame, in CoD units, and how far up its middle is.
const FRAME_HEIGHT: f32 = 88.0;
const MIDDLE: f32 = 38.0;

pub struct FigureSpec<'a> {
    pub character: &'static Character,
    pub gun: &'a str,
    pub camo: usize,
}

/// `char:<id>|gun:<spec>|camo:<n>`.
pub fn parse(spec: &str) -> Option<FigureSpec<'_>> {
    let mut parts = spec.strip_prefix("char:")?.split('|');
    let character = characters::character(parts.next()?)?;
    let mut out = FigureSpec { character, gun: "ak47:", camo: 0 };
    for part in parts {
        if let Some(g) = part.strip_prefix("gun:") {
            out.gun = g;
        } else if let Some(c) = part.strip_prefix("camo:") {
            out.camo = c.parse().unwrap_or(0);
        }
    }
    Some(out)
}

enum ZoneState {
    Loading(JoinHandle<anyhow::Result<crate::wardrobe::load::Loaded>>),
    Ready(Box<(Content, Dressing)>),
    Failed,
}

/// The zones loaded for characters, most recently used last.
#[derive(Default)]
pub struct Figures {
    zones: Vec<(Source, ZoneState)>,
}

impl Figures {
    /// Are the zones for `spec` loaded (or failed)? Starts loading them if
    /// not.
    pub fn ready(&mut self, spec: &str) -> bool {
        let Some(f) = parse(spec) else { return true };
        let src = Source::of(f.character);
        match self.zones.iter().position(|(z, _)| *z == src) {
            Some(i) => {
                let entry = self.zones.remove(i);
                self.zones.push(entry);
            }
            None => {
                self.zones.push((src.clone(), ZoneState::Loading(crate::wardrobe::load(src))));
                // Drop the least recently used, but never one still loading.
                while self.zones.len() > KEEP_ZONES {
                    match self.zones.iter().position(|(_, s)| !matches!(s, ZoneState::Loading(_))) {
                        Some(i) => drop(self.zones.remove(i)),
                        None => break,
                    }
                }
            }
        }
        let (_, state) = self.zones.last_mut().expect("just pushed");
        if let ZoneState::Loading(task) = state {
            if !task.is_finished() {
                return false;
            }
        }
        true
    }

    /// Is a zone still loading?
    pub fn busy(&self) -> bool {
        self.zones.iter().any(|(_, s)| matches!(s, ZoneState::Loading(_)))
    }

}

/// The zones' content once loaded; CoD4's textures come from `vfs`.
fn content<'a>(zones: &'a mut [(Source, ZoneState)], src: &Source, vfs: &Arc<Vfs>) -> Option<&'a mut (Content, Dressing)> {
    let (_, state) = zones.iter_mut().find(|(z, _)| z == src)?;
    if let ZoneState::Loading(task) = state {
        if !task.is_finished() {
            return None;
        }
        let ZoneState::Loading(task) = std::mem::replace(state, ZoneState::Failed) else { unreachable!() };
        match task.join() {
            Ok(Ok((zones, own_vfs))) => {
                *state = ZoneState::Ready(Box::new((Content::new(zones, own_vfs.unwrap_or_else(|| vfs.clone())), Dressing::default())));
            }
            Ok(Err(e)) => warn!("ui: characters from {} unavailable: {e:#}", src.zone),
            Err(_) => warn!("ui: loading {} panicked", src.zone),
        }
    }
    match state {
        ZoneState::Ready(c) => Some(c),
        _ => None,
    }
}

/// Spawn the figure `spec` under `pivot` on render layer `layer`, its middle
/// on the pivot and facing the camera. `common` (common_mp) has the pose and
/// the guns, `other_game` Black Ops' or World at War's guns. Returns the
/// figure and a camera distance that frames it.
#[allow(clippy::too_many_arguments)]
pub fn spawn(
    commands: &mut Commands,
    figures: &mut Figures,
    common: &mut Content,
    other_game: Option<&mut Content>,
    camos: &mut CamoCache,
    a: &mut GunAssets,
    spec: &str,
    layer: usize,
    pivot: Entity,
    fov: f32,
) -> Option<(Entity, f32)> {
    let f = parse(spec)?;
    let (content, dressing) = content(&mut figures.zones, &Source::of(f.character), &common.vfs.clone())?;
    let layers = RenderLayers::layer(layer);
    let turn = Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2);
    let owner = commands
        .spawn((
            Name::new(spec.to_owned()),
            Transform::from_rotation(turn).with_translation(Vec3::NEG_Y * crate::units::u(MIDDLE)),
            Visibility::default(),
            layers.clone(),
            ChildOf(pivot),
        ))
        .id();
    let mut skeleton = Skeleton::default();
    let mut any = false;
    for name in f.character.models {
        if let Some(m) = dressing.model(content, name, &mut *a.meshes, &mut *a.materials, &mut *a.images, &mut *a.bindposes) {
            let spec = SpawnModel { model: &m, owner, attach_to: None, layers: Some(layers.clone()), shadows: false };
            spawn_model(commands, &mut skeleton, spec);
            any = true;
        }
    }
    if !any {
        commands.entity(owner).despawn();
        return None;
    }
    let hand = skeleton.joint("tag_weapon_right");
    let target = GunTarget { owner, attach_to: hand, layers: Some(layers) };
    // A Black Ops or World at War gun comes from its game's content.
    let weapon = crate::gunmodel::parse(f.gun).0;
    let gun_content: &mut Content = match other_game {
        Some(c) if crate::bo1::is_bo1(weapon) || crate::waw::is_waw(weapon) || crate::mw2guns::is_mw2(weapon) => c,
        _ => &mut *common,
    };
    crate::gunmodel::spawn_world_gun(commands, gun_content, camos, a, &mut skeleton, f.gun, f.camo, target);
    let mut player = AnimPlayer::default();
    let pose = match f.character.game {
        Game::BlackOps => crate::wardrobe::black_ops_anim(POSE).or_else(|| common.anim(POSE)),
        Game::Cod4 => common.anim(POSE),
    };
    if let Some(pose) = pose {
        player.play(pose, 0.0);
    }
    commands.entity(owner).insert((skeleton, player));
    let distance = crate::units::u(FRAME_HEIGHT * 0.5) / (fov * 0.5).tan();
    Some((owner, distance))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs() {
        let f = parse("char:price|gun:m4:reflex+silencer|camo:3").unwrap();
        assert_eq!((f.character.id, f.gun, f.camo), ("price", "m4:reflex+silencer", 3));
        let f = parse("char:gaz").unwrap();
        assert_eq!((f.gun, f.camo), ("ak47:", 0));
        assert!(parse("char:nobody").is_none());
        assert!(parse("ak47:").is_none());
    }
}

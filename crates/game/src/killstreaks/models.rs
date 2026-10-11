//! Models for Modern Warfare 2's kill streaks: MW2's own where its install
//! is there ([`crate::mw2guns::MatchContent`] holds its `common_mp`, with the
//! sentry gun and the Little Bird, and the team crates from one of its maps),
//! else the stand-ins CoD4's zones have.

use crate::content::{Content, PreparedModel};
use crate::models::{Skeleton, SpawnModel, spawn_model};
use bevy::ecs::system::SystemParam;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;
use std::sync::Arc;

#[derive(SystemParam)]
pub(super) struct StreakModels<'w> {
    content: Option<ResMut<'w, Content>>,
    mw2: Option<ResMut<'w, crate::mw2guns::MatchContent>>,
    meshes: ResMut<'w, Assets<Mesh>>,
    materials: ResMut<'w, Assets<StandardMaterial>>,
    images: ResMut<'w, Assets<Image>>,
    bindposes: ResMut<'w, Assets<SkinnedMeshInverseBindposes>>,
}

/// A model spawned under its owner: which one, its joints, and its
/// bounds (Bevy, metres, the model's space).
pub(super) struct Spawned {
    pub name: String,
    pub skeleton: Skeleton,
    pub bounds: (Vec3, Vec3),
    /// Its surfaces' entities.
    pub surfaces: Vec<Entity>,
}

impl StreakModels<'_> {
    /// The first of `names` found (MW2's content, then the match's), ready
    /// to draw.
    fn prepare(&mut self, names: &[&str]) -> Option<Arc<PreparedModel>> {
        let mw2 = self.mw2.as_deref_mut().and_then(|m| m.get());
        for content in [mw2, self.content.as_deref_mut()].into_iter().flatten() {
            for name in names {
                if content.find(name).is_none() {
                    continue;
                }
                // (One whose surfaces live in a zone not loaded draws nothing.)
                if let Some(m) = content.model(name, &mut self.meshes, &mut self.materials, &mut self.images, &mut self.bindposes).filter(|m| !m.surfaces.is_empty()) {
                    return Some(m);
                }
            }
        }
        None
    }

    /// The first of `names` there is, as `owner`'s model (CoD's x forward
    /// along the owner's +X).
    pub fn spawn(&mut self, commands: &mut Commands, names: &[&str], owner: Entity, shadows: bool) -> Option<Spawned> {
        let m = self.prepare(names)?;
        let mut skeleton = Skeleton::default();
        let surfaces = spawn_model(commands, &mut skeleton, SpawnModel { model: &m, owner, attach_to: None, layers: None, shadows });
        Some(Spawned { name: m.name.clone(), skeleton, bounds: m.bounds, surfaces })
    }

    /// A see-through, unlit colour for a ghost (a sentry being placed).
    pub fn ghost_material(&mut self, color: Color) -> Handle<StandardMaterial> {
        self.materials.add(StandardMaterial { base_color: color, unlit: true, alpha_mode: AlphaMode::Blend, ..default() })
    }
}

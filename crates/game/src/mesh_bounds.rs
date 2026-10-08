//! Bounds for the game's own meshes, kept from when they were built.
//!
//! Models' meshes go to the GPU only (`RenderAssetUsages::RENDER_WORLD`):
//! once there, their vertices can't be read back. Bevy works out a mesh
//! entity's bounds from its vertices when it appears and again whenever its
//! mesh changes, which then panics (a dropped gun, a body put on, a model
//! spawned a second time). So meshes made through [`add`] have their
//! bounds noted first, and [`MeshBoundsPlugin`] gives entities showing them
//! those bounds itself, telling Bevy not to work them out ([`NoAutoAabb`]).

use bevy::camera::primitives::{Aabb, MeshAabb};
use bevy::camera::visibility::{NoAutoAabb, NoFrustumCulling, VisibilitySystems};
use bevy::prelude::*;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

static BOUNDS: LazyLock<Mutex<HashMap<AssetId<Mesh>, Aabb>>> = LazyLock::new(Default::default);

/// Add `mesh`, noting its bounds while its vertices are still here.
pub fn add(meshes: &mut Assets<Mesh>, mesh: Mesh) -> Handle<Mesh> {
    let aabb = mesh.compute_aabb();
    let handle = meshes.add(mesh);
    if let Some(aabb) = aabb {
        BOUNDS.lock().unwrap_or_else(|e| e.into_inner()).insert(handle.id(), aabb);
    }
    handle
}

pub struct MeshBoundsPlugin;

impl Plugin for MeshBoundsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostUpdate, known_bounds.before(VisibilitySystems::CalculateBounds));
    }
}

#[allow(clippy::type_complexity)]
fn known_bounds(
    mut commands: Commands,
    mut changed: Query<(Entity, &Mesh3d, Option<&mut Aabb>, Has<NoAutoAabb>), (Changed<Mesh3d>, Without<NoFrustumCulling>)>,
) {
    if changed.is_empty() {
        return;
    }
    let bounds = BOUNDS.lock().unwrap_or_else(|e| e.into_inner());
    for (e, mesh, aabb, manual) in &mut changed {
        let Some(known) = bounds.get(&mesh.id()) else { continue };
        match aabb {
            Some(mut aabb) => *aabb = *known,
            None => {
                commands.entity(e).try_insert(*known);
            }
        }
        if !manual {
            commands.entity(e).try_insert(NoAutoAabb);
        }
    }
}

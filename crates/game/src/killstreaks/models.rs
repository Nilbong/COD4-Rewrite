//! Models for the care package and the sentry gun, all CoD4's: the match's
//! own zones first (`common_mp` has the helicopters and the M60), then a few
//! props from another CoD4 zone ([`PROPS_ZONE`]: the plastic case and a
//! tripod), loaded on a thread when the match starts if the map lacks them,
//! made ready to draw and the zone dropped (as
//! [`crate::wardrobe`] does with characters' zones).

use crate::content::{Content, PreparedModel};
use crate::models::{Skeleton, SpawnModel, spawn_model};
use bevy::ecs::system::SystemParam;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;
use std::thread::JoinHandle;

/// The CoD4 zone the props come from, and the props.
/// (Single-player's Killhouse, which the Headquarters loads too.)
pub(super) const PROPS_ZONE: &str = "killhouse";
pub(super) const PROPS: &[&str] = &[super::carepackage::CRATE_MODEL, super::sentry::TRIPOD];

pub(super) fn build(app: &mut App) {
    app.init_resource::<StreakProps>()
        .add_systems(OnEnter(crate::state::GameState::InGame), start.after(crate::world::load_map).in_set(crate::state::Setup::Content))
        .add_systems(Update, finish.run_if(crate::state::in_game));
}

/// The props from [`PROPS_ZONE`]: loading, or ready by name.
#[derive(Resource, Default)]
pub(crate) struct StreakProps {
    loading: Option<JoinHandle<anyhow::Result<iw3::zone::Zone>>>,
    ready: HashMap<String, Arc<PreparedModel>>,
}

impl StreakProps {
    /// Still loading.
    pub fn busy(&self) -> bool {
        self.loading.is_some()
    }
}

/// Load the props' zone unless the map has them all (or they're ready
/// already, from an earlier match).
fn start(content: Res<Content>, mut props: ResMut<StreakProps>) {
    let missing = PROPS.iter().any(|n| content.find(n).is_none() && !props.ready.contains_key(*n));
    if !missing || props.loading.is_some() {
        return;
    }
    props.loading = Some(std::thread::spawn(|| {
        let install = iw3::Install::locate()?;
        Ok(iw3::zone::Zone::parse(&iw3::fastfile::load(&install.zone_path(PROPS_ZONE))?, iw3::zone::ParseOptions::default())?)
    }));
}

/// Once loaded: the props made ready to draw, the zone dropped.
fn finish(
    mut props: ResMut<StreakProps>,
    content: Option<Res<Content>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut bindposes: ResMut<Assets<SkinnedMeshInverseBindposes>>,
) {
    if !props.loading.as_ref().is_some_and(|t| t.is_finished()) {
        return;
    }
    let Some(task) = props.loading.take() else { return };
    let zone = match task.join() {
        Ok(Ok(z)) => z,
        Ok(Err(e)) => return warn!("kill streaks: no props from {PROPS_ZONE}: {e:#}"),
        Err(_) => return warn!("kill streaks: loading {PROPS_ZONE} panicked"),
    };
    let Some(main) = content else { return };
    let mut zone_content = Content::new(vec![zone], main.vfs.clone());
    for name in PROPS {
        match zone_content.model(name, &mut meshes, &mut materials, &mut images, &mut bindposes).filter(|m| !m.surfaces.is_empty()) {
            Some(m) => {
                props.ready.insert((*name).to_owned(), m);
            }
            None => warn!("kill streaks: {name} not in {PROPS_ZONE}"),
        }
    }
    info!("kill streaks: props from {PROPS_ZONE}: {:?}", props.ready.keys().collect::<Vec<_>>());
}

#[derive(SystemParam)]
pub(crate) struct StreakModels<'w> {
    content: Option<ResMut<'w, Content>>,
    props: Res<'w, StreakProps>,
    meshes: ResMut<'w, Assets<Mesh>>,
    materials: ResMut<'w, Assets<StandardMaterial>>,
    images: ResMut<'w, Assets<Image>>,
    bindposes: ResMut<'w, Assets<SkinnedMeshInverseBindposes>>,
}

/// A model spawned under its owner: which one, its joints, and its
/// bounds (Bevy, metres, the model's space).
pub(crate) struct Spawned {
    pub name: String,
    pub skeleton: Skeleton,
    pub bounds: (Vec3, Vec3),
    /// Its surfaces' entities.
    pub surfaces: Vec<Entity>,
}

impl StreakModels<'_> {
    /// The first of `names` found (the match's zones, then the props),
    /// ready to draw.
    fn prepare(&mut self, names: &[&str]) -> Option<Arc<PreparedModel>> {
        for name in names {
            if let Some(content) = self.content.as_deref_mut().filter(|c| c.find(name).is_some()) {
                // (One whose surfaces live in a zone not loaded draws nothing.)
                if let Some(m) = content.model(name, &mut self.meshes, &mut self.materials, &mut self.images, &mut self.bindposes).filter(|m| !m.surfaces.is_empty()) {
                    return Some(m);
                }
            }
            if let Some(m) = self.props.ready.get(*name) {
                return Some(m.clone());
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

    /// The match's content (systems with these can't also take `Res<Content>`).
    pub fn content(&self) -> Option<&Content> {
        self.content.as_deref()
    }

    /// A plain box under `owner`, `half` its size about `centre` (metres):
    /// a stand-in when no model is there.
    pub fn spawn_box(&mut self, commands: &mut Commands, owner: Entity, centre: Vec3, half: Vec3, color: Color) {
        let mesh = crate::mesh_bounds::add(&mut self.meshes, Mesh::from(Cuboid::new(half.x * 2.0, half.y * 2.0, half.z * 2.0)));
        let material = self.materials.add(StandardMaterial { base_color: color, perceptual_roughness: 0.8, ..default() });
        commands.spawn((Mesh3d(mesh), MeshMaterial3d(material), Transform::from_translation(centre), ChildOf(owner)));
    }

    /// A see-through, unlit colour for a ghost (a sentry being placed).
    pub fn ghost_material(&mut self, color: Color) -> Handle<StandardMaterial> {
        self.materials.add(StandardMaterial { base_color: color, unlit: true, alpha_mode: AlphaMode::Blend, ..default() })
    }
}

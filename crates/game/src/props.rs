//! The map's props that are entities rather than static models
//! (`script_model`): the destructible cars and glass, leaking barrels,
//! shootable pipes, swaying palms, security cameras, lightboxes. The map's
//! own clip and mantle brushes around them are already solid
//! ([`crate::collision`]); without these they'd stand there invisible.
//!
//! Left out, as CoD4's scripts leave them out: game-mode objects
//! (`script_gameobjectname`: bomb sites, flags, HQ radios, Domination's
//! barriers; the modes draw their own), Old School's perk pickups and
//! `exploder` models (what a bomb site looks like once blown).
//!
//! The map's clutter (its dynamic entities) is [`crate::clutter`]'s.

use crate::content::Content;
use crate::models::{Skeleton, SpawnModel, spawn_model};
use crate::state::{GameState, Setup};
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;

pub struct PropsPlugin;

impl Plugin for PropsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(GameState::InGame), spawn_props.in_set(Setup::Spawn)).add_plugins((crate::clutter::ClutterPlugin, crate::terrain::TerrainPlugin, crate::mapfx::MapFxPlugin));
    }
}

/// A map prop ([`spawn_props`]), by its model.
#[derive(Component, Clone, Debug)]
pub struct MapProp {
    pub model: String,
}

/// Is this map entity a prop that's always there?
fn is_prop(e: &iw3::ents::Entity) -> bool {
    // (Not `misc_turret`: CoD4's multiplayer zones don't ship the mounted
    // guns' models, Downpour's two SAWs included; the game deletes them.)
    e.classname() == "script_model"
        // (`fx` is the editor's marker for an effect's origin, which the
        // single-player levels leave in as `script_model`s.)
        && e.get("model").is_some_and(|m| !m.is_empty() && !m.starts_with('*') && m != "fx")
        && e.get("script_gameobjectname").is_none()
        && !matches!(e.get("targetname"), Some("oldschool_pickup" | "exploder"))
}

fn spawn_props(
    mut commands: Commands,
    mut content: ResMut<Content>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut bindposes: ResMut<Assets<SkinnedMeshInverseBindposes>>,
) {
    let ents = content.map().map_ents().map(|m| iw3::ents::parse(&m.entity_string)).unwrap_or_default();
    let root = commands.spawn((Name::new("map props"), Transform::default(), Visibility::default())).id();
    let (mut drawn, mut missing) = (0, 0);
    // Nothing moves them, so they're drawn as the static models are: their
    // surfaces merged per material and 32 m cell. Spawned one by one as
    // skinned models (a skeleton each), District's 1,900 surfaces cost the
    // render thread more than the rest of the map.
    let mut batches: std::collections::HashMap<(Handle<StandardMaterial>, IVec3), Vec<(Handle<Mesh>, Transform)>> = Default::default();
    for e in ents.iter().filter(|e| is_prop(e)) {
        let (Some(origin), Some(name)) = (e.origin(), e.get("model")) else { continue };
        let scale = e.get("modelscale").and_then(|s| s.parse::<f32>().ok()).unwrap_or(1.0);
        let tf = Transform::from_translation(crate::units::pos(origin))
            .with_rotation(crate::modes::koth::cod_rotation(e.angles()))
            .with_scale(Vec3::splat(scale));
        if std::env::var_os("COD4RW_SKINNEDPROPS").is_none() {
            if let Some(parts) = static_parts(&mut content, name, &mut meshes, &mut materials, &mut images) {
                let cell = (tf.translation / 32.0).floor().as_ivec3();
                for (material, mesh) in parts {
                    batches.entry((material, cell)).or_default().push((mesh, tf));
                }
                drawn += 1;
                continue;
            }
        }
        // Debug aid (`COD4RW_SKINNEDPROPS`), or a model without static
        // surfaces: as before.
        let Some(m) = content.model(name, &mut meshes, &mut materials, &mut images, &mut bindposes) else {
            missing += 1;
            continue;
        };
        let owner = commands.spawn((Name::new(format!("prop {name}")), MapProp { model: name.to_owned() }, tf, Visibility::default(), ChildOf(root))).id();
        spawn_model(&mut commands, &mut Skeleton::default(), SpawnModel { model: &m, owner, attach_to: None, layers: None, shadows: true });
        drawn += 1;
    }
    let mut draws = 0;
    for ((material, _), parts) in batches {
        match crate::world::merge_meshes(&parts, &meshes) {
            Some(mesh) => {
                commands.spawn((Name::new("map props batch"), Mesh3d(crate::mesh_bounds::add(&mut meshes, mesh)), MeshMaterial3d(material), Transform::default(), ChildOf(root)));
                draws += 1;
            }
            // Meshes already handed to the renderer (shared with the static
            // models) can't be read to merge: each its own, still unskinned.
            None => {
                for (mesh, tf) in parts {
                    commands.spawn((Name::new("map prop"), Mesh3d(mesh), MeshMaterial3d(material.clone()), tf, ChildOf(root)));
                    draws += 1;
                }
            }
        }
    }
    info!("props: {drawn} map props ({draws} batches){}", if missing > 0 { format!(", {missing} models not found") } else { String::new() });
}

/// A model's first LOD as (material, unskinned mesh) per surface; `None` if
/// it has none.
pub(crate) fn static_parts(
    content: &mut Content,
    name: &str,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
) -> Option<Vec<(Handle<StandardMaterial>, Handle<Mesh>)>> {
    let (zi, id) = content.find(name)?;
    let (zi, id) = content.resolve_xmodel(zi, id);
    let xm = content.zone(zi).xmodel(id)?;
    let lod = xm.lods.first().copied()?;
    let mats = xm.materials.clone();
    let mut out = Vec::new();
    for surf in lod.surf_index as usize..(lod.surf_index + lod.num_surfs) as usize {
        let Some(mat_id) = mats.get(surf).copied().flatten() else { continue };
        let Some(mat) = content.material(zi, mat_id, materials, images) else { continue };
        let Some(mesh) = content.static_mesh(zi, id, surf, meshes) else { continue };
        out.push((mat.handle, mesh));
    }
    (!out.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ent(kv: &[(&str, &str)]) -> iw3::ents::Entity {
        iw3::ents::Entity { fields: kv.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() }
    }

    #[test]
    fn props_but_not_game_objects() {
        let car = ent(&[("classname", "script_model"), ("model", "vehicle_80s_sedan1_yel_destructible_mp"), ("targetname", "destructible")]);
        let pipe = ent(&[("classname", "script_model"), ("model", "com_pipe_4x256_metal"), ("targetname", "pipe_shootable")]);
        let flag = ent(&[("classname", "script_model"), ("model", "prop_flag_neutral"), ("script_gameobjectname", "dom")]);
        let pickup = ent(&[("classname", "script_model"), ("model", "perc_martyrdom"), ("targetname", "oldschool_pickup")]);
        let blown = ent(&[("classname", "script_model"), ("model", "com_bomb_objective_d"), ("targetname", "exploder")]);
        assert!(is_prop(&car) && is_prop(&pipe));
        assert!(!is_prop(&flag) && !is_prop(&pickup) && !is_prop(&blown));
    }
}

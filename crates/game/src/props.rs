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
        // (A campaign's objective shimmer models are the scripts' to show,
        // `crate::campaign::stage`.)
        // (`crate::campaign::stage::scripted_prop`: named ones, and the
        // objective shimmer.)
        && !(crate::campaign::ACTIVE.load(std::sync::atomic::Ordering::Relaxed)
            && (e.get("targetname").is_some_and(|t| !t.is_empty()) || e.get("model").is_some_and(|m| m.to_ascii_lowercase().ends_with("_obj"))))
}

fn spawn_props(
    mut commands: Commands,
    mut content: ResMut<Content>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut bindposes: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    mut world_materials: ResMut<Assets<crate::world::WorldMaterial>>,
    probes: Option<Res<crate::world::ReflectionProbes>>,
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
        if crate::map_editor::layout::hidden(name, origin) {
            continue;
        }
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
    // The showcase wets them and has them reflect their nearest probe
    // (box-corrected, `crate::probe_parallax`), as the static models.
    // Off unless `COD4RW_WETPROPS=1`: no visible difference on Wet Work's
    // 91 props in an A/B (2026-10-14), and Downpour has none.
    let showcase = crate::atmos::climate::showcase() && std::env::var("COD4RW_WETPROPS").is_ok_and(|v| v == "1");
    let sky = showcase.then(|| content.sky_cube(&mut images)).flatten();
    let mut wet: std::collections::HashMap<(AssetId<StandardMaterial>, Option<usize>), Handle<crate::world::WorldMaterial>> = Default::default();
    let mut props: std::collections::HashMap<(AssetId<StandardMaterial>, Option<usize>), Handle<crate::world::WorldMaterial>> = Default::default();
    let prop_sky = crate::world::prop_materials().then(|| content.sky_cube(&mut images)).flatten();
    for ((material, _), parts) in batches {
        let key = (!showcase && crate::world::prop_materials()).then(|| content.material_key(material.id())).flatten();
        let shown = if let Some((zi, id)) = key {
            // CoD4's normal and specular maps, as the static models'
            // (`crate::world::prop_material`).
            let middle = parts.iter().map(|(_, tf)| tf.translation).sum::<Vec3>() / parts.len().max(1) as f32;
            let probe = probes.as_ref().and_then(|p| {
                p.0.iter().enumerate().min_by(|a, b| a.1.0.distance_squared(middle).total_cmp(&b.1.0.distance_squared(middle)))
            });
            let m = match props.get(&(material.id(), probe.map(|(i, _)| i))) {
                Some(m) => m.clone(),
                None => {
                    let maps = content.model_maps(zi, id, &mut images);
                    let base = materials.get(&material).cloned().unwrap_or_default();
                    let bits = content.material_surface_bits(zi, id);
                    let m = crate::world::prop_material(&base, maps, bits, probe.map(|(_, p)| p), prop_sky.clone(), &mut world_materials);
                    props.insert((material.id(), probe.map(|(i, _)| i)), m.clone());
                    m
                }
            };
            Shown::World(m)
        } else if showcase {
            let middle = parts.iter().map(|(_, tf)| tf.translation).sum::<Vec3>() / parts.len().max(1) as f32;
            let probe = probes.as_ref().and_then(|p| {
                p.0.iter().enumerate().min_by(|a, b| a.1.0.distance_squared(middle).total_cmp(&b.1.0.distance_squared(middle)))
            });
            let m = wet
                .entry((material.id(), probe.map(|(i, _)| i)))
                .or_insert_with(|| crate::wet::wet_material(materials.get(&material), sky.clone(), probe.map(|(_, p)| p), &mut world_materials))
                .clone();
            Shown::World(m)
        } else {
            Shown::Standard(material)
        };
        match crate::world::merge_meshes(&parts, &meshes) {
            Some(mesh) => {
                let mut e = commands.spawn((Name::new("map props batch"), Mesh3d(crate::mesh_bounds::add(&mut meshes, mesh)), Transform::default(), ChildOf(root)));
                shown.insert(&mut e);
                draws += 1;
            }
            // Meshes already handed to the renderer (shared with the static
            // models) can't be read to merge: each its own, still unskinned.
            None => {
                for (mesh, tf) in parts {
                    let mut e = commands.spawn((Name::new("map prop"), Mesh3d(mesh), tf, ChildOf(root)));
                    shown.insert(&mut e);
                    draws += 1;
                }
            }
        }
    }
    info!("props: {drawn} map props ({draws} batches){}", if missing > 0 { format!(", {missing} models not found") } else { String::new() });
}

/// A props batch's material: as the model has it, or the showcase's.
enum Shown {
    Standard(Handle<StandardMaterial>),
    World(Handle<crate::world::WorldMaterial>),
}

impl Shown {
    fn insert(&self, e: &mut EntityCommands) {
        match self {
            Shown::Standard(m) => e.insert(MeshMaterial3d(m.clone())),
            Shown::World(m) => e.insert(MeshMaterial3d(m.clone())),
        };
    }
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

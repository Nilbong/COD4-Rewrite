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
    e.classname() == "script_model"
        && e.get("model").is_some_and(|m| !m.is_empty() && !m.starts_with('*'))
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
    for e in ents.iter().filter(|e| is_prop(e)) {
        let (Some(origin), Some(name)) = (e.origin(), e.get("model")) else { continue };
        let scale = e.get("modelscale").and_then(|s| s.parse::<f32>().ok()).unwrap_or(1.0);
        let tf = Transform::from_translation(crate::units::pos(origin))
            .with_rotation(crate::modes::koth::cod_rotation(e.angles()))
            .with_scale(Vec3::splat(scale));
        let Some(m) = content.model(name, &mut meshes, &mut materials, &mut images, &mut bindposes) else {
            missing += 1;
            continue;
        };
        let owner = commands.spawn((Name::new(format!("prop {name}")), MapProp { model: name.to_owned() }, tf, Visibility::default(), ChildOf(root))).id();
        spawn_model(&mut commands, &mut Skeleton::default(), SpawnModel { model: &m, owner, attach_to: None, layers: None, shadows: true });
        drawn += 1;
    }
    info!("props: {drawn} map props{}", if missing > 0 { format!(", {missing} models not found") } else { String::new() });
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

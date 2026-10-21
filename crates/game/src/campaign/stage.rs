//! What the level's scripts animate, on screen: every script entity with
//! a model that the map itself doesn't draw (spawned `script_model`s, the
//! scripted actors), skinned and posed as the [`gsc::level::Level`] has it
//! (its animation and progress, its place from root motion and links),
//! with the tags the scripts asked about read back from the joints; and
//! the player carried on a tag (`playerlinkto`) with the link's look arcs.

use bevy::prelude::*;
use gsc::level::{AnimInfo, AnimLib, EntId, Kind, Level, PLAYER};
use iw3::xanim::XAnim;
use std::collections::HashMap;
use std::sync::Arc;

/// The XAnims the scripts can name: common.ff's and the level's, decoded
/// on first use.
#[derive(Default)]
pub struct Anims {
    raw: HashMap<String, (iw3::zone::generic::GNode, usize)>,
    /// Each zone's script strings (by the index in `raw`).
    strings: Vec<Vec<String>>,
    decoded: HashMap<String, Option<Arc<XAnim>>>,
}

impl Anims {
    /// Take a zone's animations (the level's over common's).
    pub fn add_zone(&mut self, zone: &iw3::zone::Zone) {
        let zi = self.strings.len();
        self.strings.push(zone.script_strings.clone());
        for a in &zone.assets {
            if let iw3::zone::Asset::Generic(g) = a
                && g.ty == iw3::zone::AssetType::XAnimParts
                && !g.name.starts_with(',')
            {
                self.raw.insert(g.name.to_ascii_lowercase(), (g.root.clone(), zi));
            }
        }
    }

    pub fn get(&mut self, name: &str) -> Option<Arc<XAnim>> {
        let key = name.to_ascii_lowercase();
        if let Some(a) = self.decoded.get(&key) {
            return a.clone();
        }
        let a = self.raw.get(&key).and_then(|(node, zi)| XAnim::from_node(name, node, &self.strings[*zi]).ok()).map(Arc::new);
        self.decoded.insert(key, a.clone());
        a
    }
}

/// The scripts' view of [`Anims`] (shared with the stage).
pub struct ScriptAnims(pub std::rc::Rc<std::cell::RefCell<Anims>>);

impl AnimLib for ScriptAnims {
    fn info(&mut self, name: &str) -> Option<AnimInfo> {
        let a = self.0.borrow_mut().get(name)?;
        Some(AnimInfo {
            length: a.duration().max(1.0 / 30.0),
            looping: a.looping,
            delta: a.delta,
            notes: a.notifies.iter().map(|n| (n.time, n.name.clone())).collect(),
            partial: !a.bones.iter().any(|b| b.name.eq_ignore_ascii_case("j_mainroot")),
        })
    }

    fn delta(&mut self, name: &str, frac: f32) -> ([f32; 3], f32) {
        match self.0.borrow_mut().get(name) {
            Some(a) => (a.delta_at(frac), a.delta_yaw_at(frac).to_degrees()),
            None => ([0.0; 3], 0.0),
        }
    }
}

/// A script entity drawn ([`sync`]).
#[derive(Component)]
pub struct StageEnt {
    pub id: EntId,
    /// What it was built from (rebuilt when that changes).
    models: Vec<String>,
}

/// Script entities shown, by id.
#[derive(Default)]
pub struct Stage {
    shown: HashMap<EntId, Entity>,
}

impl Stage {
    pub fn count(&self) -> usize {
        self.shown.len()
    }

    /// What shows a script entity.
    pub fn entity(&self, id: EntId) -> Option<Entity> {
        self.shown.get(&id).copied()
    }
}

/// Should the stage draw this entity? (The map's own `script_model`s are
/// drawn by `crate::props`; AI with pawns by `crate::thirdperson`.)
fn staged(e: &gsc::level::Ent) -> bool {
    !e.model.is_empty()
        && !e.model.starts_with('*')
        && e.model != "fx"
        && e.model != "tag_origin"
        // (And the levels' mounted guns: their models are in SP's zones.)
        && (matches!(e.kind, Kind::Spawned | Kind::Ai | Kind::Vehicle)
            || (e.kind == Kind::Map && e.classname == "misc_turret")
            // (And the map's pickups and their objective shimmer, which the
            // scripts move, hide, show and delete.)
            || (e.kind == Kind::Map && (e.classname.starts_with("weapon_") || (e.classname == "script_model" && scripted_prop(e)))))
}

/// A map `script_model` the scripts can reach (by its `targetname`, or an
/// objective shimmer `_obj`): drawn from the level's state (moved, hidden,
/// deleted), not as a fixed prop. Must agree with `crate::props::is_prop`.
pub fn scripted_prop(e: &gsc::level::Ent) -> bool {
    e.get("targetname").is_some_and(|t| !t.is_empty()) || e.model.to_ascii_lowercase().ends_with("_obj")
}

/// Make, move, pose and remove what the scripts show; read back their tags.
#[allow(clippy::too_many_arguments)]
pub fn sync(
    commands: &mut Commands,
    level: &mut Level,
    stage: &mut Stage,
    anims: &mut Anims,
    content: &mut crate::content::Content,
    assets: &mut ModelAssets,
    figures: &mut Query<(&StageEnt, &mut Transform, &mut Visibility, &crate::models::Skeleton, &mut crate::models::AnimPlayer), Without<crate::player::LocalPlayer>>,
    globals: &Query<&GlobalTransform>,
    weapons: &HashMap<EntId, String>,
    now: f64,
) {
    // Gone.
    let live: std::collections::HashSet<EntId> = level.ents.iter().filter(|e| staged(e)).map(|e| e.id).collect();
    stage.shown.retain(|id, e| {
        let keep = live.contains(id);
        if !keep {
            commands.entity(*e).try_despawn();
        }
        keep
    });
    for e in level.ents.iter_mut().filter(|e| staged(e)) {
        let mut models = vec![e.model.clone()];
        models.extend(e.anim_state.attached.iter().map(|(m, t)| format!("{m}@{t}")));
        // An actor's gun in its right hand.
        if let Some(w) = weapons.get(&e.id) {
            models.push(format!("{w}@tag_weapon_right"));
        }
        let fresh = match stage.shown.get(&e.id).and_then(|&b| figures.get(b).ok()) {
            Some((s, ..)) => s.models != models,
            None => true,
        };
        if fresh {
            if let Some(old) = stage.shown.remove(&e.id) {
                commands.entity(old).try_despawn();
            }
            if let Some(b) = build(commands, content, assets, e.id, &models) {
                // An actor can be shot: CoD4-sized hitboxes on it.
                if e.kind == Kind::Ai && e.alive {
                    add_hitboxes(commands, b);
                }
                stage.shown.insert(e.id, b);
            }
            continue;
        }
        let Some(&b) = stage.shown.get(&e.id) else { continue };
        let Ok((_, mut tf, mut vis, skeleton, mut player)) = figures.get_mut(b) else { continue };
        tf.translation = crate::units::pos(e.origin);
        tf.rotation = crate::modes::koth::cod_rotation(e.angles);
        vis.set_if_neq(if e.hidden { Visibility::Hidden } else { Visibility::Inherited });
        if let Some(a) = &e.anim_state.anim
            && let Some(x) = anims.get(&a.name)
        {
            let t = a.frac(now) * x.duration();
            player.pose(x, t);
        }
        // The overlay over it, posed at its progress.
        // (Debug: `COD4RW_STAGE_NO_OVERLAY` leaves it off.)
        player.overlay = e.anim_state.overlay.as_ref().filter(|_| std::env::var_os("COD4RW_STAGE_NO_OVERLAY").is_none()).and_then(|o| Some((anims.get(&o.name)?, o.frac(now))));
        player.overlay_weight = if player.overlay.is_some() { 1.0 } else { 0.0 };
        // Debug: `COD4RW_STAGE_TRACE=<model part>`: where that entity is
        // drawn, and its tags' joints.
        if let Ok(want) = std::env::var("COD4RW_STAGE_TRACE")
            && e.model.contains(&want)
            && (now * 2.0).floor() != ((now - 0.02) * 2.0).floor()
        {
            let g = globals.get(b).map(|g| crate::units::to_cod(g.translation())).ok();
            let joints: Vec<(String, Option<[f32; 3]>)> = e
                .anim_state
                .wanted
                .iter()
                .map(|t| (t.clone(), skeleton.joint(t).and_then(|j| globals.get(j).ok()).map(|g| crate::units::to_cod(g.translation()).map(f32::round))))
                .collect();
            info!("stage: {} level {:?} drawn {:?} joints {joints:?}", e.model, e.origin.map(f32::round), g.map(|v| v.map(f32::round)));
        }
        // The tags the scripts want, where they were drawn last frame.
        // (Relative to the entity: its root as drawn then.)
        let Ok(root) = globals.get(b) else { continue };
        let inv = root.affine().inverse();
        for tag in e.anim_state.wanted.clone() {
            let Some(j) = skeleton.joint(&tag) else { continue };
            let Ok(g) = globals.get(j) else { continue };
            let local = inv.transform_point3(g.translation());
            let rel = root.rotation().inverse() * g.rotation();
            // Bevy's local point to the entity's CoD axes (x forward, y
            // left, z up): the same mapping as a world point about the origin.
            let o = crate::units::to_cod(local + crate::units::pos([0.0; 3]));
            e.anim_state.tags.insert(tag, (o, cod_angles(rel)));
        }
    }
}

/// An actor's hitboxes (head, chest, stomach, legs; CoD units up from its
/// feet), shot like a pawn's: their `Damage` goes to the scripts.
fn add_hitboxes(commands: &mut Commands, owner: Entity) {
    use crate::collision::Layer;
    use crate::combat::{HitLocation, Hitbox};
    use crate::units::u;
    use avian3d::prelude::{Collider, CollisionLayers, LayerMask};
    for (loc, collider, up) in [
        (HitLocation::Head, Collider::sphere(u(5.5)), 64.0),
        (HitLocation::Torso, Collider::cuboid(u(20.0), u(16.0), u(14.0)), 48.0),
        (HitLocation::TorsoLower, Collider::cuboid(u(18.0), u(10.0), u(13.0)), 35.0),
        (HitLocation::Legs, Collider::cuboid(u(16.0), u(30.0), u(12.0)), 15.0),
    ] {
        commands.spawn((Hitbox { owner, location: loc }, collider, CollisionLayers::new(Layer::Hitbox, LayerMask::NONE), Transform::from_translation(Vec3::Y * u(up)), StageHitbox, ChildOf(owner)));
    }
}

/// A staged actor's hitbox (taken off when it dies).
#[derive(Component)]
pub struct StageHitbox;

/// The script entity a drawn one shows.
impl Stage {
    pub fn id_of(&self, e: Entity) -> Option<EntId> {
        self.shown.iter().find(|(_, b)| **b == e).map(|(id, _)| *id)
    }
}

/// The assets models are made into.
pub struct ModelAssets<'a> {
    pub meshes: &'a mut Assets<Mesh>,
    pub materials: &'a mut Assets<StandardMaterial>,
    pub images: &'a mut Assets<Image>,
    pub bindposes: &'a mut Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>,
}

/// A script entity's body: its model and attachments (`model@tag`, on that
/// joint; heads with no tag share the skeleton).
fn build(commands: &mut Commands, content: &mut crate::content::Content, a: &mut ModelAssets, id: EntId, models: &[String]) -> Option<Entity> {
    use crate::models::{AnimPlayer, Skeleton, SpawnModel, spawn_model};
    let main = content.model(&models[0], a.meshes, a.materials, a.images, a.bindposes)?;
    let owner = commands.spawn((Name::new(format!("script {id} {}", models[0])), Transform::default(), Visibility::Hidden)).id();
    let mut skeleton = Skeleton::default();
    let mut surfaces = spawn_model(commands, &mut skeleton, SpawnModel { model: &main, owner, attach_to: None, layers: None, shadows: true });
    for m in &models[1..] {
        let (name, tag) = m.split_once('@').unwrap_or((m, ""));
        let Some(part) = content.model(name, a.meshes, a.materials, a.images, a.bindposes) else { continue };
        let attach_to = if tag.is_empty() { None } else { skeleton.joint(tag) };
        surfaces.extend(spawn_model(commands, &mut skeleton, SpawnModel { model: &part, owner, attach_to, layers: None, shadows: true }));
    }
    // People get the soldiers' look (`crate::character_look`), not props.
    if crate::character_look::is_character(&models[0]) {
        crate::character_look::mark(commands, &surfaces);
    }
    commands.entity(owner).insert((StageEnt { id, models: models.to_vec() }, skeleton, AnimPlayer::default()));
    Some(owner)
}

/// A Bevy rotation as CoD angles (pitch, yaw, roll; degrees).
pub fn cod_angles(q: Quat) -> [f32; 3] {
    let o = crate::units::pos([0.0; 3]);
    let to_cod_dir = |v: Vec3| {
        let c = crate::units::to_cod(o + v);
        Vec3::new(c[0], c[1], c[2]).normalize_or_zero()
    };
    let axis = |c: [f32; 3]| crate::units::pos(c) - o;
    let f = to_cod_dir(q * axis([1.0, 0.0, 0.0]));
    let l = to_cod_dir(q * axis([0.0, 1.0, 0.0]));
    let yaw = f.y.atan2(f.x).to_degrees();
    let pitch = (-f.z).atan2((f.x * f.x + f.y * f.y).sqrt()).to_degrees();
    // Roll: the left axis against the one with no roll.
    let flat_left = Vec3::new(-f.y, f.x, 0.0).normalize_or_zero();
    let up0 = f.cross(flat_left);
    let roll = (-l.dot(up0)).atan2(l.dot(flat_left)).to_degrees();
    [pitch, yaw, roll]
}

/// Where the player's view is carried to (a tag of an entity) and how far
/// it may turn from it: (eye in CoD units, base angles, arcs).
pub fn player_carry(level: &Level) -> Option<([f32; 3], [f32; 3], [f32; 4], bool)> {
    let link = level.player_link.as_ref()?;
    let to = level.ent(link.to)?;
    let (o, a) = match &link.tag {
        Some(t) => to.tag(t),
        None => (to.origin, to.angles),
    };
    let _ = PLAYER;
    // (A mounted gun turns with the view: the view's base is where it was
    // placed facing.)
    let a = if link.turret && to.classname == "misc_turret" {
        to.get("angles").map(|v| {
            let n: Vec<f32> = v.split_whitespace().filter_map(|x| x.parse().ok()).collect();
            [n.first().copied().unwrap_or(0.0), n.get(1).copied().unwrap_or(0.0), 0.0]
        }).unwrap_or(a)
    } else {
        a
    };
    // (Sat off the tag: behind a mounted gun.)
    let o = if link.offset == [0.0; 3] { o } else { gsc::level::local_to_world(o, [0.0, a[1], 0.0], link.offset) };
    Some((o, a, link.arcs, link.delta))
}

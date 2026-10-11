//! The level's own GSC scripts running the mission ([`gsc`]): common.ff's
//! and the level's scripts in the interpreter, the map as a
//! [`gsc::level::Level`], fed the player's and the soldiers' positions
//! every frame; what the scripts ask of the game (soldiers from spawners,
//! where they go, the player moved or carried, objectives, messages, the
//! mission's end) is carried out here.
//!
//! On with `COD4RW_CAMPAIGN_SCRIPT=1`; `COD4RW_CAMPAIGN_START=<name>` picks
//! the level's start point (its `jumpto` dvar: `deck`, `hallways`...).

use super::{Actor, Campaign, Outcome};
use crate::combat::{Dead, Health, Killed, Team};
use crate::player::LocalPlayer;
use bevy::prelude::*;
use gsc::level::{Command, EntId, Level, PLAYER};
use gsc::vm::{Value, Vm};
use std::collections::HashMap;
use std::thread::JoinHandle;

pub(super) fn build(app: &mut App) {
    app.init_non_send::<Script>()
        .add_systems(OnEnter(crate::state::GameState::InGame), begin.in_set(crate::state::Setup::Spawn))
        .add_systems(Update, (start, tick, walk_to_goals).chain().run_if(super::active))
        .add_systems(OnExit(crate::state::GameState::InGame), |mut s: NonSendMut<Script>| *s = Script::default());
}

/// common.ff's scripts and animation lengths, loading.
type Loaded = anyhow::Result<(Vec<(String, String)>, HashMap<String, f32>)>;

#[derive(Default)]
pub struct Script {
    loading: Option<JoinHandle<Loaded>>,
    running: Option<Running>,
}

struct Running {
    vm: Vm,
    level: Level,
    /// The level's entities' pawns, both ways.
    pawns: HashMap<EntId, Entity>,
    ents: HashMap<Entity, EntId>,
    /// Health last frame (damage is the difference).
    health: HashMap<Entity, f32>,
    /// The player carried by an entity (`playerlinkto`).
    carried_by: Option<EntId>,
    t0: f32,
}

/// Where a scripted soldier is told to be.
#[derive(Component)]
pub struct Goal {
    pub at: Vec3,
    path: Vec<u32>,
    step: usize,
    repath_at: f32,
}

/// One of the level's entities, as the script knows it.
#[derive(Component)]
pub struct ScriptEnt(pub EntId);

fn begin(campaign: Option<Res<Campaign>>, mut script: NonSendMut<Script>) {
    *script = Script::default();
    if !campaign.is_some_and(|c| c.scripted) {
        return;
    }
    script.loading = Some(std::thread::spawn(load_common));
}

/// common.ff's scripts and every animation's length.
fn load_common() -> Loaded {
    let install = iw3::Install::locate()?;
    let zone = iw3::zone::Zone::parse(&iw3::fastfile::load(&install.zone_path("common"))?, iw3::zone::ParseOptions::default())?;
    Ok(scripts_and_anims(&zone))
}

fn scripts_and_anims(zone: &iw3::zone::Zone) -> (Vec<(String, String)>, HashMap<String, f32>) {
    let mut scripts = Vec::new();
    let mut anims = HashMap::new();
    for a in &zone.assets {
        match a {
            iw3::zone::Asset::RawFile(r) if r.name.ends_with(".gsc") => scripts.push((r.name.clone(), String::from_utf8_lossy(&r.data).into_owned())),
            iw3::zone::Asset::Generic(g) if g.ty == iw3::zone::AssetType::XAnimParts => {
                let rate = g.root.float("framerate");
                if rate > 0.0 {
                    anims.insert(g.name.to_ascii_lowercase(), g.root.int("numframes") as f32 / rate);
                }
            }
            _ => {}
        }
    }
    (scripts, anims)
}

/// Once common's scripts are in: the level's too, the map, and the
/// level's `main` started.
fn start(time: Res<Time>, campaign: Res<Campaign>, content: Res<crate::content::Content>, mut script: NonSendMut<Script>) {
    let Some(handle) = script.loading.take_if(|h| h.is_finished()) else { return };
    let (mut scripts, mut anims) = match handle.join() {
        Ok(Ok(x)) => x,
        Ok(Err(e)) => {
            warn!("campaign: common.ff's scripts: {e}");
            return;
        }
        Err(_) => return,
    };
    let t0 = std::time::Instant::now();
    let (level_scripts, level_anims) = scripts_and_anims(content.map());
    scripts.extend(level_scripts);
    anims.extend(level_anims);
    let mut vm = Vm::new();
    let mut bad = 0;
    for (name, src) in &scripts {
        if vm.add_script(name, src).is_err() {
            bad += 1;
        }
    }
    let unresolved = vm.link();
    let ents: Vec<Vec<(String, String)>> = content
        .map()
        .map_ents()
        .map(|e| iw3::ents::parse(&e.entity_string).into_iter().map(|e| e.fields.into_iter().collect()).collect())
        .unwrap_or_default();
    let bounds: Vec<([f32; 3], [f32; 3])> = content.map().clip_map().map(|c| c.cmodels.iter().map(|m| (m.mins, m.maxs)).collect()).unwrap_or_default();
    let mut level = Level::new(&ents, &bounds);
    level.anim_length = Box::new(move |name| anims.get(&name.to_ascii_lowercase()).copied());
    if let Ok(start) = std::env::var("COD4RW_CAMPAIGN_START") {
        level.dvars.insert("jumpto".into(), start);
    }
    level.install(&mut vm, &ents);
    let map = campaign.mission.map;
    let Some(main) = vm.func(&format!("maps/{map}"), "main") else {
        warn!("campaign: maps/{map}.gsc has no main");
        return;
    };
    let lv = Value::Object(vm.level);
    vm.spawn_thread(main, lv, Vec::new());
    info!(
        "campaign: {} scripts ({bad} unreadable) compiled and linked in {:.2?}; {} calls into missing scripts",
        scripts.len(),
        t0.elapsed(),
        unresolved.len()
    );
    script.running = Some(Running {
        vm,
        level,
        pawns: HashMap::new(),
        ents: HashMap::new(),
        health: HashMap::new(),
        carried_by: None,
        t0: time.elapsed_secs(),
    });
}

/// A frame of script: the world as it is now in, the scripts run, what
/// they asked for carried out.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn tick(
    mut commands: Commands,
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mut campaign: ResMut<Campaign>,
    mut script: NonSendMut<Script>,
    assets: Res<crate::combat::PawnAssets>,
    mut killed: MessageReader<Killed>,
    mut player: Query<(Entity, &mut Transform, &mut crate::movement::ViewAngles), (With<LocalPlayer>, Without<ScriptEnt>)>,
    mut pawns: Query<(Entity, &mut Transform, &mut crate::movement::ViewAngles, &Health, Has<Dead>), (With<ScriptEnt>, Without<LocalPlayer>)>,
    mut goals: Query<&mut Goal>,
) {
    let Some(run) = script.running.as_mut() else { return };
    let now = (time.elapsed_secs() - run.t0) as f64;
    let Ok((me, mut ptf, mut view)) = player.single_mut() else { return };
    // The world as it is.
    let to_angles = |v: &crate::movement::ViewAngles| [-v.pitch.to_degrees(), v.yaw.to_degrees() + 90.0, 0.0];
    if run.carried_by.is_none() {
        let mut feet = crate::units::to_cod(ptf.translation);
        feet[2] -= 1.0;
        run.level.set_pose(PLAYER, feet, to_angles(&view));
    }
    for (e, tf, v, health, dead) in &pawns {
        let (tf, v) = (&*tf, &*v);
        let Some(&id) = run.ents.get(&e) else { continue };
        run.level.set_pose(id, crate::units::to_cod(tf.translation), to_angles(v));
        let before = run.health.insert(e, health.current).unwrap_or(health.current);
        if !dead && health.current < before {
            let amount = (before - health.current).round() as i32;
            run.level.damaged(&mut run.vm, id, amount, Some(PLAYER), [0.0; 3], [0.0; 3]);
        }
    }
    for k in killed.read() {
        if let Some(&id) = run.ents.get(&k.victim) {
            let hp = run.level.ent(id).map_or(0, |e| e.health.max(1));
            let attacker = (k.attacker == Some(me)).then_some(PLAYER);
            run.level.damaged(&mut run.vm, id, hp, attacker, [0.0; 3], [0.0; 3]);
        }
    }
    if keys.just_pressed(KeyCode::KeyF) {
        run.level.use_pressed(&mut run.vm);
    }
    // The scripts.
    run.level.update(&mut run.vm, now);
    run.vm.run(&mut run.level, now);
    campaign.use_hint = run.level.use_hint().map(str::to_owned);
    // What they asked for.
    let elapsed = time.elapsed_secs();
    let skill = crate::tune::get("campaign.skill", 0.45);
    let queued = std::mem::take(&mut run.level.commands);
    // Heads (and the like) the character scripts attached as the soldier
    // spawned, before the spawn itself is reported.
    let mut attached: HashMap<EntId, Vec<String>> = HashMap::new();
    for c in &queued {
        if let Command::Call { ent, name, args } = c
            && name == "attach"
            && let Some(m) = args.first().and_then(Value::as_str)
            && !m.starts_with("weapon_")
        {
            attached.entry(*ent).or_default().push(m.to_owned());
        }
    }
    for c in queued {
        match c {
            Command::SpawnAi { ai, origin, angles, team, classname, model, .. } => {
                let team = if team == "allies" { Team::Allies } else { Team::Axis };
                let name = actor_name(&classname, ai);
                let spawn = crate::world::SpawnPoint { pos: crate::units::pos(origin), yaw: crate::units::yaw_from_cod_degrees(angles[1]), kind: crate::world::SpawnKind::Tdm };
                let pawn = crate::combat::spawn_pawn(&mut commands, &assets, &name, team, &spawn);
                let weapon = run.vm.entity_obj(ai).map(|o| run.vm.field(o, "weapon")).and_then(|w| w.as_str().map(str::to_owned)).unwrap_or_default();
                let gun = super::missions::actor_gun(&if weapon.is_empty() { classname.clone() } else { weapon });
                let mut mover = crate::movement::Mover::default();
                mover.on_ground = true;
                commands.entity(pawn).insert((ScriptEnt(ai), mover, crate::loadout::PawnClass(super::missions::actor_class(gun)), crate::weapons::WeaponInput::default()));
                if team == Team::Axis {
                    let group = run.vm.entity_obj(ai).map(|o| run.vm.field(o, "targetname")).and_then(|w| w.as_str().map(str::to_owned)).unwrap_or_default();
                    commands.entity(pawn).insert((Actor { group, awake: false }, crate::movement::Frozen));
                }
                if !model.is_empty() {
                    let mut models = vec![model];
                    models.extend(attached.remove(&ai).unwrap_or_default());
                    commands.entity(pawn).insert(super::BodyModels(models));
                }
                run.pawns.insert(ai, pawn);
                run.ents.insert(pawn, ai);
                let _ = skill;
            }
            Command::Delete(id) => {
                if let Some(e) = run.pawns.remove(&id) {
                    run.ents.remove(&e);
                    commands.entity(e).try_despawn();
                }
            }
            Command::Call { ent, name, args } => match (ent, name.as_str()) {
                (id, n) if { debug!("script: {} {n} {args:?}", if id == PLAYER { "player".to_owned() } else { id.to_string() }); false } => {}
                (PLAYER, "setorigin") => {
                    if let Some(v) = args.first().and_then(Value::as_vec) {
                        ptf.translation = crate::units::pos(v) + Vec3::Y * crate::units::u(1.0);
                    }
                }
                (PLAYER, "setplayerangles") => {
                    if let Some(v) = args.first().and_then(Value::as_vec) {
                        view.yaw = crate::units::yaw_from_cod_degrees(v[1]);
                        view.pitch = -v[0].to_radians();
                    }
                }
                (PLAYER, n) if n.starts_with("playerlinkto") => {
                    run.carried_by = args.first().and_then(|a| run.vm.entity_of(a));
                    commands.entity(me).insert(crate::movement::Frozen);
                }
                (PLAYER, "unlink") => {
                    run.carried_by = None;
                    commands.entity(me).remove::<crate::movement::Frozen>();
                }
                (id, "unlink" | "field:origin") if run.pawns.contains_key(&id) => {
                    let pawn = run.pawns[&id];
                    if let (Ok((_, mut tf, ..)), Some(e)) = (pawns.get_mut(pawn), run.level.ent(id)) {
                        tf.translation = crate::units::pos(e.origin) + Vec3::Y * crate::units::u(1.0);
                    }
                }
                (id, "teleport" | "forceteleport" | "setorigin") => {
                    let Some(&pawn) = run.pawns.get(&id) else { continue };
                    if let (Ok((_, mut tf, mut v, ..)), Some(at)) = (pawns.get_mut(pawn), args.first().and_then(Value::as_vec)) {
                        tf.translation = crate::units::pos(at) + Vec3::Y * crate::units::u(1.0);
                        if let Some(a) = args.get(1).and_then(Value::as_vec) {
                            v.yaw = crate::units::yaw_from_cod_degrees(a[1]);
                        }
                    }
                }
                (id, "setgoalpos") => {
                    let (Some(&pawn), Some(v)) = (run.pawns.get(&id), args.first().and_then(Value::as_vec)) else { continue };
                    let at = crate::units::pos(v);
                    match goals.get_mut(pawn) {
                        Ok(g) if g.at.distance(at) < crate::units::u(8.0) => {}
                        _ => {
                            commands.entity(pawn).insert(Goal { at, path: Vec::new(), step: 0, repath_at: 0.0 });
                        }
                    }
                }
                _ => {}
            },
            Command::Objective { index, state, text, at } => {
                campaign.script_objectives.insert(index, (state, text, at));
            }
            Command::ObjectiveCurrent(i) => campaign.current_objective = i,
            Command::Print { text, .. } => campaign.messages.push((text, elapsed)),
            Command::MissionFailed if campaign.ended.is_none() => campaign.ended = Some((Outcome::Failed, elapsed)),
            Command::MissionSuccess | Command::ChangeLevel(_) if campaign.ended.is_none() => campaign.ended = Some((Outcome::Complete, elapsed)),
            _ => {}
        }
    }
    // Soldiers linked to something (riding the helicopter) go with it.
    for (&id, &pawn) in &run.pawns {
        if let (Some(e), Ok((_, mut tf, ..))) = (run.level.ent(id).filter(|e| e.linked.is_some()), pawns.get_mut(pawn)) {
            tf.translation = crate::units::pos(e.origin) + Vec3::Y * crate::units::u(1.0);
        }
    }
    // Carried: where the carrier is.
    if let Some(at) = run.carried_by.and_then(|id| run.level.ent(id)).map(|e| e.origin) {
        ptf.translation = crate::units::pos(at);
    }
}

/// "Captain Price" from `actor_ally_hero_price_blackkit`, a rank for the rest.
fn actor_name(classname: &str, id: EntId) -> String {
    const HEROES: [(&str, &str); 6] = [("price", "Price"), ("mark", "Gaz"), ("gaz", "Gaz"), ("pilot", "Pilot"), ("griggs", "Griggs"), ("mac", "Mac")];
    if let Some((_, n)) = HEROES.iter().find(|(k, _)| classname.contains(k)) {
        return (*n).to_owned();
    }
    let rank = if classname.starts_with("actor_ally") { "SAS" } else { super::missions::rank_name(id as usize) };
    format!("{rank} {}", id % 1000)
}

/// Scripted soldiers who aren't fighting walk to where they're told, along
/// the bots' nav graph.
#[allow(clippy::type_complexity)]
fn walk_to_goals(
    time: Res<Time>,
    nav: Option<Res<crate::bots::nav::NavGraph>>,
    mut q: Query<(&Transform, &mut Goal, &mut crate::movement::MoveInput, &mut crate::movement::ViewAngles), (Without<crate::bots::Bot>, Without<Dead>, Without<crate::movement::Frozen>)>,
) {
    let Some(nav) = nav else { return };
    let now = time.elapsed_secs();
    for (tf, mut g, mut input, mut view) in &mut q {
        let here = tf.translation;
        if g.path.is_empty() && now >= g.repath_at {
            g.repath_at = now + 1.0;
            if let (Some(a), Some(b)) = (nav.nearest(here), nav.nearest(g.at)) {
                g.path = nav.path(a, b, 20_000, |_| 0.0).unwrap_or_default();
                g.step = 0;
            }
        }
        let next = g.path.get(g.step).map(|&n| nav.nodes[n as usize].pos).unwrap_or(g.at);
        let flat = Vec3::new(next.x - here.x, 0.0, next.z - here.z);
        if flat.length() < crate::units::u(24.0) {
            if g.step < g.path.len() {
                g.step += 1;
            }
            if g.step >= g.path.len() && Vec3::new(g.at.x - here.x, 0.0, g.at.z - here.z).length() < crate::units::u(32.0) {
                input.forward = 0.0;
                input.right = 0.0;
                continue;
            }
        }
        if flat.length() > 1e-3 {
            view.yaw = (-flat.x).atan2(-flat.z);
        }
        input.forward = 1.0;
        input.right = 0.0;
        input.sprint = false;
    }
}

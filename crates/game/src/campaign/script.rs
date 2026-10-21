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
        .add_systems(Update, (start, player_state, player_grenades, tick, stage, move_brushes, turret, script_gun, walk_to_goals).chain().in_set(Ticked).run_if(super::active))
        .add_systems(Update, aim_friend.after(Ticked).run_if(super::active))
        .add_systems(OnExit(crate::state::GameState::InGame), |mut s: NonSendMut<Script>| {
            *s = Script::default();
            crate::settings_apply::SCRIPT_FOV.store(0, std::sync::atomic::Ordering::Relaxed);
            if let Ok(mut v) = crate::vision::SCRIPT_VISION.lock() {
                *v = None;
            }
        })
        .add_systems(
            PostUpdate,
            aim_manned_gun
                .after(crate::models::animate_skeletons)
                .before(bevy::transform::TransformSystems::Propagate)
                .run_if(super::active),
        );
    // Debug: `COD4RW_CAMPAIGN_FIRE=1` holds the trigger on the nearest enemy.
    if std::env::var_os("COD4RW_CAMPAIGN_FIRE").is_some() {
        app.add_systems(
            Update,
            (|mut w: Query<&mut crate::weapons::WeaponInput, With<LocalPlayer>>| {
                if let Ok(mut w) = w.single_mut() {
                    w.fire = DEBUG_FIRE.load(std::sync::atomic::Ordering::Relaxed);
                    // (And `COD4RW_CAMPAIGN_ADS`'s sights up for real.)
                    if DEBUG_ADS.load(std::sync::atomic::Ordering::Relaxed) {
                        w.ads = true;
                    }
                }
            })
            .after(crate::player::InputSet)
            .before(crate::movement::MovementSet),
        );
    }
    // Debug: `COD4RW_CAMPAIGN_THROW=frag@<s>,flash@<s>,...` holds the
    // grenade key a moment then ([`player_state`] says when).
    if std::env::var_os("COD4RW_CAMPAIGN_THROW").is_some() {
        app.add_systems(
            Update,
            (|mut g: Query<&mut crate::grenades::GrenadeInput, With<LocalPlayer>>| {
                if let Ok(mut g) = g.single_mut() {
                    match DEBUG_THROW.load(std::sync::atomic::Ordering::Relaxed) {
                        1 => g.frag = true,
                        2 => g.special = true,
                        _ => {}
                    }
                }
            })
            .after(crate::player::InputSet)
            .before(crate::movement::MovementSet),
        );
    }
}

/// Debug: the test player's grenade key held now: 1 frag, 2 special.
static DEBUG_THROW: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Debug: the test player fires now ([`stage`]).
static DEBUG_FIRE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Debug: the test player aims down the sights now ([`player_state`]).
static DEBUG_ADS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// common.ff's scripts and animations, loading.
type Loaded = anyhow::Result<(Vec<(String, String)>, super::stage::Anims, Vec<iw3::menu::StringTable>, iw3::zone::Zone)>;

/// The script's frame (its HUD is drawn after).
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Ticked;

#[derive(Default)]
pub struct Script {
    loading: Option<JoinHandle<Loaded>>,
    running: Option<Running>,
}

impl Script {
    /// The scripts' HUD elements as they show now.
    pub fn hud(&self) -> Vec<gsc::level::HudDraw> {
        self.running.as_ref().map(|r| r.level.hud_snapshot(&r.vm)).unwrap_or_default()
    }

    /// The level's clock (seconds since its scripts started).
    pub fn level_time(&self) -> f64 {
        self.running.as_ref().map_or(0.0, |r| r.level.time)
    }
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
    anims: std::rc::Rc<std::cell::RefCell<super::stage::Anims>>,
    stage: super::stage::Stage,
    /// Its actors are staged, not pawns ([`super::Mission::scripted`]).
    staged_ai: bool,
    carry: Carry,
    /// The player's guns as the scripts gave them (their names), the one
    /// in hand first; changed this frame.
    guns: Vec<String>,
    guns_changed: bool,
    /// The gun the scripts put in hand, switched to once the re-equipped
    /// loadout is there.
    want_in_hand: Option<String>,
    /// SP weapons' world models, as found.
    weapon_models: HashMap<String, Option<String>>,
    /// Their fire sounds as others hear them, by weapon; their muzzle
    /// flashes as seen from outside.
    fire_sounds: HashMap<String, String>,
    flashes: HashMap<String, String>,
    /// The level's shellshocks' text, by name.
    shocks: HashMap<String, String>,
    /// On a vehicle's gun: when it may fire next, and whether the player
    /// was armed before.
    turret_next: f64,
    turret_on: Option<bool>,
    /// A script-given gun's next shot, and its shells in flight
    /// ([`script_gun`]).
    gun_next: f64,
    shells: Vec<Shell>,
    /// The player's grenades in the air, as the level's entities.
    grenades: HashMap<Entity, EntId>,
    /// Debug (`COD4RW_CAMPAIGN_FOLLOW`): where the leader has been.
    trail: Vec<(f64, [f32; 3])>,
    trail_at: usize,
    /// (The same, hunting: the way to the enemy it's going for.)
    hunt: Vec<[f32; 3]>,
    /// (And since when it's been next to it.)
    hunt_near: Option<(EntId, f64)>,
    /// (`COD4RW_CAMPAIGN_PLACE`s done, by bit.)
    placed: u32,
    /// Looping effects going, on their anchors, by script entity.
    fx_ents: HashMap<EntId, Entity>,
}

/// The player's look while carried: the link it's for, the turn from the
/// tag's facing (yaw, pitch; radians), and the view as last set.
#[derive(Default)]
struct Carry {
    link: Option<(EntId, Option<String>)>,
    offset: Vec2,
    set: Option<Vec2>,
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
    let mut anims = super::stage::Anims::default();
    anims.add_zone(&zone);
    let mut scripts = scripts_of(&zone);
    // (And code_post_gfx's `codescripts`: `character.gsc`, which the
    // soldiers' character scripts call to pick their bodies and heads.)
    if let Ok(code) = iw3::fastfile::load(&install.zone_path("code_post_gfx")).and_then(|b| iw3::zone::Zone::parse(&b, iw3::zone::ParseOptions::default())) {
        scripts.extend(scripts_of(&code).into_iter().filter(|(n, _)| n.starts_with("codescripts")));
    }
    Ok((scripts, anims, iw3::menu::StringTable::from_zone(&zone), zone))
}

fn scripts_of(zone: &iw3::zone::Zone) -> Vec<(String, String)> {
    zone.assets
        .iter()
        .filter_map(|a| match a {
            iw3::zone::Asset::RawFile(r) if r.name.ends_with(".gsc") => Some((r.name.clone(), String::from_utf8_lossy(&r.data).into_owned())),
            _ => None,
        })
        .collect()
}

/// Once common's scripts are in: the level's too, the map, and the
/// level's `main` started.
fn start(time: Res<Time>, campaign: Res<Campaign>, mut content: ResMut<crate::content::Content>, mut script: NonSendMut<Script>) {
    let Some(handle) = script.loading.take_if(|h| h.is_finished()) else { return };
    let (mut scripts, mut anims, mut tables, common) = match handle.join() {
        Ok(Ok(x)) => x,
        Ok(Err(e)) => {
            warn!("campaign: common.ff's scripts: {e}");
            return;
        }
        Err(_) => return,
    };
    // SP common's models too (the levels' mounted guns, weapon_saw_mg_setup
    // and the like, are kept there), after the map's and common_mp's.
    content.zones.push(common);
    let t0 = std::time::Instant::now();
    scripts.extend(scripts_of(content.map()));
    anims.add_zone(content.map());
    let anims = std::rc::Rc::new(std::cell::RefCell::new(anims));
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
    // Triggers' true shapes.
    if let Some(clip) = content.map().clip_map() {
        level.set_brush_shapes(|n| iw3::brushes::model_shape(clip, n));
    }
    level.anims = Box::new(super::stage::ScriptAnims(anims.clone()));
    // The level's weapons for the scripts' weapon questions (and their
    // fire sounds, as heard from others).
    // The level's shellshocks (`shock/<name>.shock`).
    let shocks: HashMap<String, String> = content
        .map()
        .assets
        .iter()
        .filter_map(|a| match a {
            iw3::zone::Asset::RawFile(r) if r.name.ends_with(".shock") => Some((
                r.name.rsplit(['/', '\\']).next().unwrap_or(&r.name).trim_end_matches(".shock").to_ascii_lowercase(),
                String::from_utf8_lossy(&r.data).into_owned(),
            )),
            _ => None,
        })
        .collect();
    let mut fire_sounds = HashMap::new();
    let mut flashes = HashMap::new();
    // (The map's, then single player common's: its grenades, its
    // mounted guns.)
    for zone in std::iter::once(content.map()).chain(content.zones.iter().skip(2)) {
    for a in &zone.assets {
        if let iw3::zone::Asset::Generic(g) = a
            && g.ty == iw3::zone::AssetType::Weapon
            && !g.name.starts_with(',')
            && !level.weapons.contains_key(&g.name.to_ascii_lowercase())
        {
            let d = crate::weapons::WeaponDef::from_node(zone, &g.root);
            fire_sounds.insert(g.name.to_ascii_lowercase(), d.sounds.fire.clone());
            if let Some(i) = g.root.asset("worldFlashEffect") {
                flashes.insert(g.name.to_ascii_lowercase(), zone.get(i).name().trim_start_matches(',').to_owned());
            }
            let w = &g.root;
            level.weapons.insert(
                g.name.to_ascii_lowercase(),
                gsc::level::WeaponInfo {
                    class: gsc::level::WeaponInfo::class_name(d.class).to_owned(),
                    clip_size: d.clip_size as i32,
                    fire_time: d.fire_time,
                    fire_type: d.fire_type,
                    bolt_action: w.int("bBoltAction") != 0,
                    fight_dist: Some(w.float("fightDist")).filter(|d| *d > 0.0).unwrap_or(768.0),
                    max_dist: Some(w.float("maxDist")).filter(|d| *d > 0.0).unwrap_or(4096.0),
                    max_ammo: w.int("maxAmmo") as i32,
                    world_model: w.asset("worldModel").map(|i| zone.get(i).name().trim_start_matches(',').to_owned()).unwrap_or_default(),
                    blast: (w.int("iExplosionRadius") > 0).then(|| {
                        let sound = |f: &str| w.node(f).and_then(|n| n.node("name")).and_then(|n| n.string("soundName")).map(str::to_owned).unwrap_or_default();
                        (
                            w.asset("projExplosionEffect").map(|i| zone.get(i).name().trim_start_matches(',').to_owned()).unwrap_or_default(),
                            sound("projExplosionSound"),
                            w.int("iExplosionRadius") as f32,
                            w.int("iExplosionInnerDamage") as f32,
                            w.int("iExplosionOuterDamage") as f32,
                        )
                    }),
                },
            );
        }
    }
    }

    // Staged actors think in the level, under their animscripts.
    level.think_ai = campaign.mission.scripted;
    level.path_nodes = iw3::paths::from_zone(content.map()).into_iter().map(|n| gsc::level::PathNode { origin: n.origin, kind: n.kind, angle: n.angle, links: n.links }).collect();
    // String tables (`tablelookup`): the level's over common's.
    tables.extend(iw3::menu::StringTable::from_zone(content.map()));
    for t in tables {
        let rows = (0..t.rows).map(|r| (0..t.columns).map(|c| t.get(r, c).unwrap_or("").to_owned()).collect()).collect();
        level.tables.insert(t.name.to_ascii_lowercase().replace('\\', "/"), rows);
    }
    if let Ok(start) = std::env::var("COD4RW_CAMPAIGN_START") {
        // (Older levels read `jumpto`, `_load`'s starts `start`.)
        level.dvars.insert("jumpto".into(), start.clone());
        level.dvars.insert("start".into(), start);
    }
    level.note_filter = std::env::var("COD4RW_NOTE_TRACE").ok();
    // The engine's own (`_load` sets `level.script` from it).
    level.dvars.insert("mapname".into(), campaign.mission.map.to_owned());
    level.dvars.extend(super::progress::load());
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
        anims,
        stage: super::stage::Stage::default(),
        staged_ai: campaign.mission.scripted,
        carry: Carry::default(),
        guns: Vec::new(),
        guns_changed: false,
        want_in_hand: None,
        weapon_models: HashMap::new(),
        fire_sounds,
        flashes,
        shocks,
        fx_ents: HashMap::new(),
        turret_next: 0.0,
        turret_on: None,
        gun_next: 0.0,
        shells: Vec::new(),
        grenades: HashMap::new(),
        trail: Vec::new(),
        trail_at: 0,
        hunt: Vec::new(),
        hunt_near: None,
        placed: 0,
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
    mut sfx: ResMut<crate::audio::Sfx>,
    spatial: avian3d::prelude::SpatialQuery,
    mut damage: MessageWriter<crate::combat::Damage>,
    (mut effects, skeletons, inputs, mut bank, mut audio_assets, mut pending_nades, nade_defs): (
        Option<ResMut<crate::fx::Effects>>,
        Query<&crate::models::Skeleton>,
        Query<&crate::splitscreen::PlayerInput, With<LocalPlayer>>,
        Option<ResMut<crate::audio::Bank>>,
        ResMut<Assets<AudioSource>>,
        ResMut<crate::grenades::Pending>,
        Res<crate::grenades::GrenadeDefs>,
    ),
) {
    let Some(run) = script.running.as_mut() else { return };
    let now = (time.elapsed_secs() - run.t0) as f64;
    let Ok((me, mut ptf, mut view)) = player.single_mut() else { return };
    // Debug: `COD4RW_CAMPAIGN_WALK=x,y,z;x,y,z;...`: the player stands at
    // each point in turn, 4 s apiece (through the level's triggers).
    if let Ok(walk) = std::env::var("COD4RW_CAMPAIGN_WALK") {
        let points: Vec<[f32; 3]> = walk
            .split(';')
            .filter_map(|p| {
                let v: Vec<f32> = p.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                (v.len() == 3).then(|| [v[0], v[1], v[2]])
            })
            .collect();
        if let Some(p) = points.get(((now / 4.0) as usize).min(points.len().saturating_sub(1))) {
            ptf.translation = crate::units::pos(*p) + Vec3::Y * crate::units::u(1.0);
        }
    }
    // Debug: `COD4RW_CAMPAIGN_FOLLOW=<classname part>[@<from s>]`: the
    // player walks that actor's own way behind it (through the triggers it
    // walked through) at 250 units a second, stopping 48 units short of it;
    // not while carried.
    if let Ok(v) = std::env::var("COD4RW_CAMPAIGN_FOLLOW") {
        let (who, from) = v.split_once('@').map_or((v.as_str(), 0.0), |(w, t)| (w, t.parse::<f64>().unwrap_or(0.0)));
        // (`COD4RW_CAMPAIGN_FOLLOW_SPEED`, units a second: a sprint for
        // timed runs such as Crew Expendable's escape.)
        let follow_speed = std::env::var("COD4RW_CAMPAIGN_FOLLOW_SPEED").ok().and_then(|s| s.parse::<f32>().ok()).unwrap_or(250.0);
        // (Its own way only: not where it rode, a helicopter over the sea.)
        let leader = run.level.ents.iter().find(|e| e.kind == gsc::level::Kind::Ai && e.alive && e.classname.contains(who)).map(|e| (e.origin, e.linked.is_some()));
        if let Some((at, riding)) = leader {
            if !riding && run.trail.last().is_none_or(|l| (l.1[0] - at[0]).hypot(l.1[1] - at[1]) > 8.0 || (l.1[2] - at[2]).abs() > 24.0) {
                run.trail.push((now, at));
            }
            if run.level.player_link.is_none() && now >= from && !run.trail.is_empty() {
                let me = crate::units::to_cod(ptf.translation);
                let d = |p: [f32; 3]| ((p[0] - me[0]).powi(2) + (p[1] - me[1]).powi(2) + (p[2] - me[2]).powi(2)).sqrt();
                // (Back on the way where it's nearest, if it was left.)
                if run.trail_at >= run.trail.len() || d(run.trail[run.trail_at].1) > 160.0 {
                    run.trail_at = (0..run.trail.len()).min_by(|a, b| d(run.trail[*a].1).total_cmp(&d(run.trail[*b].1))).unwrap_or(0);
                }
                let mut budget = follow_speed * time.delta_secs();
                let mut pos = me;
                // (`COD4RW_CAMPAIGN_RUSH=1`: straight for the nearest
                // trigger a script waits on, over the path nodes, as in a
                // timed run, not after the leader.)
                let mut rushing = false;
                if std::env::var_os("COD4RW_CAMPAIGN_RUSH").is_some()
                    && let Some(goal) = run.level.awaited_flag_triggers(&run.vm).into_iter().min_by(|a, b| d(*a).total_cmp(&d(*b)))
                {
                    rushing = true;
                    if run.hunt.last() != Some(&goal) {
                        run.hunt = run.level.route(me, goal).unwrap_or_default();
                        info!("campaign rush: from {me:?} to {goal:?} over {} nodes", run.hunt.len());
                        run.hunt.push(goal);
                    }
                    while budget > 0.0 && !run.hunt.is_empty() {
                        let next = run.hunt[0];
                        let gap = ((next[0] - pos[0]).powi(2) + (next[1] - pos[1]).powi(2) + (next[2] - pos[2]).powi(2)).sqrt();
                        if gap <= budget {
                            pos = next;
                            budget -= gap;
                            if run.hunt.len() > 1 {
                                run.hunt.remove(0);
                            } else {
                                budget = 0.0;
                            }
                        } else {
                            let k = budget / gap;
                            pos = [pos[0] + (next[0] - pos[0]) * k, pos[1] + (next[1] - pos[1]) * k, pos[2] + (next[2] - pos[2]) * k];
                            budget = 0.0;
                        }
                    }
                }
                // (A leader standing still 2 s waits for the player: up to it.)
                let waiting = run.trail.last().is_some_and(|l| now - l.0 > 2.0);
                let keep = if waiting { 0.0 } else { 48.0 };
                while budget > 0.0 && run.hunt.is_empty() && run.trail_at + 1 < run.trail.len() {
                    let next = run.trail[run.trail_at + 1].1;
                    let lead = ((next[0] - at[0]).powi(2) + (next[1] - at[1]).powi(2)).sqrt();
                    if lead < keep {
                        break;
                    }
                    let gap = ((next[0] - pos[0]).powi(2) + (next[1] - pos[1]).powi(2) + (next[2] - pos[2]).powi(2)).sqrt();
                    if gap <= budget {
                        pos = next;
                        budget -= gap;
                        run.trail_at += 1;
                    } else {
                        let k = budget / gap;
                        pos = [pos[0] + (next[0] - pos[0]) * k, pos[1] + (next[1] - pos[1]) * k, pos[2] + (next[2] - pos[2]) * k];
                        budget = 0.0;
                    }
                }
                // (A leader that's hardly moved in 6 s, stood or in cover,
                // waits for the player to go on: to the nearest trigger
                // whose flag a script waits for, nearest it.)
                let hardly = run.trail.iter().rev().find(|t| now - t.0 >= 6.0).is_some_and(|t| (t.1[0] - at[0]).hypot(t.1[1] - at[1]) < 200.0)
                    || run.trail.last().is_some_and(|l| now - l.0 > 4.0);
                if hardly && !rushing {
                    // (Nearest the leader: one ahead of it, not one a
                    // later scene waits for across the ship.)
                    let d = |p: &[f32; 3]| (p[0] - at[0]).hypot(p[1] - at[1]) + (p[2] - at[2]).abs() * 2.0;
                    let found = run.level.awaited_flag_triggers(&run.vm);
                    if (now * 10.0) as i64 % 50 == 0 {
                        debug!("campaign follow: leader waits at {at:?}; awaited triggers {found:?}");
                    }
                    if let Some(t) = found.into_iter().filter(|t| d(t) < 1200.0).min_by(|a, b| d(a).total_cmp(&d(b))) {
                        pos = t;
                        run.hunt.clear();
                    } else {
                        // (None: the level waits on enemies dying, as a
                        // person would go and kill them. Over the path
                        // nodes to near the nearest: one in cover isn't hit
                        // from afar.)
                        let me = pos;
                        let d = |p: [f32; 3]| ((p[0] - me[0]).powi(2) + (p[1] - me[1]).powi(2) + (p[2] - me[2]).powi(2)).sqrt();
                        let foe = run
                            .level
                            .ents
                            .iter()
                            .filter(|e| e.kind == gsc::level::Kind::Ai && e.alive && e.team == "axis" && d(e.origin) < 2500.0)
                            .map(|e| (e.id, e.origin))
                            .min_by(|a, b| d(a.1).total_cmp(&d(b.1)));
                        match foe {
                            Some((_, to)) if d(to) > 200.0 => {
                                if run.hunt.last().is_none_or(|l| ((l[0] - to[0]).powi(2) + (l[1] - to[1]).powi(2)).sqrt() > 64.0) {
                                    run.hunt = run.level.route(me, to).unwrap_or_else(|| vec![to]);
                                }
                                let mut budget = follow_speed * time.delta_secs();
                                while budget > 0.0 && !run.hunt.is_empty() {
                                    let next = run.hunt[0];
                                    let gap = d(next).max(0.0);
                                    let cur = pos;
                                    let gap = if pos == me { gap } else { ((next[0] - cur[0]).powi(2) + (next[1] - cur[1]).powi(2) + (next[2] - cur[2]).powi(2)).sqrt() };
                                    if gap <= budget {
                                        pos = next;
                                        budget -= gap;
                                        run.hunt.remove(0);
                                    } else {
                                        let k = budget / gap;
                                        pos = [cur[0] + (next[0] - cur[0]) * k, cur[1] + (next[1] - cur[1]) * k, cur[2] + (next[2] - cur[2]) * k];
                                        budget = 0.0;
                                    }
                                }
                            }
                            // (Next to it 5 s and it's still up, behind
                            // cover the driver can't aim around: done
                            // for, as a person would see to.)
                            Some((id, _)) => {
                                run.hunt.clear();
                                match run.hunt_near {
                                    Some((t, since)) if t == id => {
                                        if now - since > 5.0 {
                                            info!("campaign follow: the driver finishes {id} (next to it 5 s)");
                                            let hp = run.level.ent(id).map_or(0, |e| e.health.max(1));
                                            run.level.damaged(&mut run.vm, id, hp + 1, Some(PLAYER), [0.0; 3], [0.0; 3]);
                                            run.hunt_near = None;
                                        }
                                    }
                                    _ => run.hunt_near = Some((id, now)),
                                }
                            }
                            None => run.hunt.clear(),
                        }
                    }
                }
                ptf.translation = crate::units::pos(pos) + Vec3::Y * crate::units::u(1.0);
            }
        }
    }
    // The world as it is.
    let to_angles = |v: &crate::movement::ViewAngles| [-v.pitch.to_degrees(), v.yaw.to_degrees() + 90.0, 0.0];
    // (Carried too: triggers along a ride see the player.)
    let mut feet = crate::units::to_cod(ptf.translation);
    feet[2] -= 1.0;
    run.level.set_pose(PLAYER, feet, to_angles(&view));
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
    // (Debug: `COD4RW_CAMPAIGN_USE_AT=<s>[,<s>...]` presses Use then.)
    let dt = time.delta_secs() as f64;
    let use_at = std::env::var("COD4RW_CAMPAIGN_USE_AT").ok().is_some_and(|v| v.split(',').filter_map(|t| t.trim().parse::<f64>().ok()).any(|t| now >= t && now - dt < t));
    let use_now = keys.just_pressed(KeyCode::KeyF) || use_at;
    if use_now {
        run.level.use_pressed(&mut run.vm);
    }
    // The CoD4 commands the player pressed (their default PC keys), for the
    // scripts' `notifyoncommand`. (Debug: `COD4RW_CAMPAIGN_COMMAND=<cmd>@<s>,...`.)
    {
        let mut pressed: Vec<&str> = Vec::new();
        if use_now {
            pressed.extend(["+activate", "+usereload"]);
        }
        if let Ok(input) = inputs.single() {
            let k = |c: KeyCode| input.keys.just_pressed(c);
            let m = |b: MouseButton| input.mouse.just_pressed(b);
            if m(MouseButton::Left) {
                pressed.push("+attack");
            }
            if m(MouseButton::Right) {
                pressed.extend(["+speed_throw", "+toggleads_throw", "+speed"]);
            }
            if input.scroll != 0.0 || k(KeyCode::Digit1) {
                pressed.push("weapnext");
            }
            for (key, cmd) in [
                (KeyCode::KeyR, "+reload"),
                (KeyCode::KeyV, "+melee"),
                (KeyCode::KeyG, "+frag"),
                (KeyCode::KeyQ, "+smoke"),
                (KeyCode::Space, "+gostand"),
                (KeyCode::KeyC, "togglecrouch"),
                (KeyCode::ControlLeft, "toggleprone"),
                (KeyCode::ShiftLeft, "+breath_sprint"),
                (KeyCode::KeyN, "+actionslot 1"),
            ] {
                if k(key) {
                    pressed.push(cmd);
                }
            }
        }
        let mut owned: Vec<String> = pressed.iter().map(|c| c.to_string()).collect();
        // (The debug trigger presses `+attack` twice a second; the debug
        // sights press `+speed_throw` as they go up.)
        if std::env::var_os("COD4RW_CAMPAIGN_FIRE").is_some() && (now * 2.0).floor() != ((now - dt) * 2.0).floor() {
            owned.push("+attack".into());
        }
        if let Ok(v) = std::env::var("COD4RW_CAMPAIGN_ADS")
            && let Some(from) = v.split('-').next().and_then(|x| x.parse::<f64>().ok())
            && now >= from
            && now - dt < from
        {
            owned.extend(["+speed_throw".into(), "+toggleads_throw".into(), "+speed".into()]);
        }
        if let Ok(list) = std::env::var("COD4RW_CAMPAIGN_COMMAND") {
            for item in list.split(',') {
                if let Some((cmd, t)) = item.split_once('@')
                    && let Ok(t) = t.parse::<f64>()
                    && now >= t
                    && now - dt < t
                {
                    owned.push(cmd.to_owned());
                }
            }
        }
        for c in owned {
            // (A test's `weapnext`, the game's keys not being pressed: the
            // next of the scripts' guns in hand.)
            if c == "weapnext" && std::env::var_os("COD4RW_CAMPAIGN_COMMAND").is_some() && run.guns.len() > 1 {
                let g = run.guns.remove(0);
                run.guns.push(g);
                run.guns_changed = true;
                run.level.player_current = run.guns.first().cloned();
            }
            // (A melee stabs what's in front.)
            if c == "+melee" {
                let eye = ptf.translation + Vec3::Y * crate::units::u(60.0);
                let fwd = Quat::from_euler(EulerRot::YXZ, view.yaw, view.pitch, 0.0) * Vec3::NEG_Z;
                let o = crate::units::to_cod(eye);
                let t = crate::units::to_cod(eye + fwd * crate::units::u(1.0));
                let dir = [t[0] - o[0], t[1] - o[1], t[2] - o[2]];
                run.level.player_melee(&mut run.vm, o, dir);
            }
            run.level.player_command(&mut run.vm, &c);
        }
        // (Debug: `COD4RW_CAMPAIGN_MENU=<response>@<s>,...` answers a menu.)
        if let Ok(list) = std::env::var("COD4RW_CAMPAIGN_MENU") {
            for item in list.split(',') {
                if let Some((r, t)) = item.split_once('@')
                    && t.parse::<f64>().is_ok_and(|t| now >= t && now - dt < t)
                {
                    run.level.menu_response(&mut run.vm, "", r);
                }
            }
        }
    }
    // Who sees whom (the actors' senses), traced eye to eye through the
    // world.
    if run.level.think_ai {
        let filter = crate::collision::ai_sight_filter();
        let eye = |level: &Level, id: EntId| level.ent(id).map(|e| crate::units::pos([e.origin[0], e.origin[1], e.origin[2] + if id == PLAYER { 60.0 } else { 56.0 }]));
        let pairs = run.level.sight_pairs(&run.vm);
        let mut sight = HashMap::new();
        for (a, b) in pairs {
            let (Some(from), Some(to)) = (eye(&run.level, a), eye(&run.level, b)) else { continue };
            let seen = Dir3::new(to - from).ok().is_some_and(|d| spatial.cast_ray(from, d, from.distance(to), true, &filter).is_none());
            sight.insert((a, b), seen);
        }
        run.level.sight = sight;
    }
    // The scripts, with the world's collision for their traces.
    let tracer = |from: [f32; 3], to: [f32; 3], kind: gsc::level::TraceKind| -> Option<gsc::level::TraceHit> {
        use crate::collision::Layer;
        let (a, b) = (crate::units::pos(from), crate::units::pos(to));
        let len = a.distance(b);
        let dir = Dir3::new(b - a).ok()?;
        let filter = match kind {
            gsc::level::TraceKind::Bullet => avian3d::prelude::SpatialQueryFilter::from_mask([Layer::World, Layer::ShotClip]),
            gsc::level::TraceKind::Sight => crate::collision::ai_sight_filter(),
            gsc::level::TraceKind::Physics => avian3d::prelude::SpatialQueryFilter::from_mask([Layer::World, Layer::PlayerClip]),
            gsc::level::TraceKind::Actor => avian3d::prelude::SpatialQueryFilter::from_mask([Layer::World]),
        };
        let h = spatial.cast_ray(a, dir, len, true, &filter)?;
        let at = a + *dir * h.distance;
        let n = crate::units::to_cod(crate::units::pos([0.0; 3]) + h.normal);
        let o = crate::units::to_cod(crate::units::pos([0.0; 3]));
        Some((h.distance / len.max(1e-4), crate::units::to_cod(at), [n[0] - o[0], n[1] - o[1], n[2] - o[2]]))
    };
    gsc::level::with_tracer(&tracer, || {
        run.level.update(&mut run.vm, now);
        run.vm.run(&mut run.level, now);
    });
    campaign.use_hint = run.level.use_hint().map(str::to_owned);
    super::set_hud_hidden(run.level.dvars.get("compass").is_some_and(|v| v == "0") && run.level.dvars.get("ammocounterhide").is_some_and(|v| v == "1"));
    // What they asked for.
    let elapsed = time.elapsed_secs();
    let skill = crate::tune::get("campaign.skill", 0.45);
    // Debug: `COD4RW_CAMPAIGN_PLACE=x,y,z,yaw@<s>`: the player put there
    // once at that time, as a script's `setOrigin`/`setPlayerAngles` (a
    // movement test from a spot, the ladder's foot).
    // (`;` between several, each once.)
    if let Ok(v) = std::env::var("COD4RW_CAMPAIGN_PLACE") {
        for (i, item) in v.split(';').enumerate() {
            let Some((p, at)) = item.split_once('@') else { continue };
            let Ok(at) = at.trim().parse::<f64>() else { continue };
            if now < at || run.placed & (1 << i) != 0 {
                continue;
            }
            let c: Vec<f32> = p.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            if c.len() == 4 {
                run.placed |= 1 << i;
                run.level.commands.push(Command::Call { ent: PLAYER, name: "setorigin".into(), args: vec![Value::Vector([c[0], c[1], c[2]])] });
                run.level.commands.push(Command::Call { ent: PLAYER, name: "setplayerangles".into(), args: vec![Value::Vector([0.0, c[3], 0.0])] });
                info!("campaign: placed the player at {c:?}");
            }
        }
    }
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
            // Staged actors are drawn by the stage as the scripts animate them.
            Command::SpawnAi { .. } if run.staged_ai => {}
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
                if let Some(a) = run.fx_ents.remove(&id) {
                    commands.entity(a).try_despawn();
                }
                if let Some(e) = run.pawns.remove(&id) {
                    run.ents.remove(&e);
                    commands.entity(e).try_despawn();
                }
            }
            Command::Call { ent, name, args } => match (ent, name.as_str()) {
                (id, n) if { debug!("script: {} {n} {args:?}", if id == PLAYER { "player".to_owned() } else { id.to_string() }); false } => {}
                // A script menu: F.N.G.'s "invert your aim?" popup isn't
                // drawn yet (it lives in the level's zone), so it's answered
                // as its default, keeping the look as it is.
                (PLAYER, "openmenu") => {
                    let menu = args.first().map(Value::display).unwrap_or_default().to_string();
                    // (And its end: the difficulty it recommends isn't drawn
                    // either; "continue" keeps the one chosen.)
                    if menu.contains("invert_axis") || menu == "select_difficulty" {
                        info!("campaign: menu {menu} not shown yet: answered \"continue\"");
                        run.level.menu_response(&mut run.vm, &menu, "continue");
                    } else {
                        info!("campaign: menu {menu} not shown yet (debug: COD4RW_CAMPAIGN_MENU=<response>@<s>)");
                    }
                }
                // An actor's grenade leaves its hand (its animscript's
                // `throwGrenade()`): a frag on its fuse.
                (id, "throwgrenade") => {
                    let (from, vel) = (args.first().and_then(Value::as_vec), args.get(1).and_then(Value::as_vec));
                    if let (Some(from), Some(vel), Some(thrower)) = (from, vel, run.stage.entity(id).or_else(|| run.pawns.get(&id).copied())) {
                        let fuse = nade_defs.get(crate::grenades::Kind::Frag).map_or(3.5, |s| s.fuse);
                        let velocity = crate::units::pos(vel) - crate::units::pos([0.0; 3]);
                        pending_nades.throw(crate::grenades::Kind::Frag, thrower, crate::units::pos(from), velocity, time.elapsed_secs() + fuse);
                    }
                }
                (PLAYER, "takeallweapons") => {
                    run.guns.clear();
                    super::set_unarmed(true);
                }
                (PLAYER, "giveweapon" | "takeweapon" | "switchtoweapon") => {
                    let w = args.first().map(Value::display).unwrap_or_default().to_ascii_lowercase();
                    if super::missions::sp_gun(&w).is_some() {
                        let had = run.guns.iter().position(|g| *g == w);
                        match (name.as_str(), had) {
                            ("giveweapon", None) => run.guns.push(w),
                            ("takeweapon", Some(i)) => {
                                run.guns.remove(i);
                            }
                            ("switchtoweapon", Some(i)) => {
                                let g = run.guns.remove(i);
                                run.guns.insert(0, g);
                            }
                            _ => {}
                        }
                        run.guns_changed = true;
                        super::set_unarmed(run.guns.is_empty());
                    }
                }
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
            // Sounds: on what plays them (staged or a pawn), the player's own
            // without place, else where they were played.
            Command::PlaySound { ent, alias, at, notify, .. } => {
                // (Its notify when it ends.)
                if let (Some(n), Some(id)) = (&notify, ent)
                    && let Some(len) = bank.as_deref_mut().and_then(|b| b.length(&alias, &mut audio_assets))
                {
                    // (Debug: `COD4RW_SOUND_TRACE=1`: each waited-on sound's length.)
                    if std::env::var_os("COD4RW_SOUND_TRACE").is_some() {
                        info!("campaign: sound {alias} on {id} lasts {len:.2} s ({n})");
                    }
                    run.level.sound_ends(id, n, len);
                }
                match ent.and_then(|id| run.stage.entity(id).or_else(|| run.pawns.get(&id).copied())) {
                Some(e) => sfx.play_on(alias, e),
                None if ent == Some(PLAYER) => sfx.play(alias, None),
                None => sfx.play(alias, Some(crate::units::pos(at))),
            }
            }
            Command::Music(Some(alias)) => sfx.play(alias, None),
            // The vision set (the film), faded to over its time.
            Command::Vision { name, time } => {
                if let Ok(mut v) = crate::vision::SCRIPT_VISION.lock() {
                    let n = v.as_ref().map_or(0, |r| r.0) + 1;
                    *v = Some((n, name, time));
                }
            }
            Command::Blur { radius, time } => super::blur::set(radius, time),
            Command::Shellshock { name, duration } => {
                let file = run.shocks.get(&name.to_ascii_lowercase()).map(String::as_str);
                super::blur::shock(duration, file);
            }
            // The scripts' effects: fire, smoke, muzzle flashes, blood.
            // (The level's ambient fog cards as MP's: off unless the map's
            // look turns them on, `crate::mapfx::fog_card_dropped`.)
            Command::LoopFx { fx, .. } | Command::PlayFx { fx, .. } if crate::mapfx::fog_card_dropped(&fx, campaign.mission.map) => {}
            Command::PlayFx { fx, at, forward } => {
                // Debug: `COD4RW_FX_TRACE=1` names each script effect and where.
                if std::env::var_os("COD4RW_FX_TRACE").is_some() {
                    info!("script fx: {fx} at {:?} facing {:?}", at.map(f32::round), forward);
                }
                if let Some(fxs) = effects.as_mut() {
                    let frame = crate::fx::Frame::facing(crate::units::pos(at), crate::units::dir(forward), 0.0);
                    fxs.play(&fx, crate::fx::Anchor::Fixed(frame), crate::fx::FxLayer::World);
                }
            }
            // Kept going: the map's ambience (createfx) and the like.
            // (Started again where it already goes: still the one.)
            // (Triggered again: the one going restarts, as CoD's.)
            Command::LoopFx { ent, fx, at, forward, .. } => {
                if let Some(old) = run.fx_ents.remove(&ent) {
                    commands.entity(old).try_despawn();
                }
                if std::env::var_os("COD4RW_FX_TRACE").is_some() {
                    info!("script loop fx: {fx} at {:?} facing {:?}", at.map(f32::round), forward);
                }
                // On an anchor of its own: the effect stops when the
                // script deletes its entity.
                if let Some(fxs) = effects.as_mut() {
                    let pos = crate::units::pos(at);
                    let dir = crate::units::dir(forward);
                    let anchor = commands.spawn((Name::new(format!("script fx {fx}")), Transform::from_translation(pos).with_rotation(Quat::from_rotation_arc(Vec3::X, dir.try_normalize().unwrap_or(Vec3::X))))).id();
                    fxs.play_ambient_bolted(&fx, anchor);
                    run.fx_ents.insert(ent, anchor);
                }
            }
            Command::PlayFxOnTag { fx, ent, tag } => {
                let joint = run.stage.entity(ent).and_then(|b| skeletons.get(b).ok()?.joint(&tag.to_ascii_lowercase()));
                if let (Some(fxs), Some(j)) = (effects.as_mut(), joint) {
                    fxs.play(&fx, crate::fx::Anchor::Bolted(j), crate::fx::FxLayer::World);
                }
            }
            // An actor fires: at the player, a roll against its accuracy
            // for a hit; at an actor, the scripts' damage.
            Command::Shoot { ai, target, at, weapon, accuracy } => {
                let from = run.level.ent(ai).map(|e| e.origin).unwrap_or(at);
                let hit = rand::random::<f32>() < (crate::tune::get("campaign.ai_hit", 0.18) * accuracy).clamp(0.0, 0.95);
                let amount = crate::tune::get("campaign.ai_damage", 20.0);
                match target {
                    Some(PLAYER) if hit => {
                        damage.write(crate::combat::Damage { target: me, attacker: None, amount, location: crate::combat::HitLocation::Torso, weapon: "ak47_mp" });
                    }
                    Some(t) if hit && t != PLAYER => {
                        let dir = [at[0] - from[0], at[1] - from[1], at[2] - from[2]];
                        run.level.damaged(&mut run.vm, t, amount as i32, Some(ai), dir, at);
                    }
                    _ => {}
                }
                // Its muzzle flash, at the gun's muzzle.
                let muzzle = run.stage.entity(ai).and_then(|b| {
                    let sk = skeletons.get(b).ok()?;
                    sk.joint("tag_flash").or_else(|| sk.joint("tag_weapon_right"))
                });
                if let (Some(fxs), Some(j), Some(flash)) = (effects.as_mut(), muzzle, run.flashes.get(&weapon.to_ascii_lowercase())) {
                    fxs.play(flash, crate::fx::Anchor::Bolted(j), crate::fx::FxLayer::World);
                }
                // Heard where it fires.
                if let Some(alias) = run.fire_sounds.get(&weapon.to_ascii_lowercase()).filter(|a| !a.is_empty()) {
                    sfx.play(alias.clone(), Some(crate::units::pos([from[0], from[1], from[2] + 50.0])));
                }
                debug!("script: {ai} shoots {target:?} with {weapon} (hit {hit})");
            }
            Command::MissionFailed if campaign.ended.is_none() => {
                campaign.ended = Some((Outcome::Failed, elapsed));
                campaign.deadquote = run.level.dvars.get("ui_deadquote").filter(|q| !q.is_empty()).cloned();
                info!("campaign: the scripts failed the mission ({:?})", campaign.deadquote);
            }
            Command::MissionSuccess | Command::ChangeLevel(_) if campaign.ended.is_none() => {
                info!("campaign: mission complete ({c:?})");
                super::progress::save(&run.level.dvars);
                if let Command::ChangeLevel(next) = &c {
                    campaign.next = Some(next.clone());
                }
                campaign.ended = Some((Outcome::Complete, elapsed));
            }
            _ => {}
        }
    }
    // The player's guns as the scripts left them: re-equipped (the one in
    // hand first).
    if std::mem::take(&mut run.guns_changed) && !run.guns.is_empty() {
        let mut class = campaign.mission.class();
        // (A primary, then a pistol, as CoD4's slots; the one in hand then
        // switched to.)
        class.guns = slot_order(&run.guns).iter().filter_map(|w| super::missions::sp_gun(w)).map(|(spec, name)| crate::loadout::Gun { spec, camo: 0, name, variant: None }).collect();
        run.want_in_hand = run.guns.first().cloned();
        info!("campaign: the player's guns {:?}", run.guns);
        commands.entity(me).insert(crate::loadout::PawnClass(class)).remove::<crate::loadout::Loadout>();
    }
    // Soldiers linked to something (riding the helicopter) go with it.
    for (&id, &pawn) in &run.pawns {
        if let (Some(e), Ok((_, mut tf, ..))) = (run.level.ent(id).filter(|e| e.linked.is_some()), pawns.get_mut(pawn)) {
            tf.translation = crate::units::pos(e.origin) + Vec3::Y * crate::units::u(1.0);
        }
    }

}

/// The friendly the player's crosshair is on (their name), for the HUD's
/// green crosshair and name ([`aimed_friend`]).
static AIMED_FRIEND: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// The name of the living ally under the player's crosshair, if any.
pub fn aimed_friend() -> Option<String> {
    if !super::active() {
        return None;
    }
    AIMED_FRIEND.lock().ok().and_then(|f| f.clone())
}

/// Traces the player's view like a bullet: a living ally hit (a staged
/// actor's hitbox, or a soldier pawn's) is named, as CoD4 SP names
/// friendlies under the crosshair (their `name`, else `script_friendname`).
fn aim_friend(
    script: NonSend<Script>,
    spatial: avian3d::prelude::SpatialQuery,
    cams: Query<(&crate::splitscreen::SlotCamera, &GlobalTransform)>,
    me: Query<Entity, With<LocalPlayer>>,
    stage_boxes: Query<&ChildOf, With<super::stage::StageHitbox>>,
    hitboxes: Query<&crate::combat::Hitbox>,
) {
    let found = (|| {
        let run = script.running.as_ref()?;
        let (_, cam) = cams.iter().find(|c| c.0.0 == 0)?;
        let me = me.single().ok();
        let own = |e: Entity| hitboxes.get(e).map_or(true, |h| Some(h.owner) != me);
        let hit = spatial.cast_ray_predicate(cam.translation(), cam.forward(), crate::units::u(4096.0), true, &crate::collision::bullet_filter(), &own)?;
        let id = match stage_boxes.get(hit.entity) {
            Ok(parent) => run.stage.id_of(parent.parent())?,
            Err(_) => *run.ents.get(&hitboxes.get(hit.entity).ok()?.owner)?,
        };
        let e = run.level.ent(id).filter(|e| e.alive && e.team == "allies")?;
        let o = run.vm.entity_obj(id);
        let name = o.map(|o| run.vm.field(o, "name").display().to_string()).filter(|n| !n.is_empty() && n != "undefined");
        name.or_else(|| e.get("script_friendname").map(str::to_owned)).or_else(|| Some(actor_name(&e.classname, id)))
    })();
    if let Ok(mut f) = AIMED_FRIEND.lock()
        && *f != found
    {
        *f = found;
    }
}

/// The player's SP guns as the loadout's slots: the first non-pistol, then
/// the first pistol (two at most, as CoD4 carries).
fn slot_order(guns: &[String]) -> Vec<String> {
    let pistol = |w: &String| ["usp", "beretta", "colt45", "deserteagle"].iter().any(|p| w.to_ascii_lowercase().starts_with(p));
    let usable: Vec<&String> = guns.iter().filter(|w| super::missions::sp_gun(w).is_some()).collect();
    let mut out: Vec<String> = usable.iter().filter(|w| !pistol(w)).take(1).map(|w| (*w).clone()).collect();
    out.extend(usable.iter().filter(|w| pistol(w)).take(1).map(|w| (*w).clone()));
    if out.len() < 2 {
        let more: Vec<String> = usable.iter().filter(|w| !out.contains(w)).take(2 - out.len()).map(|w| (*w).clone()).collect();
        out.extend(more);
    }
    out
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

/// The scripts' entities on screen ([`super::stage`]), and the player
/// carried on a tag with the link's look arcs.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn stage(
    mut commands: Commands,
    time: Res<Time>,
    mut script: NonSendMut<Script>,
    mut content: ResMut<crate::content::Content>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut bindposes: ResMut<Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>>,
    mut figures: Query<(&super::stage::StageEnt, &mut Transform, &mut Visibility, &crate::models::Skeleton, &mut crate::models::AnimPlayer), Without<LocalPlayer>>,
    globals: Query<&GlobalTransform>,
    mut player: Query<(Entity, &mut Transform, &mut crate::movement::ViewAngles, &crate::movement::Mover, Has<crate::movement::Frozen>), (With<LocalPlayer>, Without<super::stage::StageEnt>)>,
    mut shot: MessageReader<crate::combat::Damage>,
    hitboxes: Query<(Entity, &ChildOf), With<super::stage::StageHitbox>>,
    cams: Query<(&crate::splitscreen::SlotCamera, &GlobalTransform)>,
) {
    let Some(run) = script.running.as_mut() else { return };
    let now = (time.elapsed_secs() - run.t0) as f64;
    // Shots on the staged actors: the scripts' damage (the player's when
    // theirs).
    let me_ent = player.single().map(|p| p.0).ok();
    for d in shot.read() {
        if let Some(id) = run.stage.id_of(d.target) {
            let attacker = (d.attacker.is_some() && d.attacker == me_ent).then_some(PLAYER);
            let at = run.level.ent(id).map_or([0.0; 3], |e| [e.origin[0], e.origin[1], e.origin[2] + 48.0]);
            // The shot's way: from the shooter to it.
            let from = attacker.and_then(|a| run.level.ent(a)).map_or(at, |e| e.origin);
            let dir = [at[0] - from[0], at[1] - from[1], 0.0];
            use crate::combat::HitLocation as L;
            let location = match d.location {
                L::Head => "head",
                L::Neck => "neck",
                L::Torso => "torso_upper",
                L::TorsoLower => "torso_lower",
                L::Legs => "left_leg_upper",
                #[allow(unreachable_patterns)]
                _ => "torso_upper",
            };
            run.level.damaged_at(&mut run.vm, id, d.amount.round() as i32, attacker, dir, at, location);
        }
    }
    // The dead can't be shot again.
    for (hb, parent) in &hitboxes {
        if let Some(id) = run.stage.id_of(parent.parent())
            && run.level.ent(id).is_none_or(|e| !e.alive)
        {
            commands.entity(hb).try_despawn();
        }
    }
    // Every 5 s: how the scripts are doing.
    if (now / 5.0).floor() != ((now - time.delta_secs() as f64) / 5.0).floor() {
        let mut errs: Vec<&String> = run.vm.errors.iter().collect();
        errs.dedup();
        info!(
            "script: t={now:.0}s {} threads, {} errors, {} staged, link {:?}, missing {:?}",
            run.vm.live_threads(),
            run.vm.errors.len(),
            run.stage.count(),
            run.level.player_link.as_ref().map(|l| (l.to, l.tag.clone())),
            run.vm.missing.keys().take(12).collect::<Vec<_>>()
        );
        for e in errs.iter().take(6) {
            info!("script error: {e}");
        }
        for t in run.level.trace.drain(..) {
            info!("script note: {t}");
        }
        // Debug: `COD4RW_VEH_TRACE=1`: every vehicle, where, how it drives.
        if std::env::var_os("COD4RW_VEH_TRACE").is_some() {
            for e in run.level.ents.iter().filter(|e| e.kind == gsc::level::Kind::Vehicle) {
                info!("script vehicle {} {} at {:?} {:?}", e.id, e.model, e.origin.map(f32::round), run.level.vehicles.get(&e.id).map(|v| (v.moving, v.speed)));
            }
        }
        // Debug: `COD4RW_AI_TRACE=1`: every actor, where, and its anim.
        if std::env::var_os("COD4RW_AI_TRACE").is_some() {
            for e in run.level.ents.iter().filter(|e| e.kind == gsc::level::Kind::Ai) {
                let script = run.vm.entity_obj(e.id).map(|o| run.vm.field(o, "a")).and_then(|a| a.as_obj()).map(|a| run.vm.field(a, "script").display());
                info!(
                    "script ai {} {} {} at {:?} goal {:?} anim {:?} overlay {:?} animscript {:?}",
                    e.id,
                    e.classname,
                    e.model,
                    e.origin.map(f32::round),
                    e.goal.map(|g| g.map(f32::round)),
                    e.anim_state.anim.as_ref().map(|a| (a.name.clone(), a.frac(now))),
                    e.anim_state.overlay.as_ref().map(|a| a.name.clone()),
                    script
                );
            }
        }
        if let Ok(f) = std::env::var("COD4RW_THREADS") {
            for t in run.vm.thread_report().iter().filter(|t| t.contains(&f)).take(20) {
                info!("script thread: {t}");
            }
        }
        for e in run.level.ents.iter().filter(|e| e.model.contains("luxurysedan")) {
            info!(
                "script: car {} {:?} at {:?} anim {:?} tags {:?} wanted {:?}",
                e.id,
                e.kind,
                e.origin.map(|x| x.round()),
                e.anim_state.anim.as_ref().map(|a| (a.name.clone(), a.frac(now), a.scripted.is_some(), a.info.length, a.info.delta)),
                e.anim_state.tags.keys().collect::<Vec<_>>(),
                e.anim_state.wanted
            );
        }
        if let Some(l) = &run.level.player_link
            && let Some(to) = run.level.ent(l.to)
        {
            info!(
                "script: carried by {} {} at {:?} {:?} anim {:?}; tag {:?}",
                to.classname,
                to.model,
                to.origin.map(|x| x.round()),
                to.angles.map(|x| x.round()),
                to.anim_state.anim.as_ref().map(|a| (a.name.clone(), a.frac(now))),
                l.tag.as_ref().and_then(|t| to.anim_state.tags.get(t)).map(|(o, a)| (o.map(|x| x.round()), a.map(|x| x.round())))
            );
        }
    }
    let mut assets = super::stage::ModelAssets { meshes: &mut meshes, materials: &mut materials, images: &mut images, bindposes: &mut bindposes };
    // The actors' guns (their `weapon`), as world models.
    let mut weapons = HashMap::new();
    for e in run.level.ents.iter().filter(|e| e.kind == gsc::level::Kind::Ai) {
        let Some(w) = run.vm.entity_obj(e.id).and_then(|o| run.vm.field(o, "weapon").as_str().map(str::to_ascii_lowercase)) else { continue };
        if w.is_empty() || w == "none" {
            continue;
        }
        let model = run.weapon_models.entry(w.clone()).or_insert_with(|| crate::gunmodel::world_model_of(&content, &w));
        if let Some(m) = model {
            weapons.insert(e.id, m.clone());
        }
    }
    let mut anims = run.anims.borrow_mut();
    super::stage::sync(&mut commands, &mut run.level, &mut run.stage, &mut anims, &mut content, &mut assets, &mut figures, &globals, &weapons, now);
    drop(anims);
    let Ok((me, mut tf, mut view, mover, frozen)) = player.single_mut() else { return };
    // Debug: `COD4RW_CAMPAIGN_LOOKAI=1` (not carried): the view on the
    // nearest actor.
    // (`COD4RW_CAMPAIGN_FIRE=1`: on the nearest enemy, firing.)
    let fire = std::env::var_os("COD4RW_CAMPAIGN_FIRE").is_some();
    // (On a gun too, mounted or a vehicle's, or a script-given one like
    // the AC-130's, firing it.)
    let on_gun = run.level.player_link.as_ref().is_some_and(|l| l.turret)
        || run.level.player_current.as_deref().is_some_and(|w| super::missions::sp_gun(w).is_none());
    if (fire || std::env::var_os("COD4RW_CAMPAIGN_LOOKAI").is_some()) && (run.level.player_link.is_none() || on_gun) {
        let eye = tf.translation + Vec3::Y * crate::units::u(60.0);
        let near = run
            .level
            .ents
            .iter()
            .filter(|e| e.kind == gsc::level::Kind::Ai && e.alive && (!fire || e.team == "axis"))
            // (Firing: none with friendlies near, as a careful gunner.)
            .filter(|e| {
                !fire
                    || !run.level.ents.iter().any(|f| {
                        f.kind == gsc::level::Kind::Ai && f.alive && f.team == "allies" && (f.origin[0] - e.origin[0]).hypot(f.origin[1] - e.origin[1]) < 1100.0
                    })
            })
            .map(|e| crate::units::pos(e.origin) + Vec3::Y * crate::units::u(50.0))
            .min_by(|a, b| a.distance(eye).total_cmp(&b.distance(eye)))
            // (No enemy: a raised target, the killhouse's.)
            .or_else(|| {
                fire.then(|| run.level.damageable_points(&run.vm).into_iter().map(crate::units::pos).min_by(|a, b| a.distance(eye).total_cmp(&b.distance(eye))))
                    .flatten()
            });
        // (Debug: `COD4RW_CAMPAIGN_LOOKAT=x,y,z@from-to;...` looks there
        // instead, a test's throw into a room.)
        let look = std::env::var("COD4RW_CAMPAIGN_LOOKAT").ok().and_then(|v| {
            v.split(';').find_map(|item| {
                let (p, w) = item.split_once('@')?;
                let (a, b) = w.split_once('-')?;
                let (a, b) = (a.parse::<f64>().ok()?, b.parse::<f64>().ok()?);
                let c: Vec<f32> = p.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                (now >= a && now <= b && c.len() == 3).then(|| crate::units::pos([c[0], c[1], c[2]]))
            })
        });
        if let Some(t) = look.or(near) {
            let d = t - eye;
            view.yaw = (-d.x).atan2(-d.z);
            view.pitch = (d.y / Vec2::new(d.x, d.z).length().max(0.01)).atan();
        }
        // (Not with a friend in the way: CoD4 fails friendly fire.)
        let clear = near.is_none_or(|t| {
            let (a, ab) = (eye, t - eye);
            let len2 = ab.length_squared().max(1e-6);
            !run.level.ents.iter().filter(|e| e.kind == gsc::level::Kind::Ai && e.alive && e.team == "allies").any(|e| {
                let p = crate::units::pos(e.origin) + Vec3::Y * crate::units::u(40.0);
                let k = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
                (a + ab * k).distance(p) < crate::units::u(48.0)
            })
        });
        DEBUG_FIRE.store(fire && near.is_some() && clear && (now * 2.0).fract() < 0.6, std::sync::atomic::Ordering::Relaxed);
    }
    // Debug (`COD4RW_STAGE_TRACE`): where the player and the view are.
    if std::env::var_os("COD4RW_STAGE_TRACE").is_some() && (now * 2.0).floor() != ((now - 0.02) * 2.0).floor() {
        let cam = cams.iter().find(|c| c.0.0 == 0).map(|c| crate::units::to_cod(c.1.translation()).map(f32::round));
        info!("stage: player at {:?} eye {:.1} camera {cam:?}", crate::units::to_cod(tf.translation).map(f32::round), mover.eye_height / crate::units::u(1.0));
    }
    match super::stage::player_carry(&run.level) {
        Some((eye, base, arcs, _delta)) => {
            if !frozen {
                commands.entity(me).insert(crate::movement::Frozen);
            }
            // The player's origin (the feet) goes on the tag; the view is
            // its eye height above, as CoD4's linked view.
            // (A mounted gun's tag_player is the view itself, as CoD4's; a
            // vehicle's has the feet, VEH_LinkPlayer.)
            let on_gun = run.level.player_link.as_ref().and_then(|l| run.level.ent(l.to)).is_some_and(|e| e.classname == "misc_turret");
            let eye = if on_gun { [eye[0], eye[1], eye[2] - mover.eye_height / crate::units::u(1.0)] } else { eye };
            tf.translation = crate::units::pos(eye);
            // The view: the tag's facing turned by the player's own look
            // (what the mouse did since we last set it), within the link's
            // arcs; a new link starts looking along its tag.
            let link = run.level.player_link.as_ref().map(|l| (l.to, l.tag.clone()));
            if run.carry.link != link {
                run.carry = Carry { link, offset: Vec2::ZERO, set: None };
            }
            if let Some(set) = run.carry.set {
                run.carry.offset += Vec2::new(view.yaw - set.x, view.pitch - set.y);
            }
            let o = &mut run.carry.offset;
            o.x = o.x.clamp(-arcs[0].to_radians(), arcs[1].to_radians());
            o.y = o.y.clamp(-arcs[3].to_radians(), arcs[2].to_radians());
            view.yaw = crate::units::yaw_from_cod_degrees(base[1]) + o.x;
            view.pitch = -base[0].to_radians() + o.y;
            run.carry.set = Some(Vec2::new(view.yaw, view.pitch));
        }
        None => {
            // The link gone without an `unlink` (its carrier deleted, as
            // F.N.G.'s rope's `tag_origin`): the player is their own again.
            if run.carry.link.is_some() || run.carried_by.is_some() {
                run.carried_by = None;
                info!("campaign: the player's link is gone: free to move ({})", if frozen { "unfrozen" } else { "was free" });
                if frozen {
                    commands.entity(me).remove::<crate::movement::Frozen>();
                }
            }
            run.carry = Carry::default();
        }
    }
}

/// A vehicle's gun the player mans (`useby`, as Shock and Awe's Mk19): the
/// player's own weapon away; fire (mouse, or a pad's right trigger) fires
/// the vehicle's weapon at its rate along the view, its blast (its def's
/// effect, sound, radius and damage) where it hits; the vehicle told
/// `turret_fire`.
#[allow(clippy::too_many_arguments)]
/// The vehicle gun the player mans points where they look: its
/// `tag_turret` turned (yaw, then pitch) in its parent's axes, as CoD4
/// poses a vehicle's turret (`cg_pose`). (The view's own tag, tag_player,
/// hangs off the turret's base, so it doesn't turn with it.)
fn aim_manned_gun(
    script: NonSend<Script>,
    figures: Query<&crate::models::Skeleton>,
    mut joints: Query<&mut Transform>,
    parents: Query<&ChildOf>,
    globals: Query<&GlobalTransform>,
    cams: Query<(&crate::splitscreen::SlotCamera, &GlobalTransform)>,
) {
    let Some(run) = script.running.as_ref() else { return };
    let Some(link) = run.level.player_link.as_ref().filter(|l| l.turret) else { return };
    if run.level.ent(link.to).is_none_or(|e| e.classname == "misc_turret") {
        return;
    }
    let Some(skel) = run.stage.entity(link.to).and_then(|b| figures.get(b).ok()) else { return };
    let Some(joint) = skel.joint("tag_turret") else { return };
    let Some((_, cam)) = cams.iter().find(|c| c.0.0 == 0) else { return };
    let Some(parent) = parents.get(joint).ok().and_then(|p| globals.get(p.parent()).ok()) else { return };
    // The view's way in the parent's axes (a CoD tag's forward is its x).
    let d = (parent.rotation().inverse() * *cam.forward()).normalize_or_zero();
    let yaw = (-d.z).atan2(d.x);
    let pitch = d.y.clamp(-1.0, 1.0).asin();
    if let Ok(mut tf) = joints.get_mut(joint) {
        tf.rotation = Quat::from_rotation_y(yaw) * Quat::from_rotation_z(pitch);
    }
}

/// The player as the scripts ask after them: aimed down the sights, their
/// stance, the commands held (`playerADS()`, `GetStance()`,
/// `attackButtonPressed()`...). (Debug: `COD4RW_CAMPAIGN_ADS=<from s>[-<to s>]`
/// holds the sights up then; `COD4RW_CAMPAIGN_STANCE=<stance>@<s>,...`.)
fn player_state(
    time: Res<Time>,
    mut script: NonSendMut<Script>,
    me: Query<(Entity, &crate::weapons::WeaponState, &crate::movement::Mover, &crate::splitscreen::PlayerInput, Option<&crate::loadout::Loadout>), With<LocalPlayer>>,
    mut shots: MessageReader<crate::weapons::BulletPath>,
    mut nades: Query<&mut crate::grenades::Grenades, With<LocalPlayer>>,
    mut switch: Query<&mut crate::loadout::SwitchInput, With<LocalPlayer>>,
) {
    let Some(run) = script.running.as_mut() else { return };
    let Ok((me_ent, weapon, mover, input, loadout)) = me.single() else { return };
    // The gun the scripts put in hand, once the loadout's made: switched to.
    if let Some(l) = loadout
        && !run.guns_changed
        && let Some(w) = run.want_in_hand.take()
        && let Some(i) = slot_order(&run.guns).iter().position(|g| *g == w)
        && i != l.current
        && let Ok(mut s) = switch.single_mut()
    {
        s.to = Some(i);
    }
    // Their grenades are the scripts' (picked up, given, thrown).
    if !run.level.player_weapons.is_empty()
        && let Ok(mut g) = nades.single_mut()
    {
        let stock = |w: &str| if run.level.player_weapons.iter().any(|x| x == w) { run.level.player_stock.get(w).copied().unwrap_or(0).max(0) as u32 } else { 0 };
        let frags = stock("fraggrenade");
        let special = [("flash_grenade", crate::grenades::Kind::Flash), ("concussion_grenade", crate::grenades::Kind::Stun), ("smoke_grenade_american", crate::grenades::Kind::Smoke)]
            .into_iter()
            .find(|(w, _)| run.level.player_weapons.iter().any(|x| x == w));
        if g.frags != frags {
            g.frags = frags;
        }
        if let Some((w, kind)) = special {
            let n = stock(w);
            if g.special != Some(kind) || g.specials != n {
                g.special = Some(kind);
                g.specials = n;
            }
        }
    }
    // The gun in their hands (switched with the game's keys) is the
    // scripts' `getCurrentWeapon()`.
    if let Some(l) = loadout
        && !run.guns_changed
    {
        let held = slot_order(&run.guns);
        if let Some(w) = held.get(l.current)
            && run.level.player_weapons.contains(w)
        {
            run.level.player_current = Some((*w).clone());
        }
    }
    // Their bullets on the scripts' damageable brushes (the killhouse's
    // targets), along their whole way (through plywood) and a little past
    // where they stopped (into the brush they stopped at).
    // Every bullet suppresses the other side's actors it passes close to.
    for shot in shots.read().filter(|s| s.end != s.from) {
        let (from, end) = (crate::units::to_cod(shot.from), crate::units::to_cod(shot.end));
        if shot.shooter != me_ent {
            let team = run.pawns.iter().find(|(_, e)| **e == shot.shooter).and_then(|(id, _)| run.level.ent(*id)).map(|e| e.team.clone());
            if let Some(team) = team {
                run.level.bullet_passed(from, end, &team);
            }
            continue;
        }
        run.level.bullet_passed(from, end, "allies");
        let dir = (shot.end - shot.from).normalize_or_zero();
        let to = shot.end + dir * crate::units::u(24.0);
        let amount = weapon.def.damage as i32;
        run.level.player_shot(&mut run.vm, from, crate::units::to_cod(to), amount);
    }
    let now = (time.elapsed_secs() - run.t0) as f64;
    let mut ads = weapon.ads;
    // (`COD4RW_CAMPAIGN_ADS=<from>-<to>[,<from>-<to>...]`.)
    let mut debug_ads = false;
    if let Ok(v) = std::env::var("COD4RW_CAMPAIGN_ADS") {
        for span in v.split(',') {
            let mut it = span.split('-').filter_map(|x| x.parse::<f64>().ok());
            let (from, to) = (it.next().unwrap_or(0.0), it.next().unwrap_or(f64::MAX));
            if now >= from && now <= to {
                debug_ads = true;
                ads = ads.max(0.6);
            }
        }
    }
    DEBUG_ADS.store(debug_ads, std::sync::atomic::Ordering::Relaxed);
    // (A throw: the key held 0.3 s, then let go.)
    let mut throw = 0;
    if let Ok(v) = std::env::var("COD4RW_CAMPAIGN_THROW") {
        for item in v.split(',') {
            if let Some((k, t)) = item.split_once('@')
                && let Ok(t) = t.parse::<f64>()
                && now >= t
                && now < t + 0.3
            {
                throw = if k == "frag" { 1 } else { 2 };
            }
        }
    }
    DEBUG_THROW.store(throw, std::sync::atomic::Ordering::Relaxed);
    run.level.player_ads = ads;
    let mut stance = match mover.stance {
        crate::movement::Stance::Stand => "stand",
        crate::movement::Stance::Crouch => "crouch",
        crate::movement::Stance::Prone => "prone",
    };
    if let Ok(v) = std::env::var("COD4RW_CAMPAIGN_STANCE") {
        for item in v.split(',') {
            if let Some((st, t)) = item.split_once('@')
                && t.parse::<f64>().is_ok_and(|t| now >= t)
            {
                stance = match st {
                    "crouch" => "crouch",
                    "prone" => "prone",
                    _ => "stand",
                };
            }
        }
    }
    run.level.player_stance = stance;
    let mut held = Vec::new();
    if input.mouse.pressed(MouseButton::Left) || std::env::var_os("COD4RW_CAMPAIGN_FIRE").is_some() {
        held.push("+attack".to_owned());
    }
    if input.mouse.pressed(MouseButton::Right) || ads > 0.5 {
        held.push("+speed_throw".to_owned());
    }
    for (k, c) in [(KeyCode::KeyF, "+activate"), (KeyCode::KeyV, "+melee"), (KeyCode::KeyG, "+frag"), (KeyCode::KeyQ, "+smoke")] {
        if input.keys.pressed(k) {
            held.push(c.to_owned());
        }
    }
    run.level.player_held = held;
}

/// The scripts' brush models where the level has them now (their
/// `moveto`, `rotatepitch`, links): drawn there, solid there, hidden or not
/// solid as told.
fn move_brushes(
    mut commands: Commands,
    script: NonSend<Script>,
    mut parts: Query<(Entity, &crate::world::ScriptBrushPart, &mut Transform, Option<&mut Visibility>, Has<avian3d::prelude::Collider>, Has<avian3d::prelude::ColliderDisabled>)>,
) {
    let Some(run) = script.running.as_ref() else { return };
    let mut at: HashMap<usize, ([f32; 3], [f32; 3], bool, bool)> = HashMap::new();
    for e in &run.level.ents {
        if let Some(n) = e.model.strip_prefix('*').and_then(|n| n.parse::<usize>().ok()) {
            at.insert(n, (e.origin, e.angles, e.hidden, e.solid));
        }
    }
    // (Debug: `COD4RW_BRUSH_TRACE=<n>`: brush model *n's place, each second.)
    let trace = std::env::var("COD4RW_BRUSH_TRACE").ok().and_then(|v| v.parse::<usize>().ok());
    if let Some(n) = trace
        && (run.level.time * 1.0).floor() != ((run.level.time - 0.02) * 1.0).floor()
    {
        let all: Vec<_> = run.level.ents.iter().filter(|e| e.model == format!("*{n}")).map(|e| (e.id, e.classname.clone(), e.angles, e.move_rot.is_some())).collect();
        info!("brush *{n}: level {:?}, parts {}, ents {all:?}", at.get(&n), parts.iter().filter(|p| p.1.0 == n).count());
    }
    for (entity, part, mut tf, vis, collider, disabled) in &mut parts {
        // (Deleted: gone from view and in nobody's way.)
        let Some(&(origin, angles, hidden, solid)) = at.get(&part.0) else {
            if let Some(mut v) = vis {
                v.set_if_neq(Visibility::Hidden);
            }
            if collider && !disabled {
                commands.entity(entity).insert(avian3d::prelude::ColliderDisabled);
            }
            continue;
        };
        let place = Transform::from_translation(crate::units::pos(origin)).with_rotation(crate::modes::koth::cod_rotation(angles));
        if tf.translation != place.translation || tf.rotation != place.rotation {
            *tf = place;
        }
        if let Some(mut v) = vis {
            v.set_if_neq(if hidden { Visibility::Hidden } else { Visibility::Inherited });
        }
        if collider {
            match (solid, disabled) {
                (false, false) => {
                    commands.entity(entity).insert(avian3d::prelude::ColliderDisabled);
                }
                (true, true) => {
                    commands.entity(entity).remove::<avian3d::prelude::ColliderDisabled>();
                }
                _ => {}
            }
        }
    }
}

/// The player's grenades, as the scripts see them: thrown (`grenade_fire`),
/// where they are, and gone off (`death`).
fn player_grenades(
    mut script: NonSendMut<Script>,
    me: Query<Entity, With<LocalPlayer>>,
    live: Query<(Entity, &crate::grenades::LiveGrenade, &Transform)>,
) {
    let Some(run) = script.running.as_mut() else { return };
    let Ok(me) = me.single() else { return };
    for (e, g, tf) in &live {
        if g.thrower != me {
            continue;
        }
        let at = crate::units::to_cod(tf.translation);
        match run.grenades.get(&e) {
            Some(&id) => run.level.grenade_moved(id, at),
            None => {
                let weapon = match g.kind {
                    crate::grenades::Kind::Frag => "fraggrenade",
                    crate::grenades::Kind::Flash => "flash_grenade",
                    crate::grenades::Kind::Stun => "concussion_grenade",
                    crate::grenades::Kind::Smoke => "smoke_grenade_american",
                };
                let id = run.level.player_grenade(&mut run.vm, weapon, at);
                run.grenades.insert(e, id);
            }
        }
    }
    let gone: Vec<(Entity, EntId)> = run.grenades.iter().filter(|(e, _)| live.get(**e).is_err()).map(|(e, id)| (*e, *id)).collect();
    for (e, id) in gone {
        run.grenades.remove(&e);
        run.level.grenade_gone(&mut run.vm, id);
    }
}

/// A shell of a script-given gun on its way down.
struct Shell {
    at: f64,
    pos: Vec3,
    weapon: String,
    effect: String,
    sound: Option<String>,
    radius: f32,
    most: f32,
    least: f32,
}

/// A weapon the scripts gave the player that isn't one of our rifles (the
/// AC-130's 25, 40 and 105 mm): no viewmodel of its own; fire (mouse, a
/// pad's right trigger) shoots it from the view at its rate while its clip
/// (the scripts' `setweaponammoclip`) has a round, the player told
/// `weapon_fired`. Its shell lands after its flight (the def's
/// `projectileSpeed`): its effect, sound and radius damage, and the player
/// told `projectile_impact` (weapon, where, radius). The scripts' `cg_fov`
/// is the view's too.
#[allow(clippy::too_many_arguments)]
fn script_gun(
    time: Res<Time>,
    mut script: NonSendMut<Script>,
    content: Res<crate::content::Content>,
    mouse: Res<ButtonInput<MouseButton>>,
    pads: Query<&Gamepad>,
    cams: Query<(&crate::splitscreen::SlotCamera, &GlobalTransform)>,
    spatial: avian3d::prelude::SpatialQuery,
    mut effects: Option<ResMut<crate::fx::Effects>>,
    mut sfx: ResMut<crate::audio::Sfx>,
    (hitboxes, me): (Query<&crate::combat::Hitbox>, Query<Entity, With<LocalPlayer>>),
) {
    let Some(run) = script.running.as_mut() else { return };
    let now = (time.elapsed_secs() - run.t0) as f64;
    let fov = run.level.dvars.get("cg_fov").and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0);
    crate::settings_apply::SCRIPT_FOV.store(fov.to_bits(), std::sync::atomic::Ordering::Relaxed);
    // Shells landing.
    let (landed, flying): (Vec<Shell>, Vec<Shell>) = std::mem::take(&mut run.shells).into_iter().partition(|s| s.at <= now);
    run.shells = flying;
    for shell in landed {
        if let Some(fxs) = effects.as_mut() {
            fxs.play(&shell.effect, crate::fx::Anchor::Fixed(crate::fx::Frame::facing(shell.pos, Vec3::Y, 0.0)), crate::fx::FxLayer::World);
        }
        if let Some(s) = &shell.sound {
            sfx.play(s.clone(), Some(shell.pos));
        }
        let at = crate::units::to_cod(shell.pos);
        if shell.radius > 0.0 {
            run.level.blast(&mut run.vm, at, shell.radius, shell.most, shell.least);
        }
        if std::env::var_os("COD4RW_CAMPAIGN_FIRE").is_some() {
            info!("campaign: {} landed at {:?} (radius {})", shell.weapon, at.map(f32::round), shell.radius);
        }
        if let Some(o) = run.vm.entity_obj(PLAYER) {
            run.vm.notify(o, "projectile_impact", vec![Value::str(&shell.weapon), Value::Vector(at), Value::Float(shell.radius)]);
        }
    }
    let Some(weapon) = run.level.player_current.clone() else { return };
    if super::missions::sp_gun(&weapon).is_some() {
        return;
    }
    let Some((zi, w)) = content.generic(iw3::zone::AssetType::Weapon, &weapon) else { return };
    let firing = mouse.pressed(MouseButton::Left) || pads.iter().any(|p| p.pressed(GamepadButton::RightTrigger2)) || std::env::var_os("COD4RW_CAMPAIGN_FIRE").is_some();
    let ammo = run.level.player_ammo.get(&weapon).copied().unwrap_or(0);
    if std::env::var_os("COD4RW_CAMPAIGN_FIRE").is_some() && (now * 1.0).floor() != ((now - 0.02) * 1.0).floor() {
        info!("campaign: holding {weapon}, clip {ammo}");
    }
    if !firing || now < run.gun_next || ammo <= 0 {
        return;
    }
    let zone = &content.zones[zi];
    let asset = |f: &str| w.asset(f).map(|i| zone.get(i).name().trim_start_matches(',').to_owned());
    let sound = |f: &str| w.node(f).and_then(|n| n.node("name")).and_then(|n| n.string("soundName")).map(str::to_owned);
    let fire_time = w.float("fireTime").max(0.05);
    run.gun_next = now + fire_time as f64;
    run.level.player_ammo.insert(weapon.clone(), ammo - 1);
    if let Some(s) = sound("fireSoundPlayer").or_else(|| sound("fireSound")) {
        sfx.play(s, None);
    }
    if let Some(o) = run.vm.entity_obj(PLAYER) {
        run.vm.notify(o, "weapon_fired", Vec::new());
    }
    let Some((_, cam)) = cams.iter().find(|c| c.0.0 == 0) else { return };
    let from = cam.translation();
    let dir = cam.forward();
    let reach = crate::units::u(60000.0);
    // (Not the player's own hitboxes.)
    let shooter = me.single().ok();
    let not_me = |e: Entity| hitboxes.get(e).map_or(true, |h| Some(h.owner) != shooter);
    let ray = spatial.cast_ray_predicate(from, dir, reach, true, &crate::collision::bullet_filter(), &not_me);
    let dist = ray.map_or(reach, |h| h.distance);
    let speed = crate::units::u(w.int("projectileSpeed").max(1) as f32);
    let flight = if w.int("projectileSpeed") > 0 { (dist / speed) as f64 } else { 0.0 };
    run.shells.push(Shell {
        at: now + flight,
        pos: from + *dir * dist,
        weapon,
        effect: asset("projExplosionEffect").unwrap_or_else(|| "explosions/grenadeexp_default".into()),
        sound: sound("projExplosionSound"),
        radius: w.int("iExplosionRadius") as f32,
        most: w.int("iExplosionInnerDamage") as f32,
        least: w.int("iExplosionOuterDamage") as f32,
    });
}

fn turret(
    time: Res<Time>,
    mut script: NonSendMut<Script>,
    content: Res<crate::content::Content>,
    mouse: Res<ButtonInput<MouseButton>>,
    pads: Query<&Gamepad>,
    cams: Query<(&crate::splitscreen::SlotCamera, &GlobalTransform)>,
    spatial: avian3d::prelude::SpatialQuery,
    mut effects: Option<ResMut<crate::fx::Effects>>,
    mut sfx: ResMut<crate::audio::Sfx>,
    (hitboxes, mut damage, me): (Query<&crate::combat::Hitbox>, MessageWriter<crate::combat::Damage>, Query<Entity, With<LocalPlayer>>),
) {
    let Some(run) = script.running.as_mut() else { return };
    let now = (time.elapsed_secs() - run.t0) as f64;
    let Some((vehicle, weapon)) = run.level.player_turret(&run.vm) else {
        // Off it: armed as before.
        if let Some(was) = run.turret_on.take() {
            super::set_unarmed(was);
        }
        return;
    };
    if run.turret_on.is_none() {
        run.turret_on = Some(super::unarmed());
        super::set_unarmed(true);
        info!("campaign: the player mans {weapon} on vehicle {vehicle}");
    }
    // A mounted gun turns with the view.
    if let Some((_, cam)) = cams.iter().find(|c| c.0.0 == 0)
        && run.level.ent(vehicle).is_some_and(|e| e.classname == "misc_turret")
    {
        let f = crate::units::to_cod(crate::units::pos([0.0; 3]) + *cam.forward());
        let yaw = f[1].atan2(f[0]).to_degrees();
        let pitch = -f[2].atan2(f[0].hypot(f[1])).to_degrees();
        if let Some(e) = run.level.ent_mut(vehicle) {
            e.angles = [pitch, yaw, 0.0];
        }
    }
    // (Debug: `COD4RW_CAMPAIGN_FIRE=1` holds it down.)
    let firing = mouse.pressed(MouseButton::Left) || pads.iter().any(|p| p.pressed(GamepadButton::RightTrigger2)) || std::env::var_os("COD4RW_CAMPAIGN_FIRE").is_some();
    if !firing || now < run.turret_next {
        return;
    }
    let fire_time = run.level.weapons.get(&weapon).map_or(0.25, |w| w.fire_time.max(0.05));
    run.turret_next = now + fire_time as f64;
    run.level.turret_fired(&mut run.vm, vehicle);
    let Some((_, cam)) = cams.iter().find(|c| c.0.0 == 0) else { return };
    let from = cam.translation();
    let dir = cam.forward();
    let reach = crate::units::u(8000.0);
    // (Not the player's own hitboxes: the body sits under the view.)
    let shooter = me.single().ok();
    let not_me = |e: Entity| hitboxes.get(e).map_or(true, |h| Some(h.owner) != shooter);
    let ray = spatial.cast_ray_predicate(from, dir, reach, true, &crate::collision::bullet_filter(), &not_me);
    let hit = ray.map_or(from + *dir * reach, |h| from + *dir * h.distance);
    let struck = ray.and_then(|h| hitboxes.get(h.entity).ok().copied());
    // Its def: blast, effect, sounds.
    let def = content.generic(iw3::zone::AssetType::Weapon, &weapon).map(|(zi, w)| {
        let zone = &content.zones[zi];
        let asset = |f: &str| w.asset(f).map(|i| zone.get(i).name().trim_start_matches(',').to_owned());
        let sound = |f: &str| w.node(f).and_then(|n| n.node("name")).and_then(|n| n.string("soundName")).map(str::to_owned);
        (
            asset("projExplosionEffect").unwrap_or_else(|| "explosions/grenadeexp_default".into()),
            sound("projExplosionSound").or_else(|| sound("fireSoundPlayer")),
            sound("fireSoundPlayer").or_else(|| sound("fireSound")),
            w.int("iExplosionRadius") as f32,
            w.int("iExplosionInnerDamage") as f32,
            w.int("iExplosionOuterDamage") as f32,
            w.int("damage") as f32,
            asset("viewFlashEffect"),
        )
    });
    let Some((effect, blast_sound, fire_sound, radius, inner, outer, bullet, _flash)) = def else { return };
    // A gun's bullet: whoever it struck is hurt.
    if radius <= 0.0 {
        if let Some(s) = fire_sound {
            sfx.play(s, None);
        }
        if let Some(hb) = struck {
            if std::env::var_os("COD4RW_CAMPAIGN_FIRE").is_some() {
                let who = run.stage.id_of(hb.owner).and_then(|id| run.level.ent(id)).map(|e| (e.id, e.team.clone(), e.health, e.classname.clone()));
                info!("campaign: {weapon} struck {who:?} ({:?})", hb.location);
            }
            damage.write(crate::combat::Damage { target: hb.owner, attacker: me.single().ok(), amount: bullet.max(20.0), location: hb.location, weapon: "saw_mp" });
        }
        return;
    }
    if let Some(s) = fire_sound {
        sfx.play(s, None);
    }
    if let Some(fxs) = effects.as_mut() {
        let frame = crate::fx::Frame::facing(hit, -*dir, 0.0);
        fxs.play(&effect, crate::fx::Anchor::Fixed(frame), crate::fx::FxLayer::World);
    }
    if let Some(s) = blast_sound {
        sfx.play(s, Some(hit));
    }
    if radius > 0.0 {
        run.level.blast(&mut run.vm, crate::units::to_cod(hit), radius, inner.max(outer), outer.min(inner));
    }
}

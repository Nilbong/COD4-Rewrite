//! Gameplay recorder: log a pawn's view, inputs and state every frame
//! together with the enemy nearest its crosshair, so a real player's habits
//! can be measured (`tools/fit_player.py`) and bots made to play like them.
//!
//! Real play is recorded on its own, for training bots to play like the
//! player (`tools/learn`): every match without a `COD4RW_*` debug variable
//! writes `%LOCALAPPDATA%\cod4rw\recordings\<map>-<unix time>.csv`.
//! `--record` writes `recordings/<map>-<unix time>.csv` for the local player;
//! `COD4RW_RECORD=<file.csv>` writes to a given file, and with
//! `COD4RW_RECORD_WHO=<bot name>` records a bot instead (for checking the
//! fitting tools against known parameters), or with `all` every bot, each to
//! `<file>-<name>.csv`.
//!
//! Columns: time, yaw and pitch (degrees), fire, ads, forward, right; the
//! enemy in view nearest the crosshair: id, its chest's yaw and pitch
//! (degrees) and distance (CoD units), empty when none is visible; feet
//! position (CoD units), stance (0 stand, 1 crouch, 2 prone), sprint, jump,
//! on ground, health, dead, reloading; this frame's events: shots fired,
//! hits, headshots, kills; and team (CoD4's numbers: 1 axis, 2 allies);
//! for bots, what it's doing (mode); then its own id, the weapon held, the
//! enemies it has line of sight to (ids, `;` between, whichever way it
//! faces: as `tools/demo_traits.py` measures real players), its motive, and
//! how far its sights are up (0..1, the weapon's own easing; `ads` is the
//! button), and how far it can see along its view (CoD units to the first
//! world surface, 8192 for open sky: a floor or wall stare is a short one).
//! Recording every bot also writes `<file>-personas.csv`: each bot's real
//! player (`bots::persona`), style and class. When the match ends, each
//! file gets `<file>.match.txt` beside it (`key=value`: map, mode, length,
//! players a side, scores): no such file means the match didn't finish
//! (quit early), so tools can keep to whole matches.

use crate::collision;
use crate::combat::{Damage, Dead, Health, HitLocation, Killed, Pawn};
use crate::movement::{MoveInput, Mover, Stance, ViewAngles};
use crate::player::LocalPlayer;
use crate::units::u;
use crate::weapons::{ShotFired, WeaponInput, WeaponState};
use avian3d::prelude::*;
use bevy::prelude::*;
use std::io::Write;
use std::path::PathBuf;

/// `--record`, set before the plugins build.
#[derive(Resource, Default)]
pub struct RecordArg(pub bool);

#[derive(Resource)]
pub struct Recorder {
    /// Where to write; files are created on the first in-game frame.
    path: Option<PathBuf>,
    /// Where a match's file goes when no file is given.
    dir: PathBuf,
    files: Vec<(Entity, std::io::BufWriter<std::fs::File>)>,
    /// Each file's path, for the match summary beside it.
    paths: Vec<PathBuf>,
    summary_written: bool,
    /// The local player's kills and deaths this match (for the balancer's
    /// rating of them, [`super::lobby::human_rating`]), and whether it was
    /// dead last frame.
    local_kd: (u32, u32, bool),
    /// Real play (no file asked for): a row every 1/20 s, not every frame,
    /// with the shots, hits, headshots and kills since the last summed in.
    next_row: f32,
    pending: (usize, u32, u32, usize),
    who: Option<String>,
    started: bool,
}

const HEADER: &str = "t,yaw,pitch,fire,ads,fwd,right,enemy,eyaw,epitch,edist,\
x,y,z,stance,sprint,jump,ground,health,dead,reloading,shots,hits,heads,kills,team,mode,id,weapon,vis,motive,ads_level,view_dist";

pub fn setup(app: &mut App) {
    let flag = app.world().get_resource::<RecordArg>().is_some_and(|a| a.0);
    let path = std::env::var("COD4RW_RECORD").ok().map(PathBuf::from);
    // Real play (no debug variables): kept with the player's data.
    let auto = !flag && path.is_none() && !debug_run();
    let dir = match auto.then(data_dir).flatten() {
        Some(d) => d,
        None if flag || path.is_some() => PathBuf::from("recordings"),
        None => return,
    };
    app.insert_resource(Recorder { path, dir, files: Vec::new(), paths: Vec::new(), summary_written: false, local_kd: (0, 0, false), next_row: 0.0, pending: (0, 0, 0, 0), who: std::env::var("COD4RW_RECORD_WHO").ok(), started: false })
        .add_systems(Update, record.after(crate::weapons::WeaponSet).run_if(crate::state::in_game))
        .add_systems(OnExit(crate::state::GameState::InGame), finish);
}

/// Out of the match: close its files, so the next match gets its own (the
/// recorder used to start once a session: on the Headquarters hub or the
/// menu's map, and never again, so no real match was recorded).
fn finish(mut rec: ResMut<Recorder>) {
    for (_, f) in &mut rec.files {
        f.flush().ok();
    }
    rec.files.clear();
    rec.paths.clear();
    rec.started = false;
    rec.summary_written = false;
    rec.local_kd = (0, 0, false);
}

/// The file a match is recorded to: one per match, named after the map.
fn default_path(dir: &std::path::Path, map: &str) -> PathBuf {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    dir.join(format!("{map}-{secs}.csv"))
}

/// Debug and test runs set `COD4RW_*` variables (the network settings
/// aside): they aren't real play.
fn debug_run() -> bool {
    std::env::vars().any(|(k, _)| k.starts_with("COD4RW_") && !crate::net::setting(&k))
}

/// `%LOCALAPPDATA%\cod4rw\recordings` (or the platform's equivalent).
fn data_dir() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_DATA_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(base.join("cod4rw").join("recordings"))
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn record(
    time: Res<Time>,
    mut rec: ResMut<Recorder>,
    map: Option<Res<crate::world::MapName>>,
    spatial: SpatialQuery,
    pawns: Query<(
        Entity,
        &Pawn,
        &Transform,
        &Mover,
        &ViewAngles,
        Option<&WeaponInput>,
        Option<&MoveInput>,
        Has<LocalPlayer>,
        Has<Dead>,
    )>,
    state: Query<(&Health, &WeaponState)>,
    bots: Query<&super::Bot>,
    mut shots: MessageReader<ShotFired>,
    mut damage: MessageReader<Damage>,
    mut killed: MessageReader<Killed>,
    match_state: Option<Res<crate::tdm::MatchState>>,
) {
    let rec = &mut *rec;
    // The match over: say so beside each file, with who played.
    if let Some(st) = match_state.as_ref().filter(|s| s.ended.is_some() && !rec.summary_written && !rec.files.is_empty()) {
        rec.summary_written = true;
        let side = |t: crate::combat::Team| pawns.iter().filter(|p| p.1.team == t).count();
        let (allies, axis) = (side(crate::combat::Team::Allies), side(crate::combat::Team::Axis));
        let summary = format!(
            "map={}
mode={:?}
ended=1
length={:.0}
allies={allies}
axis={axis}
score_allies={}
score_axis={}
score_limit={}
time_limit={:.0}
",
            map.as_ref().map_or("?", |m| m.0.as_str()),
            st.mode,
            time.elapsed_secs() - st.started,
            st.allies,
            st.axis,
            st.score_limit,
            st.time_limit,
        );
        // Real play only: how the player did, for the next match's balance.
        if rec.who.is_none() && rec.path.is_none() {
            super::lobby::rate_human(rec.local_kd.0, rec.local_kd.1);
        }
        for ((_, f), path) in rec.files.iter_mut().zip(&rec.paths) {
            f.flush().ok();
            std::fs::write(path.with_extension("match.txt"), &summary).ok();
        }
    }
    // Not a match (the hub, the campaign, photo mode), or nobody to play
    // against yet: wait.
    if !rec.started && (crate::hq::active() || crate::campaign::active() || crate::photo::active() || bots.is_empty()) {
        return;
    }
    if !rec.started {
        rec.started = true;
        let all = rec.who.as_deref().is_some_and(|w| w.eq_ignore_ascii_case("all"));
        let base = rec.path.clone().unwrap_or_else(|| default_path(&rec.dir, map.as_ref().map_or("map", |m| m.0.as_str())));
        if all {
            let stem = base.file_stem().map_or("bots".into(), |s| s.to_string_lossy());
            let mut sheet = String::from("name,id,team,source,style,rank,prestige,knowledge,skill,class
");
            for (e, pawn, ..) in pawns.iter().filter(|p| bots.contains(p.0)) {
                let Ok(bot) = bots.get(e) else { continue };
                let (source, style, knows) = bot.persona.as_ref().map_or(("", "", 0.0), |pp| (pp.source.as_str(), pp.style(), pp.knowledge()));
                let class: Vec<&str> = bot.class.guns.iter().map(|g| g.spec.as_str()).chain(bot.class.perks.iter().map(|p| p.as_str())).collect();
                sheet += &format!(
                    "{},{},{},{source},{style},{},{},{knows:.2},{:.2},{}
",
                    pawn.name.replace(',', " "),
                    e.index(),
                    match pawn.team {
                        crate::combat::Team::Axis => 1,
                        crate::combat::Team::Allies => 2,
                    },
                    bot.rank.0,
                    bot.rank.1,
                    bot.skill,
                    class.join("+")
                );
            }
            std::fs::write(base.with_file_name(format!("{stem}-personas.csv")), sheet).ok();
        }
        for (e, pawn, ..) in pawns.iter().filter(|p| match &rec.who {
            Some(_) if all => bots.contains(p.0),
            Some(name) => p.1.name.eq_ignore_ascii_case(name),
            None => p.7,
        }) {
            let path = if all {
                let stem = base.file_stem().map_or("bots".into(), |s| s.to_string_lossy());
                base.with_file_name(format!("{stem}-{}.csv", pawn.name.replace(|c: char| !c.is_alphanumeric(), "_")))
            } else {
                base.clone()
            };
            if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
                std::fs::create_dir_all(dir).ok();
            }
            match std::fs::File::create(&path) {
                Ok(f) => {
                    let mut f = std::io::BufWriter::new(f);
                    writeln!(f, "{HEADER}").ok();
                    rec.files.push((e, f));
                    rec.paths.push(path.clone());
                    info!("recording {} to {}", pawn.name, path.display());
                    // For checking the fitting tools: the recorded bot's true parameters.
                    if let Ok(bot) = bots.get(e) {
                        info!(
                            "recording bot {} skill {:.2} reaction {:.3}s aim {:?} personality {:?} style {:?}",
                            pawn.name, bot.skill, bot.reaction, bot.aim_profile, bot.personality, bot.style
                        );
                    }
                }
                Err(e) => warn!("can't record to {}: {e}", path.display()),
            }
        }
    }
    if rec.files.is_empty() {
        return;
    }
    let shots: Vec<Entity> = shots.read().map(|s| s.shooter).collect();
    let damage: Vec<(Entity, Entity, bool)> = damage
        .read()
        .filter_map(|d| d.attacker.map(|a| (a, d.target, matches!(d.location, HitLocation::Head))))
        .collect();
    let killed: Vec<(Entity, Entity)> = killed.read().filter_map(|k| k.attacker.map(|a| (a, k.victim))).collect();
    // The local player's pawn replaced (a new one on respawn, a class
    // change): carry on with the new one in the same file (it stopped
    // there, at the first death).
    let local_now = pawns.iter().find(|p| p.7).map(|p| p.0);
    let local_file = rec.who.is_none();
    let throttle = local_file && rec.path.is_none();
    let now = time.elapsed_secs();
    let write_now = !throttle || now >= rec.next_row;
    if write_now && throttle {
        rec.next_row = now + 0.05;
    }
    let rec_kd = &mut rec.local_kd;
    let pending = &mut rec.pending;
    for (me_entity, file) in &mut rec.files {
        if local_file && pawns.get(*me_entity).is_err() {
            if let Some(e) = local_now {
                *me_entity = e;
            }
        }
        let Ok(me) = pawns.get(*me_entity) else { continue };
        let me_entity = *me_entity;
        let shots_fired = shots.iter().filter(|&&s| s == me_entity).count();
        let mine = damage.iter().filter(|d| d.0 == me_entity && d.1 != me_entity);
        let (hits, heads) = mine.fold((0, 0), |(h, hd), d| (h + 1, hd + d.2 as u32));
        let kills = killed.iter().filter(|k| k.0 == me_entity && k.1 != me_entity).count();
        if local_file {
            let dead = me.8;
            rec_kd.0 += kills as u32;
            rec_kd.1 += (dead && !rec_kd.2) as u32;
            rec_kd.2 = dead;
        }
        let (shots_fired, hits, heads, kills) = if throttle {
            pending.0 += shots_fired;
            pending.1 += hits;
            pending.2 += heads;
            pending.3 += kills;
            if !write_now {
                continue;
            }
            std::mem::take(pending)
        } else {
            (shots_fired, hits, heads, kills)
        };
        let mode = bots.get(me_entity).map_or(String::new(), |b| format!("{:?},{}/{}", b.mode, b.temper.motive.name(), b.look_src));
        write_row(file, &time, &spatial, &pawns, &state, me, (shots_fired, hits, heads, kills), &mode);
    }
}

type PawnItem<'a> = (
    Entity,
    &'a Pawn,
    &'a Transform,
    &'a Mover,
    &'a ViewAngles,
    Option<&'a WeaponInput>,
    Option<&'a MoveInput>,
    bool,
    bool,
);

#[allow(clippy::type_complexity)]
fn write_row(
    file: &mut std::io::BufWriter<std::fs::File>,
    time: &Time,
    spatial: &SpatialQuery,
    pawns: &Query<(
        Entity,
        &Pawn,
        &Transform,
        &Mover,
        &ViewAngles,
        Option<&WeaponInput>,
        Option<&MoveInput>,
        Has<LocalPlayer>,
        Has<Dead>,
    )>,
    state: &Query<(&Health, &WeaponState)>,
    me: PawnItem,
    (shots_fired, hits, heads, kills): (usize, u32, u32, usize),
    mode: &str,
) {
    let (me_entity, pawn, tf, mover, view, wi, mi, _, dead) = me;

    let eye = mover.eye(tf.translation);
    let forward = view.forward();
    let sight = collision::sight_filter();
    let mut best: Option<(Entity, f32, f32, f32, f32)> = None;
    let mut vis: Vec<String> = Vec::new();
    for (e, other, otf, omover, _, _, _, _, odead) in pawns {
        if dead || odead || !crate::combat::hostile(other, pawn) {
            continue;
        }
        let chest = otf.translation + Vec3::Y * omover.eye_height * 0.75;
        let to = chest - eye;
        let dist = to.length();
        let dir = to / dist.max(1e-3);
        let clear = |to: Vec3| Dir3::new(to).ok().is_some_and(|d| spatial.cast_ray(eye, d, to.length(), true, &sight).is_none());
        // In sight whichever way we face (their eyes or chest).
        let in_sight = dist < u(6000.0) && (clear(otf.translation + Vec3::Y * omover.eye_height - eye) || clear(to));
        if in_sight {
            vis.push(e.index().to_string());
        }
        if dir.dot(forward) < 0.26 || !in_sight {
            continue;
        }
        let yaw = f32::atan2(-to.x, -to.z);
        let pitch = f32::atan2(to.y, Vec2::new(to.x, to.z).length());
        let off = dir.dot(forward);
        if best.is_none_or(|b| off > b.4) {
            best = Some((e, yaw, pitch, dist, off));
        }
    }
    let (fire, ads) = wi.map_or((false, false), |w| (w.fire, w.ads));
    let (fwd, right, jump) = mi.map_or((0.0, 0.0, false), |m| (m.forward, m.right, m.jump));
    let enemy = best.map_or(",,,".to_string(), |(e, yaw, pitch, dist, _)| {
        format!("{},{:.3},{:.3},{:.0}", e.index(), yaw.to_degrees(), pitch.to_degrees(), dist / u(1.0))
    });
    let feet = crate::units::to_cod(tf.translation);
    let stance = match mover.stance {
        Stance::Stand => 0,
        Stance::Crouch => 1,
        Stance::Prone => 2,
    };
    let (health, reloading) = state.get(me_entity).map_or((0.0, false), |(h, w)| (h.current, w.reloading()));
    let weapon = state.get(me_entity).map_or("", |(_, w)| w.def.name.as_str());
    let ads_level = state.get(me_entity).map_or(0.0, |(_, w)| w.ads);
    let view_dist = Dir3::new(forward).ok().and_then(|d| spatial.cast_ray(eye, d, u(8192.0), true, &sight)).map_or(8192.0, |h| h.distance / u(1.0));
    // (A bot's mode column carries its motive too; the player's is empty.)
    let mode = if mode.contains(',') { mode.to_string() } else { format!("{mode},") };
    let mode = {
        let (m, why) = mode.split_once(',').unwrap_or((&mode, ""));
        format!("{m},{},{weapon},{},{why},{ads_level:.2},{view_dist:.0}", me_entity.index(), vis.join(";"))
    };
    writeln!(
        file,
        "{:.4},{:.3},{:.3},{},{},{},{},{},{:.0},{:.0},{:.0},{},{},{},{},{:.0},{},{},{},{},{},{},{},{}",
        time.elapsed_secs(),
        view.yaw.to_degrees(),
        view.pitch.to_degrees(),
        fire as u8,
        ads as u8,
        fwd,
        right,
        enemy,
        feet[0],
        feet[1],
        feet[2],
        stance,
        mover.sprinting as u8,
        jump as u8,
        mover.on_ground as u8,
        health,
        dead as u8,
        reloading as u8,
        shots_fired,
        hits,
        heads,
        kills,
        match pawn.team {
            crate::combat::Team::Axis => 1,
            crate::combat::Team::Allies => 2,
        },
        mode,
    )
    .ok();
}

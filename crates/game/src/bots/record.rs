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
//! hits, headshots, kills; and team (CoD4's numbers: 1 axis, 2 allies).

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
    who: Option<String>,
    started: bool,
}

const HEADER: &str = "t,yaw,pitch,fire,ads,fwd,right,enemy,eyaw,epitch,edist,\
x,y,z,stance,sprint,jump,ground,health,dead,reloading,shots,hits,heads,kills,team";

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
    app.insert_resource(Recorder { path, dir, files: Vec::new(), who: std::env::var("COD4RW_RECORD_WHO").ok(), started: false })
        .add_systems(Update, record.after(crate::weapons::WeaponSet).run_if(crate::state::in_game));
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
) {
    let rec = &mut *rec;
    if !rec.started {
        rec.started = true;
        let all = rec.who.as_deref().is_some_and(|w| w.eq_ignore_ascii_case("all"));
        let base = rec.path.clone().unwrap_or_else(|| default_path(&rec.dir, map.as_ref().map_or("map", |m| m.0.as_str())));
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
    for (me_entity, file) in &mut rec.files {
        let Ok(me) = pawns.get(*me_entity) else { continue };
        let me_entity = *me_entity;
        let shots_fired = shots.iter().filter(|&&s| s == me_entity).count();
        let mine = damage.iter().filter(|d| d.0 == me_entity && d.1 != me_entity);
        let (hits, heads) = mine.fold((0, 0), |(h, hd), d| (h + 1, hd + d.2 as u32));
        let kills = killed.iter().filter(|k| k.0 == me_entity && k.1 != me_entity).count();
        write_row(file, &time, &spatial, &pawns, &state, me, (shots_fired, hits, heads, kills));
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
) {
    let (me_entity, pawn, tf, mover, view, wi, mi, _, dead) = me;

    let eye = mover.eye(tf.translation);
    let forward = view.forward();
    let sight = collision::sight_filter();
    let mut best: Option<(Entity, f32, f32, f32, f32)> = None;
    for (e, other, otf, omover, _, _, _, _, odead) in pawns {
        if dead || odead || !crate::combat::hostile(other, pawn) {
            continue;
        }
        let chest = otf.translation + Vec3::Y * omover.eye_height * 0.75;
        let to = chest - eye;
        let dist = to.length();
        let dir = to / dist.max(1e-3);
        if dir.dot(forward) < 0.26 {
            continue;
        }
        let blocked = Dir3::new(to).ok().is_some_and(|d| spatial.cast_ray(eye, d, dist, true, &sight).is_some());
        if blocked {
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
    writeln!(
        file,
        "{:.4},{:.3},{:.3},{},{},{},{},{},{:.0},{:.0},{:.0},{},{},{},{},{:.0},{},{},{},{},{},{},{}",
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
    )
    .ok();
}

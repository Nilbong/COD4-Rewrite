//! Search and Destroy (`_sd.gsc`, CoD4's defaults). Rounds of 2:30 with one
//! life each: the attackers pick up the bomb where it lies (`sd_bomb`) and
//! plant it at bomb site A or B (`bombzone`), holding use for 5 seconds; it
//! goes off 45 seconds later unless a defender defuses it (5 seconds). A
//! round goes to the attackers if the target blows up or every defender is
//! dead; to the defenders if they defuse the bomb, kill every attacker
//! before it's planted, or the time runs out unplanted. First to 4 rounds;
//! sides swap every 3.

use super::{Bomb, BombSite, GameMode, Objectives, UseObjective, current};
use crate::audio::{Sfx, Sides};
use crate::combat::{Damage, Dead, HitLocation, Pawn, Team};
use crate::content::Content;
use crate::fx::{Anchor, Effects, Frame, FxLayer};
use crate::models::{Skeleton, SpawnModel, spawn_model};
use crate::movement::{Frozen, MoveInput};
use crate::player::LocalPlayer;
use crate::state::{GameState, Setup, in_game};
use crate::tdm::MatchState;
use crate::units::u;
use crate::weapons::WeaponInput;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;
use std::sync::atomic::{AtomicU8, Ordering};

pub struct SdPlugin;

impl Plugin for SdPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, give_use_input)
            .add_systems(OnEnter(GameState::InGame), setup.in_set(Setup::Spawn))
            .add_systems(
                Update,
                (local_use, hold_still).chain().after(crate::player::InputSet).before(crate::movement::MovementSet).run_if(in_game.and_then(playing)),
            )
            // Sabotage plants and defuses with the same key.
            .add_systems(
                Update,
                local_use.after(crate::player::InputSet).before(crate::movement::MovementSet).run_if(in_game.and_then(|| current() == GameMode::Sab)),
            )
            .add_systems(
                Update,
                (one_life, bomb, rounds, show_models).chain().after(crate::movement::MovementSet).run_if(in_game.and_then(playing)),
            );
    }
}

fn playing() -> bool {
    current() == GameMode::Sd
}

/// `scr_sd_*` defaults.
const ROUND_TIME: f32 = 150.0;
const PLANT_TIME: f32 = 5.0;
const DEFUSE_TIME: f32 = 5.0;
const BOMB_TIMER: f32 = 45.0;
/// Sides swap every this many rounds (`scr_sd_roundswitch`).
const ROUND_SWITCH: u32 = 3;
/// From a round's end to the next.
const ROUND_DELAY: f32 = 7.0;
/// Reaches: picking the bomb up (a cylinder like its pickup trigger: this
/// far across, from `PICKUP_BELOW` under the bomb to `PICKUP_ABOVE` over it,
/// as it may sit on a table or lie where its carrier fell), and defusing it.
const PICKUP_RANGE: f32 = 40.0;
const PICKUP_BELOW: f32 = 72.0;
const PICKUP_ABOVE: f32 = 32.0;
const DEFUSE_RANGE: f32 = 64.0;

/// Can someone standing at `feet` pick up the bomb at `bomb`?
pub fn in_pickup_reach(feet: Vec3, bomb: Vec3) -> bool {
    let d = feet - bomb;
    Vec2::new(d.x, d.z).length() < u(PICKUP_RANGE) && (-u(PICKUP_BELOW)..=u(PICKUP_ABOVE)).contains(&d.y)
}
/// `radiusDamage( origin, 512, 200, 20 )`.
const BLAST_RADIUS: f32 = 512.0;
const BLAST_MAX: f32 = 200.0;
const BLAST_MIN: f32 = 20.0;

/// The attacking team this round, for [`crate::combat::pick_spawn`].
static ATTACKERS: AtomicU8 = AtomicU8::new(0);

/// Does `team` attack this round (spawning on `mp_sd_spawn_attacker`)?
pub fn attacking(team: Team) -> bool {
    (ATTACKERS.load(Ordering::Relaxed) == 1) == (team == Team::Axis)
}

fn set_attackers(team: Team) {
    ATTACKERS.store((team == Team::Axis) as u8, Ordering::Relaxed);
}

/// The round under way.
#[derive(Resource)]
struct SdRound {
    /// 1-based, over the match.
    number: u32,
    attackers: Team,
    started: f32,
    /// The round has ended: when.
    over: Option<f32>,
    /// Where the bomb starts each round.
    bomb_home: Vec3,
    /// The match this is for ([`MatchState::started`]).
    match_started: f32,
    /// Who planted (the blast is theirs), and when the bomb last ticked.
    planter: Option<Entity>,
    tick: f32,
    /// "Last one alive" said this round.
    last_alive: bool,
    /// The carrier's last place, for dropping it where they fell.
    carrier_at: Vec3,
    /// Debug aid (`COD4RW_SDBOMB=1`): the player starts round one with the
    /// bomb, wherever they are.
    hand_bomb: bool,
}

#[derive(Component)]
struct SiteModel(usize);

#[derive(Component)]
struct BombModel;

/// Everyone can hold use.
fn give_use_input(mut commands: Commands, pawns: Query<Entity, (Added<Pawn>, Without<UseObjective>)>) {
    for e in &pawns {
        commands.entity(e).insert(UseObjective::default());
    }
}

/// The map's bomb sites (brush triggers named `bombzone`, their boxes from
/// the clip map's models) and the bomb's spot (`sd_bomb`).
fn read_map(content: &Content) -> (Vec<BombSite>, Option<Vec3>) {
    let map = content.map();
    let ents = map.map_ents().map(|m| iw3::ents::parse(&m.entity_string)).unwrap_or_default();
    let clip = map.clip_map();
    let mut sites: Vec<BombSite> = ents
        .iter()
        .filter(|e| e.get("targetname") == Some("bombzone") && e.classname() == "trigger_use_touch")
        .filter_map(|e| {
            let label = e.get("script_label")?.trim_start_matches('_').chars().next()?.to_ascii_uppercase();
            let origin = crate::units::pos(e.origin()?);
            // The trigger's brush model, else a box about its origin.
            let (min, max) = super::brush_box(e, clip, 96.0)?;
            // The target model it points at.
            let target = e.get("target").and_then(|t| ents.iter().find(|x| x.get("targetname") == Some(t))).and_then(|x| x.origin());
            Some(BombSite { label, min, max, pos: target.map_or(origin, crate::units::pos), destroyed: false, team: None })
        })
        .collect();
    sites.sort_by_key(|s| s.label);
    sites.dedup_by_key(|s| s.label);
    let bomb = ents.iter().find(|e| e.get("targetname") == Some("sd_bomb")).and_then(|e| e.origin()).map(crate::units::pos);
    (sites, bomb)
}

fn setup(
    mut commands: Commands,
    time: Res<Time>,
    content: Res<Content>,
    config: Res<crate::tdm::MatchConfig>,
    mut objectives: ResMut<Objectives>,
) {
    if config.mode != GameMode::Sd {
        return;
    }
    let (sites, home) = read_map(&content);
    info!("sd: sites {:?}, bomb at {:?}", sites.iter().map(|s| (s.label, crate::units::to_cod(s.pos))).collect::<Vec<_>>(), home.map(crate::units::to_cod));
    let home = home.unwrap_or_else(|| sites.first().map_or(Vec3::ZERO, |s| s.pos));
    for (i, s) in sites.iter().enumerate() {
        commands.spawn((Name::new(format!("bomb site {}", s.label)), SiteModel(i), Transform::from_translation(s.pos), Visibility::default()));
    }
    commands.spawn((Name::new("bomb"), BombModel, Transform::from_translation(home), Visibility::default()));
    let attackers = Team::Allies;
    set_attackers(attackers);
    objectives.sites = sites;
    objectives.attackers = Some(attackers);
    objectives.bomb = Some(Bomb { pos: home, carrier: None, planted: None, explodes_at: None });
    let now = time.elapsed_secs();
    objectives.timer = Some((now + ROUND_TIME, false));
    commands.insert_resource(SdRound {
        number: 1,
        attackers,
        started: now,
        over: None,
        bomb_home: home,
        match_started: now,
        planter: None,
        tick: now,
        last_alive: false,
        carrier_at: home,
        hand_bomb: std::env::var_os("COD4RW_SDBOMB").is_some(),
    });
}

/// The player holds use with F, or a pad's X; holding it to plant or defuse
/// doesn't reload.
fn local_use(
    objectives: Res<Objectives>,
    mut players: Query<(Entity, &crate::splitscreen::PlayerInput, &mut UseObjective, &mut WeaponInput)>,
) {
    for (e, player, mut use_input, mut input) in &mut players {
        let pad = player.live && player.pad.interact;
        use_input.0 = (player.live && player.keys.pressed(KeyCode::KeyF)) || pad;
        if pad && objectives.using.iter().any(|u| u.0 == e) {
            input.reload = false;
        }
    }
}

/// Planting or defusing, nobody moves.
fn hold_still(objectives: Res<Objectives>, mut pawns: Query<&mut MoveInput>) {
    for (e, ..) in &objectives.using {
        if let Ok(mut mv) = pawns.get_mut(*e) {
            (mv.forward, mv.right, mv.jump, mv.sprint) = (0.0, 0.0, false, false);
        }
    }
}

/// One life a round: the dead wait for the next.
fn one_life(time: Res<Time>, round: Option<Res<SdRound>>, mut dead: Query<&mut Dead, Added<Dead>>) {
    let now = time.elapsed_secs();
    if round.is_some_and(|r| r.over.is_none()) {
        for mut d in &mut dead {
            // Round restarts respawn at once; killed pawns wait.
            if d.respawn_at > now + 0.01 {
                d.respawn_at = f32::INFINITY;
            }
        }
    }
}

/// The bomb: picked up, carried, dropped, planted, defused or gone off.
#[allow(clippy::too_many_arguments)]
fn bomb(
    time: Res<Time>,
    round: Option<ResMut<SdRound>>,
    mut objectives: ResMut<Objectives>,
    pawns: Query<(Entity, &Pawn, &Transform, &UseObjective, Has<Dead>)>,
    // In Last Stand the bomb is dropped and can't be taken, planted or
    // defused (`_gameobjects::onPlayerLastStand`).
    downed: Query<(), With<crate::perks::Downed>>,
    me: Query<(Entity, &Pawn), With<LocalPlayer>>,
    sides: Option<Res<Sides>>,
    mut sfx: ResMut<Sfx>,
    mut effects: ResMut<Effects>,
    mut damage: MessageWriter<Damage>,
    mut awards: MessageWriter<crate::ui::progression::Award>,
) {
    use crate::ui::progression::{Award, AwardKind};
    let Some(mut round) = round else { return };
    if round.over.is_some() {
        objectives.using.clear();
        return;
    }
    let (now, dt) = (time.elapsed_secs(), time.delta_secs());
    let Some(mut b) = objectives.bomb else { return };
    let attackers = round.attackers;
    let mine = me.single().ok().map(|(_, p)| p.team);
    let mut lines: Vec<(Team, &str)> = Vec::new();
    if round.hand_bomb && round.number == 1 && b.planted.is_none() {
        if let Some((e, p)) = me.single().ok().filter(|(_, p)| p.team == attackers) {
            round.hand_bomb = false;
            b.carrier = Some(e);
            info!("sd: debug: {} has the bomb", p.name);
        }
    }

    // Carried: it goes where the carrier goes, and falls where they die.
    if let Some(c) = b.carrier {
        match pawns.get(c) {
            Ok((_, _, tf, _, false)) if !downed.contains(c) => round.carrier_at = tf.translation,
            _ => {
                b.carrier = None;
                b.pos = round.carrier_at;
                lines.push((attackers, "bomb_lost"));
            }
        }
    }
    // Lying about: the first live attacker to it picks it up.
    if b.carrier.is_none() && b.planted.is_none() {
        if let Some((e, ..)) = pawns.iter().find(|(e, p, tf, _, dead)| !dead && !downed.contains(*e) && p.team == attackers && in_pickup_reach(tf.translation, b.pos)) {
            b.carrier = Some(e);
            lines.push((attackers, "bomb_taken"));
        }
    }

    // Planting and defusing: held use, in reach, for the whole time.
    let previous = std::mem::take(&mut objectives.using);
    let progress_of = |e: Entity| previous.iter().find(|u| u.0 == e).map(|u| u.1);
    for (e, p, tf, use_input, dead) in &pawns {
        if dead || downed.contains(e) || !use_input.0 {
            continue;
        }
        let at = tf.translation;
        let (defusing, time_needed) = if b.planted.is_some() { (true, DEFUSE_TIME) } else { (false, PLANT_TIME) };
        let able = if defusing {
            p.team != attackers && at.distance(b.pos) < u(DEFUSE_RANGE)
        } else {
            b.carrier == Some(e) && objectives.sites.iter().any(|s| !s.destroyed && s.contains(at))
        };
        if !able {
            continue;
        }
        if progress_of(e).is_none() {
            let start = if defusing { "mp_bomb_start_defusing" } else { "MP_bomb_plant" };
            sfx.play(start, Some(at));
        }
        let progress = progress_of(e).unwrap_or(0.0) + dt / time_needed;
        if progress < 1.0 {
            objectives.using.push((e, progress, defusing));
            continue;
        }
        if defusing {
            info!("sd: {} defused the bomb", p.name);
            awards.write(Award { pawn: e, kind: AwardKind::Defuse });
            b.explodes_at = None;
            sfx.play("MP_bomb_defuse", Some(b.pos));
            lines.push((attackers, "bomb_defused"));
            lines.push((attackers.other(), "bomb_defused"));
            objectives.round_over = Some((attackers.other(), "MP_BOMB_DEFUSED", "Bomb Defused"));
            objectives.bomb = Some(b);
            say(&lines, mine, sides.as_deref(), &mut sfx);
            return;
        }
        let site = objectives.sites.iter().find(|s| s.contains(at)).map(|s| (s.label, s.pos));
        if let Some((label, pos)) = site {
            info!("sd: {} planted the bomb at {label}", p.name);
            awards.write(Award { pawn: e, kind: AwardKind::Plant });
            b = Bomb { pos, carrier: None, planted: Some(label), explodes_at: Some(now + BOMB_TIMER) };
            round.planter = Some(e);
            objectives.timer = Some((now + BOMB_TIMER, true));
            lines.push((attackers, "bomb_planted"));
            lines.push((attackers.other(), "bomb_planted"));
            break;
        }
    }

    // Ticking, and going off.
    if let Some(at) = b.explodes_at {
        if now - round.tick >= 1.0 {
            round.tick = now;
            sfx.play("bomb_tick", Some(b.pos));
        }
        if now >= at {
            info!("sd: the bomb went off at {:?}", b.planted);
            effects.play("explosions/tanker_explosion", Anchor::Fixed(Frame::facing(b.pos, Vec3::Y, 0.0)), FxLayer::World);
            sfx.play("exp_suitcase_bomb_main", Some(b.pos));
            for (e, _, tf, _, dead) in &pawns {
                let d = tf.translation.distance(b.pos) / crate::units::INCH;
                if !dead && d < BLAST_RADIUS {
                    let amount = BLAST_MAX + (BLAST_MIN - BLAST_MAX) * d / BLAST_RADIUS;
                    damage.write(Damage { target: e, attacker: round.planter, amount, location: HitLocation::Torso, weapon: "briefcase_bomb_mp" });
                }
            }
            if let Some(site) = objectives.sites.iter_mut().find(|s| Some(s.label) == b.planted) {
                site.destroyed = true;
            }
            b.explodes_at = None;
            objectives.round_over = Some((attackers, "MP_TARGET_DESTROYED", "Target Destroyed"));
        }
    }
    objectives.bomb = Some(b);
    say(&lines, mine, sides.as_deref(), &mut sfx);
}

/// The announcer, to the player's team.
fn say(lines: &[(Team, &str)], mine: Option<Team>, sides: Option<&Sides>, sfx: &mut Sfx) {
    let Some(sides) = sides else { return };
    for (team, line) in lines.iter().filter(|(t, _)| Some(*t) == mine) {
        sfx.play(format!("{}_1mc_{line}", sides.of(*team).voice), None);
    }
}

/// Rounds: who's won this one, and the next one starting.
#[allow(clippy::too_many_arguments)]
fn rounds(
    mut commands: Commands,
    time: Res<Time>,
    round: Option<ResMut<SdRound>>,
    state: Option<ResMut<MatchState>>,
    mut objectives: ResMut<Objectives>,
    pawns: Query<(Entity, &Pawn, Has<Dead>)>,
    me: Query<(Entity, &Pawn), With<LocalPlayer>>,
    sides: Option<Res<Sides>>,
    mut sfx: ResMut<Sfx>,
    config: Option<Res<crate::tdm::MatchConfig>>,
) {
    let (Some(mut round), Some(mut state)) = (round, state) else { return };
    let now = time.elapsed_secs();
    // A new match: back to round one, the Allies attacking.
    if round.match_started != state.started {
        round.match_started = state.started;
        start_round(&mut commands, &mut round, &mut objectives, now, 1, Team::Allies, &pawns, false);
        return;
    }
    if state.ended.is_some() {
        return;
    }
    let attackers = round.attackers;
    let mine = me.single().ok();
    match round.over {
        None => {
            let alive = |team: Team| pawns.iter().filter(|(_, p, dead)| p.team == team && !dead).count();
            // A side with nobody on it isn't wiped out.
            let wiped = |team: Team| pawns.iter().any(|(_, p, _)| p.team == team) && alive(team) == 0;
            let planted = objectives.bomb.is_some_and(|b| b.planted.is_some());
            let settled = now - round.started > 1.0;
            let result = objectives.round_over.or_else(|| match (wiped(attackers), wiped(attackers.other())) {
                _ if !settled => None,
                // `onDeadEvent("all")`: everyone down goes to the defenders
                // unless the bomb is planted.
                (true, true) if !planted => Some((attackers.other(), "MP_ENEMIES_ELIMINATED", "Enemies Eliminated")),
                (_, true) => Some((attackers, "MP_ENEMIES_ELIMINATED", "Enemies Eliminated")),
                (true, _) if !planted => Some((attackers.other(), "MP_ENEMIES_ELIMINATED", "Enemies Eliminated")),
                _ if !planted && objectives.timer.is_some_and(|(at, _)| now >= at) => {
                    Some((attackers.other(), "MP_TIME_LIMIT_REACHED", "Time Limit Reached"))
                }
                _ => None,
            });
            // The last one standing hears it.
            if let (Some((e, p)), false) = (mine, round.last_alive) {
                if settled && alive(p.team) == 1 && pawns.get(e).is_ok_and(|x| !x.2) && alive(p.team.other()) > 0 {
                    round.last_alive = true;
                    if let Some(s) = &sides {
                        sfx.play(format!("{}_1mc_lastalive", s.of(p.team).voice), None);
                    }
                }
            }
            let Some((winner, key, text)) = result else { return };
            info!("sd: round {} to {:?} ({text})", round.number, winner);
            match winner {
                Team::Allies => state.allies += 1,
                Team::Axis => state.axis += 1,
            }
            objectives.round_over = Some((winner, key, text));
            objectives.using.clear();
            round.over = Some(now);
            for (e, ..) in &pawns {
                commands.entity(e).insert(Frozen);
            }
        }
        Some(at) if now - at >= ROUND_DELAY => {
            let number = round.number + 1;
            // Sides swap every `ROUND_SWITCH` rounds (`onRoundSwitch`),
            // except at one round each from winning: then the team ahead
            // in kills (fewer deaths if level) defends.
            let swap = (number - 1) % ROUND_SWITCH == 0;
            let limit = config.as_ref().map_or(0, |c| c.score_limit);
            let overtime = limit > 0 && state.allies + 1 == limit && state.axis + 1 == limit;
            let next = if swap && overtime {
                let tally = |team: Team| {
                    pawns.iter().filter(|(_, p, _)| p.team == team).fold((0i64, 0i64), |(k, d), (_, p, _)| (k + p.kills as i64, d + p.deaths as i64))
                };
                let ((ak, ad), (xk, xd)) = (tally(Team::Allies), tally(Team::Axis));
                let allies_better = ak > xk || (ak == xk && ad < xd);
                // The better team defends.
                if allies_better { Team::Axis } else { Team::Allies }
            } else if swap {
                attackers.other()
            } else {
                attackers
            };
            start_round(&mut commands, &mut round, &mut objectives, now, number, next, &pawns, true);
        }
        Some(_) => {}
    }
}

/// Everyone back at their side's spawns, the bomb home, the targets whole.
#[allow(clippy::too_many_arguments)]
fn start_round(
    commands: &mut Commands,
    round: &mut SdRound,
    objectives: &mut Objectives,
    now: f32,
    number: u32,
    attackers: Team,
    pawns: &Query<(Entity, &Pawn, Has<Dead>)>,
    respawn: bool,
) {
    info!("sd: round {number}, {:?} attacking", attackers);
    set_attackers(attackers);
    (round.number, round.attackers, round.started, round.over) = (number, attackers, now, None);
    (round.planter, round.last_alive, round.carrier_at) = (None, false, round.bomb_home);
    objectives.attackers = Some(attackers);
    objectives.bomb = Some(Bomb { pos: round.bomb_home, carrier: None, planted: None, explodes_at: None });
    objectives.timer = Some((now + ROUND_TIME, false));
    objectives.using.clear();
    objectives.round_over = None;
    for s in &mut objectives.sites {
        s.destroyed = false;
    }
    if respawn {
        for (e, ..) in pawns {
            commands.entity(e).insert((Dead { respawn_at: now, killer: None }, Frozen));
        }
    }
}

/// The targets (whole or blown up) and the bomb where it lies.
#[allow(clippy::too_many_arguments)]
fn show_models(
    mut commands: Commands,
    objectives: Res<Objectives>,
    mut content: ResMut<Content>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut bindposes: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    mut sites: Query<(Entity, &SiteModel, Option<&Children>), Without<BombModel>>,
    mut bomb: Query<(Entity, &mut Transform, &mut Visibility, Option<&Children>), With<BombModel>>,
    mut shown: Local<Vec<Option<bool>>>,
) {
    let mut model = |commands: &mut Commands, owner: Entity, name: &str| {
        if let Some(m) = content.model(name, &mut meshes, &mut materials, &mut images, &mut bindposes) {
            spawn_model(commands, &mut Skeleton::default(), SpawnModel { model: &m, owner, attach_to: None, layers: None, shadows: true });
        }
    };
    shown.resize(objectives.sites.len(), None);
    for (e, SiteModel(i), _) in &mut sites {
        let Some(site) = objectives.sites.get(*i) else { continue };
        if shown[*i] == Some(site.destroyed) {
            continue;
        }
        shown[*i] = Some(site.destroyed);
        commands.entity(e).despawn_children();
        model(&mut commands, e, if site.destroyed { "com_bomb_objective_d" } else { "com_bomb_objective" });
    }
    let Some(b) = objectives.bomb else { return };
    for (e, mut tf, mut vis, children) in &mut bomb {
        if children.is_none_or(|c| c.is_empty()) {
            model(&mut commands, e, "mil_tntbomb_mp");
        }
        if tf.translation != b.pos {
            tf.translation = b.pos;
        }
        vis.set_if_neq(if b.carrier.is_some() { Visibility::Hidden } else { Visibility::Inherited });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attackers_switch() {
        set_attackers(Team::Axis);
        assert!(attacking(Team::Axis) && !attacking(Team::Allies));
        set_attackers(Team::Allies);
        assert!(attacking(Team::Allies) && !attacking(Team::Axis));
    }

    #[test]
    fn bomb_pickup_is_a_cylinder() {
        let bomb = Vec3::new(0.0, u(40.0), 0.0);
        // Standing on the floor by the table it's on.
        assert!(in_pickup_reach(Vec3::new(u(30.0), 0.0, 0.0), bomb));
        assert!(!in_pickup_reach(Vec3::new(u(50.0), 0.0, 0.0), bomb));
        // Dropped at someone's feet; and not from the floor above.
        assert!(in_pickup_reach(Vec3::ZERO, Vec3::ZERO));
        assert!(!in_pickup_reach(Vec3::new(0.0, u(120.0), 0.0), Vec3::ZERO));
    }

    #[test]
    fn sites_have_some_give() {
        let s = BombSite { label: 'A', min: Vec3::ZERO, max: Vec3::splat(u(32.0)), pos: Vec3::ZERO, destroyed: false, team: None };
        assert!(s.contains(Vec3::new(u(16.0), 0.0, u(16.0))));
        assert!(s.contains(Vec3::new(u(36.0), 0.0, 0.0)));
        assert!(!s.contains(Vec3::new(u(60.0), 0.0, 0.0)));
        assert_eq!(s.letter(), 'a');
    }
}

//! The characters worn in a match ([`crate::characters`]): the player's own,
//! chosen in the Character menu, as their body ([`crate::thirdperson`],
//! seen with F5) and their first-person arms ([`crate::viewmodel`]); and the
//! bots', a cast for each side. Characters from other zones (CoD4's campaign
//! levels, Black Ops) load on threads while the map loads; their models are
//! converted once and the zones dropped. The match waits for the player's
//! own; bots wear their team's models until theirs are ready.

mod camera;
mod dressing;
pub mod load;

pub use camera::{LocalBody, ThirdPerson};
pub use dressing::Dressing;
pub use load::{Source, black_ops_anim, load, load_black_ops_anims};

use crate::characters::{CHARACTERS, Character, Game};
use crate::combat::Team;
use crate::content::{Content, PreparedModel};
use crate::state::{GameState, Setup};
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;
use load::Loaded;
use rand::seq::{IndexedRandom, SliceRandom};
use std::collections::HashMap;
use std::sync::Arc;
use std::thread::JoinHandle;

pub struct WardrobePlugin;

impl Plugin for WardrobePlugin {
    fn build(&self, app: &mut App) {
        // Debug: `COD4RW_THIRDPERSON` starts in third person.
        app.init_resource::<Wardrobe>()
            .insert_resource(ThirdPerson([std::env::var_os("COD4RW_THIRDPERSON").is_some(); crate::splitscreen::MAX_PLAYERS]))
            .add_systems(
                OnEnter(GameState::InGame),
                (begin.before(crate::world::load_map), finish.after(crate::world::load_map)).in_set(Setup::Content),
            )
            .add_systems(
                Update,
                (
                    (camera::toggle, camera::show_body).chain(),
                    (poll, refit).chain().before(crate::movement::MovementSet),
                )
                    .run_if(crate::state::in_game),
            )
            .add_systems(
                PostUpdate,
                camera::place_camera
                    .after(crate::player::follow_camera)
                    .after(crate::bodycam::pose_view)
                    .before(TransformSystems::Propagate)
                    .run_if(crate::state::in_game),
            );
    }
}

/// The most characters a side's bots wear; past that they repeat.
const CAST_SIZE: usize = 8;
/// A zone is worth loading for a side's bots with at least this many
/// soldiers of that side.
const MIN_SOLDIERS: usize = 3;

/// On a pawn wearing its team's models until its own character is ready.
#[derive(Component)]
pub struct Provisional;

/// A character ready to wear: its models, body first.
pub struct Outfit {
    pub character: &'static Character,
    pub models: Vec<Arc<PreparedModel>>,
}

/// What each pawn wears this match.
#[derive(Resource, Default)]
pub struct Wardrobe {
    /// The player's character, unless spectating.
    player: Option<&'static Character>,
    /// The player's first-person arms, to find.
    arms_wanted: Option<Arms>,
    /// Each side's characters (Allies, Axis), and the bots' by name.
    casts: [Vec<&'static Character>; 2],
    bots: HashMap<String, &'static Character>,
    /// Zones loading as the map loads.
    loading: Vec<(Source, JoinHandle<anyhow::Result<Loaded>>)>,
    anims: Option<JoinHandle<bool>>,
    /// Ready to wear, by character id.
    outfits: HashMap<&'static str, Outfit>,
    arms: Option<Arc<PreparedModel>>,
}

impl Wardrobe {
    /// Who a pawn is to wear: the player's character, or a bot's from its
    /// side.
    fn character(&self, name: &str, team: Team, local: bool) -> Option<&'static Character> {
        if local {
            return self.player;
        }
        match self.bots.get(name) {
            Some(&c) => Some(c),
            // Not in the match's line-up (a spectator's stand-in): any of the
            // side's.
            None => {
                let cast = &self.casts[side_index(team)];
                cast.get(name.bytes().fold(0usize, |h, b| h.wrapping_mul(31).wrapping_add(b as usize)) % cast.len().max(1)).copied()
            }
        }
    }

    /// What a pawn wears, once it's ready.
    pub fn outfit(&self, name: &str, team: Team, local: bool) -> Option<&Outfit> {
        self.outfits.get(self.character(name, team, local)?.id)
    }

    /// Is what a pawn wears still loading?
    pub fn pending(&self, name: &str, team: Team, local: bool) -> bool {
        self.character(name, team, local)
            .is_some_and(|c| !self.outfits.contains_key(c.id) && self.loading.iter().any(|(s, _)| *s == Source::of(c)))
    }

    /// The player's first-person arms.
    pub fn arms(&self) -> Option<Arc<PreparedModel>> {
        self.arms.clone()
    }
}

fn side_index(team: Team) -> usize {
    match team {
        Team::Allies => 0,
        Team::Axis => 1,
    }
}

/// Which side a character fights for, or none for those who don't: the
/// civilians, prisoners and wounded.
fn side(c: &Character) -> Option<Team> {
    const NONE: &[&str] = &[
        "vip",
        "russian_farmer",
        "zakhaev_wounded",
        "al_asad_captured",
        "reznov_prisoner",
        "mason_interrogation",
        "steiner",
        "jfk",
        "mcnamara",
        "bo_general",
        "bo_frozen_wehrmacht",
    ];
    const AXIS: &[&str] = &[
        "spetsnaz",
        "opfor",
        "ultranat",
        "zakhaev",
        "victor",
        "al_asad",
        "castro",
        "dragovich",
        "kravchenko",
        "steiner",
        "bo_rus_",
        "bo_vtn_",
        "bo_cub_tropas",
        "bo_tropas",
        "bo_cuban_police",
        "bo_soviet",
        "bo_spetsnaz",
        "bo_nva",
        "bo_viet_cong",
        "bo_vc_",
        "bo_wehrmacht",
        "bo_rebirth_engineer",
    ];
    if NONE.contains(&c.id) {
        None
    } else if AXIS.iter().any(|p| c.id.starts_with(p)) {
        Some(Team::Axis)
    } else {
        Some(Team::Allies)
    }
}

/// Already in the match's content: CoD4's soldiers of the map being played.
fn in_map(c: &Character, map: &str) -> bool {
    c.game == Game::Cod4 && c.zone.eq_ignore_ascii_case(map)
}

/// `count` characters for a side's bots: the map's own soldiers of that side
/// and those of one other zone, at random.
fn cast(map: &str, team: Team, count: usize, rng: &mut impl rand::Rng) -> Vec<&'static Character> {
    let soldiers = || CHARACTERS.iter().filter(move |c| side(c) == Some(team));
    let mut sources: Vec<Source> = Vec::new();
    for c in soldiers().filter(|c| !in_map(c, map)) {
        let src = Source::of(c);
        if !sources.contains(&src) && soldiers().filter(|o| Source::of(o) == src).count() >= MIN_SOLDIERS {
            sources.push(src);
        }
    }
    // Debug: `COD4RW_CAST=<zone>` picks the other zone.
    let forced = std::env::var("COD4RW_CAST").ok().and_then(|z| sources.iter().find(|s| s.zone.eq_ignore_ascii_case(&z)).cloned());
    let other = forced.or_else(|| sources.choose(rng).cloned());
    let mut pool: Vec<&'static Character> =
        soldiers().filter(|c| in_map(c, map) || other.as_ref().is_some_and(|o| Source::of(c) == *o)).collect();
    pool.shuffle(rng);
    pool.truncate(count);
    pool
}

/// A character's first-person arms: the models to try, in order, and the
/// zone they're in when not the character's own (nor the map's).
#[derive(Debug)]
struct Arms {
    models: Vec<String>,
    source: Option<Source>,
}

/// Where each of CoD4's first-person hands is: multiplayer maps first.
const COD4_HANDS: &[(&str, &[&str])] = &[
    ("viewhands_black_kit", &["mp_killhouse", "cargoship", "killhouse", "airplane"]),
    ("viewhands_sas_woodland", &["mp_bloc", "ambush", "blackout", "hunted", "jeepride", "village_assault", "village_defend"]),
    ("viewhands_marine_sniper", &["mp_bloc", "scoutsniper", "sniperescape"]),
    ("viewhands_op_force", &["mp_killhouse", "mp_bloc", "ambush"]),
    ("viewhands_desert_opfor", &["mp_crash"]),
    ("viewhands_usmc", &["mp_crash"]),
];

/// Black Ops' multiplayer outfits' kinds, each with its own arms.
const BO_KINDS: [&str; 5] = ["standard", "flak", "armor", "camo", "utility"];

/// A Black Ops multiplayer faction's arms, `first` kind first.
fn faction_arms(faction: &str, first: &str) -> Vec<String> {
    std::iter::once(first).chain(BO_KINDS.into_iter().filter(|k| *k != first)).map(|k| format!("viewmodel_{faction}_{k}_arms")).collect()
}

fn arms(c: &Character, map: &str) -> Arms {
    let id = c.id;
    let has = |parts: &[&str]| parts.iter().any(|p| id.contains(p));
    match c.game {
        Game::Cod4 => {
            let hands = if has(&["ghillie"]) {
                "viewhands_marine_sniper"
            } else if has(&["woodland", "price_nvg", "nikolai", "loyalist"]) {
                "viewhands_sas_woodland"
            } else if has(&["sas_", "gaz", "mac", "price"]) {
                "viewhands_black_kit"
            } else if has(&["marine", "griggs", "vasquez", "pilot", "force_recon"]) {
                "viewhands_usmc"
            } else if has(&["opfor", "al_asad", "vip"]) {
                "viewhands_desert_opfor"
            } else {
                "viewhands_op_force"
            };
            let zones = COD4_HANDS.iter().find(|(h, _)| *h == hands).map_or(&[][..], |(_, z)| z);
            let elsewhere = !zones.contains(&c.zone) && !zones.contains(&map);
            Arms { models: vec![hands.to_owned()], source: zones.first().filter(|_| elsewhere).map(|z| Source::zone(Game::Cod4, z)) }
        }
        Game::BlackOps => {
            // Multiplayer outfits have arms of their own.
            if let Some((faction, kind)) = c.models[0].strip_prefix("c_").and_then(|m| m.split_once("_mp_body_")) {
                return Arms { models: faction_arms(faction, kind), source: None };
            }
            // The campaign's: the level's own, or a multiplayer faction's for
            // the enemies (the player is never one of them).
            let (models, zone) = if has(&["tropas", "cuban_police", "castro"]) {
                (faction_arms("cub_tropas", "standard"), "mp_firingrange")
            } else if has(&["nva", "viet_cong", "vc_bomber"]) {
                (faction_arms("vtn_nva", "standard"), "mp_cracked")
            } else if has(&["soviet_winter", "spetsnaz_winter", "spetsnaz_snow"]) {
                (faction_arms("rus_spetwin", "standard"), "mp_array")
            } else if c.zone != "fullahead" && has(&["dragovich", "kravchenko", "soviet_heavy"]) {
                (faction_arms("rus_spet", "flak"), "mp_nuked")
            } else {
                let level = match c.zone {
                    "cuba" => "viewmodel_usa_cuban_casual_arms",
                    "flashpoint" if id.starts_with("woods") => "viewmodel_usa_blackops_urban_arms",
                    "flashpoint" => "viewmodel_usa_blackops_spetsnaz_arms",
                    "khe_sanh" | "hue_city" => "viewmodel_usa_jungmar_arms",
                    "kowloon" => "viewmodel_usa_blackops_urban_arms_getwet",
                    "fullahead" => "viewmodel_rus_reznov_winter_arms",
                    "creek_1" => "viewmodel_usa_jungmar_wet_arms",
                    "wmd_sr71" => "viewmodel_usa_blackops_winter_arms",
                    "int_escape" => "viewmodel_usa_mason_interrogation_arms",
                    "rebirth" if id.starts_with("mason") => "viewmodel_usa_blackops_urban_arms",
                    "rebirth" => "viewmodel_usa_hazmat_arms",
                    "underwaterbase" => "viewmodel_usa_ubase_arms",
                    // The Pentagon's have none: CIA arms.
                    _ => return Arms { models: faction_arms("usa_cia", "standard"), source: Some(Source::zone(Game::BlackOps, "mp_nuked")) },
                };
                (vec![level.to_owned()], c.zone)
            };
            Arms { models, source: (zone != c.zone).then(|| Source::zone(Game::BlackOps, zone)) }
        }
    }
}

/// As the match loads: choose who wears what, and start loading the zones
/// the characters are in (alongside the map's).
fn begin(
    mut wardrobe: ResMut<Wardrobe>,
    map: Res<crate::world::MapName>,
    config: Res<crate::tdm::MatchConfig>,
    spectate: Option<Res<crate::bots::spectate::Spectate>>,
) {
    let mut w = Wardrobe::default();
    let map = map.0.as_str();
    if spectate.is_none() {
        let c = crate::supply::inventory().wearing();
        w.arms_wanted = Some(arms(c, map));
        w.player = Some(c);
    }
    // Background sims skip the bots' characters: they only slow loading.
    if std::env::var_os("COD4RW_SIM").is_none() {
        let mut rng = rand::rng();
        for (side, team) in [Team::Allies, Team::Axis].into_iter().enumerate() {
            let names = &config.bots[side];
            // A spectator's stand-in joins the player's side.
            let count = names.len() + usize::from(spectate.is_some() && team == config.player_team);
            if count == 0 {
                continue;
            }
            let cast = cast(map, team, count.min(CAST_SIZE), &mut rng);
            for (name, c) in names.iter().zip(cast.iter().cycle()) {
                info!("wardrobe: {name} wears {} ({})", c.name, c.game.short());
                w.bots.insert(name.clone(), c);
            }
            w.casts[side] = cast;
        }
    }
    let mut sources: Vec<Source> = Vec::new();
    let wanted = w.player.iter().chain(w.casts.iter().flatten()).copied().filter(|c| !in_map(c, map));
    for src in wanted.map(Source::of).chain(w.arms_wanted.as_ref().and_then(|a| a.source.clone())) {
        if !sources.contains(&src) {
            sources.push(src);
        }
    }
    if sources.iter().any(|s| s.game == Game::BlackOps) {
        w.anims = Some(std::thread::spawn(load_black_ops_anims));
    }
    if let Some(c) = w.player {
        info!("wardrobe: you wear {} ({}), arms {:?}", c.name, c.game.short(), w.arms_wanted);
    }
    info!("wardrobe: loading {:?}", sources.iter().map(|s| s.zone).collect::<Vec<_>>());
    w.loading = sources.into_iter().map(|s| (s.clone(), load(s))).collect();
    *wardrobe = w;
}

/// Once the map has loaded: convert the map's own characters, and wait for
/// the zones the player's character and arms are in.
fn finish(
    mut wardrobe: ResMut<Wardrobe>,
    mut content: ResMut<Content>,
    map: Res<crate::world::MapName>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut bindposes: ResMut<Assets<SkinnedMeshInverseBindposes>>,
) {
    let started = std::time::Instant::now();
    let w = &mut *wardrobe;
    // Black Ops' animations load as the map does, before any of its
    // characters.
    if let Some(anims) = w.anims.take() {
        if !anims.join().unwrap_or(false) {
            warn!("wardrobe: no Black Ops animations; its characters use CoD4's");
        }
    }
    let mut a = (&mut *meshes, &mut *materials, &mut *images, &mut *bindposes);
    let mut dressing = Dressing::default();
    for c in w.worn() {
        if in_map(c, &map.0) {
            dress(&mut w.outfits, c, &mut content, &mut dressing, &mut a);
        }
    }
    // The player's arms may be among the map's.
    if w.player.is_some_and(|c| c.game == Game::Cod4) {
        find_arms(&mut w.arms, &mut w.arms_wanted, &mut content, &mut dressing, &mut a);
    }
    let player_sources: Vec<Source> =
        w.player.map(Source::of).into_iter().chain(w.arms_wanted.as_ref().and_then(|arms| arms.source.clone())).collect();
    let mut waited = std::time::Duration::ZERO;
    while let Some(i) = w.loading.iter().position(|(src, _)| player_sources.contains(src)) {
        let (src, task) = w.loading.remove(i);
        let wait = std::time::Instant::now();
        let joined = task.join();
        waited += wait.elapsed();
        w.prepare(&src, joined, &content.vfs, &map.0, &mut a);
    }
    info!(
        "wardrobe: {} characters ready in {:.2}s ({:.2}s waiting for zones), arms {}; {} zones still loading",
        w.outfits.len(),
        started.elapsed().as_secs_f32(),
        waited.as_secs_f32(),
        w.arms.as_ref().map_or("(the team's)", |m| m.name.as_str()),
        w.loading.len(),
    );
}

/// The bots' zones as they finish loading, one a frame.
fn poll(
    mut wardrobe: ResMut<Wardrobe>,
    content: Res<Content>,
    map: Res<crate::world::MapName>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut bindposes: ResMut<Assets<SkinnedMeshInverseBindposes>>,
) {
    let Some(i) = wardrobe.loading.iter().position(|(_, task)| task.is_finished()) else { return };
    let started = std::time::Instant::now();
    let w = &mut *wardrobe;
    let (src, task) = w.loading.remove(i);
    let mut a = (&mut *meshes, &mut *materials, &mut *images, &mut *bindposes);
    w.prepare(&src, task.join(), &content.vfs, &map.0, &mut a);
    info!("wardrobe: characters from {} ready in {:.2}s", src.zone, started.elapsed().as_secs_f32());
}

/// Bots wearing their team's models while theirs loaded change into them
/// (or stay, if it failed).
fn refit(
    mut commands: Commands,
    wardrobe: Res<Wardrobe>,
    pawns: Query<(Entity, &crate::combat::Pawn, &crate::thirdperson::Body, Has<crate::player::LocalPlayer>), With<Provisional>>,
) {
    if !wardrobe.is_changed() {
        return;
    }
    for (pawn, p, body, local) in &pawns {
        if let Some(o) = wardrobe.outfit(&p.name, p.team, local) {
            debug!("wardrobe: {} changes into {}", p.name, o.character.name);
            commands.entity(body.0).despawn();
            commands.entity(pawn).remove::<(crate::thirdperson::Body, Provisional)>();
        } else if !wardrobe.pending(&p.name, p.team, local) {
            commands.entity(pawn).remove::<Provisional>();
        }
    }
}

impl Wardrobe {
    /// Everyone's characters, once each.
    fn worn(&self) -> Vec<&'static Character> {
        let mut worn: Vec<&'static Character> = Vec::new();
        for &c in self.player.iter().chain(self.casts.iter().flatten()) {
            if !worn.iter().any(|o| o.id == c.id) {
                worn.push(c);
            }
        }
        worn
    }

    /// Convert the characters (and arms) worn from a zone that has loaded,
    /// then drop it.
    fn prepare(&mut self, src: &Source, joined: std::thread::Result<anyhow::Result<Loaded>>, vfs: &Arc<iw3::iwd::Vfs>, map: &str, a: &mut ModelAssets) {
        let (zones, own_vfs) = match joined {
            Ok(Ok(loaded)) => loaded,
            Ok(Err(e)) => {
                warn!("wardrobe: characters from {} unavailable: {e:#}", src.zone);
                return;
            }
            Err(_) => {
                warn!("wardrobe: loading {} panicked", src.zone);
                return;
            }
        };
        let mut content = Content::new(zones, own_vfs.unwrap_or_else(|| vfs.clone()));
        let mut dressing = Dressing::default();
        for c in self.worn() {
            if !in_map(c, map) && Source::of(c) == *src {
                dress(&mut self.outfits, c, &mut content, &mut dressing, a);
            }
        }
        let player_source = self.player.map(Source::of);
        let arms_here = self.arms_wanted.as_ref().is_some_and(|arms| arms.source.as_ref().or(player_source.as_ref()) == Some(src));
        if arms_here {
            find_arms(&mut self.arms, &mut self.arms_wanted, &mut content, &mut dressing, a);
        }
    }
}

type ModelAssets<'a> =
    (&'a mut Assets<Mesh>, &'a mut Assets<StandardMaterial>, &'a mut Assets<Image>, &'a mut Assets<SkinnedMeshInverseBindposes>);

/// Convert `c`'s models from `content`.
fn dress(outfits: &mut HashMap<&'static str, Outfit>, c: &'static Character, content: &mut Content, dressing: &mut Dressing, a: &mut ModelAssets) {
    let models: Vec<Arc<PreparedModel>> =
        c.models.iter().filter_map(|name| dressing.model(content, name, a.0, a.1, a.2, a.3)).filter(|m| !m.surfaces.is_empty()).collect();
    if models.is_empty() {
        warn!("wardrobe: {} has no models in {}", c.id, c.zone);
        return;
    }
    outfits.insert(c.id, Outfit { character: c, models });
}

/// The first of the wanted arms `content` has (with something to draw).
fn find_arms(
    found: &mut Option<Arc<PreparedModel>>,
    wanted: &mut Option<Arms>,
    content: &mut Content,
    dressing: &mut Dressing,
    a: &mut ModelAssets,
) {
    let Some(arms) = wanted.as_ref() else { return };
    for name in &arms.models {
        if content.find(name).is_none() {
            continue;
        }
        if let Some(m) = dressing.model(content, name, a.0, a.1, a.2, a.3).filter(|m| !m.surfaces.is_empty()) {
            *found = Some(m);
            *wanted = None;
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sides() {
        let side_of = |id| side(crate::characters::character(id).unwrap());
        assert_eq!(side_of("price"), Some(Team::Allies));
        assert_eq!(side_of("spetsnaz"), Some(Team::Axis));
        assert_eq!(side_of("victor_zakhaev"), Some(Team::Axis));
        assert_eq!(side_of("bo_usa_cia_flak"), Some(Team::Allies));
        assert_eq!(side_of("bo_rus_spet_camo"), Some(Team::Axis));
        assert_eq!(side_of("bo_hazmat"), Some(Team::Allies));
        assert_eq!(side_of("bo_soviet_hazmat"), Some(Team::Axis));
        assert_eq!(side_of("vip"), None);
        // Both sides have enough to choose from on every map.
        for team in [Team::Allies, Team::Axis] {
            assert!(cast("mp_killhouse", team, 8, &mut rand::rng()).len() >= MIN_SOLDIERS);
        }
    }

    #[test]
    fn arms_found() {
        let price = arms(crate::characters::character("price").unwrap(), "mp_crash");
        assert_eq!(price.models, ["viewhands_black_kit"]);
        assert!(price.source.is_none(), "cargoship has Price's hands");
        let marine = arms(crate::characters::character("griggs").unwrap(), "mp_killhouse");
        assert_eq!(marine.source, Some(Source::zone(Game::Cod4, "mp_crash")));
        assert!(arms(crate::characters::character("spetsnaz").unwrap(), "mp_killhouse").source.is_none());
        let cia = arms(crate::characters::character("bo_usa_cia_flak").unwrap(), "mp_killhouse");
        assert_eq!(cia.models[0], "viewmodel_usa_cia_flak_arms");
        let woods = arms(crate::characters::character("woods").unwrap(), "mp_killhouse");
        assert_eq!((woods.models[0].as_str(), woods.source), ("viewmodel_usa_jungmar_arms", None));
    }
}

//! Placed people: CoD4's characters ([`crate::characters`]) standing about
//! in the campaign's own idles (the briefing, the bored patrol's smoke and
//! phone, a guard asleep in his chair, chess...), each idle's twitches
//! mixed in, a few cycled at random, or a beat walked to and fro. Only
//! seen: not bots, nothing they react to; a box keeps players out of them.
//!
//! A layout line `@actor:<character>/<gun>/<idle>`: the character's id, the
//! gun held (a multiplayer gun spec like `m4:silencer`, `-` none) and the
//! [`PRESETS`] key.

use super::stream::Streamer;
use crate::characters::{CHARACTERS, Character, Game};
use crate::content::{Content, PreparedModel};
use crate::units::u;
use bevy::prelude::*;
use iw3::xanim::XAnim;
use std::collections::HashMap;
use std::sync::Arc;

pub const PREFIX: &str = "@actor:";

/// How an idle's animations are played.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The first loops; now and then one of the others (its twitches).
    Twitch,
    /// One after another at random.
    Random,
    /// The first (a walk) along a beat of [`BEAT`] ahead and back, the
    /// others at each end.
    Walk,
}

pub struct Preset {
    pub key: &'static str,
    pub label: &'static str,
    /// The level holding the animations (the map's own zones first).
    pub zone: &'static str,
    pub anims: &'static [&'static str],
    /// Holds his gun (else his hands are busy: a phone, a laptop, chess).
    pub armed: bool,
    pub mode: Mode,
}

const fn p(key: &'static str, label: &'static str, zone: &'static str, anims: &'static [&'static str], armed: bool, mode: Mode) -> Preset {
    Preset { key, label, zone, anims, armed, mode }
}

use Mode::{Random, Twitch, Walk};

pub const PRESETS: &[Preset] = &[
    p("at_ease", "Standing at ease", "common", &["casual_stand_idle", "casual_stand_idle_twitch", "casual_stand_idle_twitchb"], true, Twitch),
    p("radio", "Talking, on the radio", "common", &["casual_stand_v2_idle", "casual_stand_v2_twitch_talk", "casual_stand_v2_twitch_radio", "casual_stand_v2_twitch_shift"], true, Twitch),
    p("guard", "On guard, looking about", "common", &["patrolstand_idle", "patrolstand_look", "patrolstand_twitch"], true, Twitch),
    p("random", "Random idles", "common", &["casual_stand_idle", "casual_stand_idle_twitch", "casual_stand_v2_idle", "casual_stand_v2_twitch_talk", "casual_stand_v2_twitch_radio", "casual_stand_v2_twitch_shift", "patrolstand_look", "patrolstand_twitch", "cqb_stand_twitch"], true, Random),
    p("bored", "Bored: smoke, phone, stretch", "blackout", &["patrol_bored_idle", "patrol_bored_idle_smoke", "patrol_bored_idle_cellphone", "patrol_bored_twitch_stretch", "patrol_bored_twitch_checkphone", "patrol_bored_twitch_bug"], true, Twitch),
    p("smoke", "Smoking", "blackout", &["patrol_bored_idle_smoke"], true, Twitch),
    p("phone", "On his phone", "blackout", &["patrol_bored_idle_cellphone", "patrol_bored_twitch_checkphone"], true, Twitch),
    p("salute", "Saluting", "blackout", &["patrol_bored_idle", "patrol_bored_twitch_salute"], true, Random),
    p("patrol", "Patrolling (walks a beat)", "blackout", &["patrol_bored_patrolwalk", "patrol_bored_idle", "patrol_bored_idle_smoke", "patrol_bored_twitch_stretch"], true, Walk),
    p("lean_smoke", "Leaning, smoking", "blackout", &["parabolic_leaning_guy_smoking_idle", "parabolic_leaning_guy_smoking_twitch"], true, Twitch),
    p("check_gun", "Checking his gun (prone)", "common", &["prone_twitch_ammocheck", "prone_twitch_ammocheck2", "prone_idle"], true, Random),
    p("alert", "Alert, rifle ready", "common", &["exposed_idle_alert_v1", "exposed_idle_alert_v2", "exposed_idle_alert_v3"], true, Twitch),
    p("cqb", "Close quarters, ready", "common", &["cqb_stand_idle", "cqb_stand_twitch"], true, Twitch),
    p("signals", "Hand signals", "airplane", &["cqb_stand_signal_twitch_check", "cqb_stand_signal_twitch_look", "cqb_stand_signal_twitch_quicklook", "cqb_stand_signal_twitch_shift"], true, Random),
    p("wave", "Waving the squad on", "airplane", &["stand_exposed_wave_on_me", "stand_exposed_wave_go", "stand_exposed_wave_move_up", "stand_exposed_wave_halt"], true, Random),
    p("crouch", "Crouched, at ease", "common", &["casual_crouch_idle", "casual_crouch_twitch", "casual_crouch_point"], true, Twitch),
    p("cover_crouch", "Behind cover, crouched", "common", &["covercrouch_hide_idle", "covercrouch_twitch_1", "covercrouch_twitch_2", "covercrouch_twitch_3", "covercrouch_twitch_4"], true, Twitch),
    p("cover_stand", "Behind cover, standing", "common", &["coverstand_hide_idle", "coverstand_hide_idle_twitch01", "coverstand_hide_idle_twitch02", "coverstand_hide_idle_twitch03", "coverstand_hide_idle_twitch04"], true, Twitch),
    p("corner", "Leaning round a corner", "common", &["cornerstndl_lean_idle"], true, Twitch),
    p("gunner", "Machine gunner", "common", &["saw_gunner_idle"], true, Twitch),
    p("briefing_1", "Briefing (SAS 1)", "killhouse", &["killhouse_sas_1_idle"], true, Twitch),
    p("briefing_2", "Briefing (SAS 2)", "killhouse", &["killhouse_sas_2_idle"], true, Twitch),
    p("briefing_3", "Briefing (SAS 3)", "killhouse", &["killhouse_sas_3_idle"], true, Twitch),
    p("price", "Price, briefing", "killhouse", &["killhouse_sas_price_idle"], true, Twitch),
    p("gaz", "Gaz: talking, pointing", "killhouse", &["killhouse_gaz_idlea", "killhouse_gaz_idleb", "killhouse_gaz_talk_side", "killhouse_gaz_point_side"], false, Twitch),
    p("laptop", "At a laptop", "killhouse", &["killhouse_laptop_idle", "killhouse_laptop_lookup", "killhouse_laptop_twitch"], false, Twitch),
    p("passenger", "Sitting (vehicle passenger)", "killhouse", &["humvee_passenger_idle_l"], false, Twitch),
    p("bench", "Sitting on a bench", "killhouse", &["bm21_guy1_idle"], false, Twitch),
    p("sleeper", "Asleep in a chair", "blackout", &["parabolic_guard_sleeper_idle"], false, Twitch),
    p("slumped", "Asleep, slumped", "airplane", &["cargoship_sleeping_guy_idle_1", "cargoship_sleeping_guy_idle_2"], false, Twitch),
    p("chess_a", "Playing chess (white)", "blackout", &["parabolic_chessgame_idle_a"], false, Twitch),
    p("chess_b", "Playing chess (black)", "blackout", &["parabolic_chessgame_idle_b"], false, Twitch),
    p("binoculars", "Binoculars", "blackout", &["blackout_binoc_idle"], false, Twitch),
    p("phone_call", "Making a call", "icbm", &["parabolic_phoneguy_idle"], false, Twitch),
    p("spotter", "Spotter, waving", "hunted", &["hunted_spotter_idle", "hunted_spotter_wave_chat"], true, Twitch),
    p("jeer", "Guard, jeering", "coup", &["coup_guard1_idle", "coup_guard1_jeer"], true, Twitch),
    p("cower", "Cowering", "coup", &["unarmed_cowerstand_idle", "unarmed_cowercrouch_idle"], false, Random),
    p("unarmed", "Standing, no gun", "common", &["standunarmed_idle_loop"], false, Twitch),
    p("unarmed_crouch", "Crouched, no gun", "common", &["unarmed_crouch_idle1", "unarmed_crouch_twitch1"], false, Twitch),
    p("wounded", "Wounded, on his back", "common", &["dying_back_idle"], false, Twitch),
    p("waiting", "Waiting, waving over", "bog_a", &["bog_price_wait_idle", "bog_price_wait_wave_a", "bog_price_wait_wave_b"], true, Twitch),
    p("dealer", "Arms dealer", "sniperescape", &["sniper_escape_meeting_dealer_idle"], false, Twitch),
];

/// Guns to hold (multiplayer specs), `-` none.
pub const KITS: &[(&str, &str)] = &[
    ("auto", "side's rifle"),
    ("-", "no gun"),
    ("m4:", "M4"),
    ("m4:silencer", "M4 silenced"),
    ("m16:", "M16"),
    ("ak47:", "AK-47"),
    ("ak74u:", "AK-74u"),
    ("mp5:", "MP5"),
    ("p90:", "P90"),
    ("g3:", "G3"),
    ("g36c:", "G36C"),
    ("m14:", "M14"),
    ("saw:", "M249 SAW"),
    ("rpd:", "RPD"),
    ("m60e4:", "M60E4"),
    ("m40a3:", "M40A3"),
    ("dragunov:", "Dragunov"),
    ("winchester1200:", "W1200"),
];

/// A walked beat's length.
pub const BEAT: f32 = u(220.0);

/// An actor's line, taken apart.
#[derive(Clone, PartialEq, Debug)]
pub struct Spec {
    pub character: String,
    pub gun: String,
    pub preset: String,
}

impl Spec {
    pub fn parse(token: &str) -> Option<Spec> {
        let rest = token.strip_prefix(PREFIX)?;
        let mut it = rest.split('/');
        let character = it.next().filter(|c| !c.is_empty())?.to_owned();
        let gun = it.next().filter(|g| !g.is_empty()).unwrap_or("auto").to_owned();
        let preset = it.next().filter(|p| !p.is_empty()).unwrap_or(PRESETS[0].key).to_owned();
        Some(Spec { character, gun, preset })
    }

    pub fn token(&self) -> String {
        format!("{PREFIX}{}/{}/{}", self.character, self.gun, self.preset)
    }

    pub fn character(&self) -> Option<&'static Character> {
        crate::characters::character(&self.character)
    }

    pub fn preset(&self) -> &'static Preset {
        PRESETS.iter().find(|p| p.key == self.preset).unwrap_or(&PRESETS[0])
    }

    /// The gun in his hands (none if the idle's hands are busy).
    pub fn held_gun(&self) -> Option<String> {
        if !self.preset().armed {
            return None;
        }
        match self.gun.as_str() {
            "-" => None,
            "auto" => Some(if self.character().is_some_and(axis) { "ak47:" } else { "m4:" }.to_owned()),
            g => Some(g.to_owned()),
        }
    }

    pub fn label(&self) -> String {
        let who = self.character().map_or(self.character.as_str(), |c| c.name);
        let gun = match self.held_gun() {
            Some(g) => KITS.iter().find(|k| k.0 == g).map_or(g.clone(), |k| k.1.to_owned()),
            None => "no gun".into(),
        };
        format!("{who}, {} ({gun})", self.preset().label.to_ascii_lowercase())
    }

    /// The next (or previous) idle, character or gun.
    pub fn step_preset(&mut self, by: i32) {
        let i = PRESETS.iter().position(|p| p.key == self.preset).unwrap_or(0) as i32;
        self.preset = PRESETS[(i + by).rem_euclid(PRESETS.len() as i32) as usize].key.to_owned();
    }

    pub fn step_character(&mut self, by: i32) {
        let all = characters();
        let i = all.iter().position(|c| c.id == self.character).unwrap_or(0) as i32;
        self.character = all[(i + by).rem_euclid(all.len() as i32) as usize].id.to_owned();
    }

    pub fn step_gun(&mut self, by: i32) {
        let i = KITS.iter().position(|k| k.0 == self.gun).unwrap_or(0) as i32;
        self.gun = KITS[(i + by).rem_euclid(KITS.len() as i32) as usize].0.to_owned();
    }
}

/// The characters to place: CoD4's own.
pub fn characters() -> Vec<&'static Character> {
    CHARACTERS.iter().filter(|c| c.game == Game::Cod4).collect()
}

/// Fights for the other side (by its id, as the wardrobe sorts them).
fn axis(c: &Character) -> bool {
    ["spetsnaz", "opfor", "ultranat", "zakhaev", "victor", "al_asad"].iter().any(|p| c.id.starts_with(p))
}

/// The characters' models and the idles' animations, as they come in.
#[derive(Resource, Default)]
pub struct ActorAssets {
    bodies: HashMap<&'static str, Vec<Arc<PreparedModel>>>,
    anims: HashMap<&'static str, Vec<Arc<XAnim>>>,
    /// The ghost's stand-ins ([`stand_in`]), by character.
    pub statics: HashMap<&'static str, super::layout::Prepared>,
}

impl ActorAssets {
    pub fn reset(&mut self) {
        *self = ActorAssets::default();
    }
}

/// What's needed to dress and animate actors.
#[derive(bevy::ecs::system::SystemParam)]
pub struct Cast<'w> {
    pub assets: ResMut<'w, ActorAssets>,
    pub bindposes: ResMut<'w, Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>>,
    pub camo_materials: ResMut<'w, Assets<crate::gunmodel::CamoMaterial>>,
    pub camos: ResMut<'w, crate::gunmodel::WorldCamos>,
}

/// A character's models, from the map's zones or the character's own
/// level (asked for if it isn't here; `None` until then).
#[allow(clippy::too_many_arguments)]
pub fn bodies(
    c: &'static Character,
    assets: &mut ActorAssets,
    content: &mut Content,
    streamer: &mut Streamer,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    bindposes: &mut Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>,
    now: f32,
) -> Option<Vec<Arc<PreparedModel>>> {
    if let Some(b) = assets.bodies.get(c.id) {
        return Some(b.clone());
    }
    let here = c.models.iter().all(|m| content.find(m).is_some());
    let from: &mut Content = if here {
        content
    } else {
        match streamer.content(c.zone, now) {
            Some(z) => z,
            None => {
                streamer.want(c.zone);
                return None;
            }
        }
    };
    let models: Vec<Arc<PreparedModel>> = c.models.iter().filter_map(|m| from.model(m, meshes, materials, images, bindposes)).collect();
    if models.is_empty() {
        return None;
    }
    assets.bodies.insert(c.id, models.clone());
    Some(models)
}

/// A character's models unskinned, as they're bound (the palette's picture,
/// the ghost being placed), from `from` (holding them).
pub fn stand_in(c: &Character, from: &mut Content, meshes: &mut Assets<Mesh>, materials: &mut Assets<StandardMaterial>, images: &mut Assets<Image>) -> super::layout::Prepared {
    let mut parts = Vec::new();
    for m in c.models {
        if let Some(p) = crate::props::static_parts(from, m, meshes, materials, images) {
            parts.extend(p);
        }
    }
    let (min, max) = bounds();
    super::layout::Prepared { parts, min, max, boxes: vec![], solid: false, surface: 0 }
}

/// An idle's animations (asked for if their level isn't here).
pub fn anims(preset: &'static Preset, assets: &mut ActorAssets, content: &mut Content, streamer: &mut Streamer, now: f32) -> Option<Vec<Arc<XAnim>>> {
    if let Some(a) = assets.anims.get(preset.key) {
        return Some(a.clone());
    }
    // (Looked for first: `Content::anim` warns of what it hasn't.)
    let has = |c: &Content, n: &str| c.generic(iw3::zone::AssetType::XAnimParts, n).is_some();
    let mut got = Vec::new();
    for n in preset.anims {
        if has(content, n)
            && let Some(a) = content.anim(n)
        {
            got.push(a);
        }
    }
    if got.len() < preset.anims.len() {
        match streamer.content(preset.zone, now) {
            Some(z) => {
                got.clear();
                for n in preset.anims {
                    let a = if has(content, n) { content.anim(n) } else { z.anim(n) };
                    got.extend(a);
                }
            }
            None => {
                streamer.want(preset.zone);
                return None;
            }
        }
    }
    if got.is_empty() {
        warn!("map editor: no animations for {} in {}", preset.key, preset.zone);
    }
    assets.anims.insert(preset.key, got.clone());
    Some(got)
}

/// An actor's idles going.
#[derive(Component)]
pub struct Idling {
    anims: Vec<Arc<XAnim>>,
    mode: Mode,
    /// Playing `anims[current]` until then.
    until: f32,
    current: usize,
    /// A beat: from, to, which end it's making for, and at what pace.
    beat: Option<(Vec3, Vec3, bool, f32)>,
}

/// Spawns the actor of `placed` (index `i`) under `root`; false if its
/// character or idle isn't here yet (asked for).
#[allow(clippy::too_many_arguments)]
pub fn spawn(
    commands: &mut Commands,
    models: &mut super::Models,
    placed: &super::layout::Placed,
    spec: &Spec,
    i: usize,
    root: Entity,
    now: f32,
) -> bool {
    use crate::models::{AnimPlayer, Skeleton, SpawnModel, spawn_model};
    let Some(c) = spec.character() else { return false };
    let m = &mut *models;
    let Some(body) = bodies(c, &mut m.cast.assets, &mut m.content, &mut m.streamer, &mut m.meshes, &mut m.materials, &mut m.images, &mut m.cast.bindposes, now) else {
        return false;
    };
    let preset = spec.preset();
    let Some(anims) = anims(preset, &mut m.cast.assets, &mut m.content, &mut m.streamer, now) else { return false };
    let (min, max) = bounds();
    let tf = Transform::from_translation(placed.pos).with_rotation(placed.rot).with_scale(Vec3::splat(placed.scale));
    let owner = commands
        .spawn((
            Name::new(format!("placed actor {}", spec.character)),
            super::layout::PlacedObject(i),
            super::layout::ObjectBounds { min, max },
            tf,
            Visibility::default(),
            ChildOf(root),
        ))
        .id();
    // A box keeps players (and bullets) out of him.
    let s = placed.scale;
    let half = (max - min) * 0.5 * s;
    commands.entity(owner).insert((
        avian3d::prelude::RigidBody::Static,
        avian3d::prelude::Collider::compound(vec![(Vec3::Y * half.y, Quat::IDENTITY, avian3d::prelude::Collider::cuboid(half.x * 2.0, half.y * 2.0, half.z * 2.0))]),
        avian3d::prelude::CollisionLayers::new(crate::collision::Layer::World, avian3d::prelude::LayerMask::NONE),
        avian3d::prelude::Position(placed.pos),
        avian3d::prelude::Rotation(placed.rot),
    ));
    let mut skeleton = Skeleton::default();
    for b in &body {
        let surfaces = spawn_model(commands, &mut skeleton, SpawnModel { model: b, owner, attach_to: None, layers: None, shadows: true });
        if crate::character_look::is_character(&b.name) {
            crate::character_look::mark(commands, &surfaces);
        }
    }
    if let Some(gun) = spec.held_gun() {
        let hand = skeleton.joint("tag_weapon_right");
        let mut assets = crate::gunmodel::GunAssets {
            meshes: &mut m.meshes,
            materials: &mut m.materials,
            images: &mut m.images,
            bindposes: &mut m.cast.bindposes,
            camo_materials: &mut m.cast.camo_materials,
        };
        let target = crate::gunmodel::GunTarget { owner, attach_to: hand, layers: None };
        crate::gunmodel::spawn_held_gun(commands, &mut m.content, &mut m.cast.camos.0, &mut assets, &mut skeleton, &gun, 0, target);
    }
    let mut player = AnimPlayer::default();
    if let Some(first) = anims.first() {
        player.play(first.clone(), 0.0);
        // Not all in step.
        player.time = (i as f32 * 1.37) % first.duration().max(0.1);
    }
    let beat = (preset.mode == Mode::Walk).then(|| {
        let ahead = placed.rot * Vec3::X * BEAT;
        let pace = anims.first().and_then(|a| own_pace(a)).unwrap_or(u(50.0));
        (placed.pos, placed.pos + ahead.with_y(0.0), true, pace)
    });
    let first_for = anims.first().map_or(5.0, |a| a.duration().max(1.0));
    commands.entity(owner).insert((skeleton, player, Idling { anims, mode: preset.mode, until: now + first_for * 2.0, current: 0, beat }));
    true
}

/// A standing person's box (model space).
pub fn bounds() -> (Vec3, Vec3) {
    (Vec3::new(-0.35, 0.0, -0.35), Vec3::new(0.35, 1.8, 0.35))
}

fn own_pace(anim: &XAnim) -> Option<f32> {
    let d = anim.delta_at(1.0);
    let own = Vec2::new(d[0], d[1]).length() / anim.duration().max(1e-3);
    (anim.delta && own > 5.0).then(|| u(own))
}

/// The actors' idles: twitches, random turns, beats walked.
pub fn idle(time: Res<Time>, mut actors: Query<(&mut Idling, &mut Transform, &mut crate::models::AnimPlayer)>) {
    let now = time.elapsed_secs();
    let dt = time.delta_secs().min(0.1);
    for (mut a, mut tf, mut player) in &mut actors {
        if a.anims.is_empty() {
            continue;
        }
        let n = a.anims.len();
        if let Some((from, to, out, pace)) = a.beat {
            // Walking (the first), or resting at an end (until `until`).
            if now < a.until && a.current != 0 {
                continue;
            }
            let target = if out { to } else { from };
            let flat = (target - tf.translation).with_y(0.0);
            if flat.length() < u(10.0) {
                a.beat = Some((from, to, !out, pace));
                if n > 1 {
                    let k = 1 + rand::random_range(0..n - 1);
                    let rest = a.anims[k].clone();
                    a.until = now + rest.duration().clamp(3.0, 10.0);
                    a.current = k;
                    player.play(rest, 0.5);
                }
                continue;
            }
            let dir = flat.normalize();
            let want = Quat::from_rotation_y((-dir.z).atan2(dir.x));
            tf.rotation = tf.rotation.slerp(want, 1.0 - (-4.0 * dt).exp());
            if tf.rotation.angle_between(want) < 0.5 {
                tf.translation += dir * pace * dt;
                if a.current != 0 || player.anim.is_none() {
                    a.current = 0;
                    let walk = a.anims[0].clone();
                    player.play(walk, 0.5);
                }
            }
            continue;
        }
        if now < a.until || n < 2 {
            continue;
        }
        let k = match a.mode {
            Mode::Random => (a.current + 1 + rand::random_range(0..n - 1)) % n,
            // Back to the idle after a twitch; a twitch now and then.
            _ if a.current != 0 => 0,
            _ if rand::random_bool(0.45) => 1 + rand::random_range(0..n - 1),
            _ => 0,
        };
        let next = a.anims[k].clone();
        let len = next.duration().max(1.0);
        a.until = now + if k == 0 && a.mode != Mode::Random { len * rand::random_range(1.5..3.0) } else { len };
        a.current = k;
        player.play(next, 0.35);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_round_trip() {
        let s = Spec::parse("@actor:marine_assault/m4:silencer/smoke").unwrap();
        assert_eq!(s.character, "marine_assault");
        assert_eq!(s.gun, "m4:silencer");
        assert_eq!(s.preset().key, "smoke");
        assert_eq!(Spec::parse(&s.token()), Some(s));
        let d = Spec::parse("@actor:gaz").unwrap();
        assert_eq!((d.gun.as_str(), d.preset.as_str()), ("auto", "at_ease"));
        assert!(Spec::parse("@actor:").is_none());
    }

    #[test]
    fn presets_unique() {
        for (i, a) in PRESETS.iter().enumerate() {
            assert!(!a.anims.is_empty());
            assert!(PRESETS[i + 1..].iter().all(|b| b.key != a.key), "{}", a.key);
            assert!(!a.key.contains('/'));
        }
    }
}

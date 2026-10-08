//! In-match audio: CoD4's own sounds, played with each alias's volume and
//! pitch ranges and distance falloff, heard in 3D around the camera
//! ([`spatial`]: direction, distance and walls).
//!
//! The aliases come from `localized_common_mp` (where CoD4 keeps every
//! multiplayer sound; loaded in the background as the match starts) and the
//! map's zone. Gameplay code asks for sounds through [`Sfx`]; the hooks in
//! [`events`] (gunfire, impacts, footsteps, pain, reloads, ...) and
//! [`matchflow`] (music, the announcer, ambience) do most of the asking.

pub mod bank;
mod events;
mod matchflow;
mod spatial;

use crate::combat::Team;
use crate::content::Content;
use crate::player::MainCamera;
use crate::units::INCH;
use bank::{Aliases, Sources, Variant};
use bevy::audio::{AddAudioSource, AudioSinkPlayback, Volume};
use bevy::prelude::*;
use std::sync::Arc;
use std::thread::JoinHandle;

pub struct AudioPlugin;

/// Debug runs (`COD4RW_*`) are silent. `GlobalVolume` only scales a sound as
/// it starts; sounds whose volume is set again later (falloff, ducking)
/// would come back, so every volume also goes through [`audible`].
static MUTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 0 in a silent debug run, else 1: a factor for every volume set.
pub fn audible() -> f32 {
    if MUTED.load(std::sync::atomic::Ordering::Relaxed) { 0.0 } else { 1.0 }
}

impl Plugin for AudioPlugin {
    fn build(&self, app: &mut App) {
        // Debug runs (`COD4RW_*`) stay quiet.
        if std::env::vars().any(|(k, _)| k.starts_with("COD4RW_") && !crate::net::setting(&k)) {
            app.insert_resource(GlobalVolume::new(Volume::SILENT));
            MUTED.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        app.add_audio_source::<spatial::SpatialSound>()
            .init_resource::<Sfx>()
            .add_systems(OnEnter(crate::state::GameState::InGame), load_bank.after(crate::world::load_map).in_set(crate::state::Setup::Content))
            .add_systems(Update, add_listener.run_if(crate::state::in_game))
            .init_resource::<Ducking>()
            // A new sound's place only exists once transforms are
            // propagated: its ears are set after (before, they took the
            // world's origin for its place on its first frame).
            .add_systems(
                PostUpdate,
                (
                    (play_queued, duck).chain().before(bevy::transform::TransformSystems::Propagate),
                    spatial::spatialize.after(bevy::transform::TransformSystems::Propagate),
                )
                    .run_if(crate::state::in_game),
            );
        events::build(app);
        matchflow::build(app);
    }
}

/// Overall volume, under the aliases' own.
/// What a sound is, for the settings' volumes: the announcer and battle
/// chatter (`US_1mc_*`, `UK_mp_rsp_*`, ...) are voices, the match's music
/// music, the rest effects.
fn category(alias: &str) -> crate::settings_apply::Sound {
    use crate::settings_apply::Sound;
    let a = alias.to_ascii_lowercase();
    let voice_prefix = ["us_", "uk_", "ab_", "ru_", "sas_", "spetsnaz_", "opfor_", "marines_"].iter().any(|p| a.starts_with(p));
    if voice_prefix || a.contains("_1mc_") || a.contains("_mp_rsp_") || a.contains("_mp_cmd_") || a.contains("_mp_stm_") || a.contains("_mp_inform_") {
        Sound::Voice
    } else if a.starts_with("mus_") || a.contains("music") || a.starts_with("mp_victory") || a.starts_with("mp_defeat") || a.starts_with("mp_suspense") || a.starts_with("mp_spawn_") || a.starts_with("mp_time_running_out") {
        Sound::Music
    } else {
        Sound::Effects
    }
}
/// Ear spacing for Bevy's listener (it only marks the listener: the
/// panning is [`spatial`]'s), in metres.
const EAR_GAP: f32 = 0.25;

/// Sounds to start: gameplay code pushes, [`play_queued`] plays.
#[derive(Resource, Default)]
pub struct Sfx {
    queue: Vec<Request>,
    later: Vec<(f32, Request)>,
    /// How muffled the player's hearing is (0 normal, 1 deaf): a
    /// flashbang's ([`crate::grenades`]).
    pub deafness: f32,
}

#[derive(Clone)]
pub struct Request {
    pub alias: String,
    /// Where, or `None` for the listener's own sounds (heard without position).
    pub at: Option<Vec3>,
    pub gain: f32,
    /// Riding on this entity (a helicopter's rotors), ending with it.
    pub on: Option<Entity>,
    /// Heard without position even so (a bed of sound round the listener,
    /// its volume kept by whoever asked: [`crate::weather`]'s rain).
    pub flat: bool,
}

impl Sfx {
    /// Play `alias` at `at` (Bevy space), or as the player's own if `None`.
    pub fn play(&mut self, alias: impl Into<String>, at: Option<Vec3>) {
        self.queue.push(Request { alias: alias.into(), at, gain: 1.0, on: None, flat: false });
    }

    /// Play `alias` on `entity`, following it (a loop plays until it's
    /// despawned).
    pub fn play_on(&mut self, alias: impl Into<String>, entity: Entity) {
        self.queue.push(Request { alias: alias.into(), at: None, gain: 1.0, on: Some(entity), flat: false });
    }

    /// The same, `delay` seconds from `now`.
    pub fn play_later(&mut self, alias: impl Into<String>, at: Option<Vec3>, now: f32, delay: f32) {
        self.later.push((now + delay, Request { alias: alias.into(), at, gain: 1.0, on: None, flat: false }));
    }

    /// Play `alias` without position, riding on `entity` (its playback a
    /// child of it, its volume for the asker to keep: [`AudioSink`]).
    pub fn play_flat_on(&mut self, alias: impl Into<String>, entity: Entity) {
        self.queue.push(Request { alias: alias.into(), at: None, gain: 1.0, on: Some(entity), flat: true });
    }

    /// The same at `gain` of its volume.
    pub fn play_later_at_gain(&mut self, alias: impl Into<String>, at: Option<Vec3>, gain: f32, now: f32, delay: f32) {
        self.later.push((now + delay, Request { alias: alias.into(), at, gain, on: None, flat: false }));
    }
}

/// Each team's voice and music, by whose soldiers the map has.
#[derive(Resource)]
pub struct Sides {
    allies: Side,
    axis: Side,
}

#[derive(Clone, Copy, Debug)]
pub struct Side {
    /// `generic_pain_<nationality>_N`, `generic_death_...`.
    pub nationality: &'static str,
    /// Announcer and squad voices: `<voice>_1mc_...`, `<voice>_mp_stm_...`.
    pub voice: &'static str,
    /// `mp_spawn_<music>`, `mp_victory_<music>`.
    pub music: &'static str,
}

impl Sides {
    pub fn of(&self, team: Team) -> Side {
        match team {
            Team::Allies => self.allies,
            Team::Axis => self.axis,
        }
    }

    fn for_map(content: &Content) -> Sides {
        let models: Vec<&str> = content
            .map()
            .assets
            .iter()
            .filter_map(|a| match a {
                iw3::zone::Asset::XModel(x) => Some(x.name.as_str()),
                _ => None,
            })
            .collect();
        let has = |p: &str| models.iter().any(|m| m.starts_with("body_mp_") && m.contains(p));
        let side = |nationality, voice, music| Side { nationality, voice, music };
        Sides {
            allies: if has("sas") { side("british", "UK", "sas") } else { side("american", "US", "usa") },
            axis: if has("russian") || has("spetsnaz") { side("russian", "RU", "soviet") } else { side("arab", "AB", "opfor") },
        }
    }
}

/// The sound aliases this match can play.
#[derive(Resource)]
pub struct Bank {
    aliases: Aliases,
    loading: Option<JoinHandle<anyhow::Result<Vec<(String, Vec<Variant>)>>>>,
    sources: Sources,
    vfs: Arc<iw3::iwd::Vfs>,
}

impl Bank {
    pub fn has(&self, alias: &str) -> bool {
        self.aliases.contains_key(&alias.to_ascii_lowercase())
    }

    fn add(&mut self, list: Vec<(String, Vec<Variant>)>) {
        for (name, variants) in list {
            if !variants.is_empty() {
                self.aliases.entry(name).or_insert_with(|| Arc::new(variants));
            }
        }
    }
}

/// The map zone's aliases now, `localized_common_mp`'s in the background.
fn load_bank(mut commands: Commands, content: Res<Content>) {
    commands.insert_resource(Sides::for_map(&content));
    let mut bank = Bank { aliases: Aliases::new(), loading: None, sources: Sources::default(), vfs: content.vfs.clone() };
    for zone in &content.zones {
        bank.add(bank::aliases(zone));
    }
    bank.loading = Some(std::thread::spawn(|| {
        let install = iw3::Install::locate()?;
        let data = iw3::fastfile::load(&install.zone_path("localized_common_mp"))?;
        let zone = iw3::zone::Zone::parse(&data, iw3::zone::ParseOptions::default())?;
        let mut list = bank::aliases(&zone);
        // Black Ops guns' own sounds, when it's installed.
        if crate::bo1::data().is_some() {
            match t5::Install::locate().and_then(|i| t5::sound::load_bank(&i)) {
                Ok(b) => list.extend(bank::bo1_aliases(&Arc::new(b))),
                Err(e) => warn!("audio: no Black Ops sounds: {e:#}"),
            }
        }
        // World at War guns' own sounds, when it's installed.
        if let Some(waw) = crate::waw::data() {
            match waw.sound_aliases() {
                Ok(l) => list.extend(l),
                Err(e) => warn!("audio: no World at War sounds: {e:#}"),
            }
        }
        Ok(list)
    }));
    commands.insert_resource(bank);
}

fn add_listener(mut commands: Commands, cameras: Query<Entity, (With<MainCamera>, Without<SpatialListener>)>) {
    for e in &cameras {
        commands.entity(e).insert(SpatialListener::new(EAR_GAP));
    }
}

fn play_queued(
    mut commands: Commands,
    time: Res<Time>,
    mut sfx: ResMut<Sfx>,
    bank: Option<ResMut<Bank>>,
    mut audio: ResMut<Assets<AudioSource>>,
    mut spatial_sounds: ResMut<Assets<spatial::SpatialSound>>,
    listener: Query<&GlobalTransform, With<SpatialListener>>,
    carriers: Query<&GlobalTransform>,
) {
    let Some(mut bank) = bank else {
        sfx.queue.clear();
        return;
    };
    if bank.loading.as_ref().is_some_and(|t| t.is_finished()) {
        match bank.loading.take().map(|t| t.join()) {
            Some(Ok(Ok(list))) => {
                bank.add(list);
                info!("audio: {} sound aliases", bank.aliases.len());
            }
            Some(Ok(Err(e))) => warn!("audio: no localized_common_mp sounds: {e:#}"),
            _ => warn!("audio: loading sounds panicked"),
        }
    }
    let now = time.elapsed_secs();
    let due: Vec<Request> = {
        let (ready, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut sfx.later).into_iter().partition(|(at, _)| *at <= now);
        sfx.later = waiting;
        ready.into_iter().map(|(_, r)| r).collect()
    };
    let listener = listener.iter().next().copied().unwrap_or_default();
    let ear = listener.translation();
    let hearing = 1.0 - sfx.deafness.clamp(0.0, 0.95);
    let mut queue: Vec<(Request, u8)> = std::mem::take(&mut sfx.queue).into_iter().chain(due).map(|r| (r, 0)).collect();
    while let Some((req, depth)) = queue.pop() {
        let Some(variants) = bank.aliases.get(&req.alias.to_ascii_lowercase()).cloned() else {
            debug!("audio: no alias {}", req.alias);
            continue;
        };
        let Some(v) = bank::pick(&variants) else { continue };
        let carrier = match req.on {
            Some(e) => match carriers.get(e) {
                Ok(g) => Some((e, g.translation())),
                Err(_) => continue,
            },
            None => None,
        };
        // A looping alias loops only on something that ends it (`play_on`):
        // played free it would never stop (the S&D bomb's `bomb_tick`, a
        // loop played each second, ticked on into the next round).
        let mut v = v.clone();
        v.looping &= carrier.is_some();
        let v = &v;
        let two_d = v.two_d || req.flat || req.at.is_none() && carrier.is_none();
        let at = carrier.map(|c| c.1).or(req.at).unwrap_or(ear);
        let falloff = if two_d { 1.0 } else { v.falloff(ear.distance(at) / INCH) };
        // A loop on something moving may come into earshot.
        if falloff <= 0.001 && !(v.looping && carrier.is_some()) {
            continue;
        }
        let vfs = bank.vfs.clone();
        let Some(source) = bank.sources.handle(&v.file, &vfs, &mut audio) else { continue };
        let range = |(a, b): (f32, f32)| if b > a { rand::random_range(a..b) } else { a };
        let volume = range(v.volume) * req.gain * crate::settings_apply::volume(category(&req.alias)) * hearing;
        // The sink takes the alias's volume; a positioned sound's falloff,
        // direction and walls are its ears' ([`spatial`]), kept up to date.
        let settings = PlaybackSettings {
            volume: Volume::Linear(volume * audible()),
            speed: range(v.pitch).clamp(0.25, 4.0),
            ..if v.looping { PlaybackSettings::LOOP } else { PlaybackSettings::DESPAWN }
        };
        debug!("audio: {} vol {:.2}{} at {:.1} m{}", req.alias, volume * falloff, if two_d { "" } else { " 3d" }, ear.distance(at), if carrier.is_some() { " (on entity)" } else { "" });
        let place = match carrier {
            Some((c, _)) => (Transform::default(), Some(ChildOf(c))),
            None => (Transform::from_translation(at), None),
        };
        let mut e = if two_d {
            commands.spawn((AudioPlayer(source), settings, place.0, FlatSound))
        } else {
            let Some(clip) = audio.get(&source).cloned() else { continue };
            let ears = std::sync::Arc::new(spatial::Ears::default());
            // Heard right from the first sample.
            let local = listener.affine().inverse().transform_vector3(at - ear);
            ears.set(&spatial::EarSettings::new(Vec3::new(local.x, local.y, -local.z), ear.distance(at) / INCH, falloff * audible(), 0.0));
            let sound = spatial_sounds.add(spatial::SpatialSound { source: clip, ears: ears.clone(), looping: v.looping });
            // (Looped inside the sound, so its ears keep up: `SpatialSound`.)
            let settings = PlaybackSettings { mode: if v.looping { bevy::audio::PlaybackMode::Once } else { settings.mode }, ..settings };
            commands.spawn((AudioPlayer(sound), settings, place.0, spatial::Emitter::new(req.alias.clone(), ears, v.clone(), 1.0)))
        };
        if let Some(parent) = place.1 {
            e.insert(parent);
        }
        if v.master {
            e.insert(Master);
        }
        if let Some(percentage) = v.slave {
            e.insert(Slave { volume, percentage });
        }
        if let Some(layer) = v.secondary.as_ref().filter(|_| depth < 2) {
            queue.push((Request { alias: layer.clone(), ..req.clone() }, depth + 1));
        }
    }
}

/// A sound heard without position (its sink an [`AudioSink`]).
#[derive(Component)]
pub struct FlatSound;

/// A master sound playing (CoD4 ducks slave sounds meanwhile).
#[derive(Component)]
struct Master;

/// A slave sound: its volume undimmed, and how far ducking takes it.
#[derive(Component)]
struct Slave {
    volume: f32,
    percentage: f32,
}

/// CoD4's `slaveLerp`: 0 to 1 over `snd_slaveFadeTime` (0.5 s) while any
/// master sound plays, back down after.
#[derive(Resource, Default)]
struct Ducking {
    lerp: f32,
}

/// `snd_slaveFadeTime`.
const SLAVE_FADE: f32 = 0.5;

impl Ducking {
    /// A slave sound's volume scale (`SND_GetLerpedSlavePercentage`).
    fn scale(&self, percentage: f32) -> f32 {
        1.0 - (1.0 - percentage) * self.lerp
    }
}

fn duck(
    time: Res<Time>,
    mut ducking: ResMut<Ducking>,
    masters: Query<(), With<Master>>,
    mut slaves: Query<(&Slave, &mut AudioSink)>,
) {
    let step = time.delta_secs() / SLAVE_FADE;
    let lerp = if masters.is_empty() { ducking.lerp - step } else { ducking.lerp + step };
    let lerp = lerp.clamp(0.0, 1.0);
    if lerp == ducking.lerp && lerp == 0.0 {
        return;
    }
    ducking.lerp = lerp;
    for (s, mut sink) in &mut slaves {
        sink.set_volume(Volume::Linear(s.volume * ducking.scale(s.percentage) * audible()));
    }
}

//! Full-screen Bink cinematics from the install's `main/video` (the
//! campaign's loading briefings, `cod_intro`, logos), decoded by the
//! [`bink`] crate on a thread of their own: the picture into an image over
//! everything, the soundtrack streamed into Bevy's audio.
//!
//! Debug: `COD4RW_CINEMATIC=<name>` plays `main/video/<name>.bik` at start
//! (with `COD4RW_CINEMATIC_SHOTS=<dir>` saving a frame every
//! `COD4RW_CINEMATIC_EVERY` seconds (2), then exiting when it ends).

use bevy::asset::RenderAssetUsages;
use bevy::audio::{AddAudioSource, ChannelCount, Decodable, SampleRate, Source};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};

pub struct CinematicPlugin;

impl Plugin for CinematicPlugin {
    fn build(&self, app: &mut App) {
        if !crate::audio::silent_run() {
            app.add_audio_source::<StreamSound>();
        } else {
            app.init_asset::<StreamSound>();
        }
        app.init_resource::<Queue>().add_systems(Update, (show, skip, next_in_queue, shots).chain());
        // CoD4's first launch (`com_introPlayed`): the logos and the intro,
        // once. Only where the campaign is on (test builds).
        if crate::modes::test_features() && std::env::vars().all(|(k, _)| !k.starts_with("COD4RW_") || k == "COD4RW_TESTFEATURES") && !intro_played() {
            app.add_systems(Startup, |mut q: ResMut<Queue>| {
                q.0 = ["IW_logo", "atvi", "cod_intro"].iter().map(|s| s.to_string()).collect();
                q.1 = false;
                mark_intro_played();
            });
        }
        if let Ok(name) = std::env::var("COD4RW_CINEMATIC") {
            app.add_systems(Startup, move |mut commands: Commands| {
                commands.queue(Play { name: name.clone(), skippable: true });
            });
        }
    }
}

/// The movie playing.
#[derive(Resource)]
pub struct Cinematic {
    pub name: String,
    frames: Mutex<Receiver<Decoded>>,
    /// Frames decoded ahead, waiting their time.
    ahead: VecDeque<Decoded>,
    image: Handle<Image>,
    width: u32,
    height: u32,
    fps: f64,
    pub frame_count: usize,
    /// Real time (s) of its first frame, and the frame on screen.
    started: Option<f64>,
    pub shown: Option<usize>,
    audio: Option<Arc<Mutex<VecDeque<f32>>>>,
    /// The decoder has nothing more.
    decoded_all: bool,
    pub finished: bool,
    /// Keys and buttons may end it.
    pub skippable: bool,
    /// What it put on screen (despawned with it), and the full-screen node
    /// others may add to (a loading bar, a prompt).
    entities: Vec<Entity>,
    pub root: Entity,
}

struct Decoded {
    index: usize,
    rgba: Vec<u8>,
    audio: Vec<f32>,
}

/// Play `main/video/<name>.bik`.
pub struct Play {
    pub name: String,
    pub skippable: bool,
}

impl bevy::ecs::system::Command for Play {
    type Out = ();

    fn apply(self, world: &mut World) {
        stop(world);
        let Ok(install) = iw3::Install::locate() else { return };
        let path = install.main_dir().join("video").join(format!("{}.bik", self.name));
        let Ok(data) = std::fs::read(&path) else {
            warn!("cinematic: {} not found", path.display());
            return;
        };
        let movie = match bink::Bink::open(data) {
            Ok(m) => m,
            Err(e) => {
                warn!("cinematic: {}: {e:#}", self.name);
                return;
            }
        };
        let (w, h, fps, count) = (movie.width as u32, movie.height as u32, movie.fps, movie.frame_count());
        let track = movie.tracks.first().map(|t| (t.sample_rate, t.channels));
        info!("cinematic: {} ({w}x{h}, {fps:.2} fps, {count} frames, audio {track:?})", self.name);
        let (tx, rx) = std::sync::mpsc::sync_channel::<Decoded>(6);
        std::thread::Builder::new().name(format!("cinematic {}", self.name)).spawn(move || decode(movie, tx)).ok();
        let image = world.resource_mut::<Assets<Image>>().add(Image::new_fill(
            Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            TextureDimension::D2,
            &[0, 0, 0, 255],
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
        ));
        let mut entities = Vec::new();
        // Over everything: its own camera and a black backdrop, the picture
        // fitted in.
        entities.push(world.spawn((Camera2d, Camera { order: 1000, ..default() })).id());
        let cam = entities[0];
        let root = world
            .spawn((
                Node { width: percent(100), height: percent(100), position_type: PositionType::Absolute, justify_content: JustifyContent::Center, align_items: AlignItems::Center, ..default() },
                BackgroundColor(Color::BLACK),
                GlobalZIndex(5000),
                UiTargetCamera(cam),
            ))
            .id();
        entities.push(root);
        world.spawn((
            ImageNode::new(image.clone()),
            Node { width: percent(100), max_height: percent(100), aspect_ratio: Some(w as f32 / h as f32), ..default() },
            ChildOf(root),
        ));
        let audio = track.map(|(rate, channels)| {
            let buf = Arc::new(Mutex::new(VecDeque::new()));
            let sound = world.resource_mut::<Assets<StreamSound>>().add(StreamSound { buf: buf.clone(), rate, channels: channels as u16 });
            let volume = bevy::audio::Volume::Linear(crate::audio::audible());
            entities.push(world.spawn((AudioPlayer(sound), PlaybackSettings { volume, ..PlaybackSettings::DESPAWN })).id());
            buf
        });
        world.insert_resource(Cinematic {
            name: self.name,
            frames: Mutex::new(rx),
            ahead: VecDeque::new(),
            image,
            width: w,
            height: h,
            fps,
            frame_count: count,
            started: None,
            shown: None,
            audio,
            decoded_all: false,
            finished: false,
            skippable: self.skippable,
            entities,
            root,
        });
    }
}

/// End the movie playing (if any) and take it off the screen.
pub fn stop(world: &mut World) {
    if let Some(c) = world.remove_resource::<Cinematic>() {
        for e in c.entities {
            if let Ok(e) = world.get_entity_mut(e) {
                e.despawn();
            }
        }
    }
}

/// The decoder thread: frames as fast as the channel takes them.
fn decode(mut movie: bink::Bink, tx: SyncSender<Decoded>) {
    loop {
        let frame = match movie.next_frame() {
            Ok(Some(f)) => f,
            Ok(None) => return,
            Err(e) => {
                warn!("cinematic: frame decode failed: {e:#}");
                return;
            }
        };
        let mut rgba = Vec::new();
        movie.rgba(&mut rgba);
        let mut item = Decoded { index: frame.index, rgba, audio: frame.audio.into_iter().next().unwrap_or_default() };
        loop {
            match tx.try_send(item) {
                Ok(()) => break,
                Err(TrySendError::Full(back)) => {
                    item = back;
                    std::thread::sleep(std::time::Duration::from_millis(4));
                }
                Err(TrySendError::Disconnected(_)) => return,
            }
        }
    }
}

/// Put the frame that's due on screen, and its sound into the stream.
fn show(time: Res<Time<Real>>, cin: Option<ResMut<Cinematic>>, mut images: ResMut<Assets<Image>>) {
    let Some(mut cin) = cin else { return };
    let cin = &mut *cin;
    while cin.ahead.len() < 4 {
        let got = cin.frames.get_mut().map_or(Err(std::sync::mpsc::TryRecvError::Disconnected), |r| r.try_recv());
        match got {
            Ok(f) => cin.ahead.push_back(f),
            Err(std::sync::mpsc::TryRecvError::Empty) => break,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                cin.decoded_all = true;
                break;
            }
        }
    }
    let now = time.elapsed_secs_f64();
    if cin.started.is_none() && !cin.ahead.is_empty() {
        cin.started = Some(now);
    }
    let Some(t0) = cin.started else { return };
    let due = ((now - t0) * cin.fps) as usize;
    let mut latest = None;
    while cin.ahead.front().is_some_and(|f| f.index <= due) {
        let f = cin.ahead.pop_front().expect("front");
        if let Some(buf) = &cin.audio
            && let Ok(mut b) = buf.lock()
        {
            b.extend(f.audio.iter().copied());
        }
        latest = Some(f);
    }
    if let Some(f) = latest {
        cin.shown = Some(f.index);
        if let Some(mut img) = images.get_mut(&cin.image) {
            if img.width() == cin.width && img.height() == cin.height {
                img.data = Some(f.rgba);
            }
        }
    }
    if cin.decoded_all && cin.ahead.is_empty() && due >= cin.frame_count {
        cin.finished = true;
    }
}

/// A Bink soundtrack, streamed: samples arrive as frames are shown.
#[derive(Asset, TypePath, Clone)]
pub struct StreamSound {
    buf: Arc<Mutex<VecDeque<f32>>>,
    rate: u32,
    channels: u16,
}

pub struct StreamDecoder(StreamSound);

impl Decodable for StreamSound {
    type Decoder = StreamDecoder;

    fn decoder(&self) -> Self::Decoder {
        StreamDecoder(self.clone())
    }
}

impl Iterator for StreamDecoder {
    type Item = bevy::audio::Sample;

    fn next(&mut self) -> Option<Self::Item> {
        // Silence while it waits for more (never ends: the cinematic's end
        // despawns the player).
        Some(self.0.buf.lock().ok().and_then(|mut b| b.pop_front()).unwrap_or(0.0))
    }
}

impl Source for StreamDecoder {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> ChannelCount {
        ChannelCount::new(self.0.channels.max(1)).expect("channels")
    }

    fn sample_rate(&self) -> SampleRate {
        SampleRate::new(self.0.rate.max(1)).expect("rate")
    }

    fn total_duration(&self) -> Option<std::time::Duration> {
        None
    }
}

/// `COD4RW_CINEMATIC_SHOTS=<dir>`: a frame every so often, then exit once
/// the movie's over.
fn shots(
    mut commands: Commands,
    time: Res<Time<Real>>,
    cin: Option<Res<Cinematic>>,
    mut next: Local<f64>,
    mut n: Local<u32>,
    mut ended_at: Local<Option<f64>>,
    mut exit: MessageWriter<AppExit>,
) {
    use bevy::render::view::screenshot::{Screenshot, save_to_disk};
    let Ok(dir) = std::env::var("COD4RW_CINEMATIC_SHOTS") else { return };
    let now = time.elapsed_secs_f64();
    let Some(cin) = cin else {
        if let Some(t) = *ended_at
            && now - t > 1.0
        {
            exit.write(AppExit::Success);
        }
        return;
    };
    if cin.finished {
        ended_at.get_or_insert(now);
        if ended_at.is_some_and(|t| now - t > 0.5) {
            exit.write(AppExit::Success);
        }
        return;
    }
    let every = std::env::var("COD4RW_CINEMATIC_EVERY").ok().and_then(|s| s.parse().ok()).unwrap_or(2.0);
    if cin.shown.is_some() && now >= *next {
        *next = now + every;
        std::fs::create_dir_all(&dir).ok();
        let frame = cin.shown.unwrap_or(0);
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(std::path::PathBuf::from(&dir).join(format!("{:03}_f{frame:05}.png", *n))));
        *n += 1;
    }
}

/// Movies to play one after another (each skippable).
#[derive(Resource, Default)]
pub struct Queue(pub std::collections::VecDeque<String>, bool);

/// The next queued movie once the queue's last is over (others' movies
/// are theirs to end).
fn next_in_queue(mut commands: Commands, cin: Option<Res<Cinematic>>, mut q: ResMut<Queue>) {
    if cin.as_ref().is_some_and(|c| !c.finished) {
        return;
    }
    if cin.is_some() && !q.1 {
        return;
    }
    if cin.is_some() {
        commands.queue(|w: &mut World| stop(w));
    }
    q.1 = false;
    if let Some(name) = q.0.pop_front() {
        q.1 = true;
        commands.queue(Play { name, skippable: true });
    }
}

/// A skippable movie ends on a key, a click or a pad's A / Start.
fn skip(mut commands: Commands, cin: Option<Res<Cinematic>>, keys: Res<ButtonInput<KeyCode>>, mouse: Res<ButtonInput<MouseButton>>, pads: Query<&Gamepad>) {
    let Some(cin) = cin else { return };
    if !cin.skippable || cin.shown.is_none() {
        return;
    }
    let pressed = keys.any_just_pressed([KeyCode::Escape, KeyCode::Enter, KeyCode::Space])
        || mouse.just_pressed(MouseButton::Left)
        || pads.iter().any(|p| p.just_pressed(GamepadButton::South) || p.just_pressed(GamepadButton::Start));
    if pressed {
        commands.queue(|w: &mut World| stop(w));
    }
}

fn intro_flag() -> Option<std::path::PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| std::path::PathBuf::from(d).join("cod4rw").join("intro_played"))
}

/// Has the first launch's intro played (`com_introPlayed`)?
fn intro_played() -> bool {
    intro_flag().is_none_or(|p| p.exists())
}

fn mark_intro_played() {
    if let Some(p) = intro_flag() {
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).ok();
        }
        std::fs::write(p, "1").ok();
    }
}

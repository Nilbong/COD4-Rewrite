//! The shared time of day and weather (the Wet Work showcase's contract):
//! [`TimeOfDay`] and [`Weather`]. Every session reads them; only this
//! module writes them.
//!
//! Off by default: without the showcase both stay the map's own (its sun
//! where CoD4 put it, `night` 0, no rain) and nothing moves. Debug:
//! `COD4RW_SHOWCASE=1` turns the showcase on; `COD4RW_TOD=<hour>` starts
//! the clock there (0..24), `COD4RW_TOD_SPEED=<x>` runs it at x game
//! seconds per real second (360: a day in 4 minutes; 0 holds it);
//! `COD4RW_RAIN=<0..1>` sets the rain; `COD4RW_FLASH=<0..1>` holds a
//! lightning flash (screenshots).
//!
//! The sun's path ([`sun_dir_at`]): an equinox day at 35 degrees north,
//! turned so that at the map's own hour ([`TimeOfDay::map_hour`], the
//! morning hour with CoD4's sun elevation) the sun stands exactly where
//! CoD4 put it. Baked lighting keyframes must use the same function.

use bevy::prelude::*;

pub struct ClimatePlugin;

impl Plugin for ClimatePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TimeOfDay>()
            .init_resource::<Weather>()
            .add_systems(OnEnter(crate::state::GameState::InGame), reset)
            .add_systems(Update, (learn_map_sun, update_generic, tick_time, tick_weather, sun_knobs).chain().in_set(ClimateSet).run_if(crate::state::in_game));
    }
}

/// The clock and weather update here; readers that want this frame's
/// values run after it.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct ClimateSet;

/// The showcase's rain without `COD4RW_RAIN`: Wet Work's own storm.
const SHOWCASE_RAIN: f32 = 0.85;

/// How the showcase runs on a map it's tuned for.
#[derive(Clone, Copy, Debug)]
pub struct Profile {
    pub map: &'static str,
    /// The map's look in a normal launch: true, the showcase's always (the
    /// map moved over to it); false, only with `COD4RW_SHOWCASE=1`.
    pub on_by_default: bool,
    /// The clock runs (`COD4RW_TOD`, `COD4RW_TOD_SPEED`); otherwise it holds
    /// at the map's own hour, as CoD4 lit it.
    pub clock: bool,
    /// Where a running clock starts without `COD4RW_TOD` (hours; `None`:
    /// the map's own hour). Wet Work: its storm by night.
    pub start_hour: Option<f32>,
    /// The rain without `COD4RW_RAIN` (which only clocked maps take).
    pub rain: f32,
    /// The physical sky (Rayleigh/Mie, stars, moon) in place of the map's
    /// skybox colours (Wet Work's own skybox is night).
    pub physical_sky: bool,
    /// The showcase ocean replaces the map's sea.
    pub ocean: bool,
    /// How much storm cloud dims the light (0: the map's own lighting is
    /// already the storm's, Downpour's).
    pub storm_dims: f32,
    /// Snow instead of rain (Winter Crash): `Weather::snow` takes the
    /// precipitation, `rain` stays 0.
    pub snow: bool,
    /// The frame's exposure offset (stops): a downpour's light is dimmer.
    pub exposure_ev: f32,
    /// The map fog's halfway distance scaled (clearer air than CoD4's:
    /// Downpour's treeline keeps its dark green).
    pub fog_reach: f32,
    /// The map fog's colour scaled (auto-exposure lifts a dark rainy
    /// frame, and the fog with it, past CoD4's).
    pub fog_tone: f32,
    /// Standing water (the showcase's puddles) without rain: Bog's swamp.
    pub standing_water: bool,
    /// How severe the storm is (1: as before): denser, heavier, more
    /// slanted rain in gusting waves, louder, more frequent lightning,
    /// darker sky, rougher sea.
    pub severity: f32,
}

/// The maps the showcase is tuned for; elsewhere it stays off.
pub const PROFILES: &[Profile] = &[
    Profile { map: "mp_cargoship", on_by_default: true, clock: true, start_hour: Some(23.0), rain: SHOWCASE_RAIN, physical_sky: true, ocean: true, storm_dims: 1.0, snow: false, exposure_ev: 0.0, fog_reach: 1.0, fog_tone: 1.0, severity: 1.4, standing_water: false },
    // Downpour: CoD4's own rainy afternoon, its light and sky kept; our
    // rain, wet surfaces, ripples, reflections and now and then lightning.
    Profile { map: "mp_farm", on_by_default: true, clock: false, start_hour: None, rain: 0.7, physical_sky: false, ocean: false, storm_dims: 0.0, snow: false, exposure_ev: -0.7, fog_reach: 6.0, fog_tone: 0.6, severity: 1.35, standing_water: false },
    // Winter Crash: CoD4's snowy night, its light kept; our falling snow
    // (drifting with the wind, kept off under roofs), snow on upward
    // faces, frosty fog. No lightning.
    // Bog: its hot, still night, no rain (the user's call). CoD4's light
    // and hour kept, clearer air than its orange haze; its look
    // ([`MapLook`]: starry sky, warm city glow, the swamp's water dark and
    // muddy) with the showcase's shadows, grass and puddles.
    Profile { map: "mp_bog", on_by_default: true, clock: false, start_hour: None, rain: 0.0, physical_sky: false, ocean: false, storm_dims: 0.0, snow: false, exposure_ev: -1.0, fog_reach: 1.4, fog_tone: 0.55, severity: 1.4, standing_water: true },
    // Shipment: early afternoon (the user's call, after golden hour read
    // all wrong): a high sun, shorter shadows, the physical sky, only a
    // faint depth haze (its look: fog and shafts at 15%). Dry. Needs its
    // time-of-day lightmap keyframes (`bake/tod.rs`, baked 2026-10-14).
    Profile { map: "mp_shipment", on_by_default: true, clock: true, start_hour: Some(13.5), rain: 0.0, physical_sky: true, ocean: false, storm_dims: 1.0, snow: false, exposure_ev: 0.0, fog_reach: 10.0, fog_tone: 1.0, severity: 1.0, standing_water: false },
    // Crossfire after rain (fidelity #4, the MW4 wet-street look): CoD4's
    // own afternoon light, no rain falling, the streets wet and puddled.
    // Off unless `COD4RW_SHOWCASE=1` while it's judged.
    Profile { map: "mp_crossfire", on_by_default: false, clock: false, start_hour: None, rain: 0.0, physical_sky: false, ocean: false, storm_dims: 0.0, snow: false, exposure_ev: 0.0, fog_reach: 1.0, fog_tone: 1.0, severity: 1.0, standing_water: true },
    Profile { map: "mp_crash_snow", on_by_default: false, clock: false, start_hour: None, rain: 0.6, physical_sky: false, ocean: false, storm_dims: 0.0, snow: true, exposure_ev: 0.0, fog_reach: 1.0, fog_tone: 1.0, severity: 1.0, standing_water: false },
];

/// The showcase's profile for the map being played, if it runs here.
pub fn profile() -> Option<&'static Profile> {
    if forced_off() || original() {
        return None;
    }
    match SHOWCASE_MAP.load(std::sync::atomic::Ordering::Relaxed).checked_sub(1).and_then(|i| PROFILES.get(i)) {
        Some(p) if p.on_by_default || enabled() => Some(p),
        // A map without one (or with its showcase off, Crossfire): the
        // generic profile while its look (or the Map Look Editor) sets a
        // time, rain or snow ([`update_generic`]).
        _ => match GENERIC.load(std::sync::atomic::Ordering::Relaxed) {
            1 => Some(&GENERIC_HELD),
            2 => Some(&GENERIC_CLOCK),
            _ => None,
        },
    }
}

/// A map without a showcase profile, with weather from its look: CoD4's own
/// light and hour, nothing else of the showcase's.
const GENERIC_HELD: Profile = Profile {
    map: "",
    on_by_default: true,
    clock: false,
    start_hour: None,
    rain: 0.0,
    physical_sky: false,
    ocean: false,
    storm_dims: 0.0,
    snow: false,
    exposure_ev: 0.0,
    fog_reach: 1.0,
    fog_tone: 1.0,
    severity: 1.0,
    standing_water: false,
};

/// ... and with a time of day set: the clock's light (held there).
const GENERIC_CLOCK: Profile = Profile { clock: true, ..GENERIC_HELD };

/// 0 off, 1 [`GENERIC_HELD`], 2 [`GENERIC_CLOCK`].
static GENERIC: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// The generic profile is the one in use (a map without its own).
pub fn generic() -> bool {
    !showcase_profile_on() && GENERIC.load(std::sync::atomic::Ordering::Relaxed) != 0 && profile().is_some()
}

/// The map has a showcase profile and it's on.
fn showcase_profile_on() -> bool {
    SHOWCASE_MAP.load(std::sync::atomic::Ordering::Relaxed).checked_sub(1).and_then(|i| PROFILES.get(i)).is_some_and(|p| p.on_by_default || enabled())
}

/// Switches the generic profile with the map's look: a time of day set
/// clocks it, rain or snow set holds CoD4's hour; nothing set, off (the map
/// as CoD4 had it). And the clock and weather on or off with it, live.
fn update_generic(mut tod: ResMut<TimeOfDay>, mut weather: ResMut<Weather>) {
    let has = crate::tune::look_has;
    // (A showcase map whose showcase is off counts as any other map: its
    // look's time of day still clocks it, Crossfire with Live lighting.)
    let want = if showcase_profile_on() {
        0
    } else if has("tod.hour") {
        2
    } else if has("weather.rain") || has("weather.snow") {
        1
    } else {
        0
    };
    GENERIC.store(want, std::sync::atomic::Ordering::Relaxed);
    let on = showcase();
    if tod.enabled != on {
        tod.enabled = on;
        info!("climate: showcase {} (live)", if on { "on" } else { "off" });
        if on {
            // (From CoD4's own hour: `learn_map_sun` learnt it already.)
            if let Some(h) = tod.map_hour() {
                set_hour(&mut tod, h);
            }
        }
    }
    if weather.enabled != on {
        weather.enabled = on;
        if !on {
            *weather = Weather::default();
        }
    }
}

static SHOWCASE_MAP: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The showcase's exposure offset on this map (stops), 0 elsewhere.
pub fn exposure_offset() -> f32 {
    // (Live knob per map, `crate::tune`: `exposure.<map>`.)
    match profile() {
        Some(p) => crate::tune::get(&format!("exposure.{}", p.map), p.exposure_ev),
        None => look_knob("exposure", |l| l.exposure_ev, 0.0),
    }
}

/// Whether the showcase is forced on for every map that has a profile
/// (`COD4RW_SHOWCASE=1`; `0` forces it off, even where it's the default).
/// The showcase's plugins are always added and gate at run time on [`on`].
pub fn enabled() -> bool {
    std::env::var("COD4RW_SHOWCASE").is_ok_and(|v| v != "0")
}

/// The Map Look Editor's A/B (held): the map as CoD4 had it, no showcase
/// and no look, for a moment.
pub fn original() -> bool {
    ORIGINAL.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn set_original(on: bool) {
    ORIGINAL.store(on, std::sync::atomic::Ordering::Relaxed);
}

static ORIGINAL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `COD4RW_SHOWCASE=0`: every map as CoD4 had it (for A/B shots).
fn forced_off() -> bool {
    std::env::var("COD4RW_SHOWCASE").is_ok_and(|v| v == "0")
}

/// Bumped each time a map is chosen ([`set_map`]): for state kept across
/// frames (a system's `Local`) that belongs to one match.
pub fn match_id() -> u32 {
    MATCH_ID.load(std::sync::atomic::Ordering::Relaxed)
}

static MATCH_ID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// The map being loaded, for [`showcase`]. Called wherever `MapName` is set.
/// How a map looks in normal play (not the showcase), where CoD4's own needs
/// help in this renderer. Each value is also a live knob (`crate::tune`).
#[derive(Clone, Copy, Debug)]
pub struct MapLook {
    pub map: &'static str,
    /// Exposure offset (stops).
    pub exposure_ev: f32,
    /// Fog halfway distance scale, and fog colour scale.
    pub fog_reach: f32,
    pub fog_tone: f32,
    /// Cloud cover for the dynamic sky (and how dark their undersides).
    pub cloud_cover: f32,
    /// The night sky: x stars, y zenith darkening (0..1), z warm city glow
    /// at the horizon.
    pub night_sky: [f32; 3],
    /// The map's water as calm, dark, reflective mud (0 CoD4's water).
    pub muddy_water: f32,
    /// White balance warmer (+) or cooler (-): Bevy's grading temperature.
    pub warmth: f32,
    /// Green (-) or magenta (+): Bevy's grading tint (warmth alone reads
    /// pink; a little green with it reads amber).
    pub tint: f32,
    /// The sun shafts' air (volumetric fog density) scaled: 1 the map's.
    pub volumetric: f32,
    /// Only with the map's showcase on (its normal play stays CoD4's).
    pub showcase_only: bool,
}

pub const LOOKS: &[MapLook] = &[
    // Bog: a hot night over a swamp. Its orange fog, lifted by the
    // auto-exposure of a dark frame, washed buildings out; clearer air,
    // darker frame, a few clouds, a deep night sky over a warm city glow.
    MapLook { map: "mp_bog", exposure_ev: -1.0, fog_reach: 1.4, fog_tone: 0.55, cloud_cover: 0.05, night_sky: [2.2, 0.6, 0.6], muddy_water: 0.8, warmth: 0.0, tint: 0.0, volumetric: 1.0, showcase_only: false },
    // Shipment's golden hour (its dry showcase keeps this look): a mostly
    // clear sky for the low sun, clearer air for its close fights.
    MapLook { map: "mp_shipment", exposure_ev: 0.0, fog_reach: 10.0, fog_tone: 1.0, cloud_cover: 0.15, night_sky: [0.0, 0.0, 0.0], muddy_water: 0.0, warmth: 0.0, tint: 0.0, volumetric: 0.15, showcase_only: true },
];

static MAP: std::sync::RwLock<String> = std::sync::RwLock::new(String::new());

/// The map being played.
pub fn map_name() -> String {
    MAP.read().map(|m| m.clone()).unwrap_or_default()
}

/// The map's look in normal play, if it has one (not with a showcase that
/// brings weather, whose profile takes over; a dry one keeps it: Bog).
pub fn look() -> Option<&'static MapLook> {
    if profile().is_some_and(|p| p.rain > 0.0) {
        return None;
    }
    if original() {
        return None;
    }
    let map = map_name();
    LOOKS.iter().find(|l| l.map == map && (!l.showcase_only || showcase()))
}

/// A knob of the map's look: `<name>.<map>` (e.g. `exposure.mp_bog`), or the
/// look's default.
pub fn look_knob(name: &str, default: impl Fn(&MapLook) -> f32, otherwise: f32) -> f32 {
    let map = map_name();
    crate::tune::get(&format!("{name}.{map}"), look().map_or(otherwise, default))
}

/// Sets the map being loaded (returned, for `MapName`): its showcase, if
/// it has one on, and a fresh start for state kept per match.
pub fn set_map(map: &str) -> String {
    MATCH_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    TOD_KEYS.store(false, std::sync::atomic::Ordering::Relaxed);
    STORM_VEIL.store(false, std::sync::atomic::Ordering::Relaxed);
    // (The last map's generic look not carried into this one's load.)
    GENERIC.store(0, std::sync::atomic::Ordering::Relaxed);
    LIGHT_SHARE.store(1.0f32.to_bits(), std::sync::atomic::Ordering::Relaxed);
    if let Ok(mut m) = MAP.write() {
        *m = map.to_owned();
    }
    let index = PROFILES.iter().position(|p| p.map == map).map_or(0, |i| i + 1);
    SHOWCASE_MAP.store(index, std::sync::atomic::Ordering::Relaxed);
    map.to_owned()
}

/// Whether this map's showcase brings rain or snow (what wets surfaces):
/// a dry one (Bog, Shipment) needs none of the wet paths.
pub fn wet_showcase() -> bool {
    profile().is_some_and(|p| p.rain > 0.0 || p.snow)
}

/// Whether the showcase (time of day, weather) runs on this map: switched
/// on and one of [`PROFILES`].
pub fn showcase() -> bool {
    profile().is_some()
}

/// Whether storm cloud hides the sun and moon now (the showcase): read
/// where `Res<Weather>` isn't to hand.
pub fn storm_veil() -> bool {
    STORM_VEIL.load(std::sync::atomic::Ordering::Relaxed)
}

/// The showcase's sky light against the map's own (CoD4's), for light
/// baked at CoD4's hour (reflection probes): 1 without the showcase.
pub fn light_share() -> f32 {
    // (Times the night grade's dim fill, `crate::fire`: models and props
    // sink with the baked light.)
    let base = if showcase() { f32::from_bits(LIGHT_SHARE.load(std::sync::atomic::Ordering::Relaxed)) } else { 1.0 };
    base * crate::fire::night_dim()
}

pub(super) static LIGHT_SHARE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x3f80_0000);

static STORM_VEIL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Run condition: the showcase runs on this map (see [`showcase`]).
pub fn on() -> bool {
    showcase()
}

fn env_f32(name: &str) -> Option<f32> {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok())
}

/// The time of day, and the sun, moon and sky it gives.
#[derive(Resource, Clone, Debug)]
pub struct TimeOfDay {
    /// The clock, hours 0..24.
    pub hours: f32,
    /// Game seconds per real second (360: a day in 4 minutes).
    pub speed: f32,
    /// Toward the sun, Bevy space (below the horizon at night).
    pub sun_dir: Vec3,
    /// Toward the moon, Bevy space.
    pub moon_dir: Vec3,
    /// The sun's colour (linear, brightest channel 1), reddened low down.
    pub sun_color: Vec3,
    /// The sun's illuminance in the units of the map's sun light (lux as
    /// `crate::world` sets it): the map's own at its hour, 0 below the
    /// horizon.
    pub sun_illuminance: f32,
    pub moon_color: Vec3,
    pub moon_illuminance: f32,
    /// The sky's colour as ambient light (linear, brightest channel 1).
    pub sky_color: Vec3,
    /// The sky's ambient illuminance, the same units (scaled from the
    /// map's ambient).
    pub sky_illuminance: f32,
    /// 0 full day .. 1 full night (civil twilight in between).
    pub night: f32,
    /// The colour at the horizon line (linear, the frame's units: what the
    /// sky draws there and the distance fog takes), for anything that must
    /// fade into it (the ocean). Without the showcase: black, unused.
    pub horizon: Vec3,
    /// The showcase drives the world from this (false: the map's own
    /// lighting stands and nothing here changes).
    pub enabled: bool,
    /// The map's own sun, learnt as the match starts.
    pub map: Option<MapSun>,
}

/// CoD4's sun for the map: where, what colour, how strong, and the
/// hour it stands for on [`sun_dir_at`]'s path.
#[derive(Clone, Copy, Debug)]
pub struct MapSun {
    pub dir: Vec3,
    pub color: Vec3,
    pub illuminance: f32,
    pub ambient: f32,
    /// The hour at which the path puts the sun at `dir`.
    pub hour: f32,
    /// The path's turn about the vertical so it does (radians).
    pub azimuth_offset: f32,
}

impl Default for TimeOfDay {
    fn default() -> Self {
        TimeOfDay {
            hours: 10.0,
            speed: 0.0,
            sun_dir: Vec3::new(0.3, 0.7, 0.2).normalize(),
            moon_dir: Vec3::new(-0.3, -0.7, -0.2).normalize(),
            sun_color: Vec3::ONE,
            sun_illuminance: 0.0,
            moon_color: Vec3::new(0.6, 0.7, 1.0),
            moon_illuminance: 0.0,
            sky_color: Vec3::new(0.6, 0.7, 1.0),
            sky_illuminance: 0.0,
            night: 0.0,
            horizon: Vec3::ZERO,
            enabled: false,
            map: None,
        }
    }
}

impl TimeOfDay {
    /// The map's own hour, when the sun stands where CoD4 put it.
    pub fn map_hour(&self) -> Option<f32> {
        self.map.map(|m| m.hour)
    }
}

/// Toward the sun at `hours`, Bevy space, on the map's turned path
/// (`azimuth_offset` from [`MapSun`]). The path itself is `super::sun_path`
/// (shared with the lighting bake).
pub fn sun_dir_at(hours: f32, azimuth_offset: f32) -> Vec3 {
    Vec3::from(super::sun_path::sun_dir_at(hours, azimuth_offset))
}

/// The moon: opposite the sun (a full moon), a little off its path.
pub fn moon_dir_at(hours: f32, azimuth_offset: f32) -> Vec3 {
    Vec3::from(super::sun_path::moon_dir_at(hours, azimuth_offset))
}

/// The map's hour and path turn: the morning hour with CoD4's sun
/// elevation (noon if it's higher than the path goes), turned to CoD4's
/// azimuth.
pub fn fit_path(dir: Vec3) -> (f32, f32) {
    super::sun_path::fit_path(dir.to_array())
}

/// The profile's rain and snow (and the debug overrides).
fn profile_precipitation() -> (f32, f32) {
    let rain = profile().map_or(0.0, |p| if p.clock { env_f32("COD4RW_RAIN").unwrap_or(p.rain) } else { env_f32("COD4RW_RAIN_FIXED").unwrap_or(p.rain) });
    if profile().is_some_and(|p| p.snow) { (0.0, rain) } else { (rain, 0.0) }
}

/// The hours the Map Look Editor's clock may be set to: wide where the map
/// has baked time-of-day keyframes (its lightmaps follow), else close to
/// CoD4's own hour (its lightmaps are baked at that hour).
pub fn hour_window(tod: &TimeOfDay) -> (f32, f32) {
    match tod.map_hour() {
        _ if TOD_KEYS.load(std::sync::atomic::Ordering::Relaxed) => (6.0, 21.0),
        Some(h) => ((h - 1.5).max(0.0), (h + 1.5).min(24.0)),
        None => (6.0, 21.0),
    }
}

/// The map has time-of-day keyframes (`crate::tod_light`).
pub fn set_tod_keys(on: bool) {
    TOD_KEYS.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn tod_keys() -> bool {
    TOD_KEYS.load(std::sync::atomic::Ordering::Relaxed)
}

static TOD_KEYS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn reset(mut tod: ResMut<TimeOfDay>, mut weather: ResMut<Weather>) {
    let on = showcase();
    let clock = profile().is_some_and(|p| p.clock);
    *tod = TimeOfDay { enabled: on, speed: if clock { env_f32("COD4RW_TOD_SPEED").unwrap_or(0.0) } else { 0.0 }, ..default() };
    let (rain, snow) = profile_precipitation();
    // (Snow lies from the start: it's been snowing.)
    *weather = Weather { enabled: on, rain: rain.clamp(0.0, 1.0), snow: snow.clamp(0.0, 1.0), snow_cover: snow.clamp(0.0, 1.0), ..default() };
    weather.wetness = weather.rain;
    info!("climate: showcase {}", if on { "on" } else { "off" });
}

/// The Map Look Editor's sun position (`sun.azimuth.<map>`, a compass
/// bearing; `sun.elevation.<map>`, degrees up), if set: Bevy's direction
/// toward the sun.
pub fn sun_override() -> Option<Vec3> {
    let map = map_name();
    let az = crate::tune::get(&format!("sun.azimuth.{map}"), -1.0);
    let el = crate::tune::get(&format!("sun.elevation.{map}"), -1.0);
    if !(az >= 0.0 && el >= 0.0) {
        return None;
    }
    let (b, e) = (az.clamp(0.0, 360.0).to_radians(), el.clamp(0.5, 90.0).to_radians());
    // (CoD: x east, y north, z up; Bevy (x, y, z) is CoD (x, z, -y).)
    let cod = [b.sin() * e.cos(), b.cos() * e.cos(), e.sin()];
    Some(Vec3::new(cod[0], cod[2], -cod[1]).normalize())
}

/// A direction toward the sun as the editor shows it: (bearing, elevation)
/// in degrees.
pub fn sun_angles(dir: Vec3) -> (f32, f32) {
    let cod = Vec3::new(dir.x, -dir.z, dir.y);
    let bearing = cod.x.atan2(cod.y).to_degrees().rem_euclid(360.0);
    let elevation = cod.z.clamp(-1.0, 1.0).asin().to_degrees();
    (bearing, elevation)
}

/// The showcase's clock sun moved where the editor puts it.
fn sun_knobs(mut tod: ResMut<TimeOfDay>) {
    if let (true, Some(dir)) = (tod.enabled, sun_override()) {
        tod.sun_dir = dir;
    }
}

/// The map's own sun, from the sun light `crate::world` spawned.
fn learn_map_sun(
    mut tod: ResMut<TimeOfDay>,
    suns: Query<(&Name, &DirectionalLight, &GlobalTransform)>,
    ambient: Option<Res<GlobalAmbientLight>>,
) {
    if tod.map.is_some() {
        return;
    }
    let Some((_, light, tf)) = suns.iter().find(|(n, ..)| n.as_str() == "sun") else { return };
    let dir = -tf.forward().as_vec3();
    if dir == Vec3::NEG_Z {
        return; // (Not placed yet.)
    }
    let c = light.color.to_linear();
    let color = Vec3::new(c.red, c.green, c.blue);
    let (hour, azimuth_offset) = fit_path(dir);
    let map = MapSun {
        dir,
        color: color / color.max_element().max(1e-3),
        illuminance: light.illuminance,
        ambient: ambient.map_or(0.0, |a| a.brightness),
        hour,
        azimuth_offset,
    };
    info!("climate: map sun at {:.1} h on the path (turned {:.0} degrees), {:.0} lux", hour, azimuth_offset.to_degrees(), light.illuminance);
    let clock = profile().is_some_and(|p| p.clock);
    // (A profile's own hour only with its time-of-day lightmaps: without
    // them the live sun would leave the baked light, Shipment for anyone
    // without its local bake.)
    let profile_hour = profile().and_then(|p| p.start_hour).filter(|_| tod_keys());
    let start = if tod.enabled && clock { env_f32("COD4RW_TOD").or(profile_hour).map(|h| h.rem_euclid(24.0)).unwrap_or(hour) } else { hour };
    tod.hours = start;
    tod.map = Some(map);
    apply_clock(&mut tod);
}

fn tick_time(time: Res<Time>, mut tod: ResMut<TimeOfDay>) {
    if !tod.enabled || tod.map.is_none() {
        return;
    }
    // The Map Look Editor's clock (`tod.hour`, within `hour_window`), else
    // its speed (`tod.speed`, game seconds a second).
    let hour = crate::tune::get("tod.hour", -1.0);
    if hour >= 0.0 {
        let (lo, hi) = hour_window(&tod);
        let h = hour.clamp(lo, hi);
        if (tod.hours - h).abs() > 1e-3 {
            tod.hours = h;
            apply_clock(&mut tod);
        }
        return;
    }
    let speed = crate::tune::get("tod.speed", tod.speed).clamp(-3600.0, 3600.0);
    if speed == 0.0 {
        return;
    }
    tod.hours = (tod.hours + time.delta_secs() * speed / 3600.0).rem_euclid(24.0);
    apply_clock(&mut tod);
}

/// Set the clock (tests), and the sun, moon and sky with it.
pub(super) fn set_hour(tod: &mut TimeOfDay, hours: f32) {
    tod.hours = hours.rem_euclid(24.0);
    apply_clock(tod);
}

/// Sun, moon, sky and night from the clock (`super::sun_path`'s curves).
fn apply_clock(tod: &mut TimeOfDay) {
    let Some(map) = tod.map else { return };
    let path = super::sun_path::MapSun {
        dir: map.dir.to_array(),
        colour: map.color.to_array(),
        illuminance: map.illuminance,
        hour: map.hour,
        azimuth_offset: map.azimuth_offset,
    };
    let (sun, moon) = (path.sun(tod.hours), path.moon(tod.hours));
    let from_cod = |v: [f32; 3]| Vec3::new(v[0], v[2], -v[1]);
    tod.sun_dir = from_cod(sun.to_light);
    tod.moon_dir = from_cod(moon.to_light);
    tod.sun_color = Vec3::from(sun.colour);
    tod.sun_illuminance = sun.lux;
    tod.moon_color = Vec3::from(moon.colour);
    tod.moon_illuminance = moon.lux;
    tod.night = path.night(tod.hours);
    let (sky, brightness) = path.sky(tod.hours);
    tod.sky_color = Vec3::from(sky);
    tod.sky_illuminance = map.ambient * brightness;
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The weather.
#[derive(Resource, Clone, Debug)]
pub struct Weather {
    /// How hard it rains, 0..1.
    pub rain: f32,
    /// Wind (m/s, Bevy space, horizontal).
    pub wind: Vec3,
    /// How wet surfaces are, 0..1: follows the rain, slowly (drying
    /// slower than wetting).
    pub wetness: f32,
    /// A lightning flash in progress.
    pub lightning: Option<Lightning>,
    /// How much of the sky clouds cover, 0..1.
    pub cloud_cover: f32,
    /// How hard the rain comes down now (0..~1.6): `rain` × the profile's
    /// severity, swelling and easing with the gusts. Renderers of falling
    /// rain take this; wetness follows `rain`.
    pub downpour: f32,
    /// The gust now, 0..1 (waves of wind and heavier rain sweeping past).
    pub gust: f32,
    /// How hard it snows, 0..1 (snowy profiles; `rain` is 0 then).
    pub snow: f32,
    /// How much snow lies on upward faces, 0..1 (follows `snow`, slowly).
    pub snow_cover: f32,
    /// The showcase drives this (false: dry and still, as CoD4).
    pub enabled: bool,
}

/// A lightning flash: how bright now (0..1, it flickers), toward the bolt
/// (Bevy space), where it strikes and how far that is from the map's
/// centre, which strike (counts up, for one thunderclap each) and seconds
/// since it struck. Thunder reaches a listener `distance / 343` s after
/// `age` 0.
#[derive(Clone, Copy, Debug)]
pub struct Lightning {
    pub flash: f32,
    pub dir: Vec3,
    /// Where the bolt meets the ground (Bevy space, metres).
    pub strike: Vec3,
    pub distance: f32,
    pub id: u32,
    pub age: f32,
}

impl Default for Weather {
    fn default() -> Self {
        Weather { rain: 0.0, wind: Vec3::new(3.0, 0.0, 1.0), wetness: 0.0, lightning: None, cloud_cover: 0.4, downpour: 0.0, gust: 0.0, snow: 0.0, snow_cover: 0.0, enabled: false }
    }
}

/// The current strike: when it started, how long, where, and its
/// flicker's seed.
#[derive(Default)]
struct Strike {
    next: f32,
    start: f32,
    dir: Vec3,
    seed: u32,
    strike: Vec3,
    distance: f32,
}

fn tick_weather(time: Res<Time>, mut weather: ResMut<Weather>, mut strike: Local<Strike>) {
    if !weather.enabled {
        return;
    }
    let dt = time.delta_secs();
    let now = time.elapsed_secs();
    // The Map Look Editor's weather (`weather.rain`, `weather.snow`), else
    // the profile's.
    let (rain, snow) = profile_precipitation();
    weather.rain = crate::tune::get("weather.rain", rain).clamp(0.0, 1.0);
    weather.snow = crate::tune::get("weather.snow", snow).clamp(0.0, 1.0);
    STORM_VEIL.store(weather.rain >= 0.5, std::sync::atomic::Ordering::Relaxed);
    // Cloud cover and wind with the rain.
    // (Never quite full: a storm deck keeps its lumps and breaks.)
    // (A severe storm closes the sky further: a heavy deck, few breaks.)
    let severity_now = crate::tune::get("storm.severity", profile().map_or(1.0, |p| p.severity));
    weather.cloud_cover = (0.4 + (0.45 + 0.08 * (severity_now - 1.0)) * smooth(weather.rain.max(weather.snow) / 0.5)).min(0.97);
    // Snow settles in a few minutes and stays.
    if weather.snow > weather.snow_cover {
        weather.snow_cover = (weather.snow_cover + dt / 240.0).min(weather.snow);
    }
    // Gusts: two slow waves beating, so the rain sweeps in heavier bursts.
    let severity = crate::tune::get("storm.severity", profile().map_or(1.0, |p| p.severity));
    let g = 0.5 + 0.3 * (now * 0.21).sin() + 0.2 * (now * 0.57 + 1.3).sin();
    weather.gust = smooth((g - 0.35) / 0.6) * smooth((severity - 1.0) / 0.3).max(0.3);
    weather.downpour = weather.rain * severity * (0.8 + 0.4 * weather.gust);
    weather.wind = Vec3::new(3.0, 0.0, 1.0) * (1.0 + 3.0 * weather.rain) * severity.sqrt() * (0.8 + 0.6 * weather.gust);
    // Wet in about a minute of hard rain, dry in about three.
    let target = weather.rain;
    let rate = if target > weather.wetness { 1.0 / 60.0 } else { 1.0 / 180.0 };
    weather.wetness += (target - weather.wetness).clamp(-rate * dt, rate * dt);
    // (The editor's `weather.wetness`: held there.)
    let wet = crate::tune::get("weather.wetness", -1.0);
    if wet >= 0.0 {
        weather.wetness = wet.min(1.0);
    }
    // Debug: `COD4RW_FLASH=<0..1>` holds a flash (for screenshots).
    if let Some(f) = env_f32("COD4RW_FLASH") {
        let dir = Vec3::new(0.6, 0.35, 0.72).normalize();
        weather.lightning = Some(Lightning { flash: f, dir, strike: Vec3::new(dir.x, 0.0, dir.z) * 2000.0, distance: 2000.0, id: 1, age: 0.1 });
        return;
    }
    // Lightning in a storm: every 6-20 s, hard rain only (`weather.lightning`
    // 0: none).
    if weather.rain < 0.6 || crate::tune::get("weather.lightning", 1.0) < 0.5 {
        weather.lightning = None;
        return;
    }
    let rand = |s: u32| {
        let mut h = s.wrapping_mul(0x9e37_79b9) ^ 0x85eb_ca6b;
        h ^= h >> 15;
        h = h.wrapping_mul(0x2c1b_3c6d);
        h ^= h >> 12;
        (h & 0xffff) as f32 / 65535.0
    };
    if strike.next == 0.0 {
        strike.next = now + 4.0;
    }
    if now >= strike.next {
        strike.seed = strike.seed.wrapping_add(1);
        strike.start = now;
        let a = rand(strike.seed * 3) * std::f32::consts::TAU;
        // 0.8-4 km away: the bolt's foot, and toward its top in the cloud.
        strike.distance = 800.0 + 3200.0 * rand(strike.seed * 17);
        strike.strike = Vec3::new(a.cos(), 0.0, a.sin()) * strike.distance;
        strike.dir = (strike.strike + Vec3::Y * 1500.0).normalize();
        strike.next = now + (4.0 + 12.0 * rand(strike.seed * 7)) / (weather.rain * severity * severity * crate::tune::get("storm.lightning_rate", 1.0)).max(0.05);
    }
    // A flash: a bright first stroke, a dimmer return or two, ~0.6 s.
    let t = now - strike.start;
    weather.lightning = (strike.start > 0.0 && t < 0.6).then(|| {
        let pulse = |at: f32, width: f32, height: f32| height * (-((t - at) / width).powi(2)).exp();
        let flash = (pulse(0.03, 0.03, 1.0) + pulse(0.18, 0.04, 0.6 * rand(strike.seed * 11)) + pulse(0.35, 0.05, 0.4 * rand(strike.seed * 13))).min(1.0);
        Lightning { flash, dir: strike.dir, strike: strike.strike, distance: strike.distance, id: strike.seed, age: t }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_path_passes_through_the_maps_sun() {
        for dir in [Vec3::new(0.3, 0.6, -0.5), Vec3::new(-0.7, 0.3, 0.2), Vec3::new(0.1, 0.9, 0.1)] {
            let dir = dir.normalize();
            let (hour, a) = fit_path(dir);
            let at = sun_dir_at(hour, a);
            // (Below the noon sun, 55 degrees up, the path reaches it.)
            if dir.y < 0.81 {
                assert!((at - dir).length() < 0.02, "{dir} -> {at} at {hour}");
            }
        }
    }

    #[test]
    fn night_at_midnight_day_at_noon() {
        let mut tod = TimeOfDay { map: Some(MapSun { dir: Vec3::Y, color: Vec3::ONE, illuminance: 10000.0, ambient: 1000.0, hour: 10.0, azimuth_offset: 0.0 }), ..default() };
        tod.hours = 0.0;
        apply_clock(&mut tod);
        assert!(tod.night > 0.99 && tod.sun_illuminance == 0.0);
        tod.hours = 12.0;
        apply_clock(&mut tod);
        assert!(tod.night < 0.01 && tod.sun_illuminance > 5000.0);
    }
}

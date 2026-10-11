//! Modern Warfare 2's maps, played as CoD4 maps: `crates/iw4` reads a map
//! zone from a Modern Warfare 2 installation (`MW2_PATH`, or found in the
//! usual places) and converts it to CoD4's asset types, so the world,
//! collision, lights and entities load like a CoD4 map's. Its textures are
//! streamed from MW2's archives, mounted under CoD4's (CoD4's own weapons
//! and characters keep their textures).
//!
//! A map is MW2's when it's named `mw2_<zone>` (`mw2_mp_terminal`; the only
//! way to reach MW2's remakes of CoD4 maps, `mw2_mp_crash`), or when CoD4
//! has no map of its name and MW2 does (`--map mp_terminal`).

use std::path::PathBuf;
use std::sync::OnceLock;

/// The Modern Warfare 2 installation, if there is one.
pub fn install() -> Option<&'static iw4::Install> {
    static I: OnceLock<Option<iw4::Install>> = OnceLock::new();
    I.get_or_init(|| iw4::Install::locate().ok()).as_ref()
}

/// The MW2 zone a map is, if it's one of MW2's.
pub fn zone(map: &str) -> Option<String> {
    let mw2 = install()?;
    if let Some(zone) = map.strip_prefix("mw2_") {
        return mw2.zone_path(zone).is_file().then(|| zone.to_owned());
    }
    let cod4 = iw3::Install::locate().ok()?;
    (!cod4.zone_path(map).is_file() && mw2.zone_path(map).is_file()).then(|| map.to_owned())
}

/// MW2's archives (mounted before CoD4's, which override them).
pub fn iwd_paths() -> Vec<PathBuf> {
    install().and_then(|i| i.iwd_paths().ok()).unwrap_or_default()
}

/// Load and convert an MW2 map zone. Its breakable glass waits in
/// [`take_glass`] for [`crate::glass`].
pub fn load(zone: &str) -> anyhow::Result<iw3::zone::Zone> {
    let install = install().ok_or_else(|| anyhow::anyhow!("no Modern Warfare 2 install (set MW2_PATH)"))?;
    let (mut map, glass) = iw4::load_map_with_glass(install, zone)?;
    // The map's scripts and vision files under the game's name for the map
    // too (`mw2_<zone>`): the HUD finds the minimap's material in
    // `maps/mp/<map>.gsc`, the film in `maps/createart/<map>_art.gsc` and
    // `vision/<map>.vision`.
    let aliases: Vec<iw3::zone::Asset> = map
        .assets
        .iter()
        .filter_map(|a| match a {
            iw3::zone::Asset::RawFile(r) if r.name.contains(&format!("/{zone}")) => {
                Some(iw3::zone::Asset::RawFile(iw3::zone::RawFile { name: r.name.replacen(&format!("/{zone}"), &format!("/mw2_{zone}"), 1), data: r.data.clone() }))
            }
            _ => None,
        })
        .collect();
    map.assets.extend(aliases);
    // The light grid's tweaks its script sets (`r_lightGridIntensity`,
    // `r_lightGridContrast`), for `crate::model_lighting`.
    let script = map.assets.iter().find_map(|a| match a {
        iw3::zone::Asset::RawFile(r) if r.name.eq_ignore_ascii_case(&format!("maps/mp/{zone}.gsc")) => Some(String::from_utf8_lossy(&r.data).into_owned()),
        _ => None,
    });
    let dvar = |name: &str| -> Option<f32> {
        let text = script.as_deref()?;
        let at = text.find(&format!("\"{name}\""))? + name.len() + 2;
        text[at..].split(')').next()?.trim().trim_start_matches(',').trim().trim_matches('"').parse().ok()
    };
    let enabled = dvar("r_lightGridEnableTweaks").is_some_and(|v| v != 0.0);
    *LIGHT_GRID.lock().unwrap_or_else(|e| e.into_inner()) =
        enabled.then(|| (dvar("r_lightGridIntensity").unwrap_or(1.0), dvar("r_lightGridContrast").unwrap_or(0.0)));
    *PANE_CENTRES.lock().unwrap_or_else(|e| e.into_inner()) =
        glass.iter().map(|g| std::array::from_fn(|k| g.corners.iter().map(|c| c[k]).sum::<f32>() / g.corners.len().max(1) as f32)).collect();
    *GLASS.lock().unwrap_or_else(|e| e.into_inner()) = glass;
    Ok(map)
}

static LIGHT_GRID: std::sync::Mutex<Option<(f32, f32)>> = std::sync::Mutex::new(None);

/// The MW2 map's light grid intensity and contrast, when its script tweaks
/// them.
pub fn light_grid_tweaks() -> Option<(f32, f32)> {
    if !active() {
        return None;
    }
    *LIGHT_GRID.lock().unwrap_or_else(|e| e.into_inner())
}

static ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether the map loaded is MW2's (set as each map loads).
pub fn active() -> bool {
    ACTIVE.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn set_active(on: bool) {
    ACTIVE.store(on, std::sync::atomic::Ordering::Relaxed);
}

static PANE_CENTRES: std::sync::Mutex<Vec<[f32; 3]>> = std::sync::Mutex::new(Vec::new());

/// Whether a breakable pane of the map's glass stands inside this box (CoD
/// units): its collision brush is the pane's to stand in for.
pub fn pane_in(mins: [f32; 3], maxs: [f32; 3]) -> bool {
    active() && PANE_CENTRES.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|c| (0..3).all(|k| c[k] >= mins[k] - 2.0 && c[k] <= maxs[k] + 2.0))
}

static GLASS: std::sync::Mutex<Vec<iw4::convert::GlassPane>> = std::sync::Mutex::new(Vec::new());

/// The breakable glass of the MW2 map last loaded (once: it's taken).
pub fn take_glass() -> Vec<iw4::convert::GlassPane> {
    std::mem::take(&mut *GLASS.lock().unwrap_or_else(|e| e.into_inner()))
}

/// A map zone's file (for caches keyed on it).
pub fn zone_path(zone: &str) -> Option<PathBuf> {
    install().map(|i| i.zone_path(zone))
}

/// MW2's names for its maps (the zones' names are mostly code names).
const NAMES: &[(&str, &str)] = &[
    ("mp_afghan", "Afghan"),
    ("mp_derail", "Derail"),
    ("mp_estate", "Estate"),
    ("mp_favela", "Favela"),
    ("mp_highrise", "Highrise"),
    ("mp_invasion", "Invasion"),
    ("mp_checkpoint", "Karachi"),
    ("mp_quarry", "Quarry"),
    ("mp_rundown", "Rundown"),
    ("mp_rust", "Rust"),
    ("mp_boneyard", "Scrapyard"),
    ("mp_nightshift", "Skidrow"),
    ("mp_subbase", "Sub Base"),
    ("mp_terminal", "Terminal"),
    ("mp_underpass", "Underpass"),
    ("mp_brecourt", "Wasteland"),
    ("mp_complex", "Bailout"),
    ("mp_compact", "Salvage"),
    ("mp_storm", "Storm"),
    ("mp_abandon", "Carnival"),
    ("mp_fuel2", "Fuel"),
    ("mp_trailerpark", "Trailer Park"),
    ("mp_crash", "Crash"),
    ("mp_overgrown", "Overgrown"),
    ("mp_strike", "Strike"),
    ("mp_vacant", "Vacant"),
];

/// The installed MW2 maps, as map names for the lobby (`mw2_mp_terminal`)
/// and what to call them ("MW2 Terminal"), in MW2's order.
pub fn maps() -> Vec<(String, String)> {
    let Some(mw2) = install() else { return Vec::new() };
    NAMES
        .iter()
        .filter(|(zone, _)| mw2.zone_path(zone).is_file())
        .map(|(zone, name)| (format!("mw2_{zone}"), format!("MW2 {name}")))
        .collect()
}

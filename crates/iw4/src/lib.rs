//! Readers for Call of Duty: Modern Warfare 2 (IW4) data, so its maps can be
//! played in the CoD4 rewrite.
//!
//! Like `iw3`, nothing here ships game data: everything is read at runtime
//! from a user-supplied Modern Warfare 2 installation.
//!
//! * [`fastfile`] – `.ff` zone containers (signed `IWffs100`, version 276)
//! * [`zone`] – the asset stream parser (schema-driven, from OpenAssetTools'
//!   IW4 definitions)
//! * [`install`] – finding the installation and its zones
//! * [`convert`] – a map zone as `iw3` assets (world, collision, lights,
//!   entities, materials, images, models), for the game's CoD4 map pipeline
//! * [`xanim`] – animations (`XAnimParts`) as `iw3` ones

pub mod convert;
pub mod fastfile;
pub mod install;
pub mod sound;
pub mod weapons;
pub mod xanim;
pub mod zone;

pub use install::Install;

/// Parse a Modern Warfare 2 map zone (`mp_terminal`, ...) and convert it to
/// `iw3` assets. Its streamed textures are in [`Install::vfs`].
pub fn load_map(install: &Install, zone: &str) -> anyhow::Result<iw3::zone::Zone> {
    load_map_with_glass(install, zone).map(|(z, _)| z)
}

/// [`load_map`], and the map's breakable glass.
pub fn load_map_with_glass(install: &Install, zone: &str) -> anyhow::Result<(iw3::zone::Zone, Vec<convert::GlassPane>)> {
    let parse = |name: &str| -> anyhow::Result<zone::Zone> {
        let parsed = zone::Zone::parse(&fastfile::load(&install.zone_path(name))?, zone::ParseOptions::default())?;
        if let Some(stop) = &parsed.stats.stopped_at {
            anyhow::bail!("{name}: parsing stopped at {stop}");
        }
        Ok(parsed)
    };
    // (The models, materials and images a map only names are in MW2's
    // `common_mp`; without it they stay empty.)
    let (map, common) = std::thread::scope(|s| {
        let common = s.spawn(|| parse("common_mp").ok());
        let map = parse(zone);
        (map, common.join().ok().flatten())
    });
    Ok(convert::to_iw3_with_glass(&map?, common.as_ref()))
}

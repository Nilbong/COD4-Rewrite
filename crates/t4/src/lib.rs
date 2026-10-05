//! Readers for Call of Duty: World at War (T4) data, so its weapons,
//! attachments and characters can be used alongside CoD4's.
//!
//! Like `iw3`, nothing here ships game data: everything is read at runtime
//! from a user-supplied World at War installation.
//!
//! * [`fastfile`] – `.ff` zone containers (CoD4's container, version 387)
//! * [`zone`] – the asset stream parser (schema-driven, from OpenAssetTools'
//!   T4 definitions)
//! * [`convert`] – models, animations, materials and images as `iw3` assets,
//!   ready for the game's model pipeline
//! * [`weapons`] – the multiplayer weapon files and their attachments
//! * [`catalog`] – what is ready: weapons, attachments, characters
//! * [`campaign`] – the campaign's characters (named cast and soldiers)
//! * [`ui`] – menus, fonts, localized strings and string tables as the game's
//!   `iw3::menu` types
//! * [`sound`] – sound aliases, mixer buses, music, and MS-ADPCM (and,
//!   through `t5`, xWMA) decoding
//! * [`hud`] – HUD art by role (crosshairs, hit marker, minimap, icons, ...)
//!
//! Images are CoD4's `.iwi` (version 6) files in the `.iwd` archives.

pub mod campaign;
pub mod catalog;
pub mod convert;
pub mod fastfile;
pub mod hud;
pub mod install;
pub mod sound;
pub mod ui;
pub mod weapons;
pub mod zone;

pub use install::Install;

/// Parse a World at War zone (`common_mp`, `mp_airfield`, ...) and convert
/// its models, animations, materials and images to `iw3` assets. Their
/// textures are in [`Install::vfs`].
pub fn load_iw3(install: &Install, zone: &str) -> anyhow::Result<iw3::zone::Zone> {
    let parsed = zone::Zone::parse(&fastfile::load(&install.zone_path(zone))?, zone::ParseOptions::default())?;
    if let Some(stop) = &parsed.stats.stopped_at {
        anyhow::bail!("{zone}: parsing stopped at {stop}");
    }
    Ok(convert::to_iw3(&parsed))
}

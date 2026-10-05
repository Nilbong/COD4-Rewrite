//! Readers for Call of Duty: Black Ops (T5) data, so its weapons,
//! attachments and characters can be used alongside CoD4's.
//!
//! Like `iw3`, nothing here ships game data: everything is read at runtime
//! from a user-supplied Black Ops installation.
//!
//! * [`fastfile`] – `.ff` zone containers (CoD4's container, version 473)
//! * [`zone`] – the asset stream parser (schema-driven, from OpenAssetTools'
//!   T5 definitions)
//! * [`convert`] – models, materials and images as `iw3` assets, ready for
//!   the game's model pipeline
//! * [`weapons`] – the multiplayer weapon files and their attachments
//! * [`catalog`] – what is ready: weapons, attachments, characters
//! * [`sound`] – the sound banks: aliases and their audio, decoded to PCM
//!
//! Images are `.iwi` version 13 files in the `.iwd` archives, which
//! `iw3::iwi` reads.

pub mod catalog;
pub mod convert;
pub mod fastfile;
pub mod install;
pub mod sound;
pub mod weapons;
pub mod zone;

pub use install::Install;

/// Parse a Black Ops zone (`common_mp`, `mp_nuked`, ...) and convert its
/// models, materials and images to `iw3` assets. Their textures are in
/// [`Install::vfs`].
pub fn load_iw3(install: &Install, zone: &str) -> anyhow::Result<iw3::zone::Zone> {
    let parsed = zone::Zone::parse(&fastfile::load(&install.zone_path(zone))?, zone::ParseOptions::default())?;
    if let Some(stop) = &parsed.stats.stopped_at {
        anyhow::bail!("{zone}: parsing stopped at {stop}");
    }
    Ok(convert::to_iw3(&parsed))
}

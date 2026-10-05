//! Readers for Call of Duty 4: Modern Warfare (IW3) data formats.
//!
//! Nothing in this crate ships game data: everything is read at runtime from a
//! user-supplied CoD4 installation.
//!
//! * [`fastfile`] – `.ff` zone containers (zlib-compressed asset streams)
//! * [`zone`] – the asset stream parser and the typed assets it produces
//! * [`iwd`] – `.iwd` archives (zip) as a layered virtual filesystem
//! * [`iwi`] – `.iwi` textures
//! * [`ents`] – the Radiant entity string embedded in maps
//! * [`menu`] – menus, fonts, localized strings and string tables
//! * [`demo`] – recorded matches (`.dm_1`)
//!
//! Struct layouts were derived from the community's reverse-engineering work,
//! notably the OpenAssetTools project's IW3 definitions.

pub mod demo;
pub mod ents;
pub mod fastfile;
pub mod fx;
pub mod install;
pub mod iwd;
pub mod iwi;
pub mod menu;
pub mod unpack;
pub mod wavelet;
pub mod xanim;
pub mod zone;

pub use install::Install;

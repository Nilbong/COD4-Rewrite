//! Relay-only multiplayer: encrypted QUIC transport, server-assigned peer
//! identities, bounded wire messages, and reusable prediction/interpolation.
//! No player IP addresses are sent to other players. This crate does not
//! integrate the Bevy game yet; see README.md for the integration contract.

pub mod client;
pub mod hosting;
pub mod lobby;
pub mod netcode;
pub mod protocol;
pub mod relay;
pub mod security;
pub mod transport;

/// For embedding a relay (the game's test relay) without depending on quinn.
pub use quinn::Endpoint;

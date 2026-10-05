//! Relay-only multiplayer: encrypted QUIC transport, server-assigned peer
//! identities, bounded wire messages, and reusable prediction/interpolation.
//! No player IP addresses are sent to other players. This crate does not
//! integrate the Bevy game yet; see README.md for the integration contract.

pub mod client;
pub mod hosting;
pub mod netcode;
pub mod protocol;
pub mod relay;
pub mod security;
pub mod transport;

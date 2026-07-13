//! Minecraft Java 1.20.2 (protocol 764).

pub mod configuration;
pub mod login;
pub mod play;

// Handshake and status wire layouts did not change from 763.
pub use super::v1_20_1::{handshake, status};

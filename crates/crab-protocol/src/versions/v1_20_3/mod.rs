//! Minecraft Java 1.20.3/1.20.4 (protocol 765).

pub mod configuration;
pub mod play;
pub use super::v1_20_2::{handshake, login, status};

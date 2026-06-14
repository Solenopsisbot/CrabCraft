//! Packet definitions for **protocol 763** (Minecraft 1.20 / 1.20.1).
//!
//! Scope so far: the handshake, status (server-list ping), and login flows —
//! everything needed to reach the play state against an offline-mode server.
//! Play-state packets land in a follow-up once the connection pipeline exists.
//!
//! > 1.20.1 has **no** configuration state: a successful login transitions the
//! > connection straight to [`crate::packet::State::Play`].

pub mod handshake;
pub mod login;
pub mod play;
pub mod status;

//! Per-version packet definitions.
//!
//! Each supported protocol version gets its own module. The codecs in
//! [`crate::io`] and the [`crate::packet::Packet`] trait are shared; only IDs
//! and field layouts live here. New versions are added as sibling modules.

pub mod v1_20_1;

/// Protocol number for Minecraft 1.20 and 1.20.1.
pub const PROTOCOL_1_20_1: i32 = 763;

//! Per-version packet definitions.
//!
//! Each supported protocol version gets its own module. The codecs in
//! [`crate::io`] and the [`crate::packet::Packet`] trait are shared; only IDs
//! and field layouts live here. New versions are added as sibling modules.

pub mod v1_20_1;
pub mod v1_20_2;
pub mod v1_20_3;
pub mod v1_20_5;
pub mod v1_21;
pub mod v1_21_2;
pub mod v1_21_4;
pub mod v1_21_5;
pub mod v1_21_6;
/// Protocol 772 (1.21.7/1.21.8) has the same packet map and field schemas as
/// 1.21.6; this module keeps the version boundary explicit for callers.
pub mod v1_21_7 {
    pub use super::v1_21_6::*;
}

/// Protocol number for Minecraft 1.20 and 1.20.1.
pub const PROTOCOL_1_20_1: i32 = 763;
/// Protocol number for Minecraft 1.20.2.
pub const PROTOCOL_1_20_2: i32 = 764;
/// Protocol number shared by Minecraft 1.20.3 and 1.20.4.
pub const PROTOCOL_1_20_3: i32 = 765;
/// Java Edition 1.20.5/1.20.6.
pub const PROTOCOL_1_20_5: i32 = 766;
/// Java Edition 1.21/1.21.1.
pub const PROTOCOL_1_21: i32 = 767;
/// Java Edition 1.21.2 and 1.21.3.
pub const PROTOCOL_1_21_2: i32 = 768;
/// Java Edition 1.21.4.
pub const PROTOCOL_1_21_4: i32 = 769;
/// Java Edition 1.21.5.
pub const PROTOCOL_1_21_5: i32 = 770;
/// Java Edition 1.21.6.
pub const PROTOCOL_1_21_6: i32 = 771;
/// Java Edition 1.21.7/1.21.8.
pub const PROTOCOL_1_21_7: i32 = 772;

//! # crab-protocol
//!
//! The version-abstracted wire layer for Crabcraft.
//!
//! This crate intentionally knows **nothing** about networking, rendering, or
//! game logic. It provides:
//!
//! * [`io`] – low-level Minecraft type codecs ([`BufExt`] / [`BufMutExt`]) built
//!   on top of the [`bytes`] crate.
//! * [`packet`] – the [`Packet`] trait plus the [`State`] / [`Bound`] enums that
//!   describe where a packet lives in the protocol.
//! * [`versions`] – concrete packet definitions, namespaced per protocol
//!   version so new versions can be added side-by-side without touching the
//!   core. Today we ship [`versions::v1_20_1`] (protocol 763).
//!
//! ## Design goal: multi-version
//!
//! Packet *IDs and layouts* change between versions, but the *codecs* and the
//! *state machine* do not. Keeping those concerns separate is what will let us
//! add 1.20.2, 1.21, … as sibling modules (eventually code-generated from the
//! vanilla data reports) without rewriting anything below this line.

pub mod error;
pub mod io;
pub mod nbt;
pub mod packet;
pub mod versions;

pub use error::ProtoError;
pub use io::{varint_len, BufExt, BufMutExt};
pub use packet::{Bound, Packet, State};

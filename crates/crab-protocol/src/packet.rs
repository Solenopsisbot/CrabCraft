//! The [`Packet`] trait and the protocol's positional enums.

use bytes::{Buf, BufMut};

use crate::error::ProtoError;

/// A connection state in the Minecraft protocol.
///
/// The handshake selects either [`State::Status`] (server-list ping) or
/// [`State::Login`]. After a successful login the connection moves to
/// [`State::Play`].
///
/// Protocol 764+ (1.20.2 and later) inserts [`State::Configuration`] between
/// Login and Play; 763 transitions directly from Login to Play.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum State {
    Handshaking,
    Status,
    Login,
    Configuration,
    Play,
}

/// Travel direction of a packet relative to the *server*.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Bound {
    /// Sent by the server, consumed by the client.
    Clientbound,
    /// Sent by the client, consumed by the server.
    Serverbound,
}

/// A single protocol packet body (everything after the packet-ID VarInt).
///
/// Implementors describe *where* they live ([`Packet::ID`], [`Packet::STATE`],
/// [`Packet::BOUND`]) and how to (de)serialize their body. Framing,
/// compression, and encryption are handled one layer up in `crab-net`.
///
/// The methods are generic over [`Buf`]/[`BufMut`] (rather than `&mut dyn`) to
/// keep call sites monomorphized and allocation-free; dispatch over many packet
/// types is done with enums per version, not trait objects.
pub trait Packet: Sized {
    /// Packet ID for this packet's `(STATE, BOUND)` in this protocol version.
    const ID: i32;
    /// Connection state this packet belongs to.
    const STATE: State;
    /// Direction this packet travels.
    const BOUND: Bound;

    /// Serializes the packet body (no ID, no length prefix) into `dst`.
    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError>;

    /// Deserializes the packet body (ID already consumed) from `src`.
    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError>;
}

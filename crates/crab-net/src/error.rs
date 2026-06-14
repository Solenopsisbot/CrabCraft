//! Connection-layer errors.

use crab_protocol::State;

/// Anything that can go wrong while moving frames over a connection.
#[derive(Debug, thiserror::Error)]
pub enum NetError {
    /// Underlying socket / IO failure (includes peer-closed as `UnexpectedEof`).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// A packet body failed to encode/decode.
    #[error("protocol error: {0}")]
    Proto(#[from] crab_protocol::ProtoError),

    /// The VarInt length prefix exceeded 5 bytes (corrupt stream).
    #[error("frame length prefix VarInt is too long")]
    VarIntTooLong,

    /// A frame announced a length beyond our safety cap.
    #[error("frame too large: {len} bytes (max {max})")]
    FrameTooLarge {
        /// Announced length.
        len: usize,
        /// Configured maximum.
        max: usize,
    },

    /// Received a packet ID we weren't expecting in the current state.
    #[error("unexpected packet id {id:#04x} in state {state:?}")]
    Unexpected {
        /// The offending packet ID.
        id: i32,
        /// State the connection was in.
        state: State,
    },
}

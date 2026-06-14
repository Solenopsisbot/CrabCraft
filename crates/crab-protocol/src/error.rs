//! Errors produced while encoding/decoding the wire protocol.

/// Failure modes for reading or writing protocol data.
#[derive(Debug, thiserror::Error)]
pub enum ProtoError {
    /// The buffer ran out before a value could be fully read.
    #[error("unexpected end of buffer: needed {needed} more byte(s)")]
    UnexpectedEof {
        /// How many additional bytes were required.
        needed: usize,
    },

    /// A VarInt was longer than the 5-byte maximum.
    #[error("VarInt is too long (exceeds 5 bytes)")]
    VarIntTooLong,

    /// A VarLong was longer than the 10-byte maximum.
    #[error("VarLong is too long (exceeds 10 bytes)")]
    VarLongTooLong,

    /// A length-prefixed string exceeded its declared maximum.
    #[error("string too long: {len} > {max}")]
    StringTooLong {
        /// Observed length.
        len: usize,
        /// Permitted maximum.
        max: usize,
    },

    /// A string contained invalid UTF-8.
    #[error("invalid UTF-8 in string")]
    InvalidUtf8(#[from] std::string::FromUtf8Error),

    /// A discriminant did not map to any known enum variant.
    #[error("invalid value {value} for {type_name}")]
    InvalidEnum {
        /// Name of the enum/type we were decoding.
        type_name: &'static str,
        /// The offending raw value.
        value: i64,
    },

    /// Encountered an unknown NBT tag type.
    #[error("invalid NBT tag type {0}")]
    NbtTag(u8),

    /// NBT nesting exceeded the safety limit.
    #[error("NBT nesting too deep")]
    NbtTooDeep,

    /// We decoded a packet ID we don't (yet) have a definition for.
    #[error("unknown packet id {id:#04x} in state {state:?} ({bound:?})")]
    UnknownPacket {
        /// The raw packet ID.
        id: i32,
        /// Connection state it arrived in.
        state: crate::packet::State,
        /// Direction it travelled.
        bound: crate::packet::Bound,
    },
}

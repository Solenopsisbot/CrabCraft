//! # crab-net
//!
//! Everything between a raw byte stream and a typed [`crab_protocol::Packet`]:
//!
//! * **Framing** – every packet is prefixed with its VarInt length.
//! * **Compression** – after the server sends `SetCompression`, frames gain an
//!   extra VarInt "uncompressed length" header and a zlib payload (`0` length
//!   meaning "stored uncompressed").
//! * **State** – the [`Connection`] tracks the current [`crab_protocol::State`]
//!   and compression threshold so callers just `send` and `read_packet`.
//!
//! Encryption (AES-CFB8) is intentionally *not* here yet — offline-mode servers
//! never request it. The [`Connection`] is generic over the byte stream
//! precisely so an encrypting wrapper can be slotted in later without touching
//! this code.

pub mod connection;
pub mod crypt;
pub mod error;
pub mod frame;

pub use connection::Connection;
pub use crypt::Aes128Cfb8;
pub use error::NetError;
pub use frame::RawPacket;

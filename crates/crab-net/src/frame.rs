//! Frame primitives: async VarInt prefix reads, the zlib sublayer, and the
//! [`RawPacket`] type produced by the decoder.

use std::io::{Read, Write};

use bytes::Bytes;
use crab_protocol::Packet;

use crate::error::NetError;

/// Hard cap on a single frame so a hostile/corrupt length can't make us
/// allocate the universe. 8 MiB is comfortably above any legitimate packet.
pub const MAX_FRAME_LEN: usize = 1 << 23;

/// A decoded packet: its leading VarInt ID plus the remaining body bytes.
///
/// Decoding the body into a concrete type is deferred to [`RawPacket::decode`]
/// so the read loop can cheaply switch on [`RawPacket::id`] first.
#[derive(Clone, Debug)]
pub struct RawPacket {
    /// The packet ID (already stripped from `body`).
    pub id: i32,
    /// The packet body (everything after the ID).
    pub body: Bytes,
}

impl RawPacket {
    /// Decodes this packet's body as `P`.
    ///
    /// Note: does not verify `P::ID == self.id`; the caller is expected to have
    /// matched on [`RawPacket::id`] already.
    pub fn decode<P: Packet>(&self) -> Result<P, NetError> {
        let mut body = self.body.clone();
        Ok(P::decode(&mut body)?)
    }
}

/// zlib-compresses `data` (used when a frame is at/over the compression
/// threshold).
pub(crate) fn zlib_compress(data: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(data)?;
    encoder.finish()
}

/// zlib-decompresses `data`, pre-sizing the output to the announced length.
pub(crate) fn zlib_decompress(data: &[u8], expected: usize) -> std::io::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(expected.min(MAX_FRAME_LEN));
    flate2::read::ZlibDecoder::new(data).read_to_end(&mut out)?;
    Ok(out)
}

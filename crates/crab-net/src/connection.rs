//! The [`Connection`]: a framed, optionally-compressed packet pipe with a
//! tracked protocol state.

use bytes::Bytes;
use crab_protocol::{Bound, BufExt, BufMutExt, Packet, State};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::error::NetError;
use crate::frame::{self, RawPacket, MAX_FRAME_LEN};

/// Sentinel threshold meaning "compression disabled".
const NO_COMPRESSION: i32 = -1;

/// A Minecraft connection over any async byte stream.
///
/// Generic over `S` so we can drive it with a real [`tokio::net::TcpStream`] in
/// production and an in-memory [`tokio::io::duplex`] pipe in tests (and, later,
/// an encrypting wrapper).
#[derive(Debug)]
pub struct Connection<S> {
    stream: S,
    state: State,
    /// Compression threshold in bytes, or [`NO_COMPRESSION`] when disabled.
    threshold: i32,
}

impl<S> Connection<S> {
    /// Wraps an existing stream. Starts in [`State::Handshaking`] with
    /// compression disabled.
    pub fn new(stream: S) -> Self {
        Self {
            stream,
            state: State::Handshaking,
            threshold: NO_COMPRESSION,
        }
    }

    /// Current protocol state.
    pub fn state(&self) -> State {
        self.state
    }

    /// Moves the connection to a new protocol state.
    pub fn set_state(&mut self, state: State) {
        self.state = state;
    }

    /// Enables (or, with a negative value, disables) compression at `threshold`
    /// bytes, as instructed by the server's `SetCompression` packet.
    pub fn set_compression(&mut self, threshold: i32) {
        self.threshold = threshold;
    }

    /// Whether compression is currently active.
    pub fn compression_enabled(&self) -> bool {
        self.threshold >= 0
    }
}

impl<S> Connection<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    /// Encodes, frames, (optionally) compresses, and writes a packet.
    pub async fn send<P: Packet>(&mut self, packet: &P) -> Result<(), NetError> {
        debug_assert_eq!(
            P::BOUND,
            Bound::Serverbound,
            "a client connection must only send serverbound packets"
        );

        // body = VarInt(id) ++ packet body
        let mut body = Vec::new();
        body.put_varint(P::ID);
        packet.encode(&mut body)?;

        let mut out = Vec::new();
        if self.threshold >= 0 {
            // Compressed format: VarInt(packet_len) VarInt(data_len) payload,
            // where data_len == 0 means the payload is stored uncompressed.
            let mut inner = Vec::new();
            if body.len() >= self.threshold as usize {
                inner.put_varint(body.len() as i32);
                inner.extend_from_slice(&frame::zlib_compress(&body)?);
            } else {
                inner.put_varint(0);
                inner.extend_from_slice(&body);
            }
            out.put_varint(inner.len() as i32);
            out.extend_from_slice(&inner);
        } else {
            // Uncompressed format: VarInt(len) ++ body
            out.put_varint(body.len() as i32);
            out.extend_from_slice(&body);
        }

        tracing::trace!(
            id = format_args!("{:#04x}", P::ID),
            body_len = body.len(),
            wire_len = out.len(),
            compressed = self.compression_enabled(),
            "send"
        );
        self.stream.write_all(&out).await?;
        self.stream.flush().await?;
        Ok(())
    }

    /// Reads exactly one packet, transparently undoing framing and compression.
    pub async fn read_packet(&mut self) -> Result<RawPacket, NetError> {
        let frame_len = frame::read_varint_async(&mut self.stream).await?;
        if frame_len < 0 || frame_len as usize > MAX_FRAME_LEN {
            return Err(NetError::FrameTooLarge {
                len: frame_len.max(0) as usize,
                max: MAX_FRAME_LEN,
            });
        }

        let mut frame_buf = vec![0u8; frame_len as usize];
        self.stream.read_exact(&mut frame_buf).await?;

        let payload = if self.threshold >= 0 {
            let mut slice: &[u8] = &frame_buf;
            let data_len = slice.read_varint()?;
            if data_len == 0 {
                // stored uncompressed
                slice.to_vec()
            } else {
                let data_len = data_len as usize;
                if data_len > MAX_FRAME_LEN {
                    return Err(NetError::FrameTooLarge {
                        len: data_len,
                        max: MAX_FRAME_LEN,
                    });
                }
                frame::zlib_decompress(slice, data_len)?
            }
        } else {
            frame_buf
        };

        // payload = VarInt(id) ++ body
        let mut cursor: &[u8] = &payload;
        let id = cursor.read_varint()?;
        let body = Bytes::copy_from_slice(cursor);
        Ok(RawPacket { id, body })
    }
}

impl Connection<tokio::net::TcpStream> {
    /// Opens a TCP connection to `addr` (e.g. `"127.0.0.1:25565"`).
    pub async fn connect(addr: &str) -> Result<Self, NetError> {
        let stream = tokio::net::TcpStream::connect(addr).await?;
        // Minecraft is latency-sensitive and chatty with small packets.
        let _ = stream.set_nodelay(true);
        Ok(Self::new(stream))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_protocol::versions::v1_20_1::handshake::{Handshake, NextState};
    use crab_protocol::versions::PROTOCOL_1_20_1;

    fn sample_handshake(addr: &str) -> Handshake {
        Handshake {
            protocol_version: PROTOCOL_1_20_1,
            server_address: addr.to_string(),
            server_port: 25565,
            next_state: NextState::Login,
        }
    }

    #[tokio::test]
    async fn frame_roundtrip_uncompressed() {
        let (a, b) = tokio::io::duplex(4096);
        let mut client = Connection::new(a);
        let mut server = Connection::new(b);

        let pkt = sample_handshake("localhost");
        client.send(&pkt).await.unwrap();

        let raw = server.read_packet().await.unwrap();
        assert_eq!(raw.id, Handshake::ID);
        assert_eq!(raw.decode::<Handshake>().unwrap(), pkt);
    }

    #[tokio::test]
    async fn frame_roundtrip_compressed() {
        let (a, b) = tokio::io::duplex(128 * 1024);
        let mut client = Connection::new(a);
        let mut server = Connection::new(b);
        // threshold 0 => compress everything; exercise the zlib path
        client.set_compression(0);
        server.set_compression(0);

        // a non-trivial but field-legal (<=255 char) address, very compressible
        let pkt = sample_handshake(&"x".repeat(200));
        client.send(&pkt).await.unwrap();

        let raw = server.read_packet().await.unwrap();
        assert_eq!(raw.decode::<Handshake>().unwrap(), pkt);
    }

    #[tokio::test]
    async fn frame_compressed_below_threshold_is_stored_uncompressed() {
        let (a, b) = tokio::io::duplex(4096);
        let mut client = Connection::new(a);
        let mut server = Connection::new(b);
        // High threshold => small packets travel uncompressed (data_len == 0)
        client.set_compression(1024);
        server.set_compression(1024);

        let pkt = sample_handshake("tiny");
        client.send(&pkt).await.unwrap();

        let raw = server.read_packet().await.unwrap();
        assert_eq!(raw.decode::<Handshake>().unwrap(), pkt);
    }
}

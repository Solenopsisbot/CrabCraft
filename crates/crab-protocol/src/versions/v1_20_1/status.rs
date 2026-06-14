//! Status state (protocol 763): the server-list ping handshake.
//!
//! Flow: client sends [`StatusRequest`] → server replies [`StatusResponse`]
//! (a JSON blob with MOTD/version/players) → client sends [`PingRequest`] with
//! a nonce → server echoes it in [`PongResponse`]. This is the cheapest
//! end-to-end smoke test: no auth, no encryption.

use bytes::{Buf, BufMut};

use crate::error::ProtoError;
use crate::io::{BufExt, BufMutExt};
use crate::packet::{Bound, Packet, State};

/// Serverbound: "tell me your status".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StatusRequest;

impl Packet for StatusRequest {
    const ID: i32 = 0x00;
    const STATE: State = State::Status;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, _dst: &mut B) -> Result<(), ProtoError> {
        Ok(())
    }

    fn decode<B: Buf>(_src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self)
    }
}

/// Serverbound: latency probe carrying an arbitrary nonce.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PingRequest {
    pub payload: i64,
}

impl Packet for PingRequest {
    const ID: i32 = 0x01;
    const STATE: State = State::Status;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_i64(self.payload);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            payload: src.read_i64()?,
        })
    }
}

/// Clientbound: JSON status document (MOTD, versions, player sample, …).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusResponse {
    pub json: String,
}

impl Packet for StatusResponse {
    const ID: i32 = 0x00;
    const STATE: State = State::Status;
    const BOUND: Bound = Bound::Clientbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_string(&self.json);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            json: src.read_string(32767)?,
        })
    }
}

/// Clientbound: echo of the [`PingRequest`] nonce.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PongResponse {
    pub payload: i64,
}

impl Packet for PongResponse {
    const ID: i32 = 0x01;
    const STATE: State = State::Status;
    const BOUND: Bound = Bound::Clientbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_i64(self.payload);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            payload: src.read_i64()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_response_roundtrips() {
        let pkt = StatusResponse {
            json: r#"{"description":{"text":"A Crabcraft test"},"version":{"name":"1.20.1","protocol":763}}"#.into(),
        };
        let mut buf = Vec::new();
        pkt.encode(&mut buf).unwrap();
        let mut slice: &[u8] = &buf;
        assert_eq!(StatusResponse::decode(&mut slice).unwrap(), pkt);
        assert_eq!(slice.remaining(), 0);
    }

    #[test]
    fn ping_pong_roundtrips() {
        let ping = PingRequest {
            payload: 0x0BADF00D_DEADBEEF_u64 as i64,
        };
        let mut buf = Vec::new();
        ping.encode(&mut buf).unwrap();
        let mut slice: &[u8] = &buf;
        let pong = PongResponse::decode(&mut slice).unwrap();
        assert_eq!(pong.payload, ping.payload);
        assert_eq!(slice.remaining(), 0);
    }
}

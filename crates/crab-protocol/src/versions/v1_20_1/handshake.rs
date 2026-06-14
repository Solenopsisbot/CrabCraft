//! Handshaking state (protocol 763). Serverbound only.

use bytes::{Buf, BufMut};

use crate::error::ProtoError;
use crate::io::{BufExt, BufMutExt};
use crate::packet::{Bound, Packet, State};

/// Which state the client wants to enter after the handshake.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NextState {
    Status,
    Login,
}

impl NextState {
    fn to_wire(self) -> i32 {
        match self {
            NextState::Status => 1,
            NextState::Login => 2,
        }
    }

    fn from_wire(value: i32) -> Result<Self, ProtoError> {
        match value {
            1 => Ok(NextState::Status),
            2 => Ok(NextState::Login),
            other => Err(ProtoError::InvalidEnum {
                type_name: "handshake::NextState",
                value: i64::from(other),
            }),
        }
    }
}

/// The first packet a client sends. Picks the protocol version and target
/// state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Handshake {
    /// Protocol version the client is speaking (763 for 1.20.1).
    pub protocol_version: i32,
    /// Hostname the client used to connect (used by virtual hosts / proxies).
    pub server_address: String,
    /// Port the client used to connect.
    pub server_port: u16,
    /// Desired next state.
    pub next_state: NextState,
}

impl Packet for Handshake {
    const ID: i32 = 0x00;
    const STATE: State = State::Handshaking;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_varint(self.protocol_version);
        dst.put_string(&self.server_address);
        dst.put_u16(self.server_port);
        dst.put_varint(self.next_state.to_wire());
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            protocol_version: src.read_varint()?,
            // hostnames are capped at 255 on the wire
            server_address: src.read_string(255)?,
            server_port: src.read_u16()?,
            next_state: NextState::from_wire(src.read_varint()?)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::versions::PROTOCOL_1_20_1;

    #[test]
    fn handshake_roundtrips() {
        let pkt = Handshake {
            protocol_version: PROTOCOL_1_20_1,
            server_address: "play.example.net".into(),
            server_port: 25565,
            next_state: NextState::Login,
        };

        let mut buf = Vec::new();
        pkt.encode(&mut buf).unwrap();

        let mut slice: &[u8] = &buf;
        let decoded = Handshake::decode(&mut slice).unwrap();
        assert_eq!(decoded, pkt);
        assert_eq!(slice.remaining(), 0, "decoder must consume the whole body");
    }
}

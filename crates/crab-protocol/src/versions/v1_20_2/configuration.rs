//! Configuration-state packets for protocol 764.

use bytes::{Buf, BufMut};

use crate::error::ProtoError;
use crate::io::{BufExt, BufMutExt};
use crate::nbt::{self, Nbt};
use crate::packet::{Bound, Packet, State};

macro_rules! empty_packet {
    ($name:ident, $id:expr, $bound:expr) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub struct $name;
        impl Packet for $name {
            const ID: i32 = $id;
            const STATE: State = State::Configuration;
            const BOUND: Bound = $bound;
            fn encode<B: BufMut>(&self, _dst: &mut B) -> Result<(), ProtoError> {
                Ok(())
            }
            fn decode<B: Buf>(_src: &mut B) -> Result<Self, ProtoError> {
                Ok(Self)
            }
        }
    };
}

empty_packet!(ClientboundFinishConfiguration, 0x02, Bound::Clientbound);
empty_packet!(FinishConfiguration, 0x02, Bound::Serverbound);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigurationClientInformation {
    pub locale: String,
    pub view_distance: i8,
    pub chat_mode: i32,
    pub chat_colors: bool,
    pub skin_parts: u8,
    pub main_hand: i32,
    pub text_filtering: bool,
    pub server_listing: bool,
}

impl Packet for ConfigurationClientInformation {
    const ID: i32 = 0x00;
    const STATE: State = State::Configuration;
    const BOUND: Bound = Bound::Serverbound;
    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_string(&self.locale);
        dst.put_i8(self.view_distance);
        dst.put_varint(self.chat_mode);
        dst.put_bool(self.chat_colors);
        dst.put_u8(self.skin_parts);
        dst.put_varint(self.main_hand);
        dst.put_bool(self.text_filtering);
        dst.put_bool(self.server_listing);
        Ok(())
    }
    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            locale: src.read_string(16)?,
            view_distance: src.read_i8()?,
            chat_mode: src.read_varint()?,
            chat_colors: src.read_bool()?,
            skin_parts: src.read_u8()?,
            main_hand: src.read_varint()?,
            text_filtering: src.read_bool()?,
            server_listing: src.read_bool()?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConfigurationKeepAlive {
    pub id: i64,
}
impl Packet for ConfigurationKeepAlive {
    const ID: i32 = 0x03;
    const STATE: State = State::Configuration;
    const BOUND: Bound = Bound::Clientbound;
    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_i64(self.id);
        Ok(())
    }
    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            id: src.read_i64()?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConfigurationKeepAliveResponse {
    pub id: i64,
}
impl Packet for ConfigurationKeepAliveResponse {
    const ID: i32 = 0x03;
    const STATE: State = State::Configuration;
    const BOUND: Bound = Bound::Serverbound;
    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_i64(self.id);
        Ok(())
    }
    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            id: src.read_i64()?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConfigurationPing {
    pub id: i32,
}
impl Packet for ConfigurationPing {
    const ID: i32 = 0x04;
    const STATE: State = State::Configuration;
    const BOUND: Bound = Bound::Clientbound;
    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_i32(self.id);
        Ok(())
    }
    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            id: src.read_i32()?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConfigurationPong {
    pub id: i32,
}
impl Packet for ConfigurationPong {
    const ID: i32 = 0x04;
    const STATE: State = State::Configuration;
    const BOUND: Bound = Bound::Serverbound;
    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_i32(self.id);
        Ok(())
    }
    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            id: src.read_i32()?,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RegistryData {
    pub codec: Nbt,
}
impl Packet for RegistryData {
    const ID: i32 = 0x05;
    const STATE: State = State::Configuration;
    const BOUND: Bound = Bound::Clientbound;
    fn encode<B: BufMut>(&self, _dst: &mut B) -> Result<(), ProtoError> {
        Err(ProtoError::InvalidEnum {
            type_name: "RegistryData.encode",
            value: 0,
        })
    }
    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            codec: nbt::read_anonymous_nbt(src)?,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigurationResourcePackStatus {
    pub status: i32,
}
impl Packet for ConfigurationResourcePackStatus {
    const ID: i32 = 0x05;
    const STATE: State = State::Configuration;
    const BOUND: Bound = Bound::Serverbound;
    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_varint(self.status);
        Ok(())
    }
    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            status: src.read_varint()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Buf;

    fn roundtrip<P: Packet + PartialEq + std::fmt::Debug>(packet: &P) {
        let mut body = Vec::new();
        packet.encode(&mut body).unwrap();
        let mut input = body.as_slice();
        assert_eq!(&P::decode(&mut input).unwrap(), packet);
        assert_eq!(input.remaining(), 0);
    }

    #[test]
    fn configuration_responses_roundtrip() {
        roundtrip(&ConfigurationClientInformation {
            locale: "en_us".to_string(),
            view_distance: 12,
            chat_mode: 0,
            chat_colors: true,
            skin_parts: 0x7f,
            main_hand: 1,
            text_filtering: false,
            server_listing: true,
        });
        roundtrip(&ConfigurationKeepAliveResponse { id: -42 });
        roundtrip(&ConfigurationPong { id: 17 });
        roundtrip(&ConfigurationResourcePackStatus { status: 3 });
        roundtrip(&FinishConfiguration);
    }

    #[test]
    fn registry_data_reads_anonymous_network_nbt() {
        // Compound { Int("answer", 42), End } with no root-name field.
        let bytes = [
            10, 3, 0, 6, b'a', b'n', b's', b'w', b'e', b'r', 0, 0, 0, 42, 0,
        ];
        let mut input = bytes.as_slice();
        let packet = RegistryData::decode(&mut input).unwrap();
        assert_eq!(packet.codec.get("answer"), Some(&Nbt::Int(42)));
        assert_eq!(input.remaining(), 0);
    }
}

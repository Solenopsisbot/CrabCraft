use bytes::{Buf, BufMut};

use crate::error::ProtoError;
use crate::io::{BufExt, BufMutExt};
use crate::nbt::{self, Nbt};
use crate::packet::{Bound, Packet, State};

#[derive(Clone, Debug, PartialEq)]
pub struct CustomPayload {
    pub channel: String,
    pub data: Vec<u8>,
}

impl Packet for CustomPayload {
    const ID: i32 = 0x01;
    const STATE: State = State::Configuration;
    const BOUND: Bound = Bound::Clientbound;
    fn encode<B: BufMut>(&self, _dst: &mut B) -> Result<(), ProtoError> {
        unreachable!()
    }
    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            channel: src.read_string(32_767)?,
            data: src.read_byte_array()?,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FeatureFlags(pub Vec<String>);

impl Packet for FeatureFlags {
    const ID: i32 = 0x0c;
    const STATE: State = State::Configuration;
    const BOUND: Bound = Bound::Clientbound;
    fn encode<B: BufMut>(&self, _dst: &mut B) -> Result<(), ProtoError> {
        unreachable!()
    }
    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        let count = src.read_varint()?;
        if !(0..=1024).contains(&count) {
            return Err(ProtoError::InvalidEnum {
                type_name: "FeatureFlags.count",
                value: i64::from(count),
            });
        }
        let mut flags = Vec::with_capacity(count as usize);
        for _ in 0..count {
            flags.push(src.read_string(32_767)?);
        }
        Ok(Self(flags))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RegistryData {
    pub id: String,
    pub entries: Vec<(String, Option<Nbt>)>,
}

impl Packet for RegistryData {
    const ID: i32 = 0x07;
    const STATE: State = State::Configuration;
    const BOUND: Bound = Bound::Clientbound;
    fn encode<B: BufMut>(&self, _dst: &mut B) -> Result<(), ProtoError> {
        Err(ProtoError::InvalidEnum {
            type_name: "RegistryData.encode",
            value: 0,
        })
    }
    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        let id = src.read_string(32_767)?;
        let count = src.read_varint()?;
        if !(0..=65_536).contains(&count) {
            return Err(ProtoError::InvalidEnum {
                type_name: "RegistryData.entry_count",
                value: i64::from(count),
            });
        }
        let mut entries = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let key = src.read_string(32_767)?;
            let value = src
                .read_bool()?
                .then(|| nbt::read_anonymous_nbt(src))
                .transpose()?;
            entries.push((key, value));
        }
        Ok(Self { id, entries })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectKnownPacks;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectKnownPacksResponse(pub Vec<(String, String, String)>);

impl Packet for SelectKnownPacksResponse {
    const ID: i32 = 0x07;
    const STATE: State = State::Configuration;
    const BOUND: Bound = Bound::Serverbound;
    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        if self.0.len() > 1024 {
            return Err(ProtoError::InvalidEnum {
                type_name: "SelectKnownPacks.count",
                value: self.0.len() as i64,
            });
        }
        dst.put_varint(self.0.len() as i32);
        for (namespace, id, version) in &self.0 {
            dst.put_string(namespace);
            dst.put_string(id);
            dst.put_string(version);
        }
        Ok(())
    }
    fn decode<B: Buf>(_src: &mut B) -> Result<Self, ProtoError> {
        Err(ProtoError::InvalidEnum {
            type_name: "SelectKnownPacksResponse.decode",
            value: 0,
        })
    }
}

impl Packet for SelectKnownPacks {
    const ID: i32 = 0x07;
    const STATE: State = State::Configuration;
    const BOUND: Bound = Bound::Serverbound;
    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        // The vanilla core pack is stable across the 1.21.x configuration
        // protocol. Advertising it lets 1.21.7/1.21.8 servers complete the
        // known-packs negotiation instead of rejecting an empty selection.
        dst.put_varint(1);
        dst.put_string("minecraft");
        dst.put_string("core");
        dst.put_string("1.21.8");
        Ok(())
    }
    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        let count = src.read_varint()?;
        if !(0..=1_024).contains(&count) {
            return Err(ProtoError::InvalidEnum {
                type_name: "SelectKnownPacks.pack_count",
                value: i64::from(count),
            });
        }
        for _ in 0..count {
            let _ = src.read_string(32_767)?;
            let _ = src.read_string(32_767)?;
            let _ = src.read_string(32_767)?;
        }
        Ok(Self)
    }
}

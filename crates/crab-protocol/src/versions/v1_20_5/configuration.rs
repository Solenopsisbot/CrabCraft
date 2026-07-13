use bytes::{Buf, BufMut};

use crate::error::ProtoError;
use crate::io::{BufExt, BufMutExt};
use crate::nbt::{self, Nbt};
use crate::packet::{Bound, Packet, State};

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

impl Packet for SelectKnownPacks {
    const ID: i32 = 0x07;
    const STATE: State = State::Configuration;
    const BOUND: Bound = Bound::Serverbound;
    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_varint(0);
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

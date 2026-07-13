use bytes::{Buf, BufMut};
use uuid::Uuid;

use crate::error::ProtoError;
use crate::io::{BufExt, BufMutExt};
use crate::nbt::{self, Nbt};
use crate::packet::{Bound, Packet, State};

#[derive(Clone, Debug, PartialEq)]
pub struct AddResourcePack {
    pub uuid: Uuid,
    pub url: String,
    pub hash: String,
    pub forced: bool,
    pub prompt: Option<Nbt>,
}

impl Packet for AddResourcePack {
    const ID: i32 = 0x07;
    const STATE: State = State::Configuration;
    const BOUND: Bound = Bound::Clientbound;
    fn encode<B: BufMut>(&self, _dst: &mut B) -> Result<(), ProtoError> {
        Err(ProtoError::InvalidEnum {
            type_name: "AddResourcePack.encode",
            value: 0,
        })
    }
    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            uuid: src.read_uuid()?,
            url: src.read_string(32_767)?,
            hash: src.read_string(40)?,
            forced: src.read_bool()?,
            prompt: src
                .read_bool()?
                .then(|| nbt::read_anonymous_nbt(src))
                .transpose()?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourcePackStatus {
    pub uuid: Uuid,
    pub status: i32,
}

impl Packet for ResourcePackStatus {
    const ID: i32 = 0x05;
    const STATE: State = State::Configuration;
    const BOUND: Bound = Bound::Serverbound;
    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_uuid(self.uuid);
        dst.put_varint(self.status);
        Ok(())
    }
    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            uuid: src.read_uuid()?,
            status: src.read_varint()?,
        })
    }
}

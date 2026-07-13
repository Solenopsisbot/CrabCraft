//! Configuration-state payload changes in protocol 768.

use bytes::{Buf, BufMut};

use crate::error::ProtoError;
use crate::io::{BufExt, BufMutExt};
use crate::packet::{Bound, Packet, State};

/// Client settings gained the particle-status preference in 1.21.2.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientInformation {
    pub locale: String,
    pub view_distance: i8,
    pub chat_mode: i32,
    pub chat_colors: bool,
    pub skin_parts: u8,
    pub main_hand: i32,
    pub text_filtering: bool,
    pub server_listing: bool,
    /// 0 all, 1 decreased, 2 minimal.
    pub particle_status: i32,
}

impl ClientInformation {
    #[must_use]
    pub fn sensible_defaults() -> Self {
        Self {
            locale: "en_us".into(),
            view_distance: 10,
            chat_mode: 0,
            chat_colors: true,
            skin_parts: 0x7f,
            main_hand: 1,
            text_filtering: false,
            server_listing: true,
            particle_status: 0,
        }
    }
}

impl Packet for ClientInformation {
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
        dst.put_varint(self.particle_status);
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
            particle_status: src.read_varint()?,
        })
    }
}

//! Login changes introduced by protocol 764.

use bytes::{Buf, BufMut};
use uuid::Uuid;

use crate::error::ProtoError;
use crate::io::{BufExt, BufMutExt};
use crate::packet::{Bound, Packet, State};

pub use crate::versions::v1_20_1::login::{
    EncryptionRequest, EncryptionResponse, LoginDisconnect, LoginSuccess, Property, SetCompression,
};

/// 764 Login Start always carries a UUID; the 763 presence boolean was removed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginStart {
    pub name: String,
    pub uuid: Uuid,
}

impl Packet for LoginStart {
    const ID: i32 = 0x00;
    const STATE: State = State::Login;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_string(&self.name);
        dst.put_uuid(self.uuid);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            name: src.read_string(16)?,
            uuid: src.read_uuid()?,
        })
    }
}

/// Sent after Login Success to enter the Configuration state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoginAcknowledged;

impl Packet for LoginAcknowledged {
    const ID: i32 = 0x03;
    const STATE: State = State::Login;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, _dst: &mut B) -> Result<(), ProtoError> {
        Ok(())
    }

    fn decode<B: Buf>(_src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Buf;

    #[test]
    fn required_uuid_login_and_acknowledgement_roundtrip() {
        let packet = LoginStart {
            name: "Ferris".to_string(),
            uuid: Uuid::from_u128(42),
        };
        let mut body = Vec::new();
        packet.encode(&mut body).unwrap();
        let mut input = body.as_slice();
        assert_eq!(LoginStart::decode(&mut input).unwrap(), packet);
        assert_eq!(input.remaining(), 0);

        let mut empty = Vec::new();
        LoginAcknowledged.encode(&mut empty).unwrap();
        assert!(empty.is_empty());
    }
}

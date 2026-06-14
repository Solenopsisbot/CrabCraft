//! Login state (protocol 763).
//!
//! Offline-mode flow (no encryption):
//! ```text
//! C -> S  LoginStart
//! S -> C  SetCompression   (optional)
//! S -> C  LoginSuccess     -> connection enters Play
//! ```
//! Online-mode servers instead answer [`LoginStart`] with [`EncryptionRequest`];
//! we decode it so the client can fail loudly ("server is online-mode") until
//! the auth crate lands.

use bytes::{Buf, BufMut};
use uuid::Uuid;

use crate::error::ProtoError;
use crate::io::{BufExt, BufMutExt};
use crate::packet::{Bound, Packet, State};

/// A signed profile property (skin/cape textures, etc.). Empty for offline mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Property {
    pub name: String,
    pub value: String,
    pub signature: Option<String>,
}

/// Serverbound `0x00`: begin login.
///
/// In 1.20.1 this carries the username plus an *optional* player UUID (the
/// boolean was removed in 1.20.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginStart {
    pub name: String,
    pub uuid: Option<Uuid>,
}

impl Packet for LoginStart {
    const ID: i32 = 0x00;
    const STATE: State = State::Login;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_string(&self.name);
        dst.put_bool(self.uuid.is_some());
        if let Some(uuid) = self.uuid {
            dst.put_uuid(uuid);
        }
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        let name = src.read_string(16)?;
        let has_uuid = src.read_bool()?;
        let uuid = if has_uuid {
            Some(src.read_uuid()?)
        } else {
            None
        };
        Ok(Self { name, uuid })
    }
}

/// Clientbound `0x00`: login rejected, body is a JSON chat component.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginDisconnect {
    pub reason_json: String,
}

impl Packet for LoginDisconnect {
    const ID: i32 = 0x00;
    const STATE: State = State::Login;
    const BOUND: Bound = Bound::Clientbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_string(&self.reason_json);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            reason_json: src.read_string(262144)?,
        })
    }
}

/// Clientbound `0x01`: server wants encryption (i.e. it's in online mode).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncryptionRequest {
    pub server_id: String,
    pub public_key: Vec<u8>,
    pub verify_token: Vec<u8>,
}

impl Packet for EncryptionRequest {
    const ID: i32 = 0x01;
    const STATE: State = State::Login;
    const BOUND: Bound = Bound::Clientbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_string(&self.server_id);
        dst.put_byte_array(&self.public_key);
        dst.put_byte_array(&self.verify_token);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            server_id: src.read_string(20)?,
            public_key: src.read_byte_array()?,
            verify_token: src.read_byte_array()?,
        })
    }
}

/// Serverbound `0x01`: reply to [`EncryptionRequest`] with the RSA-encrypted
/// shared secret and verify token. Sent in plaintext; everything after it is
/// AES-encrypted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncryptionResponse {
    pub shared_secret: Vec<u8>,
    pub verify_token: Vec<u8>,
}

impl Packet for EncryptionResponse {
    const ID: i32 = 0x01;
    const STATE: State = State::Login;
    const BOUND: Bound = Bound::Serverbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_byte_array(&self.shared_secret);
        dst.put_byte_array(&self.verify_token);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            shared_secret: src.read_byte_array()?,
            verify_token: src.read_byte_array()?,
        })
    }
}

/// Clientbound `0x02`: login accepted. After this the connection is in Play.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginSuccess {
    pub uuid: Uuid,
    pub username: String,
    pub properties: Vec<Property>,
}

impl Packet for LoginSuccess {
    const ID: i32 = 0x02;
    const STATE: State = State::Login;
    const BOUND: Bound = Bound::Clientbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_uuid(self.uuid);
        dst.put_string(&self.username);
        dst.put_varint(self.properties.len() as i32);
        for prop in &self.properties {
            dst.put_string(&prop.name);
            dst.put_string(&prop.value);
            dst.put_bool(prop.signature.is_some());
            if let Some(sig) = &prop.signature {
                dst.put_string(sig);
            }
        }
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        let uuid = src.read_uuid()?;
        let username = src.read_string(16)?;
        let count = src.read_varint()?.max(0);
        let mut properties = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let name = src.read_string(32767)?;
            let value = src.read_string(32767)?;
            let signature = if src.read_bool()? {
                Some(src.read_string(32767)?)
            } else {
                None
            };
            properties.push(Property {
                name,
                value,
                signature,
            });
        }
        Ok(Self {
            uuid,
            username,
            properties,
        })
    }
}

/// Clientbound `0x03`: enable packet compression at/above `threshold` bytes.
/// A negative threshold disables compression.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetCompression {
    pub threshold: i32,
}

impl Packet for SetCompression {
    const ID: i32 = 0x03;
    const STATE: State = State::Login;
    const BOUND: Bound = Bound::Clientbound;

    fn encode<B: BufMut>(&self, dst: &mut B) -> Result<(), ProtoError> {
        dst.put_varint(self.threshold);
        Ok(())
    }

    fn decode<B: Buf>(src: &mut B) -> Result<Self, ProtoError> {
        Ok(Self {
            threshold: src.read_varint()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_start_with_uuid_roundtrips() {
        let pkt = LoginStart {
            name: "Ferris".into(),
            uuid: Some(Uuid::from_u128(0xdead_beef_dead_beef_dead_beef_dead_beef)),
        };
        let mut buf = Vec::new();
        pkt.encode(&mut buf).unwrap();
        let mut slice: &[u8] = &buf;
        assert_eq!(LoginStart::decode(&mut slice).unwrap(), pkt);
        assert_eq!(slice.remaining(), 0);
    }

    #[test]
    fn login_start_without_uuid_roundtrips() {
        let pkt = LoginStart {
            name: "Ferris".into(),
            uuid: None,
        };
        let mut buf = Vec::new();
        pkt.encode(&mut buf).unwrap();
        let mut slice: &[u8] = &buf;
        assert_eq!(LoginStart::decode(&mut slice).unwrap(), pkt);
        assert_eq!(slice.remaining(), 0);
    }

    #[test]
    fn login_success_with_properties_roundtrips() {
        let pkt = LoginSuccess {
            uuid: Uuid::from_u128(1),
            username: "Ferris".into(),
            properties: vec![
                Property {
                    name: "textures".into(),
                    value: "base64data".into(),
                    signature: Some("sig".into()),
                },
                Property {
                    name: "unsigned".into(),
                    value: "x".into(),
                    signature: None,
                },
            ],
        };
        let mut buf = Vec::new();
        pkt.encode(&mut buf).unwrap();
        let mut slice: &[u8] = &buf;
        assert_eq!(LoginSuccess::decode(&mut slice).unwrap(), pkt);
        assert_eq!(slice.remaining(), 0);
    }

    #[test]
    fn encryption_response_roundtrips() {
        let pkt = EncryptionResponse {
            shared_secret: vec![1, 2, 3, 4, 5, 6, 7, 8],
            verify_token: vec![9, 8, 7],
        };
        let mut buf = Vec::new();
        pkt.encode(&mut buf).unwrap();
        let mut slice: &[u8] = &buf;
        assert_eq!(EncryptionResponse::decode(&mut slice).unwrap(), pkt);
        assert_eq!(slice.remaining(), 0);
    }

    #[test]
    fn set_compression_roundtrips() {
        for threshold in [-1, 0, 256, 1_000_000] {
            let pkt = SetCompression { threshold };
            let mut buf = Vec::new();
            pkt.encode(&mut buf).unwrap();
            let mut slice: &[u8] = &buf;
            assert_eq!(SetCompression::decode(&mut slice).unwrap(), pkt);
            assert_eq!(slice.remaining(), 0);
        }
    }
}

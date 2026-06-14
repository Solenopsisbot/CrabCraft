//! Minecraft type codecs layered on top of [`bytes::Buf`] / [`bytes::BufMut`].
//!
//! All *read* operations are fallible ([`BufExt`]) and never panic on a short
//! buffer — they return [`ProtoError::UnexpectedEof`] instead. This matters
//! because packet bodies arrive from the network and may be malformed or
//! truncated. *Write* operations ([`BufMutExt`]) are infallible: growing an
//! in-memory buffer can't fail in any way we care to model.
//!
//! The read methods are named `read_*` rather than `try_get_*` to avoid
//! colliding with the fallible getters `bytes` itself added in 1.11 (which
//! return `bytes::TryGetError`); keeping our own names means our codec layer
//! stays fully self-contained and always yields [`ProtoError`].

use bytes::{Buf, BufMut};
use uuid::Uuid;

use crate::error::ProtoError;

/// Number of bytes the given value occupies when encoded as a VarInt (1..=5).
#[must_use]
pub fn varint_len(value: i32) -> usize {
    let mut v = value as u32;
    let mut n = 1;
    while v >= 0x80 {
        v >>= 7;
        n += 1;
    }
    n
}

/// Fallible reads of Minecraft protocol types.
///
/// Implemented for every [`Buf`], so `&[u8]`, [`bytes::Bytes`], etc. all gain
/// these methods for free.
pub trait BufExt: Buf {
    /// Returns `Err` unless at least `needed` bytes remain.
    fn ensure(&self, needed: usize) -> Result<(), ProtoError> {
        let have = self.remaining();
        if have < needed {
            Err(ProtoError::UnexpectedEof {
                needed: needed - have,
            })
        } else {
            Ok(())
        }
    }

    fn read_u8(&mut self) -> Result<u8, ProtoError> {
        self.ensure(1)?;
        Ok(self.get_u8())
    }

    fn read_i8(&mut self) -> Result<i8, ProtoError> {
        self.ensure(1)?;
        Ok(self.get_i8())
    }

    fn read_u16(&mut self) -> Result<u16, ProtoError> {
        self.ensure(2)?;
        Ok(self.get_u16())
    }

    fn read_i16(&mut self) -> Result<i16, ProtoError> {
        self.ensure(2)?;
        Ok(self.get_i16())
    }

    fn read_i32(&mut self) -> Result<i32, ProtoError> {
        self.ensure(4)?;
        Ok(self.get_i32())
    }

    fn read_i64(&mut self) -> Result<i64, ProtoError> {
        self.ensure(8)?;
        Ok(self.get_i64())
    }

    fn read_u64(&mut self) -> Result<u64, ProtoError> {
        self.ensure(8)?;
        Ok(self.get_u64())
    }

    fn read_f32(&mut self) -> Result<f32, ProtoError> {
        self.ensure(4)?;
        Ok(self.get_f32())
    }

    fn read_f64(&mut self) -> Result<f64, ProtoError> {
        self.ensure(8)?;
        Ok(self.get_f64())
    }

    fn read_bool(&mut self) -> Result<bool, ProtoError> {
        Ok(self.read_u8()? != 0)
    }

    /// Reads a variable-length signed 32-bit integer (LEB128-ish, max 5 bytes).
    fn read_varint(&mut self) -> Result<i32, ProtoError> {
        let mut result: i32 = 0;
        let mut shift: u32 = 0;
        loop {
            let byte = self.read_u8()?;
            result |= ((byte & 0x7F) as i32) << shift;
            if byte & 0x80 == 0 {
                return Ok(result);
            }
            shift += 7;
            if shift >= 32 {
                return Err(ProtoError::VarIntTooLong);
            }
        }
    }

    /// Reads a variable-length signed 64-bit integer (max 10 bytes).
    fn read_varlong(&mut self) -> Result<i64, ProtoError> {
        let mut result: i64 = 0;
        let mut shift: u32 = 0;
        loop {
            let byte = self.read_u8()?;
            result |= ((byte & 0x7F) as i64) << shift;
            if byte & 0x80 == 0 {
                return Ok(result);
            }
            shift += 7;
            if shift >= 64 {
                return Err(ProtoError::VarLongTooLong);
            }
        }
    }

    /// Reads exactly `len` raw bytes.
    fn read_bytes(&mut self, len: usize) -> Result<Vec<u8>, ProtoError> {
        self.ensure(len)?;
        let mut out = vec![0u8; len];
        self.copy_to_slice(&mut out);
        Ok(out)
    }

    /// Reads a VarInt-length-prefixed byte array.
    fn read_byte_array(&mut self) -> Result<Vec<u8>, ProtoError> {
        let len = self.read_varint()?.max(0) as usize;
        self.read_bytes(len)
    }

    /// Reads a VarInt-length-prefixed UTF-8 string.
    ///
    /// `max_chars` is the protocol-defined character limit; the on-wire byte
    /// budget is `max_chars * 4` (UTF-8 worst case), which we use to reject
    /// hostile allocations before reading.
    fn read_string(&mut self, max_chars: usize) -> Result<String, ProtoError> {
        let len = self.read_varint()?;
        if len < 0 {
            return Err(ProtoError::StringTooLong {
                len: 0,
                max: max_chars,
            });
        }
        let len = len as usize;
        let max_bytes = max_chars.saturating_mul(4);
        if len > max_bytes {
            return Err(ProtoError::StringTooLong {
                len,
                max: max_bytes,
            });
        }
        let bytes = self.read_bytes(len)?;
        let s = String::from_utf8(bytes)?;
        let chars = s.chars().count();
        if chars > max_chars {
            return Err(ProtoError::StringTooLong {
                len: chars,
                max: max_chars,
            });
        }
        Ok(s)
    }

    /// Reads a 128-bit UUID (big-endian).
    fn read_uuid(&mut self) -> Result<Uuid, ProtoError> {
        self.ensure(16)?;
        Ok(Uuid::from_u128(self.get_u128()))
    }

    /// Reads a block `Position`: a single i64 packing X (26 bits), Z (26 bits),
    /// and Y (12 bits), each two's-complement signed (the 1.14+ layout).
    fn read_position(&mut self) -> Result<(i32, i32, i32), ProtoError> {
        let value = self.read_i64()?;
        let x = (value >> 38) as i32;
        let y = ((value << 52) >> 52) as i32; // sign-extend low 12 bits
        let z = ((value << 26) >> 38) as i32; // sign-extend bits 12..=37
        Ok((x, y, z))
    }
}

impl<B: Buf + ?Sized> BufExt for B {}

/// Infallible writes of Minecraft protocol types.
pub trait BufMutExt: BufMut {
    fn put_bool(&mut self, value: bool) {
        self.put_u8(u8::from(value));
    }

    /// Writes a variable-length signed 32-bit integer.
    fn put_varint(&mut self, mut value: i32) {
        loop {
            let mut byte = (value & 0x7F) as u8;
            // logical (unsigned) shift so negatives terminate correctly
            value = ((value as u32) >> 7) as i32;
            if value != 0 {
                byte |= 0x80;
            }
            self.put_u8(byte);
            if value == 0 {
                break;
            }
        }
    }

    /// Writes a variable-length signed 64-bit integer.
    fn put_varlong(&mut self, mut value: i64) {
        loop {
            let mut byte = (value & 0x7F) as u8;
            value = ((value as u64) >> 7) as i64;
            if value != 0 {
                byte |= 0x80;
            }
            self.put_u8(byte);
            if value == 0 {
                break;
            }
        }
    }

    /// Writes a VarInt-length-prefixed UTF-8 string.
    fn put_string(&mut self, value: &str) {
        self.put_varint(value.len() as i32);
        self.put_slice(value.as_bytes());
    }

    /// Writes a VarInt-length-prefixed byte array.
    fn put_byte_array(&mut self, value: &[u8]) {
        self.put_varint(value.len() as i32);
        self.put_slice(value);
    }

    /// Writes a 128-bit UUID (big-endian).
    fn put_uuid(&mut self, value: Uuid) {
        self.put_u128(value.as_u128());
    }

    /// Writes a block `Position` (X:26, Z:26, Y:12 packed into one i64).
    fn put_position(&mut self, x: i32, y: i32, z: i32) {
        let value = (((x as i64) & 0x3FF_FFFF) << 38)
            | (((z as i64) & 0x3FF_FFFF) << 12)
            | ((y as i64) & 0xFFF);
        self.put_i64(value);
    }
}

impl<B: BufMut + ?Sized> BufMutExt for B {}

#[cfg(test)]
mod tests {
    use super::*;

    // Canonical VarInt test vectors from the protocol spec (wiki.vg "Data types").
    const VARINT_CASES: &[(i32, &[u8])] = &[
        (0, &[0x00]),
        (1, &[0x01]),
        (2, &[0x02]),
        (127, &[0x7f]),
        (128, &[0x80, 0x01]),
        (255, &[0xff, 0x01]),
        (25565, &[0xdd, 0xc7, 0x01]),
        (2097151, &[0xff, 0xff, 0x7f]),
        (2147483647, &[0xff, 0xff, 0xff, 0xff, 0x07]),
        (-1, &[0xff, 0xff, 0xff, 0xff, 0x0f]),
        (-2147483648, &[0x80, 0x80, 0x80, 0x80, 0x08]),
    ];

    // Canonical VarLong test vectors.
    const VARLONG_CASES: &[(i64, &[u8])] = &[
        (0, &[0x00]),
        (1, &[0x01]),
        (2, &[0x02]),
        (127, &[0x7f]),
        (128, &[0x80, 0x01]),
        (255, &[0xff, 0x01]),
        (2147483647, &[0xff, 0xff, 0xff, 0xff, 0x07]),
        (
            9223372036854775807,
            &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f],
        ),
        (
            -1,
            &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01],
        ),
        (
            -2147483648,
            &[0x80, 0x80, 0x80, 0x80, 0xf8, 0xff, 0xff, 0xff, 0xff, 0x01],
        ),
        (
            -9223372036854775808,
            &[0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x01],
        ),
    ];

    #[test]
    fn varint_encode_matches_spec() {
        for &(value, expected) in VARINT_CASES {
            let mut buf = Vec::new();
            buf.put_varint(value);
            assert_eq!(buf, expected, "encoding {value}");
            assert_eq!(varint_len(value), expected.len(), "varint_len({value})");
        }
    }

    #[test]
    fn varint_decode_matches_spec() {
        for &(value, mut bytes) in VARINT_CASES {
            let decoded = bytes.read_varint().unwrap();
            assert_eq!(decoded, value, "decoding {bytes:?}");
            assert_eq!(bytes.remaining(), 0, "leftover bytes after {value}");
        }
    }

    #[test]
    fn varlong_roundtrips_spec() {
        for &(value, expected) in VARLONG_CASES {
            let mut buf = Vec::new();
            buf.put_varlong(value);
            assert_eq!(buf, expected, "encoding {value}");

            let mut slice: &[u8] = expected;
            assert_eq!(slice.read_varlong().unwrap(), value, "decoding {value}");
            assert_eq!(slice.remaining(), 0);
        }
    }

    #[test]
    fn varint_rejects_overlong() {
        // Six continuation bytes -> too long.
        let mut bytes: &[u8] = &[0x80, 0x80, 0x80, 0x80, 0x80, 0x01];
        assert!(matches!(
            bytes.read_varint(),
            Err(ProtoError::VarIntTooLong)
        ));
    }

    #[test]
    fn varint_truncated_is_eof() {
        // Continuation bit set but no following byte.
        let mut bytes: &[u8] = &[0x80];
        assert!(matches!(
            bytes.read_varint(),
            Err(ProtoError::UnexpectedEof { .. })
        ));
    }

    #[test]
    fn string_roundtrips() {
        let original = "Crabcraft \u{1F980} こんにちは"; // ascii + emoji + japanese
        let mut buf = Vec::new();
        buf.put_string(original);

        let mut slice: &[u8] = &buf;
        let decoded = slice.read_string(64).unwrap();
        assert_eq!(decoded, original);
        assert_eq!(slice.remaining(), 0);
    }

    #[test]
    fn string_rejects_over_limit() {
        let mut buf = Vec::new();
        buf.put_string("waytoolong");
        let mut slice: &[u8] = &buf;
        assert!(matches!(
            slice.read_string(4),
            Err(ProtoError::StringTooLong { .. })
        ));
    }

    #[test]
    fn uuid_roundtrips() {
        let id = Uuid::from_u128(0x0123456789abcdef_fedcba9876543210);
        let mut buf = Vec::new();
        buf.put_uuid(id);
        assert_eq!(buf.len(), 16);

        let mut slice: &[u8] = &buf;
        assert_eq!(slice.read_uuid().unwrap(), id);
    }

    #[test]
    fn position_roundtrips() {
        let cases = [
            (0, 0, 0),
            (1, -61, 9),
            (-1, -1, -1),
            (-30_000_000, 200, 29_999_999),
            (30_000_000, -2048, -30_000_000),
        ];
        for (x, y, z) in cases {
            let mut buf = Vec::new();
            buf.put_position(x, y, z);
            let mut slice: &[u8] = &buf;
            assert_eq!(slice.read_position().unwrap(), (x, y, z), "pos {x},{y},{z}");
            assert_eq!(slice.remaining(), 0);
        }
    }

    #[test]
    fn byte_array_roundtrips() {
        let data = [9u8, 8, 7, 6, 5];
        let mut buf = Vec::new();
        buf.put_byte_array(&data);
        let mut slice: &[u8] = &buf;
        assert_eq!(slice.read_byte_array().unwrap(), data);
        assert_eq!(slice.remaining(), 0);
    }
}

//! Minimal NBT (Named Binary Tag) reader.
//!
//! Enough to parse the structures Crabcraft actually receives — chunk
//! heightmaps, the login registry codec, block entities, item tags — by fully
//! and correctly consuming an NBT blob so the surrounding packet stays aligned.
//!
//! This implements the **classic** wire form (a tag-type byte, then for the
//! root a name, then the payload). 1.20.2+ (protocol 764+) introduced a
//! "network" form that drops the root name; we'll add that alongside those
//! versions. 1.20.1 uses the classic form.
//!
//! Strings are technically Java "modified UTF-8"; we read them as standard
//! UTF-8, which is identical for all the ASCII keys we encounter in practice.

use std::collections::HashMap;

use bytes::Buf;

use crate::error::ProtoError;
use crate::io::BufExt;

/// Guards against hostile/corrupt deeply-nested tags blowing the stack.
const MAX_DEPTH: usize = 512;

/// A decoded NBT value.
#[derive(Clone, Debug, PartialEq)]
pub enum Nbt {
    /// An empty root (a lone `TAG_End`).
    End,
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<u8>),
    String(String),
    List(Vec<Nbt>),
    Compound(HashMap<String, Nbt>),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

impl Nbt {
    /// Borrow a named child if this is a compound.
    pub fn get(&self, key: &str) -> Option<&Nbt> {
        match self {
            Nbt::Compound(map) => map.get(key),
            _ => None,
        }
    }

    /// Borrow a `LongArray` payload (heightmaps, etc.) if that's what this is.
    pub fn as_long_array(&self) -> Option<&[i64]> {
        match self {
            Nbt::LongArray(v) => Some(v),
            _ => None,
        }
    }
}

/// Reads a complete classic-form NBT value (tag byte, root name, payload).
pub fn read_nbt<B: Buf>(buf: &mut B) -> Result<Nbt, ProtoError> {
    let tag = buf.read_u8()?;
    if tag == 0 {
        return Ok(Nbt::End);
    }
    // The root carries a (usually empty) name we don't need to keep.
    let _root_name = read_nbt_string(buf)?;
    read_payload(buf, tag, 0)
}

/// Reads the unnamed network-NBT form used by protocol 764+ registry data.
/// Unlike classic NBT, the root tag has no following root-name string.
pub fn read_anonymous_nbt<B: Buf>(buf: &mut B) -> Result<Nbt, ProtoError> {
    let tag = buf.read_u8()?;
    if tag == 0 {
        return Ok(Nbt::End);
    }
    read_payload(buf, tag, 0)
}

fn read_nbt_string<B: Buf>(buf: &mut B) -> Result<String, ProtoError> {
    let len = buf.read_u16()? as usize;
    let bytes = buf.read_bytes(len)?;
    Ok(String::from_utf8(bytes)?)
}

fn read_payload<B: Buf>(buf: &mut B, tag: u8, depth: usize) -> Result<Nbt, ProtoError> {
    if depth > MAX_DEPTH {
        return Err(ProtoError::NbtTooDeep);
    }
    Ok(match tag {
        1 => Nbt::Byte(buf.read_i8()?),
        2 => Nbt::Short(buf.read_i16()?),
        3 => Nbt::Int(buf.read_i32()?),
        4 => Nbt::Long(buf.read_i64()?),
        5 => Nbt::Float(buf.read_f32()?),
        6 => Nbt::Double(buf.read_f64()?),
        7 => {
            let len = buf.read_i32()?.max(0) as usize;
            Nbt::ByteArray(buf.read_bytes(len)?)
        }
        8 => Nbt::String(read_nbt_string(buf)?),
        9 => {
            let elem = buf.read_u8()?;
            let len = buf.read_i32()?.max(0) as usize;
            // An empty list may declare element type TAG_End; that's fine.
            let mut items = Vec::with_capacity(len.min(1024));
            for _ in 0..len {
                items.push(read_payload(buf, elem, depth + 1)?);
            }
            Nbt::List(items)
        }
        10 => {
            let mut map = HashMap::new();
            loop {
                let child = buf.read_u8()?;
                if child == 0 {
                    break;
                }
                let name = read_nbt_string(buf)?;
                let value = read_payload(buf, child, depth + 1)?;
                map.insert(name, value);
            }
            Nbt::Compound(map)
        }
        11 => {
            let len = buf.read_i32()?.max(0) as usize;
            let mut v = Vec::with_capacity(len.min(4096));
            for _ in 0..len {
                v.push(buf.read_i32()?);
            }
            Nbt::IntArray(v)
        }
        12 => {
            let len = buf.read_i32()?.max(0) as usize;
            let mut v = Vec::with_capacity(len.min(4096));
            for _ in 0..len {
                v.push(buf.read_i64()?);
            }
            Nbt::LongArray(v)
        }
        other => return Err(ProtoError::NbtTag(other)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_compound() {
        // Compound("") { Int("x", 5), String("name", "hi"), End }
        let bytes: &[u8] = &[
            0x0a, // TAG_Compound
            0x00, 0x00, // root name length 0
            0x03, 0x00, 0x01, b'x', 0x00, 0x00, 0x00, 0x05, // Int x = 5
            0x08, 0x00, 0x04, b'n', b'a', b'm', b'e', 0x00, 0x02, b'h',
            b'i', // String name = "hi"
            0x00, // TAG_End
        ];
        let mut cur = bytes;
        let nbt = read_nbt(&mut cur).unwrap();
        assert_eq!(nbt.get("x"), Some(&Nbt::Int(5)));
        assert_eq!(nbt.get("name"), Some(&Nbt::String("hi".to_string())));
        assert_eq!(cur.len(), 0, "must consume the whole blob");
    }

    #[test]
    fn parses_long_array() {
        // Compound("") { LongArray("hm", [1, 2]), End }
        let bytes: &[u8] = &[
            0x0a, 0x00, 0x00, // compound, empty name
            0x0c, 0x00, 0x02, b'h', b'm', // LongArray "hm"
            0x00, 0x00, 0x00, 0x02, // length 2
            0, 0, 0, 0, 0, 0, 0, 1, // 1
            0, 0, 0, 0, 0, 0, 0, 2,    // 2
            0x00, // end
        ];
        let mut cur = bytes;
        let nbt = read_nbt(&mut cur).unwrap();
        assert_eq!(
            nbt.get("hm").and_then(Nbt::as_long_array),
            Some(&[1i64, 2][..])
        );
        assert_eq!(cur.len(), 0);
    }

    #[test]
    fn empty_root_is_end() {
        let mut cur: &[u8] = &[0x00];
        assert_eq!(read_nbt(&mut cur).unwrap(), Nbt::End);
    }

    #[test]
    fn anonymous_root_omits_name() {
        let mut input: &[u8] = &[10, 3, 0, 1, b'x', 0, 0, 0, 5, 0];
        let nbt = read_anonymous_nbt(&mut input).unwrap();
        assert_eq!(nbt.get("x"), Some(&Nbt::Int(5)));
        assert!(input.is_empty());
    }
}

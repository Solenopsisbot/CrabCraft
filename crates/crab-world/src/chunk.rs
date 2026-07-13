//! Chunk column decoding: the post-1.18 paletted-container format.
//!
//! A chunk column's `chunkData` buffer is simply every section back-to-back. We
//! decode sections until the buffer is exhausted, which conveniently means we
//! don't need to know the dimension height up front.
//!
//! Each section is:
//! ```text
//! Block count : i16
//! Block states: paletted container over 4096 entries (single / indirect / direct)
//! Biomes      : paletted container over 64 entries
//! ```
//! A paletted container is `bitsPerEntry: u8`, then a palette (depending on
//! `bitsPerEntry`), then a `i64` data array that bit-packs the entries (entries
//! never straddle a long in 1.16+).

use bytes::Buf;

use crab_protocol::nbt;
use crab_protocol::{BufExt, ProtoError};

/// Blocks per section (16 x 16 x 16).
pub const SECTION_VOLUME: usize = 4096;
/// Biome cells per section (4 x 4 x 4).
pub const SECTION_BIOMES: usize = 64;

/// Block states for one 16^3 section.
///
/// Sections that are a single block (almost always air) are stored compactly as
/// [`BlockStates::Uniform`]; mixed sections are fully expanded.
#[derive(Clone, Debug)]
pub enum BlockStates {
    Uniform(u32),
    Array(Box<[u32; SECTION_VOLUME]>),
}

/// Biome ids for the 4×4×4 quart-resolution cells in a section.
#[derive(Clone, Debug)]
pub enum Biomes {
    Uniform(u32),
    Array(Box<[u32; SECTION_BIOMES]>),
}

/// A single 16^3 chunk section.
#[derive(Clone, Debug)]
pub struct Section {
    /// Server-reported count of non-air blocks (used to skip empty sections).
    pub block_count: i16,
    pub blocks: BlockStates,
    pub biomes: Biomes,
}

impl Section {
    /// Block-state index for in-section coordinates (each in `0..16`).
    /// Layout is Y-major, then Z, then X.
    #[inline]
    fn index(x: usize, y: usize, z: usize) -> usize {
        (y << 8) | (z << 4) | x
    }

    /// Whether this section is uniformly air (nothing to mesh).
    pub fn is_air_only(&self) -> bool {
        matches!(self.blocks, BlockStates::Uniform(0))
    }

    /// Block state at in-section coordinates (each in `0..16`).
    pub fn block_state(&self, x: usize, y: usize, z: usize) -> u32 {
        match &self.blocks {
            BlockStates::Uniform(v) => *v,
            BlockStates::Array(a) => a[Self::index(x, y, z)],
        }
    }

    pub fn biome(&self, x: usize, y: usize, z: usize) -> u32 {
        let index = ((y >> 2) << 4) | ((z >> 2) << 2) | (x >> 2);
        match &self.biomes {
            Biomes::Uniform(value) => *value,
            Biomes::Array(values) => values[index],
        }
    }

    /// Sets a block state, promoting a uniform section to a full array if the
    /// new value differs.
    pub fn set_block_state(&mut self, x: usize, y: usize, z: usize, state: u32) {
        let idx = Self::index(x, y, z);
        match &mut self.blocks {
            BlockStates::Uniform(v) if *v == state => {}
            BlockStates::Uniform(v) => {
                let mut arr = Box::new([*v; SECTION_VOLUME]);
                arr[idx] = state;
                self.blocks = BlockStates::Array(arr);
            }
            BlockStates::Array(a) => a[idx] = state,
        }
    }
}

/// A full chunk column at `(x, z)` (in chunk coordinates), bottom section first.
#[derive(Clone, Debug)]
pub struct Chunk {
    pub x: i32,
    pub z: i32,
    pub sections: Vec<Section>,
}

impl Chunk {
    /// Decodes a chunk column from the body of a `map_chunk` (0x24) packet.
    ///
    /// `section_count` is the dimension's height in sections (24 for the 1.20.1
    /// overworld). We read **exactly** that many sections and ignore any
    /// trailing bytes in `chunkData` — exactly what the vanilla client does.
    /// Parsing "until the buffer is empty" is wrong: the buffer can carry
    /// trailing padding that would be misread as extra (corrupt) sections.
    pub fn parse<B: Buf>(buf: &mut B, section_count: usize) -> Result<Self, ProtoError> {
        Self::parse_with_nbt(buf, section_count, false)
    }

    /// Decodes the 1.20.2+ packet form, whose heightmap uses unnamed network
    /// NBT rather than classic named-root NBT.
    pub fn parse_network<B: Buf>(buf: &mut B, section_count: usize) -> Result<Self, ProtoError> {
        Self::parse_with_nbt(buf, section_count, true)
    }

    fn parse_with_nbt<B: Buf>(
        buf: &mut B,
        section_count: usize,
        anonymous_nbt: bool,
    ) -> Result<Self, ProtoError> {
        let x = buf.read_i32()?;
        let z = buf.read_i32()?;
        // Heightmaps NBT — read it purely to stay aligned with the buffer.
        let _heightmaps = if anonymous_nbt {
            nbt::read_anonymous_nbt(buf)?
        } else {
            nbt::read_nbt(buf)?
        };

        let data = buf.read_byte_array()?;
        let mut cursor: &[u8] = &data;
        let mut sections = Vec::with_capacity(section_count);
        for _ in 0..section_count {
            sections.push(parse_section(&mut cursor)?);
        }

        Ok(Chunk { x, z, sections })
    }
}

fn parse_section<B: Buf>(buf: &mut B) -> Result<Section, ProtoError> {
    let block_count = buf.read_i16()?;
    let blocks = match read_paletted(buf, SECTION_VOLUME, 8)? {
        Paletted::Single(v) => BlockStates::Uniform(v),
        Paletted::Values(values) => {
            let arr: Box<[u32; SECTION_VOLUME]> = values
                .into_boxed_slice()
                .try_into()
                .map_err(|_| ProtoError::UnexpectedEof { needed: 0 })?;
            BlockStates::Array(arr)
        }
    };
    // Biomes share the format (over 64 entries, indirect up to 3 bits). We must
    // decode them to advance the cursor, but don't retain them yet.
    let biomes = match read_paletted(buf, SECTION_BIOMES, 3)? {
        Paletted::Single(value) => Biomes::Uniform(value),
        Paletted::Values(values) => Biomes::Array(
            values
                .into_boxed_slice()
                .try_into()
                .map_err(|_| ProtoError::UnexpectedEof { needed: 0 })?,
        ),
    };
    Ok(Section {
        block_count,
        blocks,
        biomes,
    })
}

/// Result of decoding a paletted container.
enum Paletted {
    /// Every entry is this value (bitsPerEntry == 0).
    Single(u32),
    /// One value per entry.
    Values(Vec<u32>),
}

/// Decodes a paletted container of `entries` values.
///
/// `max_indirect_bits` is the largest `bitsPerEntry` that still uses an indirect
/// palette (8 for block states, 3 for biomes); anything larger is a direct
/// palette where the packed values are already global IDs.
fn read_paletted<B: Buf>(
    buf: &mut B,
    entries: usize,
    max_indirect_bits: u8,
) -> Result<Paletted, ProtoError> {
    let bits = buf.read_u8()?;

    if bits == 0 {
        // Single-valued palette: one value, then a (zero-length) data array.
        let value = buf.read_varint()? as u32;
        let data_len = buf.read_varint()?.max(0);
        for _ in 0..data_len {
            let _ = buf.read_i64()?;
        }
        return Ok(Paletted::Single(value));
    }

    let palette = if bits <= max_indirect_bits {
        let len = buf.read_varint()?.max(0) as usize;
        let mut palette = Vec::with_capacity(len.min(4096));
        for _ in 0..len {
            palette.push(buf.read_varint()? as u32);
        }
        Some(palette)
    } else {
        None // direct palette: packed values are global IDs
    };

    let data_len = buf.read_varint()?.max(0) as usize;
    let mut longs = Vec::with_capacity(data_len.min(1024));
    for _ in 0..data_len {
        longs.push(buf.read_i64()? as u64);
    }

    let bits = bits as usize;
    let entries_per_long = 64 / bits;
    let mask = (1u64 << bits) - 1;

    let mut out = Vec::with_capacity(entries);
    for i in 0..entries {
        let long_index = i / entries_per_long;
        let offset = (i % entries_per_long) * bits;
        let raw = longs
            .get(long_index)
            .map(|l| (l >> offset) & mask)
            .unwrap_or(0);
        let value = match &palette {
            Some(p) => p.get(raw as usize).copied().unwrap_or(0),
            None => raw as u32,
        };
        out.push(value);
    }
    Ok(Paletted::Values(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BufMut;
    use crab_protocol::BufMutExt;

    #[test]
    fn single_valued_palette_is_uniform() {
        // bits=0, value=0 (air), data length 0.
        let mut bytes = Vec::new();
        bytes.push(0u8); // bits
        bytes.put_varint(0); // value = air
        bytes.put_varint(0); // data array length
        let mut cur: &[u8] = &bytes;
        match read_paletted(&mut cur, SECTION_VOLUME, 8).unwrap() {
            Paletted::Single(v) => assert_eq!(v, 0),
            Paletted::Values(_) => panic!("expected single-valued"),
        }
        assert_eq!(cur.len(), 0);
    }

    #[test]
    fn indirect_palette_unpacks_entries() {
        // bits=4, palette=[100,200], one long packing 4 entries: idx 0,1,0,1.
        let mut bytes = Vec::new();
        bytes.push(4u8); // bits
        bytes.put_varint(2); // palette length
        bytes.put_varint(100); // palette[0]
        bytes.put_varint(200); // palette[1]
        bytes.put_varint(1); // one long
                             // entries (4 bits each, low entry first): 0, 1, 0, 1
        let packed: u64 = (1u64 << 4) | (1u64 << 12);
        bytes.put_i64(packed as i64);

        let mut cur: &[u8] = &bytes;
        // decode just 4 entries for the test
        match read_paletted(&mut cur, 4, 8).unwrap() {
            Paletted::Values(v) => assert_eq!(v, vec![100, 200, 100, 200]),
            Paletted::Single(_) => panic!("expected values"),
        }
        assert_eq!(cur.len(), 0);
    }

    #[test]
    fn direct_palette_uses_raw_values() {
        // bits=9 (> 8) => direct; two entries 0x0AB and 0x1CD in one long.
        let mut bytes = Vec::new();
        bytes.push(9u8);
        bytes.put_varint(1); // one long
        let a: u64 = 0x0AB;
        let b: u64 = 0x1CD;
        let packed: u64 = a | (b << 9);
        bytes.put_i64(packed as i64);

        let mut cur: &[u8] = &bytes;
        match read_paletted(&mut cur, 2, 8).unwrap() {
            Paletted::Values(v) => assert_eq!(v, vec![0x0AB, 0x1CD]),
            Paletted::Single(_) => panic!("expected values"),
        }
    }

    #[test]
    fn section_indexing_is_y_z_x() {
        assert_eq!(Section::index(0, 0, 0), 0);
        assert_eq!(Section::index(1, 0, 0), 1);
        assert_eq!(Section::index(0, 0, 1), 16);
        assert_eq!(Section::index(0, 1, 0), 256);
        assert_eq!(Section::index(15, 15, 15), 4095);
    }

    #[test]
    fn uniform_promotes_to_array_on_write() {
        let mut s = Section {
            block_count: 0,
            blocks: BlockStates::Uniform(0),
            biomes: Biomes::Uniform(0),
        };
        s.set_block_state(1, 2, 3, 42);
        assert_eq!(s.block_state(1, 2, 3), 42);
        assert_eq!(s.block_state(0, 0, 0), 0);
        assert!(matches!(s.blocks, BlockStates::Array(_)));
    }
}

//! The loaded-chunk store and block queries.

use std::collections::HashMap;

use crab_protocol::nbt::Nbt;

use crate::chunk::Chunk;

/// Extracts `(min_y, height)` for `dimension_type` (e.g. `"minecraft:overworld"`)
/// from the dimension codec NBT carried by the Join Game packet.
///
/// Returns `None` if the codec doesn't contain that dimension or the expected
/// fields, letting the caller fall back to a default.
pub fn dimension_extent(codec: &Nbt, dimension_type: &str) -> Option<(i32, i32)> {
    let entries = match codec.get("minecraft:dimension_type")?.get("value")? {
        Nbt::List(entries) => entries,
        _ => return None,
    };
    for entry in entries {
        if let Some(Nbt::String(name)) = entry.get("name") {
            if name == dimension_type {
                let element = entry.get("element")?;
                let Nbt::Int(min_y) = element.get("min_y")? else {
                    return None;
                };
                let Nbt::Int(height) = element.get("height")? else {
                    return None;
                };
                return Some((*min_y, *height));
            }
        }
    }
    None
}

/// A collection of loaded chunk columns supporting `(x, y, z)` block queries.
///
/// `min_y` / `height` describe the vertical extent of the current dimension.
/// They default to the 1.20.1 overworld (`-64..320`); once we parse the
/// dimension registry from the Login packet we'll set these per-dimension.
#[derive(Debug)]
pub struct World {
    pub min_y: i32,
    pub height: i32,
    chunks: HashMap<(i32, i32), Chunk>,
}

impl Default for World {
    fn default() -> Self {
        Self::overworld()
    }
}

impl World {
    /// A world with the given vertical extent.
    pub fn new(min_y: i32, height: i32) -> Self {
        Self {
            min_y,
            height,
            chunks: HashMap::new(),
        }
    }

    /// A world sized for the 1.20.1 overworld (`-64..=319`).
    pub fn overworld() -> Self {
        Self::new(-64, 384)
    }

    /// Inserts/replaces a chunk column.
    pub fn load_chunk(&mut self, chunk: Chunk) {
        self.chunks.insert((chunk.x, chunk.z), chunk);
    }

    /// Drops a chunk column.
    pub fn unload_chunk(&mut self, x: i32, z: i32) {
        self.chunks.remove(&(x, z));
    }

    /// Number of currently-loaded chunk columns.
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// Number of 16-block sections in this dimension's height (24 for the
    /// overworld). This is exactly how many sections a `map_chunk` carries.
    pub fn section_count(&self) -> usize {
        (self.height / 16) as usize
    }

    /// Whether the chunk containing world coordinate `(x, z)` is loaded.
    pub fn is_loaded(&self, x: i32, z: i32) -> bool {
        self.chunks.contains_key(&(x >> 4, z >> 4))
    }

    /// Block state at world coordinates, or `None` if out of range or the chunk
    /// isn't loaded.
    pub fn block_state(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        if y < self.min_y || y >= self.min_y + self.height {
            return None;
        }
        let chunk = self.chunks.get(&(x >> 4, z >> 4))?;
        let section_index = ((y - self.min_y) >> 4) as usize;
        let section = chunk.sections.get(section_index)?;
        let lx = (x & 15) as usize;
        let lz = (z & 15) as usize;
        let ly = ((y - self.min_y) & 15) as usize;
        Some(section.block_state(lx, ly, lz))
    }

    /// Updates a single block (e.g. from a `block_change` packet). No-op if the
    /// chunk/section isn't present.
    pub fn set_block_state(&mut self, x: i32, y: i32, z: i32, state: u32) {
        if y < self.min_y || y >= self.min_y + self.height {
            return;
        }
        if let Some(chunk) = self.chunks.get_mut(&(x >> 4, z >> 4)) {
            let section_index = ((y - self.min_y) >> 4) as usize;
            if let Some(section) = chunk.sections.get_mut(section_index) {
                let lx = (x & 15) as usize;
                let lz = (z & 15) as usize;
                let ly = ((y - self.min_y) & 15) as usize;
                section.set_block_state(lx, ly, lz, state);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{BlockStates, Section};

    fn air_section() -> Section {
        Section {
            block_count: 0,
            blocks: BlockStates::Uniform(0),
        }
    }

    #[test]
    fn queries_handle_negative_coords_and_sections() {
        // 24 sections of air, bottom at y=-64.
        let sections: Vec<Section> = (0..24).map(|_| air_section()).collect();
        let mut world = World::overworld();
        world.load_chunk(Chunk {
            x: -1,
            z: -1,
            sections,
        });

        // A block in chunk (-1,-1): world (-3, -61, -7).
        assert_eq!(world.block_state(-3, -61, -7), Some(0));
        // Place a block and read it back.
        world.set_block_state(-3, -61, -7, 1234);
        assert_eq!(world.block_state(-3, -61, -7), Some(1234));
        // Neighbour untouched.
        assert_eq!(world.block_state(-2, -61, -7), Some(0));

        // Out of vertical range.
        assert_eq!(world.block_state(-3, 9000, -7), None);
        // Unloaded chunk.
        assert_eq!(world.block_state(9999, 0, 9999), None);
    }

    #[test]
    fn set_on_uniform_section_promotes_only_that_chunk() {
        let sections: Vec<Section> = (0..24).map(|_| air_section()).collect();
        let mut world = World::overworld();
        world.load_chunk(Chunk {
            x: 0,
            z: 0,
            sections,
        });
        world.set_block_state(8, -60, 8, 5);
        assert_eq!(world.block_state(8, -60, 8), Some(5));
        assert_eq!(world.block_state(8, -59, 8), Some(0));
    }
}

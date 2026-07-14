//! The loaded-chunk store and block queries.

use std::collections::HashMap;
use std::ops::Deref;
use std::sync::Arc;

use crab_protocol::nbt::Nbt;

use crate::chunk::Chunk;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BiomeColors {
    pub grass: [f32; 3],
    pub foliage: [f32; 3],
    pub water: [f32; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TintKind {
    Grass,
    Foliage,
    Water,
}

fn rgb(value: i32) -> [f32; 3] {
    let value = value as u32;
    [
        ((value >> 16) & 0xff) as f32 / 255.0,
        ((value >> 8) & 0xff) as f32 / 255.0,
        (value & 0xff) as f32 / 255.0,
    ]
}

fn climate_color(temperature: f32, downfall: f32, foliage: bool) -> [f32; 3] {
    let temperature = temperature.clamp(0.0, 1.0);
    let humidity = (downfall * temperature).clamp(0.0, 1.0);
    let dry = if foliage {
        [0.72, 0.70, 0.28]
    } else {
        [0.75, 0.72, 0.33]
    };
    let cool = if foliage {
        [0.32, 0.65, 0.30]
    } else {
        [0.40, 0.72, 0.34]
    };
    let wet = if foliage {
        [0.15, 0.72, 0.25]
    } else {
        [0.25, 0.78, 0.30]
    };
    std::array::from_fn(|index| {
        let temperate = dry[index] + (cool[index] - dry[index]) * temperature;
        temperate + (wet[index] - temperate) * humidity
    })
}

/// Reads biome ids and their effect/climate colors from the Join Game registry
/// codec. Explicit datapack colors win; otherwise grass and foliage are
/// generated from the biome's temperature/downfall climate.
pub fn biome_colors(codec: &Nbt) -> HashMap<u32, BiomeColors> {
    let entries = match codec
        .get("minecraft:worldgen/biome")
        .and_then(|tag| tag.get("value"))
    {
        Some(Nbt::List(entries)) => entries,
        _ => return HashMap::new(),
    };
    let mut colors = HashMap::new();
    for entry in entries {
        let Some(Nbt::Int(id)) = entry.get("id") else {
            continue;
        };
        let Some(element) = entry.get("element") else {
            continue;
        };
        let temperature = match element.get("temperature") {
            Some(Nbt::Float(value)) => *value,
            _ => 0.5,
        };
        let downfall = match element.get("downfall") {
            Some(Nbt::Float(value)) => *value,
            _ => 0.5,
        };
        let effects = element.get("effects");
        let explicit = |key| match effects.and_then(|effects| effects.get(key)) {
            Some(Nbt::Int(value)) => Some(rgb(*value)),
            _ => None,
        };
        colors.insert(
            *id as u32,
            BiomeColors {
                grass: explicit("grass_color")
                    .unwrap_or_else(|| climate_color(temperature, downfall, false)),
                foliage: explicit("foliage_color")
                    .unwrap_or_else(|| climate_color(temperature, downfall, true)),
                water: explicit("water_color").unwrap_or_else(|| rgb(0x3f76e4)),
            },
        );
    }
    colors
}

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
#[derive(Clone, Debug)]
pub struct World {
    pub min_y: i32,
    pub height: i32,
    chunks: HashMap<(i32, i32), Arc<Chunk>>,
    chunk_revisions: HashMap<(i32, i32), u64>,
    biome_colors: Arc<HashMap<u32, BiomeColors>>,
    revision: u64,
}

/// Cheap immutable view captured for background queries such as meshing.
/// Chunk payloads are structurally shared and dependency revisions allow stale
/// work to be rejected before it reaches presentation.
#[derive(Clone, Debug)]
pub struct WorldSnapshot {
    world: World,
    dependencies: Vec<((i32, i32), Option<u64>)>,
}

impl WorldSnapshot {
    #[must_use]
    pub fn is_current(&self, world: &World) -> bool {
        self.dependencies
            .iter()
            .all(|(coord, revision)| world.chunk_revision(*coord) == *revision)
    }

    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.world.revision
    }
}

impl Deref for WorldSnapshot {
    type Target = World;

    fn deref(&self) -> &Self::Target {
        &self.world
    }
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
            chunk_revisions: HashMap::new(),
            biome_colors: Arc::new(HashMap::new()),
            revision: 0,
        }
    }

    /// A world sized for the 1.20.1 overworld (`-64..=319`).
    pub fn overworld() -> Self {
        Self::new(-64, 384)
    }

    /// Inserts/replaces a chunk column.
    pub fn load_chunk(&mut self, chunk: Chunk) {
        let coord = (chunk.x, chunk.z);
        let revision = self.next_revision();
        self.chunks.insert(coord, Arc::new(chunk));
        self.chunk_revisions.insert(coord, revision);
    }

    /// Drops a chunk column.
    pub fn unload_chunk(&mut self, x: i32, z: i32) {
        self.chunks.remove(&(x, z));
        self.chunk_revisions.remove(&(x, z));
        self.next_revision();
    }

    /// Number of currently-loaded chunk columns.
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// Coordinates of every currently loaded chunk column. This is used by
    /// renderer-wide invalidations such as a live resource-pack reload.
    pub fn chunk_coords(&self) -> impl Iterator<Item = (i32, i32)> + '_ {
        self.chunks.keys().copied()
    }

    pub fn set_biome_colors(&mut self, colors: HashMap<u32, BiomeColors>) {
        self.biome_colors = Arc::new(colors);
        self.next_revision();
    }

    /// Monotonic revision for any world mutation.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Current revision of one loaded chunk.
    #[must_use]
    pub fn chunk_revision(&self, coord: (i32, i32)) -> Option<u64> {
        self.chunk_revisions.get(&coord).copied()
    }

    /// Captures the target chunk and its eight horizontal neighbours. The
    /// resulting snapshot can be queried and meshed without holding a world
    /// lock, while preserving border face culling and biome smoothing.
    #[must_use]
    pub fn snapshot_region(&self, cx: i32, cz: i32) -> WorldSnapshot {
        let mut chunks = HashMap::with_capacity(9);
        let mut revisions = HashMap::with_capacity(9);
        let mut dependencies = Vec::with_capacity(9);
        for x in cx - 1..=cx + 1 {
            for z in cz - 1..=cz + 1 {
                let coord = (x, z);
                let revision = self.chunk_revision(coord);
                dependencies.push((coord, revision));
                if let Some(chunk) = self.chunks.get(&coord) {
                    chunks.insert(coord, Arc::clone(chunk));
                }
                if let Some(revision) = revision {
                    revisions.insert(coord, revision);
                }
            }
        }
        WorldSnapshot {
            world: World {
                min_y: self.min_y,
                height: self.height,
                chunks,
                chunk_revisions: revisions,
                biome_colors: Arc::clone(&self.biome_colors),
                revision: self.revision,
            },
            dependencies,
        }
    }

    fn next_revision(&mut self) -> u64 {
        self.revision = self.revision.saturating_add(1);
        self.revision
    }

    pub fn biome_id(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        if y < self.min_y || y >= self.min_y + self.height {
            return None;
        }
        let chunk = self.chunks.get(&(x >> 4, z >> 4))?;
        let section = chunk.sections.get(((y - self.min_y) >> 4) as usize)?;
        Some(section.biome(
            (x & 15) as usize,
            ((y - self.min_y) & 15) as usize,
            (z & 15) as usize,
        ))
    }

    /// Vanilla-style 3×3 horizontal smoothing of the biome color around a
    /// block. Missing neighboring chunks use the center biome/fallback color.
    pub fn biome_tint(&self, x: i32, y: i32, z: i32, kind: TintKind) -> [f32; 3] {
        let fallback = match kind {
            TintKind::Grass => [0.45, 0.70, 0.33],
            TintKind::Foliage => [0.40, 0.68, 0.30],
            TintKind::Water => rgb(0x3f76e4),
        };
        let center = self
            .biome_id(x, y, z)
            .and_then(|id| self.biome_colors.get(&id).copied());
        let mut sum = [0.0; 3];
        let mut count = 0.0;
        for dx in -1..=1 {
            for dz in -1..=1 {
                let colors = self
                    .biome_id(x + dx, y, z + dz)
                    .and_then(|id| self.biome_colors.get(&id).copied())
                    .or(center);
                if let Some(colors) = colors {
                    let color = match kind {
                        TintKind::Grass => colors.grass,
                        TintKind::Foliage => colors.foliage,
                        TintKind::Water => colors.water,
                    };
                    for index in 0..3 {
                        sum[index] += color[index];
                    }
                    count += 1.0;
                }
            }
        }
        if count == 0.0 {
            fallback
        } else {
            std::array::from_fn(|index| sum[index] / count)
        }
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

    /// The inclusive world-Y range of a chunk column that actually contains
    /// blocks (skipping air-only sections), or `None` if empty/unloaded. Lets
    /// the mesher avoid scanning the whole 384-block height.
    pub fn occupied_y_bounds(&self, cx: i32, cz: i32) -> Option<(i32, i32)> {
        let chunk = self.chunks.get(&(cx, cz))?;
        let mut min_i: Option<usize> = None;
        let mut max_i = 0usize;
        for (i, section) in chunk.sections.iter().enumerate() {
            if !section.is_air_only() {
                min_i.get_or_insert(i);
                max_i = i;
            }
        }
        let min_i = min_i?;
        Some((
            self.min_y + (min_i as i32) * 16,
            self.min_y + (max_i as i32) * 16 + 15,
        ))
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
            let coord = (x >> 4, z >> 4);
            let section_index = ((y - self.min_y) >> 4) as usize;
            if let Some(section) = Arc::make_mut(chunk).sections.get_mut(section_index) {
                let lx = (x & 15) as usize;
                let lz = (z & 15) as usize;
                let ly = ((y - self.min_y) & 15) as usize;
                section.set_block_state(lx, ly, lz, state);
                let revision = self.next_revision();
                self.chunk_revisions.insert(coord, revision);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{Biomes, BlockStates, Section};

    fn air_section() -> Section {
        Section {
            block_count: 0,
            blocks: BlockStates::Uniform(0),
            biomes: Biomes::Uniform(0),
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

    #[test]
    fn region_snapshots_are_immutable_and_revision_checked() {
        let sections: Vec<Section> = (0..24).map(|_| air_section()).collect();
        let mut world = World::overworld();
        world.load_chunk(Chunk {
            x: 0,
            z: 0,
            sections,
        });
        let snapshot = world.snapshot_region(0, 0);
        assert!(snapshot.is_current(&world));
        assert_eq!(snapshot.block_state(8, -60, 8), Some(0));

        world.set_block_state(8, -60, 8, 5);
        assert!(!snapshot.is_current(&world));
        assert_eq!(snapshot.block_state(8, -60, 8), Some(0));
        assert_eq!(world.block_state(8, -60, 8), Some(5));
    }

    #[test]
    fn biome_registry_colors_and_quart_cells_drive_smoothed_tints() {
        let codec = Nbt::Compound(HashMap::from([(
            "minecraft:worldgen/biome".to_string(),
            Nbt::Compound(HashMap::from([(
                "value".to_string(),
                Nbt::List(vec![Nbt::Compound(HashMap::from([
                    ("id".to_string(), Nbt::Int(7)),
                    (
                        "element".to_string(),
                        Nbt::Compound(HashMap::from([
                            ("temperature".to_string(), Nbt::Float(0.8)),
                            ("downfall".to_string(), Nbt::Float(0.4)),
                            (
                                "effects".to_string(),
                                Nbt::Compound(HashMap::from([
                                    ("grass_color".to_string(), Nbt::Int(0x12_34_56)),
                                    ("water_color".to_string(), Nbt::Int(0x01_02_03)),
                                ])),
                            ),
                        ])),
                    ),
                ]))]),
            )])),
        )]));
        let colors = biome_colors(&codec);
        assert_eq!(colors[&7].grass, rgb(0x12_34_56));
        assert_eq!(colors[&7].water, rgb(0x01_02_03));

        let mut world = World::overworld();
        world.set_biome_colors(colors);
        world.load_chunk(Chunk {
            x: 0,
            z: 0,
            sections: (0..24)
                .map(|_| Section {
                    block_count: 0,
                    blocks: BlockStates::Uniform(0),
                    biomes: Biomes::Uniform(7),
                })
                .collect(),
        });
        assert_eq!(world.biome_id(4, 64, 4), Some(7));
        for (actual, expected) in world
            .biome_tint(4, 64, 4, TintKind::Grass)
            .into_iter()
            .zip(rgb(0x12_34_56))
        {
            assert!((actual - expected).abs() < 1e-6);
        }
        for (actual, expected) in world
            .biome_tint(4, 64, 4, TintKind::Water)
            .into_iter()
            .zip(rgb(0x01_02_03))
        {
            assert!((actual - expected).abs() < 1e-6);
        }
    }
}

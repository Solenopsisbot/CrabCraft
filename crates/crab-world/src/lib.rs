//! # crab-world
//!
//! The in-memory world: decodes chunk columns off the wire and answers block
//! queries. Rendering and pathfinding will both sit on top of this later.
//!
//! ## What lives here
//! * [`chunk`] – [`Chunk`] / [`Section`] plus the paletted-container + bit-packed
//!   long-array decoding that turns the opaque `chunkData` buffer into block
//!   states.
//! * [`world`] – [`World`], a map of loaded chunks with `(x,y,z)` block queries.
//!
//! Block **state IDs** are kept as raw `u32`s from the global palette. Mapping
//! those to `minecraft:block_name` needs the block registry (a later,
//! data-driven milestone), so for now we deal in numeric state IDs.

pub mod chunk;
pub mod world;

pub use chunk::{Biomes, BlockStates, Chunk, Section, SECTION_BIOMES, SECTION_VOLUME};
pub use world::{biome_colors, dimension_extent, BiomeColors, TintKind, World};

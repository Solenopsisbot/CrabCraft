//! # crab-world
//!
//! The in-memory world: decodes chunk columns off the wire, answers block
//! queries, and creates immutable revision-stamped snapshots for workers.
//!
//! ## What lives here
//! * [`chunk`] – [`Chunk`] / [`Section`] plus the paletted-container + bit-packed
//!   long-array decoding that turns the opaque `chunkData` buffer into block
//!   states.
//! * [`world`] – [`World`], a copy-on-write map of loaded chunks with block
//!   queries and structurally shared [`WorldSnapshot`] regions.
//!
//! Block **state IDs** remain raw `u32`s in storage. Callers interpret them with
//! the immutable [`crab_registry::RegistrySet`] from their session context.

pub mod chunk;
pub mod world;

pub use chunk::{Biomes, BlockStates, Chunk, Section, SECTION_BIOMES, SECTION_VOLUME};
pub use world::{biome_colors, dimension_extent, BiomeColors, TintKind, World, WorldSnapshot};

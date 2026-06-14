//! # crab-registry
//!
//! Data-driven game registries. Block-state IDs travel on the wire as opaque
//! numbers; this crate maps them back to names like `minecraft:grass_block`.
//!
//! The tables are **generated** (committed Rust source, no build-time or
//! runtime dependencies) from PrismarineJS `minecraft-data`, which is itself
//! derived from the vanilla data-generator reports. Regenerate with
//! `scripts/generate_blocks.py`.
//!
//! Multi-version note: today we ship the 1.20.1 (protocol 763) table. New
//! versions become sibling generated modules selected by protocol number.

mod blocks_1_20_1;

pub use blocks_1_20_1::BLOCKS_1_20_1;

/// A block and the contiguous, disjoint range of global block-state IDs it owns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockDef {
    /// Namespaced id, e.g. `"minecraft:grass_block"`.
    pub name: &'static str,
    /// First block-state ID owned by this block.
    pub min_state: u32,
    /// Last block-state ID owned by this block (inclusive).
    pub max_state: u32,
    /// The block's default block-state ID.
    pub default_state: u32,
}

/// Resolves the block owning `state` via binary search over the sorted table.
///
/// State-ID ranges are contiguous and disjoint, so this is exact.
#[must_use]
pub fn block_for_state(state: u32) -> Option<&'static BlockDef> {
    let blocks = BLOCKS_1_20_1;
    let (mut lo, mut hi) = (0usize, blocks.len());
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let b = &blocks[mid];
        if state < b.min_state {
            hi = mid;
        } else if state > b.max_state {
            lo = mid + 1;
        } else {
            return Some(b);
        }
    }
    None
}

/// Namespaced block name for a block-state ID, if known.
#[must_use]
pub fn block_name(state: u32) -> Option<&'static str> {
    block_for_state(state).map(|b| b.name)
}

/// Whether a state is one of the non-collidable air blocks.
#[must_use]
pub fn is_air(state: u32) -> bool {
    matches!(
        block_name(state),
        Some("minecraft:air" | "minecraft:cave_air" | "minecraft:void_air")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_blocks_resolve() {
        assert_eq!(block_name(0), Some("minecraft:air"));
        assert_eq!(block_name(9), Some("minecraft:grass_block"));
        assert_eq!(block_name(10), Some("minecraft:dirt"));
        assert_eq!(block_name(79), Some("minecraft:bedrock"));
    }

    #[test]
    fn air_detection() {
        assert!(is_air(0));
        assert!(!is_air(9)); // grass is not air
        assert!(is_air(12817)); // void_air
        assert!(is_air(12818)); // cave_air
    }

    #[test]
    fn out_of_range_is_none() {
        assert_eq!(block_name(u32::MAX), None);
    }

    #[test]
    fn table_is_sorted_and_disjoint() {
        let mut prev_max = None;
        for b in BLOCKS_1_20_1 {
            assert!(b.min_state <= b.max_state, "{} has inverted range", b.name);
            if let Some(pm) = prev_max {
                assert!(b.min_state > pm, "{} overlaps previous range", b.name);
            }
            assert!(
                b.min_state <= b.default_state && b.default_state <= b.max_state,
                "{} default out of range",
                b.name
            );
            prev_max = Some(b.max_state);
        }
    }
}

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
mod entities_1_20_1;
mod items_1_20_1;

pub use blocks_1_20_1::BLOCKS_1_20_1;
pub use entities_1_20_1::ENTITIES_1_20_1;
pub use items_1_20_1::ITEMS_1_20_1;

/// An entity type and its default hitbox size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EntityDef {
    pub id: u32,
    pub name: &'static str,
    pub width: f32,
    pub height: f32,
}

/// Looks up an entity type by its registry id.
#[must_use]
pub fn entity_def(id: u32) -> Option<&'static EntityDef> {
    ENTITIES_1_20_1
        .binary_search_by_key(&id, |e| e.id)
        .ok()
        .map(|i| &ENTITIES_1_20_1[i])
}

/// Entity name (e.g. `"cow"`) for a type id.
#[must_use]
pub fn entity_name(id: u32) -> Option<&'static str> {
    entity_def(id).map(|e| e.name)
}

/// An item type and its maximum stack size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ItemDef {
    pub id: u32,
    pub name: &'static str,
    pub stack_size: u8,
}

/// Looks up an item by its registry id.
#[must_use]
pub fn item_def(id: u32) -> Option<&'static ItemDef> {
    ITEMS_1_20_1
        .binary_search_by_key(&id, |e| e.id)
        .ok()
        .map(|i| &ITEMS_1_20_1[i])
}

/// Item name (e.g. `"diamond"`) for an item id.
#[must_use]
pub fn item_name(id: u32) -> Option<&'static str> {
    item_def(id).map(|e| e.name)
}

/// A block and the contiguous, disjoint range of global block-state IDs it owns.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockDef {
    /// Namespaced id, e.g. `"minecraft:grass_block"`.
    pub name: &'static str,
    /// First block-state ID owned by this block.
    pub min_state: u32,
    /// Last block-state ID owned by this block (inclusive).
    pub max_state: u32,
    /// The block's default block-state ID.
    pub default_state: u32,
    /// Mining hardness; negative means unbreakable (e.g. bedrock).
    pub hardness: f32,
    /// True if a proper tool is required to harvest (bare hands mine it ~3x
    /// slower); affects break time.
    pub needs_tool: bool,
}

/// Ticks (at 20 TPS) for a **bare hand** to break `state`, or `None` if it is
/// unbreakable. `0` means instant (e.g. plants). This is the slowest case (no
/// tool), so a server validating dig speed never rejects it as too fast.
#[must_use]
pub fn break_ticks(state: u32) -> Option<u32> {
    let b = block_for_state(state)?;
    if b.hardness < 0.0 {
        return None;
    }
    // damage/tick = speed(1.0) / hardness / (canHarvest ? 30 : 100)
    let denom = if b.needs_tool { 100.0 } else { 30.0 };
    Some((b.hardness * denom).ceil() as u32)
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
        assert_eq!(item_name(u32::MAX), None);
    }

    #[test]
    fn break_times_are_sane() {
        // dirt 0.5 * 30 = 15; grass 0.6 * 30 = 18; oak_log 2.0 * 30 = 60.
        assert_eq!(break_ticks(10), Some(15)); // dirt
        assert_eq!(break_ticks(9), Some(18)); // grass_block
                                              // stone needs a tool: 1.5 * 100 = 150.
        assert_eq!(break_ticks(1), Some(150));
        // bedrock is unbreakable.
        assert_eq!(break_ticks(79), None);
    }

    #[test]
    fn known_items_resolve() {
        assert_eq!(item_name(0), Some("air"));
        assert_eq!(item_name(1), Some("stone"));
        assert_eq!(item_name(764), Some("diamond"));
        assert_eq!(item_def(764).map(|i| i.stack_size), Some(64));
    }

    #[test]
    fn item_table_is_sorted() {
        for w in ITEMS_1_20_1.windows(2) {
            assert!(w[0].id < w[1].id, "items not sorted at {}", w[0].name);
        }
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

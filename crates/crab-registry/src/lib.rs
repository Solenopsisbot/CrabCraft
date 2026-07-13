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
//! The active 1.20.x tables are selected once at client startup from the wire
//! protocol. This keeps numeric IDs coherent across world, physics, assets and
//! rendering without leaking protocol concerns through every lookup call.

use std::sync::atomic::{AtomicU8, Ordering};

mod blocks_1_20_1;
mod blocks_1_20_2;
mod blocks_1_20_3;
mod blocks_1_20_5;
mod blocks_1_21;
mod blocks_1_21_3;
mod blocks_1_21_4;
mod blocks_1_21_5;
mod collision_generated;
mod entities_1_20_1;
mod entities_1_20_3;
mod entities_1_20_5;
mod entities_1_21_3;
mod entities_1_21_4;
mod entities_1_21_5;
mod items_1_20_1;
mod items_1_20_3;
mod items_1_20_5;
mod items_1_21;
mod items_1_21_3;
mod items_1_21_4;
mod items_1_21_5;

pub use blocks_1_20_1::BLOCKS_1_20_1;
pub use blocks_1_20_2::BLOCKS_1_20_2;
pub use blocks_1_20_3::BLOCKS_1_20_3;
pub use blocks_1_20_5::BLOCKS_1_20_5;
pub use blocks_1_21::BLOCKS_1_21;
pub use blocks_1_21_3::BLOCKS_1_21_3;
pub use blocks_1_21_4::BLOCKS_1_21_4;
pub use blocks_1_21_5::BLOCKS_1_21_5;
pub use entities_1_20_1::ENTITIES_1_20_1;
pub use entities_1_20_3::ENTITIES_1_20_3;
pub use entities_1_20_5::ENTITIES_1_20_5;
pub use entities_1_21_3::ENTITIES_1_21_3;
pub use entities_1_21_4::ENTITIES_1_21_4;
pub use entities_1_21_5::ENTITIES_1_21_5;
pub use items_1_20_1::ITEMS_1_20_1;
pub use items_1_20_3::ITEMS_1_20_3;
pub use items_1_20_5::ITEMS_1_20_5;
pub use items_1_21::ITEMS_1_21;
pub use items_1_21_3::ITEMS_1_21_3;
pub use items_1_21_4::ITEMS_1_21_4;
pub use items_1_21_5::ITEMS_1_21_5;

static REGISTRY_PROFILE: AtomicU8 = AtomicU8::new(0);

/// Selects numeric registry tables for a supported protocol. Call this before
/// loading assets or decoding packets; unknown protocols retain the 763 tables.
pub fn set_protocol(protocol: i32) {
    REGISTRY_PROFILE.store(
        match protocol {
            764 => 1,
            765 => 2,
            766 => 3,
            767 => 4,
            768 => 5,
            769 => 6,
            770 => 7,
            _ => 0,
        },
        Ordering::Relaxed,
    );
}

/// Active block registry for this process's selected server protocol.
#[must_use]
pub fn blocks() -> &'static [BlockDef] {
    match REGISTRY_PROFILE.load(Ordering::Relaxed) {
        1 => BLOCKS_1_20_2,
        2 => BLOCKS_1_20_3,
        3 => BLOCKS_1_20_5,
        4 => BLOCKS_1_21,
        5 => BLOCKS_1_21_3,
        6 => BLOCKS_1_21_4,
        7 => BLOCKS_1_21_5,
        _ => BLOCKS_1_20_1,
    }
}

/// Active item registry for this process's selected server protocol.
#[must_use]
pub fn items() -> &'static [ItemDef] {
    match REGISTRY_PROFILE.load(Ordering::Relaxed) {
        2 => ITEMS_1_20_3,
        3 => ITEMS_1_20_5,
        4 => ITEMS_1_21,
        5 => ITEMS_1_21_3,
        6 => ITEMS_1_21_4,
        7 => ITEMS_1_21_5,
        _ => ITEMS_1_20_1,
    }
}

/// Active entity registry for this process's selected server protocol.
#[must_use]
pub fn entities() -> &'static [EntityDef] {
    match REGISTRY_PROFILE.load(Ordering::Relaxed) {
        2 => ENTITIES_1_20_3,
        3 => ENTITIES_1_20_5,
        4 => ENTITIES_1_20_5,
        5 => ENTITIES_1_21_3,
        6 => ENTITIES_1_21_4,
        7 => ENTITIES_1_21_5,
        _ => ENTITIES_1_20_1,
    }
}

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
    let entities = entities();
    entities
        .binary_search_by_key(&id, |e| e.id)
        .ok()
        .map(|i| &entities[i])
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
    let items = items();
    items
        .binary_search_by_key(&id, |e| e.id)
        .ok()
        .map(|i| &items[i])
}

/// Item name (e.g. `"diamond"`) for an item id.
#[must_use]
pub fn item_name(id: u32) -> Option<&'static str> {
    item_def(id).map(|e| e.name)
}

/// One blockstate property in global-state enumeration order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockProperty {
    pub name: &'static str,
    /// Values in wire-ID radix order (the first property is most significant).
    pub values: &'static [&'static str],
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
    /// Property radices used to decode a global state ID without guessing.
    pub properties: &'static [BlockProperty],
}

/// Resolves one property value from a global block-state ID.
#[must_use]
pub fn block_state_property(state: u32, property: &str) -> Option<&'static str> {
    let block = block_for_state(state)?;
    let offset = usize::try_from(state.checked_sub(block.min_state)?).ok()?;
    let index = block
        .properties
        .iter()
        .position(|candidate| candidate.name == property)?;
    let stride = block.properties[index + 1..]
        .iter()
        .try_fold(1usize, |product, property| {
            product.checked_mul(property.values.len())
        })?;
    let values = block.properties[index].values;
    values.get((offset / stride) % values.len()).copied()
}

/// Returns all property/value pairs for a global block-state ID.
#[must_use]
pub fn block_state_properties(state: u32) -> Option<Vec<(&'static str, &'static str)>> {
    let block = block_for_state(state)?;
    block
        .properties
        .iter()
        .map(|property| {
            block_state_property(state, property.name).map(|value| (property.name, value))
        })
        .collect()
}

/// One axis-aligned part of a block collision shape, in local 0..=1 coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CollisionBox {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

/// Allocation-free block collision shape (up to sixteen boxes, matching the
/// largest generated vanilla shape in the supported registry profiles).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CollisionShape {
    boxes: [CollisionBox; 16],
    len: u8,
}

impl CollisionShape {
    #[must_use]
    pub fn boxes(&self) -> &[CollisionBox] {
        &self.boxes[..usize::from(self.len)]
    }
}

const EMPTY_BOX: CollisionBox = CollisionBox {
    min: [0.0; 3],
    max: [0.0; 3],
};

#[cfg(test)]
const FULL_BOX: CollisionBox = CollisionBox {
    min: [0.0; 3],
    max: [1.0; 3],
};

fn collision_state_shapes() -> &'static [u16] {
    use collision_generated::*;
    match REGISTRY_PROFILE.load(Ordering::Relaxed) {
        1 => COLLISION_STATES_1_20_2,
        2 => COLLISION_STATES_1_20_3,
        3 => COLLISION_STATES_1_20_5,
        4 => COLLISION_STATES_1_21,
        5 => COLLISION_STATES_1_21_3,
        6 => COLLISION_STATES_1_21_4,
        7 => COLLISION_STATES_1_21_5,
        _ => COLLISION_STATES_1_20_1,
    }
}

/// Exact vanilla collision boxes for a global block state in the active
/// registry profile. The generated data is deduplicated across supported
/// versions and comes from the vanilla voxel-shape extraction published by
/// PrismarineJS minecraft-data.
#[must_use]
pub fn collision_shape(state: u32) -> CollisionShape {
    let Some(&shape_id) = usize::try_from(state)
        .ok()
        .and_then(|state| collision_state_shapes().get(state))
    else {
        return CollisionShape {
            boxes: [EMPTY_BOX; 16],
            len: 0,
        };
    };
    let Some(&(start, len)) = collision_generated::COLLISION_SHAPES.get(usize::from(shape_id))
    else {
        return CollisionShape {
            boxes: [EMPTY_BOX; 16],
            len: 0,
        };
    };
    let start = start as usize;
    let len = usize::from(len);
    let Some(source) = collision_generated::COLLISION_BOXES.get(start..start + len) else {
        return CollisionShape {
            boxes: [EMPTY_BOX; 16],
            len: 0,
        };
    };
    let mut boxes = [EMPTY_BOX; 16];
    boxes[..len].copy_from_slice(source);
    CollisionShape {
        boxes,
        len: len as u8,
    }
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
    let blocks = blocks();
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

/// Looks up a block by (bare or namespaced) name, e.g. `"oak_planks"`.
#[must_use]
pub fn block_by_name(name: &str) -> Option<&'static BlockDef> {
    let bare = name.strip_prefix("minecraft:").unwrap_or(name);
    blocks()
        .iter()
        .find(|b| b.name.strip_prefix("minecraft:") == Some(bare))
}

/// Whether a state is one of the non-collidable air blocks.
#[must_use]
pub fn is_air(state: u32) -> bool {
    matches!(
        block_name(state),
        Some("minecraft:air" | "minecraft:cave_air" | "minecraft:void_air")
    )
}

/// Whether a block state has any player collision.
///
#[must_use]
pub fn is_collidable(state: u32) -> bool {
    !collision_shape(state).boxes().is_empty()
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
    fn collision_distinguishes_solid_and_empty_shapes() {
        for name in ["stone", "oak_planks", "bedrock", "oak_fence"] {
            let state = block_by_name(name).unwrap().default_state;
            assert!(is_collidable(state), "{name} should collide");
        }
        for name in [
            "air",
            "water",
            "lava",
            "dandelion",
            "oak_sapling",
            "torch",
            "rail",
            "redstone_wire",
            "vine",
        ] {
            let state = block_by_name(name).unwrap().default_state;
            assert!(!is_collidable(state), "{name} should not collide");
        }
    }

    #[test]
    fn slab_and_stair_states_have_partial_collision_geometry() {
        let slab = block_by_name("oak_slab").unwrap();
        let bottom = collision_shape(slab.default_state);
        assert_eq!(bottom.boxes().len(), 1);
        assert_eq!(bottom.boxes()[0].max[1], 0.5);
        assert_eq!(collision_shape(slab.min_state).boxes()[0].min[1], 0.5);
        assert_eq!(collision_shape(slab.max_state).boxes()[0].max[1], 1.0);

        let stairs = block_by_name("oak_stairs").unwrap();
        let straight = collision_shape(stairs.default_state);
        assert_eq!(straight.boxes().len(), 2); // merged base + half-width step
        assert_eq!(straight.boxes()[0].max[1], 0.5);
        let inner_bottom = collision_shape(stairs.min_state + 13);
        assert_eq!(inner_bottom.boxes().len(), 3);
    }

    #[test]
    fn generated_property_radices_decode_global_states() {
        let stairs = block_by_name("oak_stairs").unwrap();
        assert_eq!(
            block_state_property(stairs.min_state, "facing"),
            Some("north")
        );
        assert_eq!(block_state_property(stairs.min_state, "half"), Some("top"));
        assert_eq!(
            block_state_property(stairs.min_state, "shape"),
            Some("straight")
        );
        assert_eq!(
            block_state_property(stairs.default_state, "half"),
            Some("bottom")
        );

        let fence = block_by_name("oak_fence").unwrap();
        assert_eq!(block_state_property(fence.min_state, "east"), Some("true"));
        assert_eq!(
            block_state_property(fence.default_state, "east"),
            Some("false")
        );
        let properties = block_state_properties(fence.default_state).unwrap();
        assert!(properties.contains(&("waterlogged", "false")));
    }

    #[test]
    fn connected_and_openable_blocks_decode_state_shapes() {
        let fence = block_by_name("oak_fence").unwrap();
        assert_eq!(collision_shape(fence.default_state).boxes().len(), 1);
        assert_eq!(collision_shape(fence.min_state).boxes().len(), 3);
        assert_eq!(collision_shape(fence.default_state).boxes()[0].max[1], 1.5);

        let wall = block_by_name("cobblestone_wall").unwrap();
        assert_eq!(collision_shape(wall.default_state).boxes().len(), 1);
        assert_eq!(collision_shape(wall.min_state + 108).boxes().len(), 2);

        let pane = block_by_name("glass_pane").unwrap();
        assert_eq!(collision_shape(pane.default_state).boxes().len(), 1);
        assert_eq!(collision_shape(pane.min_state).boxes().len(), 3);
        assert_eq!(collision_shape(pane.min_state).boxes()[0].max[1], 1.0);

        let gate = block_by_name("oak_fence_gate").unwrap();
        assert!(collision_shape(gate.min_state).boxes().is_empty());
        assert_eq!(collision_shape(gate.default_state).boxes()[0].max[1], 1.5);

        let trapdoor = block_by_name("oak_trapdoor").unwrap();
        let closed = collision_shape(trapdoor.default_state);
        assert_eq!(closed.boxes()[0].max[1], 3.0 / 16.0);
        let open = collision_shape(trapdoor.min_state);
        assert_eq!(open.boxes()[0].max[1], 1.0);

        let snow = block_by_name("snow").unwrap();
        assert!(collision_shape(snow.min_state).boxes().is_empty());
        assert_eq!(
            collision_shape(snow.max_state).boxes()[0].max[1],
            14.0 / 16.0
        );

        let chest = collision_shape(block_by_name("chest").unwrap().default_state);
        assert_eq!(chest.boxes()[0].max[1], 14.0 / 16.0);
        let pot = collision_shape(block_by_name("flower_pot").unwrap().default_state);
        assert_eq!(pot.boxes()[0].max[1], 6.0 / 16.0);
    }

    #[test]
    fn specialized_utility_blocks_have_non_cube_collision() {
        for (name, boxes) in [
            ("cauldron", 15),
            ("composter", 5),
            ("hopper", 13),
            ("anvil", 7),
            ("bell", 1),
        ] {
            let shape = collision_shape(block_by_name(name).unwrap().default_state);
            assert_eq!(shape.boxes().len(), boxes, "{name}");
            assert_ne!(shape.boxes()[0], FULL_BOX, "{name} must not be a cube");
        }
        let head = collision_shape(block_by_name("player_head").unwrap().default_state);
        assert_eq!(head.boxes()[0].max[1], 0.5);
        let candle = collision_shape(block_by_name("candle").unwrap().default_state);
        assert_eq!(candle.boxes()[0].max[1], 6.0 / 16.0);

        for name in [
            "chain",
            "sea_pickle",
            "brewing_stand",
            "lectern",
            "grindstone",
            "lantern",
        ] {
            let shape = collision_shape(block_by_name(name).unwrap().default_state);
            assert!(!shape.boxes().is_empty(), "{name}");
            assert_ne!(shape.boxes()[0], FULL_BOX, "{name} must not be a cube");
        }
        let bamboo = collision_shape(block_by_name("bamboo").unwrap().default_state);
        assert_eq!(bamboo.boxes().len(), 1);
        assert_eq!(bamboo.boxes()[0].max[0], 0.34375);
    }

    #[test]
    fn generated_collision_tables_cover_every_supported_state() {
        use collision_generated::*;
        for (blocks, states) in [
            (BLOCKS_1_20_1, COLLISION_STATES_1_20_1),
            (BLOCKS_1_20_2, COLLISION_STATES_1_20_2),
            (BLOCKS_1_20_3, COLLISION_STATES_1_20_3),
            (BLOCKS_1_20_5, COLLISION_STATES_1_20_5),
            (BLOCKS_1_21, COLLISION_STATES_1_21),
            (BLOCKS_1_21_3, COLLISION_STATES_1_21_3),
            (BLOCKS_1_21_4, COLLISION_STATES_1_21_4),
            (BLOCKS_1_21_5, COLLISION_STATES_1_21_5),
        ] {
            assert_eq!(states.len(), blocks.last().unwrap().max_state as usize + 1);
            assert!(states
                .iter()
                .all(|shape| usize::from(*shape) < COLLISION_SHAPES.len()));
        }
        assert!(COLLISION_SHAPES.iter().all(|(_, len)| *len <= 16));
        assert!(COLLISION_SHAPES
            .iter()
            .all(|(start, len)| { *start as usize + usize::from(*len) <= COLLISION_BOXES.len() }));
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

    #[test]
    fn later_1_20_tables_preserve_their_distinct_wire_ids() {
        let crafter_item = ITEMS_1_20_3
            .iter()
            .find(|item| item.name == "crafter")
            .unwrap();
        assert_eq!(crafter_item.id, 978);
        let diamond_sword = ITEMS_1_20_3
            .iter()
            .find(|item| item.name == "diamond_sword")
            .unwrap();
        assert_eq!(diamond_sword.id, 834);

        let crafter = BLOCKS_1_20_3
            .iter()
            .find(|block| block.name == "minecraft:crafter")
            .unwrap();
        assert_eq!(crafter.default_state, 26_635);
        assert!(BLOCKS_1_20_2
            .iter()
            .all(|block| block.name != "minecraft:crafter"));

        let breeze = ENTITIES_1_20_3
            .iter()
            .find(|entity| entity.name == "breeze")
            .unwrap();
        assert_eq!(breeze.id, 10);

        let wolf_armor = ITEMS_1_20_5
            .iter()
            .find(|item| item.name == "wolf_armor")
            .unwrap();
        assert_eq!(wolf_armor.id, 797);
        let diamond_sword_766 = ITEMS_1_20_5
            .iter()
            .find(|item| item.name == "diamond_sword")
            .unwrap();
        assert_eq!(diamond_sword_766.id, 837);
        let armadillo = ENTITIES_1_20_5
            .iter()
            .find(|entity| entity.name == "armadillo")
            .unwrap();
        assert_eq!(armadillo.id, 2);

        let mace = ITEMS_1_21.iter().find(|item| item.name == "mace").unwrap();
        assert_eq!(mace.id, 1093);
        let trial_spawner = BLOCKS_1_21
            .iter()
            .find(|block| block.name == "minecraft:trial_spawner")
            .unwrap();
        assert_eq!(trial_spawner.default_state, 26_644);

        let mace_768 = ITEMS_1_21_3
            .iter()
            .find(|item| item.name == "mace")
            .unwrap();
        assert_eq!(mace_768.id, 1135);
        let pale_oak = BLOCKS_1_21_3
            .iter()
            .find(|block| block.name == "minecraft:pale_oak_planks")
            .unwrap();
        assert_eq!(pale_oak.default_state, 25);
        let creaking = ENTITIES_1_21_3
            .iter()
            .find(|entity| entity.name == "creaking")
            .unwrap();
        assert_eq!(creaking.id, 29);

        let mace_769 = ITEMS_1_21_4
            .iter()
            .find(|item| item.name == "mace")
            .unwrap();
        assert_eq!(mace_769.id, 1144);
        let resin = ITEMS_1_21_4
            .iter()
            .find(|item| item.name == "resin_clump")
            .unwrap();
        assert_eq!(resin.id, 376);
        let creaking_heart = BLOCKS_1_21_4
            .iter()
            .find(|block| block.name == "minecraft:creaking_heart")
            .unwrap();
        assert_eq!(creaking_heart.default_state, 2926);
        assert_eq!(
            ENTITIES_1_21_4
                .iter()
                .find(|entity| entity.name == "creaking")
                .unwrap()
                .id,
            29
        );

        assert_eq!(
            ITEMS_1_21_5
                .iter()
                .find(|item| item.name == "blue_egg")
                .unwrap()
                .id,
            970
        );
        assert_eq!(
            ITEMS_1_21_5
                .iter()
                .find(|item| item.name == "mace")
                .unwrap()
                .id,
            1155
        );
        assert_eq!(
            BLOCKS_1_21_5
                .iter()
                .find(|block| block.name == "minecraft:firefly_bush")
                .unwrap()
                .default_state,
            27_913
        );
        assert!(ENTITIES_1_21_5
            .iter()
            .any(|entity| entity.name == "lingering_potion"));
    }
}

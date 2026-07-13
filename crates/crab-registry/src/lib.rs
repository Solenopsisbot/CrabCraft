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
mod entities_1_20_1;
mod entities_1_20_3;
mod entities_1_20_5;
mod items_1_20_1;
mod items_1_20_3;
mod items_1_20_5;

pub use blocks_1_20_1::BLOCKS_1_20_1;
pub use blocks_1_20_2::BLOCKS_1_20_2;
pub use blocks_1_20_3::BLOCKS_1_20_3;
pub use blocks_1_20_5::BLOCKS_1_20_5;
pub use entities_1_20_1::ENTITIES_1_20_1;
pub use entities_1_20_3::ENTITIES_1_20_3;
pub use entities_1_20_5::ENTITIES_1_20_5;
pub use items_1_20_1::ITEMS_1_20_1;
pub use items_1_20_3::ITEMS_1_20_3;
pub use items_1_20_5::ITEMS_1_20_5;

static REGISTRY_PROFILE: AtomicU8 = AtomicU8::new(0);

/// Selects numeric registry tables for a supported protocol. Call this before
/// loading assets or decoding packets; unknown protocols retain the 763 tables.
pub fn set_protocol(protocol: i32) {
    REGISTRY_PROFILE.store(
        match protocol {
            764 => 1,
            765 => 2,
            766 => 3,
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
        _ => BLOCKS_1_20_1,
    }
}

/// Active item registry for this process's selected server protocol.
#[must_use]
pub fn items() -> &'static [ItemDef] {
    match REGISTRY_PROFILE.load(Ordering::Relaxed) {
        2 => ITEMS_1_20_3,
        3 => ITEMS_1_20_5,
        _ => ITEMS_1_20_1,
    }
}

/// Active entity registry for this process's selected server protocol.
#[must_use]
pub fn entities() -> &'static [EntityDef] {
    match REGISTRY_PROFILE.load(Ordering::Relaxed) {
        2 => ENTITIES_1_20_3,
        3 => ENTITIES_1_20_5,
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

/// One axis-aligned part of a block collision shape, in local 0..=1 coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CollisionBox {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

/// Allocation-free block collision shape (up to eight boxes).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CollisionShape {
    boxes: [CollisionBox; 8],
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
const FULL_BOX: CollisionBox = CollisionBox {
    min: [0.0, 0.0, 0.0],
    max: [1.0, 1.0, 1.0],
};

fn one_box(min_y: f64, max_y: f64) -> CollisionShape {
    CollisionShape {
        boxes: [
            CollisionBox {
                min: [0.0, min_y, 0.0],
                max: [1.0, max_y, 1.0],
            },
            EMPTY_BOX,
            EMPTY_BOX,
            EMPTY_BOX,
            EMPTY_BOX,
            EMPTY_BOX,
            EMPTY_BOX,
            EMPTY_BOX,
        ],
        len: 1,
    }
}

fn stair_shape(facing: usize, half: usize, shape: usize) -> CollisionShape {
    // Quadrant bits: NW, NE, SW, SE. `facing` is north/south/west/east;
    // `shape` is straight/inner-left/inner-right/outer-left/outer-right.
    const MASKS: [[u8; 5]; 4] = [
        [0b0011, 0b0111, 0b1011, 0b0001, 0b0010],
        [0b1100, 0b1110, 0b1101, 0b1000, 0b0100],
        [0b0101, 0b1101, 0b0111, 0b0100, 0b0001],
        [0b1010, 0b1011, 0b1110, 0b0010, 0b1000],
    ];
    let (base_min, base_max, step_min, step_max) = if half == 0 {
        (0.5, 1.0, 0.0, 0.5) // top stair
    } else {
        (0.0, 0.5, 0.5, 1.0) // bottom stair
    };
    let mut boxes = [EMPTY_BOX; 8];
    boxes[0] = CollisionBox {
        min: [0.0, base_min, 0.0],
        max: [1.0, base_max, 1.0],
    };
    let mut len = 1usize;
    let mask = MASKS[facing.min(3)][shape.min(4)];
    for (bit, (x0, z0)) in [
        (1, (0.0, 0.0)),
        (2, (0.5, 0.0)),
        (4, (0.0, 0.5)),
        (8, (0.5, 0.5)),
    ] {
        if mask & bit != 0 {
            boxes[len] = CollisionBox {
                min: [x0, step_min, z0],
                max: [x0 + 0.5, step_max, z0 + 0.5],
            };
            len += 1;
        }
    }
    CollisionShape {
        boxes,
        len: len as u8,
    }
}

fn single_box(min: [f64; 3], max: [f64; 3]) -> CollisionShape {
    CollisionShape {
        boxes: [
            CollisionBox { min, max },
            EMPTY_BOX,
            EMPTY_BOX,
            EMPTY_BOX,
            EMPTY_BOX,
            EMPTY_BOX,
            EMPTY_BOX,
            EMPTY_BOX,
        ],
        len: 1,
    }
}

fn box_shape(boxes: &[CollisionBox]) -> CollisionShape {
    let len = boxes.len().min(8);
    let mut shape = CollisionShape {
        boxes: [EMPTY_BOX; 8],
        len: len as u8,
    };
    shape.boxes[..len].copy_from_slice(&boxes[..len]);
    shape
}

fn connected_shape(
    center: Option<CollisionBox>,
    arms: [bool; 4],
    inset: f64,
    height: f64,
) -> CollisionShape {
    let mut boxes = [EMPTY_BOX; 8];
    let mut len = 0usize;
    if let Some(center) = center {
        boxes[len] = center;
        len += 1;
    }
    let lo = inset;
    let hi = 1.0 - inset;
    for (connected, part) in [
        (
            arms[0],
            CollisionBox {
                min: [hi, 0.0, lo],
                max: [1.0, height, hi],
            },
        ),
        (
            arms[1],
            CollisionBox {
                min: [lo, 0.0, 0.0],
                max: [hi, height, lo],
            },
        ),
        (
            arms[2],
            CollisionBox {
                min: [lo, 0.0, hi],
                max: [hi, height, 1.0],
            },
        ),
        (
            arms[3],
            CollisionBox {
                min: [0.0, 0.0, lo],
                max: [lo, height, hi],
            },
        ),
    ] {
        if connected {
            boxes[len] = part;
            len += 1;
        }
    }
    CollisionShape {
        boxes,
        len: len as u8,
    }
}

fn door_plane(facing: usize, open: bool, hinge_right: bool) -> CollisionShape {
    let direction = if open {
        match (facing, hinge_right) {
            (0, false) | (1, true) => 2,
            (0, true) | (1, false) => 3,
            (2, false) | (3, true) => 1,
            _ => 0,
        }
    } else {
        facing
    };
    let t = 3.0 / 16.0;
    match direction {
        0 => single_box([0.0, 0.0, 1.0 - t], [1.0, 1.0, 1.0]),
        1 => single_box([0.0, 0.0, 0.0], [1.0, 1.0, t]),
        2 => single_box([1.0 - t, 0.0, 0.0], [1.0, 1.0, 1.0]),
        _ => single_box([0.0, 0.0, 0.0], [t, 1.0, 1.0]),
    }
}

/// Collision boxes for a 1.20.1 block state. Air/fluids/decorations are empty;
/// slabs and all stair variants use their state-dependent vanilla geometry.
#[must_use]
pub fn collision_shape(state: u32) -> CollisionShape {
    let Some(block) = block_for_state(state) else {
        return CollisionShape {
            boxes: [EMPTY_BOX; 8],
            len: 0,
        };
    };
    if !is_collidable(state) {
        return CollisionShape {
            boxes: [EMPTY_BOX; 8],
            len: 0,
        };
    }
    let bare = block.name.strip_prefix("minecraft:").unwrap_or(block.name);
    let offset = (state - block.min_state) as usize;
    if bare.ends_with("_slab") {
        return match offset / 2 {
            0 => one_box(0.5, 1.0),
            1 => one_box(0.0, 0.5),
            _ => one_box(0.0, 1.0),
        };
    }
    if bare.ends_with("_stairs") {
        return stair_shape(offset / 20, (offset % 20) / 10, (offset % 10) / 2);
    }
    if bare == "snow" {
        let layers = offset + 1;
        return if layers == 1 {
            CollisionShape {
                boxes: [EMPTY_BOX; 8],
                len: 0,
            }
        } else {
            one_box(0.0, (layers - 1) as f64 * 2.0 / 16.0)
        };
    }
    if bare.ends_with("_fence") {
        let arms = [
            (offset / 16).is_multiple_of(2),
            (offset / 8).is_multiple_of(2),
            (offset / 4).is_multiple_of(2),
            offset.is_multiple_of(2),
        ];
        return connected_shape(
            Some(CollisionBox {
                min: [6.0 / 16.0, 0.0, 6.0 / 16.0],
                max: [10.0 / 16.0, 1.5, 10.0 / 16.0],
            }),
            arms,
            6.0 / 16.0,
            1.5,
        );
    }
    if bare.ends_with("_pane") || bare == "iron_bars" {
        let arms = [
            (offset / 16).is_multiple_of(2),
            (offset / 8).is_multiple_of(2),
            (offset / 4).is_multiple_of(2),
            offset.is_multiple_of(2),
        ];
        return connected_shape(
            Some(CollisionBox {
                min: [7.0 / 16.0, 0.0, 7.0 / 16.0],
                max: [9.0 / 16.0, 1.0, 9.0 / 16.0],
            }),
            arms,
            7.0 / 16.0,
            1.0,
        );
    }
    if bare.ends_with("_wall") {
        let east = offset / 108;
        let north = (offset % 108) / 36;
        let south = (offset % 36) / 12;
        let up = (offset % 12) / 6 == 0;
        let west = offset % 3;
        return connected_shape(
            up.then_some(CollisionBox {
                min: [4.0 / 16.0, 0.0, 4.0 / 16.0],
                max: [12.0 / 16.0, 1.5, 12.0 / 16.0],
            }),
            [east != 0, north != 0, south != 0, west != 0],
            5.0 / 16.0,
            1.5,
        );
    }
    if bare.ends_with("_fence_gate") {
        let facing = offset / 8;
        let open = (offset % 4) / 2 == 0;
        if open {
            return CollisionShape {
                boxes: [EMPTY_BOX; 8],
                len: 0,
            };
        }
        return if facing < 2 {
            single_box([0.0, 0.0, 6.0 / 16.0], [1.0, 1.5, 10.0 / 16.0])
        } else {
            single_box([6.0 / 16.0, 0.0, 0.0], [10.0 / 16.0, 1.5, 1.0])
        };
    }
    if bare.ends_with("_trapdoor") {
        let facing = offset / 16;
        let half = (offset % 16) / 8;
        let open = (offset % 8) / 4 == 0;
        let t = 3.0 / 16.0;
        return if open {
            door_plane(facing, false, false)
        } else if half == 0 {
            one_box(1.0 - t, 1.0)
        } else {
            one_box(0.0, t)
        };
    }
    if bare.ends_with("_door") {
        let facing = offset / 16;
        let hinge_right = (offset % 8) / 4 == 1;
        let open = (offset % 4) / 2 == 0;
        return door_plane(facing, open, hinge_right);
    }
    if matches!(bare, "chest" | "trapped_chest" | "ender_chest") {
        return single_box(
            [1.0 / 16.0, 0.0, 1.0 / 16.0],
            [15.0 / 16.0, 14.0 / 16.0, 15.0 / 16.0],
        );
    }
    if bare.ends_with("_bed") {
        return one_box(0.0, 9.0 / 16.0);
    }
    if bare.starts_with("potted_") || bare == "flower_pot" {
        return single_box(
            [5.0 / 16.0, 0.0, 5.0 / 16.0],
            [11.0 / 16.0, 6.0 / 16.0, 11.0 / 16.0],
        );
    }
    if bare == "cactus" {
        return single_box(
            [1.0 / 16.0, 0.0, 1.0 / 16.0],
            [15.0 / 16.0, 15.0 / 16.0, 15.0 / 16.0],
        );
    }
    if bare.ends_with("cauldron") || bare == "composter" {
        let t = 2.0 / 16.0;
        return box_shape(&[
            CollisionBox {
                min: [0.0, 0.0, 0.0],
                max: [1.0, t, 1.0],
            },
            CollisionBox {
                min: [0.0, 0.0, 0.0],
                max: [t, 1.0, 1.0],
            },
            CollisionBox {
                min: [1.0 - t, 0.0, 0.0],
                max: [1.0, 1.0, 1.0],
            },
            CollisionBox {
                min: [t, 0.0, 0.0],
                max: [1.0 - t, 1.0, t],
            },
            CollisionBox {
                min: [t, 0.0, 1.0 - t],
                max: [1.0 - t, 1.0, 1.0],
            },
        ]);
    }
    if bare == "hopper" {
        return box_shape(&[
            CollisionBox {
                min: [4.0 / 16.0, 0.0, 4.0 / 16.0],
                max: [12.0 / 16.0, 10.0 / 16.0, 12.0 / 16.0],
            },
            CollisionBox {
                min: [0.0, 10.0 / 16.0, 0.0],
                max: [2.0 / 16.0, 1.0, 1.0],
            },
            CollisionBox {
                min: [14.0 / 16.0, 10.0 / 16.0, 0.0],
                max: [1.0, 1.0, 1.0],
            },
            CollisionBox {
                min: [2.0 / 16.0, 10.0 / 16.0, 0.0],
                max: [14.0 / 16.0, 1.0, 2.0 / 16.0],
            },
            CollisionBox {
                min: [2.0 / 16.0, 10.0 / 16.0, 14.0 / 16.0],
                max: [14.0 / 16.0, 1.0, 1.0],
            },
        ]);
    }
    if bare.ends_with("anvil") {
        let along_x = offset.is_multiple_of(2);
        let (top_min, top_max) = if along_x {
            ([0.0, 10.0 / 16.0, 3.0 / 16.0], [1.0, 1.0, 13.0 / 16.0])
        } else {
            ([3.0 / 16.0, 10.0 / 16.0, 0.0], [13.0 / 16.0, 1.0, 1.0])
        };
        return box_shape(&[
            CollisionBox {
                min: [2.0 / 16.0, 0.0, 2.0 / 16.0],
                max: [14.0 / 16.0, 4.0 / 16.0, 14.0 / 16.0],
            },
            CollisionBox {
                min: [4.0 / 16.0, 4.0 / 16.0, 4.0 / 16.0],
                max: [12.0 / 16.0, 10.0 / 16.0, 12.0 / 16.0],
            },
            CollisionBox {
                min: top_min,
                max: top_max,
            },
        ]);
    }
    if bare == "bell" {
        return box_shape(&[
            CollisionBox {
                min: [6.0 / 16.0, 0.0, 6.0 / 16.0],
                max: [10.0 / 16.0, 13.0 / 16.0, 10.0 / 16.0],
            },
            CollisionBox {
                min: [4.0 / 16.0, 3.0 / 16.0, 4.0 / 16.0],
                max: [12.0 / 16.0, 11.0 / 16.0, 12.0 / 16.0],
            },
        ]);
    }
    if bare.ends_with("_head") || bare.ends_with("_skull") {
        let wall = bare.contains("wall_");
        return if wall {
            single_box(
                [4.0 / 16.0, 4.0 / 16.0, 4.0 / 16.0],
                [12.0 / 16.0, 12.0 / 16.0, 12.0 / 16.0],
            )
        } else {
            single_box(
                [4.0 / 16.0, 0.0, 4.0 / 16.0],
                [12.0 / 16.0, 8.0 / 16.0, 12.0 / 16.0],
            )
        };
    }
    if bare.ends_with("candle") || bare == "candle" {
        return single_box(
            [6.0 / 16.0, 0.0, 6.0 / 16.0],
            [10.0 / 16.0, 6.0 / 16.0, 10.0 / 16.0],
        );
    }
    if bare == "chain" {
        let axis = offset / 2;
        let r = 3.0 / 16.0;
        return match axis {
            0 => single_box([0.0, 0.5 - r, 0.5 - r], [1.0, 0.5 + r, 0.5 + r]),
            1 => single_box([0.5 - r, 0.0, 0.5 - r], [0.5 + r, 1.0, 0.5 + r]),
            _ => single_box([0.5 - r, 0.5 - r, 0.0], [0.5 + r, 0.5 + r, 1.0]),
        };
    }
    if bare == "sea_pickle" {
        let pickles = offset / 2 + 1;
        let inset = match pickles {
            1 => 6.0 / 16.0,
            2 => 4.0 / 16.0,
            _ => 2.0 / 16.0,
        };
        let height = if pickles == 1 { 6.0 / 16.0 } else { 7.0 / 16.0 };
        return single_box([inset, 0.0, inset], [1.0 - inset, height, 1.0 - inset]);
    }
    if bare == "brewing_stand" {
        return box_shape(&[
            CollisionBox {
                min: [7.0 / 16.0, 0.0, 7.0 / 16.0],
                max: [9.0 / 16.0, 14.0 / 16.0, 9.0 / 16.0],
            },
            CollisionBox {
                min: [1.0 / 16.0, 0.0, 1.0 / 16.0],
                max: [15.0 / 16.0, 2.0 / 16.0, 15.0 / 16.0],
            },
        ]);
    }
    if bare == "lectern" {
        return box_shape(&[
            CollisionBox {
                min: [0.0, 0.0, 0.0],
                max: [1.0, 2.0 / 16.0, 1.0],
            },
            CollisionBox {
                min: [4.0 / 16.0, 2.0 / 16.0, 4.0 / 16.0],
                max: [12.0 / 16.0, 14.0 / 16.0, 12.0 / 16.0],
            },
            CollisionBox {
                min: [0.0, 14.0 / 16.0, 0.0],
                max: [1.0, 1.0, 1.0],
            },
        ]);
    }
    if bare == "grindstone" {
        return box_shape(&[
            CollisionBox {
                min: [2.0 / 16.0, 0.0, 2.0 / 16.0],
                max: [14.0 / 16.0, 4.0 / 16.0, 14.0 / 16.0],
            },
            CollisionBox {
                min: [4.0 / 16.0, 4.0 / 16.0, 4.0 / 16.0],
                max: [12.0 / 16.0, 1.0, 12.0 / 16.0],
            },
        ]);
    }
    if bare == "lantern" || bare == "soul_lantern" {
        return box_shape(&[
            CollisionBox {
                min: [5.0 / 16.0, 0.0, 5.0 / 16.0],
                max: [11.0 / 16.0, 7.0 / 16.0, 11.0 / 16.0],
            },
            CollisionBox {
                min: [6.0 / 16.0, 7.0 / 16.0, 6.0 / 16.0],
                max: [10.0 / 16.0, 9.0 / 16.0, 10.0 / 16.0],
            },
        ]);
    }
    let height = match bare {
        "farmland" | "dirt_path" => 15.0 / 16.0,
        "soul_sand" | "mud" => 14.0 / 16.0,
        "enchanting_table" => 12.0 / 16.0,
        "daylight_detector" => 6.0 / 16.0,
        "stonecutter" => 9.0 / 16.0,
        "cake" => 8.0 / 16.0,
        _ if bare.ends_with("_carpet") => 1.0 / 16.0,
        _ => 1.0,
    };
    if height == 1.0 {
        CollisionShape {
            boxes: [
                FULL_BOX, EMPTY_BOX, EMPTY_BOX, EMPTY_BOX, EMPTY_BOX, EMPTY_BOX, EMPTY_BOX,
                EMPTY_BOX,
            ],
            len: 1,
        }
    } else {
        one_box(0.0, height)
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
/// This deliberately answers only the empty-vs-present part of the collision
/// shape. The physics crate still approximates present shapes as full cubes,
/// but fluid, plant and thin decorative blocks must not become invisible
/// one-block walls. These names have empty collision shapes in vanilla 1.20.1.
#[must_use]
pub fn is_collidable(state: u32) -> bool {
    let Some(name) = block_name(state) else {
        return false;
    };
    let bare = name.strip_prefix("minecraft:").unwrap_or(name);
    if matches!(
        bare,
        "air"
            | "cave_air"
            | "void_air"
            | "water"
            | "lava"
            | "grass"
            | "fern"
            | "dead_bush"
            | "seagrass"
            | "tall_seagrass"
            | "kelp"
            | "kelp_plant"
            | "vine"
            | "glow_lichen"
            | "fire"
            | "soul_fire"
            | "redstone_wire"
            | "tripwire"
            | "torch"
            | "soul_torch"
            | "rail"
            | "wheat"
            | "carrots"
            | "potatoes"
            | "beetroots"
            | "nether_wart"
            | "sugar_cane"
            | "torchflower_crop"
            | "pitcher_crop"
            | "bamboo"
            | "bamboo_sapling"
            | "brown_mushroom"
            | "red_mushroom"
            | "crimson_fungus"
            | "warped_fungus"
            | "crimson_roots"
            | "warped_roots"
            | "nether_sprouts"
            | "hanging_roots"
            | "spore_blossom"
            | "cobweb"
            | "ladder"
            | "lever"
            | "end_portal"
            | "end_gateway"
            | "nether_portal"
            | "light"
            | "structure_void"
    ) {
        return false;
    }

    let is_flower = matches!(
        bare,
        "dandelion"
            | "poppy"
            | "blue_orchid"
            | "allium"
            | "azure_bluet"
            | "red_tulip"
            | "orange_tulip"
            | "white_tulip"
            | "pink_tulip"
            | "oxeye_daisy"
            | "cornflower"
            | "lily_of_the_valley"
            | "wither_rose"
            | "sunflower"
            | "lilac"
            | "rose_bush"
            | "peony"
            | "torchflower"
            | "pitcher_plant"
    );
    !(is_flower
        || bare.ends_with("_sapling")
        || bare.ends_with("_torch")
        || bare.ends_with("_wall_torch")
        || bare.ends_with("_rail")
        || bare.ends_with("_button")
        || bare.ends_with("_pressure_plate")
        || bare.ends_with("_sign")
        || bare.ends_with("_wall_sign")
        || bare.ends_with("_hanging_sign")
        || bare.ends_with("_wall_hanging_sign")
        || bare.ends_with("_banner")
        || bare.ends_with("_wall_banner")
        || bare.ends_with("_coral")
        || bare.ends_with("_coral_fan")
        || bare.ends_with("_coral_wall_fan"))
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
        assert_eq!(straight.boxes().len(), 3); // base + two high quadrants
        assert_eq!(straight.boxes()[0].max[1], 0.5);
        let inner_bottom = collision_shape(stairs.min_state + 13);
        assert_eq!(inner_bottom.boxes().len(), 4); // base + three quadrants
    }

    #[test]
    fn connected_and_openable_blocks_decode_state_shapes() {
        let fence = block_by_name("oak_fence").unwrap();
        assert_eq!(collision_shape(fence.default_state).boxes().len(), 1);
        assert_eq!(collision_shape(fence.min_state).boxes().len(), 5);
        assert_eq!(collision_shape(fence.default_state).boxes()[0].max[1], 1.5);

        let wall = block_by_name("cobblestone_wall").unwrap();
        assert_eq!(collision_shape(wall.default_state).boxes().len(), 1);
        assert_eq!(collision_shape(wall.min_state + 108).boxes().len(), 2);

        let pane = block_by_name("glass_pane").unwrap();
        assert_eq!(collision_shape(pane.default_state).boxes().len(), 1);
        assert_eq!(collision_shape(pane.min_state).boxes().len(), 5);
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
            ("cauldron", 5),
            ("composter", 5),
            ("hopper", 5),
            ("anvil", 3),
            ("bell", 2),
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
        assert!(
            collision_shape(block_by_name("bamboo").unwrap().default_state)
                .boxes()
                .is_empty()
        );
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
    }
}

//! Turning a [`World`] region into a textured mesh.
//!
//! Face culling: a block face is emitted only when its neighbour is air (or
//! absent). Each face is textured from the [`crab_assets::Atlas`] (per-block,
//! per-face atlas UVs + a tint multiplier); blocks the atlas doesn't have a cube
//! model for fall back to a flat tinted tile.

use crab_assets::Atlas;
use crab_world::{TintKind, World};

/// A vertex: position, face normal (lighting), atlas UV, and tint multiplier.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub tint: [f32; 3],
    pub opacity: f32,
}

impl Vertex {
    /// wgpu vertex layout matching this struct.
    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        const ATTRS: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
            0 => Float32x3, // position
            1 => Float32x3, // normal
            2 => Float32x2, // uv
            3 => Float32x3, // tint
            4 => Float32, // opacity
        ];
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &ATTRS,
        }
    }
}

/// A CPU-side mesh (non-indexed triangle list).
#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
}

impl Mesh {
    pub fn vertex_count(&self) -> u32 {
        self.vertices.len() as u32
    }

    pub fn triangle_count(&self) -> usize {
        self.vertices.len() / 3
    }
}

struct Face {
    normal: [f32; 3],
    /// Four corners (unit cube space), wound as a quad.
    corners: [[f32; 3]; 4],
    /// Per-corner UV in tile space (0..1, v=0 at the texture's top), chosen so
    /// side textures stand upright.
    uvs: [[f32; 2]; 4],
}

#[rustfmt::skip]
const FACES: [Face; 6] = [
    // +X (east)
    Face { normal: [1.0, 0.0, 0.0],
        corners: [[1.0,0.0,0.0],[1.0,1.0,0.0],[1.0,1.0,1.0],[1.0,0.0,1.0]],
        uvs: [[0.0,1.0],[0.0,0.0],[1.0,0.0],[1.0,1.0]] },
    // -X (west)
    Face { normal: [-1.0, 0.0, 0.0],
        corners: [[0.0,0.0,1.0],[0.0,1.0,1.0],[0.0,1.0,0.0],[0.0,0.0,0.0]],
        uvs: [[0.0,1.0],[0.0,0.0],[1.0,0.0],[1.0,1.0]] },
    // +Y (top)
    Face { normal: [0.0, 1.0, 0.0],
        corners: [[0.0,1.0,0.0],[1.0,1.0,0.0],[1.0,1.0,1.0],[0.0,1.0,1.0]],
        uvs: [[0.0,0.0],[1.0,0.0],[1.0,1.0],[0.0,1.0]] },
    // -Y (bottom)
    Face { normal: [0.0, -1.0, 0.0],
        corners: [[0.0,0.0,1.0],[1.0,0.0,1.0],[1.0,0.0,0.0],[0.0,0.0,0.0]],
        uvs: [[0.0,1.0],[1.0,1.0],[1.0,0.0],[0.0,0.0]] },
    // +Z (south)
    Face { normal: [0.0, 0.0, 1.0],
        corners: [[1.0,0.0,1.0],[1.0,1.0,1.0],[0.0,1.0,1.0],[0.0,0.0,1.0]],
        uvs: [[0.0,1.0],[0.0,0.0],[1.0,0.0],[1.0,1.0]] },
    // -Z (north)
    Face { normal: [0.0, 0.0, -1.0],
        corners: [[0.0,0.0,0.0],[0.0,1.0,0.0],[1.0,1.0,0.0],[1.0,0.0,0.0]],
        uvs: [[0.0,1.0],[0.0,0.0],[1.0,0.0],[1.0,1.0]] },
];

/// Whether `state` is a full opaque cube, so it hides neighbouring faces.
fn occludes(atlas: &Atlas, state: u32) -> bool {
    !crab_registry::is_air(state)
        && crab_registry::block_name(state).is_some_and(|n| atlas.is_state_cube(state, n))
}

fn block_model_seed(cell: [i32; 3], part: usize) -> u64 {
    let mut seed = i64::from(cell[0]).wrapping_mul(3_129_871)
        ^ i64::from(cell[2]).wrapping_mul(116_129_781)
        ^ i64::from(cell[1]);
    seed = seed
        .wrapping_mul(seed)
        .wrapping_mul(42_317_861)
        .wrapping_add(seed.wrapping_mul(11));
    (seed ^ i64::try_from(part).unwrap_or(0).wrapping_mul(0x5deece66d)) as u64
}

fn select_state_alternative(
    part: &crab_assets::BlockStateModelPart,
    cell: [i32; 3],
    part_index: usize,
) -> Option<&crab_assets::BlockStateModelAlternative> {
    let total = part
        .alternatives
        .iter()
        .fold(0u64, |sum, model| sum + u64::from(model.weight));
    if total == 0 {
        return part.alternatives.first();
    }
    let mut choice = block_model_seed(cell, part_index) % total;
    part.alternatives.iter().find(|model| {
        if choice < u64::from(model.weight) {
            true
        } else {
            choice -= u64::from(model.weight);
            false
        }
    })
}

fn biome_tint(world: &World, name: &str, cell: [i32; 3], model_tint: [f32; 3]) -> [f32; 3] {
    if model_tint == [1.0, 1.0, 1.0] {
        return model_tint;
    }
    let bare = name.strip_prefix("minecraft:").unwrap_or(name);
    let kind = if matches!(bare, "water" | "bubble_column") {
        TintKind::Water
    } else if bare.ends_with("_leaves") || bare.contains("vine") {
        TintKind::Foliage
    } else {
        TintKind::Grass
    };
    world.biome_tint(cell[0], cell[1], cell[2], kind)
}

fn door_visual(offset: usize) -> (&'static str, f32) {
    let facing = (offset / 16).min(3);
    let upper = (offset % 16) / 8 == 0;
    let right = (offset % 8) / 4 == 1;
    let open = (offset % 4) / 2 == 0;
    let variant = match (upper, right, open) {
        (false, false, false) => "bottom_left",
        (false, false, true) => "bottom_left_open",
        (false, true, false) => "bottom_right",
        (false, true, true) => "bottom_right_open",
        (true, false, false) => "top_left",
        (true, false, true) => "top_left_open",
        (true, true, false) => "top_right",
        (true, true, true) => "top_right_open",
    };
    let yaw = if open {
        if right {
            [180.0, 0.0, 90.0, 270.0][facing]
        } else {
            [0.0, 180.0, 270.0, 90.0][facing]
        }
    } else {
        [270.0, 90.0, 180.0, 0.0][facing]
    };
    (variant, yaw)
}

fn trapdoor_visual(offset: usize) -> (&'static str, f32) {
    let facing = (offset / 16).min(3);
    let top = (offset % 16) / 8 == 0;
    let open = (offset % 8) / 4 == 0;
    if open {
        ("open", [0.0, 180.0, 270.0, 90.0][facing])
    } else if top {
        ("top", 0.0)
    } else {
        ("bottom", 0.0)
    }
}

fn rail_visual(bare: &str, offset: usize) -> (&'static str, f32) {
    let (shape, powered) = if bare == "rail" {
        ((offset / 2).min(9), false)
    } else {
        (((offset % 12) / 2).min(5), offset / 12 == 0)
    };
    match shape {
        0 => (if powered { "on" } else { "base" }, 0.0),
        1 => (if powered { "on" } else { "base" }, 90.0),
        2 => (if powered { "on_raised_ne" } else { "raised_ne" }, 90.0),
        3 => (if powered { "on_raised_sw" } else { "raised_sw" }, 90.0),
        4 => (if powered { "on_raised_ne" } else { "raised_ne" }, 0.0),
        5 => (if powered { "on_raised_sw" } else { "raised_sw" }, 0.0),
        6 => ("corner", 0.0),
        7 => ("corner", 90.0),
        8 => ("corner", 180.0),
        9 => ("corner", 270.0),
        _ => (if powered { "on" } else { "base" }, 0.0),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RedstoneVisual {
    /// East, north, south, west: 0=up, 1=side, 2=none.
    connections: [usize; 4],
    power: usize,
    dot: bool,
}

fn redstone_visual(offset: usize) -> RedstoneVisual {
    let east = offset / 432;
    let north = (offset % 432) / 144;
    let power = (offset % 144) / 9;
    let south = (offset % 9) / 3;
    let west = offset % 3;
    let connected = |value: usize| value != 2;
    let dot = (!connected(east) && !connected(north) && !connected(south) && !connected(west))
        || (connected(east) && connected(north))
        || (connected(east) && connected(south))
        || (connected(south) && connected(west))
        || (connected(north) && connected(west));
    RedstoneVisual {
        connections: [east, north, south, west],
        power,
        dot,
    }
}

fn redstone_tint(power: usize) -> [f32; 3] {
    let level = power.min(15) as f32;
    let f = level / 15.0;
    let red = if power == 0 { 0.3 } else { f * 0.6 + 0.4 };
    let green = (f * f * 0.7 - 0.5).max(0.0);
    let blue = (f * f * 0.6 - 0.7).max(0.0);
    [red, green, blue]
}

fn furnace_visual(offset: usize) -> (&'static str, f32) {
    let facing = (offset / 2).min(3);
    let lit = offset.is_multiple_of(2);
    (
        if lit { "on" } else { "off" },
        [0.0, 180.0, 270.0, 90.0][facing],
    )
}

fn axis_rotation(name: &str, offset: usize) -> Option<[f32; 3]> {
    let bare = name.strip_prefix("minecraft:").unwrap_or(name);
    let has_axis = bare.ends_with("_log")
        || bare.ends_with("_wood")
        || bare.ends_with("_stem")
        || bare.ends_with("_hyphae")
        || bare.ends_with("_pillar")
        || matches!(
            bare,
            "hay_block"
                | "bone_block"
                | "basalt"
                | "polished_basalt"
                | "deepslate"
                | "infested_deepslate"
        );
    has_axis.then(|| match offset.min(2) {
        0 => [0.0, 0.0, -90.0], // vertical model axis Y -> world X
        1 => [0.0, 0.0, 0.0],
        _ => [90.0, 0.0, 0.0], // vertical model axis Y -> world Z
    })
}

fn horizontal_rotation(name: &str, offset: usize) -> Option<[f32; 3]> {
    let bare = name.strip_prefix("minecraft:").unwrap_or(name);
    let stride = match bare {
        "bee_nest" | "beehive" => 6,
        "lectern" => 4,
        "ladder" | "end_portal_frame" => 2,
        "carved_pumpkin" | "jack_o_lantern" | "loom" | "stonecutter" => 1,
        _ if bare.ends_with("_glazed_terracotta") || bare.ends_with("anvil") => 1,
        _ => return None,
    };
    let facing = (offset / stride).min(3);
    let yaw = if bare.ends_with("_glazed_terracotta") || bare.ends_with("anvil") {
        [180.0, 0.0, 90.0, 270.0][facing]
    } else {
        [0.0, 180.0, 270.0, 90.0][facing]
    };
    Some([0.0, yaw, 0.0])
}

fn campfire_visual(offset: usize) -> (&'static str, f32) {
    let facing = (offset / 8).min(3);
    let lit = (offset % 8) / 4 == 0;
    (
        if lit { "on" } else { "off" },
        [180.0, 0.0, 90.0, 270.0][facing],
    )
}

/// Builds a textured mesh for the inclusive world-coordinate box `[min, max]`.
pub fn mesh_region(world: &World, atlas: &Atlas, min: [i32; 3], max: [i32; 3]) -> Mesh {
    let mut vertices = Vec::new();
    for x in min[0]..=max[0] {
        for y in min[1]..=max[1] {
            for z in min[2]..=max[2] {
                let Some(state) = world.block_state(x, y, z) else {
                    continue;
                };
                if crab_registry::is_air(state) {
                    continue;
                }
                let name = crab_registry::block_name(state).unwrap_or("");
                let base = [x as f32, y as f32, z as f32];

                if let Some(parts) = atlas.block_state_model(state) {
                    for (part_index, part) in parts.iter().enumerate() {
                        if let Some(model) = select_state_alternative(part, [x, y, z], part_index) {
                            emit_state_elements(
                                &mut vertices,
                                world,
                                atlas,
                                base,
                                [x, y, z],
                                &model.elements,
                                model.rotation,
                                model.uvlock,
                            );
                        }
                    }
                    continue;
                }

                if name.ends_with("_wall") {
                    if let Some(block) = crab_registry::block_for_state(state) {
                        let offset = (state - block.min_state) as usize;
                        let levels = [
                            offset / 108,
                            (offset % 108) / 36,
                            (offset % 36) / 12,
                            offset % 3,
                        ];
                        let up = (offset % 12) / 6 == 0;
                        if up {
                            if let Some(post) = atlas.block_elements_variant(name, "post") {
                                emit_elements(
                                    &mut vertices,
                                    world,
                                    atlas,
                                    base,
                                    [x, y, z],
                                    post,
                                    [0.0, 0.0, 0.0],
                                );
                            }
                        }
                        for (level, yaw) in levels.into_iter().zip([90.0, 0.0, 180.0, 270.0]) {
                            let variant = match level {
                                1 => "side",
                                2 => "side_tall",
                                _ => continue,
                            };
                            if let Some(side) = atlas.block_elements_variant(name, variant) {
                                emit_elements(
                                    &mut vertices,
                                    world,
                                    atlas,
                                    base,
                                    [x, y, z],
                                    side,
                                    [0.0, yaw, 0.0],
                                );
                            }
                        }
                        continue;
                    }
                }

                if name.ends_with("_fence")
                    || name.ends_with("_pane")
                    || name == "minecraft:iron_bars"
                {
                    if let Some(post) = atlas.block_elements_variant(name, "post") {
                        emit_elements(
                            &mut vertices,
                            world,
                            atlas,
                            base,
                            [x, y, z],
                            post,
                            [0.0, 0.0, 0.0],
                        );
                        if let Some(side) = atlas.block_elements_variant(name, "side") {
                            let Some(block) = crab_registry::block_for_state(state) else {
                                continue;
                            };
                            let offset = (state - block.min_state) as usize;
                            let arms = [
                                (offset / 16).is_multiple_of(2),
                                (offset / 8).is_multiple_of(2),
                                (offset / 4).is_multiple_of(2),
                                offset.is_multiple_of(2),
                            ];
                            for (connected, yaw) in arms.into_iter().zip([90.0, 0.0, 180.0, 270.0])
                            {
                                if connected {
                                    emit_elements(
                                        &mut vertices,
                                        world,
                                        atlas,
                                        base,
                                        [x, y, z],
                                        side,
                                        [0.0, yaw, 0.0],
                                    );
                                }
                            }
                        }
                        continue;
                    }
                }

                if name.ends_with("_door") && !name.ends_with("_trapdoor") {
                    if let Some(block) = crab_registry::block_for_state(state) {
                        let (variant, yaw) = door_visual((state - block.min_state) as usize);
                        if let Some(elements) = atlas.block_elements_variant(name, variant) {
                            emit_elements(
                                &mut vertices,
                                world,
                                atlas,
                                base,
                                [x, y, z],
                                elements,
                                [0.0, yaw, 0.0],
                            );
                            continue;
                        }
                    }
                }

                if name.ends_with("_trapdoor") {
                    if let Some(block) = crab_registry::block_for_state(state) {
                        let (variant, yaw) = trapdoor_visual((state - block.min_state) as usize);
                        if let Some(elements) = atlas.block_elements_variant(name, variant) {
                            emit_elements(
                                &mut vertices,
                                world,
                                atlas,
                                base,
                                [x, y, z],
                                elements,
                                [0.0, yaw, 0.0],
                            );
                            continue;
                        }
                    }
                }

                if name.ends_with("rail") {
                    if let Some(block) = crab_registry::block_for_state(state) {
                        let bare = name.strip_prefix("minecraft:").unwrap_or(name);
                        let (variant, yaw) = rail_visual(bare, (state - block.min_state) as usize);
                        let elements = if variant == "base" {
                            atlas.block_elements(name)
                        } else {
                            atlas.block_elements_variant(name, variant)
                        };
                        if let Some(elements) = elements {
                            emit_elements(
                                &mut vertices,
                                world,
                                atlas,
                                base,
                                [x, y, z],
                                elements,
                                [0.0, yaw, 0.0],
                            );
                            continue;
                        }
                    }
                }

                if name == "minecraft:redstone_wire" {
                    if let Some(block) = crab_registry::block_for_state(state) {
                        let visual = redstone_visual((state - block.min_state) as usize);
                        let tint = redstone_tint(visual.power);
                        if visual.dot {
                            if let Some(elements) = atlas.block_elements_variant(name, "dot") {
                                emit_elements_tinted(
                                    &mut vertices,
                                    world,
                                    atlas,
                                    base,
                                    [x, y, z],
                                    elements,
                                    [0.0, 0.0, 0.0],
                                    false,
                                    Some(tint),
                                );
                            }
                        }
                        for ((connection, variant), yaw) in visual
                            .connections
                            .into_iter()
                            .zip(["east", "north", "south", "west"])
                            .zip([270.0, 0.0, 0.0, 270.0])
                        {
                            if connection == 2 {
                                continue;
                            }
                            if let Some(elements) = atlas.block_elements_variant(name, variant) {
                                emit_elements_tinted(
                                    &mut vertices,
                                    world,
                                    atlas,
                                    base,
                                    [x, y, z],
                                    elements,
                                    [0.0, yaw, 0.0],
                                    false,
                                    Some(tint),
                                );
                            }
                            if connection == 0 {
                                if let Some(elements) = atlas.block_elements_variant(name, "up") {
                                    let up_yaw = match variant {
                                        "east" => 90.0,
                                        "south" => 180.0,
                                        "west" => 270.0,
                                        _ => 0.0,
                                    };
                                    emit_elements_tinted(
                                        &mut vertices,
                                        world,
                                        atlas,
                                        base,
                                        [x, y, z],
                                        elements,
                                        [0.0, up_yaw, 0.0],
                                        false,
                                        Some(tint),
                                    );
                                }
                            }
                        }
                        continue;
                    }
                }

                if matches!(
                    name,
                    "minecraft:furnace" | "minecraft:blast_furnace" | "minecraft:smoker"
                ) {
                    if let Some(block) = crab_registry::block_for_state(state) {
                        let (variant, yaw) = furnace_visual((state - block.min_state) as usize);
                        if let Some(elements) = atlas.block_elements_variant(name, variant) {
                            emit_elements(
                                &mut vertices,
                                world,
                                atlas,
                                base,
                                [x, y, z],
                                elements,
                                [0.0, yaw, 0.0],
                            );
                            continue;
                        }
                    }
                }

                if matches!(name, "minecraft:campfire" | "minecraft:soul_campfire") {
                    if let Some(block) = crab_registry::block_for_state(state) {
                        let (variant, yaw) = campfire_visual((state - block.min_state) as usize);
                        if let Some(elements) = atlas.block_elements_variant(name, variant) {
                            emit_elements(
                                &mut vertices,
                                world,
                                atlas,
                                base,
                                [x, y, z],
                                elements,
                                [0.0, yaw, 0.0],
                            );
                            continue;
                        }
                    }
                }

                // Non-cube blocks (slabs, stairs, fences, plants, …) emit their
                // model's element geometry.
                if let Some(default_elements) = atlas.block_elements(name) {
                    let mut elements = default_elements;
                    let mut model_rotation = crab_registry::block_for_state(state)
                        .and_then(|block| {
                            horizontal_rotation(name, (state - block.min_state) as usize)
                        })
                        .unwrap_or([0.0, 0.0, 0.0]);
                    if name.ends_with("_stairs") {
                        if let Some(block) = crab_registry::block_for_state(state) {
                            let offset = (state - block.min_state) as usize;
                            let facing = offset / 20;
                            let half = (offset % 20) / 10;
                            let shape = (offset % 10) / 2;
                            let variant = match shape {
                                1 | 2 => "inner",
                                3 | 4 => "outer",
                                _ => "straight",
                            };
                            if variant != "straight" {
                                elements = atlas
                                    .block_elements_variant(name, variant)
                                    .unwrap_or(default_elements);
                            }
                            model_rotation = [
                                if half == 0 { 180.0 } else { 0.0 },
                                [270.0, 90.0, 180.0, 0.0][facing.min(3)],
                                0.0,
                            ];
                        }
                    }
                    emit_elements(
                        &mut vertices,
                        world,
                        atlas,
                        base,
                        [x, y, z],
                        elements,
                        model_rotation,
                    );
                    continue;
                }

                // Full cube (or flat fallback): one quad per non-occluded face.
                let model = atlas.model(name);
                let rotation = crab_registry::block_for_state(state)
                    .and_then(|block| {
                        let offset = (state - block.min_state) as usize;
                        axis_rotation(name, offset).or_else(|| horizontal_rotation(name, offset))
                    })
                    .unwrap_or([0.0, 0.0, 0.0]);
                for (fi, face) in FACES.iter().enumerate() {
                    let normal = rotate_model(face.normal, rotation, false);
                    let direction = [
                        normal[0].round() as i32,
                        normal[1].round() as i32,
                        normal[2].round() as i32,
                    ];
                    let neighbor =
                        world.block_state(x + direction[0], y + direction[1], z + direction[2]);
                    if neighbor.is_some_and(|s| occludes(atlas, s)) {
                        continue;
                    }
                    let tex = model.faces[fi];
                    let [u0, v0, u1, v1] = tex.uv;
                    // two triangles: (0,1,2) and (0,2,3)
                    for &ci in &[0usize, 1, 2, 0, 2, 3] {
                        let c = rotate_model(face.corners[ci], rotation, true);
                        let [cu, cv] = face.uvs[ci];
                        vertices.push(Vertex {
                            position: [base[0] + c[0], base[1] + c[1], base[2] + c[2]],
                            normal,
                            uv: [u0 + cu * (u1 - u0), v0 + cv * (v1 - v0)],
                            tint: biome_tint(world, name, [x, y, z], tex.tint),
                            opacity: if name == "minecraft:water" {
                                0.58
                            } else if name == "minecraft:lava" {
                                0.88
                            } else {
                                1.0
                            },
                        });
                    }
                }
            }
        }
    }
    Mesh { vertices }
}

/// Rotates a normalized (0..1) point about an [`crab_assets::ElementRotation`].
fn rotate_point(p: [f32; 3], rot: &crab_assets::ElementRotation) -> [f32; 3] {
    let o = [
        rot.origin[0] / 16.0,
        rot.origin[1] / 16.0,
        rot.origin[2] / 16.0,
    ];
    let a = rot.angle.to_radians();
    let (s, c) = (a.sin(), a.cos());
    let mut d = [p[0] - o[0], p[1] - o[1], p[2] - o[2]];
    let rescale = if rot.rescale { 1.0 / c.abs() } else { 1.0 };
    match rot.axis {
        0 => {
            let (yy, zz) = (d[1], d[2]);
            d[1] = (yy * c - zz * s) * rescale;
            d[2] = (yy * s + zz * c) * rescale;
        }
        2 => {
            let (xx, yy) = (d[0], d[1]);
            d[0] = (xx * c - yy * s) * rescale;
            d[1] = (xx * s + yy * c) * rescale;
        }
        _ => {
            let (xx, zz) = (d[0], d[2]);
            d[0] = (xx * c + zz * s) * rescale;
            d[2] = (-xx * s + zz * c) * rescale;
        }
    }
    [o[0] + d[0], o[1] + d[1], o[2] + d[2]]
}

/// Emits one block's element geometry (used for non-full-cube models).
fn emit_elements(
    verts: &mut Vec<Vertex>,
    world: &World,
    atlas: &Atlas,
    base: [f32; 3],
    cell: [i32; 3],
    elements: &[crab_assets::ElementData],
    model_rotation: [f32; 3],
) {
    emit_elements_tinted(
        verts,
        world,
        atlas,
        base,
        cell,
        elements,
        model_rotation,
        false,
        None,
    );
}

#[allow(clippy::too_many_arguments)]
fn emit_state_elements(
    verts: &mut Vec<Vertex>,
    world: &World,
    atlas: &Atlas,
    base: [f32; 3],
    cell: [i32; 3],
    elements: &[crab_assets::ElementData],
    model_rotation: [f32; 3],
    uvlock: bool,
) {
    emit_elements_tinted(
        verts,
        world,
        atlas,
        base,
        cell,
        elements,
        model_rotation,
        uvlock,
        None,
    );
}

#[allow(clippy::too_many_arguments)]
fn emit_elements_tinted(
    verts: &mut Vec<Vertex>,
    world: &World,
    atlas: &Atlas,
    base: [f32; 3],
    cell: [i32; 3],
    elements: &[crab_assets::ElementData],
    model_rotation: [f32; 3],
    uvlock: bool,
    tint_override: Option<[f32; 3]>,
) {
    let block_name = world
        .block_state(cell[0], cell[1], cell[2])
        .and_then(crab_registry::block_name)
        .unwrap_or("minecraft:air");
    for el in elements {
        let from_n = [el.from[0] / 16.0, el.from[1] / 16.0, el.from[2] / 16.0];
        let to_n = [el.to[0] / 16.0, el.to[1] / 16.0, el.to[2] / 16.0];
        for (fi, face) in FACES.iter().enumerate() {
            let Some(ef) = el.faces[fi] else {
                continue;
            };
            // Cull this face only when a cullface neighbour is a full cube.
            if let Some(cd) = ef.cull {
                let direction = rotate_model(FACES[cd as usize].normal, model_rotation, false);
                let d = direction.map(|value| value.round() as i32);
                let n = world.block_state(cell[0] + d[0], cell[1] + d[1], cell[2] + d[2]);
                if n.is_some_and(|s| occludes(atlas, s)) {
                    continue;
                }
            }
            let [su0, sv0, su1, sv1] = ef.uv;
            let mut normal = face.normal;
            if let Some(rotation) = &el.rotation {
                normal = rotate_element_normal(normal, rotation);
            }
            normal = rotate_model(normal, model_rotation, false);
            for &ci in &[0usize, 1, 2, 0, 2, 3] {
                let uc = face.corners[ci];
                let mut p = [
                    if uc[0] == 0.0 { from_n[0] } else { to_n[0] },
                    if uc[1] == 0.0 { from_n[1] } else { to_n[1] },
                    if uc[2] == 0.0 { from_n[2] } else { to_n[2] },
                ];
                if let Some(rot) = &el.rotation {
                    p = rotate_point(p, rot);
                }
                p = rotate_model(p, model_rotation, true);
                let [mut cu, mut cv] = face.uvs[ci];
                if uvlock {
                    (cu, cv) = uvlock_coordinates(fi, model_rotation, cu, cv);
                }
                for _ in 0..ef.uv_rotation {
                    (cu, cv) = (1.0 - cv, cu);
                }
                verts.push(Vertex {
                    position: [base[0] + p[0], base[1] + p[1], base[2] + p[2]],
                    normal,
                    uv: [su0 + cu * (su1 - su0), sv0 + cv * (sv1 - sv0)],
                    tint: tint_override
                        .unwrap_or_else(|| biome_tint(world, block_name, cell, ef.tint)),
                    opacity: 1.0,
                });
            }
        }
    }
}

fn face_texture_basis(face: usize) -> ([f32; 3], [f32; 3]) {
    match face {
        0 => ([0.0, 0.0, 1.0], [0.0, -1.0, 0.0]),
        1 => ([0.0, 0.0, -1.0], [0.0, -1.0, 0.0]),
        2 => ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        3 => ([1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
        4 => ([-1.0, 0.0, 0.0], [0.0, -1.0, 0.0]),
        _ => ([1.0, 0.0, 0.0], [0.0, -1.0, 0.0]),
    }
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn uvlock_coordinates(source_face: usize, model_rotation: [f32; 3], u: f32, v: f32) -> (f32, f32) {
    let normal = rotate_model(FACES[source_face].normal, model_rotation, false);
    let target_face = FACES
        .iter()
        .enumerate()
        .max_by(|(_, left), (_, right)| {
            dot(normal, left.normal).total_cmp(&dot(normal, right.normal))
        })
        .map_or(source_face, |(index, _)| index);
    let (source_u, source_v) = face_texture_basis(source_face);
    let rotated_u = rotate_model(source_u, model_rotation, false);
    let rotated_v = rotate_model(source_v, model_rotation, false);
    let (target_u, target_v) = face_texture_basis(target_face);
    let du = u - 0.5;
    let dv = v - 0.5;
    (
        0.5 + du * dot(rotated_u, target_u) + dv * dot(rotated_v, target_u),
        0.5 + du * dot(rotated_u, target_v) + dv * dot(rotated_v, target_v),
    )
}

fn rotate_element_normal(
    mut normal: [f32; 3],
    rotation: &crab_assets::ElementRotation,
) -> [f32; 3] {
    let angle = rotation.angle.to_radians();
    let (sin, cos) = angle.sin_cos();
    match rotation.axis {
        0 => {
            let (y, z) = (normal[1], normal[2]);
            normal[1] = y * cos - z * sin;
            normal[2] = y * sin + z * cos;
        }
        2 => {
            let (x, y) = (normal[0], normal[1]);
            normal[0] = x * cos - y * sin;
            normal[1] = x * sin + y * cos;
        }
        _ => {
            let (x, z) = (normal[0], normal[2]);
            normal[0] = x * cos + z * sin;
            normal[2] = -x * sin + z * cos;
        }
    }
    normal
}

fn rotate_model(mut point: [f32; 3], rotation: [f32; 3], around_center: bool) -> [f32; 3] {
    let center = if around_center { 0.5 } else { 0.0 };
    point[0] -= center;
    point[1] -= center;
    point[2] -= center;
    let (sx, cx) = rotation[0].to_radians().sin_cos();
    let (y, z) = (point[1], point[2]);
    point[1] = y * cx - z * sx;
    point[2] = y * sx + z * cx;
    let (sy, cy) = rotation[1].to_radians().sin_cos();
    let (x, z) = (point[0], point[2]);
    point[0] = x * cy + z * sy;
    point[2] = -x * sy + z * cy;
    let (sz, cz) = rotation[2].to_radians().sin_cos();
    let (x, y) = (point[0], point[1]);
    point[0] = x * cz - y * sz;
    point[1] = x * sz + y * cz;
    point[0] += center;
    point[1] += center;
    point[2] += center;
    point
}

/// Builds a 36-vertex box spanning `[min, max]`, textured with `uv` (an atlas
/// rect, e.g. a white tile) tinted by `tint`. Used for entity boxes.
pub fn box_mesh(min: [f32; 3], max: [f32; 3], uv: [f32; 4], tint: [f32; 3]) -> Vec<Vertex> {
    let [u0, v0, u1, v1] = uv;
    let mut verts = Vec::with_capacity(36);
    for face in &FACES {
        for &ci in &[0usize, 1, 2, 0, 2, 3] {
            let c = face.corners[ci];
            let [cu, cv] = face.uvs[ci];
            verts.push(Vertex {
                position: [
                    min[0] + c[0] * (max[0] - min[0]),
                    min[1] + c[1] * (max[1] - min[1]),
                    min[2] + c[2] * (max[2] - min[2]),
                ],
                normal: face.normal,
                uv: [u0 + cu * (u1 - u0), v0 + cv * (v1 - v0)],
                tint,
                opacity: 1.0,
            });
        }
    }
    verts
}

/// Builds a miniature six-face block model for dropped block items.
#[must_use]
pub fn block_item_mesh(
    atlas: &Atlas,
    block_name: &str,
    center: [f32; 3],
    size: f32,
    yaw_degrees: f32,
) -> Vec<Vertex> {
    let (sin_yaw, cos_yaw) = yaw_degrees.to_radians().sin_cos();
    if let Some(elements) = atlas.block_elements(block_name) {
        return transformed_block_elements(elements, center, size, yaw_degrees, [0.0; 3], false);
    }
    let model = atlas.model(block_name);
    let mut vertices = Vec::with_capacity(36);
    for (face_index, face) in FACES.iter().enumerate() {
        let texture = model.faces[face_index];
        let [u0, v0, u1, v1] = texture.uv;
        for &corner_index in &[0usize, 1, 2, 0, 2, 3] {
            let corner = face.corners[corner_index];
            let [u, v] = face.uvs[corner_index];
            let local_x = (corner[0] - 0.5) * size;
            let local_y = (corner[1] - 0.5) * size;
            let local_z = (corner[2] - 0.5) * size;
            vertices.push(Vertex {
                position: [
                    center[0] + local_x * cos_yaw + local_z * sin_yaw,
                    center[1] + local_y,
                    center[2] - local_x * sin_yaw + local_z * cos_yaw,
                ],
                normal: [
                    face.normal[0] * cos_yaw + face.normal[2] * sin_yaw,
                    face.normal[1],
                    -face.normal[0] * sin_yaw + face.normal[2] * cos_yaw,
                ],
                uv: [u0 + u * (u1 - u0), v0 + v * (v1 - v0)],
                tint: texture.tint,
                opacity: 1.0,
            });
        }
    }
    vertices
}

/// Builds a block mesh using the exact global block state, so falling stairs,
/// slabs, multipart blocks, and rotated variants retain their server-supplied
/// shape. Falls back to the legacy block model when the pack has no matching
/// blockstate definition.
#[must_use]
pub fn block_state_item_mesh(
    atlas: &Atlas,
    state: u32,
    center: [f32; 3],
    size: f32,
    yaw_degrees: f32,
) -> Vec<Vertex> {
    if let Some(parts) = atlas.block_state_model(state) {
        let cell = center.map(|coordinate| coordinate.floor() as i32);
        let mut vertices = Vec::new();
        for (part_index, part) in parts.iter().enumerate() {
            if let Some(model) = select_state_alternative(part, cell, part_index) {
                vertices.extend(transformed_block_elements(
                    &model.elements,
                    center,
                    size,
                    yaw_degrees,
                    model.rotation,
                    model.uvlock,
                ));
            }
        }
        return vertices;
    }
    crab_registry::block_name(state)
        .map(|name| block_item_mesh(atlas, name, center, size, yaw_degrees))
        .unwrap_or_default()
}

fn transformed_block_elements(
    elements: &[crab_assets::ElementData],
    center: [f32; 3],
    size: f32,
    yaw_degrees: f32,
    model_rotation: [f32; 3],
    uvlock: bool,
) -> Vec<Vertex> {
    let (sin_yaw, cos_yaw) = yaw_degrees.to_radians().sin_cos();
    let mut vertices = Vec::new();
    for element in elements {
        let from = element.from.map(|coordinate| coordinate / 16.0);
        let to = element.to.map(|coordinate| coordinate / 16.0);
        for (face_index, face) in FACES.iter().enumerate() {
            let Some(texture) = element.faces[face_index] else {
                continue;
            };
            let [u0, v0, u1, v1] = texture.uv;
            let mut normal = face.normal;
            if let Some(rotation) = &element.rotation {
                normal = rotate_element_normal(normal, rotation);
            }
            normal = rotate_model(normal, model_rotation, false);
            normal = [
                normal[0] * cos_yaw + normal[2] * sin_yaw,
                normal[1],
                -normal[0] * sin_yaw + normal[2] * cos_yaw,
            ];
            for &corner_index in &[0usize, 1, 2, 0, 2, 3] {
                let corner = face.corners[corner_index];
                let mut point = [
                    if corner[0] == 0.0 { from[0] } else { to[0] },
                    if corner[1] == 0.0 { from[1] } else { to[1] },
                    if corner[2] == 0.0 { from[2] } else { to[2] },
                ];
                if let Some(rotation) = &element.rotation {
                    point = rotate_point(point, rotation);
                }
                point = rotate_model(point, model_rotation, true);
                let local = point.map(|coordinate| (coordinate - 0.5) * size);
                let [mut u, mut v] = face.uvs[corner_index];
                if uvlock {
                    (u, v) = uvlock_coordinates(face_index, model_rotation, u, v);
                }
                for _ in 0..texture.uv_rotation {
                    (u, v) = (1.0 - v, u);
                }
                vertices.push(Vertex {
                    position: [
                        center[0] + local[0] * cos_yaw + local[2] * sin_yaw,
                        center[1] + local[1],
                        center[2] - local[0] * sin_yaw + local[2] * cos_yaw,
                    ],
                    normal,
                    uv: [u0 + u * (u1 - u0), v0 + v * (v1 - v0)],
                    tint: texture.tint,
                    opacity: 1.0,
                });
            }
        }
    }
    vertices
}

/// Builds a dropped-item mesh from the resolved `models/item/<name>.json`
/// geometry and its inherited vanilla `ground` display transform. Flat
/// generated items return `None` and should use their atlas billboard instead.
#[must_use]
pub fn item_model_mesh(
    atlas: &crab_assets::ItemAtlas,
    item_name: &str,
    center: [f32; 3],
    yaw_degrees: f32,
) -> Option<Vec<Vertex>> {
    let model = atlas.model(item_name)?;
    let (sin_yaw, cos_yaw) = yaw_degrees.to_radians().sin_cos();
    let mut vertices = Vec::new();
    for element in &model.elements {
        let from = element.from.map(|coordinate| coordinate / 16.0);
        let to = element.to.map(|coordinate| coordinate / 16.0);
        for (face_index, face) in FACES.iter().enumerate() {
            let Some(texture) = element.faces[face_index] else {
                continue;
            };
            let [u0, v0, u1, v1] = texture.uv;
            let mut normal = face.normal;
            if let Some(rotation) = &element.rotation {
                normal = rotate_element_normal(normal, rotation);
            }
            normal = rotate_model(normal, model.ground.rotation, false);
            normal = [
                normal[0] * cos_yaw + normal[2] * sin_yaw,
                normal[1],
                -normal[0] * sin_yaw + normal[2] * cos_yaw,
            ];
            for &corner_index in &[0usize, 1, 2, 0, 2, 3] {
                let corner = face.corners[corner_index];
                let mut point = [
                    if corner[0] == 0.0 { from[0] } else { to[0] },
                    if corner[1] == 0.0 { from[1] } else { to[1] },
                    if corner[2] == 0.0 { from[2] } else { to[2] },
                ];
                if let Some(rotation) = &element.rotation {
                    point = rotate_point(point, rotation);
                }
                point = [
                    (point[0] - 0.5) * model.ground.scale[0],
                    (point[1] - 0.5) * model.ground.scale[1],
                    (point[2] - 0.5) * model.ground.scale[2],
                ];
                point = rotate_model(point, model.ground.rotation, false);
                point = [
                    point[0] + model.ground.translation[0] / 16.0,
                    point[1] + model.ground.translation[1] / 16.0,
                    point[2] + model.ground.translation[2] / 16.0,
                ];
                let [mut u, mut v] = face.uvs[corner_index];
                for _ in 0..texture.uv_rotation {
                    (u, v) = (1.0 - v, u);
                }
                vertices.push(Vertex {
                    position: [
                        center[0] + point[0] * cos_yaw + point[2] * sin_yaw,
                        center[1] + point[1],
                        center[2] - point[0] * sin_yaw + point[2] * cos_yaw,
                    ],
                    normal,
                    uv: [u0 + u * (u1 - u0), v0 + v * (v1 - v0)],
                    tint: texture.tint,
                    opacity: 1.0,
                });
            }
        }
    }
    Some(vertices)
}

fn push_quad(verts: &mut Vec<Vertex>, c: [[f32; 3]; 4], normal: [f32; 3], uv: [[f32; 2]; 4]) {
    for &i in &[0usize, 1, 2, 0, 2, 3] {
        verts.push(Vertex {
            position: c[i],
            normal,
            uv: uv[i],
            tint: [1.0, 1.0, 1.0],
            opacity: 1.0,
        });
    }
}

/// Rotates `p` about `pivot` by Euler `deg` (X, then Y, then Z).
fn rotate_euler(p: [f32; 3], pivot: [f32; 3], deg: [f32; 3]) -> [f32; 3] {
    let mut d = [p[0] - pivot[0], p[1] - pivot[1], p[2] - pivot[2]];
    if deg[0] != 0.0 {
        let a = deg[0].to_radians();
        let (s, c) = (a.sin(), a.cos());
        let (y, z) = (d[1], d[2]);
        d[1] = y * c - z * s;
        d[2] = y * s + z * c;
    }
    if deg[1] != 0.0 {
        let a = deg[1].to_radians();
        let (s, c) = (a.sin(), a.cos());
        let (x, z) = (d[0], d[2]);
        d[0] = x * c + z * s;
        d[2] = -x * s + z * c;
    }
    if deg[2] != 0.0 {
        let a = deg[2].to_radians();
        let (s, c) = (a.sin(), a.cos());
        let (x, y) = (d[0], d[1]);
        d[0] = x * c - y * s;
        d[1] = x * s + y * c;
    }
    [pivot[0] + d[0], pivot[1] + d[1], pivot[2] + d[2]]
}

/// Procedural walk swing (degrees about X) for a limb bone, or 0 for others.
/// `swing` is the walk-cycle phase in radians; `amount` scales with speed.
fn limb_swing_deg(name: &str, swing: f32, amount: f32) -> f32 {
    let n = name.to_ascii_lowercase();
    let is_leg = n.contains("leg");
    let is_arm = n.contains("arm");
    if !is_leg && !is_arm {
        return 0.0;
    }
    // Side/phase: explicit left/right, else trailing-digit (quadruped diagonals).
    let mut sign = if n.contains("left") {
        -1.0
    } else if n.contains("right") {
        1.0
    } else if let Some(d) = n.bytes().rev().find(u8::is_ascii_digit) {
        if matches!(d, b'0' | b'3') {
            1.0
        } else {
            -1.0
        }
    } else {
        1.0
    };
    if is_arm {
        sign = -sign; // arms swing opposite the legs
    }
    (swing.sin() * amount * sign).to_degrees() * 1.2
}

/// Builds a standing 3D mesh for a Bedrock entity geometry. This compatibility
/// entry point is used by examples and previews; live entities use
/// [`entity_mesh_with_pose`] with their server metadata pose.
///
/// The entity's texture occupies the rectangle at `uv_origin` (pixels) within a
/// texture of size `uv_size` (pixels) — pass `([0,0], [tex_w, tex_h])` for a
/// dedicated texture, or the atlas placement for a shared entity atlas.
#[allow(clippy::too_many_arguments)]
pub fn entity_mesh(
    geo: &crab_assets::EntityGeometry,
    offset: [f32; 3],
    uv_origin: [f32; 2],
    uv_size: [f32; 2],
    limb_swing: f32,
    limb_amount: f32,
    scale: f32,
    yaw_deg: f32,
    head_yaw_deg: f32,
    attack_progress: f32,
) -> Vec<Vertex> {
    entity_mesh_with_pose(
        geo,
        offset,
        uv_origin,
        uv_size,
        limb_swing,
        limb_amount,
        scale,
        yaw_deg,
        head_yaw_deg,
        attack_progress,
        0,
    )
}

/// Builds a posed 3D entity mesh, feet at `offset`. Each bone's rest rotation
/// is combined with procedural walking/attacking and the authoritative Pose
/// metadata value. Pose IDs follow Java's stable enum ordering: fall-flying 1,
/// sleeping 2, swimming 3, spin attack 4, crouching 5, dying 7, sitting 10.
#[allow(clippy::too_many_arguments)]
pub fn entity_mesh_with_pose(
    geo: &crab_assets::EntityGeometry,
    offset: [f32; 3],
    uv_origin: [f32; 2],
    uv_size: [f32; 2],
    limb_swing: f32,
    limb_amount: f32,
    scale: f32,
    yaw_deg: f32,
    head_yaw_deg: f32,
    attack_progress: f32,
    pose: i32,
) -> Vec<Vertex> {
    // Whole-model facing: Minecraft yaw 0 = south (+Z) and increases clockwise
    // (90 = west/-X). Our model's front (head) is at -Z and we mirror X, so the
    // spin that lands yaw 0 facing +Z (calibrated by render) is `-yaw`.
    let (yaw_sin, yaw_cos) = (-yaw_deg).to_radians().sin_cos();
    let (sw, sh) = (uv_size[0].max(1.0), uv_size[1].max(1.0));
    let (ox, oy) = (uv_origin[0], uv_origin[1]);
    let uvr = |x0: f32, y0: f32, x1: f32, y1: f32| {
        [
            [(ox + x0) / sw, (oy + y0) / sh],
            [(ox + x1) / sw, (oy + y0) / sh],
            [(ox + x1) / sw, (oy + y1) / sh],
            [(ox + x0) / sw, (oy + y1) / sh],
        ]
    };
    let mut verts = Vec::new();
    for bone in &geo.bones {
        let swing = limb_swing_deg(&bone.name, limb_swing, limb_amount);
        // Minecraft's bone rotation is the opposite sense to our math rotation,
        // so negate the rest rotation (e.g. the cow body's 90° tilt connects to
        // the head/legs). The swing keeps our sign (it's symmetric anyway).
        let head_turn = if bone.name.to_ascii_lowercase().contains("head") {
            head_yaw_deg - yaw_deg
        } else {
            0.0
        };
        let lower_name = bone.name.to_ascii_lowercase();
        let attack_swing = if lower_name.contains("arm")
            && (lower_name.contains("right") || lower_name.ends_with('2'))
        {
            -((1.0 - attack_progress.clamp(0.0, 1.0)) * std::f32::consts::PI).sin() * 85.0
        } else {
            0.0
        };
        let pose_rotation = pose_bone_rotation(&bone.name, pose);
        let euler = [
            -bone.rotation[0] + swing + attack_swing + pose_rotation[0],
            -bone.rotation[1] + head_turn + pose_rotation[1],
            -bone.rotation[2] + pose_rotation[2],
        ];
        let whole_rotation = whole_pose_rotation(pose);
        // Bedrock model space -> world: rotate about the bone pivot, then /16,
        // X negated (Bedrock/Java mirror), spin by the entity yaw about the
        // vertical axis, then translate to the feet offset.
        let place = |px: f32, py: f32, pz: f32| {
            let r = rotate_euler([px, py, pz], bone.pivot, euler);
            let r = rotate_euler(r, [0.0, 14.4, 0.0], whole_rotation);
            let (lx, ly, lz) = (
                -r[0] / 16.0 * scale,
                r[1] / 16.0 * scale,
                r[2] / 16.0 * scale,
            );
            [
                offset[0] + lx * yaw_cos - lz * yaw_sin,
                offset[1] + ly,
                offset[2] + lx * yaw_sin + lz * yaw_cos,
            ]
        };
        let nrm = |n: [f32; 3]| {
            let r = rotate_euler(n, [0.0, 0.0, 0.0], euler);
            let r = rotate_euler(r, [0.0, 0.0, 0.0], whole_rotation);
            // Match `place`: mirror X, then apply the yaw spin.
            let (lx, lz) = (-r[0], r[2]);
            [
                lx * yaw_cos - lz * yaw_sin,
                r[1],
                lx * yaw_sin + lz * yaw_cos,
            ]
        };

        for cube in &bone.cubes {
            let o = cube.origin;
            let s = cube.size;
            let (x0, y0, z0) = (o[0], o[1], o[2]);
            let (x1, y1, z1) = (o[0] + s[0], o[1] + s[1], o[2] + s[2]);
            let (u, v) = (cube.uv[0], cube.uv[1]);
            let (sx, sy, sz) = (s[0], s[1], s[2]);

            // +Z (front/south)
            push_quad(
                &mut verts,
                [
                    place(x0, y1, z1),
                    place(x1, y1, z1),
                    place(x1, y0, z1),
                    place(x0, y0, z1),
                ],
                nrm([0.0, 0.0, 1.0]),
                uvr(u + sz, v + sz, u + sz + sx, v + sz + sy),
            );
            // -Z (back/north)
            push_quad(
                &mut verts,
                [
                    place(x1, y1, z0),
                    place(x0, y1, z0),
                    place(x0, y0, z0),
                    place(x1, y0, z0),
                ],
                nrm([0.0, 0.0, -1.0]),
                uvr(
                    u + 2.0 * sz + sx,
                    v + sz,
                    u + 2.0 * sz + 2.0 * sx,
                    v + sz + sy,
                ),
            );
            // +X (east) — world +X is model -X (negated), so it uses model x0 plane
            push_quad(
                &mut verts,
                [
                    place(x0, y1, z0),
                    place(x0, y1, z1),
                    place(x0, y0, z1),
                    place(x0, y0, z0),
                ],
                nrm([1.0, 0.0, 0.0]),
                uvr(u, v + sz, u + sz, v + sz + sy),
            );
            // -X (west)
            push_quad(
                &mut verts,
                [
                    place(x1, y1, z1),
                    place(x1, y1, z0),
                    place(x1, y0, z0),
                    place(x1, y0, z1),
                ],
                nrm([-1.0, 0.0, 0.0]),
                uvr(u + sz + sx, v + sz, u + 2.0 * sz + sx, v + sz + sy),
            );
            // +Y (top)
            push_quad(
                &mut verts,
                [
                    place(x1, y1, z0),
                    place(x0, y1, z0),
                    place(x0, y1, z1),
                    place(x1, y1, z1),
                ],
                nrm([0.0, 1.0, 0.0]),
                uvr(u + sz, v, u + sz + sx, v + sz),
            );
            // -Y (bottom)
            push_quad(
                &mut verts,
                [
                    place(x1, y0, z1),
                    place(x0, y0, z1),
                    place(x0, y0, z0),
                    place(x1, y0, z0),
                ],
                nrm([0.0, -1.0, 0.0]),
                uvr(u + sz + sx, v, u + sz + 2.0 * sx, v + sz),
            );
        }
    }
    verts
}

/// Builds a simple bone-following armour layer for a humanoid geometry. Unlike
/// an axis-aligned bounds overlay, these pieces inherit walking, attacking,
/// crouching, swimming, gliding, sleeping, and death transforms from the same
/// bones as the underlying entity. `slot` uses protocol equipment numbering:
/// 2 boots, 3 leggings, 4 chestplate, 5 helmet.
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn entity_armour_mesh(
    geo: &crab_assets::EntityGeometry,
    offset: [f32; 3],
    white_uv: [f32; 4],
    limb_swing: f32,
    limb_amount: f32,
    scale: f32,
    yaw_deg: f32,
    head_yaw_deg: f32,
    attack_progress: f32,
    pose: i32,
    slot: usize,
    color: [f32; 3],
) -> Vec<Vertex> {
    let mut armour = geo.clone();
    armour.bones.retain(|bone| {
        let name = bone.name.to_ascii_lowercase();
        let head = name.contains("head") || name.contains("hat");
        let body = name.contains("body") || name.contains("torso");
        let arm = name.contains("arm");
        let leg = name.contains("leg") || name.contains("foot");
        match slot {
            2 => leg,
            3 => leg || body,
            4 => body || arm,
            5 => head,
            _ => false,
        }
    });
    for bone in &mut armour.bones {
        for cube in &mut bone.cubes {
            let inflation = if slot == 5 { 0.5 } else { 0.25 };
            cube.origin = cube.origin.map(|coordinate| coordinate - inflation);
            cube.size = cube.size.map(|extent| extent + inflation * 2.0);
        }
    }
    let mut vertices = entity_mesh_with_pose(
        &armour,
        offset,
        [0.0; 2],
        [1.0; 2],
        limb_swing,
        limb_amount,
        scale,
        yaw_deg,
        head_yaw_deg,
        attack_progress,
        pose,
    );
    let uv = [
        (white_uv[0] + white_uv[2]) * 0.5,
        (white_uv[1] + white_uv[3]) * 0.5,
    ];
    for vertex in &mut vertices {
        vertex.uv = uv;
        vertex.tint = color;
    }
    vertices
}

fn whole_pose_rotation(pose: i32) -> [f32; 3] {
    match pose {
        1 | 3 | 4 => [-90.0, 0.0, 0.0],
        2 => [0.0, 0.0, 90.0],
        6 => [-25.0, 0.0, 0.0],
        7 => [0.0, 0.0, -90.0],
        _ => [0.0; 3],
    }
}

fn pose_bone_rotation(name: &str, pose: i32) -> [f32; 3] {
    let name = name.to_ascii_lowercase();
    let is_head = name.contains("head");
    let is_body = name.contains("body") || name.contains("torso");
    let is_arm = name.contains("arm");
    let is_leg = name.contains("leg");
    match pose {
        5 if is_body => [22.0, 0.0, 0.0],
        5 if is_head => [-12.0, 0.0, 0.0],
        5 if is_arm => [18.0, 0.0, 0.0],
        5 if is_leg => [-12.0, 0.0, 0.0],
        10 if is_body => [12.0, 0.0, 0.0],
        10 if is_arm => [-15.0, 0.0, 0.0],
        10 if is_leg => [-75.0, 0.0, 0.0],
        _ => [0.0; 3],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_world::{Biomes, BlockStates, Chunk, Section, World};

    const STONE: u32 = 1;

    fn world_with_chunk0() -> World {
        let mut world = World::overworld();
        let sections = (0..24)
            .map(|_| Section {
                block_count: 0,
                blocks: BlockStates::Uniform(0),
                biomes: Biomes::Uniform(0),
            })
            .collect();
        world.load_chunk(Chunk {
            x: 0,
            z: 0,
            sections,
        });
        world
    }

    #[test]
    fn lone_block_emits_six_faces() {
        let mut world = world_with_chunk0();
        world.set_block_state(5, -60, 5, STONE);
        let atlas = Atlas::debug_uniform();
        let mesh = mesh_region(&world, &atlas, [5, -60, 5], [5, -60, 5]);
        assert_eq!(mesh.vertices.len(), 36); // 6 faces * 2 tris * 3 verts
    }

    #[test]
    fn dropped_block_item_emits_textured_cube_faces() {
        let atlas = Atlas::debug_uniform();
        let vertices = block_item_mesh(&atlas, "minecraft:stone", [0.0, 0.0, 0.0], 0.36, 45.0);
        assert_eq!(vertices.len(), 36);
        assert!(vertices.iter().all(|vertex| vertex.opacity == 1.0));
    }

    #[test]
    fn uvlock_keeps_top_texture_aligned_after_model_rotation() {
        let (u, v) = uvlock_coordinates(2, [0.0, 90.0, 0.0], 0.0, 0.0);
        assert!((u - 0.0).abs() < 1e-5);
        assert!((v - 1.0).abs() < 1e-5);

        let (u, v) = uvlock_coordinates(4, [0.0, 90.0, 0.0], 0.0, 0.0);
        assert!((u - 0.0).abs() < 1e-5);
        assert!((v - 0.0).abs() < 1e-5);
    }

    #[test]
    fn limb_swing_alternates_and_skips_body() {
        use std::f32::consts::FRAC_PI_2;
        // Non-limb bones never swing.
        assert_eq!(limb_swing_deg("body", FRAC_PI_2, 1.0), 0.0);
        assert_eq!(limb_swing_deg("head", FRAC_PI_2, 1.0), 0.0);
        // Opposite legs swing in opposite directions.
        let l0 = limb_swing_deg("leg0", FRAC_PI_2, 1.0);
        let l1 = limb_swing_deg("leg1", FRAC_PI_2, 1.0);
        assert!(l0 > 0.0 && l1 < 0.0 && (l0 + l1).abs() < 1e-3);
        // Arm swings opposite the leg on the same side.
        let leg = limb_swing_deg("right_leg", FRAC_PI_2, 1.0);
        let arm = limb_swing_deg("right_arm", FRAC_PI_2, 1.0);
        assert!(leg * arm < 0.0);
        // No movement -> no swing.
        assert_eq!(limb_swing_deg("leg0", FRAC_PI_2, 0.0), 0.0);
    }

    #[test]
    fn attack_progress_moves_player_arm_vertices() {
        let geometry = crab_assets::player_geometry();
        let rest = entity_mesh(
            &geometry,
            [0.0; 3],
            [0.0; 2],
            [64.0, 64.0],
            0.0,
            0.0,
            1.0,
            0.0,
            0.0,
            0.0,
        );
        let attack = entity_mesh(
            &geometry,
            [0.0; 3],
            [0.0; 2],
            [64.0, 64.0],
            0.0,
            0.0,
            1.0,
            0.0,
            0.0,
            0.5,
        );
        assert_eq!(rest.len(), attack.len());
        assert!(rest
            .iter()
            .zip(&attack)
            .any(|(before, after)| before.position != after.position));
    }

    #[test]
    fn metadata_poses_transform_humanoid_bones_and_whole_model() {
        let geometry = crab_assets::player_geometry();
        let mesh = |pose| {
            entity_mesh_with_pose(
                &geometry,
                [0.0; 3],
                [0.0; 2],
                [64.0, 64.0],
                0.0,
                0.0,
                1.0,
                0.0,
                0.0,
                0.0,
                pose,
            )
        };
        let standing = mesh(0);
        let crouching = mesh(5);
        let swimming = mesh(3);
        assert_eq!(standing.len(), crouching.len());
        assert_eq!(standing.len(), swimming.len());
        assert!(standing
            .iter()
            .zip(&crouching)
            .any(|(before, after)| before.position != after.position));
        assert!(standing
            .iter()
            .zip(&swimming)
            .any(|(before, after)| before.position != after.position));
        assert_eq!(whole_pose_rotation(3), [-90.0, 0.0, 0.0]);
        assert_eq!(whole_pose_rotation(0), [0.0; 3]);
        assert_eq!(pose_bone_rotation("body", 5), [22.0, 0.0, 0.0]);
        assert_eq!(pose_bone_rotation("body", 0), [0.0; 3]);
    }

    #[test]
    fn armour_layers_follow_filtered_humanoid_bones_and_pose() {
        let geometry = crab_assets::player_geometry();
        let mesh = |slot, pose| {
            entity_armour_mesh(
                &geometry,
                [0.0; 3],
                [0.0, 0.0, 1.0, 1.0],
                0.0,
                0.0,
                1.0,
                0.0,
                0.0,
                0.0,
                pose,
                slot,
                [0.2, 0.8, 0.9],
            )
        };
        let helmet = mesh(5, 0);
        let crouching_helmet = mesh(5, 5);
        let chestplate = mesh(4, 0);
        assert_eq!(helmet.len(), 36);
        assert_eq!(chestplate.len(), 108);
        assert!(helmet
            .iter()
            .zip(&crouching_helmet)
            .any(|(standing, crouching)| standing.position != crouching.position));
        assert!(helmet.iter().all(|vertex| vertex.tint == [0.2, 0.8, 0.9]));
    }

    #[test]
    fn unloaded_region_is_empty() {
        let world = World::overworld();
        let atlas = Atlas::debug_uniform();
        let mesh = mesh_region(&world, &atlas, [0, 0, 0], [4, 4, 4]);
        assert!(mesh.vertices.is_empty());
    }

    #[test]
    fn fully_enclosed_block_is_culled() {
        let mut world = world_with_chunk0();
        world.set_block_state(5, -60, 5, STONE);
        for (dx, dy, dz) in [
            (1, 0, 0),
            (-1, 0, 0),
            (0, 1, 0),
            (0, -1, 0),
            (0, 0, 1),
            (0, 0, -1),
        ] {
            world.set_block_state(5 + dx, -60 + dy, 5 + dz, STONE);
        }
        let atlas = Atlas::debug_uniform();
        let mesh = mesh_region(&world, &atlas, [5, -60, 5], [5, -60, 5]);
        assert!(mesh.vertices.is_empty());
    }

    #[test]
    fn door_and_trapdoor_state_radices_select_vanilla_models() {
        assert_eq!(door_visual(0), ("top_left_open", 0.0));
        assert_eq!(door_visual(62), ("bottom_right", 0.0));
        assert_eq!(trapdoor_visual(0), ("open", 0.0));
        assert_eq!(trapdoor_visual(28), ("bottom", 0.0));
    }

    #[test]
    fn rail_state_radices_include_waterlogging_power_and_corners() {
        assert_eq!(rail_visual("rail", 0), ("base", 0.0));
        assert_eq!(rail_visual("rail", 5), ("raised_ne", 90.0));
        assert_eq!(rail_visual("rail", 12), ("corner", 0.0));
        assert_eq!(rail_visual("powered_rail", 4), ("on_raised_ne", 90.0));
        assert_eq!(rail_visual("powered_rail", 14), ("base", 90.0));
    }

    #[test]
    fn redstone_state_radices_drive_connections_dot_and_power_color() {
        let isolated = redstone_visual(1_160);
        assert_eq!(isolated.connections, [2, 2, 2, 2]);
        assert!(isolated.dot);
        assert_eq!(isolated.power, 0);

        let line = redstone_visual(863);
        assert_eq!(line.connections, [1, 2, 2, 2]);
        assert!(!line.dot);
        assert_eq!(line.power, 15);

        assert!(redstone_visual(584).dot);
        assert!(redstone_tint(15)[0] > redstone_tint(0)[0]);
        assert_eq!(redstone_tint(0), [0.3, 0.0, 0.0]);
    }

    #[test]
    fn axis_and_furnace_states_rotate_models_from_vanilla_ordering() {
        assert_eq!(
            axis_rotation("minecraft:oak_log", 0),
            Some([0.0, 0.0, -90.0])
        );
        assert_eq!(axis_rotation("minecraft:oak_log", 1), Some([0.0, 0.0, 0.0]));
        assert_eq!(
            axis_rotation("minecraft:oak_log", 2),
            Some([90.0, 0.0, 0.0])
        );
        assert_eq!(axis_rotation("minecraft:stone", 0), None);
        assert_eq!(furnace_visual(0), ("on", 0.0));
        assert_eq!(furnace_visual(3), ("off", 180.0));
        assert_eq!(furnace_visual(6), ("on", 90.0));
        assert_eq!(
            horizontal_rotation("minecraft:loom", 3),
            Some([0.0, 90.0, 0.0])
        );
        assert_eq!(
            horizontal_rotation("minecraft:white_glazed_terracotta", 0),
            Some([0.0, 180.0, 0.0])
        );
        assert_eq!(
            horizontal_rotation("minecraft:ladder", 7),
            Some([0.0, 90.0, 0.0])
        );
        assert_eq!(campfire_visual(0), ("on", 180.0));
        assert_eq!(campfire_visual(12), ("off", 0.0));
    }
}

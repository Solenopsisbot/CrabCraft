//! Turning a [`World`] region into a renderable mesh.
//!
//! We do straightforward **face culling**: a block face is emitted only when
//! its neighbour is air (or absent). Faces between two solid blocks are hidden,
//! which is the cheapest meaningful optimisation and keeps the vertex count
//! sane. No texturing yet — each block gets a flat colour (see [`block_color`]),
//! which is enough to read terrain shape; the texture atlas comes later.

use crab_world::World;

/// A vertex: position, face normal (for lighting), and flat colour.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 3],
}

impl Vertex {
    /// wgpu vertex layout matching this struct.
    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        const ATTRS: [wgpu::VertexAttribute; 3] =
            wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x3];
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
    /// Neighbour direction this face points toward.
    dir: [i32; 3],
    normal: [f32; 3],
    /// Four corners (unit cube space), wound as a quad.
    corners: [[f32; 3]; 4],
}

#[rustfmt::skip]
const FACES: [Face; 6] = [
    // +X
    Face { dir: [1, 0, 0], normal: [1.0, 0.0, 0.0],
        corners: [[1.0,0.0,0.0],[1.0,1.0,0.0],[1.0,1.0,1.0],[1.0,0.0,1.0]] },
    // -X
    Face { dir: [-1, 0, 0], normal: [-1.0, 0.0, 0.0],
        corners: [[0.0,0.0,1.0],[0.0,1.0,1.0],[0.0,1.0,0.0],[0.0,0.0,0.0]] },
    // +Y (top)
    Face { dir: [0, 1, 0], normal: [0.0, 1.0, 0.0],
        corners: [[0.0,1.0,0.0],[1.0,1.0,0.0],[1.0,1.0,1.0],[0.0,1.0,1.0]] },
    // -Y (bottom)
    Face { dir: [0, -1, 0], normal: [0.0, -1.0, 0.0],
        corners: [[0.0,0.0,1.0],[1.0,0.0,1.0],[1.0,0.0,0.0],[0.0,0.0,0.0]] },
    // +Z
    Face { dir: [0, 0, 1], normal: [0.0, 0.0, 1.0],
        corners: [[1.0,0.0,1.0],[1.0,1.0,1.0],[0.0,1.0,1.0],[0.0,0.0,1.0]] },
    // -Z
    Face { dir: [0, 0, -1], normal: [0.0, 0.0, -1.0],
        corners: [[0.0,0.0,0.0],[0.0,1.0,0.0],[1.0,1.0,0.0],[1.0,0.0,0.0]] },
];

/// Builds a mesh for the inclusive world-coordinate box `[min, max]`.
///
/// Blocks outside loaded chunks (or outside the world) count as "not solid", so
/// the region's outer shell is drawn.
pub fn mesh_region(world: &World, min: [i32; 3], max: [i32; 3]) -> Mesh {
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
                let color = block_color(state);
                for face in &FACES {
                    let neighbor =
                        world.block_state(x + face.dir[0], y + face.dir[1], z + face.dir[2]);
                    let neighbor_solid = neighbor.is_some_and(|s| !crab_registry::is_air(s));
                    if neighbor_solid {
                        continue;
                    }
                    let base = [x as f32, y as f32, z as f32];
                    let c = &face.corners;
                    // two triangles: (0,1,2) and (0,2,3)
                    for &i in &[0usize, 1, 2, 0, 2, 3] {
                        vertices.push(Vertex {
                            position: [base[0] + c[i][0], base[1] + c[i][1], base[2] + c[i][2]],
                            normal: face.normal,
                            color,
                        });
                    }
                }
            }
        }
    }
    Mesh { vertices }
}

/// Flat debug colour for a block state. Common terrain blocks get hand-picked
/// colours; everything else is hashed to a stable, distinguishable hue.
pub fn block_color(state: u32) -> [f32; 3] {
    match crab_registry::block_name(state) {
        Some("minecraft:grass_block") => [0.34, 0.62, 0.24],
        Some("minecraft:dirt") => [0.46, 0.33, 0.22],
        Some("minecraft:stone" | "minecraft:cobblestone") => [0.50, 0.50, 0.52],
        Some("minecraft:bedrock") => [0.20, 0.20, 0.22],
        Some("minecraft:water") => [0.20, 0.40, 0.85],
        Some("minecraft:sand") => [0.82, 0.77, 0.55],
        Some("minecraft:oak_log" | "minecraft:spruce_log") => [0.40, 0.29, 0.16],
        Some("minecraft:oak_leaves" | "minecraft:spruce_leaves") => [0.18, 0.45, 0.16],
        Some(name) => hash_color(name),
        None => [1.0, 0.0, 1.0],
    }
}

fn hash_color(name: &str) -> [f32; 3] {
    let mut h: u32 = 2_166_136_261;
    for b in name.bytes() {
        h = (h ^ u32::from(b)).wrapping_mul(16_777_619);
    }
    let r = ((h >> 16) & 0xff) as f32 / 255.0;
    let g = ((h >> 8) & 0xff) as f32 / 255.0;
    let b = (h & 0xff) as f32 / 255.0;
    // keep it mid-bright so nothing is near-black or blown out
    [0.3 + r * 0.6, 0.3 + g * 0.6, 0.3 + b * 0.6]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crab_world::{BlockStates, Chunk, Section, World};

    const STONE: u32 = 1;

    fn world_with_chunk0() -> World {
        let mut world = World::overworld();
        let sections = (0..24)
            .map(|_| Section {
                block_count: 0,
                blocks: BlockStates::Uniform(0),
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
        let mesh = mesh_region(&world, [5, -60, 5], [5, -60, 5]);
        // 6 faces * 2 triangles * 3 vertices
        assert_eq!(mesh.vertices.len(), 36);
        assert_eq!(mesh.triangle_count(), 12);
    }

    #[test]
    fn unloaded_region_is_empty() {
        let world = World::overworld(); // nothing loaded
        let mesh = mesh_region(&world, [0, 0, 0], [4, 4, 4]);
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
        // mesh only the centre cell: every neighbour is solid -> no faces
        let mesh = mesh_region(&world, [5, -60, 5], [5, -60, 5]);
        assert!(mesh.vertices.is_empty());
    }

    #[test]
    fn known_block_colors_differ_from_unknown() {
        assert_eq!(block_color(9), [0.34, 0.62, 0.24]); // grass
        assert_ne!(block_color(9), block_color(1)); // grass != stone
    }
}

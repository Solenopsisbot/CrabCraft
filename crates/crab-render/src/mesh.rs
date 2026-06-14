//! Turning a [`World`] region into a textured mesh.
//!
//! Face culling: a block face is emitted only when its neighbour is air (or
//! absent). Each face is textured from the [`crab_assets::Atlas`] (per-block,
//! per-face atlas UVs + a tint multiplier); blocks the atlas doesn't have a cube
//! model for fall back to a flat tinted tile.

use crab_assets::Atlas;
use crab_world::World;

/// A vertex: position, face normal (lighting), atlas UV, and tint multiplier.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub tint: [f32; 3],
}

impl Vertex {
    /// wgpu vertex layout matching this struct.
    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        const ATTRS: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
            0 => Float32x3, // position
            1 => Float32x3, // normal
            2 => Float32x2, // uv
            3 => Float32x3, // tint
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
    /// Neighbour direction this face points toward.
    dir: [i32; 3],
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
    Face { dir: [1, 0, 0], normal: [1.0, 0.0, 0.0],
        corners: [[1.0,0.0,0.0],[1.0,1.0,0.0],[1.0,1.0,1.0],[1.0,0.0,1.0]],
        uvs: [[0.0,1.0],[0.0,0.0],[1.0,0.0],[1.0,1.0]] },
    // -X (west)
    Face { dir: [-1, 0, 0], normal: [-1.0, 0.0, 0.0],
        corners: [[0.0,0.0,1.0],[0.0,1.0,1.0],[0.0,1.0,0.0],[0.0,0.0,0.0]],
        uvs: [[0.0,1.0],[0.0,0.0],[1.0,0.0],[1.0,1.0]] },
    // +Y (top)
    Face { dir: [0, 1, 0], normal: [0.0, 1.0, 0.0],
        corners: [[0.0,1.0,0.0],[1.0,1.0,0.0],[1.0,1.0,1.0],[0.0,1.0,1.0]],
        uvs: [[0.0,0.0],[1.0,0.0],[1.0,1.0],[0.0,1.0]] },
    // -Y (bottom)
    Face { dir: [0, -1, 0], normal: [0.0, -1.0, 0.0],
        corners: [[0.0,0.0,1.0],[1.0,0.0,1.0],[1.0,0.0,0.0],[0.0,0.0,0.0]],
        uvs: [[0.0,1.0],[1.0,1.0],[1.0,0.0],[0.0,0.0]] },
    // +Z (south)
    Face { dir: [0, 0, 1], normal: [0.0, 0.0, 1.0],
        corners: [[1.0,0.0,1.0],[1.0,1.0,1.0],[0.0,1.0,1.0],[0.0,0.0,1.0]],
        uvs: [[0.0,1.0],[0.0,0.0],[1.0,0.0],[1.0,1.0]] },
    // -Z (north)
    Face { dir: [0, 0, -1], normal: [0.0, 0.0, -1.0],
        corners: [[0.0,0.0,0.0],[0.0,1.0,0.0],[1.0,1.0,0.0],[1.0,0.0,0.0]],
        uvs: [[0.0,1.0],[0.0,0.0],[1.0,0.0],[1.0,1.0]] },
];

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
                let model = atlas.model(crab_registry::block_name(state).unwrap_or(""));
                let base = [x as f32, y as f32, z as f32];

                for (fi, face) in FACES.iter().enumerate() {
                    let neighbor =
                        world.block_state(x + face.dir[0], y + face.dir[1], z + face.dir[2]);
                    if neighbor.is_some_and(|s| !crab_registry::is_air(s)) {
                        continue;
                    }
                    let tex = model.faces[fi];
                    let [u0, v0, u1, v1] = tex.uv;
                    // two triangles: (0,1,2) and (0,2,3)
                    for &ci in &[0usize, 1, 2, 0, 2, 3] {
                        let c = face.corners[ci];
                        let [cu, cv] = face.uvs[ci];
                        vertices.push(Vertex {
                            position: [base[0] + c[0], base[1] + c[1], base[2] + c[2]],
                            normal: face.normal,
                            uv: [u0 + cu * (u1 - u0), v0 + cv * (v1 - v0)],
                            tint: tex.tint,
                        });
                    }
                }
            }
        }
    }
    Mesh { vertices }
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
        let atlas = Atlas::debug_uniform();
        let mesh = mesh_region(&world, &atlas, [5, -60, 5], [5, -60, 5]);
        assert_eq!(mesh.vertices.len(), 36); // 6 faces * 2 tris * 3 verts
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
}

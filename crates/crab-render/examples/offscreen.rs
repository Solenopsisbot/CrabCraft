//! Renders a synthetic test world (grass plain + stepped stone pyramid) to a
//! PNG, exercising the full meshing + wgpu pipeline headlessly.
//!
//! Usage: `cargo run -p crab-render --example offscreen [OUTPUT.png]`

use crab_render::{mesh_region, render_to_png, Camera};
use crab_world::{BlockStates, Chunk, Section, World};
use glam::Vec3;

// Block-state IDs (1.20.1 global palette).
const AIR: u32 = 0;
const STONE: u32 = 1;
const GRASS: u32 = 9;
const DIRT: u32 = 10;
const BEDROCK: u32 = 79;

fn air_chunk(x: i32, z: i32) -> Chunk {
    Chunk {
        x,
        z,
        sections: (0..24)
            .map(|_| Section {
                block_count: 0,
                blocks: BlockStates::Uniform(AIR),
            })
            .collect(),
    }
}

fn main() {
    let mut world = World::overworld();
    for cx in 0..=2 {
        for cz in 0..=2 {
            world.load_chunk(air_chunk(cx, cz));
        }
    }

    // Flat ground: bedrock / dirt / dirt / grass, like a superflat world.
    for x in 0..40 {
        for z in 0..40 {
            world.set_block_state(x, -64, z, BEDROCK);
            world.set_block_state(x, -63, z, DIRT);
            world.set_block_state(x, -62, z, DIRT);
            world.set_block_state(x, -61, z, GRASS);
        }
    }

    // A stepped stone pyramid centred on the plain.
    for level in 0..9i32 {
        let lo = 8 + level;
        let hi = 32 - level;
        if lo >= hi {
            break;
        }
        let y = -60 + level;
        for x in lo..hi {
            for z in lo..hi {
                world.set_block_state(x, y, z, STONE);
            }
        }
    }

    let mesh = mesh_region(&world, [0, -64, 0], [39, -50, 39]);
    eprintln!(
        "meshed: {} vertices ({} triangles)",
        mesh.vertices.len(),
        mesh.triangle_count()
    );

    let (width, height) = (1280u32, 720u32);
    let camera = Camera {
        eye: Vec3::new(58.0, -22.0, 58.0),
        target: Vec3::new(20.0, -58.0, 20.0),
        up: Vec3::Y,
        aspect: width as f32 / height as f32,
        fovy_radians: 50f32.to_radians(),
        znear: 0.1,
        zfar: 1000.0,
    };

    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/crabcraft_render.png".to_string());
    render_to_png(&mesh, &camera, width, height, std::path::Path::new(&path))
        .expect("render failed");
    eprintln!("wrote {path}");
}

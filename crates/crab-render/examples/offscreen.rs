//! Renders a synthetic test world (grass plain + stepped stone pyramid) to a
//! PNG, exercising meshing + the texture atlas + the wgpu pipeline headlessly.
//!
//! Usage:
//!   cargo run -p crab-render --example offscreen -- [OUTPUT.png] [CLIENT.jar]
//!
//! If a client jar is given, blocks are textured from it; otherwise a flat
//! debug atlas is used.

use crab_assets::Atlas;
use crab_render::{mesh_region, render_to_png, Camera};
use crab_world::{BlockStates, Chunk, Section, World};
use glam::Vec3;

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
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/crabcraft_render.png".to_string());

    // Load the texture atlas from a client jar if one was supplied.
    let atlas = match std::env::args().nth(2) {
        Some(jar) => {
            let names: Vec<String> = crab_registry::BLOCKS_1_20_1
                .iter()
                .map(|b| b.name.to_string())
                .collect();
            match crab_assets::load_block_atlas(std::path::Path::new(&jar), &names) {
                Ok(a) => {
                    eprintln!("loaded atlas {}x{} from {jar}", a.width, a.height);
                    a
                }
                Err(e) => {
                    eprintln!("atlas load failed ({e}); using debug atlas");
                    Atlas::debug_uniform()
                }
            }
        }
        None => Atlas::debug_uniform(),
    };

    let mut world = World::overworld();
    for cx in 0..=2 {
        for cz in 0..=2 {
            world.load_chunk(air_chunk(cx, cz));
        }
    }
    for x in 0..40 {
        for z in 0..40 {
            world.set_block_state(x, -64, z, BEDROCK);
            world.set_block_state(x, -63, z, DIRT);
            world.set_block_state(x, -62, z, DIRT);
            world.set_block_state(x, -61, z, GRASS);
        }
    }
    for level in 0..9i32 {
        let (lo, hi) = (8 + level, 32 - level);
        if lo >= hi {
            break;
        }
        for x in lo..hi {
            for z in lo..hi {
                world.set_block_state(x, -60 + level, z, STONE);
            }
        }
    }

    let mesh = mesh_region(&world, &atlas, [0, -64, 0], [39, -50, 39]);
    eprintln!("meshed {} triangles", mesh.triangle_count());

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

    render_to_png(
        &mesh,
        &atlas,
        &camera,
        width,
        height,
        std::path::Path::new(&path),
    )
    .expect("render failed");
    eprintln!("wrote {path}");
}

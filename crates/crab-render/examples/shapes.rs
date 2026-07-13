//! Renders a row of non-full-cube blocks (slab, stairs, fence, wall, plants,
//! lantern) on a grass platform to a PNG, to verify element-model meshing.
//!
//! Usage: cargo run -p crab-render --example shapes -- <out.png> <jar>

use std::path::Path;

use crab_render::{mesh_region, render_to_png, Camera};
use crab_world::{Biomes, BlockStates, Chunk, Section, World};
use glam::Vec3;

const AIR: u32 = 0;
const STONE: u32 = 1;
const GRASS: u32 = 9;
const DIRT: u32 = 10;

fn air_chunk(x: i32, z: i32) -> Chunk {
    Chunk {
        x,
        z,
        sections: (0..24)
            .map(|_| Section {
                block_count: 0,
                blocks: BlockStates::Uniform(AIR),
                biomes: Biomes::Uniform(0),
            })
            .collect(),
    }
}

fn main() {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/shapes.png".to_string());
    let jar = std::env::args()
        .nth(2)
        .expect("usage: shapes <out.png> <jar>");
    let names: Vec<String> = crab_registry::BLOCKS_1_20_1
        .iter()
        .map(|b| b.name.to_string())
        .collect();
    let atlas = crab_assets::load_block_atlas(Path::new(&jar), &names).expect("atlas");
    eprintln!("atlas {}x{}", atlas.width, atlas.height);

    let mut world = World::overworld();
    for cx in 0..=1 {
        for cz in 0..=1 {
            world.load_chunk(air_chunk(cx, cz));
        }
    }
    for x in 0..20 {
        for z in 0..20 {
            world.set_block_state(x, -62, z, DIRT);
            world.set_block_state(x, -61, z, GRASS);
        }
    }

    // A row of non-cube blocks (default states) on top of the grass, plus a
    // reference full stone cube at the end.
    let row: [(&str, u32); 8] = [
        ("oak_slab", 11024),
        ("oak_stairs", 2885),
        ("oak_fence", 5848),
        ("cobblestone_wall", 7922),
        ("dandelion", 2075),
        ("poppy", 2077),
        ("lantern", 18365),
        ("stone", STONE),
    ];
    for (i, (name, st)) in row.iter().enumerate() {
        let x = 4 + i as i32 * 2;
        let elems = atlas.block_elements(name).map(<[_]>::len);
        eprintln!("{name}: elements={elems:?} is_cube={}", atlas.is_cube(name));
        world.set_block_state(x, -60, 8, *st);
    }

    let mesh = mesh_region(&world, &atlas, [0, -62, 0], [19, -58, 19]);
    eprintln!("meshed {} triangles", mesh.triangle_count());

    let (width, height) = (1280u32, 720u32);
    let camera = Camera {
        eye: Vec3::new(11.0, -56.6, 16.5),
        target: Vec3::new(11.0, -59.7, 8.0),
        up: Vec3::Y,
        aspect: width as f32 / height as f32,
        fovy_radians: 60f32.to_radians(),
        znear: 0.05,
        zfar: 1000.0,
    };
    render_to_png(
        &mesh,
        &atlas.rgba,
        atlas.width,
        atlas.height,
        &camera,
        width,
        height,
        Path::new(&out),
    )
    .expect("render failed");
    eprintln!("wrote {out}");
}

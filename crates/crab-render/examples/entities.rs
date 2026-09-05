//! Builds a jar-only entity atlas (cow/pig/creeper) and renders all three from
//! it, to verify shared-atlas UV offsetting + multi-model path.
//!
//! Usage: cargo run -p crab-render --example entities -- CLIENT.jar [out.png]

use crab_render::{entity_mesh, render_to_png, Camera, Mesh};
use glam::Vec3;

fn main() {
    let mut args = std::env::args().skip(1);
    let jar = args.next().expect("jar path");
    let out = args
        .next()
        .unwrap_or_else(|| "/tmp/crab_entities3.png".to_string());

    // Build the atlas exactly like the live client: from the full entity
    // registry (so models are keyed by real entity-type ids).
    let types: Vec<(i32, String)> = crab_registry::ENTITIES_1_20_1
        .iter()
        .map(|e| (e.id as i32, e.name.to_string()))
        .collect();
    let atlas = crab_assets::load_entity_atlas_from_jar_with_registry(
        std::path::Path::new(&jar),
        crab_registry::RegistrySet::for_protocol(763),
        &types,
    );
    eprintln!(
        "entity atlas {}x{}, {} models and {} paintings loaded",
        atlas.width,
        atlas.height,
        atlas.models.len(),
        atlas.paintings.len()
    );

    let dims = [atlas.width as f32, atlas.height as f32];
    let mut verts = Vec::new();
    let mut ids: Vec<i32> = atlas.models.keys().copied().collect();
    ids.sort_unstable();
    for (i, id) in ids.iter().enumerate() {
        let m = &atlas.models[id];
        let x = (i as f32 - (ids.len() as f32 - 1.0) / 2.0) * 1.6;
        verts.extend(entity_mesh(
            &m.geo,
            [x, 0.0, 0.0],
            [m.atlas_x, m.atlas_y],
            dims,
            0.0,
            0.0,
            0.0,
            1.0,
            0.0,
            0.0,
        ));
    }
    let mesh = Mesh { vertices: verts };

    let (w, h) = (1000u32, 600u32);
    let camera = Camera {
        eye: Vec3::new(0.0, 1.4, 4.5),
        target: Vec3::new(0.0, 0.7, 0.0),
        up: Vec3::Y,
        aspect: w as f32 / h as f32,
        fovy_radians: 50f32.to_radians(),
        znear: 0.05,
        zfar: 100.0,
    };
    render_to_png(
        &mesh,
        &atlas.rgba,
        atlas.width,
        atlas.height,
        &camera,
        w,
        h,
        std::path::Path::new(&out),
    )
    .unwrap();
    eprintln!("wrote {out}");
}

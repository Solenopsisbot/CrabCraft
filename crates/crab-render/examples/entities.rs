//! Builds an entity atlas (cow/pig/creeper) and renders all three from it, to
//! verify the shared-atlas UV offsetting + multi-model path.
//!
//! Usage: cargo run -p crab-render --example entities -- MODELS_DIR CLIENT.jar [out.png]

use crab_render::{entity_mesh, render_to_png, Camera, Mesh};
use glam::Vec3;

fn main() {
    let mut args = std::env::args().skip(1);
    let models_dir = args.next().expect("models dir");
    let jar = args.next().expect("jar path");
    let out = args
        .next()
        .unwrap_or_else(|| "/tmp/crab_entities3.png".to_string());

    let types = vec![
        (1, "cow".to_string()),
        (2, "pig".to_string()),
        (3, "creeper".to_string()),
    ];
    let atlas = crab_assets::load_entity_atlas(
        std::path::Path::new(&jar),
        std::path::Path::new(&models_dir),
        &types,
    );
    eprintln!(
        "entity atlas {}x{}, {} models",
        atlas.width,
        atlas.height,
        atlas.models.len()
    );

    let dims = [atlas.width as f32, atlas.height as f32];
    let mut verts = Vec::new();
    for (id, x) in [(1, -1.6f32), (2, 0.0), (3, 1.6)] {
        if let Some(m) = atlas.models.get(&id) {
            verts.extend(entity_mesh(
                &m.geo,
                [x, 0.0, 0.0],
                [m.atlas_x, m.atlas_y],
                dims,
            ));
        }
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

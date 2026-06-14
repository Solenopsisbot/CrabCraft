//! Renders one entity 3D model (Bedrock geometry + jar texture) to a PNG, to
//! verify the geometry parser + box-UV mapping.
//!
//! Usage: cargo run -p crab-render --example entity -- GEO.geo.json CLIENT.jar [name] [out.png]

use crab_render::{entity_mesh, render_to_png, Camera, Mesh};
use glam::Vec3;

fn main() {
    let mut args = std::env::args().skip(1);
    let geo_path = args.next().expect("geo path");
    let jar = args.next().expect("jar path");
    let name = args.next().unwrap_or_else(|| "cow".to_string());
    let out = args
        .next()
        .unwrap_or_else(|| "/tmp/crab_entity.png".to_string());

    let geo = crab_assets::parse_geometry(&std::fs::read_to_string(&geo_path).unwrap())
        .expect("parse geometry");
    eprintln!(
        "geo: {} cubes, texture {}x{}",
        geo.cubes.len(),
        geo.texture_width,
        geo.texture_height
    );
    let (rgba, tw, th) =
        crab_assets::load_entity_texture(std::path::Path::new(&jar), &name).expect("texture");

    let mesh = Mesh {
        vertices: entity_mesh(
            &geo,
            [0.0, 0.0, 0.0],
            [0.0, 0.0],
            [geo.texture_width, geo.texture_height],
        ),
    };

    let (w, h) = (800u32, 800u32);
    let camera = Camera {
        eye: Vec3::new(2.2, 1.3, 2.8),
        target: Vec3::new(0.0, 0.8, 0.0),
        up: Vec3::Y,
        aspect: w as f32 / h as f32,
        fovy_radians: 45f32.to_radians(),
        znear: 0.05,
        zfar: 100.0,
    };
    render_to_png(
        &mesh,
        &rgba,
        tw,
        th,
        &camera,
        w,
        h,
        std::path::Path::new(&out),
    )
    .unwrap();
    eprintln!("wrote {out}");
}

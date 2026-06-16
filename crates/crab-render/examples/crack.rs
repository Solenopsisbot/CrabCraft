//! Renders the block-breaking destroy-stage overlay (one crack stage) on a cube
//! to a PNG, to verify the destroy-stage atlas load + per-stage UV mapping.
//!
//! Usage: cargo run -p crab-render --example crack -- <jar> [stage 0..9] [out.png]

use crab_render::{box_mesh, render_to_png, Camera, Mesh};
use glam::Vec3;

fn main() {
    let mut args = std::env::args().skip(1);
    let jar = args.next().expect("usage: crack <jar> [stage] [out.png]");
    let stage: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(5);
    let out = args
        .next()
        .unwrap_or_else(|| "/tmp/crab_crack.png".to_string());

    let (rgba, w, h) =
        crab_assets::load_destroy_stages(std::path::Path::new(&jar)).expect("destroy_stage");
    eprintln!("destroy atlas {w}x{h}, stage {stage}");

    let n = stage.min(9) as f32;
    let (v0, v1) = (n / 10.0, (n + 1.0) / 10.0);
    let mesh = Mesh {
        vertices: box_mesh(
            [0.0, 0.0, 0.0],
            [1.0, 1.0, 1.0],
            [0.0, v0, 1.0, v1],
            [1.0, 1.0, 1.0],
        ),
    };

    let (rw, rh) = (400u32, 400u32);
    let camera = Camera {
        eye: Vec3::new(1.9, 1.5, 2.3),
        target: Vec3::new(0.5, 0.5, 0.5),
        up: Vec3::Y,
        aspect: rw as f32 / rh as f32,
        fovy_radians: 45f32.to_radians(),
        znear: 0.05,
        zfar: 100.0,
    };
    render_to_png(
        &mesh,
        &rgba,
        w,
        h,
        &camera,
        rw,
        rh,
        std::path::Path::new(&out),
    )
    .unwrap();
    eprintln!("wrote {out}");
}

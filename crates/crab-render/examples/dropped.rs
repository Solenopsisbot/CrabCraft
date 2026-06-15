//! Renders a dropped-item billboard (item icon as a camera-facing quad) to a
//! PNG, to verify dropped-item rendering.
//!
//! Usage: cargo run -p crab-render --example dropped -- <out.png> <jar> [item]

use std::path::Path;

use crab_render::{render_to_png, Camera, Mesh, Vertex};
use glam::Vec3;

fn main() {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/dropped.png".to_string());
    let jar = std::env::args()
        .nth(2)
        .expect("usage: dropped <out.png> <jar> [item]");
    let item = std::env::args()
        .nth(3)
        .unwrap_or_else(|| "diamond".to_string());

    let atlas =
        crab_assets::load_item_atlas(Path::new(&jar), std::slice::from_ref(&item)).expect("atlas");
    let uv = atlas.icon(&item).expect("icon");

    // Build a camera-facing billboard quad (same math as the live renderer).
    let yaw: f32 = 0.0;
    let (s, yr) = (0.4, yaw.to_radians());
    let r = [yr.cos() * s, 0.0, yr.sin() * s];
    let up = [0.0, s, 0.0];
    let c = [0.0, 0.0, 0.0];
    let n = [yr.sin(), 0.4, -yr.cos()];
    let [u0, v0, u1, v1] = uv;
    let corner = |sx: f32, sy: f32, u, v| Vertex {
        position: [c[0] + sx * r[0], c[1] + sy * up[1], c[2] + sx * r[2]],
        normal: n,
        uv: [u, v],
        tint: [1.0, 1.0, 1.0],
    };
    let (tl, tr, br, bl) = (
        corner(-1.0, 1.0, u0, v0),
        corner(1.0, 1.0, u1, v0),
        corner(1.0, -1.0, u1, v1),
        corner(-1.0, -1.0, u0, v1),
    );
    let mesh = Mesh {
        vertices: vec![tl, tr, br, tl, br, bl],
    };

    let (w, h) = (512u32, 512u32);
    let camera = Camera {
        eye: Vec3::new(0.0, 0.2, 1.4),
        target: Vec3::new(0.0, 0.0, 0.0),
        up: Vec3::Y,
        aspect: w as f32 / h as f32,
        fovy_radians: 45f32.to_radians(),
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
        Path::new(&out),
    )
    .expect("render");
    eprintln!("wrote {out}");
}

//! Renders the hardcoded player model with the default skin to a PNG.
//!
//! Usage: cargo run -p crab-render --example player -- <out.png> <jar> [swing]

use std::path::Path;

use crab_render::{entity_mesh, render_to_png, Camera, Mesh};
use glam::Vec3;

fn main() {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/player.png".to_string());
    let jar = std::env::args()
        .nth(2)
        .expect("usage: player <out.png> <jar> [swing]");
    let swing: f32 = std::env::args()
        .nth(3)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);

    let geo = crab_assets::player_geometry();
    let (rgba, tw, th) =
        crab_assets::load_entity_texture(Path::new(&jar), "player").expect("steve skin");
    eprintln!("skin {tw}x{th}, {} bones", geo.bones.len());

    let mesh = Mesh {
        vertices: entity_mesh(
            &geo,
            [0.0, 0.0, 0.0],
            [0.0, 0.0],
            [tw as f32, th as f32],
            swing,
            1.0,
            1.0,
            0.0,
        ),
    };

    let (w, h) = (600u32, 800u32);
    let camera = Camera {
        eye: Vec3::new(0.0, 1.1, 2.6),
        target: Vec3::new(0.0, 1.0, 0.0),
        up: Vec3::Y,
        aspect: w as f32 / h as f32,
        fovy_radians: 45f32.to_radians(),
        znear: 0.05,
        zfar: 100.0,
    };
    render_to_png(&mesh, &rgba, tw, th, &camera, w, h, Path::new(&out)).expect("render");
    eprintln!("wrote {out}");
}

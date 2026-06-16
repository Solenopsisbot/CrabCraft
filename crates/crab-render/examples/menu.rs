//! Renders the pause menu (vanilla button sprites + labels) to a PNG.
//!
//! Usage: cargo run -p crab-render --example menu -- <out.png> <jar>

use std::path::Path;

use crab_render::{menu_geometry, render_hud_to_png, HudFrame};

fn main() {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/menu.png".to_string());
    let jar = std::env::args()
        .nth(2)
        .expect("usage: menu <out.png> <jar>");
    let gui = crab_assets::load_gui_atlas(Path::new(&jar)).expect("gui atlas");

    let labels = ["Back to Game", "Options", "Quit"];
    // Hover the middle button.
    let (color, g, _) = menu_geometry(&gui, &labels, Some(1), 16.0 / 9.0);

    render_hud_to_png(
        &HudFrame {
            color: &color,
            gui: &g,
            item: &[],
            text: &[],
        },
        &gui.rgba,
        gui.width,
        gui.height,
        &[0u8; 4],
        1,
        1,
        960,
        540,
        Path::new(&out),
    )
    .expect("render");
    eprintln!("wrote {out}");
}

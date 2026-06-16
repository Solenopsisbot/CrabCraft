//! Renders the HUD (vanilla hotbar widget, item icons, bars, font text) to a
//! PNG headlessly, to verify the overlay without a display.
//!
//! Usage: cargo run -p crab-render --example hud -- <out.png> <jar>

use std::path::Path;

use crab_render::{hud_geometry, push_text, render_hud_to_png, HudFrame};

fn main() {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/hud.png".to_string());
    let jar = std::env::args().nth(2).expect("usage: hud <out.png> <jar>");
    let jar = Path::new(&jar);

    let gui = crab_assets::load_gui_atlas(jar).expect("gui atlas");
    eprintln!(
        "gui atlas loaded={} {}x{}",
        gui.loaded, gui.width, gui.height
    );

    let names = [
        "diamond_sword",
        "apple",
        "oak_log",
        "diamond",
        "bread",
        "iron_pickaxe",
        "cobblestone",
        "golden_apple",
        "arrow",
    ];
    let owned: Vec<String> = names.iter().map(ToString::to_string).collect();
    let atlas = crab_assets::load_item_atlas(jar, &owned).expect("item atlas");
    let hotbar: Vec<Option<[f32; 4]>> = names.iter().map(|n| atlas.icon(n)).collect();

    let aspect: f32 = std::env::args()
        .nth(3)
        .and_then(|s| s.parse().ok())
        .unwrap_or(16.0 / 9.0);
    let height = 540u32;
    let width = (height as f32 * aspect) as u32;
    let (color, g, item) = hud_geometry(&gui, 7.0, 13, 0.6, 30, 3, &hotbar, aspect);
    let mut text = Vec::new();
    push_text(&mut text, &gui, "Crabcraft 1.20.1", -0.6, 0.9, 0.05, aspect);
    push_text(&mut text, &gui, "x64", -0.2, 0.8, 0.04, aspect);

    render_hud_to_png(
        &HudFrame {
            color: &color,
            gui: &g,
            item: &item,
            text: &text,
        },
        &gui.rgba,
        gui.width,
        gui.height,
        &atlas.rgba,
        atlas.width,
        atlas.height,
        width,
        height,
        Path::new(&out),
    )
    .expect("render hud");
    eprintln!("wrote {out}");
}

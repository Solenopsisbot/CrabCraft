//! Renders the HUD (crosshair, hotbar with item icons, health/food bars) to a
//! PNG headlessly, to verify the overlay without a display.
//!
//! Usage: cargo run -p crab-render --example hud -- <out.png> <jar>
//! Fills the hotbar with a sample of items from the jar's item atlas.

use std::path::Path;

use crab_render::{hud_geometry, render_hud_to_png};

fn main() {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/hud.png".to_string());
    let jar = std::env::args().nth(2).expect("usage: hud <out.png> <jar>");

    // Build an item atlas for a sample hotbar.
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
    let atlas = crab_assets::load_item_atlas(Path::new(&jar), &owned).expect("item atlas");
    eprintln!("resolved {}/{} item icons", atlas.len(), owned.len());

    let hotbar: Vec<Option<[f32; 4]>> = names.iter().map(|n| atlas.icon(n)).collect();

    // 7/20 health, 13/20 food, slot 3 selected, 16:9 aspect.
    let (color, tex) = hud_geometry(7.0, 13, 3, &hotbar, 16.0 / 9.0);
    render_hud_to_png(
        &color,
        &tex,
        &atlas.rgba,
        atlas.width,
        atlas.height,
        960,
        540,
        Path::new(&out),
    )
    .expect("render hud");
    eprintln!("wrote {out} (960x540)");
}

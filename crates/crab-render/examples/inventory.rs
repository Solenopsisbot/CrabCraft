//! Renders the open-inventory panel (grid + item icons) over the HUD to a PNG.
//!
//! Usage: cargo run -p crab-render --example inventory -- <out.png> <jar>

use std::path::Path;

use crab_render::{hud_geometry, inventory_geometry, render_hud_to_png};

fn main() {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/inventory.png".to_string());
    let jar = std::env::args()
        .nth(2)
        .expect("usage: inventory <out.png> <jar>");

    let names = [
        "diamond",
        "iron_ingot",
        "gold_ingot",
        "oak_log",
        "cobblestone",
        "apple",
        "bread",
        "diamond_sword",
        "iron_pickaxe",
        "arrow",
        "bow",
        "emerald",
        "redstone",
        "coal",
        "stick",
        "torch",
    ];
    let owned: Vec<String> = names.iter().map(ToString::to_string).collect();
    let atlas = crab_assets::load_item_atlas(Path::new(&jar), &owned).expect("item atlas");
    eprintln!("resolved {}/{} icons", atlas.len(), owned.len());

    // Fill the 36 inventory slots with a sampling of items (some empty).
    let inv: Vec<Option<[f32; 4]>> = (0..36)
        .map(|i| {
            if i % 3 == 1 {
                None
            } else {
                atlas.icon(names[i % names.len()])
            }
        })
        .collect();
    // A hotbar sample too.
    let hotbar: Vec<Option<[f32; 4]>> = (0..9).map(|i| atlas.icon(names[i])).collect();

    let aspect = 16.0 / 9.0;
    let (mut color, mut tex) = hud_geometry(15.0, 18, 2, &hotbar, aspect);
    let (ic, it) = inventory_geometry(&inv, aspect);
    color.extend(ic);
    tex.extend(it);

    render_hud_to_png(
        &color,
        &tex,
        &atlas.rgba,
        atlas.width,
        atlas.height,
        1280,
        720,
        Path::new(&out),
    )
    .expect("render");
    eprintln!("wrote {out}");
}

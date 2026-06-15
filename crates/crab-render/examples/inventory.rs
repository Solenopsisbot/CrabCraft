//! Renders the open inventory (vanilla container background + item icons + the
//! HUD behind it) to a PNG.
//!
//! Usage: cargo run -p crab-render --example inventory -- <out.png> <jar>

use std::path::Path;

use crab_render::{hud_geometry, inventory_geometry, render_hud_to_png, HudFrame};

fn main() {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/inventory.png".to_string());
    let jar = std::env::args()
        .nth(2)
        .expect("usage: inventory <out.png> <jar>");
    let jar = Path::new(&jar);

    let gui = crab_assets::load_gui_atlas(jar).expect("gui atlas");
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
    let atlas = crab_assets::load_item_atlas(jar, &owned).expect("item atlas");

    let inv: Vec<Option<[f32; 4]>> = (0..36)
        .map(|i| {
            if i % 3 == 1 {
                None
            } else {
                atlas.icon(names[i % names.len()])
            }
        })
        .collect();
    let hotbar: Vec<Option<[f32; 4]>> = (0..9).map(|i| atlas.icon(names[i])).collect();

    let aspect = 16.0 / 9.0;
    let (mut color, mut g, mut item) = hud_geometry(&gui, 15.0, 18, 2, &hotbar, aspect);
    let (ic, ig, iitem) = inventory_geometry(&gui, &inv, aspect);
    color.extend(ic);
    g.extend(ig);
    item.extend(iitem);

    render_hud_to_png(
        &HudFrame {
            color: &color,
            gui: &g,
            item: &item,
            text: &[],
        },
        &gui.rgba,
        gui.width,
        gui.height,
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

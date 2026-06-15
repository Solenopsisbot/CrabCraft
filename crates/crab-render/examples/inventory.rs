//! Renders the open inventory (vanilla container background + item icons + the
//! HUD behind it) to a PNG.
//!
//! Usage: cargo run -p crab-render --example inventory -- <out.png> <jar>

use std::path::Path;

use crab_render::{
    hud_geometry, inventory_geometry, inventory_rect, inventory_slot_rect, push_text,
    render_hud_to_png, HudFrame,
};

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

    // 46 window slots: 0=result, 1-4 craft, 5-8 armour, 9-35 main, 36-44 hotbar,
    // 45 offhand. Populate a representative spread to check positions.
    let mut inv: Vec<Option<[f32; 4]>> = vec![None; 46];
    inv[0] = atlas.icon("oak_log"); // crafting result
    inv[1] = atlas.icon("diamond"); // crafting input
    inv[4] = atlas.icon("cobblestone");
    inv[5] = atlas.icon("iron_pickaxe"); // armour column
    inv[8] = atlas.icon("bow");
    inv[45] = atlas.icon("diamond_sword"); // offhand
    for (i, slot) in inv.iter_mut().enumerate().take(45).skip(9) {
        if i % 3 != 1 {
            *slot = atlas.icon(names[i % names.len()]);
        }
    }
    let hotbar: Vec<Option<[f32; 4]>> = (0..9).map(|i| atlas.icon(names[i])).collect();

    let aspect = 16.0 / 9.0;
    let (mut color, mut g, mut item) = hud_geometry(&gui, 15.0, 18, 2, &hotbar, aspect);
    let (ic, ig, iitem) = inventory_geometry(&gui, &inv, aspect);
    color.extend(ic);
    g.extend(ig);
    item.extend(iitem);

    // Stack-size numbers on a few slots (bottom-right aligned).
    let mut text = Vec::new();
    let rect = inventory_rect(aspect);
    for (slot, count) in [(0usize, "64"), (3, "16"), (12, "5"), (27, "32")] {
        let (_x0, y0, x1, y1) = inventory_slot_rect(rect, slot);
        let h = (y1 - y0) * 0.5;
        let w = gui.text_width(count) * (h / 8.0) / aspect;
        push_text(&mut text, &gui, count, x1 - w, y0 + h, h, aspect);
    }

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
        1280,
        720,
        Path::new(&out),
    )
    .expect("render");
    eprintln!("wrote {out}");
}

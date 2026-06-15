//! Dumps an item-icon atlas built from a client jar to a PNG, for eyeballing.
//!
//! Usage: cargo run -p crab-assets --example item_atlas -- <jar> <out.png> [item ...]

use std::path::Path;

use crab_assets::load_item_atlas;

fn main() {
    let mut args = std::env::args().skip(1);
    let jar = args
        .next()
        .expect("usage: item_atlas <jar> <out.png> [item ...]");
    let out = args
        .next()
        .expect("usage: item_atlas <jar> <out.png> [item ...]");
    let items: Vec<String> = args.collect();
    let items = if items.is_empty() {
        [
            "diamond",
            "iron_sword",
            "oak_log",
            "stone",
            "apple",
            "stick",
            "cobblestone",
            "bread",
            "golden_apple",
            "ender_pearl",
            "redstone",
            "emerald",
        ]
        .iter()
        .map(ToString::to_string)
        .collect()
    } else {
        items
    };

    let atlas = load_item_atlas(Path::new(&jar), &items).expect("load item atlas");
    println!("resolved {}/{} icons", atlas.len(), items.len());
    for it in &items {
        let status = if atlas.icon(it).is_some() {
            "OK"
        } else {
            "MISSING"
        };
        println!("  {it:16} {status}");
    }

    // Scale up 8x so individual 16px icons are easy to see.
    let src = image::RgbaImage::from_raw(atlas.width, atlas.height, atlas.rgba).expect("atlas img");
    let scaled = image::imageops::resize(
        &src,
        atlas.width * 8,
        atlas.height * 8,
        image::imageops::FilterType::Nearest,
    );
    scaled.save(&out).expect("save png");
    println!("wrote {out} ({}x{})", atlas.width * 8, atlas.height * 8);
}

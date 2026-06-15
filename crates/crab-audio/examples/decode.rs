//! Loads + decodes a few block-break sounds from a launcher asset store, to
//! verify the sound pipeline without needing to hear anything.
//!
//! Usage: cargo run -p crab-audio --example decode -- <assets_dir> [index_id]

use std::path::Path;

use crab_audio::{break_sound, ogg_sample_count, read_sound, AssetIndex, SoundPlayer};

fn main() {
    let assets = std::env::args()
        .nth(1)
        .expect("usage: decode <assets_dir> [index_id]");
    let id = std::env::args().nth(2).unwrap_or_else(|| "5".to_string());
    let assets = Path::new(&assets);
    let index =
        AssetIndex::load(&assets.join("indexes").join(format!("{id}.json"))).expect("index");
    println!("asset index {id}.json: {} objects", index.len());

    let player = SoundPlayer::new();
    println!("audio output device available: {}", player.available());

    for name in [
        "dig/grass1",
        "dig/stone1",
        "step/grass1",
        "step/stone1",
        "step/wood1",
        "damage/hit1",
    ] {
        match read_sound(assets, &index, name) {
            Some(bytes) => {
                let samples = ogg_sample_count(&bytes).unwrap_or(0);
                let ok = player.play_ogg(bytes.clone());
                println!(
                    "  {name}: {} bytes, {samples} samples decoded, play/decode_ok={ok}",
                    bytes.len()
                );
            }
            None => println!("  {name}: NOT FOUND"),
        }
    }

    for b in [
        "minecraft:grass_block",
        "minecraft:oak_log",
        "minecraft:stone",
    ] {
        println!("  break_sound({b}) = {}", break_sound(b));
    }
}

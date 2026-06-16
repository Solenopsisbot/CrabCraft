//! Loads + decodes a few block-break sounds from a launcher asset store, to
//! verify the sound pipeline without needing to hear anything.
//!
//! Usage: cargo run -p crab-audio --example decode -- <assets_dir> [index_id]

use std::path::Path;

use crab_audio::{
    break_event, hit_event, ogg_sample_count, place_event, read_sound, sound_group, AssetIndex,
    SoundPlayer, Sounds,
};

fn main() {
    let assets = std::env::args()
        .nth(1)
        .expect("usage: decode <assets_dir> [index_id]");
    let id = std::env::args().nth(2).unwrap_or_else(|| "5".to_string());
    let assets = Path::new(&assets);
    let index =
        AssetIndex::load(&assets.join("indexes").join(format!("{id}.json"))).expect("index");
    println!("asset index {id}.json: {} objects", index.len());

    let sounds = Sounds::load(assets, &index).unwrap_or_default();
    println!("sounds.json events: {}", sounds.len());

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

    // Block -> sound group -> event -> resolved file (what vanilla would play).
    for b in [
        "minecraft:grass_block",
        "minecraft:oak_log",
        "minecraft:stone",
        "minecraft:glass",
        "minecraft:white_wool",
        "minecraft:deepslate",
    ] {
        let (brk, hit, place) = (break_event(b), hit_event(b), place_event(b));
        println!(
            "  {b}: group={} break={brk}->{:?} hit={hit}->{:?} place={place}->{:?}",
            sound_group(b),
            sounds.pick(&brk),
            sounds.pick(&hit),
            sounds.pick(&place),
        );
    }
}

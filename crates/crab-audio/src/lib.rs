//! # crab-audio
//!
//! Loads Minecraft sounds from the launcher **asset store** (the shared
//! `assets/` dir of `indexes/<id>.json` + content-addressed `objects/`) and
//! plays them with `rodio`.
//!
//! Sounds are never bundled with Crabcraft — they come from the user's own
//! install, located via the `CRABCRAFT_ASSETS` directory at runtime. Playback
//! degrades gracefully to a no-op when no audio output device is available
//! (e.g. headless CI), while decoding still works for verification.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Errors loading the asset index.
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("bad asset index: {0}")]
    Index(String),
}

/// Maps logical asset paths (e.g. `minecraft/sounds/dig/grass1.ogg`) to the
/// content hash naming the file under `objects/`.
pub struct AssetIndex {
    objects: HashMap<String, String>,
}

impl AssetIndex {
    /// Loads a launcher `assets/indexes/<id>.json` file.
    pub fn load(index_json: &Path) -> Result<Self, AudioError> {
        let text = std::fs::read_to_string(index_json)?;
        let v: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| AudioError::Index(e.to_string()))?;
        let objs = v
            .get("objects")
            .and_then(serde_json::Value::as_object)
            .ok_or_else(|| AudioError::Index("missing `objects`".into()))?;
        let objects = objs
            .iter()
            .filter_map(|(k, val)| {
                let h = val.get("hash")?.as_str()?;
                Some((k.clone(), h.to_string()))
            })
            .collect();
        Ok(Self { objects })
    }

    /// Content hash for a logical asset path, if present.
    #[must_use]
    pub fn hash(&self, logical: &str) -> Option<&str> {
        self.objects.get(logical).map(String::as_str)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.objects.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }
}

/// Path of a content-addressed object: `objects/<hash[0:2]>/<hash>`.
#[must_use]
pub fn object_path(assets_dir: &Path, hash: &str) -> PathBuf {
    let prefix = &hash[..2.min(hash.len())];
    assets_dir.join("objects").join(prefix).join(hash)
}

/// Reads a sound's OGG bytes by logical name (e.g. `"dig/grass1"`).
#[must_use]
pub fn read_sound(assets_dir: &Path, index: &AssetIndex, name: &str) -> Option<Vec<u8>> {
    let logical = format!("minecraft/sounds/{name}.ogg");
    let hash = index.hash(&logical)?;
    std::fs::read(object_path(assets_dir, hash)).ok()
}

/// Parsed `sounds.json`: maps a sound **event** (e.g. `block.stone.break`) to
/// the list of sound-file logical names it can play (e.g. `dig/stone1`). This is
/// the same indirection vanilla uses, so events resolve to exactly the files the
/// game would pick (incl. the quieter `*.hit` mining sound = `step/*`).
#[derive(Default)]
pub struct Sounds {
    events: HashMap<String, Vec<String>>,
    /// Registry order from the source `sounds.json`; protocol ids are one-based.
    registry: Vec<String>,
}

impl Sounds {
    /// Loads `minecraft/sounds.json` from the asset store via the index.
    #[must_use]
    pub fn load(assets_dir: &Path, index: &AssetIndex) -> Option<Self> {
        let hash = index.hash("minecraft/sounds.json")?;
        let bytes = std::fs::read(object_path(assets_dir, hash)).ok()?;
        let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        let obj = v.as_object()?;
        let registry = obj.keys().cloned().collect();
        let mut events = HashMap::new();
        for (event, def) in obj {
            let Some(list) = def.get("sounds").and_then(serde_json::Value::as_array) else {
                continue;
            };
            let mut files = Vec::new();
            for s in list {
                match s {
                    serde_json::Value::String(name) => files.push(name.clone()),
                    serde_json::Value::Object(o) => {
                        // Skip references to *other events* (`type: "event"`); we
                        // only want concrete sound files here.
                        if o.get("type").and_then(serde_json::Value::as_str) == Some("event") {
                            continue;
                        }
                        if let Some(name) = o.get("name").and_then(serde_json::Value::as_str) {
                            files.push(name.to_string());
                        }
                    }
                    _ => {}
                }
            }
            if !files.is_empty() {
                events.insert(event.clone(), files);
            }
        }
        Some(Self { events, registry })
    }

    /// Number of known events.
    #[must_use]
    pub fn len(&self) -> usize {
        self.events.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Picks one sound file for an event, cycling through variants like vanilla's
    /// random choice. Returns `None` for unknown events.
    #[must_use]
    pub fn pick(&self, event: &str) -> Option<&str> {
        let files = self.events.get(event)?;
        match files.len() {
            0 => None,
            1 => Some(files[0].as_str()),
            n => {
                static NEXT: AtomicUsize = AtomicUsize::new(0);
                let i = NEXT.fetch_add(1, Ordering::Relaxed) % n;
                Some(files[i].as_str())
            }
        }
    }

    /// Resolves a one-based 1.20.1 sound registry holder id.
    #[must_use]
    pub fn protocol_event(&self, id: i32) -> Option<&str> {
        usize::try_from(id - 1)
            .ok()
            .and_then(|index| self.registry.get(index))
            .map(String::as_str)
    }
}

/// Decodes an OGG to its total sample count (verification; needs no device).
#[must_use]
pub fn ogg_sample_count(ogg: &[u8]) -> Option<usize> {
    let decoder = rodio::Decoder::new(Cursor::new(ogg.to_vec())).ok()?;
    Some(decoder.count())
}

/// The vanilla `SoundType` group for a block, named to match the `sounds.json`
/// event groups (`block.<group>.{break,step,place,hit}`). A keyword heuristic
/// over block names covering the common groups; falls back to `"stone"`.
///
/// This is the data Minecraft hardcodes in `SoundType` (not present in the
/// generated data reports), so we approximate it by name. Pairing it with the
/// real `sounds.json` ([`Sounds`]) yields the exact files the game would play.
#[must_use]
pub fn sound_group(block_name: &str) -> &'static str {
    let n = block_name.strip_prefix("minecraft:").unwrap_or(block_name);
    let has = |kws: &[&str]| kws.iter().any(|k| n.contains(k));
    let wood_shape = |x: &str| {
        x.contains("planks")
            || x.contains("log")
            || x.contains("wood")
            || x.contains("stem")
            || x.contains("hyphae")
            || x.contains("fence")
            || x.contains("door")
            || x.contains("sign")
            || x.contains("stairs")
            || x.contains("slab")
            || x.contains("button")
            || x.contains("pressure_plate")
            || x.contains("trapdoor")
    };

    // Glass / wool first (distinctive break sounds).
    if n.contains("glass") || n.contains("beacon") {
        return "glass";
    }
    if n.contains("wool") || n.contains("carpet") {
        return "wool";
    }

    // Stone-family specials (before the generic "stone" fallback).
    if n.contains("amethyst") {
        return "amethyst_block";
    }
    if n.contains("deepslate") {
        if n.contains("tiles") {
            return "deepslate_tiles";
        }
        if n.contains("brick") {
            return "deepslate_bricks";
        }
        if n.contains("polished") || n.contains("chiseled") {
            return "polished_deepslate";
        }
        return "deepslate";
    }
    if n.contains("copper") {
        return "copper";
    }
    if n.contains("calcite") {
        return "calcite";
    }
    if n.contains("tuff") {
        return "tuff";
    }
    if n.contains("basalt") {
        return "basalt";
    }
    if n.contains("netherite_block") {
        return "netherite_block";
    }
    if n.contains("ancient_debris") {
        return "ancient_debris";
    }
    if n.contains("lodestone") {
        return "lodestone";
    }
    if n.contains("nether_brick") {
        return "nether_bricks";
    }
    if n.contains("netherrack") || n.contains("nether_gold_ore") || n.contains("nether_quartz") {
        return "netherrack";
    }
    if n.contains("nylium") {
        return "nylium";
    }
    if n.contains("soul_sand") {
        return "soul_sand";
    }
    if n.contains("soul_soil") {
        return "soul_soil";
    }
    if n.contains("bone_block") {
        return "bone_block";
    }
    if n.contains("honey_block") {
        return "honey_block";
    }
    if n.contains("slime_block") {
        return "slime_block";
    }
    if n.contains("nether_wart_block") || n == "shroomlight" {
        return "wart_block";
    }
    if n.contains("froglight") {
        return "froglight";
    }
    if n.contains("sculk") {
        return "sculk";
    }

    // Metal blocks/items.
    if has(&[
        "iron_block",
        "gold_block",
        "diamond_block",
        "emerald_block",
        "raw_iron",
        "raw_gold",
        "raw_copper",
        "iron_door",
        "iron_trapdoor",
        "iron_bars",
        "anvil",
        "hopper",
        "cauldron",
        "bell",
        "chain",
        "heavy_weighted",
        "light_weighted",
    ]) {
        return "metal";
    }
    if n.contains("lantern") {
        return "lantern";
    }
    if n == "ladder" {
        return "ladder";
    }
    if n.contains("scaffolding") {
        return "scaffolding";
    }

    // Plants / organics.
    if n.contains("bamboo") {
        return if wood_shape(n) || n.contains("mosaic") {
            "bamboo_wood"
        } else {
            "bamboo"
        };
    }
    if n.contains("moss") {
        return "moss";
    }
    if n.contains("mud_brick") {
        return "mud_bricks";
    }
    if n.contains("packed_mud") {
        return "packed_mud";
    }
    if n == "mud" || n.contains("muddy_mangrove") {
        return "mud";
    }
    if has(&["roots", "weeping_vines", "twisting_vines", "nether_sprouts"]) {
        return "roots";
    }
    if n.contains("fungus") {
        return "fungus";
    }
    if n.contains("sweet_berry") {
        return "sweet_berry_bush";
    }
    if n.contains("nether_wart") {
        return "nether_wart";
    }
    if has(&["wheat", "carrots", "potatoes", "beetroots"]) {
        return "crop";
    }
    if n.ends_with("_stem") {
        return "stem";
    }
    if n.contains("lily_pad") || n.contains("kelp") || n.contains("seagrass") {
        return "wet_grass";
    }

    // Wood (catch crimson/warped/cherry first so they get their own events).
    if (n.contains("crimson") || n.contains("warped")) && wood_shape(n) {
        return "nether_wood";
    }
    if n.contains("cherry") && wood_shape(n) {
        return "cherry_wood";
    }
    if wood_shape(n)
        || has(&[
            "crafting_table",
            "bookshelf",
            "chest",
            "barrel",
            "jukebox",
            "note_block",
            "loom",
            "beehive",
            "bee_nest",
            "composter",
            "cartography",
            "fletching",
            "smithing_table",
            "lectern",
            "sapling",
            "bamboo",
        ])
    {
        return "wood";
    }

    // Loose granular / soft ground.
    if n.contains("sand") && !n.contains("sandstone") {
        return "sand";
    }
    if has(&[
        "gravel", "dirt", "clay", "podzol", "mycelium", "farmland", "rooted",
    ]) {
        return "gravel";
    }
    if n.contains("powder_snow") {
        return "powder_snow";
    }
    if n.contains("snow") {
        return "snow";
    }

    // Foliage / grass.
    if has(&[
        "grass",
        "leaves",
        "fern",
        "flower",
        "vine",
        "mushroom",
        "azalea",
        "sugar_cane",
        "hanging_roots",
        "spore_blossom",
        "pink_petals",
        "hay_block",
    ]) {
        return "grass";
    }

    "stone"
}

/// The block-break sound **event** (e.g. `block.grass.break`).
#[must_use]
pub fn break_event(block_name: &str) -> String {
    format!("block.{}.break", sound_group(block_name))
}

/// The block-place sound **event** (e.g. `block.grass.place`).
#[must_use]
pub fn place_event(block_name: &str) -> String {
    format!("block.{}.place", sound_group(block_name))
}

/// The block-mining "hit" sound **event** played repeatedly while digging
/// (quieter than the break sound — vanilla resolves it to the `step/*` files).
#[must_use]
pub fn hit_event(block_name: &str) -> String {
    format!("block.{}.hit", sound_group(block_name))
}

/// The footstep sound **event** for the block walked on (e.g. `block.grass.step`).
#[must_use]
pub fn step_event(block_name: &str) -> String {
    format!("block.{}.step", sound_group(block_name))
}

/// The player-hurt sound event.
#[must_use]
pub fn hurt_event() -> &'static str {
    "entity.player.hurt"
}

/// The player melee-attack sound event.
#[must_use]
pub fn attack_event() -> &'static str {
    "entity.player.attack.weak"
}

/// The sound event played when the local player collects an item entity.
#[must_use]
pub fn pickup_event() -> &'static str {
    "entity.item.pickup"
}

/// Fire-and-forget OGG playback. A no-op (but still decode-checkable) when no
/// audio output device is available.
pub struct SoundPlayer {
    // Field order matters: the stream must outlive the handle.
    handle: Option<rodio::OutputStreamHandle>,
    _stream: Option<rodio::OutputStream>,
}

impl SoundPlayer {
    /// Opens the default audio output, or a silent player if none is available.
    #[must_use]
    pub fn new() -> Self {
        match rodio::OutputStream::try_default() {
            Ok((stream, handle)) => Self {
                handle: Some(handle),
                _stream: Some(stream),
            },
            Err(e) => {
                tracing::warn!("no audio output ({e}); sounds disabled");
                Self {
                    handle: None,
                    _stream: None,
                }
            }
        }
    }

    /// Whether a real output device is driving playback.
    #[must_use]
    pub fn available(&self) -> bool {
        self.handle.is_some()
    }

    /// Decodes and plays an OGG; returns whether it decoded successfully.
    /// (Plays only if an output device exists, but always validates decoding.)
    pub fn play_ogg(&self, ogg: Vec<u8>) -> bool {
        self.play_ogg_volume(ogg, 1.0)
    }

    /// Decodes and plays an OGG with a linear gain multiplier.
    pub fn play_ogg_volume(&self, ogg: Vec<u8>, volume: f32) -> bool {
        match &self.handle {
            Some(handle) => match rodio::Decoder::new(Cursor::new(ogg)) {
                Ok(source) => {
                    use rodio::Source;
                    let _ = handle.play_raw(source.amplify(volume.max(0.0)).convert_samples());
                    true
                }
                Err(_) => false,
            },
            None => rodio::Decoder::new(Cursor::new(ogg)).is_ok(),
        }
    }
}

impl Default for SoundPlayer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sound_group_picks_material() {
        assert_eq!(sound_group("minecraft:grass_block"), "grass");
        assert_eq!(sound_group("minecraft:oak_log"), "wood");
        assert_eq!(sound_group("minecraft:oak_planks"), "wood");
        assert_eq!(sound_group("minecraft:crimson_planks"), "nether_wood");
        assert_eq!(sound_group("minecraft:cherry_fence"), "cherry_wood");
        assert_eq!(sound_group("minecraft:sand"), "sand");
        assert_eq!(sound_group("minecraft:red_sand"), "sand");
        assert_eq!(sound_group("minecraft:sandstone"), "stone");
        assert_eq!(sound_group("minecraft:gravel"), "gravel");
        assert_eq!(sound_group("minecraft:dirt"), "gravel");
        assert_eq!(sound_group("minecraft:stone"), "stone");
        assert_eq!(sound_group("minecraft:cobblestone"), "stone");
        assert_eq!(sound_group("minecraft:glass"), "glass");
        assert_eq!(sound_group("minecraft:white_wool"), "wool");
        assert_eq!(sound_group("minecraft:iron_block"), "metal");
        assert_eq!(sound_group("minecraft:deepslate"), "deepslate");
        assert_eq!(
            sound_group("minecraft:deepslate_bricks"),
            "deepslate_bricks"
        );
        assert_eq!(sound_group("minecraft:oak_leaves"), "grass");
    }

    #[test]
    fn events_are_namespaced() {
        assert_eq!(break_event("minecraft:stone"), "block.stone.break");
        assert_eq!(hit_event("minecraft:oak_log"), "block.wood.hit");
        assert_eq!(place_event("minecraft:sand"), "block.sand.place");
        assert_eq!(step_event("minecraft:grass_block"), "block.grass.step");
        assert_eq!(pickup_event(), "entity.item.pickup");
    }

    #[test]
    fn sounds_json_resolves_events() {
        // A tiny inline sounds.json: string + object + event-ref forms.
        let json = r#"{
            "block.stone.break": { "sounds": ["dig/stone1", "dig/stone2"] },
            "block.stone.hit": { "sounds": [ {"name": "step/stone1"}, {"name": "x", "type": "event"} ] }
        }"#;
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        let mut events = HashMap::new();
        for (event, def) in v.as_object().unwrap() {
            let mut files = Vec::new();
            for s in def["sounds"].as_array().unwrap() {
                match s {
                    serde_json::Value::String(name) => files.push(name.clone()),
                    serde_json::Value::Object(o) => {
                        if o.get("type").and_then(serde_json::Value::as_str) == Some("event") {
                            continue;
                        }
                        if let Some(name) = o.get("name").and_then(serde_json::Value::as_str) {
                            files.push(name.to_string());
                        }
                    }
                    _ => {}
                }
            }
            events.insert(event.clone(), files);
        }
        let sounds = Sounds {
            registry: vec!["block.stone.break".into(), "block.stone.hit".into()],
            events,
        };
        assert!(sounds
            .pick("block.stone.break")
            .is_some_and(|f| f.starts_with("dig/stone")));
        // The `type: "event"` ref was skipped, leaving exactly one file.
        assert_eq!(sounds.pick("block.stone.hit"), Some("step/stone1"));
        assert_eq!(sounds.pick("block.unknown.break"), None);
        assert_eq!(sounds.protocol_event(1), Some("block.stone.break"));
        assert_eq!(sounds.protocol_event(2), Some("block.stone.hit"));
    }
}
